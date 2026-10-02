// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Direct port of g17p_resource_record.py. These functions encode supplied
//! owner state; they do not choose slots, publish work or reclaim resources.

use super::{
    g17p_compute::{add, clear, u32_at, u64_at, Error, Result},
    g17p_render::Kind,
};
const PAGE: usize = 0x4000;
const BASE: u64 = 0x1000000000;

pub(crate) fn build_primary_index_address(index_dva: u64) -> Result<[u8; 4]> {
    if !(BASE..2 * BASE).contains(&index_dva) || index_dva & 15 != 0 {
        return Err(Error::Invalid);
    }
    Ok((((index_dva - BASE) >> 4) as u32).to_le_bytes())
}
pub(crate) fn build_shared_slot_head(index_head: u32) -> [u8; 4] {
    index_head.to_le_bytes()
}
pub(crate) fn build_primary_index_registration(index_dva: u64, count: u32) -> Result<[u8; 8]> {
    if count == 0 {
        return Err(Error::Invalid);
    }
    let mut out = [0; 8];
    out[..4].copy_from_slice(&build_primary_index_address(index_dva)?);
    out[4..].copy_from_slice(&count.to_le_bytes());
    Ok(out)
}
pub(crate) fn build_primary_index_page(out: &mut [u8], groups: &[u32]) -> Result {
    if groups.is_empty() || groups.len() > 0x400 {
        return Err(Error::Invalid);
    }
    for (i, &base) in groups.iter().enumerate() {
        base.checked_add(3).ok_or(Error::Overflow)?;
        if groups[..i].iter().any(|&other| other.abs_diff(base) < 4) {
            return Err(Error::Invalid);
        }
    }
    clear(out, PAGE)?;
    for (i, &base) in groups.iter().enumerate() {
        for member in 0..4 {
            u32_at(out, i * 16 + member * 4, base + member as u32);
        }
    }
    Ok(())
}

pub(crate) struct PartialIndexSharedObject {
    pub(crate) index_high: u64,
    pub(crate) index_low: u64,
    pub(crate) pool_b_slots: u64,
    pub(crate) shared_slots: u64,
    pub(crate) flag: u64,
    pub(crate) owner: u32,
    pub(crate) head: u32,
    pub(crate) tail: u32,
    pub(crate) group_count: u32,
    pub(crate) opaque_84: u32,
}
impl PartialIndexSharedObject {
    pub(crate) fn build(&self, out: &mut [u8]) -> Result {
        let pointers = [
            self.index_high,
            self.index_low,
            self.pool_b_slots,
            self.shared_slots,
            self.flag,
        ];
        if self.group_count != 8
            || self.owner >= 64
            || self.head.checked_sub(self.tail) != Some(self.group_count)
            || pointers.iter().any(|&p| p == 0 || p & 0x3fff != 0)
        {
            return Err(Error::Invalid);
        }
        build_primary_index_address(self.index_low)?;
        clear(out, PAGE)?;
        for (offset, pointer) in [0x20, 0x28, 0x44, 0x4c, 0x64].into_iter().zip(pointers) {
            u64_at(out, offset, pointer);
        }
        for (offset, value) in [
            (0x0c, self.owner),
            (0x30, 0x10000),
            (0x34, self.group_count * 4),
            (0x38, 0xc18),
            (0x3c, self.head),
            (0x40, self.tail),
            (0x54, self.group_count * 4 - 1),
            (0x58, 0x20000),
            (0x7c, 0x3060),
            (0x80, 0x1020),
            (0x84, self.opaque_84),
        ] {
            u32_at(out, offset, value);
        }
        Ok(())
    }
}
pub(crate) fn build_partial_pool_b(
    out: &mut [u8],
    slot_base: u64,
    shared_slot: u64,
    index_base: u32,
    cycle_base: u32,
    ready_record: usize,
) -> Result {
    if slot_base == 0
        || shared_slot == 0
        || (slot_base | shared_slot) & 3 != 0
        || ready_record >= 80
    {
        return Err(Error::Invalid);
    }
    index_base.checked_add(79 * 4).ok_or(Error::Overflow)?;
    cycle_base.checked_add(35 * 0x20).ok_or(Error::Overflow)?;
    add(slot_base, 79 * 4)?;
    clear(out, PAGE)?;
    for i in 0..80 {
        let offset = i * 0x80;
        u32_at(out, offset, index_base + i as u32 * 4);
        u32_at(out, offset + 4, 0x10);
        u64_at(out, offset + 8, slot_base + i as u64 * 4);
        u32_at(out, offset + 0x28, cycle_base + (i % 36) as u32 * 0x20);
        u64_at(out, offset + 0x40, shared_slot);
    }
    u32_at(out, ready_record * 0x80 + 0x4c, 1);
    Ok(())
}
pub(crate) fn build_partial_pool_a(
    out: &mut [u8],
    slot_base: u64,
    ready_record: usize,
    node_id: u32,
) -> Result {
    if slot_base == 0 || slot_base & 3 != 0 || ready_record >= 36 {
        return Err(Error::Invalid);
    }
    add(slot_base, 35 * 4)?;
    clear(out, PAGE)?;
    for i in 0..36 {
        u64_at(out, i * 0x100, slot_base + i as u64 * 4);
    }
    u32_at(out, ready_record * 0x100 + 8, node_id);
    u32_at(out, ready_record * 0x100 + 0x10, 0x50);
    Ok(())
}
pub(crate) fn build_pool_a_ready_fields(
    record: u64,
    slot: u64,
    node_id: u32,
) -> Result<[(u64, [u8; 4]); 3]> {
    if record == 0 || slot == 0 || (record | slot) & 3 != 0 {
        return Err(Error::Invalid);
    }
    Ok([
        (add(record, 8)?, node_id.to_le_bytes()),
        (add(record, 0x10)?, 0x50u32.to_le_bytes()),
        (slot, 2u32.to_le_bytes()),
    ])
}
pub(crate) fn build_event_notification(counter: u32, grid: u16, fragment: bool) -> [u8; 0x40] {
    let mut out = [0; 0x40];
    u32_at(&mut out, 0, 0xe);
    u32_at(&mut out, 4, 0x10000 | grid as u32);
    u32_at(&mut out, 8, counter);
    u32_at(&mut out, 0x10, if fragment { 0x100 } else { 0 });
    out
}
pub(crate) fn build_partial_queue_context_item(
    out: &mut [u8],
    kind: Kind,
    descriptor: u64,
    queue: u64,
    grid: u32,
    item_index: u32,
) -> Result {
    let fragment = kind == Kind::Fragment;
    if grid >= 128 || grid % 2 != u32::from(fragment) || item_index >= 0x3fffffff || queue == 0 {
        return Err(Error::Invalid);
    }
    let base = if fragment {
        0xfffffc20c00b0000
    } else {
        0xfffffc20c0018000
    };
    let delta = descriptor.checked_sub(base).ok_or(Error::Invalid)?;
    if delta % 0x20 != 0 {
        return Err(Error::Invalid);
    }
    clear(out, 0x180)?;
    for (offset, value) in [
        (
            0,
            0x1000000000000000 | ((grid as u64) << 42) | (4 * (item_index as u64 + 1)),
        ),
        (0x10, descriptor),
        (0x18, queue),
        (
            0x20,
            (if fragment {
                0xffff180000000003
            } else {
                0xffff0c0000000001
            }) | ((grid as u64 / 2) << 32),
        ),
        (
            0x28,
            ((grid as u64 - u64::from(fragment)) << 40) | (item_index as u64 + u64::from(fragment)),
        ),
        (0x178, 0x003fffffffffffff),
    ] {
        u64_at(out, offset, value);
    }
    let locators: &[(usize, u64)] = if fragment {
        u64_at(out, 0x30, ((grid as u64) << 40) | item_index as u64);
        &[
            (0x150, 0x0002b00380004c05),
            (0x158, 0x0000800380004c3e),
            (0x160, 0x0000b80380004c77),
            (0x168, 0x0000500380004cb0),
        ]
    } else {
        &[(0x150, 0x0002380380000003)]
    };
    for &(offset, locator) in locators {
        u64_at(out, offset, add(locator, delta / 0x20)?);
    }
    Ok(())
}
