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

static SECTIONS: [&Section; 11] = [
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
];

/// Times a section until dropped, when timeline >= 2.
pub(crate) struct Timer(Option<(u64, &'static Section)>);

impl Timer {
    #[inline(always)]
    pub(crate) fn start(section: &'static Section) -> Self {
        if crate::module_parameters::timeline.value() >= 2 {
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

/// Runtime copy of `fw_cache_maint`, writable from debugfs
/// `asahi_m4/fw_cache_maint` for same-boot A/B. Maintenance uses clean+invalidate
/// (`dc civac`), so switching it back on never discards lines written while it
/// was off.
pub(crate) static FW_CACHE_MAINT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[inline(always)]
pub(crate) fn fw_cache_maint() -> bool {
    FW_CACHE_MAINT.load(core::sync::atomic::Ordering::Relaxed) != 0
}

/// The debugfs view of `FW_CACHE_MAINT` (accepts 0 or 1).
pub(crate) struct MaintKnob;

impl kernel::debugfs::Writer for MaintKnob {
    fn write(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "{}", FW_CACHE_MAINT.load(core::sync::atomic::Ordering::Relaxed))
    }
}

impl kernel::debugfs::Reader for MaintKnob {
    fn read_from_slice(&self, reader: &mut kernel::uaccess::UserSliceReader) -> Result {
        let mut buf = [0u8; 8];
        let n = reader.len();
        if n == 0 || n > buf.len() {
            return Err(EINVAL);
        }
        reader.read_slice(&mut buf[..n])?;
        let value = match core::str::from_utf8(&buf[..n]).map_err(|_| EINVAL)?.trim() {
            "0" => 0,
            "1" => 1,
            _ => return Err(EINVAL),
        };
        // Order earlier maintained writes before later unmaintained ones.
        crate::mem::sync();
        FW_CACHE_MAINT.store(value, core::sync::atomic::Ordering::Relaxed);
        pr_info!("G16: fw_cache_maint={}\n", value);
        Ok(())
    }
}
