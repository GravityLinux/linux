// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Compute allocator/control memory constructors. Every output is generated
//! from explicit addresses and source ABI fields, before firmware publication.

use super::g17p_compute::{add, clear, u32_at, u64_at, Error, Result, PAGE};

pub(crate) fn shared_state(out: &mut [u8], active: u32) -> Result {
    clear(out, PAGE)?;
    u32_at(out, 0, active);
    Ok(())
}
pub(crate) fn operand_table(out: &mut [u8], bases: &[u64]) -> Result {
    if bases.len() > PAGE / 0x40 {
        return Err(Error::Invalid);
    }
    clear(out, PAGE)?;
    for (index, base) in bases.iter().enumerate() {
        u64_at(out, index * 0x40, *base | 0x1000000000000000);
    }
    Ok(())
}
pub(crate) fn operand_table_contiguous(out: &mut [u8], base: u64, entries: usize) -> Result {
    if entries > PAGE / 0x40 {
        return Err(Error::Invalid);
    }
    if entries > 0 {
        add(base, (entries - 1) as u64 * 0x108000)?;
    }
    clear(out, PAGE)?;
    for index in 0..entries {
        u64_at(
            out,
            index * 0x40,
            (base + index as u64 * 0x108000) | 0x1000000000000000,
        );
    }
    Ok(())
}

pub(crate) struct PageLists {
    pub(crate) base: u64,
    pub(crate) entries: usize,
    pub(crate) buffer_size: usize,
    pub(crate) buffer_stride: u64,
    pub(crate) page_size: usize,
}
impl PageLists {
    fn dimensions(&self) -> Result<(usize, usize, usize)> {
        if self.entries == 0
            || self.buffer_size == 0
            || self.page_size == 0
            || self.buffer_size % self.page_size != 0
        {
            return Err(Error::Invalid);
        }
        let pages = self.buffer_size / self.page_size;
        let buffers = PAGE / 8 / pages;
        if buffers == 0 {
            return Err(Error::Invalid);
        }
        let size = self
            .entries
            .div_ceil(buffers)
            .checked_mul(PAGE)
            .ok_or(Error::Overflow)?;
        let last = (self.entries - 1) as u64;
        let offset = last
            .checked_mul(self.buffer_stride)
            .ok_or(Error::Overflow)?;
        add(
            add(self.base, offset)?,
            (self.buffer_size - self.page_size) as u64,
        )?;
        Ok((pages, buffers, size))
    }
    pub(crate) fn size(&self) -> Result<usize> {
        Ok(self.dimensions()?.2)
    }
    pub(crate) fn build(&self, out: &mut [u8]) -> Result {
        let (pages, buffers, size) = self.dimensions()?;
        clear(out, size)?;
        for index in 0..self.entries {
            let first = index / buffers * PAGE + index % buffers * pages * 8;
            let base = self.base + index as u64 * self.buffer_stride;
            for page in 0..pages {
                u64_at(out, first + page * 8, base + (page * self.page_size) as u64);
            }
        }
        Ok(())
    }
    pub(crate) fn build_full_page(&self, out: &mut [u8]) -> Result {
        let (pages, _, _) = self.dimensions()?;
        if self.entries.checked_mul(pages) != Some(PAGE / 8) {
            return Err(Error::Invalid);
        }
        self.build(out)
    }
}

pub(crate) struct Support {
    /// Some(class, low buffer) selects the packed class-1/2 form; otherwise
    /// word_10 is the full ordinary shared-support field.
    pub(crate) compact: Option<(u32, u64)>,
    pub(crate) header: u64,
    pub(crate) word_08: u64,
    pub(crate) word_10: u64,
    pub(crate) resource_class: u32,
    pub(crate) word_20: Option<u64>,
    pub(crate) word_28: Option<u64>,
    pub(crate) client_state: u64,
    pub(crate) firmware_state: u64,
    pub(crate) cursor: u32,
    pub(crate) field_54: u32,
    pub(crate) field_5c: u32,
    pub(crate) final_kind: u32,
}
impl Support {
    pub(crate) fn build(&self, out: &mut [u8]) -> Result {
        let resource = if self.word_20.is_some() && self.word_28.is_some() {
            0
        } else {
            (self.resource_class as u64)
                .checked_mul(1 << 40)
                .ok_or(Error::Overflow)?
        };
        if self.compact.is_some()
            && (self.header > u32::MAX as u64 || self.word_08 > u32::MAX as u64)
        {
            return Err(Error::Invalid);
        }
        clear(out, PAGE)?;
        if let Some((class, buffer)) = self.compact {
            u32_at(out, 0, self.header as u32);
            u32_at(out, 8, self.word_08 as u32);
            u32_at(out, 0x10, class);
            u32_at(out, 0x14, buffer as u32);
        } else {
            u64_at(out, 0, self.header);
            u64_at(out, 8, self.word_08);
            u64_at(out, 0x10, self.word_10);
        }
        for (at, value) in [
            (0x18, 0x4000000000070),
            (0x20, self.word_20.unwrap_or(resource)),
            (0x28, self.word_28.unwrap_or(resource)),
            (0x30, self.client_state),
            (0x40, 4),
            (0x4c, self.firmware_state),
        ] {
            u64_at(out, at, value);
        }
        for (at, value) in [
            (0x48, self.cursor),
            (0x54, self.field_54),
            (0x5c, self.field_5c),
            (0x60, self.final_kind),
        ] {
            u32_at(out, at, value);
        }
        Ok(())
    }
}

pub(crate) struct Class2Pool<'a> {
    pub(crate) low_slots: u64,
    pub(crate) high_slots: u64,
    pub(crate) shared_state: u64,
    pub(crate) records: usize,
    pub(crate) index_base: u64,
    pub(crate) active: &'a [u32],
}
impl Class2Pool<'_> {
    pub(crate) fn build(&self, out: &mut [u8]) -> Result {
        if !(1..=PAGE / 0x80).contains(&self.records) {
            return Err(Error::Invalid);
        }
        let last = (0x280 + (self.records - 1) * 4) as u64;
        add(self.low_slots, last)?;
        add(self.high_slots, last)?;
        add(self.index_base, (self.records - 1).min(35) as u64 * 0x20)?;
        let shared = add(self.shared_state, 0x40)?;
        clear(out, PAGE)?;
        for index in 0..self.records {
            let record = index * 0x80;
            let offset = (0x280 + index * 4) as u64;
            for (at, value) in [
                (0, self.low_slots + offset),
                (8, self.high_slots + offset),
                (0x28, self.index_base + (index % 36) as u64 * 0x20),
                (0x40, shared),
            ] {
                u64_at(out, record + at, value);
            }
            if index > 0 {
                if let Some(value) = self.active.get(index - 1) {
                    u64_at(out, record + 0x10, *value as u64);
                    u32_at(out, record + 0x48, *value);
                    u32_at(out, record + 0x4c, 1);
                }
            }
        }
        Ok(())
    }
}
pub(crate) fn pool_state(out: &mut [u8], limit: u32, active: u32) -> Result {
    clear(out, PAGE)?;
    u32_at(out, 0, limit);
    u32_at(out, 4, limit);
    u32_at(out, 0x40, active);
    Ok(())
}
#[derive(Clone, Copy)]
pub(crate) enum Predecessor {
    Seed,
    Active,
    Minimal,
}
pub(crate) fn predecessor(
    out: &mut [u8],
    slots: u64,
    job_list: u64,
    profile: Predecessor,
) -> Result {
    add(slots, 35 * 4)?;
    clear(out, PAGE)?;
    for index in 0..36 {
        u64_at(out, index * 0x100, slots + index as u64 * 4);
    }
    match profile {
        Predecessor::Seed => (),
        Predecessor::Active => {
            for (index, (state, high, low)) in [
                (3, 0x2000279, 0x2000278),
                (5, 0x2000304, 0x2000303),
                (7, 0x200030f, 0x200030e),
            ]
            .into_iter()
            .enumerate()
            {
                let base = (index + 1) * 0x100;
                for (at, value) in [(8, state), (0xc, 2), (0x20, 0), (0x24, 2)] {
                    u32_at(out, base + at, value);
                }
                for (at, value) in [
                    (0x10, 0x50),
                    (0xa0, job_list),
                    (0xa8, high),
                    (0xb0, low),
                    (0xc0, 1),
                ] {
                    u64_at(out, base + at, value);
                }
            }
        }
        Predecessor::Minimal => {
            for (at, value) in [(0x10c, 1), (0x124, 3)] {
                u32_at(out, at, value);
            }
            for (at, value) in [
                (0x110, 0x50),
                (0x1a0, job_list),
                (0x1b8, 0x3000220),
                (0x1c0, 2),
                (0x208, 1),
                (0x210, 0x50),
            ] {
                u64_at(out, at, value);
            }
        }
    }
    Ok(())
}
pub(crate) fn predecessor_slots(out: &mut [u8], minimal: bool) -> Result {
    clear(out, PAGE)?;
    for index in 1..=if minimal { 2 } else { 3 } {
        u32_at(out, index * 4, if minimal { 1 } else { 2 });
    }
    Ok(())
}
