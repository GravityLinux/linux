// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from Niklas Sheth's linux-m4-integration prototype.

//! G16 firmware initialization descriptors, serialized independently of Rust
//! structure alignment. Fields follow the source-built m1n1 G16 model.

#[path = "g16_queue.rs"]
pub(crate) mod queue;

pub(crate) const ROOT_ADDRESS: u64 = 0xffff_fc20_0010_0000;
pub(crate) const ROOT_SIZE: usize = 0xc8;
pub(crate) const BUNDLE_ADDRESS: u64 = 0xffff_fc20_c04d_8000;
pub(crate) const BUNDLE_SIZE: usize = 0x20000;
pub(crate) const MAIN_OFFSET: usize = 0x1da40;
pub(crate) const MAIN_SIZE: usize = 0x500;
pub(crate) const CONTROL_DATA: u64 = 0xffff_fc20_c052_8000;
pub(crate) const CONTROL_AUX: u64 = 0xffff_fc20_0014_8000;
pub(crate) const OPERAND_TABLE: u64 = 0x70_0040_8000;

/// The two publications following firmware's already-consumed slot zero.
pub(crate) fn opening_control() -> [u8; 0x80] {
    let mut out = [0u8; 0x80];
    out[..4].copy_from_slice(&0x16u32.to_le_bytes());
    let record = &mut out[0x40..];
    for (offset, value) in [(0, 0x20u32), (4, 1), (8, 0x3f), (0x2c, 20 * 8), (0x34, 1)] {
        record[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    put64(record, 0x14, CONTROL_DATA);
    put64(record, 0x1c, OPERAND_TABLE);
    put64(record, 0x24, OPERAND_TABLE);
    out
}

/// Fresh opening FList; its operand descriptors are admitted by opcode 0x20.
pub(crate) fn opening_flist() -> [u8; 0x80] {
    let mut out = [0; 0x80];
    for (off, value) in [(0, 1u32), (0x1c, 0x400000 / 8), (0x28, 1)] {
        out[off..off + 4].copy_from_slice(&value.to_le_bytes());
    }
    put64(&mut out, 0x14, 0x70_0000_0000);
    put64(&mut out, 0x30, OPERAND_TABLE);
    put64(&mut out, 0x4c, CONTROL_AUX);
    out
}

/// One freshly owned parameter-buffer descriptor; firmware owns both cursors.
pub(crate) fn parameter_buffer_descriptor(page_list: u64, page_count: u32) -> Option<[u8; 16]> {
    if !(0x10_0000_0000..0x20_0000_0000).contains(&page_list)
        || page_list & 0x7f != 0
        || page_count == 0
        || page_count >= 1 << 22
    {
        return None;
    }
    let mut out = [0; 16];
    out[..4].copy_from_slice(&((page_list >> 4) as u32 & 0xfffffff8).to_le_bytes());
    out[4..8].copy_from_slice(&page_count.to_le_bytes());
    Some(out)
}

/// Addresses of 4 KiB accelerator pages within twenty guarded 2 MiB buffers.
pub(crate) fn operand_page(index: usize) -> u64 {
    0x70_0042_0000 + (index / 512) as u64 * 0x208000 + (index % 512) as u64 * 0x1000
}

fn put64(out: &mut [u8], offset: usize, value: u64) {
    out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// Three independent cursor addresses followed by the ring address.
pub(crate) struct Channel {
    pub(crate) state: [u64; 3],
    pub(crate) ring: u64,
}

pub(crate) struct MainConfig {
    pub(crate) hwdata: u64,
    pub(crate) repeated: u64,
    pub(crate) header: u64,
    pub(crate) channels: [Channel; 17],
    pub(crate) addresses: [u64; 5],
    pub(crate) regions: [u64; 6],
    pub(crate) unaligned: u64,
}

impl MainConfig {
    pub(crate) fn bootstrap(bundle: u64) -> Self {
        let rings = [
            0x10240, 0x16240, 0x1c240, 0xea40, 0x14a40, 0x1aa40, 0xd240, 0x13240, 0x19240, 0xba40,
            0x11a40, 0x17a40,
        ];
        let states = [
            (0, 0),
            (0x40, 0x20),
            (0x80, 0x40),
            (0x10, 8),
            (0x50, 0x28),
            (0x90, 0x48),
            (0x20, 0x10),
            (0x60, 0x30),
            (0xa0, 0x50),
            (0x30, 0x18),
            (0x70, 0x38),
            (0xb0, 0x58),
        ];
        let channels = core::array::from_fn(|i| {
            if i < 12 {
                let a = 0xffff_fc20_0002_8000 + states[i].0;
                Channel {
                    state: [a, a + 8, 0xffff_fc20_0002_0000 + states[i].1],
                    ring: bundle + rings[i],
                }
            } else {
                let (state, ring) = match i {
                    12 => (
                        [
                            0xffff_fc20_0002_80c0,
                            0xffff_fc20_0002_80c8,
                            0xffff_fc20_0002_0060,
                        ],
                        BUNDLE_ADDRESS + 0x1df00,
                    ),
                    13 => (
                        [
                            0xffff_fc20_0003_f1c0,
                            0xffff_fc20_0003_f440,
                            0xffff_fc20_0003_f200,
                        ],
                        0xffff_fc20_0004_3c40,
                    ),
                    14 => (
                        [
                            0xffff_fc20_0003_f3c0,
                            0xffff_fc20_000e_5c40,
                            0xffff_fc20_0003_f400,
                        ],
                        0xffff_fc20_000e_ec40,
                    ),
                    15 => ([0xffff_fc20_0006_c440, 0, 0], 0),
                    _ => ([0, 0, 0], 0),
                };
                Channel { state, ring }
            }
        });
        Self {
            hwdata: bundle,
            repeated: bundle + 0xb980,
            header: 0xffff_fc20_0002_0068,
            channels,
            addresses: [
                0,
                bundle + 0x2680,
                bundle + 0x32c0,
                bundle + 0x4540,
                bundle + 0xb900,
            ],
            regions: [
                0xffff_fc20_c051_8000,
                0x70_0027_8000,
                0xffff_fc20_0012_8000,
                0x70_0028_0000,
                0xffff_fc20_0013_0000,
                0,
            ],
            unaligned: bundle + 0x5380,
        }
    }

    pub(crate) fn encode(&self) -> [u8; MAIN_SIZE] {
        let mut out = [0; MAIN_SIZE];
        for (offset, value) in [
            (0, self.hwdata),
            (8, self.repeated),
            (16, self.repeated),
            (24, self.header),
            (0x469, self.unaligned),
        ] {
            put64(&mut out, offset, value);
        }
        for (i, channel) in self.channels.iter().enumerate() {
            let base = 0x20 + i * 0x20;
            for (j, value) in channel.state.iter().enumerate() {
                put64(&mut out, base + j * 8, *value);
            }
            put64(&mut out, base + 24, channel.ring);
        }
        for (i, value) in self.addresses.iter().enumerate() {
            put64(&mut out, 0x24c + i * 8, *value);
        }
        for (i, value) in self.regions.iter().enumerate() {
            put64(&mut out, 0x2c8 + i * 8, *value);
        }
        for (offset, value) in [(0x2f8, 4u32), (0x3d8, 0xff), (0x4c0, 0x16)] {
            out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        out
    }
}

pub(crate) struct RootPointers {
    pub(crate) region_a: u64,
    pub(crate) main_config: u64,
    pub(crate) region_c: u64,
    pub(crate) status_a: u64,
    pub(crate) status_b: u64,
    pub(crate) extra_0: u64,
    pub(crate) extra_1: u64,
}

impl RootPointers {
    /// Initial firmware namespace. Client addresses are allocated separately.
    pub(crate) const fn bootstrap() -> Self {
        Self {
            region_a: 0xffff_fc20_0010_8000,
            main_config: BUNDLE_ADDRESS + MAIN_OFFSET as u64,
            region_c: 0xffff_fc20_000f_8000,
            status_a: 0xffff_fc20_0003_f180,
            status_b: 0xffff_fc20_0003_0000,
            extra_0: 0xffff_fc20_0003_eb00,
            extra_1: 0xffff_fc20_000f_3440,
        }
    }

    pub(crate) fn encode(&self) -> [u8; ROOT_SIZE] {
        let mut out = [0u8; ROOT_SIZE];
        for (i, value) in [0x04b8u16, 0x8392, 0xc357, 0x0c89].iter().enumerate() {
            out[i * 2..i * 2 + 2].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, value) in [
            (0x08, self.region_a),
            (0x18, self.main_config),
            (0x20, self.region_c),
            (0xa8, self.status_a),
            (0xb0, self.status_b),
            (0xb8, self.extra_0),
            (0xc0, self.extra_1),
        ] {
            out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        out[0x2c..0x30].copy_from_slice(&1u32.to_le_bytes());
        out[0x30..0x32].copy_from_slice(&0x4000u16.to_le_bytes());
        out[0x32] = 14;
        out[0x33] = 3;
        for (i, (shift, entries)) in [(36u8, 64u16), (25, 2048), (14, 2048)].iter().enumerate() {
            let level = &mut out[0x34 + i * 0x20..0x54 + i * 0x20];
            level[..4].copy_from_slice(&[8, 14, 14, *shift]);
            level[4..6].copy_from_slice(&entries.to_le_bytes());
            level[6..8].copy_from_slice(&0x4000u16.to_le_bytes());
            level[8..16].copy_from_slice(&1u64.to_le_bytes());
            level[16..24].copy_from_slice(&0x0000_03ff_ffff_c000u64.to_le_bytes());
            level[24..32].copy_from_slice(&((u64::from(*entries) - 1) << shift).to_le_bytes());
        }
        out
    }
}
