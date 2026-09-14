// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from Niklas Sheth's linux-m4-integration prototype.

//! G16 ring publication and independent firmware retirement.

use super::Bootstrap;
use crate::{g16_compute as compute, g16_fw, g16_render as render, g16_vm};
use core::sync::atomic::{AtomicU64, Ordering};
use kernel::time::{Instant, Monotonic};
use kernel::{
    c_str, io,
    prelude::*,
    sync::{Arc, Mutex},
};

/// One userspace queue's three firmware workqueues. Channel rings remain
/// device-wide transports; their messages select these independent queues.
/// Firmware-owned allocations are retained by Bootstrap until device reset.
pub(crate) struct FirmwareQueues {
    lanes: Option<[Lane; 3]>,
    priority: u32,
    events: Option<QueueEvents>,
    pending: usize,
    render_count: u64,
    compute_count: u64,
    pub(crate) last: [Option<Stamp>; 2],
}

#[derive(Clone, Copy)]
struct Lane {
    queue: u64,
    pointers: u64,
    ring: u64,
    head: u32,
    new: bool,
}

/// Reserve three distinct event IDs while this queue has unfinished work.
/// Idle queues release their lease, so creating idle queues does not consume
/// firmware event slots. Slots 0..31 are left to the bootstrap protocol.
struct QueueEvents {
    slots: Arc<AtomicU64>,
    index: u32,
}

impl Drop for QueueEvents {
    fn drop(&mut self) {
        self.slots.fetch_and(!(1 << self.index), Ordering::AcqRel);
    }
}

impl FirmwareQueues {
    pub(crate) fn new(priority: u32) -> Self {
        Self {
            lanes: None,
            priority,
            events: None,
            pending: 0,
            render_count: 0,
            compute_count: 0,
            last: [None; 2],
        }
    }

    fn event(&self, stage: u32) -> Result<u32> {
        Ok(32 + self.events.as_ref().ok_or(EIO)?.index * 3 + stage)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Stamp {
    pub(crate) address: u64,
    pub(crate) value: u32,
    pub(crate) event: u32,
}
#[derive(Clone, Copy)]
pub(crate) struct Receipt {
    pub(crate) id: u64,
    pub(crate) stamp: Stamp,
}
struct RenderFlight {
    addresses: render::Addresses,
    client: Arc<Mutex<g16_vm::AddressSpace>>,
    root: u64,
    events: RenderEvents,
    scene: u32,
}
pub(super) struct Flight {
    id: u64,
    owner: Arc<Mutex<FirmwareQueues>>,
    context: u32,
    started: Instant<Monotonic>,
    stamps: [Option<Stamp>; 2],
    queues: [(u64, u32); 2],
    channels: [(usize, u32); 2],
    render: Option<RenderFlight>,
}

/// Ring distance is unambiguous because admission keeps less than half a ring
/// outstanding. Equality alone would lose completions when firmware advances
/// past several commands between notifications, including over the wrap boundary.
fn reached(cursor: u32, target: u32, capacity: u32) -> bool {
    cursor < capacity && target < capacity && (cursor + capacity - target) % capacity < capacity / 2
}

/// Both firmware cursors must leave enough space for the entire publication.
/// The half-ring bound also makes completion comparisons unambiguous.
fn ring_room(tail: u32, read: u32, done: u32, entries: u32, capacity: u32) -> bool {
    tail < capacity
        && read < capacity
        && done < capacity
        && entries < capacity / 2
        && [read, done]
            .iter()
            .all(|&cursor| (tail + capacity - cursor) % capacity + entries < capacity / 2)
}

impl Bootstrap {
    /// Report each exhausted resource once, without logging every retry or
    /// enabling firmware KTrace (which changes command execution timing).
    fn report_pressure(&self, bit: u64, resource: &str) {
        if *crate::module_parameters::fw_trace.value() & 2 != 0
            && self.pressure_reported.fetch_or(bit, Ordering::Relaxed) & bit == 0
        {
            dev_info!(
                self.dev.as_ref(),
                "G16: backpressure {} outstanding={}\n",
                resource,
                self.flights.len()
            );
        }
    }

    fn prepare_queues(&mut self, owner: &mut FirmwareQueues) -> Result {
        use crate::pgtable::prot::{PROT_FW_PRIV_RW, PROT_FW_SHARED_RW};
        use g16_fw::queue as q;

        if owner.lanes.is_none() {
            let base = self.next_work_va;
            self.next_work_va = base.checked_add(0x24000).ok_or(ENOSPC)?;
            let lanes = core::array::from_fn(|stage| {
                let queue = base + stage as u64 * 0xc000;
                Lane {
                    queue,
                    pointers: queue + 0x4000,
                    ring: queue + 0x8000,
                    head: 0,
                    new: true,
                }
            });
            let fw = self._firmware_space.as_mut().ok_or(EIO)?;
            // A notifier list and GPU context belong to the public queue,
            // shared only by its own vertex, fragment and compute lanes.
            let jobs = base + 0x4100;
            let context = base + 0x4140;
            for lane in &lanes {
                fw.alloc(lane.queue, 0x4000, PROT_FW_PRIV_RW)?;
                fw.alloc(lane.pointers, 0x8000, PROT_FW_SHARED_RW)?;
                let mut descriptor = q::Queue {
                    pointers: lane.pointers,
                    ring: lane.ring,
                    jobs,
                    private: lane.queue + 0xb0,
                    context,
                    uuid: ((lane.queue - 0xffff_fc22_0000_0000) >> 14) as u32 + 2,
                }
                .encode_priority(owner.priority)
                .ok_or(EINVAL)?;
                // Run an active Work to completion on each engine. The
                // preempting profile lost compute updates and, with mixed
                // engines, a rendered image. Engines still overlap, and TA
                // pipelines ahead of fragment; priorities select channels.
                descriptor[0x30..0x34].copy_from_slice(&0u32.to_le_bytes());
                descriptor[0x48..0x4c].copy_from_slice(&1u32.to_le_bytes());
                fw.write(lane.queue, &descriptor)?;
                fw.write(lane.pointers, &q::pointers(0x500, 0))?;
            }
            fw.write(jobs, &q::jobs(jobs))?;
            fw.write(context, &q::context())?;
            fw.sync();
            crate::mem::tlbi_all();
            crate::mem::sync();
            owner.lanes = Some(lanes);
            if *crate::module_parameters::fw_trace.value() & 1 != 0 {
                dev_info!(
                    self.dev.as_ref(),
                    "G16: queues vertex={:#x} fragment={:#x} compute={:#x} priority={}\n",
                    lanes[0].queue,
                    lanes[1].queue,
                    lanes[2].queue,
                    owner.priority
                );
            }
        }
        if owner.events.is_none() {
            let previous = self
                .queue_events
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |mask| {
                    let free = !mask & 0xffff_ffff;
                    (free != 0).then(|| mask | (1 << free.trailing_zeros()))
                })
                .map_err(|_| {
                    self.report_pressure(8, "events");
                    EBUSY
                })?;
            owner.events = Some(QueueEvents {
                slots: self.queue_events.clone(),
                index: (!previous & 0xffff_ffff).trailing_zeros(),
            });
        }
        Ok(())
    }

    fn reserve_flight(
        &mut self,
        queue: &FirmwareQueues,
        render: bool,
        root: Option<u64>,
    ) -> Result {
        if self.render_failed {
            return Err(EIO);
        }
        // Check scarce resources before allocating any per-Work storage.
        // A live render VM can share its ASID; compute uses a private root.
        if !(1..64).any(|i| {
            self.context_users[i] == 0
                || root.is_some_and(|r| self.context_roots[i].is_some_and(|s| s.low == r))
        }) {
            self.report_pressure(1, "ASIDs");
            return Err(EBUSY);
        }
        let stages: &[usize] = if render { &[0, 1] } else { &[2] };
        let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
        let guard = self._sgx.try_access().ok_or(ENODEV)?;
        for &stage in stages {
            let channel = queue.priority as usize * 3 + stage;
            let route = &config.channels[channel];
            let read =
                guard.try_read32(0xd64000 + (route.state[0] - 0xffff_fc20_0002_8000) as usize)?;
            let done =
                guard.try_read32(0xd64000 + (route.state[1] - 0xffff_fc20_0002_8000) as usize)?;
            let tail =
                guard.try_read32(0xd60000 + (route.state[2] - 0xffff_fc20_0002_0000) as usize)?;
            if read >= 256 || done >= 256 || tail >= 256 {
                return Err(EIO);
            }
            if !ring_room(tail, read, done, 1, 256) {
                self.report_pressure(2, "channel");
                return Err(EBUSY);
            }
            // Keep even already-consumed, unretired targets within the
            // half-ring window used by reached(). Notification can coalesce.
            if self
                .flights
                .iter()
                .filter(|f| f.channels.iter().any(|&(c, _)| c == channel))
                .count()
                >= 127
            {
                self.report_pressure(2, "channel");
                return Err(EBUSY);
            }
            if let Some(lanes) = &queue.lanes {
                let lane = lanes[stage];
                let fw = self._firmware_space.as_ref().ok_or(EIO)?;
                let entries = if render { 2 } else { 1 };
                let read = fw.read_u32(lane.pointers)?;
                let done = fw.read_u32(lane.pointers + 0x30)?;
                if read >= 0x500 || done >= 0x500 || lane.head >= 0x500 {
                    return Err(EIO);
                }
                if !ring_room(lane.head, read, done, entries, 0x500) {
                    self.report_pressure(16, "workqueue");
                    return Err(EBUSY);
                }
            }
        }
        self.flights.reserve(1, GFP_KERNEL)?;
        Ok(())
    }

    pub(crate) fn dependencies_done(&self, dependencies: &[Option<Stamp>; 2]) -> Result<bool> {
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        for stamp in dependencies.iter().flatten() {
            if fw.read_u32(stamp.address)? != stamp.value {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn poison(&mut self) {
        self.render_failed = true;
    }

    fn stamps_done(&self, flight: &Flight) -> Result<bool> {
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        for stamp in flight.stamps.iter().flatten() {
            if fw.read_u32(stamp.address)? != stamp.value {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn trace_publication(&self, kind: &str, id: u64) -> Result {
        if *crate::module_parameters::fw_trace.value() & 1 != 0 {
            let mut unretired = 0;
            for flight in &self.flights {
                if !self.stamps_done(flight).unwrap_or(false) {
                    unretired += 1;
                }
            }
            let flight = self.flights.iter().find(|f| f.id == id).ok_or(EIO)?;
            dev_info!(
                self.dev.as_ref(),
                "G16: publish {} {:#x} outstanding={} unfinished_stamps={} queues={:#x}/{:#x} events={}/{}\n",
                kind,
                id,
                self.flights.len(),
                unretired,
                flight.queues[0].0.saturating_sub(0x4000),
                flight.queues[1].0.saturating_sub(0x4000),
                flight.stamps[0].map_or(u32::MAX, |s| s.event),
                flight.stamps[1].map_or(u32::MAX, |s| s.event)
            );
        }
        Ok(())
    }

    /// Dispatch events by the outstanding Work identity, never by the most
    /// recently submitted job. Completion notifications are hints; stamps and
    /// ring retirement remain authoritative even if event bits coalesce.
    fn service_flights(&mut self) -> Result {
        const STATE: u64 = 0xffff_fc20_0003_f1c0;
        const RING: u64 = 0xffff_fc20_0003_f440;
        for _ in 0..256 {
            let fw = self._firmware_space.as_mut().ok_or(EIO)?;
            let cursor = fw.read_u32(STATE)?;
            let end = fw.read_u32(STATE + 0x20)?;
            if cursor >= 256 || end >= 256 {
                return Err(EIO);
            }
            if cursor == end {
                break;
            }
            let base = RING + u64::from(cursor) * 0x48;
            let kind = fw.read_u32(base)?;
            if kind == 1
                || (kind == 13
                    && self.opening_notification_pending
                    && fw.read_u32(base + 4)? == 1
                    && fw.read_u32(base + 8)? == 0
                    && fw.read_u64(base + 12)? == g16_fw::CONTROL_DATA
                    && fw.read_u64(base + 20)? == g16_fw::OPERAND_TABLE
                    && fw.read_u64(base + 28)? == g16_fw::OPERAND_TABLE
                    && fw.read_u32(base + 36)? == 20 * 8)
            {
                if kind == 13 {
                    self.opening_notification_pending = false;
                }
                fw.write_live(STATE, &((cursor + 1) & 255).to_le_bytes())?;
                continue;
            }
            let mut selected = None;
            for (index, flight) in self.flights.iter().enumerate() {
                if let Some(render) = &flight.render {
                    let a = &render.addresses;
                    let matches = match kind {
                        6 => {
                            fw.read_u32(base + 4)? == a.context_id as u32
                                && fw.read_u32(base + 8)? == a.buffer_manager_slot as u32
                                && fw.read_u32(a.fragment_firmware_stamp)?
                                    != a.fragment_stamp as u32
                        }
                        7 => {
                            fw.read_u32(base + 12)? == a.fragment_event as u32
                                && fw.read_u32(base + 16)? == a.fragment_stamp as u32
                        }
                        _ => false,
                    };
                    if matches {
                        selected = Some(index);
                        break;
                    }
                }
            }
            let index = selected.ok_or_else(|| {
                dev_err!(
                    self.dev.as_ref(),
                    "G16: unowned Event kind {} cursor {}\n",
                    kind,
                    cursor
                );
                EIO
            })?;
            let mut render = self.flights[index].render.take().ok_or(EIO)?;
            let pool_index = self
                .tvb_pools
                .iter()
                .position(|p| p.root == render.root)
                .ok_or(EIO)?;
            let mut pool = self.tvb_pools.swap_remove(pool_index);
            let result = self.service_render_events(
                &mut render.client.lock(),
                &mut pool,
                &render.addresses,
                &mut render.events,
            );
            self.tvb_pools.push(pool, GFP_KERNEL)?;
            self.flights[index].render = Some(render);
            result?;
            if self._firmware_space.as_ref().ok_or(EIO)?.read_u32(STATE)? == cursor {
                break;
            }
        }
        self.drain_trace_rx()
    }

    pub(crate) fn retire(&mut self) -> Result<Option<(u64, Result)>> {
        if self.render_failed {
            return Err(EIO);
        }
        self.service_flights().inspect_err(|error| {
            dev_err!(
                self.dev.as_ref(),
                "G16: firmware event service failed: {:?}\n",
                error
            );
        })?;
        let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
        for index in 0..self.flights.len() {
            let flight = &self.flights[index];
            if !self.stamps_done(flight)? {
                continue;
            }
            let fw = self._firmware_space.as_ref().ok_or(EIO)?;
            let mut done = true;
            for &(pointers, head) in &flight.queues {
                if pointers != 0 {
                    done &= reached(fw.read_u32(pointers)?, head, 0x500)
                        && reached(fw.read_u32(pointers + 0x30)?, head, 0x500);
                }
            }
            let guard = self._sgx.try_access().ok_or(ENODEV)?;
            for &(channel, target) in &flight.channels {
                if channel == usize::MAX {
                    continue;
                }
                for address in &config.channels[channel].state[..2] {
                    done &= reached(
                        guard.try_read32(0xd64000 + (*address - 0xffff_fc20_0002_8000) as usize)?,
                        target,
                        256,
                    );
                }
            }
            // Growth replies can refer to a Work after its main stage stamp.
            if flight.render.is_some() {
                done &= guard.try_read32(0xd640c0)? == guard.try_read32(0xd60060)?
                    && guard.try_read32(0xd640c8)? == guard.try_read32(0xd60060)?;
            }
            drop(guard);
            if !done {
                continue;
            }
            let limited = flight
                .render
                .as_ref()
                .is_some_and(|r| r.events.memory_limit);
            let id = flight.id;
            if let Some(render) = &flight.render {
                let pool = self
                    .tvb_pools
                    .iter_mut()
                    .find(|p| p.root == render.root)
                    .ok_or(EIO)?;
                pool.release_scene(render.scene);
            }
            let mut owner = flight.owner.lock();
            owner.pending -= 1;
            if owner.pending == 0 {
                owner.events = None;
            }
            drop(owner);
            self.context_users[flight.context as usize] -= 1;
            self.flights.remove(index).map_err(|_| EIO)?;
            // Queued commands can legitimately wait longer than one Work's
            // timeout. Watch for a lack of retirement progress, rather than
            // charging every command for all its predecessors' execution.
            let now = Instant::now();
            for pending in &mut self.flights {
                pending.started = now;
            }
            return Ok(Some((id, if limited { Err(ENOMEM) } else { Ok(()) })));
        }
        if let Some(flight) = self
            .flights
            .iter()
            .find(|f| f.started.elapsed().as_millis() >= 10000)
        {
            dev_err!(
                self.dev.as_ref(),
                "G16: async Work {:#x} context {} timeout without retirement progress\n",
                flight.id,
                flight.context
            );
            self.render_failed = true;
            return Err(ETIMEDOUT);
        }
        Ok(None)
    }

    /// Notifications do not extend the watchdog. Only retirement progress
    /// resets it, so trace traffic cannot keep a stuck command alive.
    pub(crate) fn watchdog_remaining_ms(&self) -> u32 {
        self.flights
            .iter()
            .map(|flight| (10000 - flight.started.elapsed().as_millis()).clamp(1, 10000) as u32)
            .min()
            .unwrap_or(10000)
    }
}

/// A per-VM firmware support object retained by every queued execution until
/// its GPU accesses finish, including after public VM/queue handle destruction.
pub(crate) struct Support {
    slots: Arc<AtomicU64>,
    index: u32,
    growth_base: u64,
    submissions: AtomicU64,
}
impl Support {
    pub(crate) fn address(&self) -> u64 {
        render::SUPPORT_BASE + u64::from(self.index) * 0x4000
    }
}
impl Drop for Support {
    fn drop(&mut self) {
        self.slots.fetch_and(!(1 << self.index), Ordering::AcqRel);
    }
}

impl Bootstrap {
    pub(crate) fn reserve_timestamp(&mut self, size: u64) -> Result<u64> {
        if size == 0 || size & 0x3fff != 0 {
            return Err(EINVAL);
        }
        // Match the shim's firmware-shared special-object aperture.
        let mut base = g16_fw::TIMESTAMP_BASE;
        for range in &self.timestamp_ranges {
            if base.checked_add(size).ok_or(ENOSPC)? <= range.start {
                break;
            }
            base = base.max(range.end);
        }
        let end = base.checked_add(size).ok_or(ENOSPC)?;
        if end > g16_fw::TIMESTAMP_BASE + g16_fw::TIMESTAMP_SIZE {
            return Err(ENOSPC);
        }
        self.timestamp_ranges.push(base..end, GFP_KERNEL)?;
        self.timestamp_ranges.sort_unstable_by_key(|r| r.start);
        Ok(base)
    }

    pub(crate) fn map_timestamp(&mut self, address: u64, physical: u64, size: usize) -> Result {
        self._firmware_space.as_mut().ok_or(EIO)?.map_external(
            address,
            physical,
            size,
            crate::pgtable::prot::PROT_FW_SHARED_RW,
        )?;
        self._firmware_space.as_ref().ok_or(EIO)?.sync();
        Ok(())
    }
    pub(crate) fn release_timestamp(&mut self, range: core::ops::Range<u64>) -> Result {
        self._firmware_space
            .as_mut()
            .ok_or(EIO)?
            .unmap_external(range.clone())?;
        self.timestamp_ranges.retain(|r| *r != range);
        Ok(())
    }

    pub(crate) fn render_failed(&self) -> bool {
        self.render_failed
    }

    pub(crate) fn acquire_render_support(&mut self, pool: &render::OperandPool) -> Result<Support> {
        if self.render_failed {
            return Err(EIO);
        }
        let mut used = self.render_support.load(Ordering::Acquire);
        let index = loop {
            let index = (!used).trailing_zeros();
            if index >= 57 {
                return Err(ENOSPC);
            }
            match self.render_support.compare_exchange_weak(
                used,
                used | (1 << index),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break index,
                Err(value) => used = value,
            }
        };
        let lease = Support {
            slots: self.render_support.clone(),
            index,
            growth_base: pool.growth_base().ok_or(EINVAL)?,
            submissions: AtomicU64::new(0),
        };
        let mut a = render::Addresses::bootstrap();
        a.support = lease.address();
        a.render_shared_state = lease.address() + 0x1000;
        let mut page = KVec::new();
        page.extend_with(0x4000, 0u8, GFP_KERNEL)?;
        page[..0x100].copy_from_slice(&pool.support(3 + u64::from(index), a.render_shared_state));
        self._firmware_space
            .as_mut()
            .ok_or(EIO)?
            .write_live(a.support, &page)?;
        Ok(lease)
    }

    fn set_context(&mut self, index: usize, roots: Option<g16_vm::Roots>) -> Result {
        let res = self
            .dev
            .as_ref()
            .of_node()
            .ok_or(EINVAL)?
            .reserved_mem_region_to_resource_byname(c_str!("ttbs"))?;
        // SAFETY: The reserved table has 64 two-root slots. A slot is changed
        // only with no outstanding Work using it, under the runtime mutex.
        let table = unsafe { io::mem::Mem::try_new(res, io::mem::MemFlag::WB.into())? };
        unsafe {
            let ptr = table.ptr().add(index * 16);
            ptr.cast::<u64>()
                .write_volatile(ptr.cast::<u64>().read_volatile() & !1);
            ptr.add(8)
                .cast::<u64>()
                .write_volatile(ptr.add(8).cast::<u64>().read_volatile() & !1);
            core::arch::asm!("dc cvac, {addr}", "dsb oshst", addr=in(reg) ptr);
            crate::mem::tlbi_asid(index as u8);
            core::arch::asm!("dsb osh", "isb");
            if let Some(roots) = roots {
                ptr.cast::<u64>()
                    .write_volatile(roots.low | ((index as u64) << 48) | 1);
                ptr.add(8)
                    .cast::<u64>()
                    .write_volatile(roots.high | ((index as u64) << 48) | 1);
                core::arch::asm!("dc cvac, {addr}", addr=in(reg) ptr);
            }
        }
        crate::mem::sync();
        self.context_roots[index] = roots;
        Ok(())
    }

    fn acquire_context(&mut self, roots: g16_vm::Roots, preferred: usize) -> Result<u32> {
        let existing = (1..64).find(|&i| self.context_roots[i].is_some_and(|r| r.low == roots.low));
        let slot = existing
            .or_else(|| {
                if self.context_users[preferred] == 0 {
                    Some(preferred)
                } else {
                    (1..64).find(|&i| self.context_users[i] == 0)
                }
            })
            .ok_or(EBUSY)?;
        if existing.is_none() {
            self.set_context(slot, Some(roots))?;
        }
        self.context_users[slot] += 1;
        Ok(slot as u32)
    }

    pub(crate) fn release_render_vm(&mut self, space: g16_vm::Roots) -> Result {
        if self.render_failed {
            return Err(EIO);
        }
        for index in 1..64 {
            if self.context_roots[index].is_some_and(|r| r.low == space.low) {
                if self.context_users[index] != 0 {
                    return Err(EBUSY);
                }
                self.set_context(index, None)?;
            }
        }
        if let Some(index) = self.tvb_pools.iter().position(|p| p.root == space.low) {
            let a = self.tvb_pools[index].addresses;
            let fw = self._firmware_space.as_mut().ok_or(EIO)?;
            fw.write_live(0xfffffc2000128000 + a.buffer_manager_slot * 16, &[0; 16])?;
            fw.release_region(a.buffer_manager)?;
            self.tvb_pools.swap_remove(index);
        }
        Ok(())
    }

    fn drain_trace_rx(&mut self) -> Result {
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        for (state, capacity) in [
            (0xffff_fc20_0003_f3c0, 512),
            (0xffff_fc20_0003_f400, 256),
            (0xffff_fc20_0003_f200, 256),
            (0xffff_fc20_0003_f230, 256),
            (0xffff_fc20_0003_f260, 256),
            (0xffff_fc20_0003_f290, 256),
            (0xffff_fc20_0003_f2c0, 256),
            (0xffff_fc20_0003_f2f0, 256),
        ] {
            let read = fw.read_u32(state)?;
            let write = fw.read_u32(state + 0x20)?;
            if read >= capacity || write >= capacity {
                dev_err!(
                    self.dev.as_ref(),
                    "G16: invalid RX cursors at {:#x}: {}/{} capacity {}\n",
                    state,
                    read,
                    write,
                    capacity
                );
                return Err(EIO);
            }
            if read != write {
                if state == 0xffff_fc20_0003_f3c0 {
                    let mut cursor = read;
                    while cursor != write {
                        let base = 0xffff_fc20_000e_5c40 + u64::from(cursor) * 0x48;
                        if fw.read_u32(base)? == 5
                            && fw.read_u32(base + 0x2c)? & 0xffff == 4
                            && fw.read_u64(base + 0x30)? == 0
                            && fw.read_u64(base + 0x14)? != 0
                        {
                            pr_info!(
                                "G16: firmware Start3D partial render queue {:#x}\n",
                                fw.read_u64(base + 0x24)?
                            );
                        }
                        cursor = (cursor + 1) % capacity;
                    }
                }
                fw.write_live(state, &write.to_le_bytes())?;
            }
        }
        Ok(())
    }

    /// The caller retains all client mappings throughout this call. On failure
    /// after publication it must quarantine those mappings until reset.
    pub(crate) fn submit_render(
        &mut self,
        queues: Arc<Mutex<FirmwareQueues>>,
        owner: Arc<Mutex<g16_vm::AddressSpace>>,
        params: &render::Parameters,
        support: &Support,
        timestamps: [u64; 4],
    ) -> Result<Receipt> {
        let mut queue = queues.lock();
        self.reserve_flight(&queue, true, Some(owner.lock().roots().low))?;
        self.prepare_queues(&mut queue)?;
        let result = self.submit_render_queue(
            &mut queue,
            queues.clone(),
            owner,
            params,
            support,
            timestamps,
        );
        if queue.pending == 0 {
            queue.events = None;
        }
        result
    }

    fn submit_render_queue(
        &mut self,
        queue: &mut FirmwareQueues,
        queues: Arc<Mutex<FirmwareQueues>>,
        owner: Arc<Mutex<g16_vm::AddressSpace>>,
        params: &render::Parameters,
        support: &Support,
        timestamps: [u64; 4],
    ) -> Result<Receipt> {
        let mut guard = owner.lock();
        let client = &mut *guard;
        let root = client.roots().low;
        let index = match self.tvb_pools.iter().position(|p| p.root == root) {
            Some(index) => index,
            None => {
                self.tvb_pools.reserve(1, GFP_KERNEL)?;
                let base = self.next_work_va;
                self.next_work_va = base.checked_add(0x44000).ok_or(ENOSPC)?;
                let pool = crate::g16_tvb::Tvb::new(
                    self._firmware_space.as_mut().ok_or(EIO)?,
                    client,
                    base,
                    7 + u64::from(support.index),
                    support.growth_base,
                )?;
                self.tvb_pools.push(pool, GFP_KERNEL)?;
                self.tvb_pools.len() - 1
            }
        };
        let scene = self.tvb_pools[index].reserve_scene().inspect_err(|_| {
            self.report_pressure(4, "scenes");
        })?;
        let mut pool = self.tvb_pools.swap_remove(index);
        let result = self.submit_render_pool(
            client,
            params,
            support,
            queue,
            queues,
            &mut pool,
            owner.clone(),
            scene,
            timestamps,
        );
        if result.is_err() && !self.render_failed {
            pool.release_scene(scene);
        }
        self.tvb_pools.push(pool, GFP_KERNEL)?;
        result
    }

    fn submit_render_pool(
        &mut self,
        client: &mut g16_vm::AddressSpace,
        params: &render::Parameters,
        support: &Support,
        queue: &mut FirmwareQueues,
        queues: Arc<Mutex<FirmwareQueues>>,
        pool: &mut crate::g16_tvb::Tvb,
        owner: Arc<Mutex<g16_vm::AddressSpace>>,
        scene: u32,
        timestamps: [u64; 4],
    ) -> Result<Receipt> {
        if self.render_failed {
            return Err(EIO);
        }
        if !params.valid() || !(1..=2).contains(&queue.priority) {
            return Err(EINVAL);
        }
        let space = client.roots();
        let mut params = *params;
        params.tiling_admission = pool.addresses.buffer_manager_slot;
        params.fragment_admission = pool.addresses.buffer_manager_slot;
        // These accelerator pointers must select the same leased scene as
        // BufferThing. Reusing scene 1 here corrupts other in-flight renders.
        params.scene_slot = u64::from(scene);
        params.cycle = 0x240000 + u64::from(scene) * 0x30;
        client.prepare_render(&mut params)?;
        let mut a = render::Addresses::bootstrap()
            .publication(self.render_publication)
            .ok_or(ENOSPC)?;
        pool.work_addresses(&mut a, scene);
        // Keep profiling storage private to the Work. Public timestamp BOs
        // are written by the firmware through its separate user destinations.
        let lanes = queue.lanes.as_ref().ok_or(EIO)?;
        a.tiling_queue = lanes[0].queue;
        a.fragment_queue = lanes[1].queue;
        a.tiling_event = u64::from(queue.event(0)?);
        a.fragment_event = u64::from(queue.event(1)?);
        a.support = support.address();
        a.render_shared_state = support.address() + 0x1000;
        a.tiling_status_page = params.ta_status & !0x3fff;
        a.fragment_status_page = params.fragment_status & !0x3fff;
        let queue_ordinal = queue.render_count;
        a.fragment_counter = queue_ordinal * 2;
        a.tiling_counter = queue_ordinal * 2 + 1;
        // Firmware's dependency graph requires consecutive stamp updates
        // within each lane, even when other queues publish between them.
        a.fragment_stamp = 0x100 + queue_ordinal * 0x100;
        a.tiling_stamp = 0xc00 + queue_ordinal * 0x100;
        let va = self.next_work_va;
        self.next_work_va = va.checked_add(0x18000).ok_or(ENOSPC)?;
        a.tiling_shared_tail = va + 0x10000;
        a.fragment_shared_tail = va + 0x14000;
        a.event_control = va + 0x200;
        a.event_count_array = va + 0x300;
        a.tiling_driver_stamp = va + 0x400;
        a.fragment_driver_stamp = va + 0x440;
        a.tiling_firmware_stamp = va + 0x480;
        a.fragment_firmware_stamp = va + 0x4c0;
        let alias = 0x72_0000_0000 + self.render_publication * 0x8000;
        if alias + 0x8000 > 0x73_0000_0000 {
            return Err(ENOSPC);
        }
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        fw.alloc(va, 0x10000, crate::pgtable::prot::PROT_FW_PRIV_RW)?;
        fw.alloc(
            va + 0x10000,
            0x8000,
            crate::pgtable::prot::PROT_FW_SHARED_RW,
        )?;
        for stage in 0..2u64 {
            let work_page = va + stage * 0x8000;
            let register_page = alias + stage * 0x4000;
            let pa = fw.physical(work_page)?;
            for view in [self._address_space.as_mut().ok_or(EIO)?, &mut *client] {
                view.low.map_pages(
                    register_page..register_page + 0x4000,
                    pa,
                    crate::pgtable::prot::PROT_GPU_SHARED_RW,
                    false,
                )?;
                view.sync();
            }
            if stage == 0 {
                a.tiling_work = work_page + (a.tiling_work & 0x3fff);
                a.tiling_microsequence = work_page + 0x4000 + (a.tiling_microsequence & 0x3fff);
                a.tiling_prelude = work_page + 0x4000;
                a.tiling_register_alias = register_page + (a.tiling_register_alias & 0x3fff);
            } else {
                a.fragment_work = work_page + (a.fragment_work & 0x3fff);
                a.fragment_microsequence = work_page + 0x4000 + (a.fragment_microsequence & 0x3fff);
                a.fragment_prelude = work_page + 0x4000;
                a.fragment_register_alias = register_page + (a.fragment_register_alias & 0x3fff);
            }
        }
        fw.sync();
        crate::mem::tlbi_all();
        crate::mem::sync();
        // A live context lease keeps these roots fixed through retirement.
        a.context_id = u64::from(self.acquire_context(space, 9)?);
        a.tiling_user_timestamp_start = timestamps[0];
        a.tiling_user_timestamp_end = timestamps[1];
        a.fragment_user_timestamp_start = timestamps[2];
        a.fragment_user_timestamp_end = timestamps[3];
        // From here any publication failure quarantines the roots.
        self.render_failed = true;
        let mut ta = params.tiling_work(&a).ok_or(EINVAL)?;
        let mut frag = params.fragment_work(&a).ok_or(EINVAL)?;
        // Keep firmware's internal profiling destinations distinct from the
        // explicit user timestamp slots used for UAPI completion above.
        ta[0x8c8..0x8d0].copy_from_slice(&(va + 0x100).to_le_bytes());
        ta[0x8d0..0x8d8].copy_from_slice(&(va + 0x108).to_le_bytes());
        frag[0xc10..0xc18].copy_from_slice(&(va + 0x110).to_le_bytes());
        frag[0xc18..0xc20].copy_from_slice(&(va + 0x118).to_le_bytes());
        // TVB initialization and first publication of a firmware queue are
        // independent: several public queues can share one VM's TVB pool.
        let initbm = !pool.initialized;

        // The shared count covers individual Work items, not render pairs.
        // Firmware retires one tiling and one fragment item per submission.
        let submitted = support.submissions.load(Ordering::Relaxed);
        let next_submitted = submitted.checked_add(2).ok_or(ENOSPC)?;
        let use_count = u32::try_from(next_submitted).map_err(|_| ENOSPC)?;
        // From here an error conservatively quarantines the client root.
        self.render_failed = true;
        self._firmware_space
            .as_mut()
            .ok_or(EIO)?
            .write_live(a.render_shared_state, &use_count.to_le_bytes())?;
        let (heads, epoch) = self.publish_render(&a, &ta, &frag, queue, initbm, pool.blocks)?;
        pool.initialized = true;
        support.submissions.store(next_submitted, Ordering::Relaxed);
        self.render_publication += 1;
        queue.render_count = queue_ordinal + 1;
        self.render_channel_publications[queue.priority as usize] += 1;
        let lanes = queue.lanes.as_mut().ok_or(EIO)?;
        for stage in 0..2 {
            lanes[stage].head = heads[stage];
            lanes[stage].new = false;
        }
        let stamp = Stamp {
            address: a.fragment_firmware_stamp,
            value: a.fragment_stamp as u32,
            event: a.fragment_event as u32,
        };
        let receipt = Receipt { id: va, stamp };
        queue.last[0] = Some(stamp);
        queue.pending += 1;
        self.flights.push(
            Flight {
                id: va,
                owner: queues,
                context: a.context_id as u32,
                started: Instant::now(),
                stamps: [
                    Some(Stamp {
                        address: a.tiling_firmware_stamp,
                        value: a.tiling_stamp as u32,
                        event: a.tiling_event as u32,
                    }),
                    Some(stamp),
                ],
                queues: [(lanes[0].pointers, heads[0]), (lanes[1].pointers, heads[1])],
                channels: [
                    (queue.priority as usize * 3, epoch),
                    (queue.priority as usize * 3 + 1, epoch),
                ],
                render: Some(RenderFlight {
                    addresses: a,
                    client: owner,
                    root: space.low,
                    events: RenderEvents::default(),
                    scene,
                }),
            },
            GFP_KERNEL,
        )?;
        self.render_failed = false;
        self.trace_publication("render", va)?;
        Ok(receipt)
    }

    fn publish_render(
        &mut self,
        a: &render::Addresses,
        ta: &[u8],
        frag: &[u8],
        queue: &FirmwareQueues,
        initbm: bool,
        blocks: u32,
    ) -> Result<([u32; 2], u32)> {
        use g16_fw::queue as q;
        let priority = queue.priority;
        let lanes = queue.lanes.as_ref().ok_or(EIO)?;
        let publication = self.render_publication;
        let epoch = (self.render_channel_publications[priority as usize] & 255) as u32;
        let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
        let routes = [
            &config.channels[priority as usize * 3],
            &config.channels[priority as usize * 3 + 1],
        ];
        let producer_offset = |address: u64| 0xd60000 + (address - 0xffff_fc20_0002_0000) as usize;
        let next = (epoch + 1) & 255;
        let firsts = [lanes[0].head, lanes[1].head];
        let heads = [
            (firsts[0] + 1 + u32::from(initbm)) % 0x500,
            (firsts[1] + 2) % 0x500,
        ];
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        fw.write_live(a.event_control, &a.event_count_array.to_le_bytes())?;
        fw.write_live(a.event_control + 0x10, &0x50u32.to_le_bytes())?;
        fw.write_live(a.event_count_array, &2u32.to_le_bytes())?;
        // The queue lanes persist independently of the physical work channel.
        for stage in 0..2 {
            let Lane { pointers, ring, .. } = lanes[stage];
            let prelude = if stage == 0 {
                a.tiling_prelude
            } else {
                a.fragment_prelude
            };
            let work = if stage == 0 {
                a.tiling_work
            } else {
                a.fragment_work
            };
            let micro = if stage == 0 {
                a.tiling_microsequence
            } else {
                a.fragment_microsequence
            };
            fw.write_live(
                prelude,
                &if stage == 0 {
                    a.tiling_prelude(blocks)
                } else {
                    a.fragment_prelude()
                },
            )?;
            fw.write_live(work, if stage == 0 { ta } else { frag })?;
            if stage == 0 {
                fw.write_live(micro, &a.tiling_microsequence())?;
            } else {
                fw.write_live(micro, &a.fragment_microsequence())?;
            }
            let mut pos = firsts[stage];
            if stage == 1 || initbm {
                fw.write_live(ring + u64::from(pos) * 8, &prelude.to_le_bytes())?;
                pos = (pos + 1) % 0x500;
            }
            fw.write_live(ring + u64::from(pos) * 8, &work.to_le_bytes())?;
            fw.write_live(pointers + 0x40, &heads[stage].to_le_bytes())?;
        }
        crate::mem::sync();
        let sgx_guard = self._sgx.try_access().ok_or(ENODEV)?;
        let sgx = &*sgx_guard;
        // The selected fragment channel is published before tiling; its barrier
        // waits for the matching TA completion stamp.
        for stage in [1usize, 0] {
            let route = routes[stage];

            let producer = producer_offset(route.state[2]);
            if sgx.try_read32(producer)? != epoch {
                return Err(EIO);
            }
            let ring = route.ring;
            let (queue, event) = if stage == 0 {
                (a.tiling_queue, a.tiling_event)
            } else {
                (a.fragment_queue, a.fragment_event)
            };
            self._firmware_space.as_mut().ok_or(EIO)?.write_live(
                ring + u64::from(epoch) * 0x18,
                &q::channel(
                    queue,
                    heads[stage] as u16,
                    event as u8,
                    stage as u32,
                    lanes[stage].new,
                    publication + 1,
                ),
            )?;
            sgx.try_write32(next, producer)?;
        }
        crate::mem::sync();
        drop(sgx_guard);
        self._rtkit
            .as_mut()
            .send_message(0x21, 0x0083_0000_0000_0000 | (u64::from(priority) << 2))?;
        Ok((heads, next))
    }
}

impl Bootstrap {
    pub(crate) fn map_compute_work(&mut self, space: &mut g16_vm::AddressSpace) -> Result {
        let opening = self._opening_client.as_mut().ok_or(EIO)?;
        for (base, size) in compute::private_ranges() {
            for va in (base..base + size as u64).step_by(0x4000) {
                if space.low.translate(va)?.is_some() {
                    return Err(EEXIST);
                }
                let pa = opening.low.translate(va)?.ok_or(EIO)?;
                space.low.map_pages(
                    va..va + 0x4000,
                    pa,
                    crate::pgtable::prot::PROT_GPU_SHARED_RW,
                    false,
                )?;
            }
        }
        space.sync();
        Ok(())
    }

    pub(crate) fn submit_compute(
        &mut self,
        queues: Arc<Mutex<FirmwareQueues>>,
        parent: &mut g16_vm::AddressSpace,
        params: &compute::Parameters,
        memory: &impl crate::g16_cdm::Memory,
        timestamps: [u64; 4],
    ) -> Result<Receipt> {
        let mut queue = queues.lock();
        self.reserve_flight(&queue, false, None)?;
        self.prepare_queues(&mut queue)?;
        let result = self.submit_compute_queue(
            &mut queue,
            queues.clone(),
            parent,
            params,
            memory,
            timestamps,
        );
        if queue.pending == 0 {
            queue.events = None;
        }
        result
    }

    fn submit_compute_queue(
        &mut self,
        queue: &mut FirmwareQueues,
        queues: Arc<Mutex<FirmwareQueues>>,
        parent: &mut g16_vm::AddressSpace,
        params: &compute::Parameters,
        memory: &impl crate::g16_cdm::Memory,
        timestamps: [u64; 4],
    ) -> Result<Receipt> {
        let lane = queue.lanes.as_ref().ok_or(EIO)?[2];
        let priority = queue.priority;

        parent.views.reserve(1, GFP_KERNEL)?;
        let mut view = parent.fork()?;
        let client = &mut view;
        // Different firmware compute queues can suspend/resume independently.
        // Their scratch and resume marker must not alias the device's opening
        // context (or another Work), even when they use the same virtual VA.
        for (base, size) in [(params.scratch, 0x20000), (params.marker, 0x4000)] {
            client.low.unmap_pages(base..base + size as u64)?;
            client.alloc_low(base, size, crate::pgtable::prot::PROT_GPU_SHARED_RW)?;
        }
        let space = client.roots();
        use g16_fw::queue as q;
        if self.render_failed {
            return Err(EIO);
        }
        if !(1..=2).contains(&priority) {
            return Err(EINVAL);
        }
        let ordinal = queue.compute_count;
        let resource = crate::g16_cdm::resource(memory, params.cdm, params.cdm_end)?;
        client.snapshot(memory, resource)?;
        let mut params = *params;
        params.resource = resource;
        let work = self.next_work_va;
        let alias = 0x71_0000_0000 + self.compute_publication * 0x4000;
        if alias + 0x4000 > 0x72_0000_0000 {
            return Err(ENOSPC);
        }
        self.next_work_va = work.checked_add(0x8000).ok_or(ENOSPC)?;
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        fw.alloc(work, 0x4000, crate::pgtable::prot::PROT_FW_PRIV_RW)?;
        let pa = fw.physical(work)?;
        self._address_space.as_mut().ok_or(EIO)?.low.map_pages(
            alias..alias + 0x4000,
            pa,
            crate::pgtable::prot::PROT_GPU_SHARED_RW,
            false,
        )?;
        client.low.map_pages(
            alias..alias + 0x4000,
            pa,
            crate::pgtable::prot::PROT_GPU_SHARED_RW,
            false,
        )?;
        let mut a = compute::Addresses::publication(self.compute_publication, ordinal, work, alias)
            .ok_or(ENOSPC)?;
        a.queue = lane.queue;
        a.event = queue.event(2)?;
        a.stamp = u32::try_from((ordinal + 1) * 0x100).map_err(|_| ENOSPC)?;
        self._address_space.as_ref().ok_or(EIO)?.sync();
        fw.sync();
        client.sync();
        crate::mem::tlbi_all();
        crate::mem::sync();
        a.context = self.acquire_context(space, 1)?;
        a.identity = (u64::from(a.context) << 32) | (a.identity & 0xffff_ffff);
        a.register_identity = (u64::from(a.context) << 32) | 1;
        // Retain the new tables and snapshot before publishing any Work.
        parent.views.push(view, GFP_KERNEL)?;
        self.render_failed = true;
        a.cdm_entry = Some(alias + compute::ENTRY_OFFSET);
        let work = params.work(&a).ok_or(EINVAL)?;
        // Timestamp's user destinations must lie in the validated special-
        // object aperture. Firmware writes nanoseconds directly, as on M1/M2.
        // The pointer pair is private to this Work and may be consumed/reset.
        let user_timestamps = a.work + 0x17c0;
        let count = self.compute_publication.checked_add(1).ok_or(ENOSPC)?;
        let count32 = u32::try_from(count).map_err(|_| ENOSPC)?;

        let first = lane.head;
        let head = (first + 1) % 0x500;
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        fw.write_live(compute::SHARED_STATE, &count32.to_le_bytes())?;
        fw.write_live(a.work, &work)?;
        fw.write_live(a.work + compute::ENTRY_OFFSET, &compute::entry(params.cdm))?;
        let mut microsequence = params.microsequence(&a);
        for base in [0x1bc, 0x20c] {
            microsequence[base + 0x24..base + 0x2c].copy_from_slice(&user_timestamps.to_le_bytes());
        }
        fw.write_live(user_timestamps, &timestamps[0].to_le_bytes())?;
        fw.write_live(user_timestamps + 8, &timestamps[1].to_le_bytes())?;
        fw.write_live(a.microsequence, &microsequence)?;
        let notifier = a.notifier(count32);
        // Retained notifier list links are firmware-owned even after completion.
        fw.write_live(a.notifier, &notifier[..0x14])?;
        fw.write_live(a.notifier + 0x24, &notifier[0x24..0x28])?;
        fw.write_live(a.threshold, &count32.to_le_bytes())?;
        let previous = a.stamp.checked_sub(0x100).ok_or(EINVAL)?;
        fw.write_live(a.driver_stamp, &previous.to_le_bytes())?;
        fw.write_live(a.firmware_stamp, &previous.to_le_bytes())?;
        fw.write_live(lane.ring + u64::from(first) * 8, &a.work.to_le_bytes())?;
        fw.write_live(lane.pointers + 0x40, &head.to_le_bytes())?;
        crate::mem::sync();
        let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
        let channel = priority as usize * 3 + 2;
        let route = &config.channels[channel];
        let producer = 0xd60000 + (route.state[2] - 0xffff_fc20_0002_0000) as usize;
        let epoch = (self.compute_channel_publications[priority as usize] & 255) as u32;
        let next = (epoch + 1) & 255;
        let guard = self._sgx.try_access().ok_or(ENODEV)?;
        let sgx = &*guard;
        if sgx.try_read32(producer)? != epoch {
            return Err(EIO);
        }
        self._firmware_space.as_mut().ok_or(EIO)?.write_live(
            route.ring + u64::from(epoch) * 0x18,
            &q::channel(
                a.queue,
                head as u16,
                a.event as u8,
                2,
                lane.new,
                self.compute_publication + 1,
            ),
        )?;
        crate::mem::sync();
        sgx.try_write32(next, producer)?;
        crate::mem::sync();
        drop(guard);
        self._rtkit
            .as_mut()
            .send_message(0x21, 0x0083_0000_0000_0002 | (u64::from(priority) << 2))?;
        self.compute_publication += 1;
        queue.compute_count = ordinal + 1;
        self.compute_channel_publications[priority as usize] += 1;
        let lane = &mut queue.lanes.as_mut().ok_or(EIO)?[2];
        lane.head = head;
        lane.new = false;
        let stamp = Stamp {
            address: a.firmware_stamp,
            value: a.stamp,
            event: a.event,
        };
        let pointers = lane.pointers;
        queue.last[1] = Some(stamp);
        queue.pending += 1;
        self.flights.push(
            Flight {
                id: a.work,
                owner: queues.clone(),
                context: a.context,
                started: Instant::now(),
                stamps: [Some(stamp), None],
                queues: [(pointers, head), (0, 0)],
                channels: [(channel, next), (usize::MAX, 0)],
                render: None,
            },
            GFP_KERNEL,
        )?;
        self.render_failed = false;
        self.trace_publication("compute", a.work)?;
        Ok(Receipt { id: a.work, stamp })
    }
}

#[derive(Default)]
struct RenderEvents {
    memory_limit: bool,
}

impl Bootstrap {
    /// Consume only events owned by this publication. Growth transfers retained
    /// pages -> block list -> host counts -> reply -> producer, as in the shim.
    fn service_render_events(
        &mut self,
        client: &mut g16_vm::AddressSpace,
        pool: &mut crate::g16_tvb::Tvb,
        a: &render::Addresses,
        service: &mut RenderEvents,
    ) -> Result {
        const STATE: u64 = 0xfffffc200003f1c0;
        const RING: u64 = 0xfffffc200003f440;
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        let mut cursor = fw.read_u32(STATE)?;
        let end = fw.read_u32(STATE + 0x20)?;
        if cursor >= 256 || end >= 256 {
            return Err(EIO);
        }
        while cursor != end {
            let base = RING + u64::from(cursor) * 0x48;
            let kind = fw.read_u32(base)?;
            let next = (cursor + 1) & 255;
            if kind == 1 {
                fw.write_live(STATE, &next.to_le_bytes())?;
                cursor = next;
                continue;
            }
            if !matches!(kind, 6 | 7) {
                dev_err!(self.dev.as_ref(), "G16: unexpected render Event {}\n", kind);
                return Err(EIO);
            }
            let guard = self._sgx.try_access().ok_or(ENODEV)?;
            let sgx = &*guard;
            let head = sgx.try_read32(0xd640c0)?;
            let ack = sgx.try_read32(0xd640c8)?;
            let tail = sgx.try_read32(0xd60060)?;
            if head >= 256 || ack >= 256 || tail >= 256 {
                return Err(EIO);
            }
            let successor = (tail + 1) & 255;
            if successor == head || successor == ack {
                break;
            }
            let mut reply = [0u8; 64];
            if kind == 6 {
                let subpipe = fw.read_u32(base + 16)?;
                let halt_count = fw.read_u64(base + 20)?;
                if fw.read_u32(base + 4)? != a.context_id as u32
                    || fw.read_u32(base + 8)? != pool.addresses.buffer_manager_slot as u32
                    || fw.read_u32(base + 12)? != pool.counter
                    || subpipe >= 64
                {
                    return Err(EIO);
                }
                let grown = pool.grow(fw, client)?;
                dev_info!(
                    self.dev.as_ref(),
                    "G16: TVB slot {} blocks {} growth {} subpipe {}\n",
                    pool.addresses.buffer_manager_slot,
                    pool.blocks,
                    grown,
                    subpipe
                );
                for (i, word) in [
                    8,
                    u32::from(grown),
                    pool.addresses.buffer_manager_slot as u32,
                    a.context_id as u32,
                    pool.counter,
                ]
                .iter()
                .enumerate()
                {
                    reply[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
                }
                // Native allocateMemoryEvent echoes the request's subpipe
                // and halt counter. Queued work can request growth on another
                // subpipe before its main stage starts, so this state belongs
                // to the pool, not whichever Work is currently executing.
                reply[20..24].copy_from_slice(&subpipe.to_le_bytes());
                reply[24..32].copy_from_slice(&halt_count.to_le_bytes());
                pool.refused |= !grown;
                pool.counter = pool.counter.checked_add(1).ok_or(EIO)?;
            } else {
                if !pool.refused
                    || service.memory_limit
                    || fw.read_u32(base + 4)? != 0
                    || fw.read_u32(base + 8)? != 1
                    || fw.read_u32(base + 12)? != a.fragment_event as u32
                    || fw.read_u32(base + 16)? != a.fragment_stamp as u32
                {
                    return Err(EIO);
                }
                reply[..4].copy_from_slice(&9u32.to_le_bytes());
                reply[4..8].copy_from_slice(&1u32.to_le_bytes());
                reply[8..16].copy_from_slice(&a.fragment_queue.to_le_bytes());
                reply[16..20].copy_from_slice(&(a.fragment_stamp as u32).to_le_bytes());
                service.memory_limit = true;
            }
            crate::mem::sync();
            let control = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS).channels[12].ring;
            fw.write_live(control + u64::from(tail) * 64, &reply)?;
            crate::mem::sync();
            fw.write_live(STATE, &next.to_le_bytes())?;
            sgx.try_write32(successor, 0xd60060)?;
            crate::mem::sync();
            self._rtkit
                .as_mut()
                .send_message(0x21, 0x0084_0000_0000_0011)?;
            return Ok(());
        }
        Ok(())
    }
}
