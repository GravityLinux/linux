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
