// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Scheduled-family Work15, SKSM and compute scheduler staging records.
//! These owned byte images neither publish producers nor borrow live firmware.

use super::wire::Error;
use kernel::macros::versions;

pub(crate) struct WrapperParameters {
    pub(crate) scratch_low: u64,
    pub(crate) scratch_high: u64,
    pub(crate) support: u64,
    pub(crate) queue_control: u64,
    pub(crate) grid: u16,
    pub(crate) item: u16,
    pub(crate) ordinal: u16,
    pub(crate) context: u16,
    pub(crate) support_id: u16,
    pub(crate) uuid: u16,
    pub(crate) scheduler: u16,
    pub(crate) scheduler_hint: u16,
    pub(crate) first: bool,
    // Resolve independent Max defaults in the owner; do not infer these from
    // each other. Neo retains its existing caller-supplied/truncation policy.
    pub(crate) field_52: u16,
    pub(crate) field_5e: u16,
    pub(crate) field_62: u16,
}

#[versions(AGX_SCHEDULED)]
#[repr(C)]
pub(crate) struct WorkWrapper {
    pub(crate) opcode: [u8; 4],
    pub(crate) pad_04: [u8; 4],
    pub(crate) scratch_low: [u8; 8],
    pub(crate) scratch_high: [u8; 8],
    pub(crate) grid: [u8; 2],
    pub(crate) first: [u8; 2],
    pub(crate) pad_1c: [u8; 2],
    pub(crate) scheduler: [u8; 2],
    pub(crate) pad_20: [u8; 2],
    pub(crate) kind: [u8; 2],
    pub(crate) pad_24: [u8; 2],
    pub(crate) render_control: [u8; 2],
    pub(crate) pad_28: [u8; 2],
    pub(crate) item: [u8; 4],
    pub(crate) item_shifted: [u8; 2],
    pub(crate) pad_30: [u8; 2],
    pub(crate) context: [u8; 2],
    pub(crate) pad_34: [u8; 2],
    pub(crate) support: [u8; 8],
    pub(crate) ordinal: [u8; 2],
    pub(crate) pad_40: [u8; 6],
    pub(crate) scheduler_hint: [u8; 2],
    pub(crate) pad_48: [u8; 2],
    pub(crate) queue_control: [u8; 8],
    pub(crate) field_52: [u8; 2],
    pub(crate) pad_54: [u8; 2],
    pub(crate) support_id: [u8; 2],
    pub(crate) pad_58: [u8; 2],
    pub(crate) uuid: [u8; 2],
    pub(crate) pad_5c: [u8; 2],
    pub(crate) field_5e: [u8; 2],
    pub(crate) pad_60: [u8; 2],
    pub(crate) field_62: [u8; 2],
    pub(crate) pad_64: [u8; 2],
    pub(crate) field_66: [u8; 2],
    pub(crate) pad_68: [u8; 6],
    pub(crate) buffer_manager: [u8; 8],
    pub(crate) lifecycle: [u8; 2],
    pub(crate) pad_78: [u8; 6],
    pub(crate) namespace: [u8; 2],
    pub(crate) pad_80: [u8; 2],
    pub(crate) next_event: [u8; 2],
    pub(crate) pad_84: [u8; 2],
    pub(crate) pad_86: [u8; 0x3a],
}

#[versions(AGX_SCHEDULED)]
impl Default for WorkWrapper::ver {
    fn default() -> Self {
        // SAFETY: Every field is a byte array, so all-zero is valid staging.
        let mut raw: Self = unsafe { core::mem::zeroed() };
        raw.opcode = 15u32.to_le_bytes();
        raw.kind = 2u16.to_le_bytes();
        raw.field_66 = 1u16.to_le_bytes();
        raw.lifecycle = [0xff; 2];
        raw.pad_78 = [0xff; 6];
        raw.namespace = [0xff; 2];
        raw.pad_80 = [0xff; 2];
        raw.next_event = [0xff; 2];
        raw.pad_84 = [0xff; 2];
        raw
    }
}

#[versions(AGX_SCHEDULED)]
impl WorkWrapper::ver {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();

    fn from_parameters(p: &WrapperParameters) -> Self {
        Self {
            scratch_low: p.scratch_low.to_le_bytes(),
            scratch_high: p.scratch_high.to_le_bytes(),
            support: p.support.to_le_bytes(),
            queue_control: p.queue_control.to_le_bytes(),
            grid: p.grid.to_le_bytes(),
            item: u32::from(p.item).to_le_bytes(),
            item_shifted: p.item.wrapping_shl(8).to_le_bytes(),
            ordinal: p.ordinal.to_le_bytes(),
            context: p.context.to_le_bytes(),
            support_id: p.support_id.to_le_bytes(),
            uuid: p.uuid.to_le_bytes(),
            scheduler: p.scheduler.to_le_bytes(),
            scheduler_hint: p.scheduler_hint.to_le_bytes(),
            first: (p.first as u16).to_le_bytes(),
            field_52: p.field_52.to_le_bytes(),
            field_5e: p.field_5e.to_le_bytes(),
            field_62: p.field_62.to_le_bytes(),
            ..Self::default()
        }
    }

    #[ver(G == G16C)]
    pub(crate) fn new(p: &WrapperParameters) -> Result<Self, Error> {
        if !(1..64).contains(&p.context)
            || p.grid >= 128
            || [p.scratch_low, p.scratch_high, p.support, p.queue_control].contains(&0)
        {
            return Err(Error::Invalid);
        }
        Ok(Self::from_parameters(p))
    }

    #[ver(G == G17P)]
    pub(crate) fn new(p: &WrapperParameters) -> Self {
        Self::from_parameters(p)
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: Initialized byte arrays have alignment one and no padding.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }
}

pub(crate) struct RecordFields {
    pub(crate) header: u64,
    pub(crate) descriptor: u64,
    pub(crate) queue: u64,
    pub(crate) dependency_header: u64,
    pub(crate) previous: u64,
    pub(crate) field_30: u64,
    pub(crate) control_130: u64,
    pub(crate) retire_control: u64,
    pub(crate) ready: u32,
    pub(crate) arrays: [u64; 2],
    pub(crate) mask: u64,
}

/// Semantic role is a constructor input, not another layout/version axis.
pub(crate) struct RenderWrapperParameters {
    pub(crate) common: WrapperParameters,
    pub(crate) fragment: bool,
    pub(crate) buffer_manager: u64,
    pub(crate) lifecycle: u16,
    pub(crate) namespace: u16,
    pub(crate) item: u32,
    pub(crate) item_control: u16,
}

impl WorkWrapperG16C {
    pub(crate) fn for_render(p: &RenderWrapperParameters) -> Result<Self, Error> {
        let mut raw = Self::new(&p.common)?;
        raw.kind = u16::from(p.fragment).to_le_bytes();
        raw.render_control = 1u16.to_le_bytes();
        raw.item = p.item.to_le_bytes();
        raw.item_shifted = p.item_control.to_le_bytes();
        raw.field_5e = p.common.scheduler.to_le_bytes();
        if !p.fragment {
            raw.buffer_manager = p.buffer_manager.to_le_bytes();
            raw.lifecycle = p.lifecycle.to_le_bytes();
            raw.pad_78 = [0; 6];
            raw.namespace = p.namespace.to_le_bytes();
            raw.pad_80 = [0; 2];
            raw.next_event = (p.common.grid + 1).to_le_bytes();
            raw.pad_84 = [0; 2];
        }
        Ok(raw)
    }
}

pub(crate) struct RenderRecordParametersG16C<'a> {
    pub(crate) descriptor: u64,
    pub(crate) queue: u64,
    pub(crate) grid: u8,
    pub(crate) item: u32,
    pub(crate) fragment: bool,
    pub(crate) timeout: u16,
    pub(crate) ready: bool,
    pub(crate) arrays: &'a [(u32, u16)],
    pub(crate) control: Option<u16>,
    pub(crate) control_low: Option<u32>,
    pub(crate) field_30: Option<u64>,
    pub(crate) field_38: u64,
}

impl WorkRecordG16C {
    pub(crate) fn for_render(p: &RenderRecordParametersG16C<'_>) -> Result<Self, Error> {
        if p.grid >= 128 || p.item >= 0x3fffffff || p.queue == 0
            || (p.fragment && p.grid == 0) || p.arrays.is_empty() || p.arrays.len() > 4 {
            return Err(Error::Invalid);
        }
        let mut arrays = [[0; 8]; 4];
        for (i, &(offset, count)) in p.arrays.iter().enumerate() {
            arrays[i] = register_locator_g16c(p.descriptor, offset, count)?.to_le_bytes();
        }
        let control = p.control.unwrap_or(if p.fragment { 0x1804 } else { 0xc04 });
        let low = p.control_low.unwrap_or(if p.fragment { 3 } else { 1 });
        let previous = ((p.grid - u8::from(p.fragment)) as u64) << 40
            | (p.item + u32::from(p.fragment)) as u64;
        let mut raw = Self {
            header: (1u64 << 60 | (p.grid as u64) << 42 | (p.item as u64 + 1) * 4).to_le_bytes(),
            descriptor: p.descriptor.to_le_bytes(),
            queue: p.queue.to_le_bytes(),
            dependency_header: ((p.timeout as u64) << 48 | (control as u64) << 32 | low as u64).to_le_bytes(),
            previous: previous.to_le_bytes(),
            retire_control: [0; 8],
            ready: if p.ready { 0x40000000u32.to_le_bytes() } else { [0; 4] },
            arrays,
            ..Self::default()
        };
        raw.field_30.copy_from_slice(&p.field_30.unwrap_or(
            if p.fragment { (p.grid as u64) << 40 | p.item as u64 } else { 0 }).to_le_bytes());
        raw.field_38.copy_from_slice(&p.field_38.to_le_bytes());
        Ok(raw)
    }
}

#[versions(AGX_SCHEDULED)]
#[repr(C)]
pub(crate) struct WorkRecord {
    pub(crate) header: [u8; 8],
    pub(crate) pad_008: [u8; 8],
    pub(crate) descriptor: [u8; 8],
    pub(crate) queue: [u8; 8],
    pub(crate) dependency_header: [u8; 8],
    pub(crate) previous: [u8; 8],
    pub(crate) field_30: [u8; 8],
    pub(crate) field_38: [u8; 8],
    pub(crate) field_40: [u8;8],
    pub(crate) dependency_tail: [u8; 0xe8],
    pub(crate) control_130: [u8; 8],
    pub(crate) retire_control: [u8; 8],
    pub(crate) pad_140: [u8; 4],
    pub(crate) ready: [u8; 4],
    pub(crate) pad_148: [u8; 8],
    pub(crate) arrays: [[u8; 8]; 4],
    pub(crate) pad_170: [u8; 8],
    pub(crate) mask: [u8; 8],
    pub(crate) pad_180: [u8; 0x80],
}

#[versions(AGX_SCHEDULED)]
impl Default for WorkRecord::ver {
    fn default() -> Self {
        // SAFETY: Every field is a byte array, so all-zero is valid staging.
        #[allow(unused_mut)]
        let mut raw: Self = unsafe { core::mem::zeroed() };
        #[ver(G == G16C)]
        {
            raw.header = (1u64 << 60).to_le_bytes();
            raw.dependency_header = 0x0fa0080400000001u64.to_le_bytes();
            raw.retire_control = 8u64.to_le_bytes();
            raw.ready = 0x40000000u32.to_le_bytes();
            raw.mask = 0x003fffffffffffffu64.to_le_bytes();
        }
        raw
    }
}

#[versions(AGX_SCHEDULED)]
impl WorkRecord::ver {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();

    pub(crate) fn from_fields(p: RecordFields) -> Self {
        let mut raw = Self {
            header: p.header.to_le_bytes(),
            descriptor: p.descriptor.to_le_bytes(),
            queue: p.queue.to_le_bytes(),
            dependency_header: p.dependency_header.to_le_bytes(),
            previous: p.previous.to_le_bytes(),
            control_130: p.control_130.to_le_bytes(),
            retire_control: p.retire_control.to_le_bytes(),
            ready: p.ready.to_le_bytes(),
            mask: p.mask.to_le_bytes(),
            ..Self::default()
        };
        raw.field_30.copy_from_slice(&p.field_30.to_le_bytes());
        for (out, value) in raw.arrays.iter_mut().zip(p.arrays) {
            *out = value.to_le_bytes();
        }
        raw
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: Initialized byte arrays have alignment one and no padding.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }
}

pub(crate) struct RecordParametersG16C {
    pub(crate) descriptor: u64,
    pub(crate) queue: u64,
    pub(crate) grid: u8,
    pub(crate) item: u32,
    pub(crate) primary_count: u16,
    pub(crate) timeout: u16,
    pub(crate) ready: bool,
    pub(crate) retire_control: u64,
    pub(crate) flags: u64,
    pub(crate) control: u16,
    pub(crate) control_130: u64,
    pub(crate) header_grid: Option<u8>,
    pub(crate) header_count: Option<u32>,
    pub(crate) control_low: u32,
    pub(crate) field_30: u64,
}

impl Default for RecordParametersG16C {
    fn default() -> Self {
        Self {
            descriptor: 0,
            queue: 0,
            grid: 0,
            item: 0,
            primary_count: 34,
            timeout: 0xfa0,
            ready: true,
            retire_control: 8,
            flags: 1 << 60,
            control: 0x0804,
            control_130: 0,
            header_grid: None,
            header_count: None,
            control_low: 1,
            field_30: 0,
        }
    }
}

pub(crate) const DESCRIPTOR_BASE_G16C: u64 = 0xfffffc20c0018000;

pub(crate) fn register_locator_g16c(
    descriptor: u64,
    offset: u32,
    count: u16,
) -> Result<u64, Error> {
    let address = descriptor
        .checked_add(offset as u64)
        .ok_or(Error::Overflow)?;
    let delta = address
        .checked_sub(DESCRIPTOR_BASE_G16C)
        .ok_or(Error::Invalid)?;
    if delta & 31 != 0 || delta / 32 >= 1 << 31 || !(1..128).contains(&count) {
        return Err(Error::Invalid);
    }
    Ok((count as u64 * 8) << 40 | 3 << 32 | 1 << 31 | delta / 32)
}

impl WorkRecordG16C {
    /// Ordered engine handoff vector from the two/four pending Metal queues.
    /// Each entry names the preceding item on that engine; the current
    /// engine's own entry must name exactly this record's prior item.
    pub(crate) fn for_compute_dependencies(p:&RecordParametersG16C,points:&[(u8,u32)])->Result<Self,Error> {
        Self::for_compute_dependency_span(p, points.len() as u8, points)
    }
    /// Compatibility with the original two/four-queue fixture selector.
    /// Its bound is not a general relationship between control and waits.
    pub(crate) fn for_compute_dependency_span(p:&RecordParametersG16C,span:u8,points:&[(u8,u32)])->Result<Self,Error> {
        if !(1..=4).contains(&span) || points.len()>span as usize { return Err(Error::Invalid); }
        Self::for_compute_control_dependencies(p, 0x0800 + u16::from(span) - 1, points)
    }
    /// Keep the opaque queue control independent of the enabled predecessor
    /// vector. Two physical queues belonging to one API queue both use 0x0800.
    pub(crate) fn for_compute_control_dependencies(p:&RecordParametersG16C,control:u16,points:&[(u8,u32)])->Result<Self,Error> {
        if points.is_empty() || points.len()>4 || points.iter().enumerate().any(|(i,(grid,value))|
            *grid>=128 || *value>=1<<30 || (i>0 && points[i-1].0>=*grid))
            || !points.iter().any(|&(grid,value)|grid==p.grid && value==p.item) {
            return Err(Error::Invalid);
        }
        let mut raw=Self::new(p)?;
        raw.dependency_header=((u64::from(p.timeout)<<48)|(u64::from(control)<<32)|((1u64<<points.len())-1)).to_le_bytes();
        let mut values=[[0u8;8];4];
        for (out,&(grid,value))in values.iter_mut().zip(points) {*out=((u64::from(grid)<<40)|u64::from(value)).to_le_bytes();}
        [raw.previous,raw.field_30,raw.field_38,raw.field_40]=values;
        Ok(raw)
    }
    pub(crate) fn new(p: &RecordParametersG16C) -> Result<Self, Error> {
        if p.grid >= 128 || p.item >= 0x3fffffff || p.queue == 0 || ![0, 1 << 60].contains(&p.flags)
        {
            return Err(Error::Invalid);
        }
        let grid = p.header_grid.unwrap_or(p.grid);
        let count = p.header_count.unwrap_or(p.item + 1);
        if grid >= 128 || count >= 1 << 30 {
            return Err(Error::Invalid);
        }
        Ok(Self::from_fields(RecordFields {
            header: p.flags | (grid as u64) << 42 | count as u64 * 4,
            descriptor: p.descriptor,
            queue: p.queue,
            dependency_header: (p.timeout as u64) << 48
                | (p.control as u64) << 32
                | p.control_low as u64,
            previous: (p.grid as u64) << 40 | p.item as u64,
            field_30: p.field_30,
            control_130: p.control_130,
            retire_control: p.retire_control,
            ready: if p.ready { 0x40000000 } else { 0 },
            arrays: [
                register_locator_g16c(p.descriptor, 0x40, p.primary_count)?,
                register_locator_g16c(p.descriptor, 0x760, 4)?,
            ],
            mask: 0x003fffffffffffff,
        }))
    }
}

pub(crate) struct SchedulerParameters {
    pub(crate) slot: u64,
    pub(crate) work_id: u32,
    pub(crate) phase: u32,
    pub(crate) job_list: u64,
    pub(crate) node_id: u64,
    pub(crate) completion_kind: u64,
}

#[versions(AGX_SCHEDULED)]
#[repr(C)]
pub(crate) struct ComputeScheduler {
    pub(crate) slot: [u8; 8],
    pub(crate) work_id: [u8; 4],
    pub(crate) phase: [u8; 4],
    pub(crate) control: [u8; 4],
    pub(crate) pad_14: [u8; 0x10],
    pub(crate) active: [u8; 4],
    pub(crate) pad_28: [u8; 0x78],
    pub(crate) job_list: [u8; 8],
    pub(crate) node: [u8; 8],
    pub(crate) previous_node: [u8; 8],
    pub(crate) pad_b8: [u8; 8],
    pub(crate) completion_kind: [u8; 8],
    pub(crate) pad_c8: [u8; 0x38],
}

#[versions(AGX_SCHEDULED)]
impl Default for ComputeScheduler::ver {
    fn default() -> Self {
        // SAFETY: Every field is a byte array, so all-zero is valid staging.
        let mut raw: Self = unsafe { core::mem::zeroed() };
        raw.control = 0x50u32.to_le_bytes();
        raw
    }
}

#[versions(AGX_SCHEDULED)]
impl ComputeScheduler::ver {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();

    #[ver(G == G16C)]
    pub(crate) fn for_compute(slot: u64) -> Self {
        Self {
            slot: slot.to_le_bytes(),
            ..Self::default()
        }
    }

    #[ver(G == G17P)]
    pub(crate) fn new(p: SchedulerParameters) -> Self {
        let mut raw = Self {
            slot: p.slot.to_le_bytes(),
            work_id: p.work_id.to_le_bytes(),
            phase: p.phase.to_le_bytes(),
            active: u32::from(p.phase != 0).to_le_bytes(),
            job_list: p.job_list.to_le_bytes(),
            ..Self::default()
        };
        if p.node_id != 0 {
            raw.node = (0x2000000 | p.node_id).to_le_bytes();
            raw.previous_node = (0x2000000 | (p.node_id - 1)).to_le_bytes();
            raw.completion_kind = 1u64.to_le_bytes();
        }
        if p.completion_kind != 0 {
            raw.completion_kind = p.completion_kind.to_le_bytes();
        }
        raw
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: Initialized byte arrays have alignment one and no padding.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }
}

#[versions(AGX_SCHEDULED)]
const COMPUTEWRAPPER_LAYOUT: () = {
    assert!(core::mem::size_of::<WorkWrapper::ver>() == 0xc0);
    assert!(core::mem::align_of::<WorkWrapper::ver>() == 1);
    assert!(core::mem::offset_of!(WorkWrapper::ver, opcode) == 0x0);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_04) == 0x4);
    assert!(core::mem::offset_of!(WorkWrapper::ver, scratch_low) == 0x8);
    assert!(core::mem::offset_of!(WorkWrapper::ver, scratch_high) == 0x10);
    assert!(core::mem::offset_of!(WorkWrapper::ver, grid) == 0x18);
    assert!(core::mem::offset_of!(WorkWrapper::ver, first) == 0x1a);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_1c) == 0x1c);
    assert!(core::mem::offset_of!(WorkWrapper::ver, scheduler) == 0x1e);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_20) == 0x20);
    assert!(core::mem::offset_of!(WorkWrapper::ver, kind) == 0x22);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_24) == 0x24);
    assert!(core::mem::offset_of!(WorkWrapper::ver, render_control) == 0x26);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_28) == 0x28);
    assert!(core::mem::offset_of!(WorkWrapper::ver, item) == 0x2a);
    assert!(core::mem::offset_of!(WorkWrapper::ver, item_shifted) == 0x2e);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_30) == 0x30);
    assert!(core::mem::offset_of!(WorkWrapper::ver, context) == 0x32);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_34) == 0x34);
    assert!(core::mem::offset_of!(WorkWrapper::ver, support) == 0x36);
    assert!(core::mem::offset_of!(WorkWrapper::ver, ordinal) == 0x3e);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_40) == 0x40);
    assert!(core::mem::offset_of!(WorkWrapper::ver, scheduler_hint) == 0x46);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_48) == 0x48);
    assert!(core::mem::offset_of!(WorkWrapper::ver, queue_control) == 0x4a);
    assert!(core::mem::offset_of!(WorkWrapper::ver, field_52) == 0x52);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_54) == 0x54);
    assert!(core::mem::offset_of!(WorkWrapper::ver, support_id) == 0x56);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_58) == 0x58);
    assert!(core::mem::offset_of!(WorkWrapper::ver, uuid) == 0x5a);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_5c) == 0x5c);
    assert!(core::mem::offset_of!(WorkWrapper::ver, field_5e) == 0x5e);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_60) == 0x60);
    assert!(core::mem::offset_of!(WorkWrapper::ver, field_62) == 0x62);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_64) == 0x64);
    assert!(core::mem::offset_of!(WorkWrapper::ver, field_66) == 0x66);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_68) == 0x68);
    assert!(core::mem::offset_of!(WorkWrapper::ver, buffer_manager) == 0x6e);
    assert!(core::mem::offset_of!(WorkWrapper::ver, lifecycle) == 0x76);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_78) == 0x78);
    assert!(core::mem::offset_of!(WorkWrapper::ver, namespace) == 0x7e);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_80) == 0x80);
    assert!(core::mem::offset_of!(WorkWrapper::ver, next_event) == 0x82);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_84) == 0x84);
    assert!(core::mem::offset_of!(WorkWrapper::ver, pad_86) == 0x86);
};

#[versions(AGX_SCHEDULED)]
const COMPUTERECORD_LAYOUT: () = {
    assert!(core::mem::size_of::<WorkRecord::ver>() == 0x200);
    assert!(core::mem::align_of::<WorkRecord::ver>() == 1);
    assert!(core::mem::offset_of!(WorkRecord::ver, header) == 0x0);
    assert!(core::mem::offset_of!(WorkRecord::ver, pad_008) == 0x8);
    assert!(core::mem::offset_of!(WorkRecord::ver, descriptor) == 0x10);
    assert!(core::mem::offset_of!(WorkRecord::ver, queue) == 0x18);
    assert!(core::mem::offset_of!(WorkRecord::ver, dependency_header) == 0x20);
    assert!(core::mem::offset_of!(WorkRecord::ver, previous) == 0x28);
    assert!(core::mem::offset_of!(WorkRecord::ver, field_30) == 0x30);
    assert!(core::mem::offset_of!(WorkRecord::ver, field_38) == 0x38);
    assert!(core::mem::offset_of!(WorkRecord::ver, field_40) == 0x40);
    assert!(core::mem::offset_of!(WorkRecord::ver, dependency_tail) == 0x48);
    assert!(core::mem::offset_of!(WorkRecord::ver, control_130) == 0x130);
    assert!(core::mem::offset_of!(WorkRecord::ver, retire_control) == 0x138);
    assert!(core::mem::offset_of!(WorkRecord::ver, pad_140) == 0x140);
    assert!(core::mem::offset_of!(WorkRecord::ver, ready) == 0x144);
    assert!(core::mem::offset_of!(WorkRecord::ver, pad_148) == 0x148);
    assert!(core::mem::offset_of!(WorkRecord::ver, arrays) == 0x150);
    assert!(core::mem::offset_of!(WorkRecord::ver, pad_170) == 0x170);
    assert!(core::mem::offset_of!(WorkRecord::ver, mask) == 0x178);
    assert!(core::mem::offset_of!(WorkRecord::ver, pad_180) == 0x180);
};

#[versions(AGX_SCHEDULED)]
const COMPUTESCHEDULER_LAYOUT: () = {
    assert!(core::mem::size_of::<ComputeScheduler::ver>() == 0x100);
    assert!(core::mem::align_of::<ComputeScheduler::ver>() == 1);
    assert!(core::mem::offset_of!(ComputeScheduler::ver, slot) == 0x0);
    assert!(core::mem::offset_of!(ComputeScheduler::ver, work_id) == 0x8);
    assert!(core::mem::offset_of!(ComputeScheduler::ver, phase) == 0xc);
    assert!(core::mem::offset_of!(ComputeScheduler::ver, control) == 0x10);
    assert!(core::mem::offset_of!(ComputeScheduler::ver, pad_14) == 0x14);
    assert!(core::mem::offset_of!(ComputeScheduler::ver, active) == 0x24);
    assert!(core::mem::offset_of!(ComputeScheduler::ver, pad_28) == 0x28);
    assert!(core::mem::offset_of!(ComputeScheduler::ver, job_list) == 0xa0);
    assert!(core::mem::offset_of!(ComputeScheduler::ver, node) == 0xa8);
    assert!(core::mem::offset_of!(ComputeScheduler::ver, previous_node) == 0xb0);
    assert!(core::mem::offset_of!(ComputeScheduler::ver, pad_b8) == 0xb8);
    assert!(core::mem::offset_of!(ComputeScheduler::ver, completion_kind) == 0xc0);
    assert!(core::mem::offset_of!(ComputeScheduler::ver, pad_c8) == 0xc8);
};

// Compatibility names for existing compute callers; one wire definition
// serves the three queue kinds in each hardware ABI profile.
pub(crate) type ComputeWrapperG16C = WorkWrapperG16C;
pub(crate) type ComputeWrapperG17P = WorkWrapperG17P;
pub(crate) type ComputeRecordG16C = WorkRecordG16C;
pub(crate) type ComputeRecordG17P = WorkRecordG17P;
