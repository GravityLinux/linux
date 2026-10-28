// SPDX-License-Identifier: GPL-2.0-only

//! Wrapped submission-entry encoding for the common immutable preparation pass.
//! This module owns bytes only: no Session lock, live memory or producer writes.

use super::{PreparationSeed,PreparedWork};
use super::super::g17p_compute::{self as protocol,work};
use kernel::prelude::*;
const PAGE:usize=0x4000;

#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) struct WrappedSeed {
    pub(crate) context:u16,
    pub(crate) root:u64,
    pub(crate) queue:u64,
    pub(crate) queue_control:u64,
    pub(crate) uuid:u16,
    pub(crate) queue_ordinal:u32,
    pub(crate) support_ordinal:u32,
    pub(crate) support:u64,
    pub(crate) save:u64,
    pub(crate) robustness:u64,
    pub(crate) operand_state:u64,
    pub(crate) save_slot:usize,
}
pub(super) struct WrappedWork {
    pub(super) wrapper:[u8;0xc0],
    pub(super) marker:[u8;0x40],
    pub(super) scheduler:[u8;0x100],
}
impl WrappedSeed {
    pub(super) fn prepare(self,seed:PreparationSeed)->Result<KBox<PreparedWork>> {
        let l=seed.layout;let sequence=seed.ordinal;let command=sequence%256;
        let p=&seed.parameters;let event=l.grid;let stamp=sequence.wrapping_add(1).wrapping_shl(8);
        let command_save=self.save;let command_robustness=self.robustness;
        let scheduler_address=l.scheduler(command);let scheduler_slot=l.scheduler_slot(command);
        let descriptor_address=l.descriptor(sequence%240);let alias=l.descriptor_low(sequence%240);
        let status=l.status(command);let driver_address=l.dispatch(command)[0];
        let namespace=self.context.checked_sub(1).ok_or(EIO)?;
        let support_ordinal=self.support_ordinal;
        let identity=((u64::from(self.uuid))<<32)|0xc0|u64::from(self.context);
        let program = protocol::ProgramG16C {
            preempt: command_save,
            cdm: p.cdm,
            identity,
            context: self.context as u32,
            ordinal: support_ordinal & 255,
            robustness: command_robustness,
            operand_state: self.operand_state,
            usc_exec_base: Some(1 << 40),
            helper_binary: 0,
            helper_data: 0,
            helper_cfg: 0,
            // Our live-pool reservation uses context-1. This is a host policy,
            // not an inherent firmware association between bank and ASID.
            execution_gate: u64::from(self.context - 1),
            register_a599: 0x13200400020,
        }.build()
        .map_err(|_| EINVAL)?;
        let mut descriptor=KVVec::with_capacity(PAGE,GFP_KERNEL)?;
        descriptor.resize(PAGE,0,GFP_KERNEL)?;
        protocol::DescriptorG16C {
                save_area: command_save,
                cdm: p.cdm,
                identity,
                sequence: u64::from(support_ordinal),
                context: self.context as u32,
                scheduler: scheduler_address,
                low_alias: alias,
                cdm_terminator: p.end.checked_sub(4).ok_or(EINVAL)?,
                sampler_array: p.sampler,
                sampler_count: p.sampler_count,
                // Shared Neo names: dispatch = completion stamps; status = internal timestamps.
                dispatch: [driver_address, self.queue + 0x1040],
                grid: u32::from(event),
                queue_submission: stamp >> 8,
                queue_ordinal: 0,
                submission_index: sequence.checked_add(1).ok_or(EOVERFLOW)?,
                ordinal: support_ordinal,
                status: [status, status + 8],
                timestamps: p.timestamps,
                shared_control: self.support,
                support_control: 0x6c000001,
                support_flags: 6,
                support_kind: 0x45f,
                support_phase: 1,
                zero_page: l.completion_scratch(command),
            }.build(&mut descriptor,program.registers())
        .map_err(|_| EINVAL)?;

        let wrapper = work::ComputeWrapperG16C::new(&work::WrapperParameters {
            scratch_low: l.context_low(),
            scratch_high: l.context(),
            support: self.support,
            queue_control: self.queue_control,
            grid: u16::from(event),
            item: self.queue_ordinal as u16 & 255,
            ordinal: self.queue_ordinal as u16,
            context: self.context,
            support_id: namespace,
            uuid: self.uuid,
            scheduler: seed.priority as u16,
            scheduler_hint: 0,
            first: sequence == 0,
            field_52: u16::from(self.queue_ordinal == 0),
            field_5e: 2,
            field_62: u16::from(self.queue_ordinal == 0),
        })
        .map_err(|_| EINVAL)?;

        let mut end = [0u8; 0x40];
        end[..4].copy_from_slice(&16u32.to_le_bytes());
        end[4..8].copy_from_slice(&(((sequence.wrapping_add(1) & 0xffff) << 16) | u32::from(event)).to_le_bytes());
        end[8..16].copy_from_slice(
            &(0x00fe000000000000u64 | ((sequence as u64 + 1) >> 16)).to_le_bytes(),
        );

        let mut points=[(event,sequence);4];let mut point_count=1;
        for &point in &seed.dependencies[..seed.dependency_count] {
            if point.0>=128 || point.1>=1<<30 || (point.0==event && point.1>sequence) {return Err(EIO);}
            points[point_count]=point;point_count+=1;
        }
        let mut record = work::ComputeRecordG16C::new(&work::RecordParametersG16C {
            descriptor: descriptor_address,
            queue: self.queue,
            grid: event,
            item: sequence,
            primary_count: (program.registers().len() - 2) as u16,
            control: 0x0800 | namespace,
            retire_control: 2,
            ..Default::default()
        })
        .map_err(|_| EINVAL)?;
        record.append_dependencies(&points[..point_count]).map_err(|_|EINVAL)?;

        let mut context=KVVec::with_capacity(0x200,GFP_KERNEL)?;
        context.resize(0x200,0,GFP_KERNEL)?;context.copy_from_slice(record.as_bytes());
        let wrapped=WrappedWork {
            wrapper:wrapper.as_bytes().try_into().map_err(|_|EINVAL)?, marker:end,
            scheduler:work::ComputeSchedulerG16C::for_compute(scheduler_slot,support_ordinal)
                .as_bytes().try_into().map_err(|_|EINVAL)?,
        };
        Ok(KBox::new(PreparedWork {seed,descriptor,context,optional:None,wrapped:Some(wrapped)},GFP_KERNEL)?)
    }
}
