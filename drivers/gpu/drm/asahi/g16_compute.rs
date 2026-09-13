// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from Niklas Sheth's linux-m4-integration prototype.

//! T8132 compute work and microsequence encoding. All links are supplied by
//! the owner of the work allocation, independently of Rust structure layout.

pub(crate) const QUEUE: u64 = 0xffff_fc20_c500_0000;
pub(crate) const STATS: u64 = QUEUE + 0x8000;
pub(crate) const SHARED: u64 = 0xffff_fc20_0200_0000;
pub(crate) const PRIVATE_SIZE: usize = 0x24000;

// Fixed opening compute namespace used by the working shim. These pages
// belong to the device and are aliased into whichever client root is active.
pub(crate) const DIRECTORY: u64 = 0x70_02cc_0000;
pub(crate) const DIRECTORY_SIZE: usize = 0x18000;
pub(crate) const BUFFER_BASE: u64 = 0x70_0310_8000;
pub(crate) const BUFFER_STRIDE: u64 = 0x208000;
pub(crate) const BUFFER_SIZE: usize = 0x200000;
pub(crate) const BUFFER_COUNT: usize = 16;
pub(crate) const SCRATCH: u64 = 0x70_030e_0000;
pub(crate) const MARKER: u64 = 0x10_00c6_0000;
pub(crate) const SUPPORT: u64 = 0xffff_fc20_c05e_8000;
pub(crate) const SHARED_STATE: u64 = 0xffff_fc20_001b_0000;
pub(crate) const ENTRY_OFFSET: u64 = 0x1a00;

/// Drain/invalidate compute caches before entering the caller's CDM stream.
/// Back-to-back M4 Works require the full barrier used by Mesa's CDM helper;
/// 0x60000168 does not preserve dependent SSBO writes across context views.
pub(crate) fn entry(stream: u64) -> [u8; 12] {
    let mut out = [0; 12];
    u32_at(&mut out, 0, 0x600fffff);
    u32_at(&mut out, 4, 0x20000000 | (stream >> 32) as u32);
    u32_at(&mut out, 8, stream as u32);
    out
}

pub(crate) fn private_ranges() -> [(u64, usize); 19] {
    core::array::from_fn(|i| match i {
        0 => (DIRECTORY, DIRECTORY_SIZE),
        1 => (SCRATCH, 0x20000),
        2 => (MARKER, 0x4000),
        _ => (BUFFER_BASE + (i - 3) as u64 * BUFFER_STRIDE, BUFFER_SIZE),
    })
}

pub(crate) fn support() -> [u8; 0x100] {
    let mut out = [0; 0x100];
    let pages = (BUFFER_COUNT * BUFFER_SIZE / 0x1000) as u32;
    u64_at(&mut out, 0, 2);
    u64_at(&mut out, 0x14, DIRECTORY);
    u32_at(&mut out, 0x1c, (DIRECTORY_SIZE / 8) as u32);
    u32_at(&mut out, 0x24, pages % (DIRECTORY_SIZE / 8) as u32);
    u32_at(&mut out, 0x2c, pages);
    u64_at(&mut out, 0x4c, SHARED_STATE);
    out
}

fn u32_at(out: &mut [u8], off: usize, value: u32) {
    out[off..off + 4].copy_from_slice(&value.to_le_bytes());
}
fn u64_at(out: &mut [u8], off: usize, value: u64) {
    out[off..off + 8].copy_from_slice(&value.to_le_bytes());
}

#[derive(Clone, Copy)]
pub(crate) struct Parameters {
    pub(crate) cdm: u64,
    pub(crate) cdm_end: u64,
    pub(crate) sampler: u64,
    pub(crate) sampler_count: u32,
    pub(crate) scratch: u64,
    pub(crate) marker: u64,
    pub(crate) resource: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct Addresses {
    pub(crate) work: u64,
    pub(crate) microsequence: u64,
    pub(crate) register_table: u64,
    pub(crate) queue: u64,
    pub(crate) stats: u64,
    pub(crate) support: u64,
    pub(crate) notifier: u64,
    pub(crate) threshold: u64,
    pub(crate) driver_stamp: u64,
    pub(crate) firmware_stamp: u64,
    pub(crate) timestamp_start: u64,
    pub(crate) timestamp_end: u64,
    pub(crate) context: u32,
    pub(crate) identity: u64,
    pub(crate) register_identity: u64,
    pub(crate) counter: u64,
    pub(crate) event_generation: u32,
    pub(crate) stamp: u32,
    pub(crate) event: u32,
    pub(crate) ordinal: u32,
    pub(crate) cdm_entry: Option<u64>,
}

impl Addresses {
    /// All Work storage is freshly allocated and retained by its owner.
    pub(crate) fn publication(index: u64, ordinal: u64, work: u64, alias: u64) -> Option<Self> {
        if index > 0x00ff_fff0 || ordinal > u32::MAX as u64 {
            return None;
        }
        Some(Self {
            work,
            microsequence: work + 0x1000,
            register_table: alias + 0x20,
            queue: QUEUE,
            stats: STATS,
            support: SUPPORT,
            notifier: work + 0x1400,
            threshold: work + 0x1800,
            driver_stamp: work + 0x1600,
            firmware_stamp: work + 0x1680,
            timestamp_start: work + 0x1700,
            timestamp_end: work + 0x1708,
            context: 1,
            identity: (1u64 << 32) + 1 + index,
            register_identity: (1u64 << 32) + 1,
            counter: index + 1,
            event_generation: (index + 1) as u32,
            stamp: ((index + 1) * 0x100) as u32,
            event: 4,
            ordinal: ordinal as u32,
            cdm_entry: None,
        })
    }

    pub(crate) fn notifier(&self, count: u32) -> [u8; 0x100] {
        let mut out = [0; 0x100];
        u64_at(&mut out, 0, self.threshold);
        for (off, val) in [
            (8, self.event_generation),
            (12, count),
            (16, 0x50),
            (0x24, self.context),
        ] {
            u32_at(&mut out, off, val);
        }
        out
    }
}

impl Parameters {
    fn valid(&self) -> bool {
        self.cdm != 0
            && self.cdm & 3 == 0
            && self.cdm_end > self.cdm
            && self.cdm_end & 3 == 0
            && self.sampler & 7 == 0
            && self.sampler_count < u32::MAX
            && (self.sampler_count == 0) == (self.sampler == 0)
    }

    pub(crate) fn work(&self, a: &Addresses) -> Option<[u8; 0x8c0]> {
        if !self.valid() {
            return None;
        }
        let mut out = [0; 0x8c0];
        u32_at(&mut out, 0, 3);
        u64_at(&mut out, 4, a.counter);
        u32_at(&mut out, 12, a.context);
        u64_at(&mut out, 16, a.notifier);
        let registers = [
            (0x1a510, self.resource),
            (0x1a420, a.cdm_entry.unwrap_or(self.cdm)),
            (0x1a4d0, self.resource + 0x1480),
            (0x1a4d8, self.resource + 0x1488),
            (0x1a4e0, self.resource + 0x1490),
            (0x1a4e8, self.resource + 0x1498),
            (0x1a440, 0x154024201),
            (0x1a458, 0x10c08ae0),
            (0x101d9, 0x1c),
            (0x1a089, 0),
            (0x1a091, 0),
            (0x1a059, 0),
            (0x1a061, 0),
            (0x1a0b9, 0),
            (0x1a0c1, 0),
            (0x101d1, 0),
            (0x0d479, 0),
            (0x1a0e9, 8),
            (0x107a1, 0x00ff0000),
            (0x0a599, 0x20000400020),
            (0x0d411, 0x200000001),
            (0x1a540, a.register_identity),
            (0x014a9, a.register_identity),
            (0x0a351, a.register_identity),
        ];
        for (index, (reg, value)) in registers.iter().enumerate() {
            u32_at(&mut out, 0x20 + index * 12, *reg);
            u64_at(&mut out, 0x24 + index * 12, *value);
        }
        for (off, value) in [
            (0x720, a.register_table),
            (0x760, a.microsequence),
            (0x798, self.resource),
            (0x7a0, self.cdm_end - 4),
            (
                0x7e8,
                if self.sampler_count == 0 {
                    0xffff_ffff
                } else {
                    self.sampler
                },
            ),
            (0x800, a.driver_stamp),
            (0x808, a.firmware_stamp),
            (0x834, a.timestamp_start),
            (0x83c, a.timestamp_end),
            (0x86a, a.support),
            (0x883, 0),
        ] {
            u64_at(&mut out, off, value);
        }
        for (off, value) in [
            (0x728, 24 | (288 << 16)),
            (0x768, 0x300),
            (0x7c8, 0x54024201),
            (0x7cc, 1),
            (0x7e0, (a.identity >> 32) as u32),
            (0x7f0, self.sampler_count),
            (
                0x7f4,
                if self.sampler_count == 0 {
                    0
                } else {
                    self.sampler_count + 1
                },
            ),
            (0x810, a.stamp),
            (0x814, a.event),
            (0x820, a.identity as u32),
            (0x828, a.ordinal),
            (0x874, 0x21b00),
            (0x87c, 0x11700),
        ] {
            u32_at(&mut out, off, value);
        }
        out[0x872] = 1;
        out[0x88b] = 1;
        Some(out)
    }

    pub(crate) fn microsequence(&self, a: &Addresses) -> [u8; 0x300] {
        let mut out = [0; 0x300];
        for (off, value) in [
            (0, 0x0b),
            (0x2c, a.context),
            (0x30, 1),
            (0x34, a.event_generation),
            (0x50, a.identity as u32),
            (0x1b0, a.counter as u32),
            (0x1b8, a.event),
            (0x208, 1),
            (0x258, 0x0c),
            (0x26c, a.context),
            (0x27c, a.identity as u32),
            (0x28c, a.stamp),
            (0x2bc, (-0x258i32) as u32),
            (0x2d4, 0x40000002),
        ] {
            u32_at(&mut out, off, value);
        }
        for (off, value) in [
            (0x14, a.work + 0x20),
            (0x1c, a.stats),
            (0x24, a.queue),
            (0x44, a.work + 0x76c),
            (0x160, a.support),
            (0x178, self.marker),
            (0x188, self.scratch),
            (0x190, self.scratch + 0xf400),
            (0x198, self.scratch + 0x1e800),
            (0x1a0, self.scratch + 0x1f000),
            (0x1a8, a.work + 0x8ac),
            (0x25c, a.stats),
            (0x264, a.queue),
            (0x270, a.work + 0x76c),
            (0x284, a.firmware_stamp),
            (0x2b4, a.microsequence + 0x160),
            (0x2c1, a.work + 0x8ac),
            (0x2c9, a.work + 0x88c),
        ] {
            u64_at(&mut out, off, value);
        }
        for (base, opcode, update) in [(0x1bc, 0x80000003, 0x834), (0x20c, 3, 0x83c)] {
            u32_at(&mut out, base, opcode);
            for (off, value) in [
                (4, a.work + 0x82c),
                (12, a.work + 0x834),
                (20, a.work + update),
                (28, a.queue),
                (44, a.work + 0x89c),
                (52, a.work + 0x7dc),
            ] {
                u64_at(&mut out, base + off, value);
            }
            u32_at(&mut out, base + 68, a.identity as u32);
        }
        out
    }
}
