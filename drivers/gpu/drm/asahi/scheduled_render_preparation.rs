// SPDX-License-Identifier: GPL-2.0-only

//! Wrapped render entry encoding for the common immutable preparation pass.
//! No live reads, allocations of GPU backing, or producer publication occur here.

use super::{Deferred,Parameters};
use super::super::{g17p_render::{program,wire as body},g17p_compute::work};
use kernel::prelude::*;
const PAGE:usize=0x4000;
const STORAGE_BASE:u64=0xfffffc20d1040000;
const STORAGE_STRIDE:u64=0x20000;
pub(crate) fn storage_base(slot:u32)->Result<u64> {
    if slot>=super::super::g17p_render_lifecycle::STORAGE_SUBMISSIONS {return Err(EINVAL);}
    Ok(STORAGE_BASE+u64::from(slot)*STORAGE_STRIDE)
}

#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) struct Lane {
    pub(crate) queue:u64,
    pub(crate) scratch_low:u64,
    pub(crate) scratch_high:u64,
    pub(crate) priority:u8,
}
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) struct WrappedSeed {
    pub(crate) base:u64,
    pub(crate) storage:u32,
    pub(crate) ordinal:u32,
    pub(crate) local:u32,
    pub(crate) sequence:u32,
    pub(crate) milestone:u32,
    pub(crate) grids:[u8;2],
    pub(crate) context:u16,
    pub(crate) slot:u32,
    /// Queue completion namespace, separate from the PB manager slot below.
    pub(crate) event_slot:u8,
    pub(crate) support:u64,
    pub(crate) manager:u64,
    pub(crate) scene:u64,
    pub(crate) empty:u64,
    pub(crate) layermeta:u64,
    pub(crate) queue_control:u64,
    pub(crate) uuid:u16,
    pub(crate) lanes:[Lane;2],
}
fn push(out:&mut KVec<Deferred>,address:u64,bytes:&[u8])->Result {
    let mut body=KVVec::with_capacity(bytes.len(),GFP_KERNEL)?;body.resize(bytes.len(),0,GFP_KERNEL)?;
    body.copy_from_slice(bytes);out.push(Deferred{address,body,previous:None},GFP_KERNEL)?;Ok(())
}
impl WrappedSeed {
    /// Patch only the global publication identity after another independent
    /// queue publishes. Programs, pool-local milestones and backing stay
    /// unchanged. The caller has revalidated every other seed field.
    pub(super) fn rebase_ordinal(self, objects: &mut [KVec<Deferred>; 2], ordinal: u32) -> Result {
        if ordinal >= 0x3fffffff { return Err(EOVERFLOW); }
        // Validate all affected CPU images before editing any of them.
        for stage in 0..2 {
            let descriptor = self.base + if stage == 0 {0x2940} else {0x9b40};
            let row = objects[stage].iter().find(|row|row.address == descriptor).ok_or(EINVAL)?;
            let size = if stage == 0 {body::TilingDescriptorG16C::SIZE} else {body::FragmentDescriptorG16C::SIZE};
            if row.body.len() != size {return Err(EINVAL);}
        }
        let wrapper = objects[0].iter().find(|row|row.address == self.base + 0x4000).ok_or(EINVAL)?;
        if wrapper.body.len() != core::mem::size_of::<work::WorkWrapperG16C>() {return Err(EINVAL);}
        for stage in 0..2 {
            let address = self.base + if stage == 0 {0x2940} else {0x9b40};
            let row = objects[stage].iter_mut().find(|row|row.address == address).ok_or(EINVAL)?;
            let uuid = ((0x010000e1 + ordinal * 2 - stage as u32) & 0x00ffffff)
                | u32::from(self.context) << 24;
            // SAFETY: Lengths above match the generated alignment-one wire
            // types containing only byte arrays. These are private CPU images.
            if stage == 0 {
                let raw = unsafe {&mut *row.body.as_mut_ptr().cast::<body::TilingDescriptorG16C>()};
                raw.uuid = uuid.to_le_bytes();
                raw.work_index = (if ordinal == self.local {self.local}
                    else {self.local + self.local / 2}).to_le_bytes();
            } else {
                let raw = unsafe {&mut *row.body.as_mut_ptr().cast::<body::FragmentDescriptorG16C>()};
                raw.uuid = uuid.to_le_bytes();
            }
        }
        let row = objects[0].iter_mut().find(|row|row.address == self.base + 0x4000).ok_or(EINVAL)?;
        // SAFETY: Validated size and alignment-one byte-array staging type.
        let raw = unsafe {&mut *row.body.as_mut_ptr().cast::<work::WorkWrapperG16C>()};
        raw.lifecycle = (ordinal as u16).to_le_bytes();
        Ok(())
    }
    pub(super) fn build(self,p:&Parameters)->Result<[KVec<Deferred>;2]> {
        let mut objects=[KVec::new(),KVec::new()];
        let n=self.ordinal;let local=self.local;let item=self.milestone;let first_event=self.grids[0];
        if self.grids[1]!=first_event.checked_add(1).ok_or(EINVAL)? {return Err(EINVAL);}
        let context=u64::from(self.context);let slot=self.slot;let support=self.support;
        if slot>=super::super::g17p_render_lifecycle::POOL_SLOTS
            || u32::from(self.event_slot)>=super::super::g17p_render_lifecycle::POOL_SLOTS {return Err(EINVAL);}
        let manager=self.manager;let scene=self.scene;let empty=self.empty;
        let scene_slot=self.sequence%79+1;
        let tpc_size=p.scratch_layout().map_err(|_|EINVAL)?.2.checked_mul(4).ok_or(EOVERFLOW)?;
        let stamp=local.wrapping_add(1).wrapping_shl(8);
        let addresses=[p.ta_user_timestamp_start,p.ta_user_timestamp_end,p.fragment_user_timestamp_start,p.fragment_user_timestamp_end];
        let ps = program::ProgramStateG16C {
            layermeta: self.layermeta,
            cycle: 0x420000 + u64::from(scene_slot % 36) * 0x70,
            scene_slot: u64::from(scene_slot),
            tiling_admission: u64::from(self.slot),
            fragment_admission: u64::from(self.slot),
            tiling_gate: 0x7f00400020,
            fragment_gate: 0xa000400020,
            // The first-context template's identity byte is the UAT context,
            // independently of the pool, queue milestone and CPU fence ID.
            tiling_lifecycle: ((0xbc010000e1 + u64::from(self.sequence) * 2) & !(0xff << 24)) | context << 24,
            fragment_lifecycle: ((0xbc010000e0 + u64::from(self.sequence) * 2) & !(0xff << 24)) | context << 24,
            tiling_work_stamp: context << 8 | u64::from(self.sequence & 255),
            fragment_work_stamp: context << 8 | u64::from(self.sequence & 255),
            stencil_clear_flags: 0x100,
            partial_load_depth_flags: Some(p.depth_flags | ((p.depth_flags & (3 << 18)) >> 4)),
            partial_resume_depth_flags: Some(p.depth_flags | ((p.depth_flags & (3 << 18)) >> 4)),
            ..Default::default()
        };
        let parameters = program::descriptor_parameters(&p, &ps, tpc_size).map_err(|_| EINVAL)?;

        let mut descriptor=KVVec::with_capacity(body::FragmentDescriptorG16C::SIZE,GFP_KERNEL)?;
        descriptor.resize(body::FragmentDescriptorG16C::SIZE,0,GFP_KERNEL)?;
        for stage in 0..2usize {
            let fragment = stage == 1;
            let lane = &self.lanes[stage];
            let address = self.base + if fragment { 0x8000 + 0x1b40 } else { 0x2940 };
            let prelude = self.base + if fragment { 0xc000 } else { 0x4000 };
            let state = body::State {
                context: context as u32,
                pool: slot,
                event_control: self.base + 0x200,
                manager,
                scene,
                empty_scene: empty,
                alias: 0x7000000000 + address - work::DESCRIPTOR_BASE_G16C,
                shared_tail: self.base + 0x10000 + stage as u64 * PAGE as u64 - u64::from(item) * 0x40,
                driver_stamp: self.base + 0x18000 + stage as u64 * 0x40,
                firmware_stamp: lane.queue + 0x1040,
                support,
                counter: if n != local {
                    u64::from(local) * 2 + 1 - stage as u64
                } else {
                    u64::from(n) * 2 + 1 - stage as u64
                },
                stamp,
                event: u32::from(first_event) + stage as u32,
                uuid: ((0x010000e1 + n * 2 - stage as u32) & 0x00ffffff) | (context as u32) << 24,
                timestamp: self.base + 0x18100,
                timestamp_end: if fragment { None } else { Some(0) },
                user_timestamp_start: addresses[stage * 2],
                user_timestamp_end: addresses[stage * 2 + 1],
                fragment_event: u32::from(first_event) + 1,
                fragment_stamp: stamp,
                stage_flag: 0,
                field_93c: 0x305800001,
                field_2128: 0,
                field_21d8: 0x3be00,
                field_222c: 0,
                // Native submission 65536 still carries secondary stamp 0x100.
                // Keep its 16-bit rotation when other queues have advanced
                // the global ordinal independently of this queue.
                secondary_stamp: Some(stamp & 65535),
                descriptor_ordinal: Some(if fragment { local } else { 0 }),
                work_index: Some(if fragment {
                    (local + 1) & 255
                } else if n != local {
                    local + local / 2
                } else {
                    n
                }),
                tail_flags: 1,
                tiling_submission_tag: ((item + 1) & 255) * 0x101,
                tiling_queue_ordinal: item,
            };
            let primary_count;
            if fragment {
                let primary = program::FragmentProgramG16C::new(&p, &ps).map_err(|_| EINVAL)?;
                let store = program::partial_store(&p, &ps).map_err(|_| EINVAL)?;
                let resume = program::partial_resume(&p, &ps).map_err(|_| EINVAL)?;
                let load = program::partial_load(&p, &ps).map_err(|_| EINVAL)?;
                body::FragmentDescriptorG16C::write(
                    &mut descriptor,
                    &parameters,
                    &state,
                    &body::Programs {
                        primary: primary.registers(),
                        partial_store: &store,
                        partial_resume: &resume,
                        partial_load: &load,
                    },
                    item,
                    u32::from(first_event) + 1,
                )
                .map_err(|_| EINVAL)?;
                primary_count = primary.registers().len() as u16 - 4;
                push(&mut objects[stage],address,&descriptor)?;
            } else {
                let primary = program::TilingProgramG16C::new(&p, &ps).map_err(|_| EINVAL)?;
                body::TilingDescriptorG16C::write(
                    &mut descriptor[..body::TilingDescriptorG16C::SIZE],
                    &parameters,
                    &state,
                    primary.registers(),
                    item,
                    u32::from(first_event),
                )
                .map_err(|_| EINVAL)?;
                primary_count = primary.registers().len() as u16 - 2;
                push(&mut objects[stage],address,&descriptor[..body::TilingDescriptorG16C::SIZE])?;
            }
            let arrays = if fragment {
                [
                    (0xa0, primary_count),
                    (0x7c0, 16),
                    (0xee0, 23),
                    (0x1600, 10),
                ]
            } else {
                [(0x60, primary_count), (0, 0), (0, 0), (0, 0)]
            };
            let mut record = work::WorkRecordG16C::for_render(&work::RenderRecordParametersG16C {
                descriptor: address,
                queue: lane.queue,
                grid: first_event + stage as u8,
                item,
                fragment,
                timeout: 0xfa0,
                ready: true,
                arrays: if fragment { &arrays } else { &arrays[..1] },
                // Same physical event slot passed to Neo's points builder.
                // Native Max pool1 records carry 0xc01/0x1801 even with
                // fresh queue-local item0 and application ASID2.
                control: Some((if fragment { 0x1800 } else { 0xc00 }) | u16::from(self.event_slot)),
                control_low: None,
                field_30: None,
                field_38: 0,
            })
            .map_err(|_| EINVAL)?;
            let kind=if fragment {super::super::g17p_render::Kind::Fragment}
                else {super::super::g17p_render::Kind::Tiling};
            let external=[
                if !fragment {p.prior_queue_ta}else{None},
                if !fragment {p.cdm_dependency}else{None},
                if fragment==p.vdm_barrier_fragment {p.vdm_dependency}else{None},
            ];
            let (points,count)=super::super::g17p_render_graph::render_dependencies(
                kind,[u32::from(first_event),u32::from(first_event)+1],item,external).map_err(|_|EINVAL)?;
            record.set_dependencies(&points[..count]).map_err(|_|EINVAL)?;
            push(&mut objects[stage],lane.scratch_high+u64::from((item+1)&255)*0x200,record.as_bytes())?;
            let wrapper = work::WorkWrapperG16C::for_render(&work::RenderWrapperParameters {
                common: work::WrapperParameters {
                    scratch_low: lane.scratch_low,
                    scratch_high: lane.scratch_high,
                    support,
                    queue_control: self.queue_control,
                    grid: u16::from(first_event) + stage as u16,
                    item: 0,
                    ordinal: item as u16,
                    context: context as u16,
                    // As in compute, this is the zero-based UAT context,
                    // independently of the PB manager and completion slot.
                    support_id: self.context.checked_sub(1).ok_or(EINVAL)?,
                    uuid: self.uuid,
                    scheduler: lane.priority as u16,
                    scheduler_hint: 2,
                    first: item == 0,
                    // As in the common compute entry, initialization belongs
                    // to this queue's first publication, not the global job.
                    field_52: u16::from(local == 0),
                    field_5e: lane.priority as u16,
                    field_62: u16::from(local == 0),
                },
                fragment,
                buffer_manager: manager,
                // This identity advances across render owners; the selected
                // SKSM record/marker milestone above remains pool-local.
                lifecycle: n as u16,
                namespace: u16::from(self.event_slot),
                item,
                item_control: ((item & 255) << 8) as u16,
            })
            .map_err(|_| EINVAL)?;
            push(&mut objects[stage],prelude,wrapper.as_bytes())?;
            let mut end = [0u8; 0x40];
            end[..4].copy_from_slice(&16u32.to_le_bytes());
            end[4..8].copy_from_slice(
                &(((item + 1) & 65535) << 16 | (u32::from(first_event) + stage as u32)).to_le_bytes(),
            );
            end[8..16]
                .copy_from_slice(&(0x00fe000000000000u64 | (u64::from(item + 1) >> 16)).to_le_bytes());
            push(&mut objects[stage],prelude+0xc0,&end)?;
            push(&mut objects[stage],state.driver_stamp,&stamp.wrapping_sub(0x100).to_le_bytes())?;
        }

        Ok(objects)
    }
}
