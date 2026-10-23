// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Max's wrapped-work publication transaction, owned by the common Session.
//! Memory/VM and client leases are the existing native owners. This layer does
//! not schedule CPU jobs or resolve UAPI dependencies; it emits their wire plan.

use super::{
    g17p_compute_runtime::ClientLease,
    g17p_memory::{self, Memory},
    g17p_queue as q,
    g17p_vm::Vm,
    scheduled_initdata::Channel,
    scheduled_queues::LaneG16C,
};
use kernel::prelude::*;

#[derive(Clone, Copy)]
pub(crate) struct Route {
    pub(crate) read: u64,
    pub(crate) done: u64,
    pub(crate) producer: u64,
    pub(crate) ring: u64,
}
impl Route {
    pub(crate) fn decode(channel: &Channel) -> Result<Self> {
        let [read, done, producer] = channel.states.map(u64::from_le_bytes);
        let route = Self {
            read,
            done,
            producer,
            ring: u64::from_le_bytes(channel.ring),
        };
        route.validate()?;
        Ok(route)
    }
    fn validate(&self) -> Result {
        if [self.read, self.done, self.producer, self.ring]
            .iter()
            .any(|&v| v == 0 || v & 3 != 0)
        {
            return Err(EINVAL);
        }
        self.ring.checked_add(256 * 24).ok_or(EINVAL)?;
        for a in [self.read, self.done, self.producer] {
            a.checked_add(4).ok_or(EINVAL)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Stamp {
    pub(crate) address: u64,
    pub(crate) value: u32,
}
impl Stamp {
    fn validate(&self) -> Result {
        if self.address == 0 || self.address & 3 != 0 || self.address.checked_add(4).is_none() {
            return Err(EINVAL);
        }
        Ok(())
    }
}
pub(crate) struct Requirements {
    pub(crate) firmware: KVec<Stamp>,
    pub(crate) driver: KVec<Stamp>,
    pub(crate) notifiers: KVec<u64>,
    pub(crate) drains: KVec<Route>,
    pub(crate) counters: KVec<Stamp>,
}
impl Requirements {
    fn stamps(&self) -> impl Iterator<Item = &Stamp> {
        Iterator::chain(
            Iterator::chain(self.firmware.iter(), self.driver.iter()),
            self.counters.iter(),
        )
    }

    fn validate(&self) -> Result {
        if self.driver.is_empty()
            || (self.notifiers.is_empty() && self.counters.is_empty())
            || (self.firmware.is_empty() && self.counters.is_empty())
        {
            return Err(EINVAL);
        }
        for s in self.stamps() {
            s.validate()?;
        }
        for s in &self.counters {
            if s.value == 0 {
                return Err(EINVAL);
            }
        }
        for &a in &self.notifiers {
            Stamp {
                address: a,
                value: 0,
            }
            .validate()?;
        }
        for r in &self.drains {
            r.validate()?;
        }
        Ok(())
    }
}

pub(crate) struct Request<'a> {
    pub(crate) lane: &'a mut LaneG16C,
    pub(crate) works: &'a [u64],
    pub(crate) event: u8,
    pub(crate) tag: u64,
}

pub(crate) fn stamp_reached(current: u32, target: u32) -> bool {
    current.wrapping_sub(target) < 0x80000000
}
pub(crate) fn ring_reached(current: u32, target: u32, capacity: u32) -> bool {
    capacity >= 4
        && current < capacity
        && target < capacity
        && (current + capacity - target) % capacity < capacity / 2
}
pub(crate) fn ring_room(tail: u32, read: u32, done: u32, entries: usize, capacity: u32) -> bool {
    capacity >= 4
        && capacity <= 0x10000
        && capacity % 2 == 0
        && [tail, read, done].iter().all(|&v| v < capacity)
        && entries > 0
        && entries < (capacity / 2) as usize
        && [read, done]
            .iter()
            .all(|&v| (tail + capacity - v) % capacity + (entries as u32) < capacity / 2)
}

#[derive(Clone, Copy)]
struct QueueTarget {
    queue: u64,
    pointers: u64,
    capacity: u32,
    head: u32,
}
#[derive(Clone, Copy)]
struct ChannelTarget {
    index: usize,
    head: u32,
}
struct Flight {
    id: u64,
    requirements: Requirements,
    queues: KVec<QueueTarget>,
    channels: KVec<ChannelTarget>,
    // Pins executable roots, GEM identities/backing and their cache owners.
    _owners: KVec<ClientLease>,
    completed: bool,
    quiescent: bool,
    error: Option<Error>,
}
struct Plan {
    request: usize,
    index: usize,
    tail: u32,
    head: u32,
    epoch: u32,
}

/// Prepared host publication plan. The transport already owns its Flight and
/// leases; no allocation is permitted after the engines are armed.
pub(crate) struct PreparedG16C {
    id: u64,
    plans: KVec<Plan>,
}

pub(crate) struct StateG16C {
    routes: [Route; 12],
    flights: KVec<Flight>,
    next: u64,
    poisoned: bool,
    allocation_faulted: bool,
}
fn read(memory: &Memory, vm: &Vm, address: u64) -> Result<u32> {
    memory.read_firmware32(vm.physical(memory, 2, address)?)
}
fn poll(
    memory: &Memory,
    vm: &Vm,
    routes: &[Route; 12],
    poisoned: bool,
    f: &mut Flight,
) -> Result<bool> {
    if f.quiescent {
        return Ok(true);
    }
    if let Some(e) = f.error {
        return Err(e);
    }
    if poisoned {
        f.error = Some(EIO);
        return Err(EIO);
    }
    f.completed = true;
    for s in f.requirements.stamps() {
        if !stamp_reached(read(memory, vm, s.address)?, s.value) {
            f.completed = false;
            return Ok(false);
        }
    }
    if !f.completed {
        return Ok(false);
    }
    let mut idle = true;
    for &a in &f.requirements.notifiers {
        idle &= read(memory, vm, a)? == 0;
    }
    for q in &f.queues {
        for a in [q.pointers, q.pointers + 0x30] {
            idle &= ring_reached(read(memory, vm, a)?, q.head, q.capacity);
        }
    }
    for c in &f.channels {
        let r = routes[c.index];
        for a in [r.read, r.done] {
            idle &= ring_reached(read(memory, vm, a)?, c.head, 256);
        }
    }
    for r in &f.requirements.drains {
        let tail = read(memory, vm, r.producer)?;
        idle &= read(memory, vm, r.read)? == tail && read(memory, vm, r.done)? == tail;
    }
    f.quiescent = idle;
    Ok(idle)
}

impl StateG16C {
    pub(crate) fn new(channels: &[Channel]) -> Result<Self> {
        if channels.len() != 12 {
            return Err(EINVAL);
        }
        let mut routes = [Route {
            read: 0,
            done: 0,
            producer: 0,
            ring: 0,
        }; 12];
        for (out, input) in routes.iter_mut().zip(channels) {
            *out = Route::decode(input)?;
        }
        Ok(Self {
            routes,
            flights: KVec::new(),
            next: 1,
            poisoned: false,
            allocation_faulted: false,
        })
    }
    pub(crate) fn poisoned(&self) -> bool {
        self.poisoned
    }
    pub(crate) fn retains_owner(&self, owner: (u64, u32)) -> bool {
        self.flights.iter().any(|flight|
            flight._owners.iter().any(|lease| lease.owner == owner))
    }

    pub(crate) fn submit(
        &mut self,
        memory: &mut Memory,
        vm: &Vm,
        requests: &mut [Request<'_>],
        requirements: Requirements,
        owners: KVec<ClientLease>,
        send: impl FnMut(u8, u64) -> Result,
    ) -> Result<u64> {
        let prepared = self.prepare(memory, vm, requests, requirements, owners)?;
        self.publish(memory, vm, requests, prepared, send)
    }

    pub(crate) fn prepare(
        &mut self, memory: &mut Memory, vm: &Vm, requests: &mut [Request<'_>],
        requirements: Requirements, owners: KVec<ClientLease>,
    ) -> Result<PreparedG16C> {
        vm.require_g16c_graph()?;
        if self.poisoned {
            return Err(EIO);
        }
        if requests.is_empty() || requests.len() > 12 || owners.is_empty() {
            return Err(EINVAL);
        }
        requirements.validate()?;
        let mut seen = 0u16;
        for request in requests.iter() {
            let l = &request.lane;
            if l.priority >= 4
                || l.stage >= 3
                || l.capacity < 4
                || l.capacity > 0x10000
                || l.capacity % 2 != 0
                || l.head >= l.capacity
                || request.works.is_empty()
                || [l.queue, l.pointers, l.ring]
                    .iter()
                    .any(|&a| a == 0 || a & 7 != 0)
                || request.works.iter().any(|&a| a == 0 || a & 3 != 0)
            {
                return Err(EINVAL);
            }
            l.pointers.checked_add(0x44).ok_or(EINVAL)?;
            l.ring.checked_add(l.capacity as u64 * 8).ok_or(EINVAL)?;
            let bit = 1u16 << (l.priority * 3 + l.stage);
            if seen & bit != 0 {
                return Err(EINVAL);
            }
            seen |= bit;
        }
        for s in requirements.stamps() {
            if stamp_reached(read(memory, vm, s.address)?, s.value) {
                return Err(EINVAL);
            }
        }
        for old in &mut self.flights {
            if poll(memory, vm, &self.routes, false, old)? {
                continue;
            }
            for previous in old.requirements.stamps() {
                for s in requirements.stamps() {
                    if s.address == previous.address
                        && (s.value == previous.value || !stamp_reached(s.value, previous.value))
                    {
                        return Err(EINVAL);
                    }
                }
            }
        }
        let mut plans = KVec::with_capacity(requests.len(), GFP_KERNEL)?;
        let mut queues = KVec::with_capacity(requests.len(), GFP_KERNEL)?;
        let mut channels = KVec::with_capacity(requests.len(), GFP_KERNEL)?;
        for (ri, request) in requests.iter().enumerate() {
            let l = &request.lane;
            let index = (l.priority * 3 + l.stage) as usize;
            let r = self.routes[index];
            let cr = read(memory, vm, l.pointers)?;
            let cd = read(memory, vm, l.pointers + 0x30)?;
            if read(memory, vm, l.pointers + 0x40)? != l.head {
                return Err(EIO);
            }
            if !ring_room(l.head, cr, cd, request.works.len(), l.capacity) {
                return Err(EBUSY);
            }
            let tail = read(memory, vm, r.producer)?;
            if !ring_room(
                tail,
                read(memory, vm, r.read)?,
                read(memory, vm, r.done)?,
                1,
                256,
            ) {
                return Err(EBUSY);
            }
            let head = (l.head + request.works.len() as u32) % l.capacity;
            let epoch = (tail + 1) % 256;
            for old in &self.flights {
                if old.quiescent {
                    continue;
                }
                for q in &old.queues {
                    if q.queue == l.queue && !ring_reached(head, q.head, l.capacity) {
                        return Err(EBUSY);
                    }
                }
                for c in &old.channels {
                    if c.index == index && !ring_reached(epoch, c.head, 256) {
                        return Err(EBUSY);
                    }
                }
            }
            plans.push(
                Plan {
                    request: ri,
                    index,
                    tail,
                    head,
                    epoch,
                },
                GFP_KERNEL,
            )?;
            queues.push(
                QueueTarget {
                    queue: l.queue,
                    pointers: l.pointers,
                    capacity: l.capacity,
                    head,
                },
                GFP_KERNEL,
            )?;
            channels.push(ChannelTarget { index, head: epoch }, GFP_KERNEL)?;
        }
        if *crate::module_parameters::g16c_initial_alloc_fail.value() == 9 && !self.allocation_faulted {
            self.allocation_faulted = true;
            kernel::pr_info!("G16C: injected unpublished transport ENOMEM before flight ownership/producers\n");
            return Err(ENOMEM);
        }
        let id = self.next;
        let next = id.checked_add(1).ok_or(EINVAL)?;
        // Retain everything before the first visible store. A send/write may
        // have reached hardware even when it subsequently reports an error.
        self.flights.push(
            Flight {
                id,
                requirements,
                queues,
                channels,
                _owners: owners,
                completed: false,
                quiescent: false,
                error: None,
            },
            GFP_KERNEL,
        )?;
        self.next = next;
        Ok(PreparedG16C { id, plans })
    }

    pub(crate) fn publish(
        &mut self, memory: &mut Memory, vm: &Vm, requests: &mut [Request<'_>],
        prepared: PreparedG16C, mut send: impl FnMut(u8, u64) -> Result,
    ) -> Result<u64> {
        let PreparedG16C { id, plans } = prepared;
        if self.poisoned || self.flights.last().is_none_or(|flight| flight.id != id)
            || plans.len() != requests.len() {
            self.poisoned = true;
            return Err(EIO);
        }
        let result = (|| -> Result {
            for stage in [1, 0, 2] {
                for p in &plans {
                    let request = &requests[p.request];
                    let l = &request.lane;
                    if l.stage != stage {
                        continue;
                    }
                    for (slot, address) in request.works.iter().enumerate() {
                        let at = l.ring + ((l.head + slot as u32) % l.capacity) as u64 * 8;
                        vm.write(memory, 2, at, &address.to_le_bytes())?;
                    }
                    vm.write(memory, 2, l.pointers + 0x40, &p.head.to_le_bytes())?;
                    let kind = match l.stage {
                        0 => q::Kind::Tiling,
                        1 => q::Kind::Fragment,
                        _ => q::Kind::Compute,
                    };
                    let body = q::ring_slot_tag(
                        request.tag,
                        l.queue,
                        p.head,
                        request.event as u32,
                        l.first,
                        kind,
                    );
                    vm.write(
                        memory,
                        2,
                        self.routes[p.index].ring + p.tail as u64 * 24,
                        &body,
                    )?;
                }
            }
            g17p_memory::sync();
            for stage in [1, 0, 2] {
                for p in &plans {
                    if requests[p.request].lane.stage == stage {
                        vm.write(
                            memory,
                            2,
                            self.routes[p.index].producer,
                            &p.epoch.to_le_bytes(),
                        )?;
                    }
                }
            }
            g17p_memory::sync();
            for stage in [1, 0, 2] {
                for p in &plans {
                    let l = &mut requests[p.request].lane;
                    if l.stage == stage {
                        send(
                            0x21,
                            0x0083000000000000 | ((l.priority as u64) << 2) | l.stage as u64,
                        )?;
                        l.head = p.head;
                        l.first = false;
                    }
                }
            }
            send(0x21, 0x0083000000000010)
        })();
        if let Err(e) = result {
            self.poisoned = true;
            self.flights.last_mut().ok_or(EIO)?.error = Some(e);
            return Err(e);
        }
        Ok(id)
    }

    pub(crate) fn poll(&mut self, memory: &Memory, vm: &Vm, id: u64) -> Result<bool> {
        let f = self.flights.iter_mut().find(|f| f.id == id).ok_or(EINVAL)?;
        poll(memory, vm, &self.routes, self.poisoned, f)
    }
    pub(crate) fn release(&mut self, memory: &Memory, vm: &Vm, id: u64) -> Result {
        if !self.poll(memory, vm, id)? {
            return Err(EBUSY);
        }
        let i = self.flights.iter().position(|f| f.id == id).ok_or(EINVAL)?;
        drop(self.flights.remove(i).map_err(|_| EIO)?);
        Ok(())
    }
}
