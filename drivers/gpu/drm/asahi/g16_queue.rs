// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from Niklas Sheth's linux-m4-integration prototype.

//! G16 queue transport fields, independent of allocation addresses.

fn u32_at(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn u64_at(out: &mut [u8], offset: usize, value: u64) {
    out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub(crate) struct Queue {
    pub(crate) pointers: u64,
    pub(crate) ring: u64,
    pub(crate) jobs: u64,
    pub(crate) private: u64,
    pub(crate) context: u64,
    pub(crate) uuid: u32,
}

impl Queue {
    pub(crate) fn encode(&self) -> [u8; 0xb0] {
        let mut out = [0; 0xb0];
        for (off, value) in [
            (0, self.pointers),
            (8, self.ring),
            (0x10, self.jobs),
            (0x18, self.private),
            (0xa4, self.context),
        ] {
            u64_at(&mut out, off, value);
        }
        // Priority zero's scheduler profile.
        u64_at(&mut out, 0x38, 0xffff_ffff_ffff_0000);
        for (off, value) in [
            (0x2c, u32::MAX),
            (0x40, 1),
            (0x48, 1),
            (0x4c, u32::MAX),
            (0x50, self.uuid),
        ] {
            u32_at(&mut out, off, value);
        }
        out
    }
}

impl Queue {
    pub(crate) fn encode_priority(&self, priority: u32) -> Option<[u8; 0xb0]> {
        let (mask, first, last) = match priority {
            0 => (0xffff_ffff_ffff_0000, 1, 1),
            1 => (0xffff_ffff_0000_0000, 0, 0),
            2 => (0xffff_0000_0000_0000, 0, 2),
            3 => (0, 0, 3),
            _ => return None,
        };
        let mut out = self.encode();
        u32_at(&mut out, 0x30, priority);
        u32_at(&mut out, 0x34, priority);
        u64_at(&mut out, 0x38, mask);
        u32_at(&mut out, 0x40, first);
        u32_at(&mut out, 0x48, last);
        Some(out)
    }
}

pub(crate) fn pointers(capacity: u32, write: u32) -> [u8; 0x60] {
    let mut out = [0; 0x60];
    u32_at(&mut out, 0x40, write);
    u32_at(&mut out, 0x50, capacity);
    out
}

pub(crate) fn context() -> [u8; 0x40] {
    let mut out = [0; 0x40];
    out[..2].copy_from_slice(&[0xff, 0xff]);
    out[5] = 1;
    out[0x26] = 2;
    out[0x33] = 0xff;
    out
}

pub(crate) fn jobs(address: u64) -> [u8; 0x18] {
    let mut out = [0; 0x18];
    u64_at(&mut out, 8, address);
    out
}

pub(crate) fn barrier(stamp: u64, wait: u32, event: u32, stamp_self: u32, uuid: u32) -> [u8; 0x40] {
    let mut out = [0; 0x40];
    u32_at(&mut out, 0, 4);
    u64_at(&mut out, 4, stamp);
    for (off, value) in [(0xc, wait), (0x10, event), (0x14, stamp_self), (0x18, uuid)] {
        u32_at(&mut out, off, value);
    }
    out
}

pub(crate) fn channel(
    queue: u64,
    head: u16,
    event: u8,
    kind: u32,
    first: bool,
    tag: u64,
) -> [u8; 0x18] {
    let mut out = [0; 0x18];
    u64_at(&mut out, 0, tag);
    u64_at(&mut out, 8, queue);
    u32_at(&mut out, 0x10, kind);
    u32_at(
        &mut out,
        0x14,
        u32::from(head) | (u32::from(event) << 16) | (u32::from(first) << 24),
    );
    out
}

// Initial device-lifetime transport allocation. Addresses are constructor
// inputs to the serializers above, not part of their wire format.
pub(crate) const QUEUE: u64 = 0xffff_fc20_c000_24c0;
pub(crate) const POINTERS: u64 = 0xffff_fc20_0001_2860;
pub(crate) const RING: u64 = 0xffff_fc20_c001_2860;
pub(crate) const JOBS: u64 = 0xffff_fc20_0000_0000;
pub(crate) const CONTEXT: u64 = 0xffff_fc20_c050_8040;
pub(crate) const STAMP: u64 = 0xffff_fc20_0013_8000;
