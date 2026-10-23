// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Field-built compute protocol objects from the current g17p_compute.py.
//! The caller supplies CDM and resource addresses; no shader or workload data
//! is embedded. Large output buffers belong to the caller, not the CPU stack.

#[path = "scheduled_compute.rs"]
#[allow(dead_code)]
pub(crate) mod wire;
#[path = "scheduled_work.rs"]
#[allow(dead_code)]
pub(crate) mod work;

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

fn wire_error(e: wire::Error) -> Error {
    match e {
        wire::Error::Invalid => Error::Invalid,
        wire::Error::Overflow => Error::Overflow,
        wire::Error::MissingRegister => Error::MissingRegister,
        wire::Error::UnsupportedExecBase => Error::UnsupportedExecBase,
    }
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
        wire::ComputeProgramG17P::new(&wire::ProgramParameters {
            save_area: self.preempt, cdm: self.cdm, identity: self.identity,
            context: self.context, ordinal: self.ordinal, robustness: self.robustness,
            operand_state: self.operand_state, usc_exec_base: Some(self.usc_exec_base),
            helper_binary: self.helper_binary, helper_data: self.helper_data,
            helper_config: self.helper_cfg, execution_gate: self.execution_gate,
            register_a599: 0x13200400020,
        }).map(wire::ComputeProgramG17P::into_array).map_err(wire_error)
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
        if out.len() != PAGE { return Err(Error::Invalid); }
        wire::ComputeDescriptorG17P::write(out, &wire::Parameters {
            save_area: 0, cdm: 0, identity: 0, // Neo scalars come from the register program.
            sequence: self.sequence,
            context: self.context,
            scheduler: self.scheduler,
            alias: self.low_alias,
            terminator: self.cdm_terminator,
            sampler_array: self.sampler_array,
            sampler_count: self.sampler_count,
            dispatch: self.dispatch,
            grid: self.grid,
            queue_submission: self.queue_submission,
            queue_ordinal: self.queue_ordinal,
            submission_index: self.submission_index,
            ordinal: self.ordinal,
            status: self.status,
            timestamps: self.timestamps,
            support: self.shared_control,
            support_control: self.support_control,
            support_flags: self.support_flags,
            support_kind: 0x9f,
            support_phase: 1,
            zero_page: self.zero_page,
        }, registers).map_err(wire_error)
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
        let raw = work::ComputeWrapperG17P::new(&work::WrapperParameters {
            scratch_low: self.context_low, scratch_high: self.context_high,
            support: self.shared_control, queue_control: self.channel_control,
            grid: self.grid as u16, item: self.item_index as u16,
            ordinal: self.ordinal as u16, context: self.field_32 as u16,
            support_id: self.field_56 as u16, uuid: self.uuid as u16,
            scheduler: self.field_1e as u16, scheduler_hint: self.field_46 as u16,
            first: self.first, field_52: self.first as u16,
            field_5e: self.field_5e as u16, field_62: self.first as u16,
        });
        let mut out = [0; 0xc0];
        out.copy_from_slice(raw.as_bytes());
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
    let mut own = ((previous >> 40) as u8, previous as u32);
    let mut count = 0;
    for &(grid, value) in dependencies {
        if grid >= 128 || value == 0 || value >= 1 << 30 { return Err(Error::Invalid); }
        if grid == own.0 { own.1 = own.1.max(value); continue; }
        if let Some(existing) = merged[..count].iter_mut().find(|point| point.0 == grid) {
            existing.1 = existing.1.max(value);
        } else {
            merged[count] = (grid, value);
            count += 1;
        }
    }
    // Native consumer-first captures place the receiver's previous-value
    // point last, after every foreign grid dependency.
    merged[count] = own;
    count += 1;
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
        let raw = work::ComputeRecordG17P::from_fields(work::RecordFields {
            header, descriptor: self.descriptor, queue: self.queue,
            dependency_header: self.word_220,
            previous: ((self.grid as u64) << 40) | self.item_index as u64,
            field_30: 0, control_130: self.word_330,
            retire_control: self.word_338, ready: 0,
            arrays: [self.word_350, self.word_358], mask: self.word_378,
        });
        out.copy_from_slice(raw.as_bytes());
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
        let raw = work::ComputeSchedulerG17P::new(work::SchedulerParameters {
            slot: self.slot, work_id: self.work_id, phase: self.phase,
            job_list: self.job_list, node_id: self.node_id,
            completion_kind: self.completion_kind,
        });
        let mut out = [0; 0x100];
        out.copy_from_slice(raw.as_bytes());
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
