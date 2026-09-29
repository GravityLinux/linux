// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Render allocator, support and queue-context objects from g17p_submission.py.
//! These serializers do not publish queues or infer resource retirement.

use super::{
    g17p_compute::{self as c, add, clear, u16_at, u32_at, u64_at, Error, Result, PAGE},
    g17p_compute_memory as cm,
    g17p_render::Kind,
};

pub(crate) const POOL_A_SIZE: usize = 35 * 0x100;
pub(crate) const POOL_B_SIZE: usize = 79 * 0x80;
pub(crate) const CONTEXT_SIZE: usize = 0x180;
pub(crate) const DEFAULT_INDEX_GROUPS: [(u32, u32); 2] = [(0x11, 6), (0x4a, 26)];
pub(crate) const CONTEXT2_INDEX_GROUPS: [(u32, u32); 2] = [(0x11, 6), (0x3c, 2)];
fn narrow(value: u64) -> Result<u32> {
    value.try_into().map_err(|_| Error::Overflow)
}

pub(crate) fn record_array_a(out: &mut [u8], slots: u64, work: u32) -> Result {
    add(slots, 34 * 4)?;
    clear(out, POOL_A_SIZE)?;
    for i in 0..35 {
        u64_at(out, i * 0x100, slots + i as u64 * 4);
    }
    u32_at(out, 8, work);
    u32_at(out, 0x10, 0x50);
    Ok(())
}
pub(crate) fn record_array_b(
    out: &mut [u8],
    slots: u64,
    shared: u64,
    pair: u32,
    work: u32,
) -> Result {
    let index_delta = pair as u64 * 0x140;
    let cycle_delta = pair as u64 * 0x5e0000;
    let work_delta = (work as u64) << 15;
    narrow(0x80004 + work_delta + index_delta + 78 * 4)?;
    narrow(0x178020 + 34 * 0x20 + work_delta + cycle_delta)?;
    add(slots, index_delta + 78 * 4)?;
    clear(out, POOL_B_SIZE)?;
    for i in 0..79 {
        let at = i * 0x80;
        let phase = i % 36;
        let cycle = if phase == 35 {
            0x178000
        } else {
            0x178020 + phase as u64 * 0x20
        };
        u32_at(
            out,
            at,
            (0x80004 + work_delta + index_delta + i as u64 * 4) as u32,
        );
        u32_at(out, at + 4, 0x10);
        u64_at(out, at + 8, slots + index_delta + i as u64 * 4);
        u32_at(out, at + 0x28, (cycle + work_delta + cycle_delta) as u32);
        u64_at(out, at + 0x40, shared);
    }
    u32_at(out, 0x4c, 1);
    Ok(())
}

pub(crate) struct Shared {
    pub(crate) pointers: [u64; 4],
    pub(crate) pair: u32,
    pub(crate) groups: u32,
    pub(crate) work: u32,
}
impl Shared {
    pub(crate) fn build(&self, out: &mut [u8]) -> Result {
        if !(1..=32).contains(&self.groups) {
            return Err(Error::Invalid);
        }
        let delta = ((self.pair as u64 * 0x5e) << 16) + ((self.work as u64) << 15);
        narrow(0x190000 + delta)?;
        clear(out, 0x88)?;
        for (at, v) in [0x20, 0x44, 0x4c, 0x64].into_iter().zip(self.pointers) {
            u64_at(out, at, v);
        }
        for (at, v) in [
            (0xc, self.pair),
            (0x28, (0x190000 + delta) as u32),
            (0x2c, 0x10),
            (0x30, 0x10000),
            (0x34, self.groups * 4),
            (0x38, 0xc18),
            (0x3c, self.groups),
            (0x54, self.groups * 4 - 1),
            (0x58, 0x20000),
            (0x7c, 0x3060),
            (0x80, 0x1020),
            (0x84, (0x180000 + delta) as u32),
        ] {
            u32_at(out, at, v);
        }
        Ok(())
    }
}
pub(crate) fn context2_shared(out: &mut [u8], pointers: [u64; 4], context: u32) -> Result {
    Shared {
        pointers,
        pair: 0,
        groups: 8,
        work: 0,
    }
    .build(out)?;
    u32_at(out, 0xc, context);
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) enum Leaf {
    PrimaryIndex,
    SecondaryIndex,
    PoolASlots,
    PoolBSlots,
    SharedSlots,
    Flag,
}
pub(crate) fn leaf(
    out: &mut [u8],
    kind: Leaf,
    pair: u32,
    ranges: &[(u32, u32)],
    shared_count: u32,
    work: u32,
) -> Result {
    let mut count = 0usize;
    for &(start, groups) in ranges {
        count = count.checked_add(groups as usize).ok_or(Error::Overflow)?;
        if count > PAGE / 16 {
            return Err(Error::Invalid);
        }
        if groups != 0 {
            narrow(start as u64 + pair as u64 * 0xbc + (groups - 1) as u64 * 5 + work as u64 + 3)?;
        }
    }
    clear(out, PAGE)?;
    match kind {
        Leaf::PrimaryIndex | Leaf::SecondaryIndex => {
            let mut index = 0;
            for &(start, groups) in ranges {
                for group in 0..groups {
                    let base = start as u64 + pair as u64 * 0xbc + group as u64 * 5 + work as u64;
                    match kind {
                        Leaf::PrimaryIndex => {
                            for member in 0..4 {
                                u32_at(out, index * 16 + member * 4, base as u32 + member as u32);
                            }
                        }
                        _ => u64_at(out, index * 8, base),
                    }
                    index += 1;
                }
            }
        }
        Leaf::PoolASlots => u32_at(out, 4, 2),
        Leaf::PoolBSlots => (),
        Leaf::SharedSlots => {
            u32_at(out, 0, shared_count);
            u32_at(out, 4, shared_count);
            u32_at(out, 0x60, 1);
        }
        Leaf::Flag => u32_at(out, 0, 1),
    }
    Ok(())
}

pub(crate) fn operand_table(out: &mut [u8], base: u64, count: usize) -> Result {
    cm::operand_table_contiguous(out, base, count)
}
pub(crate) fn operand_directory_size(count: usize) -> Result<usize> {
    count
        .checked_mul(256 * 8)
        .and_then(|v| v.checked_add(PAGE - 1))
        .map(|v| v & !(PAGE - 1))
        .ok_or(Error::Overflow)
}
pub(crate) fn operand_directory(out: &mut [u8], base: u64, count: usize) -> Result {
    if count == 0 {
        return clear(out, 0);
    }
    cm::PageLists {
        base,
        entries: count,
        buffer_size: 0x100000,
        buffer_stride: 0x108000,
        page_size: 0x1000,
    }
    .build(out)
}

pub(crate) struct Optional<'a> {
    pub(crate) kind: Kind,
    pub(crate) context_scratch: u64,
    pub(crate) firmware_scratch: u64,
    pub(crate) shared_control: u64,
    pub(crate) channel_control: u64,
    pub(crate) tiling_shared: Option<u64>,
    pub(crate) grid: u16,
    pub(crate) item: u16,
    pub(crate) ordinal: u16,
    pub(crate) context: Option<u16>,
    pub(crate) uuid: Option<u16>,
    pub(crate) scheduler_class: Option<u16>,
    pub(crate) context_index: Option<u16>,
    pub(crate) context_phase: Option<u16>,
    pub(crate) first: Option<bool>,
    pub(crate) lifecycle: Option<u16>,
    pub(crate) namespace: Option<u16>,
    pub(crate) overrides: &'a [(usize, u16)],
}
impl Optional<'_> {
    pub(crate) fn build(&self, out: &mut [u8]) -> Result {
        if (self.kind == Kind::Tiling) != self.tiling_shared.is_some()
            || self
                .overrides
                .iter()
                .any(|&(at, _)| at > 0xbe || at & 1 != 0)
            || self.kind == Kind::Tiling && self.grid == u16::MAX
        {
            return Err(Error::Invalid);
        }
        let phase = match self.context_phase {
            Some(v) => v,
            None => (self.item as u32 * 256)
                .try_into()
                .map_err(|_| Error::Overflow)?,
        };
        let context = self.context.unwrap_or(self.grid / 2);
        let scheduler = self.scheduler_class.unwrap_or(context);
        clear(out, 0xc0)?;
        u32_at(out, 0, 0xf);
        for (at, v) in [
            (8, self.context_scratch),
            (0x10, self.firmware_scratch),
            (0x36, self.shared_control),
            (0x4a, self.channel_control),
        ] {
            u64_at(out, at, v);
        }
        if let Some(shared) = self.tiling_shared {
            u64_at(out, 0x6e, shared);
        } else {
            out[0x76..0x86].fill(0xff);
            u16_at(out, 0x22, 1);
        }
        for at in [0x1a, 0x26, 0x32, 0x52, 0x5e, 0x62, 0x66] {
            u16_at(out, at, 1);
        }
        u16_at(out, 0x5a, self.uuid.unwrap_or(0xa6));
        if !self.first.unwrap_or(self.item == 0) {
            for at in [0x1a, 0x52, 0x62] {
                u16_at(out, at, 0);
            }
        }
        for (at, v) in [
            (0x2a, self.context_index.unwrap_or(self.item)),
            (0x2e, phase),
            (0x18, self.grid),
            (0x3e, self.ordinal),
            (0x32, context),
            (0x56, context),
            (0x5e, scheduler),
        ] {
            u16_at(out, at, v);
        }
        if self.scheduler_class.is_some() || context == 2 {
            u16_at(out, 0x1e, scheduler);
            u16_at(out, 0x46, scheduler);
        }
        if self.kind == Kind::Tiling {
            u16_at(out, 0x76, self.lifecycle.unwrap_or(self.ordinal));
            u16_at(out, 0x7e, self.namespace.unwrap_or(self.grid / 2));
            u16_at(out, 0x82, self.grid + 1);
        }
        for &(at, v) in self.overrides {
            u16_at(out, at, v);
        }
        Ok(())
    }
}

/// Initialize only the host's 0x40-byte event record. Adjacent TA/fragment
/// events share storage and must not be erased with a 0x400-byte write.
pub(crate) fn event(out: &mut [u8], group: u32, subtype: u32, unknown_10: u32) -> Result {
    if group > 0xffffff {
        return Err(Error::Overflow);
    }
    clear(out, 0x40)?;
    for (at, v) in [(0, 0xe), (4, subtype), (8, group << 8), (0x10, unknown_10)] {
        u32_at(out, at, v);
    }
    Ok(())
}

pub(crate) fn paired_dependencies(
    kind: Kind,
    pair: u8,
    item: u32,
) -> Result<([(u8, u32); 2], usize)> {
    if pair > 1 || item >= (1 << 30) - 1 {
        return Err(Error::Invalid);
    }
    let grid = pair * 2 + u8::from(kind == Kind::Fragment);
    Ok(if kind == Kind::Tiling {
        ([(grid, item), (0, 0)], 1)
    } else {
        ([(grid - 1, item + 1), (grid, item)], 2)
    })
}

pub(crate) struct Context<'a> {
    pub(crate) kind: Kind,
    pub(crate) descriptor: u64,
    pub(crate) queue: u64,
    pub(crate) pair: u32,
    pub(crate) item: u32,
    pub(crate) context: Option<u32>,
    pub(crate) grid: Option<u32>,
    pub(crate) locator_context: Option<u32>,
    pub(crate) partial_opening: bool,
    pub(crate) dependency_grid: Option<u32>,
    pub(crate) points: Option<&'a [(u8, u32)]>,
    pub(crate) event_slot: Option<u8>,
    pub(crate) completion: Option<u32>,
}
impl Context<'_> {
    pub(crate) fn build(&self, out: &mut [u8]) -> Result {
        let fragment = self.kind == Kind::Fragment;
        let dependency = self.dependency_grid.is_some();
        let extended =
            self.context.is_some_and(|v| v >= 2) && (self.pair >= 2 || self.grid.is_some());
        if (dependency || self.partial_opening)
            && (self.pair != 0 || self.item != 0 || self.context != Some(1))
        {
            return Err(Error::Invalid);
        }
        let mut w = [0u64; CONTEXT_SIZE / 8];
        w[0x178 / 8] = 0x003fffffffffffff;
        let mut extended_locators = false;
        let base = if fragment {
            0xfffffc20c00b0000
        } else {
            0xfffffc20c0018000
        };
        if dependency || (!self.partial_opening && extended) {
            if self.descriptor == 0 {
                return Err(Error::Invalid);
            }
            let grid = match self.dependency_grid.or(self.grid) {
                Some(v) => v,
                None => self
                    .pair
                    .checked_mul(2)
                    .and_then(|v| v.checked_add(fragment as u32))
                    .ok_or(Error::Overflow)?,
            };
            if grid >= 1 << 22 || fragment && grid == 0 {
                return Err(Error::Invalid);
            }
            let context = self.context.unwrap();
            w[0] = 0x1000000000000000 | ((grid as u64 * 0x400) << 32) | (4 + self.item as u64 * 4);
            w[0x20 / 8] = (if fragment {
                0xffff180000000003
            } else {
                0xffff0c0000000001
            }) | ((context as u64) << 32);
            w[0x28 / 8] = ((grid - u32::from(fragment)) as u64) << 40
                | if fragment {
                    (context - 1).max(1) as u64
                } else {
                    self.item as u64
                };
            // Dependency mode's first fragment waits for TA value one.
            if fragment {
                if dependency {
                    w[0x28 / 8] = ((grid - 1) as u64) << 40 | 1;
                }
                w[0x30 / 8] = (grid as u64) << 40 | self.item as u64;
            }
            extended_locators = dependency || self.locator_context.unwrap_or(context) >= 3;
        } else {
            w[0] = if fragment { 0x0400040000000004 } else { 4 };
            w[0x20 / 8] = if fragment {
                0xffff180000000003
            } else {
                0xffff0c0000000001
            };
            w[0x150 / 8] = if fragment {
                0x0002b00380004c05
            } else {
                0x0002380380000003
            };
            if fragment {
                w[1] = 0x004000e000130d40;
                w[0x28 / 8] = 1;
                w[0x30 / 8] = 0x10000000000;
                w[0x158 / 8] = 0x0000100380004c3e;
                w[0x160 / 8] = 0x0000100380004c77;
                w[0x168 / 8] = 0x0000100380004cb0;
            }
            if self.partial_opening {
                w[0] = 0x1000000000000004 | if fragment { 0x400 << 32 } else { 0 };
                w[1] = 0;
                extended_locators = true;
            } else {
                if self.pair > 1 {
                    return Err(Error::Invalid);
                }
                if self.pair == 1 {
                    w[0] = if fragment {
                        0x04000c0000000004
                    } else {
                        0x0000080000000004
                    };
                    w[0x20 / 8] = if fragment {
                        0xffff180100000003
                    } else {
                        0xffff0c0100000001
                    };
                    w[0x28 / 8] = 0x0000020000000000 | u64::from(fragment);
                    w[0x150 / 8] = if fragment {
                        0x0002b00380004d17
                    } else {
                        0x0002380380000051
                    };
                    if fragment {
                        w[1] = 0x004000e0001351c0;
                        w[0x30 / 8] = 0x0000030000000000;
                        w[0x158 / 8] = 0x0000100380004d50;
                        w[0x160 / 8] = 0x0000100380004d89;
                        w[0x168 / 8] = 0x0000100380004dc2;
                    }
                }
                w[0] = add(w[0], self.item as u64 * 4)?;
                w[0x28 / 8] = add(w[0x28 / 8], self.item as u64)?;
                w[0x150 / 8] = add(
                    w[0x150 / 8],
                    self.item as u64 * if fragment { 0x224 } else { 0x9c },
                )?;
                if fragment {
                    w[1] = add(w[1], self.item as u64 * 0x8900)?;
                    w[0x30 / 8] = add(w[0x30 / 8], self.item as u64)?;
                    for at in [0x158, 0x160, 0x168] {
                        w[at / 8] = add(w[at / 8], self.item as u64 * 0x224)?;
                    }
                }
            }
        }
        if self.descriptor != 0 || dependency || self.partial_opening || extended {
            let delta = self.descriptor.checked_sub(base).ok_or(Error::Invalid)?;
            if delta % 0x20 != 0 {
                return Err(Error::Invalid);
            }
            w[0x150 / 8] = add(
                if fragment {
                    0x0002b00380004c05
                } else {
                    0x0002380380000003
                },
                delta / 0x20,
            )?;
            if fragment {
                let locators = if extended_locators {
                    [0x0000800380004c3e, 0x0000b80380004c77, 0x0000500380004cb0]
                } else {
                    [0x0000100380004c3e, 0x0000100380004c77, 0x0000100380004cb0]
                };
                for (at, locator) in [0x158, 0x160, 0x168].into_iter().zip(locators) {
                    w[at / 8] = add(locator, delta / 0x20)?;
                }
                if !dependency && !self.partial_opening && !extended {
                    w[1] = add(
                        if self.item == 0 {
                            0x004000e000130d40
                        } else {
                            0x000000e000130d40
                        },
                        delta.checked_mul(2).ok_or(Error::Overflow)?,
                    )?;
                }
            }
        }
        w[0x10 / 8] = self.descriptor;
        w[0x18 / 8] = self.queue;
        if let Some(completion) = self.completion {
            w[0] = c::completion_header(w[0], completion)?;
        }
        let mut points = [0; 0x110];
        if let Some(values) = self.points {
            c::points(
                &mut points,
                if fragment { 24 } else { 12 },
                self.event_slot.unwrap_or((w[0x20 / 8] >> 32) as u8),
                values,
            )?;
        }
        clear(out, CONTEXT_SIZE)?;
        for (i, v) in w.into_iter().enumerate() {
            u64_at(out, i * 8, v);
        }
        if self.points.is_some() {
            out[0x20..0x130].copy_from_slice(&points);
        }
        Ok(())
    }
    pub(crate) fn page(&self, out: &mut [u8]) -> Result {
        if out.len() < 0x200 + CONTEXT_SIZE {
            return Err(Error::Invalid);
        }
        let mut record = [0; CONTEXT_SIZE];
        self.build(&mut record)?;
        out.fill(0);
        out[0x200..0x200 + CONTEXT_SIZE].copy_from_slice(&record);
        Ok(())
    }
}

/// Replace only the source's host-owned fields after a proven retirement.
/// The caller, not this byte builder, is responsible for that proof.
pub(crate) fn update_context(kind: Kind, previous: &mut [u8], current: &[u8]) -> Result {
    if previous.len() != CONTEXT_SIZE || current.len() != CONTEXT_SIZE {
        return Err(Error::Invalid);
    }
    let mask = u32::from_le_bytes(current[0x20..0x24].try_into().unwrap());
    if mask == 0 || (mask != u32::MAX && mask & (mask + 1) != 0) {
        return Err(Error::Invalid);
    }
    let end = 0x28 + (32 - mask.leading_zeros()) as usize * 8;
    for at in [0, 0x10, 0x18, 0x20, 0x28, 0x150, 0x178] {
        previous[at..at + 8].copy_from_slice(&current[at..at + 8]);
    }
    if kind == Kind::Fragment {
        for at in [8, 0x30, 0x158, 0x160, 0x168] {
            previous[at..at + 8].copy_from_slice(&current[at..at + 8]);
        }
    }
    previous[0x20..end].copy_from_slice(&current[0x20..end]);
    Ok(())
}
