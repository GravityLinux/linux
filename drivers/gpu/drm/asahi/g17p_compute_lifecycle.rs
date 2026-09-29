// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Retained direct-queue profile from stage_next_workload with fresh command
//! objects, persistent_startup_queue and fast_sequential. Storage admission,
//! ownership and publication are deliberately left to the runtime.
use super::g17p_compute::{self as c, Error, Result};

pub(crate) const DESCRIPTOR_BASE: u64 = 0xfffffc20c0358000;
pub(crate) const DESCRIPTOR_LOW: u64 = 0x7000340000;
pub(crate) const CONTEXT_HIGH: u64 = 0xfffffc2000278000;
pub(crate) const SUPPORT: u64 = 0xfffffc20c08d0000;
pub(crate) const SUPPORT_STATE: u64 = 0xfffffc2001688000;
pub(crate) const ZERO: u64 = 0xfffffc2001698000;

/// Cold source startup uses the compact 36-register program. The four UAPI
/// USC/helper registers belong to the subsequent caller submission profile.
pub(crate) fn opening_program(preempt: u64, cdm: u64) -> Result<[c::Register; 36]> {
    let program = c::Program {
        preempt,
        cdm,
        identity: 0x010001d7020001dc,
        context: 2,
        ordinal: 0,
        robustness: 0x1000018000,
        operand_state: 0x7000220000,
        usc_exec_base: c::USC_EXEC_BASE,
        helper_binary: 0,
        helper_data: 0,
        helper_cfg: 0,
        execution_gate: 1,
    }
    .build()?;
    Ok(core::array::from_fn(|index| {
        program[if index < 6 { index } else { index + 4 }]
    }))
}

pub(crate) struct Retained {
    pub(crate) ordinal: u32,
    pub(crate) descriptor: u64,
    pub(crate) descriptor_low: u64,
    pub(crate) optional: u64,
    pub(crate) event: u64,
    pub(crate) context_record: u64,
    pub(crate) scheduler: u64,
    pub(crate) scheduler_slot: u64,
    pub(crate) dispatch: [u64; 2],
    pub(crate) status: [u64; 2],
    pub(crate) identity: u64,
}
impl Retained {
    pub(crate) fn new(ordinal: u32) -> Result<Self> {
        // The descriptor's submission count is a 24-bit field.
        if ordinal == 0 || ordinal >= 0xffffff {
            return Err(Error::Invalid);
        }
        let n = ordinal as u64;
        let descriptor_step = (n % 240) * 0x1040;
        let (scheduler, scheduler_slot) = match ordinal {
            1 => (0xfffffc20c0870200, 0xfffffc2001638008),
            2 => (0xfffffc20c0870300, 0xfffffc200163800c),
            3 => (0xfffffc20c08c8400, 0xfffffc2001680010),
            _ => (
                0xfffffc20c0b00000 + ((n + 1) % 36) * 0x100,
                0xfffffc2001800000 + ((n + 1) % 36) * 4,
            ),
        };
        let identity = match ordinal {
            1 => 0x0200021c03000245,
            2 => 0x020002500300029c,
            3 => 0x0200020803000247,
            _ => ((0x02000208 + 2 * (n - 3)) << 32) | (0x03000247 + n - 3),
        };
        Ok(Self {
            ordinal,
            descriptor: DESCRIPTOR_BASE + descriptor_step,
            descriptor_low: DESCRIPTOR_LOW + descriptor_step,
            optional: 0xfffffc20c0605e80 + n * 0xc0,
            event: 0xfffffc20c05e9600 + n * 0x40,
            context_record: CONTEXT_HIGH + c::context_offset(ordinal, 256)? as u64,
            scheduler,
            scheduler_slot,
            dispatch: [0xfffffc20001c8008 + n * 8, 0xfffffc20c07c0008 + n * 8],
            status: [0xfffffc2000024c78, 0xfffffc2000024c80],
            identity,
        })
    }
    pub(crate) fn program(
        &self,
        preempt: u64,
        cdm: u64,
        client_slot: u32,
    ) -> Result<[c::Register; 40]> {
        let operand_slot = client_slot.min(1) as u64;
        c::Program {
            preempt: c::add(preempt, client_slot as u64 * 0x78000)?,
            cdm,
            identity: self.identity,
            context: 3,
            ordinal: self.ordinal,
            robustness: 0x1000018000 + operand_slot * 0x8000,
            operand_state: 0x7000220000 + operand_slot * 0x15c0000,
            usc_exec_base: c::USC_EXEC_BASE,
            helper_binary: 0,
            helper_data: 0,
            helper_cfg: 0,
            execution_gate: u64::from(self.ordinal == 3),
        }
        .build()
    }
    pub(crate) fn scheduler_body(&self) -> [u8; 0x100] {
        c::Scheduler {
            slot: self.scheduler_slot,
            work_id: self.ordinal,
            phase: 0,
            job_list: 0,
            node_id: 0,
            completion_kind: 0,
        }
        .build()
    }
    pub(crate) fn descriptor_body(
        &self,
        out: &mut [u8],
        registers: &[c::Register],
        end: u64,
        sampler: u64,
        sampler_count: u32,
    ) -> Result {
        c::Descriptor {
            scheduler: self.scheduler,
            low_alias: self.descriptor_low,
            cdm_terminator: end.checked_sub(4).ok_or(Error::Invalid)?,
            sequence: self.ordinal as u64,
            context: 3,
            grid: 4,
            dispatch: self.dispatch,
            status: self.status,
            timestamps: [0; 2],
            shared_control: SUPPORT,
            zero_page: ZERO,
            support_control: 0xe0a00001,
            support_flags: 0,
            ordinal: self.ordinal,
            queue_submission: self.ordinal + 1,
            queue_ordinal: 0,
            submission_index: self.ordinal + 1,
            sampler_array: sampler,
            sampler_count,
        }
        .build(out, registers)
    }
    pub(crate) fn optional_body(&self) -> [u8; 0xc0] {
        c::Optional {
            context_low: 0x70004d8000,
            context_high: CONTEXT_HIGH,
            grid: 4,
            ordinal: 0x29 + self.ordinal,
            shared_control: SUPPORT,
            channel_control: 0xfffffc20c07b8040,
            uuid: 0x159,
            field_46: 0,
            field_1e: 2,
            field_32: 3,
            field_56: 2,
            field_5e: 2,
            first: true,
            item_index: 0,
        }
        .build()
    }
    pub(crate) fn context_body(&self, out: &mut [u8]) -> Result {
        let step = (self.descriptor - DESCRIPTOR_BASE) / 0x20;
        c::Context {
            descriptor: self.descriptor,
            queue: 0xfffffc20c0000300,
            grid: 4,
            flags: 0x1000000000000000,
            word_220: 0xffff080200000001,
            word_330: 0,
            word_338: 8,
            word_350: 0x000110038001a002 + step,
            word_358: 0x000020038001a03b + step,
            word_378: 0x003fffffffffffff,
            item_index: self.ordinal,
            points: None,
            event_slot: None,
            completion: None,
        }
        .build(out)
    }
}

pub(crate) fn channel_control() -> [u8; 0x40] {
    let mut out = [0; 0x40];
    for (offset, value) in [
        (0, 0x00c8010402040202),
        (8, 0x2ee00000),
        (0x10, 0x100000),
        (0x20, 0x0002000000000000),
        (0x28, 0xa000000000000000),
        (0x30, 0x02000001),
    ] {
        c::u64_at(&mut out, offset, value);
    }
    out
}
