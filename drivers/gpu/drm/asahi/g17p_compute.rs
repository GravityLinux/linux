// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Field-built compute protocol objects from the current g17p_compute.py.
//! The caller supplies CDM and resource addresses; no shader or workload data
//! is embedded. Large output buffers belong to the caller, not the CPU stack.

pub(crate) const PAGE: usize = 0x4000;
pub(crate) const USC_EXEC_BASE: u64 = 0x10000000000;
pub(crate) type Register = (u32, u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    Invalid,
    Overflow,
    MissingRegister,
    UnsupportedExecBase,
}
pub(super) type Result<T = ()> = core::result::Result<T, Error>;
pub(super) fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or(Error::Overflow)
}
pub(super) fn clear(out: &mut [u8], size: usize) -> Result {
    if out.len() != size {
        return Err(Error::Invalid);
    }
    out.fill(0);
    Ok(())
}
pub(super) fn u16_at(out: &mut [u8], at: usize, value: u16) {
    out[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
pub(super) fn u32_at(out: &mut [u8], at: usize, value: u32) {
    out[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
pub(super) fn u64_at(out: &mut [u8], at: usize, value: u64) {
    out[at..at + 8].copy_from_slice(&value.to_le_bytes());
}
fn qword(out: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(out[at..at + 8].try_into().unwrap())
}

pub(crate) fn register_value(
    registers: &[Register],
    number: u32,
    occurrence: usize,
) -> Result<u64> {
    if registers.len() > 128 {
        return Err(Error::Invalid);
    }
    registers
        .iter()
        .filter(|(n, _)| *n == number)
        .nth(occurrence)
        .map(|(_, v)| *v)
        .ok_or(Error::MissingRegister)
}
fn register_array(out: &mut [u8], registers: &[Register]) -> Result {
    if registers.len() > 128 {
        return Err(Error::Invalid);
    }
    clear(out, 128 * 12)?;
    for (index, (number, value)) in registers.iter().enumerate() {
        u32_at(out, index * 12, *number);
        u64_at(out, index * 12 + 4, *value);
    }
    Ok(())
}

pub(crate) struct Program {
    pub(crate) preempt: u64,
    pub(crate) cdm: u64,
    pub(crate) identity: u64,
    pub(crate) context: u32,
    pub(crate) ordinal: u32,
    pub(crate) robustness: u64,
    pub(crate) operand_state: u64,
    pub(crate) usc_exec_base: u64,
    pub(crate) helper_binary: u64,
    pub(crate) helper_data: u64,
    pub(crate) helper_cfg: u64,
    pub(crate) execution_gate: u64,
}
impl Program {
    pub(crate) fn build(&self) -> Result<[Register; 40]> {
        if self.usc_exec_base != USC_EXEC_BASE {
            return Err(Error::UnsupportedExecBase);
        }
        // The low eight bits are a reusable hardware work tag. Preserve the
        // context above it; full ordinals live in the descriptor/scheduler.
        let context = (self.context as u64) << 8 | (self.ordinal as u64 & 0xff);
        Ok([
            (0x1a510, self.preempt),
            (0x1a420, self.cdm),
            (0x1a4d0, add(self.preempt, 0x1480)?),
            (0x1a4d8, add(self.preempt, 0x1488)?),
            (0x1a4e0, add(self.preempt, 0x1490)?),
            (0x1a4e8, add(self.preempt, 0x1498)?),
            (0x10071, self.usc_exec_base),
            (0x11841, self.helper_binary),
            (0x11849, self.helper_data),
            (0x11f81, self.helper_cfg),
            (0x1a440, 0x154024201),
            (0x1a458, 0x10c08860),
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
            (0x107a1, 0xff0000),
            (0x0a599, 0x13200400020),
            (0x0d411, 0x200000001),
            (0x1a540, self.identity),
            (0x014a9, self.identity),
            (0x0a351, self.identity),
            (0x10201, context),
            (0x10428, context),
            (0x14028, self.execution_gate),
            (0x14070, self.robustness | 1),
            (0x10229, add(self.operand_state, 0x12800)?),
            (0x140a8, add(self.operand_state, 0x13000)?),
            (0x10099, add(self.operand_state, 0x9405)?),
            (0x10091, add(self.operand_state, 0x12400)?),
            (0x0a5c1, add(self.operand_state, 5)?),
            (0x0a5c9, add(self.operand_state, 0x9000)?),
            (0x1a440, 0x154024209),
            (0x0a599, 0x6000400020),
        ])
    }
}

pub(crate) struct Descriptor {
    pub(crate) scheduler: u64,
    pub(crate) low_alias: u64,
    pub(crate) cdm_terminator: u64,
    pub(crate) sequence: u64,
    pub(crate) context: u32,
    pub(crate) grid: u32,
    pub(crate) dispatch: [u64; 2],
    pub(crate) status: [u64; 2],
    pub(crate) timestamps: [u64; 2],
    pub(crate) shared_control: u64,
    pub(crate) zero_page: u64,
    pub(crate) support_control: u32,
    pub(crate) support_flags: u32,
    pub(crate) ordinal: u32,
    pub(crate) queue_submission: u32,
    pub(crate) queue_ordinal: u32,
    pub(crate) submission_index: u32,
    pub(crate) sampler_array: u64,
    pub(crate) sampler_count: u32,
}
impl Descriptor {
    pub(crate) fn build(&self, out: &mut [u8], registers: &[Register]) -> Result {
        if out.len() != PAGE
            || registers.len() > 128
            || self.submission_index == 0
            || self.queue_submission == 0
            || self.queue_submission > 0xffffff
            || self.sampler_count == u32::MAX
            || (self.sampler_array == 0) != (self.sampler_count == 0)
            || self.sampler_array & 7 != 0
        {
            return Err(Error::Invalid);
        }
        let one = |number| register_value(registers, number, 0);
        let secondary = [
            (0x10099, one(0x0a5c1)?),
            (0x10091, one(0x0a5c9)?),
            (0x0a5c1, one(0x10099)?),
            (0x0a5c9, one(0x10091)?),
        ];
        let resource = one(0x1a510)?;
        let cdm = one(0x1a420)?;
        let identity = one(0x1a540)?;
        let control = one(0x1a440)?;
        if self.cdm_terminator < cdm {
            return Err(Error::Invalid);
        }
        let primary_address = add(self.low_alias, 0x40)?;
        let secondary_address = add(self.low_alias, 0x760)?;
        clear(out, PAGE)?;
        u32_at(out, 0, 3);
        u64_at(out, 4, self.sequence);
        u32_at(out, 0xc, self.context);
        u64_at(out, 0x10, self.scheduler);
        for (index, value) in [0x22, 0x23, 0x23, 0x24].into_iter().enumerate() {
            u16_at(out, 0x18 + index * 2, value);
        }
        register_array(&mut out[0x40..0x640], registers)?;
        register_array(&mut out[0x760..0xd60], &secondary)?;
        for (at, value) in [
            (0x740, primary_address),
            (0xe60, secondary_address),
            (0xed8, resource),
            (0xee0, self.cdm_terminator),
            (0xf08, control),
            (0xf2c, self.sampler_array),
            (0xf40, self.dispatch[0]),
            (0xf48, self.dispatch[1]),
            (0xf68, identity & 0xffffffff),
            (0xf7c, self.status[0]),
            (0xf84, self.status[1]),
            (0xf8c, self.timestamps[0]),
            (0xf94, self.timestamps[1]),
            (0xfb2, self.shared_control),
        ] {
            u64_at(out, at, value);
        }
        for (at, value) in [
            (
                0x748,
                ((registers.len() * 12) as u32) << 16 | registers.len() as u32,
            ),
            (0xe68, 0x300004),
            (0xf20, (identity >> 32) as u32),
            (0xf28, u32::MAX),
            (0xf34, self.sampler_count),
            (
                0xf38,
                if self.sampler_count == 0 {
                    0
                } else {
                    self.sampler_count + 1
                },
            ),
            (0xf50, self.queue_submission << 8),
            (0xf54, self.grid),
            (0xf58, self.queue_ordinal),
            (0xf60, self.submission_index),
            (0xf70, self.ordinal),
            (0xfba, self.support_control),
            (0xfbe, self.support_flags),
            (0xfc8, (self.ordinal & 3) << 30),
        ] {
            u32_at(out, at, value);
        }
        u16_at(out, 0xfb0, 0x1a);
        out[0xfc5] = 0x9f;
        // The unaligned pointer overwrites byte 0xfcb of the preceding word,
        // so preserve the Python builder's store order at this packed overlap.
        u64_at(out, 0xfcb, self.zero_page);
        out[0xfd3] = 1;
        Ok(())
    }
}

pub(crate) struct Optional {
    pub(crate) context_low: u64,
    pub(crate) context_high: u64,
    pub(crate) grid: u32,
    pub(crate) ordinal: u32,
    pub(crate) shared_control: u64,
    pub(crate) channel_control: u64,
    pub(crate) uuid: u32,
    pub(crate) field_46: u32,
    pub(crate) field_1e: u32,
    pub(crate) field_32: u32,
    pub(crate) field_56: u32,
    pub(crate) field_5e: u32,
    pub(crate) first: bool,
    pub(crate) item_index: u32,
}
impl Optional {
    pub(crate) fn build(&self) -> [u8; 0xc0] {
        let mut out = [0; 0xc0];
        u32_at(&mut out, 0, 0xf);
        for (at, value) in [
            (8, self.context_low),
            (0x10, self.context_high),
            (0x36, self.shared_control),
            (0x4a, self.channel_control),
        ] {
            u64_at(&mut out, at, value);
        }
        for (at, value) in [
            (0x18, self.grid),
            (0x1a, self.first as u32),
            (0x1e, self.field_1e),
            (0x22, 2),
            (0x32, self.field_32),
            (0x3e, self.ordinal),
            (0x46, self.field_46),
            (0x52, self.first as u32),
            (0x56, self.field_56),
            (0x5a, self.uuid),
            (0x5e, self.field_5e),
            (0x62, self.first as u32),
            (0x66, 1),
            (0x2a, self.item_index),
            (0x2e, self.item_index << 8),
        ] {
            u16_at(&mut out, at, value as u16);
        }
        out[0x76..0x86].fill(0xff);
        out
    }
}

pub(crate) fn event(out: &mut [u8], group: u32, grid: u32, counter_low: u32) -> Result {
    if group > 0xffffff {
        return Err(Error::Overflow);
    }
    clear(out, 0x400)?;
    for (at, value) in [
        (0, 0xe),
        (4, 0x10000 | (grid & 0xffff)),
        (8, (group << 8) | (counter_low & 0xff)),
        (0x10, 0x200),
    ] {
        u32_at(out, at, value);
    }
    Ok(())
}

pub(crate) fn points(out: &mut [u8], engine: u8, event_slot: u8, points: &[(u8, u32)]) -> Result {
    if points.is_empty() || points.len() > 32 || points.iter().any(|(queue, _)| *queue >= 128) {
        return Err(Error::Invalid);
    }
    clear(out, 0x110)?;
    u64_at(
        out,
        0,
        0xffff000000000000
            | ((engine as u64) << 40)
            | ((event_slot as u64) << 32)
            | ((1u64 << points.len()) - 1),
    );
    for (index, (queue, value)) in points.iter().enumerate() {
        u64_at(out, 8 + index * 8, ((*queue as u64) << 40) | *value as u64);
    }
    Ok(())
}
/// Extend the existing Source same-queue point without changing its
/// completion, receiver event slot, or firmware-owned context tail.
pub(crate) fn context_dependencies(out: &mut [u8], dependencies: &[(u8, u32)]) -> Result {
    if dependencies.is_empty() { return Ok(()); }
    if out.len() != 0x200 || dependencies.len() > 3 { return Err(Error::Invalid); }
    let header = qword(out, 0x20);
    let previous = qword(out, 0x28);
    let mut merged = [(0u8, 0u32); 4];
    merged[0] = ((previous >> 40) as u8, previous as u32);
    let mut count = 1;
    for &(grid, value) in dependencies {
        if grid >= 128 || value == 0 || value >= 1 << 30 { return Err(Error::Invalid); }
        if let Some(existing) = merged[..count].iter_mut().find(|point| point.0 == grid) {
            existing.1 = existing.1.max(value);
        } else {
            merged[count] = (grid, value);
            count += 1;
        }
    }
    points(&mut out[0x20..0x130], 8, (header >> 32) as u8, &merged[..count])
}

pub(crate) fn completion_header(header: u64, value: u32) -> Result<u64> {
    if value == 0 || value >= 1 << 30 {
        return Err(Error::Invalid);
    }
    Ok(header & !0xfffffffc | ((value as u64) << 2))
}
pub(crate) fn context_offset(item_index: u32, records: u32) -> Result<usize> {
    if !(2..=256).contains(&records) {
        return Err(Error::Invalid);
    }
    Ok(((item_index as u64 + 1) % records as u64 * 0x200) as usize)
}

pub(crate) struct Context<'a> {
    pub(crate) descriptor: u64,
    pub(crate) queue: u64,
    pub(crate) grid: u32,
    pub(crate) flags: u64,
    pub(crate) word_220: u64,
    pub(crate) word_330: u64,
    pub(crate) word_338: u64,
    pub(crate) word_350: u64,
    pub(crate) word_358: u64,
    pub(crate) word_378: u64,
    pub(crate) item_index: u32,
    pub(crate) points: Option<&'a [(u8, u32)]>,
    pub(crate) event_slot: Option<u8>,
    pub(crate) completion: Option<u32>,
}
impl Context<'_> {
    pub(crate) fn build(&self, out: &mut [u8]) -> Result {
        // grid*4 is shifted by 40 in a u64 field in the Python serializer.
        if self.grid >= 1 << 22 {
            return Err(Error::Overflow);
        }
        let mut header =
            self.flags | ((self.grid as u64 * 4) << 40) | ((self.item_index as u64 + 1) * 4);
        if let Some(value) = self.completion {
            header = completion_header(header, value)?;
        }
        clear(out, 0x200)?;
        for (at, value) in [
            (0, header),
            (0x10, self.descriptor),
            (0x18, self.queue),
            (0x20, self.word_220),
            (0x28, ((self.grid as u64) << 40) | self.item_index as u64),
            (0x130, self.word_330),
            (0x138, self.word_338),
            (0x150, self.word_350),
            (0x158, self.word_358),
            (0x178, self.word_378),
        ] {
            u64_at(out, at, value);
        }
        if let Some(values) = self.points {
            points(
                &mut out[0x20..0x130],
                8,
                self.event_slot.unwrap_or((self.word_220 >> 32) as u8),
                values,
            )?;
        }
        Ok(())
    }
}
/// Reuse preserves firmware-owned bytes and replaces the authored point prefix.
pub(crate) fn update_context(previous: &mut [u8], current: &[u8]) -> Result {
    if previous.len() != 0x200 || current.len() != 0x200 {
        return Err(Error::Invalid);
    }
    let mask = qword(current, 0x20) as u32;
    if mask == 0 || (mask != u32::MAX && mask & (mask + 1) != 0) {
        return Err(Error::Invalid);
    }
    let end = 0x28 + (32 - mask.leading_zeros()) as usize * 8;
    for at in [0, 0x10, 0x18, 0x20, 0x28, 0x130, 0x138, 0x150, 0x158, 0x178] {
        previous[at..at + 8].copy_from_slice(&current[at..at + 8]);
    }
    previous[0x20..end].copy_from_slice(&current[0x20..end]);
    Ok(())
}

pub(crate) struct Scheduler {
    pub(crate) slot: u64,
    pub(crate) work_id: u32,
    pub(crate) phase: u32,
    pub(crate) job_list: u64,
    pub(crate) node_id: u64,
    pub(crate) completion_kind: u64,
}
impl Scheduler {
    pub(crate) fn build(&self) -> [u8; 0x100] {
        let mut out = [0; 0x100];
        u64_at(&mut out, 0, self.slot);
        for (at, value) in [(8, self.work_id), (0xc, self.phase), (0x10, 0x50)] {
            u32_at(&mut out, at, value);
        }
        if self.phase != 0 {
            u32_at(&mut out, 0x24, 1);
        }
        u64_at(&mut out, 0xa0, self.job_list);
        if self.node_id != 0 {
            u64_at(&mut out, 0xa8, 0x2000000 | self.node_id);
            u64_at(&mut out, 0xb0, 0x2000000 | (self.node_id - 1));
            u64_at(&mut out, 0xc0, 1);
        }
        if self.completion_kind != 0 {
            u64_at(&mut out, 0xc0, self.completion_kind);
        }
        out
    }
}
pub(crate) fn scheduler_slot(first: u64, occupied: usize) -> Result<[u8; 0x40]> {
    if occupied > 8 {
        return Err(Error::Invalid);
    }
    let mut out = [0; 0x40];
    for index in 0..occupied {
        u64_at(
            &mut out,
            index * 8,
            (2 << 32) | if index == 0 { first } else { 0 },
        );
    }
    Ok(out)
}
