// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! Per-submission timeline records (asahi_m4.timeline=1/2), kept in a
//! fixed-size ring and read as binary from debugfs
//! (`/sys/kernel/debug/asahi_m4/timeline`).
//!
//! The file is a header of `HEADER_WORDS` little-endian u64 words followed by
//! `capacity` records of `RECORD_WORDS` u64 words, in slot order:
//!
//! header: magic, version, record bytes, capacity, records written, 0, 0, 0
//! record: seq (1-based, 0 = empty slot), exec id, pid, file id,
//!         queue | cmds << 32, pending inputs | total inputs << 32,
//!         deferrals | error << 32, deferral reasons,
//!         then CNTVCT ticks (1 GHz, 0 = never) at: ioctl, scheduler run,
//!         first and last publication, completion, doorbell, IRQ, RTKit
//!         delivery, pump wake, retire scan; two reserved words.
//!
//! Slot = (seq - 1) % capacity, so after wrap-around the oldest records are
//! overwritten. Reads hold the ring lock, so read after the measurement window.

use kernel::debugfs::BinaryWriter;
use kernel::fs::file;
use kernel::prelude::*;
use kernel::sync::Mutex;
use kernel::uaccess::UserSliceWriter;

const MAGIC: u64 = u64::from_le_bytes(*b"G16TLRNG");
const VERSION: u64 = 1;
const HEADER_WORDS: usize = 8;
pub(crate) const RECORD_WORDS: usize = 20;

/// One submission, as `trace_line` used to print it, with absolute ticks.
pub(crate) struct Record {
    pub(crate) exec: u64,
    pub(crate) pid: u32,
    pub(crate) file: u64,
    pub(crate) queue: u32,
    pub(crate) cmds: u32,
    pub(crate) pending: u32,
    pub(crate) inputs: u32,
    pub(crate) busy: u32,
    pub(crate) error: bool,
    pub(crate) why: u64,
    /// ioctl, run, first, last, done, kick, irq, rx, wake, seen.
    pub(crate) ticks: [u64; 10],
}

struct Inner {
    words: KVVec<u64>,
    written: u64,
    capacity: u64,
}

#[pin_data]
pub(crate) struct Ring {
    #[pin]
    inner: Mutex<Inner>,
}

impl Ring {
    pub(crate) fn new(capacity: usize) -> Result<impl PinInit<Self>> {
        let capacity = capacity.max(1);
        let len = capacity
            .checked_mul(RECORD_WORDS)
            .and_then(|n| n.checked_add(HEADER_WORDS))
            .ok_or(EINVAL)?;
        let mut words = KVVec::with_capacity(len, GFP_KERNEL)?;
        words.resize(len, 0, GFP_KERNEL)?;
        words[0] = MAGIC;
        words[1] = VERSION;
        words[2] = (RECORD_WORDS * 8) as u64;
        words[3] = capacity as u64;
        let inner = Inner {
            words,
            written: 0,
            capacity: capacity as u64,
        };
        Ok(pin_init!(Self {
            inner <- kernel::new_mutex!(inner),
        }))
    }

    pub(crate) fn push(&self, r: &Record) {
        let mut g = self.inner.lock();
        g.written += 1;
        let seq = g.written;
        let base = HEADER_WORDS + ((seq - 1) % g.capacity) as usize * RECORD_WORDS;
        g.words[4] = seq;
        let w = &mut g.words[base..base + RECORD_WORDS];
        w[0] = seq;
        w[1] = r.exec;
        w[2] = r.pid as u64;
        w[3] = r.file;
        w[4] = r.queue as u64 | (r.cmds as u64) << 32;
        w[5] = r.pending as u64 | (r.inputs as u64) << 32;
        w[6] = r.busy as u64 | (r.error as u64) << 32;
        w[7] = r.why;
        w[8..18].copy_from_slice(&r.ticks);
        w[18] = 0;
        w[19] = 0;
    }
}

impl BinaryWriter for Ring {
    fn write_to_slice(
        &self,
        writer: &mut UserSliceWriter,
        offset: &mut file::Offset,
    ) -> Result<usize> {
        let g = self.inner.lock();
        // SAFETY: u64 has no padding; the slice covers exactly `words`.
        let bytes = unsafe {
            core::slice::from_raw_parts(g.words.as_ptr().cast::<u8>(), g.words.len() * 8)
        };
        writer.write_slice_file(bytes, offset)
    }
}

/// timeline=2: cumulative CNTVCT ticks and call counts of publish-path
/// sections, read as text from debugfs `asahi_m4/publish_profile`. Counters
/// only grow; diff two reads to bracket a window.
pub(crate) struct Section {
    name: &'static str,
    calls: core::sync::atomic::AtomicU64,
    ticks: core::sync::atomic::AtomicU64,
}

impl Section {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            calls: core::sync::atomic::AtomicU64::new(0),
            ticks: core::sync::atomic::AtomicU64::new(0),
        }
    }
}

pub(crate) static P_SUBMIT_RENDER: Section = Section::new("submit_render");
pub(crate) static P_SUBMIT_COMPUTE: Section = Section::new("submit_compute");
pub(crate) static P_PREPARE_RENDER: Section = Section::new("prepare_render");
pub(crate) static P_PREPARE_COMPUTE: Section = Section::new("prepare_compute");
pub(crate) static P_LIVE: Section = Section::new("fw write_live/zero_live");
pub(crate) static P_READ: Section = Section::new("fw read_u32");
pub(crate) static P_FW_SYNC: Section = Section::new("fw sync");
pub(crate) static P_VM_SYNC: Section = Section::new("vm sync");
pub(crate) static P_WRITE_LOW: Section = Section::new("vm write_low");
pub(crate) static P_FLUSH: Section = Section::new("flush (doorbell)");
pub(crate) static P_PUMP: Section = Section::new("pump pass");
/// Read attribution (calls only): firmware reads made inside each site.
pub(crate) static P_RD_EVENTS: Section = Section::new("reads: event ring");
pub(crate) static P_RD_SCAN: Section = Section::new("reads: retire scan");
pub(crate) static P_RD_ADMIT: Section = Section::new("reads: admission cursors");
pub(crate) static P_RD_ELIDE: Section = Section::new("reads: dep_elide reached");
pub(crate) static P_RD_WEDGE: Section = Section::new("reads: wedge check");

static SECTIONS: [&Section; 16] = [
    &P_SUBMIT_RENDER,
    &P_SUBMIT_COMPUTE,
    &P_PREPARE_RENDER,
    &P_PREPARE_COMPUTE,
    &P_LIVE,
    &P_READ,
    &P_FW_SYNC,
    &P_VM_SYNC,
    &P_WRITE_LOW,
    &P_FLUSH,
    &P_PUMP,
    &P_RD_EVENTS,
    &P_RD_SCAN,
    &P_RD_ADMIT,
    &P_RD_ELIDE,
    &P_RD_WEDGE,
];

/// Firmware reads so far (counted only while profiling).
#[inline(always)]
pub(crate) fn reads() -> u64 {
    P_READ.calls.load(core::sync::atomic::Ordering::Relaxed)
}

/// Attribute the firmware reads since `before` to `site` (profiling only).
#[inline(always)]
pub(crate) fn attribute(site: &Section, before: u64) {
    if PROFILE.load(core::sync::atomic::Ordering::Relaxed) != 0 {
        site.calls.fetch_add(reads().wrapping_sub(before), core::sync::atomic::Ordering::Relaxed);
    }
}

/// Section timing switch (asahi_m4.profile, runtime: debugfs
/// `asahi_m4/profile`). Off by default: each timed section costs two
/// serialized counter reads, which a pump-bound load pays per firmware read.
pub(crate) static PROFILE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Times a section until dropped, when profiling is on.
pub(crate) struct Timer(Option<(u64, &'static Section)>);

impl Timer {
    #[inline(always)]
    pub(crate) fn start(section: &'static Section) -> Self {
        if PROFILE.load(core::sync::atomic::Ordering::Relaxed) != 0 {
            Timer(Some((crate::g16_drm::ticks(), section)))
        } else {
            Timer(None)
        }
    }
}

impl Drop for Timer {
    #[inline(always)]
    fn drop(&mut self) {
        use core::sync::atomic::Ordering::Relaxed;
        if let Some((start, s)) = self.0 {
            s.calls.fetch_add(1, Relaxed);
            s.ticks.fetch_add(crate::g16_drm::ticks().wrapping_sub(start), Relaxed);
        }
    }
}

/// The debugfs view of the sections.
pub(crate) struct Profile;

impl kernel::debugfs::Writer for Profile {
    fn write(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use core::sync::atomic::Ordering::Relaxed;
        writeln!(f, "# section calls ticks(1GHz) avg_ns")?;
        for s in SECTIONS.iter() {
            let (c, t) = (s.calls.load(Relaxed), s.ticks.load(Relaxed));
            writeln!(f, "{}\t{}\t{}\t{}", s.name, c, t, if c != 0 { t / c } else { 0 })?;
        }
        Ok(())
    }
}

/// Shared bounded text read; individual knobs retain their accepted syntax.
fn knob_text<'a>(reader: &mut kernel::uaccess::UserSliceReader, buf: &'a mut [u8; 8]) -> Result<&'a str> {
    let n = reader.len();
    if n == 0 || n > buf.len() {
        return Err(EINVAL);
    }
    reader.read_slice(&mut buf[..n])?;
    Ok(core::str::from_utf8(&buf[..n]).map_err(|_| EINVAL)?.trim())
}

fn knob_u32(reader: &mut kernel::uaccess::UserSliceReader) -> Result<u32> {
    knob_text(reader, &mut [0; 8])?.parse().map_err(|_| EINVAL)
}

/// Runtime copies of `max_queue_entries` and `max_lane_entries`, writable from
/// debugfs `asahi_m4/queue_cap` and `asahi_m4/lane_cap` for same-session A/B.
/// Every lane must stay below the firmware's 128-Work window for dependency
/// items (k24: 127 clean, 129 stalled), so writes keep one bound in force: the queue cap at most 127
/// (it bounds every lane), or a lane cap of at most 127.
pub(crate) static QUEUE_CAP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(96);
pub(crate) static LANE_CAP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn caps_safe(queue: u32, lane: u32) -> bool {
    (1..=127).contains(&queue) || (1..=127).contains(&lane)
}

/// The debugfs view of one cap.
pub(crate) struct CapKnob {
    lane: bool,
}

impl CapKnob {
    pub(crate) const QUEUE: Self = Self { lane: false };
    pub(crate) const LANE: Self = Self { lane: true };
}

impl kernel::debugfs::Writer for CapKnob {
    fn write(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let cap = if self.lane { &LANE_CAP } else { &QUEUE_CAP };
        writeln!(f, "{}", cap.load(core::sync::atomic::Ordering::Relaxed))
    }
}

impl kernel::debugfs::Reader for CapKnob {
    fn read_from_slice(&self, reader: &mut kernel::uaccess::UserSliceReader) -> Result {
        use core::sync::atomic::Ordering::Relaxed;
        let value = knob_u32(reader)?;
        let (queue, lane) = if self.lane {
            (QUEUE_CAP.load(Relaxed), value)
        } else {
            (value, LANE_CAP.load(Relaxed))
        };
        if !caps_safe(queue, lane) || lane > 127 {
            return Err(EINVAL);
        }
        (if self.lane { &LANE_CAP } else { &QUEUE_CAP }).store(value, Relaxed);
        pr_info!("G16: queue_cap={} lane_cap={}\n", queue, lane);
        Ok(())
    }
}

/// Validates the boot parameters with the same rule as the knobs.
pub(crate) fn init_caps(queue: u32, lane: u32) -> Result {
    if !caps_safe(queue, lane) || lane > 127 {
        pr_err!("G16: max_queue_entries={} max_lane_entries={} leave a lane unbounded\n", queue, lane);
        return Err(EINVAL);
    }
    QUEUE_CAP.store(queue, core::sync::atomic::Ordering::Relaxed);
    LANE_CAP.store(lane, core::sync::atomic::Ordering::Relaxed);
    Ok(())
}

/// Test: one-shot failure injection for queue-failure tests (debugfs
/// `fail_prep`, `fail_retire`, `fail_skip`). Writing a TGID arms the next
/// command of that process to fail preparation with EINVAL, or to retire
/// with ENOMEM as a TVB memory-limit render does, after letting `fail_skip`
/// matching commands pass. 0 (default) disarms; nothing else is affected.
pub(crate) static FAIL_PREP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub(crate) static FAIL_RETIRE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub(crate) static FAIL_SKIP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Whether an armed injection in `cell` fires for a command of `tgid`;
/// firing disarms it.
pub(crate) fn take_injection(cell: &core::sync::atomic::AtomicU32, tgid: i32) -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    let armed = cell.load(Relaxed);
    if armed == 0 || armed as i32 != tgid {
        return false;
    }
    if FAIL_SKIP.fetch_update(Relaxed, Relaxed, |n| n.checked_sub(1)).is_ok() {
        return false;
    }
    let fired = cell.compare_exchange(armed, 0, Relaxed, Relaxed).is_ok();
    if fired {
        pr_info!("G16: injected {} failure for tgid {}\n",
                 if core::ptr::eq(cell, &FAIL_PREP) { "preparation" } else { "retirement" }, tgid);
    }
    fired
}

/// One completion notifier per submission, shared by its renders (1,
/// default, as Metal shares one per command buffer), or each render's own
/// (0, the fallback). asahi_m4.shared_notifier; runtime: debugfs
/// asahi_m4/shared_notifier, for renders published afterwards.
pub(crate) static SHARED_NOTIFIER: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

/// Whether new renders share their submission's notifier (see `SHARED_NOTIFIER`).
pub(crate) fn shared_notifier() -> bool {
    SHARED_NOTIFIER.load(core::sync::atomic::Ordering::Relaxed) != 0
}

/// A 0/1 debugfs switch over a static.
pub(crate) struct FlagKnob {
    cell: &'static core::sync::atomic::AtomicU32,
    name: &'static str,
    max: u32,
}

impl FlagKnob {
    pub(crate) fn init(&self, value: u32) {
        self.cell.store(value.min(self.max), core::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) const PROFILE: Self = Self { cell: &PROFILE, name: "profile", max: 1 };
    pub(crate) const SHARED_NOTIFIER: Self = Self { cell: &SHARED_NOTIFIER, name: "shared_notifier", max: 1 };
    pub(crate) const FAIL_PREP: Self = Self { cell: &FAIL_PREP, name: "fail_prep", max: i32::MAX as u32 };
    pub(crate) const FAIL_RETIRE: Self = Self { cell: &FAIL_RETIRE, name: "fail_retire", max: i32::MAX as u32 };
    pub(crate) const FAIL_SKIP: Self = Self { cell: &FAIL_SKIP, name: "fail_skip", max: 4096 };
}

/// debugfs `asahi_m4/native_mesh`: the runtime native mesh mode. Each write
/// starts a new generation (see `g16_mesh::set_mode`).
pub(crate) struct NativeMeshKnob;

impl kernel::debugfs::Writer for NativeMeshKnob {
    fn write(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "{}", crate::g16_mesh::mode())
    }
}

impl kernel::debugfs::Reader for NativeMeshKnob {
    fn read_from_slice(&self, reader: &mut kernel::uaccess::UserSliceReader) -> Result {
        let value = knob_u32(reader)?;
        if value > crate::g16_mesh::MAX_MODE {
            return Err(EINVAL);
        }
        let generation = crate::g16_mesh::set_mode(value);
        pr_info!("G16: native_mesh={} (generation {})\n", value, generation);
        Ok(())
    }
}

impl kernel::debugfs::Writer for FlagKnob {
    fn write(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "{}", self.cell.load(core::sync::atomic::Ordering::Relaxed))
    }
}

impl kernel::debugfs::Reader for FlagKnob {
    fn read_from_slice(&self, reader: &mut kernel::uaccess::UserSliceReader) -> Result {
        let value = knob_u32(reader)?;
        if value > self.max {
            return Err(EINVAL);
        }
        self.cell.store(value, core::sync::atomic::Ordering::Relaxed);
        pr_info!("G16: {}={}\n", self.name, value);
        Ok(())
    }
}

/// Firmware dependency-item counters (see elide_item), in debugfs
/// `asahi_m4/dep_stats`.
pub(crate) static DEP_ITEMS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static DEP_SAME_LANE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static DEP_REACHED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static DEP_REPEAT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static DEP_DROPPED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static DEP_WORKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The debugfs view of the dependency-item counters.
pub(crate) struct DepStats;

impl kernel::debugfs::Writer for DepStats {
    fn write(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use core::sync::atomic::Ordering::Relaxed;
        writeln!(
            f,
            "works {}\nitems {}\nsame_lane {}\nrepeat {}\nreached {}\ndropped {}\npoll_timeouts {}\npoll_retired {}\n\
             tvb_requests {}\ntvb_grown {}\ntvb_refused {}\ntvb_memory_limit {}\npartial_renders {}\n\
             tvb_refused_cap {}\ntvb_refused_va {}\ntvb_refused_memory {}\nsubmit_rejected {}\n\
             deferred {}\ndeferred_asids {}\ndeferred_channel {}\ndeferred_scenes {}\ndeferred_events {}\n\
             deferred_queue {}\ndeferred_lane_entries {}\ndeferred_work_storage {}\ndeferred_flights {}\n\
             deferred_cpu_hold {}",
            DEP_WORKS.load(Relaxed),
            DEP_ITEMS.load(Relaxed),
            DEP_SAME_LANE.load(Relaxed),
            DEP_REPEAT.load(Relaxed),
            DEP_REACHED.load(Relaxed),
            DEP_DROPPED.load(Relaxed),
            POLL_TIMEOUTS.load(Relaxed),
            POLL_RETIRED.load(Relaxed),
            TVB_REQUESTS.load(Relaxed),
            TVB_GROWN.load(Relaxed),
            TVB_REFUSED.load(Relaxed),
            TVB_MEMORY_LIMIT.load(Relaxed),
            PARTIAL_RENDERS.load(Relaxed),
            TVB_REFUSED_CAP.load(Relaxed),
            TVB_REFUSED_VA.load(Relaxed),
            TVB_REFUSED_MEMORY.load(Relaxed),
            SUBMIT_REJECTED.load(Relaxed),
            DEFERRED.load(Relaxed),
            DEFERRED_BY[0].load(Relaxed),
            DEFERRED_BY[1].load(Relaxed),
            DEFERRED_BY[2].load(Relaxed),
            DEFERRED_BY[3].load(Relaxed),
            DEFERRED_BY[4].load(Relaxed),
            DEFERRED_BY[5].load(Relaxed),
            DEFERRED_BY[6].load(Relaxed),
            DEFERRED_BY[7].load(Relaxed),
            DEFERRED_CPU_HOLD.load(Relaxed)
        )
    }
}

/// With work outstanding, the pump rechecks completion at least every this
/// many ms, so a late or lost notification costs milliseconds. Counters in
/// dep_stats: poll_timeouts (waits that timed out), poll_retired (Works those
/// polls retired, i.e. completions whose notification was late or lost).
pub(crate) const SAFETY_POLL_MS: u32 = 50;
pub(crate) static POLL_TIMEOUTS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static POLL_RETIRED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Rate limit for "retired 0" poll warnings (slow work, not a fault): the
/// CNTVCT tick of the last one printed, and how many were skipped since.
pub(crate) static SLOW_POLL_REPORTED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static SLOW_POLL_SUPPRESSED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// TVB counters in dep_stats: firmware allocation requests (render Event 6),
/// those answered with growth or refused (at tvb_max_blocks or under memory
/// pressure), memory-limit Events (7) that follow a refusal, and Start3D
/// partial-render entries seen on the firmware trace ring.
pub(crate) static TVB_REQUESTS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static TVB_GROWN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static TVB_REFUSED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static TVB_MEMORY_LIMIT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static PARTIAL_RENDERS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Refusals by reason (g16_tvb::Refusal); they sum to tvb_refused.
pub(crate) static TVB_REFUSED_CAP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static TVB_REFUSED_VA: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static TVB_REFUSED_MEMORY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// DRM_IOCTL_ASAHI_SUBMIT calls rejected with EINVAL or ENOTSUPP; each names
/// its check in the kernel log, up to a per-boot budget.
pub(crate) static SUBMIT_REJECTED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// Commands deferred (EBUSY) and retried after a retirement, and the
/// backpressure that deferred them, by report_pressure bit: ASIDs, channel,
/// scenes, events, queue entries, lane entries, Work storage, flights. The
/// reasons count per reporting site, and one attempt can report more than
/// one, so they may sum to more than `deferred`.
pub(crate) static DEFERRED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
pub(crate) static DEFERRED_BY: [core::sync::atomic::AtomicU64; 8] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 8];
/// Deferrals for a dependency held on the CPU, a host round trip each: a
/// compute waiting on another queue's stamp, or a native render's producers.
pub(crate) static DEFERRED_CPU_HOLD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Debug: read-only view of the firmware regions that live as long as the
/// firmware (debugfs `asahi_m4/fw_mem`), for sampling the firmware's own
/// state (power and performance controllers, statistics) while work runs.
/// File offset = firmware VA - `FW_MEM_BASE`; offsets and lengths are whole
/// 32-bit words, and a read stops at the first unmapped page (ENXIO if that
/// is the first). Reads take no driver lock and do no cache maintenance
/// (firmware memory is coherent).
pub(crate) const FW_MEM_BASE: u64 = 0xffff_fc00_0000_0000;

pub(crate) struct FwMem {
    /// Page VA and backing page, sorted by VA.
    pages: KVec<(u64, kernel::addr::PhysicalAddr)>,
}

impl FwMem {
    /// The pages must stay allocated for as long as this view exists.
    pub(crate) fn new(pages: KVec<(u64, kernel::addr::PhysicalAddr)>) -> Self {
        Self { pages }
    }
}

impl BinaryWriter for FwMem {
    fn write_to_slice(
        &self,
        writer: &mut UserSliceWriter,
        offset: &mut file::Offset,
    ) -> Result<usize> {
        const PGSZ: usize = crate::pgtable::UAT_PGSZ;
        let start = u64::try_from(*offset).map_err(|_| EINVAL)?;
        if start & 3 != 0 {
            return Err(EINVAL);
        }
        let want = writer.len().min(0x40000) & !3;
        let mut buf = crate::g16_platform::zeroed(want)?;
        let mut va = FW_MEM_BASE.checked_add(start).ok_or(EINVAL)?;
        let mut done = 0;
        while done < want {
            let page_va = va & !(PGSZ as u64 - 1);
            let Ok(index) = self.pages.binary_search_by_key(&page_va, |p| p.0) else {
                break;
            };
            let within = (va - page_va) as usize;
            let n = (PGSZ - within).min(want - done);
            // SAFETY: the page is one of the firmware's bootstrap regions,
            // never released, and freed only after this file is removed.
            let page = unsafe { kernel::page::Page::borrow_phys_unchecked(&self.pages[index].1) };
            page.with_pointer_into_page(within, n, |p| {
                for word in 0..n / 4 {
                    // SAFETY: the aligned word lies in this mapped page;
                    // firmware may write it concurrently, hence volatile.
                    let value = unsafe { p.add(word * 4).cast::<u32>().read_volatile() };
                    buf[done + word * 4..done + word * 4 + 4].copy_from_slice(&value.to_ne_bytes());
                }
                Ok(())
            })?;
            done += n;
            va += n as u64;
        }
        if done == 0 && want != 0 {
            return Err(ENXIO);
        }
        writer.write_slice(&buf[..done])?;
        *offset += done as file::Offset;
        Ok(done)
    }
}
