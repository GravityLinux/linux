// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from Niklas Sheth's linux-m4-integration prototype.

//! G16 ring publication and independent firmware retirement: the queue,
//! flight and ledger types shared by the submodules, which hold the
//! `Bootstrap` methods for queue lifecycle (`queues`), admission and
//! publication (`publish`), retirement (`retire`), firmware contexts and
//! support (`contexts`), render and compute submission (`render_submit`,
//! `compute_submit`) and diagnostics (`diag`).

use crate::util::comm_str;

use kernel::io::Io;
use super::Bootstrap;
use crate::{g16_compute as compute, g16_fw, g16_mesh, g16_render as render, g16_stamp, g16_vm};
use core::sync::atomic::{AtomicU64, Ordering};
use kernel::time::{Instant, Monotonic};
use kernel::{
    c_str, io,
    prelude::*,
    sync::{Arc, Mutex},
};

#[path = "g16_runtime_queues.rs"]
mod queues;
#[path = "g16_runtime_publish.rs"]
mod publish;
#[path = "g16_runtime_retire.rs"]
mod retire;
#[path = "g16_runtime_contexts.rs"]
mod contexts;
#[path = "g16_runtime_render.rs"]
mod render_submit;
#[path = "g16_runtime_compute.rs"]
mod compute_submit;
#[path = "g16_runtime_diag.rs"]
mod diag;

/// One userspace queue's three firmware workqueues. Channel rings remain
/// device-wide transports; their messages select these independent queues.
/// Their storage returns to Bootstrap when the last command retires.
pub(crate) struct FirmwareQueues {
    lanes: Option<[Lane; 3]>,
    storage: Option<QueueStorage>,
    priority: u32,
    events: Option<QueueEvents>,
    /// The event index this queue held last (for fw_trace bit 256).
    last_event: Option<u32>,
    pending: usize,
    render_count: u64,
    compute_count: u64,
    /// Render Works counted into the queue's shared notifier (two per render).
    notified_works: u32,
    pub(crate) last: [Option<Stamp>; 2],
    /// A command on this queue was rejected before publication or completed
    /// with an error. Userspace may split one submission into several jobs
    /// whose later parts rely on queue order, so every later job on the queue
    /// fails with ECANCELED instead of running without that command, and new
    /// submissions are refused.
    pub(crate) rejected: bool,
    /// Per lane (TA, 3D, compute), the latest stamps its published Works
    /// waited on as firmware items, keyed by address.
    waited: [[Option<Stamp>; 2]; 3],
    /// The last (read, done) cursors admission read per lane;
    /// u32::MAX until read. Cursors only advance, so a stale view
    /// underestimates room.
    cursors: [core::cell::Cell<(u32, u32)>; 3],
}

impl FirmwareQueues {
    /// Whether the queue's firmware rings exist (see `Runtime::provision_queue`).
    pub(crate) fn provisioned(&self) -> bool {
        self.lanes.is_some()
    }

    /// Remember that a published Work on `lane` waited on `dep` (a field-level
    /// borrow, usable while the queue's lanes are borrowed).
    fn note_wait(waited: &mut [[Option<Stamp>; 2]; 3], lane: usize, dep: Stamp) {
        let slots = &mut waited[lane];
        if let Some(s) = slots.iter_mut().find(|s| s.is_some_and(|w| w.address == dep.address)) {
            *s = Some(dep);
        } else {
            slots[1] = slots[0];
            slots[0] = Some(dep);
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
            waited: [[None; 2]; 3],
            cursors: [const { core::cell::Cell::new((u32::MAX, u32::MAX)) }; 3],
            rejected: false,
        }
    }

    fn event(&self, stage: u32) -> Result<u32> {
        Ok(32 + self.events.as_ref().ok_or(EIO)?.index * 3 + stage)
    }
}

/// Firmware words read once per retirement scan (per-lane stamps and ring
/// cursors are shared by every flight on the lane).
/// A fixed table on the stack: no allocation per scan. Words beyond its
/// capacity are read directly.
#[derive(Default, Clone)]
pub(crate) struct WordSnapshot {
    words: [(u64, u32); 32],
    len: usize,
}

impl WordSnapshot {
    fn read(&mut self, fw: &g16_vm::FirmwareSpace, address: u64) -> Result<u32> {
        if let Some(&(_, value)) = self.words[..self.len].iter().find(|(a, _)| *a == address) {
            return Ok(value);
        }
        let value = fw.read_u32(address)?;
        if self.len < self.words.len() {
            self.words[self.len] = (address, value);
            self.len += 1;
        }
        Ok(value)
    }

    /// A value seen by this snapshot, without reading.
    fn peek(&self, address: u64) -> Option<u32> {
        self.words[..self.len].iter().find(|(a, _)| *a == address).map(|&(_, v)| v)
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
    /// shared_notifier=1: the Work whose notifier slot this render's
    /// submission shares (see `JobNotifier`).
    job_notifier: Option<u64>,
    /// Retirement trace (fw_trace & 16): when the stage stamps were first seen
    /// complete, and which later conditions still blocked retirement.
    stamps_seen: Option<Instant<Monotonic>>,
    blocked: u32,
    /// Stamps this Work waits on before it may run, for hang reports.
    dependencies: [Option<Stamp>; 2],
    /// Firmware dependency items this flight queued on each of `queues`.
    lane_items: [u32; 2],
}

/// shared_notifier=1: one completion notifier per submission, as Metal has
/// one per command buffer. It lives in the notifier slot of the submission's
/// first published render Work (the owner), whose storage is kept until
/// every render using the notifier has retired.
pub(crate) struct JobNotifier {
    job: u64,
    owner: u64,
    /// The owner's free_work root, set once the owner itself has retired.
    owner_root: Option<u64>,
    /// Works counted into the notifier so far (two per render).
    works: u32,
    /// Published renders using the notifier and not yet retired.
    refs: u32,
}

/// Dependency items of one submission attempt, counted into dep_stats only
/// when it publishes: a deferred (EBUSY) command is resolved again on its
/// next try.
#[derive(Default)]
struct DepTally {
    items: u64,
    same_lane: u64,
    repeat: u64,
    reached: u64,
    dropped: u64,
}

impl DepTally {
    fn publish(&self) {
        use crate::g16_timeline as t;
        use core::sync::atomic::Ordering::Relaxed;
        t::DEP_WORKS.fetch_add(1, Relaxed);
        t::DEP_ITEMS.fetch_add(self.items, Relaxed);
        t::DEP_SAME_LANE.fetch_add(self.same_lane, Relaxed);
        t::DEP_REPEAT.fetch_add(self.repeat, Relaxed);
        t::DEP_REACHED.fetch_add(self.reached, Relaxed);
        t::DEP_DROPPED.fetch_add(self.dropped, Relaxed);
    }
}

/// Per-lane totals of the unretired flights on that lane.
struct LaneBook {
    pointers: u64,
    /// Flights using the lane.
    works: u32,
    /// Works plus the dependency items they queued here: one per flight using
    /// the lane, plus the items it queued on it.
    load: u32,
    /// Works plus all their dependencies, for flights whose first lane this
    /// is: one queue's total is the sum over its lanes (`QUEUE_CAP`).
    owned: u32,
}

/// A compact view of `flights` for admission, kept in step at their only
/// push and removal sites, so reserve_flight never walks the flights
/// themselves. Also caches the work channels' transport cursors: the tail
/// is written only by this driver, and stale read/done cursors are behind
/// the real ones, so a cached view can only under-report ring room.
/// Encoding scratch for one Work at a time: publication runs under the
/// runtime lock and copies the encodings into firmware memory before it
/// returns, so one set allocated with the runtime serves every Work and
/// publication itself does not allocate. On the heap: two copies of each
/// encoding on the job worker's stack would overflow it.
#[derive(Default)]
pub(crate) struct Encodings {
    tiling: KVec<u8>,
    fragment: KVec<u8>,
    compute: KVec<u8>,
}

impl Encodings {
    pub(crate) fn new() -> Result<Self> {
        let mut encodings = Self::default();
        encodings.tiling.extend_with(0x940, 0u8, GFP_KERNEL)?;
        encodings.fragment.extend_with(0xcc0, 0u8, GFP_KERNEL)?;
        encodings.compute.extend_with(0x8c0, 0u8, GFP_KERNEL)?;
        Ok(encodings)
    }
}

/// Most Works in flight at once. Flight bookkeeping is preallocated to this at
/// init so publication and retirement never allocate; at the cap, admission
/// waits for retirement (EBUSY). Logged peaks under churn are below 400.
pub(crate) const MAX_FLIGHTS: usize = 1024;

pub(crate) struct Ledger {
    lanes: KVec<LaneBook>,
    /// (lane pointers, ring head) per flight and lane.
    heads: KVec<(u64, u32)>,
    /// (channel, transport target) per flight and channel.
    targets: KVec<(usize, u32)>,
    /// Per work channel: read, done, tail.
    cursors: [Option<[u32; 3]>; 12],
}

impl Ledger {
    /// Room for `flights` flights: each adds at most two lanes, heads and targets.
    pub(crate) fn with_capacity(flights: usize) -> Result<Self> {
        Ok(Self {
            lanes: KVec::with_capacity(2 * flights, GFP_KERNEL)?,
            heads: KVec::with_capacity(2 * flights, GFP_KERNEL)?,
            targets: KVec::with_capacity(2 * flights, GFP_KERNEL)?,
            cursors: [None; 12],
        })
    }

    fn lane(&self, pointers: u64) -> Option<&LaneBook> {
        self.lanes.iter().find(|l| l.pointers == pointers)
    }

    /// Room for one more flight, so `add` cannot fail after publication.
    fn has_room(&self) -> bool {
        [
            (self.lanes.capacity(), self.lanes.len()),
            (self.heads.capacity(), self.heads.len()),
            (self.targets.capacity(), self.targets.len()),
        ]
        .iter()
        .all(|&(capacity, len)| capacity - len >= 2)
    }

    fn add(&mut self, f: &Flight) -> Result {
        let dependencies = f.dependencies.iter().flatten().count() as u32;
        for (index, (&(pointers, head), items)) in f.queues.iter().zip(f.lane_items).enumerate() {
            if pointers == 0 {
                continue;
            }
            let book = match self.lanes.iter().position(|l| l.pointers == pointers) {
                Some(i) => &mut self.lanes[i],
                None => {
                    self.lanes
                        .push_within_capacity(LaneBook { pointers, works: 0, load: 0, owned: 0 })
                        .map_err(|_| ENOMEM)?;
                    self.lanes.last_mut().ok_or(EIO)?
                }
            };
            book.works += 1;
            book.load += 1 + items;
            if index == 0 {
                book.owned += 1 + dependencies;
            }
            self.heads.push_within_capacity((pointers, head)).map_err(|_| ENOMEM)?;
        }
        for &(channel, target) in &f.channels {
            if channel != usize::MAX {
                self.targets.push_within_capacity((channel, target)).map_err(|_| ENOMEM)?;
            }
        }
        Ok(())
    }

    /// Never allocates (retirement).
    fn remove(&mut self, f: &Flight) -> Result {
        let dependencies = f.dependencies.iter().flatten().count() as u32;
        for (index, (&(pointers, head), items)) in f.queues.iter().zip(f.lane_items).enumerate() {
            if pointers == 0 {
                continue;
            }
            let i = self.lanes.iter().position(|l| l.pointers == pointers).ok_or(EIO)?;
            let book = &mut self.lanes[i];
            book.works = book.works.checked_sub(1).ok_or(EIO)?;
            book.load = book.load.checked_sub(1 + items).ok_or(EIO)?;
            if index == 0 {
                book.owned = book.owned.checked_sub(1 + dependencies).ok_or(EIO)?;
            }
            if book.works == 0 {
                self.lanes.swap_remove(i);
            }
            // Publication order, and retirement is mostly oldest first: the
            // entry is usually at the front. remove() keeps that order.
            let i = self.heads.iter().position(|&e| e == (pointers, head)).ok_or(EIO)?;
            self.heads.remove(i).map_err(|_| EIO)?;
        }
        for &(channel, target) in &f.channels {
            if channel != usize::MAX {
                let i = self.targets.iter().position(|&e| e == (channel, target)).ok_or(EIO)?;
                self.targets.remove(i).map_err(|_| EIO)?;
            }
        }
        Ok(())
    }
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

/// A per-VM firmware support object retained by every queued execution until
/// its GPU accesses finish, including after public VM/queue handle destruction.
pub(crate) struct Support {
    slots: Arc<AtomicU64>,
    index: u32,
    submissions: AtomicU64,
    /// Initialized in native pool mode (native_mesh was nonzero when this VM
    /// was created). Fixed for the support lifetime.
    native: bool,
    /// The VM's latched native mesh mode, which decodes all its renders.
    mode: g16_mesh::Mode,
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

#[derive(Default)]
struct RenderEvents {
    memory_limit: bool,
}

/// Firmware VA of the first Work arena; Work offsets from here alias into the
/// GPU windows at 0x71_0000_0000 (compute) and 0x72_0000_0000 (render).
pub(crate) const WORK_BASE: u64 = 0xffff_fc22_0000_0000;
/// Size of each alias window.
pub(crate) const WORK_WINDOW: u64 = 0x1_0000_0000;
/// Firmware VA for queue identities and TVB pools, above the Work window.
pub(crate) const OBJECT_BASE: u64 = WORK_BASE + WORK_WINDOW;

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

pub(crate) const RETIRE_LOG: usize = 256;

fn micros(later: Instant<Monotonic>, earlier: Instant<Monotonic>) -> i64 {
    (later - earlier).as_nanos() / 1000
}

/// Diagnostics: CLOCK_MONOTONIC in ns, comparable with dmesg timestamps.
fn monotonic_ns() -> i64 {
    // SAFETY: ktime_get has no preconditions.
    unsafe { kernel::bindings::ktime_get() }
}
