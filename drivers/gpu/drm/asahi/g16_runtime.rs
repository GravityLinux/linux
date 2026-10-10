// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from Niklas Sheth's linux-m4-integration prototype.

//! G16 ring publication and independent firmware retirement.

use kernel::io::Io;
use super::Bootstrap;
use crate::{g16_compute as compute, g16_fw, g16_render as render, g16_stamp, g16_vm};
use core::sync::atomic::{AtomicU64, Ordering};
use kernel::time::{Instant, Monotonic};
use kernel::{
    c_str, io,
    prelude::*,
    sync::{Arc, Mutex},
};

/// One userspace queue's three firmware workqueues. Channel rings remain
/// device-wide transports; their messages select these independent queues.
/// Their storage returns to Bootstrap when the last command retires.
pub(crate) struct FirmwareQueues {
    lanes: Option<[Lane; 3]>,
    storage: Option<QueueStorage>,
    priority: u32,
    events: Option<QueueEvents>,
    /// The event index this queue held last (event_affinity).
    last_event: Option<u32>,
    pending: usize,
    render_count: u64,
    compute_count: u64,
    /// Render Works counted into the queue's shared notifier (two per render).
    notified_works: u32,
    pub(crate) last: [Option<Stamp>; 2],
    /// Kernel-side dependency resolution statistics (logged at teardown with
    /// dep_stats=1).
    deps: DependencyStats,
}

/// Cumulative per-queue counts of kernel-resolved dependencies.
#[derive(Default)]
struct DependencyStats {
    /// Renders admitted whose tiling stage waited on an earlier fragment
    /// stamp in the kernel (render_deps=1: the vertex-consumer order).
    ta_ordered: u64,
    /// Admission attempts deferred (EBUSY) because that fragment stamp was
    /// not yet reached; retries are counted each time.
    ta_held: u64,
    /// Render admissions deferred on a compute or other-queue stamp.
    cross_held: u64,
    /// Fragment-stage waits on the queue's own fragment lane dropped as
    /// already ordered by the lane.
    lane_dropped: u64,
    /// Compute admissions deferred on another queue's stamp.
    compute_held: u64,
    /// Works admitted after at least one deferral, and the total time from
    /// their first deferral to admission.
    held_works: u64,
    held_us: u64,
    /// First deferral of the Work currently waiting, if any.
    hold_start: Option<Instant<Monotonic>>,
}

impl DependencyStats {
    fn defer(&mut self) {
        if self.hold_start.is_none() {
            self.hold_start = Some(Instant::now());
        }
    }

    fn admit(&mut self) {
        if let Some(start) = self.hold_start.take() {
            self.held_works += 1;
            self.held_us += start.elapsed().as_micros_ceil() as u64;
        }
    }
}

impl Drop for FirmwareQueues {
    fn drop(&mut self) {
        let d = &self.deps;
        if crate::module_parameters::dep_stats.value() != 0
            && d.ta_ordered + d.ta_held + d.cross_held + d.lane_dropped + d.compute_held != 0
        {
            pr_info!(
                "G16: queue deps renders={} computes={} ta_ordered={} ta_held={} cross_held={} lane_dropped={} compute_held={} held_works={} held_us={}\n",
                self.render_count,
                self.compute_count,
                d.ta_ordered,
                d.ta_held,
                d.cross_held,
                d.lane_dropped,
                d.compute_held,
                d.held_works,
                d.held_us
            );
        }
    }
}

#[derive(Clone, Copy)]
struct Lane {
    queue: u64,
    pointers: u64,
    ring: u64,
    head: u32,
    new: bool,
}

/// fw_trace & 128 dumps the live firmware configuration once.
static CONFIG_DUMPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

impl Lane {
    fn firmware_stamp(&self) -> u64 {
        self.pointers + 0x1040
    }

    /// Completion stamp the notifier pass writes for this lane, shared by all
    /// of the lane's renders as on macOS. Shared, uncached like the stamp above.
    fn driver_stamp(&self) -> u64 {
        self.pointers + 0x2000
    }
}

/// The queue's render notifier, shared by all of its renders as on macOS:
/// +0 points at the submitted-Work count, +0xc counts completed Works.
/// Firmware-private, after the queue descriptor's 0x2410-byte private state.
fn render_notifier(lanes: &[Lane; 3]) -> u64 {
    lanes[0].queue + 0x3000
}

/// Cumulative count of render Works submitted to the shared notifier.
fn render_notifier_count(lanes: &[Lane; 3]) -> u64 {
    lanes[0].pointers + 0x2040
}

fn shared_notifier() -> bool {
    crate::module_parameters::shared_notifier.value() != 0
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

/// A public queue's nine firmware pages. Flights own the queue, so dropping it
/// means every command has retired: its stamps, rings and notifier list are
/// referenced only by that queue's own commands, never across queues.
struct QueueStorage {
    base: u64,
    retired: Arc<Mutex<KVec<u64>>>,
}

impl Drop for QueueStorage {
    fn drop(&mut self) {
        // Bootstrap reserved a slot for every region when it was allocated.
        if self.retired.lock().push_within_capacity(self.base).is_err() {
            pr_warn!("G16: queue storage {:#x} retained\n", self.base);
        }
    }
}

impl FirmwareQueues {
    pub(crate) fn new(priority: u32) -> Self {
        Self {
            lanes: None,
            storage: None,
            priority,
            events: None,
            last_event: None,
            pending: 0,
            render_count: crate::module_parameters::fw_counter_start.value(),
            compute_count: crate::module_parameters::fw_counter_start.value(),
            notified_works: 0,
            last: [None; 2],
            deps: DependencyStats::default(),
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
    /// Diagnostics: the command stream (VDM encoder or CDM) and the Work's
    /// ordinal on its queue, to join hang reports with userspace traces.
    stream: u64,
    ordinal: u64,
    tgid: i32,
    comm: [u8; 16],
    started: Instant<Monotonic>,
    stamps: [Option<Stamp>; 2],
    driver_stamps: [Option<Stamp>; 2],
    scratch: g16_vm::Scratch,
    client: Arc<Mutex<g16_vm::AddressSpace>>,
    queues: [(u64, u32); 2],
    channels: [(usize, u32); 2],
    render: Option<RenderFlight>,
    /// Shared-notifier renders: the notifier's completed-Work counter and the
    /// total that includes this render. Other flights own a per-Work notifier.
    notifier_count: Option<(u64, u32)>,
    /// Retirement trace (fw_trace & 16): when the stage stamps were first seen
    /// complete, and which later conditions still blocked retirement.
    stamps_seen: Option<Instant<Monotonic>>,
    blocked: u32,
    /// Stamps this Work waits on before it may run, for hang reports.
    dependencies: [Option<Stamp>; 2],
}

/// One channel message for all staged commands on this firmware queue.
/// Its epoch remains stable while head advances, so every flight can retain
/// the correct transport-retirement target before the batch is published.
#[derive(Clone, Copy)]
pub(super) struct Publication {
    channel: usize,
    epoch: u32,
    lane: Lane,
    event: u32,
    sequence: u64,
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
    /// A Work held on a stamp waits for a completion to advance it. With no
    /// Work in flight anywhere nothing can write that stamp, so the hold
    /// would never clear and the pump would keep the job forever with an
    /// idle GPU. If the same wait stays unreached with nothing in flight
    /// for a second (the pump re-polls every 256 ms), fail just that
    /// command and say why. A recorded hold that later resolves is counted
    /// and logged as a near miss.
    fn hold_check(&self, kind: &str, dep: &Stamp, now: u32, done: bool) -> Result {
        let key = dep.address ^ (u64::from(dep.value) << 32).rotate_left(16);
        let recorded = self.hold_key.load(Ordering::Relaxed);
        if done {
            if recorded == key {
                self.hold_key.store(0, Ordering::Relaxed);
                let misses = self.hold_near_misses.fetch_add(1, Ordering::Relaxed) + 1;
                if misses <= 16 {
                    dev_info!(self.dev.as_ref(),
                              "G16: {} wait on stamp {:#x} want {:#x} resolved after being unreachable with nothing in flight (near miss #{})\n",
                              kind, dep.address, dep.value, misses);
                }
            }
            return Ok(());
        }
        if !self.flights.is_empty() {
            if recorded == key {
                self.hold_key.store(0, Ordering::Relaxed);
            }
            return Ok(());
        }
        let now_ns = monotonic_ns();
        if recorded != key {
            self.hold_key.store(key, Ordering::Relaxed);
            self.hold_since_ns.store(now_ns, Ordering::Relaxed);
            return Ok(());
        }
        if now_ns - self.hold_since_ns.load(Ordering::Relaxed) < 1_000_000_000 {
            return Ok(());
        }
        self.hold_key.store(0, Ordering::Relaxed);
        dev_err!(
            self.dev.as_ref(),
            "G16: {} wait on stamp {:#x} want {:#x} (now {:#x}, event {}) unreachable with nothing in flight for 1 s; failing the command\n",
            kind, dep.address, dep.value, now, dep.event
        );
        Err(EDEADLK)
    }

    fn report_pressure(&self, bit: u64, resource: &str) {
        // Every resource here is released when work retires. Backpressure with
        // nothing in flight can therefore never clear: it means an accounting
        // leak, and submission stalls indefinitely with an idle GPU. Always
        // report that, and event/ASID exhaustion, once per boot; transient
        // ring and scene pressure under load stays behind fw_trace & 2.
        let persistent = bit & (1 | 8) != 0 || self.flights.is_empty();
        if (persistent || crate::module_parameters::fw_trace.value() & 2 != 0)
            && self.pressure_reported.fetch_or(bit, Ordering::Relaxed) & bit == 0
        {
            let asids = (1..64).filter(|&i| self.context_users[i] != 0).count();
            dev_info!(
                self.dev.as_ref(),
                "G16: backpressure {} outstanding={} queue_events={}/32 asids={}/63\n",
                resource,
                self.flights.len(),
                (self.queue_events.load(Ordering::Acquire) & 0xffff_ffff).count_ones(),
                asids
            );
        }
    }

    /// Lease Work storage until notifier/driver stamps and transport retire.
    /// Backing stays mapped. Queue-owned wrapping stamps outlive reused Work,
    /// so a pending dependency never follows a recycled completion address.
    fn allocate_work(&mut self, render: bool) -> Result<u64> {
        use crate::pgtable::prot::{PROT_FW_PRIV_RW, PROT_FW_SHARED_RW};
        if self.retire_log.capacity() < RETIRE_LOG {
            self.retire_log.reserve(RETIRE_LOG, GFP_KERNEL)?;
        }
        let index = usize::from(render);
        if let Some(va) = self.free_work[index].pop() {
            return Ok(va);
        }
        // Retirement must not allocate. Space for every outstanding lease
        // also covers all of them completing before another acquisition.
        self.free_work[index].reserve(self.flights.len() + 1, GFP_KERNEL)?;
        let stride = if render { 0x1c000 } else { 0x8000 };
        if self.work_arenas[index].is_empty() {
            let base = self.next_work_va;
            let end = base.checked_add(16 * stride).ok_or(ENOSPC)?;
            // Work VA only grows; a steady rise under constant load means
            // leaked Work allocations.
            pr_info!(
                "G16: work arena refill render={} base={:#x} free=[{}, {}] flights={}\n",
                render,
                base,
                self.free_work[0].len(),
                self.free_work[1].len(),
                self.flights.len()
            );
            // Failed refills retain their partial allocations, but no part of
            // that range becomes available for a later Work or another arena.
            self.next_work_va = end;
            let fw = self._firmware_space.as_mut().ok_or(EIO)?;
            for va in (base..end).step_by(stride as usize) {
                fw.alloc(va, if render { 0x10000 } else { 0x4000 }, PROT_FW_PRIV_RW)?;
                if render {
                    fw.alloc(va + 0x10000, 0xc000, PROT_FW_SHARED_RW)?;
                } else {
                    fw.alloc(va + 0x4000, 0x4000, PROT_FW_SHARED_RW)?;
                }
            }
            fw.sync();
            self.work_arenas[index] = base..end;
        }
        let va = self.work_arenas[index].start;
        self.work_arenas[index].start += stride;
        Ok(va)
    }

    /// Collect storage whose context destruction firmware has consumed. Retire
    /// polls this too, so the half-ring comparison sees each target promptly.
    /// It must not allocate: publication reserved each free-list slot.
    fn collect_destroyed_queues(&mut self) -> Result {
        if self.destroying_queues.is_empty() {
            return Ok(());
        }
        let guard = self._sgx.try_access().ok_or(ENODEV)?;
        let read = guard.try_read32(0xd640c0)?;
        let done = guard.try_read32(0xd640c8)?;
        drop(guard);
        if read >= 256 || done >= 256 {
            return Err(EIO);
        }
        let mut i = 0;
        while i < self.destroying_queues.len() {
            let (base, target) = self.destroying_queues[i];
            if reached(read, target, 256) && reached(done, target, 256) {
                self.destroying_queues.swap_remove(i);
                self.free_queues
                    .push_within_capacity(base)
                    .map_err(|_| EIO)?;
            } else {
                i += 1;
            }
        }
        Ok(())
    }

    /// Firmware may still hold a retired queue's context ID binding, so the
    /// context is destroyed before its storage serves another queue.
    fn recycle_queues(&mut self) -> Result {
        self.collect_destroyed_queues()?;
        loop {
            self.destroying_queues.reserve(1, GFP_KERNEL)?;
            self.free_queues
                .reserve(self.destroying_queues.len() + 1, GFP_KERNEL)?;
            let Some(base) = self.retired_queues.lock().pop() else {
                return Ok(());
            };
            match self.destroy_queue_context(base)? {
                Some(target) => self.destroying_queues.push((base, target), GFP_KERNEL)?,
                None => {
                    // The control ring is full. Its slot was just vacated.
                    let _ = self.retired_queues.lock().push_within_capacity(base);
                    return Ok(());
                }
            }
        }
    }

    /// Publish a context destruction without waiting, as growth replies are.
    /// Returns the control cursor that acknowledges it, or None when full.
    fn destroy_queue_context(&mut self, base: u64) -> Result<Option<u32>> {
        let context = base + 0x4140;
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        let ids = fw.read_u32(context)?;
        let fields = [
            (fw.read_u32(context + 0x24)? >> 16) as u8,
            ids as u8,
            (ids >> 8) as u8,
            fw.read_u32(context + 4)? as u8,
        ];
        let guard = self._sgx.try_access().ok_or(ENODEV)?;
        let head = guard.try_read32(0xd640c0)?;
        let ack = guard.try_read32(0xd640c8)?;
        let tail = guard.try_read32(0xd60060)?;
        drop(guard);
        if head >= 256 || ack >= 256 || tail >= 256 {
            return Err(EIO);
        }
        let successor = (tail + 1) & 255;
        if successor == head || successor == ack {
            return Ok(None);
        }
        let control = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS).channels[12].ring;
        fw.write_live(
            control + u64::from(tail) * 64,
            &g16_fw::destroy_context(context, fields),
        )?;
        crate::mem::sync();
        let guard = self._sgx.try_access().ok_or(ENODEV)?;
        guard.try_write32(successor, 0xd60060)?;
        drop(guard);
        crate::mem::sync();
        self._rtkit
            .as_mut()
            .send_message(0x21, 0x0084_0000_0000_0011)?;
        Ok(Some(successor))
    }

    /// Initialize three lanes in `base`, which is newly allocated or recycled.
    /// `identity` is a fresh address whose page index names the queues, so a
    /// recycled queue never reuses a destroyed queue's identity.
    fn init_queues(
        &mut self,
        owner: &FirmwareQueues,
        base: u64,
        identity: u64,
        fresh: bool,
    ) -> Result<[Lane; 3]> {
        use crate::pgtable::prot::{PROT_FW_PRIV_RW, PROT_FW_SHARED_RW};
        use g16_fw::queue as q;

        let lanes: [Lane; 3] = core::array::from_fn(|stage| {
            let queue = base + stage as u64 * 0xc000;
            Lane {
                queue,
                pointers: queue + 0x4000,
                ring: queue + 0x8000,
                head: 0,
                new: true,
            }
        });
        if fresh {
            // Reserve before the region exists, so it can always be retired.
            let total = self.queue_regions + 1;
            let mut retired = self.retired_queues.lock();
            let additional = total - retired.len();
            retired.reserve(additional, GFP_KERNEL)?;
            drop(retired);
            self.queue_regions = total;
        }
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        for lane in &lanes {
            if fresh {
                fw.alloc(lane.queue, 0x4000, PROT_FW_PRIV_RW)?;
                fw.alloc(lane.pointers, 0x8000, PROT_FW_SHARED_RW)?;
            } else {
                // Restore a new allocation's zeroed state, including the
                // firmware-private words after each queue descriptor.
                fw.zero_live(lane.queue, 0x4000)?;
                fw.zero_live(lane.pointers, 0x8000)?;
            }
        }
        // A notifier list and GPU context belong to the public queue,
        // shared only by its own vertex, fragment and compute lanes.
        let jobs = base + 0x4100;
        let context = base + 0x4140;
        for (stage, lane) in lanes.iter().enumerate() {
            let named = identity + stage as u64 * 0xc000;
            let mut descriptor = q::Queue {
                pointers: lane.pointers,
                ring: lane.ring,
                jobs,
                private: lane.queue + 0xb0,
                context,
                uuid: ((named - 0xffff_fc22_0000_0000) >> 14) as u32 + 2,
            }
            .encode_priority(owner.priority)
            .ok_or(EINVAL)?;
            // Native TA/3D profiles allow firmware to suspend and restart
            // renders. Their Work/scratch leases survive until the final
            // notifier/driver stamp, including intervening partial runs.
            // Compute remains non-preemptible, matching the M1/M2 driver.
            // Preemptible renders can wedge the firmware when a compressed
            // render is timesliced against other contexts (Dota 2 with two
            // compositors, 2026-10-06), so they are off by default.
            if stage == 2 || crate::module_parameters::render_preempt.value() == 0 {
                descriptor[0x30..0x34].copy_from_slice(&0u32.to_le_bytes());
                descriptor[0x48..0x4c].copy_from_slice(&1u32.to_le_bytes());
            }
            fw.write_live(lane.queue, &descriptor)?;
            fw.write_live(lane.pointers, &q::pointers(0x500, 0))?;
            // A newly created timeline starts immediately before its first
            // target. This also permits qualification near stamp rollover.
            let (ordinal, first) = match stage {
                0 => (owner.render_count, 0xc00),
                1 => (owner.render_count, 0x100),
                _ => (owner.compute_count, 0x100),
            };
            let current = g16_stamp::previous(g16_stamp::value(ordinal, first));
            fw.write_live(lane.firmware_stamp(), &current.to_le_bytes())?;
            if stage < 2 {
                fw.write_live(lane.driver_stamp(), &current.to_le_bytes())?;
            }
        }
        fw.write_live(jobs, &q::jobs(jobs))?;
        fw.write_live(context, &q::context())?;
        let notifier = render_notifier(&lanes);
        let count = render_notifier_count(&lanes);
        fw.write_live(notifier, &[0u8; 0x100])?;
        fw.write_live(notifier, &count.to_le_bytes())?;
        fw.write_live(notifier + 0x10, &0x50u32.to_le_bytes())?;
        fw.write_live(count, &0u32.to_le_bytes())?;
        fw.sync();
        Ok(lanes)
    }

    fn prepare_queues(&mut self, owner: &mut FirmwareQueues) -> Result {
        if owner.lanes.is_none() {
            self.recycle_queues()?;
            let identity = self.next_work_va;
            self.next_work_va = identity.checked_add(0x24000).ok_or(ENOSPC)?;
            let recycled = self.free_queues.pop();
            let base = recycled.unwrap_or(identity);
            let lanes = match self.init_queues(owner, base, identity, recycled.is_none()) {
                Ok(lanes) => lanes,
                Err(error) => {
                    // A failed fresh allocation keeps any partial regions
                    // owned, like a failed Work refill; recycled storage returns.
                    if let Some(base) = recycled {
                        let _ = self.free_queues.push_within_capacity(base);
                    }
                    return Err(error);
                }
            };
            owner.storage = Some(QueueStorage {
                base,
                retired: self.retired_queues.clone(),
            });
            owner.lanes = Some(lanes);
            owner.notified_works = 0;
            if crate::module_parameters::fw_trace.value() & 1 != 0 {
                dev_info!(
                    self.dev.as_ref(),
                    "G16: queues vertex={:#x} fragment={:#x} compute={:#x} priority={} recycled={}\n",
                    lanes[0].queue,
                    lanes[1].queue,
                    lanes[2].queue,
                    owner.priority,
                    recycled.is_some()
                );
            }
        }
        if owner.events.is_none() {
            // Bits 0-31 are the event indices in use. With event_affinity,
            // bits 32-36 are a next-fit cursor: a queue retakes the index it
            // last held, and otherwise takes the next free index after the
            // last one handed out, so an index a queue just released is not
            // immediately given to a different queue.
            let affinity = crate::module_parameters::event_affinity.value() != 0;
            let last = owner.last_event;
            let mut picked = 0u32;
            self.queue_events
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    let free = !(value as u32);
                    if free == 0 {
                        return None;
                    }
                    let cursor = ((value >> 32) & 31) as u32;
                    picked = if !affinity {
                        free.trailing_zeros()
                    } else if let Some(last) = last.filter(|&l| free & (1 << l) != 0) {
                        last
                    } else {
                        (free.rotate_right(cursor).trailing_zeros() + cursor) % 32
                    };
                    let cursor = if affinity { u64::from((picked + 1) % 32) } else { u64::from(cursor) };
                    Some((value & 0xffff_ffff) | (1 << picked) | (cursor << 32))
                })
                .map_err(|_| {
                    self.report_pressure(8, "events");
                    EBUSY
                })?;
            if crate::module_parameters::fw_trace.value() & 256 != 0 && owner.last_event != Some(picked) {
                pr_info!("G16: queue event index {:?} -> {}\n", owner.last_event, picked);
            }
            owner.last_event = Some(picked);
            if let Some(lanes) = owner.lanes {
                for (stage, lane) in lanes.iter().enumerate() {
                    self.set_lane_event(lane.firmware_stamp(), 32 + picked * 3 + stage as u32);
                }
            }
            owner.events = Some(QueueEvents {
                slots: self.queue_events.clone(),
                index: picked,
            });
        }
        Ok(())
    }

    fn reserve_flight(
        &mut self,
        queue: &FirmwareQueues,
        render: bool,
        root: u64,
        entries: [u32; 3],
    ) -> Result {
        if self.render_failed {
            return Err(EIO);
        }
        // Check scarce resources before allocating any per-Work storage.
        // Commands in either persistent VM root can share its live ASID.
        if !(1..64).any(|i| {
            self.context_users[i] == 0 || self.context_roots[i].is_some_and(|s| s.low == root)
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
            let staged = self
                .publications
                .iter()
                .filter(|p| p.channel == channel)
                .count() as u32;
            let coalesced = queue.lanes.as_ref().is_some_and(|lanes| {
                self.publications
                    .iter()
                    .any(|p| p.lane.queue == lanes[stage].queue)
            });
            if !ring_room(
                (tail + staged) & 255,
                read,
                done,
                u32::from(!coalesced),
                256,
            ) {
                self.report_pressure(2, "channel");
                return Err(EBUSY);
            }
            // Bound transport epochs, not the number of Works sharing each
            // epoch. A coalesced batch consumes only one channel-ring slot.
            let next = (tail + staged + u32::from(!coalesced)) & 255;
            if self.flights.iter().any(|f| {
                f.channels
                    .iter()
                    .any(|&(c, target)| c == channel && !reached(next, target, 256))
            }) {
                self.report_pressure(2, "channel");
                return Err(EBUSY);
            }
            if let Some(lanes) = &queue.lanes {
                let lane = lanes[stage];
                let fw = self._firmware_space.as_ref().ok_or(EIO)?;
                let read = fw.read_u32(lane.pointers)?;
                let done = fw.read_u32(lane.pointers + 0x30)?;
                if read >= 0x500 || done >= 0x500 || lane.head >= 0x500 {
                    return Err(EIO);
                }
                if !ring_room(lane.head, read, done, entries[stage], 0x500) {
                    self.report_pressure(16, "workqueue");
                    return Err(EBUSY);
                }
                // Retirement compares the firmware cursors with each flight's
                // head modulo the ring, which is only unambiguous within half
                // a ring. Like the channel epochs above, never let the head
                // run half a ring past an unretired flight on this lane, or
                // that flight's ring check stays false and it never retires.
                // The firmware reads up to 128 Works ahead on each lane. With
                // more unretired Works than that on a lane it stops completing
                // them (Dota 2 bursts: 127 in flight never stalled, 129 did).
                // This applies to every lane (render TA and 3D as well as
                // compute), so bound each well below the window, as Metal does
                // with at most 64 command buffers in flight per queue.
                let cap = crate::module_parameters::max_lane_works.value() as usize;
                if cap != 0
                    && self
                        .flights
                        .iter()
                        .filter(|f| f.queues.iter().any(|&(pointers, _)| pointers == lane.pointers))
                        .count()
                        >= cap
                {
                    self.report_pressure(16, "workqueue");
                    return Err(EBUSY);
                }
                let next = (lane.head + entries[stage]) % 0x500;
                if self.flights.iter().any(|f| {
                    f.queues
                        .iter()
                        .any(|&(pointers, head)| pointers == lane.pointers && !reached(next, head, 0x500))
                }) {
                    self.report_pressure(16, "workqueue");
                    return Err(EBUSY);
                }
            }
        }
        drop(guard);
        self.flights.reserve(1, GFP_KERNEL)?;
        self.publications.reserve(stages.len(), GFP_KERNEL)?;
        Ok(())
    }

    /// Stage a queue head; multiple Works share one transport message.
    fn stage_queue(
        &mut self,
        channel: usize,
        lane: Lane,
        event: u32,
        sequence: u64,
    ) -> Result<u32> {
        if let Some(p) = self
            .publications
            .iter_mut()
            .find(|p| p.lane.queue == lane.queue)
        {
            p.lane.head = lane.head;
            p.sequence = sequence;
            return Ok(p.epoch);
        }
        let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
        let route = &config.channels[channel];
        let guard = self._sgx.try_access().ok_or(ENODEV)?;
        let tail =
            guard.try_read32(0xd60000 + (route.state[2] - 0xffff_fc20_0002_0000) as usize)?;
        if tail >= 256 {
            return Err(EIO);
        }
        drop(guard);
        let reserved = self
            .publications
            .iter()
            .filter(|p| p.channel == channel)
            .count() as u32;
        let epoch = (tail + reserved) & 255;
        self.publications.push(
            Publication {
                channel,
                epoch,
                lane,
                event,
                sequence,
            },
            GFP_KERNEL,
        )?;
        Ok(epoch)
    }

    /// Publish fully prepared rings together, then kick each used engine once
    /// per priority. No staged Work may remain when the worker sleeps.
    pub(crate) fn flush(&mut self) -> Result {
        if self.publications.is_empty() {
            return Ok(());
        }
        self.render_failed = true;
        let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        let mut tails = [None; 12];
        let mut kicks = 0u32;
        // Fragment transports must precede tiling, matching normal publication.
        for stage in [1usize, 0, 2] {
            for p in self.publications.iter().filter(|p| p.channel % 3 == stage) {
                let route = &config.channels[p.channel];
                fw.write_live(p.lane.pointers + 0x40, &p.lane.head.to_le_bytes())?;
                fw.write_live(
                    route.ring + u64::from(p.epoch) * 0x18,
                    &g16_fw::queue::channel(
                        p.lane.queue,
                        p.lane.head as u16,
                        p.event as u8,
                        stage as u32,
                        p.lane.new,
                        p.sequence,
                    ),
                )?;
                tails[p.channel] = Some((p.epoch + 1) & 255);
                kicks |= 1 << ((p.channel / 3) * 2 + usize::from(stage == 2));
            }
        }
        crate::mem::sync();
        let guard = self._sgx.try_access().ok_or(ENODEV)?;
        for stage in [1usize, 0, 2] {
            for channel in (stage..12).step_by(3) {
                if let Some(tail) = tails[channel] {
                    let route = &config.channels[channel];
                    guard.try_write32(
                        tail,
                        0xd60000 + (route.state[2] - 0xffff_fc20_0002_0000) as usize,
                    )?;
                }
            }
        }
        crate::mem::sync();
        drop(guard);
        for bit in 0..8 {
            if kicks & (1 << bit) != 0 {
                let priority = bit / 2;
                let engine = (bit % 2) * 2;
                self._rtkit.as_mut().send_message(
                    0x21,
                    0x0083_0000_0000_0000 | ((priority as u64) << 2) | engine as u64,
                )?;
            }
        }
        if crate::module_parameters::fw_trace.value() & 4 != 0 {
            dev_info!(
                self.dev.as_ref(),
                "G16: batch works={} messages={} kicks={}\n",
                self.batch_works,
                self.publications.len(),
                kicks.count_ones()
            );
        }
        self.publications.clear();
        self.batch_works = 0;
        self.render_failed = false;
        Ok(())
    }

    pub(crate) fn poison(&mut self) {
        self.render_failed = true;
    }

    fn stamps_done(&self, flight: &Flight) -> Result<bool> {
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        for stamp in flight.stamps.iter().flatten() {
            if !g16_stamp::reached(fw.read_u32(stamp.address)?, stamp.value) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn trace_publication(&self, kind: &str, id: u64) -> Result {
        if crate::module_parameters::fw_trace.value() & 1 != 0 {
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

    /// Optional qualification trace. Readiness is observed only for diagnostics;
    /// it does not decide whether a dependent Work can enter the firmware ring.
    fn trace_dependencies(
        &self,
        kind: &str,
        id: u64,
        context: u32,
        dependencies: &[Option<Stamp>; 2],
    ) -> Result {
        if crate::module_parameters::fw_trace.value() & 8 != 0 {
            let fw = self._firmware_space.as_ref().ok_or(EIO)?;
            let mut unfinished = 0;
            for dep in dependencies.iter().flatten() {
                unfinished += usize::from(!g16_stamp::reached(fw.read_u32(dep.address)?, dep.value));
            }
            let flight = self.flights.iter().find(|flight| flight.id == id).ok_or(EIO)?;
            let mut driver_values = [0; 2];
            for (index, stamp) in flight.driver_stamps.iter().enumerate() {
                if let Some(stamp) = stamp {
                    driver_values[index] = fw.read_u32(stamp.address)?;
                }
            }
            dev_info!(
                self.dev.as_ref(),
                "G16: staged {} {:#x} context={} dependencies={} unfinished={} stamps={:#x}/{:#x} driver_stamps={:#x}/{:#x}\n",
                kind,
                id,
                context,
                dependencies.iter().flatten().count(),
                unfinished,
                flight.stamps[0].map_or(0, |stamp| stamp.value),
                flight.stamps[1].map_or(0, |stamp| stamp.value),
                driver_values[0],
                driver_values[1]
            );
        }
        Ok(())
    }

    /// Preserve the event and unfinished queue identities before poisoning the
    /// device. Firmware timeout events name a stamp slot, which can be shared
    /// by several in-flight Works from the same userspace queue.
    /// Hang diagnostics: GPU fault registers, every pending flight's
    /// firmware and driver stamps against their targets, and the newest
    /// firmware receive-ring entries.
    fn dump_hang_state(&self) -> Result {
        // Device-wide transports: whether firmware consumed each channel's
        // messages (read/done) up to what the driver published (tail).
        {
            let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
            if let Some(guard) = self._sgx.try_access() {
                for (index, route) in config.channels.iter().enumerate().take(12) {
                    let read = guard
                        .try_read32(0xd64000 + (route.state[0] - 0xffff_fc20_0002_8000) as usize)
                        .unwrap_or(u32::MAX);
                    let done = guard
                        .try_read32(0xd64000 + (route.state[1] - 0xffff_fc20_0002_8000) as usize)
                        .unwrap_or(u32::MAX);
                    let tail = guard
                        .try_read32(0xd60000 + (route.state[2] - 0xffff_fc20_0002_0000) as usize)
                        .unwrap_or(u32::MAX);
                    dev_err!(
                        self.dev.as_ref(),
                        "G16: hang channel {} (priority {} stage {}) read={} done={} tail={}\n",
                        index,
                        index / 3,
                        index % 3,
                        read,
                        done,
                        tail
                    );
                }
            }
        }
        if let Some(guard) = self._sgx.try_access() {
            let read64 = |offset: usize| -> u64 {
                let lo = guard.try_read32(offset).unwrap_or(0xdeadbeef);
                let hi = guard.try_read32(offset + 4).unwrap_or(0xdeadbeef);
                u64::from(lo) | (u64::from(hi) << 32)
            };
            let info = read64(0xd8c0);
            let addr = read64(0xd8c8);
            dev_err!(self.dev.as_ref(), "G16: fault info d8c0={:#018x} d8c8={:#018x} 17030={:#018x}\n",
                     info, addr, read64(0x17030));
            // Field layout as on G14X (regs.rs get_fault_info); not yet
            // validated on G16 beyond the VM slot matching the hung context.
            if info & 1 != 0 {
                let access = if info & (1 << 4) != 0 { "read" } else { "write" };
                dev_err!(self.dev.as_ref(),
                         "G16: fault addr={:#x} {} reason={} unit={:#04x} vm_slot={} level={} sideband={:#04x}\n",
                         addr << 6, access,
                         (info >> 1) & 7, (info >> 9) & 0xff, (info >> 17) & 0x3f,
                         (info >> 7) & 3, (info >> 23) & 0x7f);
            }
        }
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        for status in [0xffff_fc20_0003_f180u64, 0xffff_fc20_0003_0000] {
            dev_err!(self.dev.as_ref(), "G16: status {:#x}: {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x}\n",
                     status, fw.read_u64(status)?, fw.read_u64(status + 8)?, fw.read_u64(status + 0x10)?,
                     fw.read_u64(status + 0x18)?, fw.read_u64(status + 0x20)?, fw.read_u64(status + 0x28)?,
                     fw.read_u64(status + 0x30)?, fw.read_u64(status + 0x38)?);
        }
        // Never let one unreadable word end the report early.
        let word = |address: u64| fw.read_u32(address).unwrap_or(0xdead_beef);
        if let Some(flight) = self.flights.first() {
            for (stage, (pointers, head)) in flight.queues.iter().enumerate() {
                if *pointers == 0 {
                    continue;
                }
                // Ring words +0x0/+0x30/+0x40; on G13 these are the GPU done
                // pointer, GPU read pointer and CPU write pointer.
                dev_err!(self.dev.as_ref(), "G16: hang queue stage {} ring+0={} ring+0x30={} ring+0x40={} flight head={}\n",
                         stage, word(*pointers), word(*pointers + 0x30),
                         word(*pointers + 0x40), head);
            }
            if let Some(render) = &flight.render {
                let a = &render.addresses;
                dev_err!(self.dev.as_ref(), "G16: hang render TA={:#x} 3D={:#x} scene={} context={}\n",
                         a.tiling_work, a.fragment_work, render.scene, a.context_id);
                let mut line = KVec::new();
                for i in 0..0x600u64 / 12 {
                    let at = a.fragment_work + 0x80 + i * 12;
                    let number = fw.read_u32(at)?;
                    let value = u64::from(fw.read_u32(at + 4)?) | (u64::from(fw.read_u32(at + 8)?) << 32);
                    if number == 0 && value == 0 {
                        break;
                    }
                    line.push((number, value), GFP_KERNEL)?;
                    if line.len() == 6 {
                        dev_err!(self.dev.as_ref(), "G16: hang 3D regs {:x?}\n", &line[..]);
                        line.clear();
                    }
                }
                if !line.is_empty() {
                    dev_err!(self.dev.as_ref(), "G16: hang 3D regs {:x?}\n", &line[..]);
                }
            }
        }
        self.print_render_rings();
        self.print_reuse_history();
        self.print_lane_events();
        for flight in self.flights.iter().take(24) {
            // blocked: bit 1 notifier, 2 ring, 3 channel, 4 control reply.
            dev_err!(self.dev.as_ref(), "G16: hang flight Work={:#x} context={} age_ms={} render={} stream={:#x} ordinal={} blocked={:#x} notifier={:?} tgid={} comm={}\n",
                     flight.id, flight.context, flight.started.elapsed().as_millis(),
                     flight.render.is_some(), flight.stream, flight.ordinal, flight.blocked,
                     flight.notifier_count.map(|(addr, total)| (word(addr), total)), flight.tgid,
                     comm_str(&flight.comm));
            for (stage, stamp) in flight.stamps.iter().enumerate() {
                if let Some(stamp) = stamp {
                    dev_err!(self.dev.as_ref(), "G16:   stage {} fw stamp {:#x}={:#x} want {:#x}\n",
                             stage, stamp.address, word(stamp.address), stamp.value);
                }
            }
            for stamp in flight.driver_stamps.iter().flatten() {
                dev_err!(self.dev.as_ref(), "G16:   driver stamp {:#x}={:#x} want {:#x}\n",
                         stamp.address, word(stamp.address), stamp.value);
            }
            for &(pointers, head) in flight.queues.iter().filter(|q| q.0 != 0) {
                dev_err!(self.dev.as_ref(), "G16:   ring {:#x} +0={} +0x30={} +0x40={} head={}\n",
                         pointers, word(pointers), word(pointers + 0x30),
                         word(pointers + 0x40), head);
            }
            for &(channel, target) in flight.channels.iter().filter(|c| c.0 != usize::MAX) {
                dev_err!(self.dev.as_ref(), "G16:   channel {} target {}\n", channel, target);
            }
            for (kind, dep) in flight.dependencies.iter().enumerate() {
                if let Some(dep) = dep {
                    let stage = if kind == 0 { "render" } else { "compute" };
                    dev_err!(self.dev.as_ref(), "G16:   waits on {} stamp {:#x}={:#x} want {:#x} event {}\n",
                             stage, dep.address, word(dep.address), dep.value, dep.event);
                }
            }
        }
        // Newest firmware trace-ring entries (512 x 0x48 at 0xe5c40).
        let write = fw.read_u32(0xffff_fc20_0003_f3c0 + 0x20)?;
        for back in 1..=6u32 {
            let index = (write + 512 - back) % 512;
            let base = 0xffff_fc20_000e_5c40 + u64::from(index) * 0x48;
            dev_err!(self.dev.as_ref(), "G16: trace[{}] {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x}\n",
                     index, fw.read_u64(base)?, fw.read_u64(base + 8)?, fw.read_u64(base + 0x10)?,
                     fw.read_u64(base + 0x18)?, fw.read_u64(base + 0x20)?, fw.read_u64(base + 0x28)?);
        }
        Ok(())
    }

    fn report_failed_event(&self, base: u64, kind: u32, cursor: u32) -> Result {
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        dev_err!(self.dev.as_ref(), "G16: failed Event kind={} cursor={} flights={}\n",
                 kind, cursor, self.flights.len());
        for offset in (0..0x48).step_by(8) {
            dev_err!(self.dev.as_ref(), "G16: Event +{:#x}={:#018x}\n",
                     offset, fw.read_u64(base + offset)?);
        }
        let _ = self.dump_hang_state();
        for flight in self.flights.iter().take(16) {
            dev_err!(self.dev.as_ref(), "G16: pending Work={:#x} context={} age_ms={} render={}\n",
                     flight.id, flight.context, flight.started.elapsed().as_millis(),
                     flight.render.is_some());
            for (stage, stamp) in flight.stamps.iter().enumerate() {
                if let Some(stamp) = stamp {
                    let (pointers, head) = flight.queues[stage];
                    dev_err!(self.dev.as_ref(),
                             "G16: stage={} event={} stamp={:#x} actual={:#x} expected={:#x} pointers={:#x} read={} done={} target={}\n",
                             stage, stamp.event, stamp.address, fw.read_u32(stamp.address)?,
                             stamp.value, pointers, fw.read_u32(pointers)?,
                             fw.read_u32(pointers + 0x30)?, head);
                }
            }
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
            if kind == 0 || kind == 4 {
                // Firmware fault (0) or timeout (4) report, as upstream's
                // EventMsg::Fault/Timeout. Upstream fails the pending work and
                // resumes the halted firmware; the G16 resume handshake is not
                // known yet, so nothing can continue after one of these.
                let what = if kind == 0 { "fault" } else { "timeout" };
                dev_err!(self.dev.as_ref(), "G16: firmware {} event\n", what);
                self.report_failed_event(base, kind, cursor)?;
                return Err(EIO);
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
            let Some(index) = selected else {
                self.report_failed_event(base, kind, cursor)?;
                return Err(EIO);
            };
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
        // One scan retires every finished flight; later calls return them
        // without rereading firmware state for each one.
        if let Some((id, limited)) = self.retired.pop() {
            return Ok(Some((id, if limited { Err(ENOMEM) } else { Ok(()) })));
        }
        self.service_flights().inspect_err(|error| {
            dev_err!(
                self.dev.as_ref(),
                "G16: firmware event service failed: {:?}\n",
                error
            );
        })?;
        self.collect_destroyed_queues()?;
        let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
        // Device-wide transport state is the same for every flight: read each
        // register once per scan instead of once per flight.
        let mut channel_state: [Option<[u32; 2]>; 12] = [None; 12];
        let control_replied = {
            let guard = self._sgx.try_access().ok_or(ENODEV)?;
            let tail = guard.try_read32(0xd60060)?;
            guard.try_read32(0xd640c0)? == tail && guard.try_read32(0xd640c8)? == tail
        };
        self.retired.reserve(self.flights.len(), GFP_KERNEL)?;
        let mut index = self.flights.len();
        while index > 0 {
            index -= 1;
            let flight = &self.flights[index];
            if !self.stamps_done(flight)? {
                continue;
            }
            let fw = self._firmware_space.as_ref().ok_or(EIO)?;
            let mut done = true;
            let mut blocked = 0u32;
            // The notifier pass drops embedded JobMeta references and cleans
            // their cache lines before publishing these later driver stamps.
            for stamp in flight.driver_stamps.iter().flatten() {
                done &= g16_stamp::reached(fw.read_u32(stamp.address)?, stamp.value);
            }
            blocked |= u32::from(!done);
            // A reusable Work has its own notifier and exact submitted count.
            // Removal must be visible before that storage can change queues.
            if done {
                let notified = match flight.notifier_count {
                    // The shared notifier has counted this render's Works.
                    Some((completed, total)) => {
                        (fw.read_u32(completed)?.wrapping_sub(total) as i32) >= 0
                    }
                    None => {
                        let notifier = flight.id
                            + if flight.render.is_some() {
                                0x200
                            } else {
                                0x1400
                            };
                        fw.read_u32(notifier + 0x94)? == 0
                    }
                };
                blocked |= u32::from(!notified) << 1;
                done &= notified;
            }
            if done {
                for &(pointers, head) in &flight.queues {
                    if pointers != 0 {
                        let lane = reached(fw.read_u32(pointers)?, head, 0x500)
                            && reached(fw.read_u32(pointers + 0x30)?, head, 0x500);
                        blocked |= u32::from(!lane) << 2;
                        done &= lane;
                    }
                }
            }
            if done {
                for &(channel, target) in &flight.channels {
                    if channel == usize::MAX {
                        continue;
                    }
                    let state = match channel_state.get(channel).copied().flatten() {
                        Some(state) => state,
                        None => {
                            let guard = self._sgx.try_access().ok_or(ENODEV)?;
                            let mut state = [0; 2];
                            for (value, address) in
                                state.iter_mut().zip(&config.channels[channel].state[..2])
                            {
                                *value = guard.try_read32(
                                    0xd64000 + (*address - 0xffff_fc20_0002_8000) as usize,
                                )?;
                            }
                            if let Some(slot) = channel_state.get_mut(channel) {
                                *slot = Some(state);
                            }
                            state
                        }
                    };
                    for value in state {
                        let consumed = reached(value, target, 256);
                        blocked |= u32::from(!consumed) << 3;
                        done &= consumed;
                    }
                }
            }
            // Growth replies can refer to a Work after its main stage stamp.
            if flight.render.is_some() {
                blocked |= u32::from(!control_replied) << 4;
                done &= control_replied;
            }
            {
                let flight = &mut self.flights[index];
                if flight.stamps_seen.is_none() {
                    flight.stamps_seen = Some(Instant::now());
                }
                if !done {
                    flight.blocked = blocked;
                }
            }
            if !done {
                continue;
            }
            let flight = &self.flights[index];
            if crate::module_parameters::fw_trace.value() & 16 != 0 {
                dev_info!(
                    self.dev.as_ref(),
                    "G16: retire {:#x} render={} after_stamps_us={} last_blocked={:#x} flights={}\n",
                    flight.id,
                    flight.render.is_some(),
                    flight.stamps_seen.map_or(0, |t| t.elapsed().as_micros_ceil()),
                    flight.blocked,
                    self.flights.len()
                );
            }
            let limited = flight
                .render
                .as_ref()
                .is_some_and(|r| r.events.memory_limit);
            let id = flight.id;
            let render_work = flight.render.is_some();
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
            if owner.pending == 0 && crate::module_parameters::event_affinity.value() < 2 {
                owner.events = None;
            }
            drop(owner);
            self.context_users[flight.context as usize] -= 1;
            let flight = self.flights.remove(index).map_err(|_| EIO)?;
            let root = match &flight.render {
                Some(render) => render.root,
                None => flight.client.lock().roots().low,
            };
            let entry = Retired {
                work: id,
                context: flight.context,
                root,
                tgid: flight.tgid,
                comm: flight.comm,
                ordinal: flight.ordinal,
                render: render_work,
                submitted: flight.started,
                retired: Instant::now(),
            };
            // Retirement must not allocate: capacity is reserved up front.
            if self.retire_log.len() < RETIRE_LOG {
                let _ = self.retire_log.push_within_capacity(entry);
            } else {
                self.retire_log[self.retire_next % RETIRE_LOG] = entry;
            }
            self.retire_next = self.retire_next.wrapping_add(1);
            flight.client.lock().release_scratch(flight.scratch)?;
            self.free_work[usize::from(render_work)].push(id, GFP_KERNEL)?;
            self.retired.push_within_capacity((id, limited)).map_err(|_| ENOMEM)?;
        }
        if !self.retired.is_empty() {
            // Queued commands can legitimately wait longer than one Work's
            // timeout. Watch for a lack of retirement progress, rather than
            // charging every command for all its predecessors' execution.
            let now = Instant::now();
            for pending in &mut self.flights {
                pending.started = now;
            }
            let (id, limited) = self.retired.pop().ok_or(EIO)?;
            return Ok(Some((id, if limited { Err(ENOMEM) } else { Ok(()) })));
        }
        self.check_wedge()?;
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
            let _ = self.dump_hang_state();
            self.render_failed = true;
            return Err(ETIMEDOUT);
        }
        Ok(None)
    }

    /// Notifications do not extend the watchdog. Only retirement progress
    /// resets it, so trace traffic cannot keep a stuck command alive.
    pub(crate) fn watchdog_remaining_ms(&self) -> u32 {
        let watchdog = self
            .flights
            .iter()
            .map(|flight| (10000 - flight.started.elapsed().as_millis()).clamp(1, 10000) as u32)
            .min()
            .unwrap_or(10000);
        // Wedge detection samples render progress a few times per window.
        // A recorded unreachable hold is re-checked within its grace second.
        if self.flights.iter().any(|f| f.render.is_some()) {
            watchdog.min(250)
        } else if self.hold_key.load(Ordering::Relaxed) != 0 {
            watchdog.min(500)
        } else {
            watchdog
        }
    }
}

/// A per-VM firmware support object retained by every queued execution until
/// its GPU accesses finish, including after public VM/queue handle destruction.
pub(crate) struct Support {
    slots: Arc<AtomicU64>,
    index: u32,
    submissions: AtomicU64,
}
impl Support {
    pub(crate) fn address(&self) -> u64 {
        render::SUPPORT_BASE + u64::from(self.index) * 0x4000
    }

    pub(crate) fn shared_state(&self) -> u64 {
        render::SUPPORT_SHARED_BASE + u64::from(self.index) * 0x4000
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
            submissions: AtomicU64::new(
                crate::module_parameters::fw_counter_start.value().wrapping_mul(2),
            ),
        };
        let mut a = render::Addresses::bootstrap();
        a.support = lease.address();
        a.render_shared_state = lease.shared_state();
        let mut page = KVec::new();
        page.extend_with(0x4000, 0u8, GFP_KERNEL)?;
        page[..0x100].copy_from_slice(&pool.support(3 + u64::from(index), a.render_shared_state));
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        fw.write_live(a.support, &page)?;
        // A reused lease starts its submitted-Work count afresh.
        fw.zero_live(a.render_shared_state, 0x4000)?;
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
        if let Some(next) = roots {
            self.handovers[index] = Some(Handover {
                at: Instant::now(),
                previous: self.context_roots[index].map_or(0, |r| r.low),
                next: next.low,
            });
        }
        self.context_roots[index] = roots;
        Ok(())
    }

    /// Invalidate only slots actually bound to this root. New roots need no
    /// shootdown: set_context invalidates the ASID before installing them.
    pub(crate) fn invalidate_client(&self, client: &g16_vm::AddressSpace) {
        let root = client.roots().low;
        if self
            ._address_space
            .as_ref()
            .is_some_and(|s| s.roots().low == root)
        {
            client.low.invalidate(Some(0));
            client.high.invalidate(Some(0));
        }
        for (index, roots) in self.context_roots.iter().enumerate().skip(1) {
            if roots.is_some_and(|r| r.low == root) {
                client.low.invalidate(Some(index as u8));
                client.high.invalidate(Some(index as u8));
            }
        }
        crate::mem::sync();
        client.low.clear_invalidations();
        client.high.clear_invalidations();
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
            let previous = self.context_roots[slot].map(|r| r.low);
            self.set_context(slot, Some(roots))?;
            if crate::module_parameters::debug_ctx.value() != 0 {
                pr_info!(
                    "G16: context {} bound to root {:#x} (previous {:#x})\n",
                    slot,
                    roots.low,
                    previous.unwrap_or(0)
                );
            }
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
            // The pool is three regions: control pages, the shared counter
            // and the block and page lists (see Tvb::new).
            fw.release_region(a.buffer_manager)?;
            fw.release_region(a.buffer_manager_counter)?;
            fw.release_region(a.buffer_manager_block_list)?;
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
            if read != write && crate::module_parameters::fw_trace.value() & 64 != 0 {
                // SAFETY: ktime_get() has no preconditions.
                let now = unsafe { kernel::bindings::ktime_get() };
                let entry = if state == 0xffff_fc20_0003_f3c0 {
                    0xffff_fc20_000e_5c40 + u64::from(read) * 0x48
                } else {
                    0
                };
                let (w0, w1) = if entry != 0 {
                    (fw.read_u32(entry)?, fw.read_u32(entry + 4)?)
                } else {
                    (0, 0)
                };
                pr_info!(
                    "G16: rxring {} {:#x} {}..{} first={:#x}/{:#x}\n",
                    now, state, read, write, w0, w1
                );
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
    /// Experiment (render_deps=1): order a render against earlier work in the
    /// kernel instead of with firmware dependency items, as for compute. A
    /// render waiting on this queue's own fragment lane from its fragment
    /// stage (VDM_BARRIER_FRAGMENT) is ordered by that lane. Every other
    /// awaited stamp, including the previous render's fragment stamp that a
    /// tiling stage must wait for when the render reads earlier fragment
    /// output, must be reached before admission; until then return EBUSY and
    /// let the worker retry after the next completion. The render then
    /// reaches the firmware with no dependency items; its internal TA-to-3D
    /// link is unchanged. render_deps=0 keeps the items.
    fn resolve_render_dependencies(
        &self,
        queue: &mut FirmwareQueues,
        params: &render::Parameters,
        dependencies: &[Option<Stamp>; 2],
    ) -> Result<[Option<Stamp>; 2]> {
        if crate::module_parameters::render_deps.value() == 0 {
            return Ok(*dependencies);
        }
        let fragment_stage = params.vdm_barrier_fragment
            || crate::module_parameters::render_barrier.value() & 1 != 0;
        let own_fragment = queue.lanes.as_ref().map(|lanes| lanes[1].firmware_stamp());
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        let mut ta_ordered = false;
        let mut lane_dropped = false;
        for (slot, dep) in dependencies.iter().enumerate() {
            let Some(dep) = dep else { continue };
            // Slot 0 is the render dependency: on the fragment stage with a
            // fragment barrier, else on the tiling stage. Slot 1 (compute) is
            // always a tiling-stage wait.
            let lane_ordered = slot == 0 && fragment_stage && own_fragment == Some(dep.address);
            let ta_wait = slot == 0 && !fragment_stage && own_fragment == Some(dep.address);
            let now = if lane_ordered { 0 } else { fw.read_u32(dep.address)? };
            let done = lane_ordered || g16_stamp::reached(now, dep.value);
            if !lane_ordered {
                self.hold_check("render", dep, now, done)?;
            }
            if !done {
                if ta_wait {
                    queue.deps.ta_held += 1;
                } else {
                    queue.deps.cross_held += 1;
                }
                queue.deps.defer();
                return Err(EBUSY);
            }
            ta_ordered |= ta_wait;
            lane_dropped |= lane_ordered;
        }
        queue.deps.ta_ordered += u64::from(ta_ordered);
        queue.deps.lane_dropped += u64::from(lane_dropped);
        queue.deps.admit();
        Ok([None; 2])
    }

    pub(crate) fn submit_render(
        &mut self,
        queues: Arc<Mutex<FirmwareQueues>>,
        owner: Arc<Mutex<g16_vm::AddressSpace>>,
        params: &render::Parameters,
        support: &Support,
        timestamps: [u64; 4],
        dependencies: &[Option<Stamp>; 2],
    ) -> Result<Receipt> {
        let mut queue = queues.lock();
        let resolved = self.resolve_render_dependencies(&mut queue, params, dependencies)?;
        let dependencies = &resolved;
        let root = owner.lock().roots().low;
        let initbm = !self
            .tvb_pools
            .iter()
            .any(|p| p.root == root && p.initialized);
        self.reserve_flight(
            &queue,
            true,
            root,
            [
                1 + u32::from(initbm) + dependencies.iter().flatten().count() as u32,
                2,
                0,
            ],
        )?;
        self.prepare_queues(&mut queue)?;
        let result = self.submit_render_queue(
            &mut queue,
            queues.clone(),
            owner,
            params,
            support,
            timestamps,
            dependencies,
        );
        if queue.pending == 0 && crate::module_parameters::event_affinity.value() < 2 {
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
        dependencies: &[Option<Stamp>; 2],
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
            dependencies,
        );
        if result.is_err() && !self.render_failed {
            pool.release_scene(scene);
        }
        self.tvb_pools.push(pool, GFP_KERNEL)?;
        result
    }

    #[inline(never)]
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
        dependencies: &[Option<Stamp>; 2],
    ) -> Result<Receipt> {
        if self.render_failed {
            return Err(EIO);
        }
        if !params.valid() || !(2..=3).contains(&queue.priority) {
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
        let scratch = client.prepare_render(&mut params)?;
        let mut a = render::Addresses::bootstrap().publication(self.render_publication);
        pool.work_addresses(&mut a, scene);
        // Keep profiling storage private to the Work. Public timestamp BOs
        // are written by the firmware through its separate user destinations.
        let lanes = queue.lanes.as_ref().ok_or(EIO)?;
        a.tiling_queue = lanes[0].queue;
        a.fragment_queue = lanes[1].queue;
        a.tiling_event = u64::from(queue.event(0)?);
        a.fragment_event = u64::from(queue.event(1)?);
        a.support = support.address();
        a.render_shared_state = support.shared_state();
        a.tiling_status_page = params.ta_status & !0x3fff;
        a.fragment_status_page = params.fragment_status & !0x3fff;
        let queue_ordinal = queue.render_count;
        if crate::module_parameters::work_layout.value() & 8 != 0 && queue_ordinal % 40 != 0 {
            // As macOS: only the first render of each 40-render cycle uses
            // 0xd5 in this fragment register; the others use 0xb5.
            params.fragment_gate = 0xb5_0040_0020;
        }
        a.fragment_counter = queue_ordinal.wrapping_mul(2);
        a.tiling_counter = a.fragment_counter.wrapping_add(1);
        // Firmware's dependency graph requires consecutive stamp updates
        // within each lane, even when other queues publish between them.
        a.fragment_stamp = u64::from(g16_stamp::value(queue_ordinal, 0x100));
        a.tiling_stamp = u64::from(g16_stamp::value(queue_ordinal, 0xc00));
        let va = self.allocate_work(true)?;
        a.tiling_shared_tail = va + 0x10000;
        a.fragment_shared_tail = va + 0x14000;
        // The notifier pass stores driver stamps, then sends the IRQ after
        // a DSB. These destinations must be firmware-uncached: a DSB does
        // not clean a cached stamp, leaving Linux asleep on stale memory.
        let shared = shared_notifier();
        if shared {
            // One notifier and one stamp per lane for the whole queue, as
            // macOS does: the notifier pass then does not walk each Work.
            a.event_control = render_notifier(lanes);
            a.event_count_array = render_notifier_count(lanes);
            a.tiling_driver_stamp = lanes[0].driver_stamp();
            a.fragment_driver_stamp = lanes[1].driver_stamp();
        } else {
            a.event_control = va + 0x200;
            a.event_count_array = va + 0x300;
            a.tiling_driver_stamp = va + 0x18000;
            a.fragment_driver_stamp = va + 0x18040;
        }
        a.tiling_firmware_stamp = lanes[0].firmware_stamp();
        a.fragment_firmware_stamp = lanes[1].firmware_stamp();
        let alias = 0x72_0000_0000 + (va - 0xffff_fc22_0000_0000);
        if alias + 0x8000 > 0x73_0000_0000 {
            return Err(ENOSPC);
        }
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        for stage in 0..2u64 {
            let work_page = va + stage * 0x8000;
            let register_page = alias + stage * 0x4000;
            let pa = fw.physical(work_page)?;
            for view in [self._address_space.as_mut().ok_or(EIO)?, &mut *client] {
                view.map_work_alias(register_page, pa)?;
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
        self.invalidate_client(self._address_space.as_ref().ok_or(EIO)?);
        self.invalidate_client(client);
        // A live context lease keeps these roots fixed through retirement.
        a.context_id = u64::from(self.acquire_context(space, 9)?);
        a.tiling_user_timestamp_start = timestamps[0];
        a.tiling_user_timestamp_end = timestamps[1];
        a.fragment_user_timestamp_start = timestamps[2];
        a.fragment_user_timestamp_end = timestamps[3];
        // From here any publication failure quarantines the roots.
        self.render_failed = true;
        // Heap buffers: two copies of each encoding on this frame would
        // exceed the kernel stack together with the job worker's own frame.
        let mut ta = KVec::new();
        ta.extend_with(0x940, 0u8, GFP_KERNEL)?;
        let mut frag = KVec::new();
        frag.extend_with(0xcc0, 0u8, GFP_KERNEL)?;
        params.tiling_work(&a, &mut ta).ok_or(EINVAL)?;
        params.fragment_work(&a, &mut frag).ok_or(EINVAL)?;
        // Keep firmware's internal profiling destinations distinct from the
        // explicit user timestamp slots used for UAPI completion above.
        if crate::module_parameters::work_timestamps.value() != 0 {
            ta[0x8c8..0x8d0].copy_from_slice(&(va + 0x100).to_le_bytes());
            ta[0x8d0..0x8d8].copy_from_slice(&(va + 0x108).to_le_bytes());
            frag[0xc10..0xc18].copy_from_slice(&(va + 0x110).to_le_bytes());
            frag[0xc18..0xc20].copy_from_slice(&(va + 0x118).to_le_bytes());
        } else {
            // As macOS: no per-Work start or tiling-end profiling stamps; the
            // fragment end stamp goes to one location shared by the queue.
            ta[0x8c8..0x8d8].fill(0);
            frag[0xc10..0xc18].fill(0);
            frag[0xc18..0xc20].copy_from_slice(&(lanes[1].pointers + 0x2080).to_le_bytes());
        }
        if crate::module_parameters::fw_trace.value() & 128 != 0
            && !CONFIG_DUMPED.swap(true, Ordering::Relaxed)
        {
            // Live firmware configuration, for comparison with macOS.
            let fw = self._firmware_space.as_ref().ok_or(EIO)?;
            for (base, size) in [
                (0xffff_fc20_0010_0000u64, 0x4000u64),
                (0xffff_fc20_0010_8000, 0x4000),
                (0xffff_fc20_000f_8000, 0x4000),
                (0xffff_fc20_c04d_8000, 0x20000),
                (0xffff_fc20_0003_e000, 0x2000),
            ] {
                for line in (base..base + size).step_by(32) {
                    let words = [
                        fw.read_u64(line)?,
                        fw.read_u64(line + 8)?,
                        fw.read_u64(line + 16)?,
                        fw.read_u64(line + 24)?,
                    ];
                    if words != [0; 4] {
                        pr_info!(
                            "G16: cfgdump {:#x}: {:016x} {:016x} {:016x} {:016x}\n",
                            line, words[0], words[1], words[2], words[3]
                        );
                    }
                }
            }
        }
        let layout = crate::module_parameters::work_layout.value();
        if layout & 1 != 0 {
            // As macOS in steady state: no TVB growth request and neither
            // fragment flag that this driver set unconditionally.
            ta[0x864] = 0;
            frag[0xba8..0xbac].fill(0);
            frag[0xbdc..0xbe0].fill(0);
        }
        if layout & 2 != 0 && params.layers == 1 {
            // As macOS: a render index modulo 40, and modulo 4 in the top
            // bits of the support-page operands.
            let index = queue_ordinal % 40;
            ta[0x8a0..0x8a8].copy_from_slice(&index.to_le_bytes());
            frag[0xbe8..0xbf0].copy_from_slice(&index.to_le_bytes());
            for (bytes, offset) in [(&mut ta[..], 0x908usize), (&mut frag[..], 0xc50)] {
                let mut word = u64::from_le_bytes(bytes[offset..offset + 8].try_into().map_err(|_| EINVAL)?);
                word = (word & !(3 << 62)) | ((queue_ordinal % 4) << 62);
                bytes[offset..offset + 8].copy_from_slice(&word.to_le_bytes());
            }
        }
        let vm_root = client.roots().low;
        let pages = if crate::module_parameters::dump_render_recent.value() != 0
            || crate::module_parameters::dump_render_buffered.value() != 0
        {
            resolve_pages(client, &frag)
        } else {
            [(0, 0, 0); 8]
        };
        self.dump_render(&a, (params.tgid, params.comm), vm_root, params.encoder, queue_ordinal, pages, &ta, &frag);
        if crate::module_parameters::fw_trace.value() & 32 != 0 && queue_ordinal == 10 {
            pr_info!(
                "G16: work dump TA={:#x} 3D={:#x} queues={:#x}/{:#x} bm={:#x} thing={:#x} support={:#x} micro={:#x}/{:#x}\n",
                a.tiling_work, a.fragment_work, a.tiling_queue, a.fragment_queue,
                a.buffer_manager, a.buffer_thing, a.support,
                a.tiling_microsequence, a.fragment_microsequence
            );
            for (name, bytes) in [("TA", &ta[..]), ("3D", &frag[..])] {
                for (i, chunk) in bytes.chunks(32).enumerate() {
                    pr_info!("G16: work {} {:#06x}: {:02x?}\n", name, i * 32, chunk);
                }
            }
        }
        // TVB initialization and first publication of a firmware queue are
        // independent: several public queues can share one VM's TVB pool.
        let initbm = !pool.initialized;

        // The shared count covers individual Work items, not render pairs.
        // Firmware retires one tiling and one fragment item per submission.
        let submitted = support.submissions.load(Ordering::Relaxed);
        let next_submitted = submitted.wrapping_add(2);
        // Firmware observes the low word of this cumulative count.
        let use_count = next_submitted as u32;
        // From here an error conservatively quarantines the client root.
        self.render_failed = true;
        self._firmware_space
            .as_mut()
            .ok_or(EIO)?
            .write_live(a.render_shared_state, &use_count.to_le_bytes())?;
        pool.increment(self._firmware_space.as_mut().ok_or(EIO)?)?;
        let own = queue.lanes.map_or([0; 3], |l| l.map(|lane| lane.firmware_stamp()));
        self.note_dependency_events(dependencies, own);
        let (heads, epochs) = self.publish_render(
            &a, &ta, &frag, queue, initbm, pool.blocks, dependencies,
            params.vdm_barrier_fragment,
        )?;
        pool.initialized = true;
        support.submissions.store(next_submitted, Ordering::Relaxed);
        self.render_publication = self.render_publication.wrapping_add(1);
        queue.render_count = queue_ordinal.wrapping_add(1);
        let notifier_count = if shared {
            queue.notified_works = queue.notified_works.wrapping_add(2);
            Some((a.event_control + 0xc, queue.notified_works))
        } else {
            None
        };
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
                stream: params.encoder,
                ordinal: queue_ordinal,
                tgid: params.tgid,
                comm: params.comm,
                started: Instant::now(),
                notifier_count,
                stamps_seen: None,
                blocked: 0,
                dependencies: *dependencies,
                stamps: [
                    Some(Stamp {
                        address: a.tiling_firmware_stamp,
                        value: a.tiling_stamp as u32,
                        event: a.tiling_event as u32,
                    }),
                    Some(stamp),
                ],
                driver_stamps: [
                    Some(Stamp {
                        address: a.tiling_driver_stamp,
                        value: a.tiling_stamp as u32,
                        event: a.tiling_event as u32,
                    }),
                    Some(Stamp {
                        address: a.fragment_driver_stamp,
                        ..stamp
                    }),
                ],
                scratch,
                client: owner.clone(),
                queues: [(lanes[0].pointers, heads[0]), (lanes[1].pointers, heads[1])],
                channels: [
                    (queue.priority as usize * 3, (epochs[0] + 1) & 255),
                    (queue.priority as usize * 3 + 1, (epochs[1] + 1) & 255),
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
        self.batch_works += 1;
        self.trace_publication("render", va)?;
        self.trace_dependencies("render", va, a.context_id as u32, dependencies)?;
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
        dependencies: &[Option<Stamp>; 2],
        vdm_barrier_fragment: bool,
    ) -> Result<([u32; 2], [u32; 2])> {
        let priority = queue.priority;
        let lanes = queue.lanes.as_ref().ok_or(EIO)?;
        let publication = self.render_publication;
        let firsts = [lanes[0].head, lanes[1].head];
        // Only the render dependency can move to fragment. Compute output
        // may feed the vertex stage, and input fences were already consumed
        // by the scheduler before either lane was published.
        let mode = crate::module_parameters::render_barrier.value();
        let vdm_barrier_fragment = vdm_barrier_fragment || mode & 1 != 0;
        // A fragment-stage wait on this queue's own fragment lane is already
        // satisfied by that lane's submission order.
        let fragment_dependency = dependencies[0].filter(|d| {
            mode & 2 == 0 || d.address != lanes[1].firmware_stamp()
        });
        let stage_dependencies = if vdm_barrier_fragment {
            [[None, dependencies[1]], [fragment_dependency, None]]
        } else {
            [*dependencies, [None, None]]
        };
        let heads = [
            (firsts[0] + 1 + u32::from(initbm)
                + stage_dependencies[0].iter().flatten().count() as u32)
                % 0x500,
            (firsts[1] + 2 + stage_dependencies[1].iter().flatten().count() as u32) % 0x500,
        ];
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        if a.event_control == render_notifier(lanes) {
            // Shared notifier: count this render's tiling and fragment Works
            // into the queue's cumulative total. Firmware counts completions
            // at +0xc and writes the lanes' driver stamps as they retire.
            fw.write_live(a.event_control + 0x24, &(a.context_id as u32).to_le_bytes())?;
            fw.write_live(
                a.event_count_array,
                &queue.notified_works.wrapping_add(2).to_le_bytes(),
            )?;
        } else {
            fw.write_live(a.event_control, &a.event_count_array.to_le_bytes())?;
            fw.write_live(
                a.event_control + 8,
                &(a.fragment_stamp as u32).to_le_bytes(),
            )?;
            fw.write_live(a.event_control + 12, &0u32.to_le_bytes())?;
            fw.write_live(a.event_control + 0x10, &0x50u32.to_le_bytes())?;
            fw.write_live(a.event_control + 0x24, &(a.context_id as u32).to_le_bytes())?;
            fw.write_live(a.event_count_array, &2u32.to_le_bytes())?;
            fw.write_live(
                a.tiling_driver_stamp,
                &g16_stamp::previous(a.tiling_stamp as u32).to_le_bytes(),
            )?;
            fw.write_live(
                a.fragment_driver_stamp,
                &g16_stamp::previous(a.fragment_stamp as u32).to_le_bytes(),
            )?;
        }
        // The queue lanes persist independently of the physical work channel.
        for stage in 0..2 {
            let Lane { ring, .. } = lanes[stage];
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
            // The prelude page leaves +0x40..+0xc0 free before its
            // microsequence. All barrier bytes share the Work's lifetime.
            let render_kind = crate::module_parameters::dependency_kind.value();
            let deps = stage_dependencies[stage]
                .iter()
                .enumerate()
                .filter_map(|(slot, dep)| dep.map(|d| (if slot == 0 { render_kind } else { 1 }, d)));
            for (index, (kind, dep)) in deps.enumerate() {
                let address = prelude + 0x40 + index as u64 * 0x40;
                let (stamp, uuid) = if stage == 0 {
                    (a.tiling_stamp, a.tiling_uuid)
                } else {
                    (a.fragment_stamp, a.fragment_uuid)
                };
                fw.write_live(
                    address,
                    &g16_fw::queue::dependency(
                        dep.address, dep.value, dep.event, stamp as u32, uuid as u32, kind,
                    ),
                )?;
                fw.write_live(ring + u64::from(pos) * 8, &address.to_le_bytes())?;
                pos = (pos + 1) % 0x500;
            }
            if stage == 1 || initbm {
                fw.write_live(ring + u64::from(pos) * 8, &prelude.to_le_bytes())?;
                pos = (pos + 1) % 0x500;
            }
            fw.write_live(ring + u64::from(pos) * 8, &work.to_le_bytes())?;
        }
        let mut epochs = [0; 2];
        for stage in [1usize, 0] {
            let mut lane = lanes[stage];
            lane.head = heads[stage];
            epochs[stage] = self.stage_queue(
                priority as usize * 3 + stage,
                lane,
                if stage == 0 {
                    a.tiling_event
                } else {
                    a.fragment_event
                } as u32,
                publication.wrapping_add(1),
            )?;
        }
        Ok((heads, epochs))
    }
}

impl Bootstrap {
    pub(crate) fn map_compute_work(
        &mut self,
        space: &mut g16_vm::AddressSpace,
        private: core::ops::Range<u64>,
    ) -> Result {
        let opening = self._opening_client.as_mut().ok_or(EIO)?;
        for (base, size) in compute::private_ranges() {
            // Scratch and marker are private to each command at fresh VAs.
            // Only the device-owned FList directory and spill pool are shared.
            if base == compute::SCRATCH || base == compute::MARKER {
                continue;
            }
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
        space.init_compute_private(private);
        space.sync();
        Ok(())
    }

    /// Order a compute Work against other queues in the kernel rather than
    /// with firmware dependency items, which can stall the firmware when a
    /// burst of compute Works carries them (Dota 2's loading screen); Metal's
    /// compute Works carry none either. The queue's own compute lane is
    /// ordered by its ring. Any other awaited stamp must already be reached:
    /// otherwise return EBUSY, and the submission worker publishes what it
    /// has staged and retries after the next completion, holding back the
    /// rest of this queue. compute_deps=0 keeps the firmware items.
    fn resolve_compute_dependencies(
        &self,
        queue: &mut FirmwareQueues,
        dependencies: &[Option<Stamp>; 2],
    ) -> Result<[Option<Stamp>; 2]> {
        if crate::module_parameters::compute_deps.value() == 0 {
            return Ok(*dependencies);
        }
        let own_lane = queue.lanes.as_ref().map(|lanes| lanes[2].firmware_stamp());
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        for dep in dependencies.iter().flatten() {
            if own_lane != Some(dep.address) {
                let now = fw.read_u32(dep.address)?;
                let done = g16_stamp::reached(now, dep.value);
                self.hold_check("compute", dep, now, done)?;
                if !done {
                    queue.deps.compute_held += 1;
                    queue.deps.defer();
                    return Err(EBUSY);
                }
            }
        }
        queue.deps.admit();
        Ok([None; 2])
    }

    pub(crate) fn submit_compute(
        &mut self,
        queues: Arc<Mutex<FirmwareQueues>>,
        owner: Arc<Mutex<g16_vm::AddressSpace>>,
        params: &compute::Parameters,
        timestamps: [u64; 4],
        dependencies: &[Option<Stamp>; 2],
    ) -> Result<Receipt> {
        let mut queue = queues.lock();
        let mut client = owner.lock();
        let resolved = self.resolve_compute_dependencies(&mut queue, dependencies)?;
        let dependencies = &resolved;
        self.reserve_flight(
            &queue,
            false,
            client.roots().low,
            [0, 0, 1 + dependencies.iter().flatten().count() as u32],
        )?;
        self.prepare_queues(&mut queue)?;
        let result = self.submit_compute_queue(
            &mut queue,
            queues.clone(),
            &mut client,
            owner.clone(),
            params,
            timestamps,
            dependencies,
        );
        if queue.pending == 0 && crate::module_parameters::event_affinity.value() < 2 {
            queue.events = None;
        }
        result
    }

    #[inline(never)]
    fn submit_compute_queue(
        &mut self,
        queue: &mut FirmwareQueues,
        queues: Arc<Mutex<FirmwareQueues>>,
        client: &mut g16_vm::AddressSpace,
        owner: Arc<Mutex<g16_vm::AddressSpace>>,
        params: &compute::Parameters,
        timestamps: [u64; 4],
        dependencies: &[Option<Stamp>; 2],
    ) -> Result<Receipt> {
        let lane = queue.lanes.as_ref().ok_or(EIO)?[2];
        let priority = queue.priority;

        let mut params = *params;
        let scratch = client.prepare_compute(&mut params)?;
        let space = client.roots();
        if self.render_failed {
            return Err(EIO);
        }
        if !(2..=3).contains(&priority) {
            return Err(EINVAL);
        }
        let ordinal = queue.compute_count;
        let work = self.allocate_work(false)?;
        let alias = 0x71_0000_0000 + (work - 0xffff_fc22_0000_0000);
        if alias + 0x4000 > 0x72_0000_0000 {
            return Err(ENOSPC);
        }
        // Firmware save state is independent of shader resource tables.
        // The upper half of the zeroed Work page is GPU-addressable storage.
        params.save_area = alias + 0x2000;
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        let pa = fw.physical(work)?;
        self._address_space
            .as_mut()
            .ok_or(EIO)?
            .map_work_alias(alias, pa)?;
        client.map_work_alias(alias, pa)?;
        fw.zero_live(work + 0x2000, 0x2000)?;
        let mut a = compute::Addresses::publication(self.compute_publication, ordinal, work, alias);
        a.queue = lane.queue;
        a.driver_stamp = work + 0x4000;
        a.firmware_stamp = lane.firmware_stamp();
        a.event = queue.event(2)?;
        self._address_space.as_ref().ok_or(EIO)?.sync();
        fw.sync();
        client.sync();
        self.invalidate_client(self._address_space.as_ref().ok_or(EIO)?);
        self.invalidate_client(client);
        a.context = self.acquire_context(space, 1)?;
        a.identity = (u64::from(a.context) << 32) | (a.identity & 0xffff_ffff);
        a.register_identity = (u64::from(a.context) << 32) | 1;
        self.render_failed = true;
        let own = queue.lanes.map_or([0; 3], |l| l.map(|lane| lane.firmware_stamp()));
        self.note_dependency_events(dependencies, own);
        a.cdm_entry = Some(alias + compute::ENTRY_OFFSET);
        let mut work = KVec::new();
        work.extend_with(0x8c0, 0u8, GFP_KERNEL)?;
        params.work(&a, &mut work).ok_or(EINVAL)?;
        // Timestamp's user destinations must lie in the validated special-
        // object aperture. Firmware writes nanoseconds directly, as on M1/M2.
        // Firmware consumes this pointer pair, so it lives in the Work's
        // uncached shared page: in the cached private page, stale firmware
        // cache lines lost timestamps once the Work was reused.
        let user_timestamps = a.work + 0x4040;
        let count = self.compute_publication.wrapping_add(1);
        let count32 = count as u32;

        let first = lane.head;
        let head = (first + 1 + dependencies.iter().flatten().count() as u32) % 0x500;
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
        let notifier = a.notifier(0);
        // Retained notifier list links are firmware-owned even after completion.
        fw.write_live(a.notifier, &notifier[..0x14])?;
        fw.write_live(a.notifier + 0x24, &notifier[0x24..0x28])?;
        fw.write_live(a.threshold, &1u32.to_le_bytes())?;
        fw.write_live(a.driver_stamp, &g16_stamp::previous(a.stamp).to_le_bytes())?;
        let mut pos = first;
        for (index, dep) in dependencies.iter().flatten().enumerate() {
            // Between the threshold/timestamps and the CDM entry trampoline.
            let address = a.work + 0x1900 + index as u64 * 0x40;
            fw.write_live(
                address,
                &g16_fw::queue::dependency(
                    dep.address,
                    dep.value,
                    dep.event,
                    a.stamp,
                    a.identity as u32,
                    1,
                ),
            )?;
            fw.write_live(lane.ring + u64::from(pos) * 8, &address.to_le_bytes())?;
            pos = (pos + 1) % 0x500;
        }
        fw.write_live(lane.ring + u64::from(pos) * 8, &a.work.to_le_bytes())?;
        let channel = priority as usize * 3 + 2;
        let mut pending_lane = lane;
        pending_lane.head = head;
        let epoch =
            self.stage_queue(channel, pending_lane, a.event, count)?;
        self.compute_publication = count;
        queue.compute_count = ordinal.wrapping_add(1);
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
                stream: params.cdm,
                ordinal,
                tgid: params.tgid,
                comm: params.comm,
                started: Instant::now(),
                notifier_count: None,
                stamps_seen: None,
                blocked: 0,
                dependencies: *dependencies,
                stamps: [Some(stamp), None],
                driver_stamps: [
                    Some(Stamp {
                        address: a.driver_stamp,
                        ..stamp
                    }),
                    None,
                ],
                scratch,
                client: owner,
                queues: [(pointers, head), (0, 0)],
                channels: [(channel, (epoch + 1) & 255), (usize::MAX, 0)],
                render: None,
            },
            GFP_KERNEL,
        )?;
        self.render_failed = false;
        self.batch_works += 1;
        self.trace_publication("compute", a.work)?;
        self.trace_dependencies("compute", a.work, a.context, dependencies)?;
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
        if cursor != end && crate::module_parameters::fw_trace.value() & 64 != 0 {
            // SAFETY: ktime_get() has no preconditions.
            let now = unsafe { kernel::bindings::ktime_get() };
            let base = RING + u64::from(cursor) * 0x48;
            pr_info!(
                "G16: rxring {} event {}..{} first={:#x}/{:#x}\n",
                now, cursor, end, fw.read_u32(base)?, fw.read_u32(base + 4)?
            );
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
            drop(guard);
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
                let grown = pool.grow(fw, client, a.context_id as u8)?;
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
            let guard = self._sgx.try_access().ok_or(ENODEV)?;
            guard.try_write32(successor, 0xd60060)?;
            drop(guard);
            crate::mem::sync();
            self._rtkit
                .as_mut()
                .send_message(0x21, 0x0084_0000_0000_0011)?;
            return Ok(());
        }
        Ok(())
    }
}

static DUMPED_RENDERS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

const DUMP_TA: usize = 0x940;
const DUMP_FRAG: usize = 0xcc0;

/// A render as submitted, for dump_render. The Work bytes live separately.
#[derive(Clone, Copy)]
pub(crate) struct RenderDump {
    /// User-VA register values of the 3D table and the physical pages
    /// they resolved to at submission: (register, VA, PA).
    pages: [(u32, u64, u64); 8],
    /// Submission time, CLOCK_MONOTONIC ns (as dmesg's uptime).
    at_ns: i64,
    tgid: i32,
    comm: [u8; 16],
    /// The submitting VM's render root, which identifies it at detach
    /// whichever task tears it down. 0 once printed at detach.
    vm_root: u64,
    context: u64,
    stream: u64,
    ordinal: u64,
    tiling_work: u64,
    fragment_work: u64,
    uuids: (u64, u64),
    stamps: (u64, u64),
}

impl RenderDump {
    fn new(a: &render::Addresses, (tgid, comm): (i32, [u8; 16]), vm_root: u64, stream: u64, ordinal: u64, pages: [(u32, u64, u64); 8]) -> Self {
        Self {
            pages,
            at_ns: monotonic_ns(),
            tgid,
            comm,
            stamps: (a.tiling_stamp, a.fragment_stamp),
            vm_root,
            context: a.context_id,
            stream,
            ordinal,
            tiling_work: a.tiling_work,
            fragment_work: a.fragment_work,
            uuids: (a.tiling_uuid, a.fragment_uuid),
        }
    }

    /// Print in the same form as the hang report's 3D register table.
    fn print(&self, tag: &str, ta: &[u8], frag: &[u8]) {
        let ordinal = self.ordinal;
        let word = |bytes: &[u8], at: usize| -> u32 {
            bytes.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let size = (0..0x600 / 12)
            .map(|i| 0x80 + i * 12)
            .find(|&at| word(frag, at) == 0x15211)
            .map_or(0, |at| u64::from(word(frag, at + 4)) | (u64::from(word(frag, at + 8)) << 32));
        pr_info!(
            "G16: {} render at={}.{:06} tgid={} comm={} vm={:#x} context={} stream={:#x} ordinal={} size={}x{} stamps={:#x}/{:#x} TA={:#x} 3D={:#x} uuid={:#x}/{:#x}\n",
            tag, self.at_ns / 1_000_000_000, (self.at_ns % 1_000_000_000) / 1000,
            self.tgid, comm_str(&self.comm), self.vm_root, self.context, self.stream, ordinal,
            size & 0xffff_ffff, size >> 32, self.stamps.0, self.stamps.1,
            self.tiling_work, self.fragment_work, self.uuids.0, self.uuids.1
        );
        if self.pages[0].1 != 0 {
            let n = self.pages.iter().take_while(|p| p.1 != 0).count();
            pr_info!("G16: {} render {} pages (reg, va, pa) {:x?}\n", tag, ordinal, &self.pages[..n]);
        }
        let mut line = KVec::new();
        for i in 0..0x600 / 12 {
            let at = 0x80 + i * 12;
            let number = word(frag, at);
            let value = u64::from(word(frag, at + 4)) | (u64::from(word(frag, at + 8)) << 32);
            if number == 0 && value == 0 {
                break;
            }
            if line.push((number, value), GFP_KERNEL).is_err() {
                return;
            }
            if line.len() == 6 {
                pr_info!("G16: {} render {} 3D regs {:x?}\n", tag, ordinal, &line[..]);
                line.clear();
            }
        }
        if !line.is_empty() {
            pr_info!("G16: {} render {} 3D regs {:x?}\n", tag, ordinal, &line[..]);
        }
        // Descriptor words outside the register table: the (offset,
        // descriptor) pairs and partial/EOT state, plus the lifecycle tail.
        for (name, bytes, ranges) in [
            ("3D", frag, &[(0x440usize, 0x700usize), (0xb80, 0xc00)][..]),
            ("TA", ta, &[(0x800usize, 0x8c0usize)][..]),
        ] {
            for &(start, end) in ranges {
                for at in (start..end.min(bytes.len())).step_by(32) {
                    let chunk = &bytes[at..(at + 32).min(bytes.len())];
                    pr_info!("G16: {} render {} {} {:#05x}: {:02x?}\n", tag, ordinal, name, at, chunk);
                }
            }
        }
    }
}

/// Debug: whether a render matches the dump_render shape filters.
fn dump_matches(a: &render::Addresses, frag: &[u8]) -> bool {
    let context = crate::module_parameters::dump_render_context.value();
    if context != u32::MAX && a.context_id != u64::from(context) {
        return false;
    }
    let reg = crate::module_parameters::dump_render_reg.value();
    if reg == 0 {
        return true;
    }
    let want = crate::module_parameters::dump_render_value.value();
    let word = |at: usize| -> u32 {
        frag.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    for i in 0..0x600 / 12 {
        let at = 0x80 + i * 12;
        let number = word(at);
        let value = u64::from(word(at + 4)) | (u64::from(word(at + 8)) << 32);
        if number == 0 && value == 0 {
            break;
        }
        if number == reg {
            return value == want;
        }
    }
    false
}

fn dump_window(ordinal: u64) -> bool {
    let to = crate::module_parameters::dump_render_to.value();
    to != 0
        && ordinal >= u64::from(crate::module_parameters::dump_render_from.value())
        && ordinal <= u64::from(to)
}

/// Debug: a ring of submitted renders, allocated once on first use so that
/// recording never allocates or prints on the submission path.
pub(crate) struct RenderRing {
    headers: KVec<RenderDump>,
    bytes: KVVec<u8>,
    next: usize,
}

impl RenderRing {
    pub(crate) const fn new() -> Self {
        Self { headers: KVec::new(), bytes: KVVec::new(), next: 0 }
    }

    fn record(&mut self, capacity: usize, header: RenderDump, ta: &[u8], frag: &[u8]) {
        if capacity == 0 || ta.len() != DUMP_TA || frag.len() != DUMP_FRAG {
            return;
        }
        if self.bytes.is_empty() {
            let total = capacity * (DUMP_TA + DUMP_FRAG);
            if self.headers.reserve(capacity, GFP_KERNEL).is_err()
                || self.bytes.resize(total, 0, GFP_KERNEL).is_err()
            {
                self.bytes.clear();
                return;
            }
        }
        let slot = self.next % capacity;
        if slot < self.headers.len() {
            self.headers[slot] = header;
        } else if self.headers.push(header, GFP_KERNEL).is_err() {
            return;
        }
        let base = slot * (DUMP_TA + DUMP_FRAG);
        self.bytes[base..base + DUMP_TA].copy_from_slice(ta);
        self.bytes[base + DUMP_TA..base + DUMP_TA + DUMP_FRAG].copy_from_slice(frag);
        self.next = self.next.wrapping_add(1);
    }

    /// Print the entries `keep` selects, oldest first; returns the count.
    fn print(&self, tag: &str, keep: impl Fn(&RenderDump) -> bool) -> usize {
        let len = self.headers.len();
        let start = if self.next > len && len != 0 { self.next % len } else { 0 };
        let mut printed = 0;
        for i in 0..len {
            let slot = (start + i) % len;
            let entry = &self.headers[slot];
            if entry.vm_root == 0 || !keep(entry) {
                continue;
            }
            let base = slot * (DUMP_TA + DUMP_FRAG);
            let Some(bytes) = self.bytes.get(base..base + DUMP_TA + DUMP_FRAG) else {
                break;
            };
            entry.print(tag, &bytes[..DUMP_TA], &bytes[DUMP_TA..]);
            printed += 1;
        }
        printed
    }

    /// Stop printing a detached VM's entries.
    fn forget(&mut self, vm_root: u64) {
        for entry in self.headers.iter_mut() {
            if entry.vm_root == vm_root {
                entry.vm_root = 0;
            }
        }
    }
}

impl Bootstrap {
    /// Debug (dump_render_*): log a render as submitted, or keep it in the
    /// rings printed by the hang report, so a stuck Work can be diffed
    /// against completed renders without printing on the submission path.
    /// dump_render_buffered keeps renders matching the shape filters;
    /// dump_render_recent keeps every render, for the stuck context's
    /// immediate predecessors.
    #[inline(never)]
    fn dump_render(&mut self, a: &render::Addresses, tgid: (i32, [u8; 16]), vm_root: u64, stream: u64, ordinal: u64, pages: [(u32, u64, u64); 8], ta: &[u8], frag: &[u8]) {
        let recent = crate::module_parameters::dump_render_recent.value() as usize;
        if recent != 0 {
            self.recent_ring.record(recent, RenderDump::new(a, tgid, vm_root, stream, ordinal, pages), ta, frag);
        }
        let ring = crate::module_parameters::dump_render_buffered.value() as usize;
        let enabled = ring != 0 || crate::module_parameters::dump_render_to.value() != 0;
        if !enabled || !dump_matches(a, frag) {
            return;
        }
        if ring != 0 {
            if crate::module_parameters::dump_render_to.value() != 0 && !dump_window(ordinal) {
                return;
            }
            self.render_ring.record(ring, RenderDump::new(a, tgid, vm_root, stream, ordinal, pages), ta, frag);
            return;
        }
        if !dump_window(ordinal) {
            return;
        }
        let max = crate::module_parameters::dump_render_max.value();
        if DUMPED_RENDERS.fetch_add(1, Ordering::Relaxed) >= max {
            return;
        }
        RenderDump::new(a, tgid, vm_root, stream, ordinal, pages).print("dump", ta, frag);
    }

    /// Hang report: the shape ring, then each stuck render context's
    /// recent renders of any shape.
    fn print_render_rings(&self) {
        let now = monotonic_ns();
        pr_info!("G16: render rings printed at={}.{:06}\n", now / 1_000_000_000, (now % 1_000_000_000) / 1000);
        if crate::module_parameters::dump_render_buffered.value() != 0
            && self.render_ring.print("ring", |_| true) == 0
        {
            pr_info!("G16: ring holds no matching renders ({} recorded since boot)\n", self.render_ring.next);
        }
        if crate::module_parameters::dump_render_recent.value() == 0 {
            return;
        }
        // Everything submitted just before the hang, on every context and
        // VM; each header names its tgid, VM, context and size.
        let printed = self.recent_ring.print("recent", |_| true);
        pr_info!(
            "G16: recent renders: {} printed ({} recorded since boot)\n",
            printed, self.recent_ring.next
        );
    }

    /// Debug (dump_render_buffered): print a detached VM's buffered renders,
    /// a same-run baseline for runs that do not stall.
    pub(crate) fn flush_render_ring(&mut self, vm_root: u64) {
        if crate::module_parameters::fw_trace.value() & 256 != 0 {
            let now = monotonic_ns();
            pr_info!(
                "G16: event summary at={}.{:06} (VM {:#x} detached): {} stale ({} live, {} live on own predecessor) of {} waits since boot\n",
                now / 1_000_000_000, (now % 1_000_000_000) / 1000, vm_root,
                self.stale_event_tags, self.stale_live_tags, self.stale_live_own_tags, self.dependency_waits
            );
        }
        if crate::module_parameters::dump_render_buffered.value() != 0 {
            let printed = self.render_ring.print("exit", |e| e.vm_root == vm_root);
            // Always say so, so an empty result is not mistaken for no hook.
            pr_info!(
                "G16: exit ring vm root {:#x}: {} matching renders printed, {} recorded since boot\n",
                vm_root, printed, self.render_ring.next
            );
        }
        self.render_ring.forget(vm_root);
        self.recent_ring.forget(vm_root);
    }
}

/// Diagnostics: a process name from a NUL-padded comm.
fn comm_str(comm: &[u8; 16]) -> &str {
    let len = comm.iter().position(|&c| c == 0).unwrap_or(16);
    core::str::from_utf8(&comm[..len]).unwrap_or("?")
}

/// Diagnostics: one retired Work, kept to explain later reuse of its slot
/// and its Work memory in hang reports.
#[derive(Clone, Copy)]
pub(crate) struct Retired {
    work: u64,
    context: u32,
    root: u64,
    tgid: i32,
    comm: [u8; 16],
    ordinal: u64,
    render: bool,
    submitted: Instant<Monotonic>,
    retired: Instant<Monotonic>,
}

/// Diagnostics: the latest rebinding of a firmware context slot.
#[derive(Clone, Copy)]
pub(crate) struct Handover {
    at: Instant<Monotonic>,
    previous: u64,
    next: u64,
}

const RETIRE_LOG: usize = 256;

fn micros(later: Instant<Monotonic>, earlier: Instant<Monotonic>) -> i64 {
    (later - earlier).as_nanos() / 1000
}

impl Bootstrap {
    /// Hang report: for each stuck render, when its context slot last changed
    /// hands and what the previous occupant last retired there, and who used
    /// its Work memory before it. Negative times are after the stuck submit.
    fn print_reuse_history(&self) {
        for flight in self.flights.iter().filter(|f| f.render.is_some()).take(8) {
            let slot = flight.context as usize;
            let root = flight.render.as_ref().map_or(0, |r| r.root);
            match self.handovers.get(slot).copied().flatten() {
                Some(h) => {
                    let other = if h.next == root { "" } else { " (not to this Work's VM)" };
                    dev_err!(self.dev.as_ref(),
                        "G16: hang history Work={:#x} slot {}: rebound {:#x} -> {:#x} {} us before submit{}\n",
                        flight.id, slot, h.previous, h.next, micros(flight.started, h.at),
                        other);
                    if let Some(r) = self.retire_log.iter()
                        .filter(|r| r.context as usize == slot && r.root == h.previous)
                        .max_by_key(|r| micros(r.retired, flight.started))
                    {
                        dev_err!(self.dev.as_ref(),
                            "G16: hang history   previous occupant tgid={} comm={} ordinal={} render={} retired {} us before the rebind, {} us before submit\n",
                            r.tgid, comm_str(&r.comm), r.ordinal, r.render,
                            micros(h.at, r.retired), micros(flight.started, r.retired));
                    }
                }
                None => dev_err!(self.dev.as_ref(),
                    "G16: hang history Work={:#x} slot {}: no rebind recorded\n", flight.id, slot),
            }
            match self.retire_log.iter().filter(|r| r.work == flight.id).max_by_key(|r| micros(r.retired, flight.started)) {
                Some(r) => dev_err!(self.dev.as_ref(),
                    "G16: hang history   Work memory last used by tgid={} comm={} slot {} ordinal={} vm {:#x}: submitted {} us and retired {} us before this submit\n",
                    r.tgid, comm_str(&r.comm), r.context, r.ordinal, r.root,
                    micros(flight.started, r.submitted), micros(flight.started, r.retired)),
                None => dev_err!(self.dev.as_ref(),
                    "G16: hang history   Work memory: no earlier use in the last {} retirements\n", RETIRE_LOG),
            }
        }
    }
}

/// Diagnostics: CLOCK_MONOTONIC in ns, comparable with dmesg timestamps.
fn monotonic_ns() -> i64 {
    // SAFETY: ktime_get has no preconditions.
    unsafe { kernel::bindings::ktime_get() }
}

/// Diagnostics: resolve 3D register values that look like user VAs through
/// the client's page tables, so ring entries can show which renders touch
/// the same physical pages (e.g. one dma-buf under two VAs).
fn resolve_pages(client: &mut g16_vm::AddressSpace, frag: &[u8]) -> [(u32, u64, u64); 8] {
    let mut out = [(0, 0, 0); 8];
    let mut n = 0;
    let word = |at: usize| -> u32 {
        frag.get(at..at + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    for i in 0..0x600 / 12 {
        let at = 0x80 + i * 12;
        let number = word(at);
        let value = u64::from(word(at + 4)) | (u64::from(word(at + 8)) << 32);
        if number == 0 && value == 0 {
            break;
        }
        if !(0x10_0000_0000..1 << 42).contains(&value) || n == out.len() {
            continue;
        }
        if let Ok(Some(pa)) = client.low.translate(value) {
            out[n] = (number, value, pa + (value & 0x3fff));
            n += 1;
        }
    }
    out
}


impl Bootstrap {
    /// Diagnostics: the event a lane's queue signals now, by the lane's
    /// firmware stamp address (the address cross-queue waits name).
    fn set_lane_event(&mut self, stamp: u64, event: u32) {
        if let Some(entry) = self.lane_events.iter_mut().find(|e| e.0 == stamp) {
            entry.1 = event;
        } else {
            let _ = self.lane_events.push((stamp, event), GFP_KERNEL);
        }
    }

    /// Diagnostics: count waits whose event tag differs from the event the
    /// producing lane's queue currently holds ("stale tag at publish").
    #[inline(never)]
    fn note_dependency_events(&mut self, dependencies: &[Option<Stamp>; 2], own: [u64; 3]) {
        for dep in dependencies.iter().flatten() {
            let Some(&(_, current)) = self.lane_events.iter().find(|e| e.0 == dep.address) else {
                continue;
            };
            self.dependency_waits = self.dependency_waits.wrapping_add(1);
            if current != dep.event {
                self.stale_event_tags = self.stale_event_tags.wrapping_add(1);
                // Is the stale tag's index live for some other queue right
                // now (its event fires for them), or free?
                let index = dep.event.wrapping_sub(32) / 3;
                let live = index < 32
                    && self.queue_events.load(Ordering::Acquire) & (1 << index) != 0;
                if live {
                    self.stale_live_tags = self.stale_live_tags.wrapping_add(1);
                    // A wait on the publishing queue's own lane: its
                    // predecessor Work, published before an idle gap.
                    if own.contains(&dep.address) {
                        self.stale_live_own_tags = self.stale_live_own_tags.wrapping_add(1);
                    }
                }
                let slot = self.stale_next % self.stale_recent.len();
                self.stale_recent[slot] = (monotonic_ns(), dep.address, dep.value, dep.event, current, live);
                self.stale_next = self.stale_next.wrapping_add(1);
                if crate::module_parameters::fw_trace.value() & 256 != 0 && self.stale_event_tags <= 64 {
                    pr_info!(
                        "G16: stale event tag: wait on {:#x} want {:#x} names event {}, its queue now holds {}{}\n",
                        dep.address, dep.value, dep.event, current,
                        if live { " (index live for another queue)" } else { "" }
                    );
                }
            }
        }
    }

    /// Hang report: stale-tag totals and the event each known lane holds.
    fn print_lane_events(&self) {
        dev_err!(self.dev.as_ref(), "G16: hang events: {} stale ({} on an index live for another queue, {} of those on the queue's own predecessor) of {} waits since boot; events in use {:#010x}\n",
                 self.stale_event_tags, self.stale_live_tags, self.stale_live_own_tags, self.dependency_waits,
                 self.queue_events.load(Ordering::Acquire) & 0xffff_ffff);
        // The most recent stale tags, oldest first.
        let len = self.stale_recent.len();
        let count = self.stale_next.min(len);
        for i in 0..count {
            let (at, stamp, value, tag, current, live) = self.stale_recent[(self.stale_next - count + i) % len];
            dev_err!(self.dev.as_ref(), "G16: hang stale tag at={}.{:06} wait on {:#x} want {:#x} names event {}, queue holds {}{}\n",
                     at / 1_000_000_000, (at % 1_000_000_000) / 1000, stamp, value, tag, current,
                     if live { " (live for another queue)" } else { "" });
        }
        let mut line = KVec::new();
        for &(stamp, event) in self.lane_events.iter().take(96) {
            if line.push((stamp, event), GFP_KERNEL).is_err() {
                break;
            }
            if line.len() == 6 {
                dev_err!(self.dev.as_ref(), "G16: hang lane events (stamp, event) {:x?}\n", &line[..]);
                line.clear();
            }
        }
        if !line.is_empty() {
            dev_err!(self.dev.as_ref(), "G16: hang lane events (stamp, event) {:x?}\n", &line[..]);
        }
    }
}

impl Bootstrap {
    /// A4 wedge (see tmp/kernel-73-validate/a4-recovery-design.md): a render
    /// whose TA is done and whose waits are all satisfied, while its 3D never
    /// starts, every channel is drained and no fragment stamp on any lane
    /// moves for wedge_ms. Firmware reports nothing for this state, so the
    /// kernel detects it. wedge_recover=0 only logs the detection; 1 also
    /// walks the recovery ladder: R1 re-kicks the stuck render's 3D lane, R2
    /// re-kicks the TA and 3D lanes of every live render queue.
    fn check_wedge(&mut self) -> Result {
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        let mut stuck = None;
        let mut signature = 0u64;
        for (index, flight) in self.flights.iter().enumerate() {
            let (Some(_), Some(tiling), Some(fragment)) = (&flight.render, flight.stamps[0], flight.stamps[1]) else {
                continue;
            };
            let ta = fw.read_u32(tiling.address)?;
            let frag = fw.read_u32(fragment.address)?;
            signature = signature.wrapping_mul(0x100_0000_01b3).wrapping_add(u64::from(frag));
            if stuck.is_none()
                && g16_stamp::reached(ta, tiling.value)
                && !g16_stamp::reached(frag, fragment.value)
                && flight.dependencies.iter().flatten().all(|d| {
                    fw.read_u32(d.address).is_ok_and(|v| g16_stamp::reached(v, d.value))
                })
            {
                stuck = Some(index);
            }
        }
        let drained = {
            let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
            let guard = self._sgx.try_access().ok_or(ENODEV)?;
            config.channels.iter().take(12).all(|route| {
                let read = guard.try_read32(0xd64000 + (route.state[0] - 0xffff_fc20_0002_8000) as usize);
                let done = guard.try_read32(0xd64000 + (route.state[1] - 0xffff_fc20_0002_8000) as usize);
                let tail = guard.try_read32(0xd60000 + (route.state[2] - 0xffff_fc20_0002_0000) as usize);
                matches!((read, done, tail), (Ok(r), Ok(d), Ok(t)) if r == d && d == t)
            })
        };
        let Some(index) = stuck.filter(|_| drained) else {
            self.wedge_since = None;
            self.wedge_rung = 0;
            return Ok(());
        };
        let now = Instant::now();
        match self.wedge_since {
            Some(since) if signature == self.wedge_signature => {
                let wedge_ms = u64::from(crate::module_parameters::wedge_ms.value().max(100));
                if (now - since).as_millis() < wedge_ms as i64 {
                    return Ok(());
                }
            }
            _ => {
                // First sighting, or some fragment stamp moved: restart.
                self.wedge_since = Some(now);
                self.wedge_signature = signature;
                return Ok(());
            }
        }
        let flight = &self.flights[index];
        let owner = flight.owner.clone();
        let recover = crate::module_parameters::wedge_recover.value() != 0;
        if self.wedge_rung < 3 {
            dev_err!(self.dev.as_ref(),
                "G16: A4 wedge: Work {:#x} context {} stream {:#x} ordinal {} age {} ms, no fragment progress for {} ms; {}\n",
                flight.id, flight.context, flight.stream, flight.ordinal,
                flight.started.elapsed().as_millis(), (now - self.wedge_since.unwrap_or(now)).as_millis(),
                match (recover, self.wedge_rung) {
                    (false, _) => "detection only (wedge_recover=0)",
                    (true, 0) => "R1: re-kicking its 3D lane",
                    (true, 1) => "R2: re-kicking every render queue's TA and 3D lanes",
                    _ => "ladder exhausted, leaving it to the watchdog",
                });
        }
        if !recover || self.wedge_rung >= 2 {
            self.wedge_rung = 3;
            return Ok(());
        }
        if self.wedge_rung == 0 {
            self.rekick(&owner, &[1])?;
        } else {
            let mut owners = KVec::new();
            for f in self.flights.iter().filter(|f| f.render.is_some()) {
                if !owners.iter().any(|o: &Arc<Mutex<FirmwareQueues>>| Arc::ptr_eq(o, &f.owner)) {
                    owners.push(f.owner.clone(), GFP_KERNEL)?;
                }
            }
            for o in owners.iter() {
                self.rekick(o, &[0, 1])?;
            }
        }
        self.flush()?;
        self.wedge_rung += 1;
        self.wedge_since = Some(Instant::now());
        Ok(())
    }

    /// Re-send a render queue's channel messages at its current heads, as a
    /// fresh publication with no new Work: firmware re-reads the lane.
    fn rekick(&mut self, owner: &Arc<Mutex<FirmwareQueues>>, stages: &[usize]) -> Result {
        let (lanes, priority, events) = {
            let q = owner.lock();
            let Some(lanes) = q.lanes else { return Ok(()) };
            (lanes, q.priority, [q.event(0).ok(), q.event(1).ok()])
        };
        for &stage in stages {
            let Some(event) = events[stage] else { continue };
            let mut lane = lanes[stage];
            lane.new = false;
            self.stage_queue(priority as usize * 3 + stage, lane, event, self.render_publication)?;
        }
        Ok(())
    }
}
