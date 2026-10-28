// SPDX-License-Identifier: GPL-2.0-only

//! Scheduled-family TA/fragment descriptor DATA, macOS 26.6.2.
//! Byte-array layouts apply only to exclusive CPU staging, never live DMA.
//! Programs and ownership/lifecycle inputs remain explicit constructor inputs.

use super::super::g17p_compute::{Error, Register};
use kernel::macros::versions;

/// Host values shared by the descriptor bodies. Geometry/register construction
/// supplies derived sizes explicitly; this is not a firmware wire structure.
pub(crate) struct Parameters {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) samples: u32,
    pub(crate) tiles: u64,
    pub(crate) tpc: u64,
    pub(crate) tpc_size: u64,
    pub(crate) sampler_array: u64,
    pub(crate) sampler_count: u32,
    pub(crate) request_growth: bool,
    pub(crate) sync_grow: bool,
    pub(crate) process_empty_tiles: bool,
    pub(crate) depth_bias_array: u64,
    pub(crate) scissor_array: u64,
    pub(crate) occlusion_query_base: u64,
    pub(crate) load_pipeline_bind: u64,
    pub(crate) load_pipeline: u64,
    pub(crate) partial_load_pipeline_bind: u64,
    pub(crate) partial_load_pipeline: u64,
    pub(crate) depth_flags: u64,
    pub(crate) depth_buffer: u64,
    pub(crate) depth_stride: u64,
    pub(crate) depth_aux_stride: u64,
    pub(crate) depth_aux_buffer: u64,
    pub(crate) stencil_buffer: u64,
    pub(crate) stencil_stride: u64,
    pub(crate) stencil_aux_stride: u64,
    pub(crate) stencil_aux_buffer: u64,
    pub(crate) tib_encoding: u64,
    pub(crate) aux_fb_flags: u64,
    pub(crate) aux_fb_page_count: u64,
    pub(crate) tile_config: u64,
    pub(crate) store_pipeline_bind: u32,
    pub(crate) store_pipeline: u64,
    pub(crate) partial_store_pipeline_bind: u32,
    pub(crate) partial_store_pipeline: u64,
    pub(crate) depth_clear_value_bits: u32,
    pub(crate) stencil_clear: u32,
    pub(crate) sample_size: u32,
    pub(crate) depth_dimensions: u64,
}

/// Owned submission state. Independent protocol scalars are not inferred from
/// a counter, dimensions, or each other. The owner resolves first-use policy.
pub(crate) struct State {
    pub(crate) context: u32,
    pub(crate) pool: u32,
    pub(crate) event_control: u64,
    pub(crate) manager: u64,
    pub(crate) scene: u64,
    pub(crate) empty_scene: u64,
    pub(crate) alias: u64,
    pub(crate) shared_tail: u64,
    pub(crate) driver_stamp: u64,
    pub(crate) firmware_stamp: u64,
    pub(crate) support: u64,
    pub(crate) counter: u64,
    pub(crate) stamp: u32,
    pub(crate) event: u32,
    pub(crate) uuid: u32,
    pub(crate) timestamp: u64,
    pub(crate) timestamp_end: Option<u64>,
    pub(crate) user_timestamp_start: u64,
    pub(crate) user_timestamp_end: u64,
    pub(crate) fragment_event: u32,
    pub(crate) fragment_stamp: u32,
    pub(crate) stage_flag: u32,
    pub(crate) field_93c: u64,
    pub(crate) field_2128: u32,
    pub(crate) field_21d8: u32,
    pub(crate) field_222c: u32,
    pub(crate) secondary_stamp: Option<u32>,
    pub(crate) descriptor_ordinal: Option<u32>,
    pub(crate) work_index: Option<u32>,
    pub(crate) tail_flags: u8,
    pub(crate) tiling_submission_tag: u32,
    pub(crate) tiling_queue_ordinal: u32,
}

pub(crate) struct Programs<'a> {
    pub(crate) primary: &'a [Register],
    pub(crate) partial_store: &'a [Register],
    pub(crate) partial_resume: &'a [Register],
    pub(crate) partial_load: &'a [Register],
}

fn one(program: &[Register], number: u32) -> Result<u64, Error> {
    program
        .iter()
        .rev()
        .find(|(n, _)| *n == number)
        .map(|(_, value)| *value)
        .ok_or(Error::MissingRegister)
}

fn narrow(value: u64) -> Result<u32, Error> {
    value.try_into().map_err(|_| Error::Overflow)
}

fn count(program: &[Register], capacity: usize) -> Result<u32, Error> {
    let n = program.len();
    if n == 0 || n > 128 || n * 12 > capacity {
        return Err(Error::Invalid);
    }
    Ok(((n as u32 * 12) << 16) | n as u32)
}

fn encode(out: &mut [u8], program: &[Register]) {
    for (out, &(number, value)) in out.chunks_exact_mut(12).zip(program) {
        out[..4].copy_from_slice(&number.to_le_bytes());
        out[4..].copy_from_slice(&value.to_le_bytes());
    }
}

fn preflight(p: &Parameters, s: &State, item: u32, grid: u32) -> Result<(u32, u64), Error> {
    if !(1..64).contains(&s.context)
        || s.pool >= 15
        || grid >= 128
        || s.event != grid
        || !(1..=16384).contains(&p.width)
        || !(1..=16384).contains(&p.height)
        || ![1, 2, 4].contains(&p.samples)
        || p.sampler_count == u32::MAX
        || (p.sampler_array == 0) != (p.sampler_count == 0)
        || p.sampler_array & 7 != 0
    {
        return Err(Error::Invalid);
    }
    let next = item.checked_add(1).ok_or(Error::Overflow)?;
    // The source validates this even when work_index overrides the field.
    item.checked_add(item / 2).ok_or(Error::Overflow)?;
    let tail = s
        .shared_tail
        .checked_add(u64::from(item) * 0x40)
        .ok_or(Error::Overflow)?;
    Ok((next, tail))
}

#[versions(AGX_SCHEDULED)]
#[repr(C)]
// Native adjacent allocations prove these bounds for both G16C and G17P.
// Bytes beyond the object can be the next job header, not an extension.
pub(crate) struct TilingDescriptor {
    pub(crate) opcode: [u8; 0x4],
    pub(crate) counter: [u8; 0x8],
    pub(crate) context: [u8; 0x4],
    pub(crate) event_control: [u8; 0x8],
    pub(crate) pool: [u8; 0x4],
    pub(crate) pad_001c: [u8; 0x4],
    pub(crate) manager: [u8; 0x8],
    pub(crate) scene: [u8; 0x8],
    pub(crate) empty_scene: [u8; 0x8],
    pub(crate) counts: [u8; 0x6],
    pub(crate) pad_003e: [u8; 0xa],
    pub(crate) work_index: [u8; 0x4],
    pub(crate) pad_004c: [u8; 0x14],
    pub(crate) primary: [u8; 0x700],
    pub(crate) primary_address: [u8; 0x8],
    pub(crate) primary_count: [u8; 0x4],
    pub(crate) pad_076c: [u8; 0x14],
    pub(crate) tpc: [u8; 0x8],
    pub(crate) tpc_size: [u8; 0x8],
    pub(crate) pad_0790: [u8; 0xc],
    pub(crate) fragment_event: [u8; 0x4],
    pub(crate) fragment_stamp: [u8; 0x4],
    pub(crate) pad_07a4: [u8; 0x4],
    pub(crate) next_item: [u8; 0x4],
    pub(crate) pad_07ac: [u8; 0x4],
    pub(crate) submission_tag: [u8; 0x4],
    pub(crate) pad_07b4: [u8; 0x22],
    pub(crate) deflake: [u8; 0x4],
    pub(crate) pad_07da: [u8; 0x94],
    pub(crate) lifecycle_high: [u8; 0x8],
    pub(crate) sampler_sentinel: [u8; 0x4],
    pub(crate) sampler_array: [u8; 0x8],
    pub(crate) sampler_count: [u8; 0x4],
    pub(crate) sampler_limit: [u8; 0x4],
    pub(crate) pad_088a: [u8; 0x8],
    pub(crate) request_growth: [u8; 0x1],
    pub(crate) pad_0893: [u8; 0x7],
    pub(crate) vertex_store_flag: [u8; 0x1],
    pub(crate) pad_089b: [u8; 0xb],
    pub(crate) driver_stamp: [u8; 0x8],
    pub(crate) firmware_stamp: [u8; 0x8],
    pub(crate) stamp: [u8; 0x4],
    pub(crate) event: [u8; 0x4],
    pub(crate) pad_08be: [u8; 0x4],
    pub(crate) stage_flag: [u8; 0x4],
    pub(crate) next_work: [u8; 0x4],
    pub(crate) secondary_stamp: [u8; 0x4],
    pub(crate) uuid: [u8; 0x4],
    pub(crate) descriptor_ordinal: [u8; 0x4],
    pub(crate) queue_ordinal: [u8; 0x4],
    pub(crate) pad_08da: [u8; 0x24],
    pub(crate) timestamp_start: [u8; 0x8],
    pub(crate) timestamp_end: [u8; 0x8],
    pub(crate) user_timestamp_start: [u8; 0x8],
    pub(crate) user_timestamp_end: [u8; 0x8],
    pub(crate) pad_091e: [u8; 0x14],
    pub(crate) support_opcode: [u8; 0x1],
    pub(crate) pad_0933: [u8; 0x1],
    pub(crate) support: [u8; 0x8],
    pub(crate) field_93c: [u8; 0x8],
    pub(crate) pad_0944: [u8; 0x1],
    pub(crate) shared_tail: [u8; 0x8],
    pub(crate) tail_flags: [u8; 0x1],
    pub(crate) pad_094e: [u8; 0x72],
}

#[versions(AGX_SCHEDULED)]
const TILING_LAYOUT: () = {
    assert!(core::mem::size_of::<TilingDescriptor::ver>() == 0x9c0);
    assert!(core::mem::align_of::<TilingDescriptor::ver>() == 1);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, opcode) == 0x0);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, counter) == 0x4);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, context) == 0xc);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, event_control) == 0x10);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pool) == 0x18);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_001c) == 0x1c);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, manager) == 0x20);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, scene) == 0x28);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, empty_scene) == 0x30);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, counts) == 0x38);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_003e) == 0x3e);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, work_index) == 0x48);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_004c) == 0x4c);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, primary) == 0x60);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, primary_address) == 0x760);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, primary_count) == 0x768);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_076c) == 0x76c);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, tpc) == 0x780);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, tpc_size) == 0x788);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_0790) == 0x790);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, fragment_event) == 0x79c);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, fragment_stamp) == 0x7a0);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_07a4) == 0x7a4);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, next_item) == 0x7a8);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_07ac) == 0x7ac);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, submission_tag) == 0x7b0);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_07b4) == 0x7b4);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, deflake) == 0x7d6);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_07da) == 0x7da);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, lifecycle_high) == 0x86e);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, sampler_sentinel) == 0x876);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, sampler_array) == 0x87a);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, sampler_count) == 0x882);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, sampler_limit) == 0x886);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_088a) == 0x88a);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, request_growth) == 0x892);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_0893) == 0x893);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, vertex_store_flag) == 0x89a);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_089b) == 0x89b);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, driver_stamp) == 0x8a6);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, firmware_stamp) == 0x8ae);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, stamp) == 0x8b6);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, event) == 0x8ba);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_08be) == 0x8be);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, stage_flag) == 0x8c2);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, next_work) == 0x8c6);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, secondary_stamp) == 0x8ca);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, uuid) == 0x8ce);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, descriptor_ordinal) == 0x8d2);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, queue_ordinal) == 0x8d6);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_08da) == 0x8da);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, timestamp_start) == 0x8fe);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, timestamp_end) == 0x906);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, user_timestamp_start) == 0x90e);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, user_timestamp_end) == 0x916);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_091e) == 0x91e);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, support_opcode) == 0x932);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_0933) == 0x933);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, support) == 0x934);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, field_93c) == 0x93c);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_0944) == 0x944);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, shared_tail) == 0x945);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, tail_flags) == 0x94d);
    assert!(core::mem::offset_of!(TilingDescriptor::ver, pad_094e) == 0x94e);
};

#[versions(AGX_SCHEDULED)]
#[repr(C)]
pub(crate) struct FragmentDescriptor {
    pub(crate) opcode: [u8; 0x4],
    pub(crate) counter: [u8; 0x8],
    pub(crate) context: [u8; 0x4],
    pub(crate) pad_0010: [u8; 0x10],
    pub(crate) event_control: [u8; 0x8],
    pub(crate) manager: [u8; 0x8],
    pub(crate) scene: [u8; 0x8],
    pub(crate) empty_scene: [u8; 0x8],
    pub(crate) tilemap: [u8; 0x8],
    pub(crate) multisample_control: [u8; 0x8],
    pub(crate) samples: [u8; 0x4],
    pub(crate) macro_size: [u8; 0x4],
    pub(crate) pad_0058: [u8; 0x10],
    pub(crate) merge_upper_x: [u8; 0x4],
    pub(crate) merge_upper_y: [u8; 0x4],
    pub(crate) pad_0070: [u8; 0x8],
    pub(crate) tiles: [u8; 0x8],
    pub(crate) counts: [u8; 0x6],
    pub(crate) pad_0086: [u8; 0x2],
    pub(crate) primary_words: [u8; 0x2],
    pub(crate) pad_008a: [u8; 0x6],
    pub(crate) work_index: [u8; 0x4],
    pub(crate) pad_0094: [u8; 0xc],
    pub(crate) primary: [u8; 0x700],
    pub(crate) primary_address: [u8; 0x8],
    pub(crate) primary_count: [u8; 0x4],
    pub(crate) pad_07ac: [u8; 0x14],
    pub(crate) partial_store: [u8; 0x700],
    pub(crate) partial_store_address: [u8; 0x8],
    pub(crate) partial_store_count: [u8; 0x4],
    pub(crate) pad_0ecc: [u8; 0x14],
    pub(crate) partial_resume: [u8; 0x700],
    pub(crate) partial_resume_address: [u8; 0x8],
    pub(crate) partial_resume_count: [u8; 0x4],
    pub(crate) pad_15ec: [u8; 0x14],
    pub(crate) partial_load: [u8; 0x100],
    pub(crate) pad_1700: [u8; 0x600],
    pub(crate) partial_load_address: [u8; 0x8],
    pub(crate) partial_load_count: [u8; 0x4],
    pub(crate) pad_1d0c: [u8; 0x14],
    pub(crate) depth_bias_array: [u8; 0x8],
    pub(crate) pad_1d28: [u8; 0x8],
    pub(crate) scissor_array: [u8; 0x8],
    pub(crate) pad_1d38: [u8; 0x8],
    pub(crate) occlusion_query_base: [u8; 0x8],
    pub(crate) pad_1d48: [u8; 0x130],
    pub(crate) load_pipeline_bind: [u8; 0x8],
    pub(crate) load_pipeline: [u8; 0x8],
    pub(crate) pad_1e88: [u8; 0x20],
    pub(crate) partial_load_pipeline_bind: [u8; 0x8],
    pub(crate) partial_load_pipeline: [u8; 0x8],
    pub(crate) depth_flags: [u8; 0x8],
    pub(crate) depth_control: [u8; 0x4],
    pub(crate) pad_1ec4: [u8; 0x4],
    pub(crate) depth_buffer: [u8; 0x8],
    pub(crate) depth_stride: [u8; 0x8],
    pub(crate) depth_aux_stride: [u8; 0x8],
    pub(crate) depth_buffer_repeat: [u8; 0x8],
    pub(crate) depth_buffer_repeat2: [u8; 0x8],
    pub(crate) depth_aux_buffer: [u8; 0x8],
    pub(crate) stencil_buffer: [u8; 0x8],
    pub(crate) stencil_stride: [u8; 0x8],
    pub(crate) stencil_aux_stride: [u8; 0x8],
    pub(crate) stencil_buffer_repeat: [u8; 0x8],
    pub(crate) stencil_buffer_repeat2: [u8; 0x8],
    pub(crate) stencil_aux_buffer: [u8; 0x8],
    pub(crate) pad_1f28: [u8; 0x10],
    pub(crate) tib_encoding: [u8; 0x8],
    pub(crate) aux_fb_flags: [u8; 0x8],
    pub(crate) width: [u8; 0x4],
    pub(crate) height: [u8; 0x4],
    pub(crate) aux_fb_page_count: [u8; 0x8],
    pub(crate) tile_config: [u8; 0x8],
    pub(crate) pad_1f60: [u8; 0x18],
    pub(crate) store_pipeline_bind: [u8; 0x4],
    pub(crate) store_pipeline: [u8; 0x8],
    pub(crate) pad_1f84: [u8; 0x14],
    pub(crate) partial_store_pipeline_bind: [u8; 0x4],
    pub(crate) partial_store_pipeline: [u8; 0x8],
    pub(crate) pad_1fa4: [u8; 0x4],
    pub(crate) depth_clear_value_bits: [u8; 0x4],
    pub(crate) stencil_clear: [u8; 0x4],
    pub(crate) sample_size: [u8; 0x4],
    pub(crate) pad_1fb4: [u8; 0x1c],
    pub(crate) depth_dimensions: [u8; 0x8],
    pub(crate) pad_1fd8: [u8; 0x124],
    pub(crate) fragment_store_flag: [u8; 0x1],
    pub(crate) pad_20fd: [u8; 0x3],
    pub(crate) sync_grow: [u8; 0x1],
    pub(crate) pad_2101: [u8; 0x7],
    pub(crate) lifecycle_high: [u8; 0x8],
    pub(crate) sampler_sentinel: [u8; 0x4],
    pub(crate) sampler_array: [u8; 0x8],
    pub(crate) sampler_count: [u8; 0x4],
    pub(crate) sampler_limit: [u8; 0x4],
    pub(crate) process_empty_tiles: [u8; 0x1],
    pub(crate) pad_2125: [u8; 0x3],
    pub(crate) field_2128: [u8; 0x4],
    pub(crate) multisample: [u8; 0x4],
    pub(crate) pad_2130: [u8; 0x10],
    pub(crate) driver_stamp: [u8; 0x8],
    pub(crate) firmware_stamp: [u8; 0x8],
    pub(crate) stamp: [u8; 0x4],
    pub(crate) event: [u8; 0x4],
    pub(crate) pad_2158: [u8; 0x4],
    pub(crate) stage_flag: [u8; 0x4],
    pub(crate) next_work: [u8; 0x4],
    pub(crate) secondary_stamp: [u8; 0x4],
    pub(crate) uuid: [u8; 0x4],
    pub(crate) pad_216c: [u8; 0x4],
    pub(crate) descriptor_ordinal: [u8; 0x4],
    pub(crate) pad_2174: [u8; 0x24],
    pub(crate) timestamp_start: [u8; 0x8],
    pub(crate) timestamp_end: [u8; 0x8],
    pub(crate) user_timestamp_start: [u8; 0x8],
    pub(crate) user_timestamp_end: [u8; 0x8],
    pub(crate) pad_21b8: [u8; 0x14],
    pub(crate) support_opcode: [u8; 0x1],
    pub(crate) pad_21cd: [u8; 0x1],
    pub(crate) support: [u8; 0x8],
    pub(crate) support_enable: [u8; 0x1],
    pub(crate) pad_21d7: [u8; 0x1],
    pub(crate) field_21d8: [u8; 0x4],
    pub(crate) pad_21dc: [u8; 0x3],
    pub(crate) shared_tail: [u8; 0x8],
    pub(crate) tail_flags: [u8; 0x1],
    pub(crate) pad_21e8: [u8; 0x20],
    pub(crate) enable_2208: [u8; 0x1],
    pub(crate) pad_2209: [u8; 0x3],
    pub(crate) enable_220c: [u8; 0x1],
    pub(crate) pad_220d: [u8; 0x1f],
    pub(crate) field_222c: [u8; 0x4],
    pub(crate) pad_2230: [u8; 0x10],
}

#[versions(AGX_SCHEDULED)]
const FRAGMENT_LAYOUT: () = {
    assert!(core::mem::size_of::<FragmentDescriptor::ver>() == 0x2240);
    assert!(core::mem::align_of::<FragmentDescriptor::ver>() == 1);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, opcode) == 0x0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, counter) == 0x4);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, context) == 0xc);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_0010) == 0x10);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, event_control) == 0x20);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, manager) == 0x28);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, scene) == 0x30);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, empty_scene) == 0x38);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, tilemap) == 0x40);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, multisample_control) == 0x48);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, samples) == 0x50);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, macro_size) == 0x54);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_0058) == 0x58);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, merge_upper_x) == 0x68);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, merge_upper_y) == 0x6c);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_0070) == 0x70);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, tiles) == 0x78);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, counts) == 0x80);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_0086) == 0x86);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, primary_words) == 0x88);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_008a) == 0x8a);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, work_index) == 0x90);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_0094) == 0x94);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, primary) == 0xa0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, primary_address) == 0x7a0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, primary_count) == 0x7a8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_07ac) == 0x7ac);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, partial_store) == 0x7c0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, partial_store_address) == 0xec0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, partial_store_count) == 0xec8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_0ecc) == 0xecc);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, partial_resume) == 0xee0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, partial_resume_address) == 0x15e0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, partial_resume_count) == 0x15e8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_15ec) == 0x15ec);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, partial_load) == 0x1600);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_1700) == 0x1700);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, partial_load_address) == 0x1d00);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, partial_load_count) == 0x1d08);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_1d0c) == 0x1d0c);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, depth_bias_array) == 0x1d20);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_1d28) == 0x1d28);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, scissor_array) == 0x1d30);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_1d38) == 0x1d38);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, occlusion_query_base) == 0x1d40);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_1d48) == 0x1d48);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, load_pipeline_bind) == 0x1e78);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, load_pipeline) == 0x1e80);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_1e88) == 0x1e88);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, partial_load_pipeline_bind) == 0x1ea8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, partial_load_pipeline) == 0x1eb0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, depth_flags) == 0x1eb8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, depth_control) == 0x1ec0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_1ec4) == 0x1ec4);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, depth_buffer) == 0x1ec8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, depth_stride) == 0x1ed0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, depth_aux_stride) == 0x1ed8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, depth_buffer_repeat) == 0x1ee0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, depth_buffer_repeat2) == 0x1ee8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, depth_aux_buffer) == 0x1ef0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, stencil_buffer) == 0x1ef8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, stencil_stride) == 0x1f00);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, stencil_aux_stride) == 0x1f08);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, stencil_buffer_repeat) == 0x1f10);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, stencil_buffer_repeat2) == 0x1f18);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, stencil_aux_buffer) == 0x1f20);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_1f28) == 0x1f28);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, tib_encoding) == 0x1f38);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, aux_fb_flags) == 0x1f40);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, width) == 0x1f48);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, height) == 0x1f4c);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, aux_fb_page_count) == 0x1f50);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, tile_config) == 0x1f58);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_1f60) == 0x1f60);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, store_pipeline_bind) == 0x1f78);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, store_pipeline) == 0x1f7c);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_1f84) == 0x1f84);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, partial_store_pipeline_bind) == 0x1f98);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, partial_store_pipeline) == 0x1f9c);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_1fa4) == 0x1fa4);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, depth_clear_value_bits) == 0x1fa8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, stencil_clear) == 0x1fac);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, sample_size) == 0x1fb0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_1fb4) == 0x1fb4);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, depth_dimensions) == 0x1fd0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_1fd8) == 0x1fd8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, fragment_store_flag) == 0x20fc);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_20fd) == 0x20fd);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, sync_grow) == 0x2100);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_2101) == 0x2101);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, lifecycle_high) == 0x2108);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, sampler_sentinel) == 0x2110);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, sampler_array) == 0x2114);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, sampler_count) == 0x211c);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, sampler_limit) == 0x2120);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, process_empty_tiles) == 0x2124);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_2125) == 0x2125);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, field_2128) == 0x2128);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, multisample) == 0x212c);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_2130) == 0x2130);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, driver_stamp) == 0x2140);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, firmware_stamp) == 0x2148);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, stamp) == 0x2150);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, event) == 0x2154);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_2158) == 0x2158);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, stage_flag) == 0x215c);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, next_work) == 0x2160);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, secondary_stamp) == 0x2164);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, uuid) == 0x2168);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_216c) == 0x216c);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, descriptor_ordinal) == 0x2170);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_2174) == 0x2174);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, timestamp_start) == 0x2198);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, timestamp_end) == 0x21a0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, user_timestamp_start) == 0x21a8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, user_timestamp_end) == 0x21b0);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_21b8) == 0x21b8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, support_opcode) == 0x21cc);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_21cd) == 0x21cd);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, support) == 0x21ce);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, support_enable) == 0x21d6);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_21d7) == 0x21d7);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, field_21d8) == 0x21d8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_21dc) == 0x21dc);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, shared_tail) == 0x21df);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, tail_flags) == 0x21e7);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_21e8) == 0x21e8);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, enable_2208) == 0x2208);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_2209) == 0x2209);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, enable_220c) == 0x220c);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_220d) == 0x220d);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, field_222c) == 0x222c);
    assert!(core::mem::offset_of!(FragmentDescriptor::ver, pad_2230) == 0x2230);
};

#[versions(AGX_SCHEDULED)]
impl TilingDescriptor::ver {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();

    const fn template() -> Self {
        // SAFETY: Every field is an initialized byte array, alignment one.
        let mut raw: Self = unsafe { core::mem::zeroed() };
        raw.sampler_sentinel = u32::MAX.to_le_bytes();
        #[ver(G == G16C)]
        {
            raw.support_opcode = [0x4f];
        }
        #[ver(G == G17P)]
        {
            raw.support_opcode = [0x44];
        }
        raw.field_93c = 1u64.to_le_bytes();
        raw.tail_flags = [1];
        raw
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: repr(C), all byte arrays, no padding, fully initialized.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }
}

#[versions(AGX_SCHEDULED)]
impl Default for TilingDescriptor::ver {
    fn default() -> Self {
        Self::template()
    }
}

#[versions(AGX_SCHEDULED)]
impl FragmentDescriptor::ver {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();

    const fn template() -> Self {
        // SAFETY: Every field is an initialized byte array, alignment one.
        let mut raw: Self = unsafe { core::mem::zeroed() };
        raw.opcode = 1u32.to_le_bytes();
        raw.depth_control = 0x04040404u32.to_le_bytes();
        raw.sampler_sentinel = u32::MAX.to_le_bytes();
        raw.field_2128 = 1u32.to_le_bytes();
        #[ver(G == G16C)]
        {
            raw.support_opcode = [0x54];
        }
        #[ver(G == G17P)]
        {
            raw.support_opcode = [0x53];
        }
        raw.support_enable = [1];
        raw.tail_flags = [1];
        raw.enable_2208 = [1];
        raw.enable_220c = [1];
        raw.field_222c = 0x100u32.to_le_bytes();
        raw
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: repr(C), all byte arrays, no padding, fully initialized.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }
}

#[versions(AGX_SCHEDULED)]
impl Default for FragmentDescriptor::ver {
    fn default() -> Self {
        Self::template()
    }
}

pub(crate) static TILING_TEMPLATE_G16C: TilingDescriptorG16C = TilingDescriptorG16C::template();
pub(crate) static TILING_TEMPLATE_G17P: TilingDescriptorG17P = TilingDescriptorG17P::template();
pub(crate) static FRAGMENT_TEMPLATE_G16C: FragmentDescriptorG16C =
    FragmentDescriptorG16C::template();
pub(crate) static FRAGMENT_TEMPLATE_G17P: FragmentDescriptorG17P =
    FragmentDescriptorG17P::template();

impl TilingDescriptorG17P {
    pub(crate) fn apply_constants(out: &mut [u8]) {
        assert_eq!(out.len(), Self::SIZE);
        // SAFETY: Exclusive initialized staging; alignment one, no padding.
        let raw = unsafe { &mut *out.as_mut_ptr().cast::<Self>() };
        let template = &TILING_TEMPLATE_G17P;
        raw.sampler_sentinel = template.sampler_sentinel;
        raw.support_opcode = template.support_opcode;
        // Neo writes only this byte at its late overlay boundary. Adjacent
        // caller item overrides must retain their original precedence.
        raw.field_93c[0] = template.field_93c[0];
        raw.tail_flags = template.tail_flags;
    }
}

impl FragmentDescriptorG17P {
    pub(crate) fn apply_constants(out: &mut [u8]) {
        assert_eq!(out.len(), Self::SIZE);
        // SAFETY: Exclusive initialized staging; alignment one, no padding.
        let raw = unsafe { &mut *out.as_mut_ptr().cast::<Self>() };
        let template = &FRAGMENT_TEMPLATE_G17P;
        raw.depth_control = template.depth_control;
        raw.sampler_sentinel = template.sampler_sentinel;
        raw.field_2128[0] = template.field_2128[0];
        raw.support_opcode = template.support_opcode;
        raw.support_enable = template.support_enable;
        raw.tail_flags = template.tail_flags;
        raw.enable_2208 = template.enable_2208;
        raw.enable_220c = template.enable_220c;
        raw.field_222c[1] = template.field_222c[1];
    }
}

impl TilingDescriptorG16C {
    pub(crate) fn write(
        out: &mut [u8],
        p: &Parameters,
        s: &State,
        primary: &[Register],
        item: u32,
        grid: u32,
    ) -> Result<(), Error> {
        if out.len() != Self::SIZE || primary.len() < 2 {
            return Err(Error::Invalid);
        }
        let (next, tail) = preflight(p, s, item, grid)?;
        let primary_count = count(primary, 0x700)?;
        let address = s.alias.checked_add(0x60).ok_or(Error::Overflow)?;
        let timestamp_end = match s.timestamp_end {
            Some(value) => value,
            None => s.timestamp.checked_add(8).ok_or(Error::Overflow)?,
        };
        let lifecycle = one(primary, 0x1ca10)?;
        let deflake = narrow(one(primary, 0x10111)?)?;
        out.copy_from_slice(TILING_TEMPLATE_G16C.as_bytes());
        // SAFETY: Validated length, alignment one and all byte-array fields.
        // The borrow is CPU staging only; no firmware-visible memory is cast.
        let raw = unsafe { &mut *out.as_mut_ptr().cast::<Self>() };
        raw.counter = s.counter.to_le_bytes();
        raw.context = s.context.to_le_bytes();
        raw.event_control = s.event_control.to_le_bytes();
        raw.pool = s.pool.to_le_bytes();
        raw.manager = s.manager.to_le_bytes();
        raw.scene = s.scene.to_le_bytes();
        raw.empty_scene = s.empty_scene.to_le_bytes();
        let n = primary.len() as u16;
        for (out, value) in raw.counts.chunks_exact_mut(2).zip([n - 2, n, n]) {
            out.copy_from_slice(&value.to_le_bytes());
        }
        raw.work_index = s.work_index.unwrap_or(item + item / 2).to_le_bytes();
        raw.primary_address = address.to_le_bytes();
        raw.primary_count = primary_count.to_le_bytes();
        raw.tpc = p.tpc.to_le_bytes();
        raw.tpc_size = p.tpc_size.to_le_bytes();
        raw.fragment_event = s.fragment_event.to_le_bytes();
        raw.fragment_stamp = s.fragment_stamp.to_le_bytes();
        raw.next_item = next.to_le_bytes();
        raw.submission_tag = s.tiling_submission_tag.to_le_bytes();
        raw.deflake = deflake.to_le_bytes();
        raw.lifecycle_high = (lifecycle >> 32).to_le_bytes();
        raw.sampler_array = p.sampler_array.to_le_bytes();
        raw.sampler_count = p.sampler_count.to_le_bytes();
        raw.sampler_limit = (if p.sampler_count == 0 {
            0
        } else {
            p.sampler_count + 1
        })
        .to_le_bytes();
        raw.request_growth = [p.request_growth as u8];
        raw.driver_stamp = s.driver_stamp.to_le_bytes();
        raw.firmware_stamp = s.firmware_stamp.to_le_bytes();
        raw.stamp = s.stamp.to_le_bytes();
        raw.event = s.event.to_le_bytes();
        raw.stage_flag = s.stage_flag.to_le_bytes();
        raw.next_work = next.to_le_bytes();
        raw.secondary_stamp = s.secondary_stamp.unwrap_or(s.stamp).to_le_bytes();
        // The source lifecycle-low qword at 0x8ce is completely overwritten
        // by UUID/ordinal. These final fields describe the actual wire bytes.
        raw.uuid = s.uuid.to_le_bytes();
        raw.descriptor_ordinal = s.descriptor_ordinal.unwrap_or(item).to_le_bytes();
        raw.queue_ordinal = s.tiling_queue_ordinal.to_le_bytes();
        raw.timestamp_start = s.timestamp.to_le_bytes();
        raw.timestamp_end = timestamp_end.to_le_bytes();
        raw.user_timestamp_start = s.user_timestamp_start.to_le_bytes();
        raw.user_timestamp_end = s.user_timestamp_end.to_le_bytes();
        raw.support = s.support.to_le_bytes();
        raw.field_93c = s.field_93c.to_le_bytes();
        raw.shared_tail = tail.to_le_bytes();
        raw.tail_flags = [s.tail_flags];
        // Max writes primary programs LAST; Neo's separate semantic builder
        // deliberately overlays protocol fields into its primary program.
        encode(&mut raw.primary, primary);
        Ok(())
    }
}

impl FragmentDescriptorG16C {
    pub(crate) fn write(
        out: &mut [u8],
        p: &Parameters,
        s: &State,
        programs: &Programs<'_>,
        item: u32,
        grid: u32,
    ) -> Result<(), Error> {
        if out.len() != Self::SIZE || programs.primary.len() < 4 {
            return Err(Error::Invalid);
        }
        let (next, tail) = preflight(p, s, item, grid)?;
        let primary = programs.primary;
        let counts = [
            count(primary, 0x700)?,
            count(programs.partial_store, 0x700)?,
            count(programs.partial_resume, 0x700)?,
            count(programs.partial_load, 0x100)?,
        ];
        let mut addresses = [0; 4];
        for (out, offset) in addresses.iter_mut().zip([0xa0, 0x7c0, 0xee0, 0x1600]) {
            *out = s.alias.checked_add(offset).ok_or(Error::Overflow)?;
        }
        let timestamp_start = s.timestamp.checked_add(16).ok_or(Error::Overflow)?;
        let timestamp_end = s.timestamp.checked_add(24).ok_or(Error::Overflow)?;
        let lifecycle = one(primary, 0x160e0)?;
        let tilemap = one(primary, 0x16429)?;
        let multisample = one(primary, 0x10019)?;
        let macro_size = narrow(one(primary, 0x100b1)?)?;
        let merge_x = narrow(one(primary, 0x15131)?)?;
        let merge_y = narrow(one(primary, 0x15139)?)?;
        out.copy_from_slice(FRAGMENT_TEMPLATE_G16C.as_bytes());
        // SAFETY: Validated length, alignment one and all byte-array fields.
        // The borrow is CPU staging only; no firmware-visible memory is cast.
        let raw = unsafe { &mut *out.as_mut_ptr().cast::<Self>() };
        raw.counter = s.counter.to_le_bytes();
        raw.context = s.context.to_le_bytes();
        raw.event_control = s.event_control.to_le_bytes();
        raw.manager = s.manager.to_le_bytes();
        raw.scene = s.scene.to_le_bytes();
        raw.empty_scene = s.empty_scene.to_le_bytes();
        raw.tilemap = tilemap.to_le_bytes();
        raw.multisample_control = multisample.to_le_bytes();
        raw.samples = p.samples.to_le_bytes();
        raw.macro_size = macro_size.to_le_bytes();
        raw.merge_upper_x = merge_x.to_le_bytes();
        raw.merge_upper_y = merge_y.to_le_bytes();
        raw.tiles = p.tiles.to_le_bytes();
        let n = primary.len() as u16;
        for (out, value) in raw.counts.chunks_exact_mut(2).zip([n - 4, n - 3, n - 3]) {
            out.copy_from_slice(&value.to_le_bytes());
        }
        raw.primary_words = n.to_le_bytes();
        raw.work_index = s.work_index.unwrap_or(next).to_le_bytes();
        raw.primary_address = addresses[0].to_le_bytes();
        raw.primary_count = counts[0].to_le_bytes();
        raw.partial_store_address = addresses[1].to_le_bytes();
        raw.partial_store_count = counts[1].to_le_bytes();
        raw.partial_resume_address = addresses[2].to_le_bytes();
        raw.partial_resume_count = counts[2].to_le_bytes();
        raw.partial_load_address = addresses[3].to_le_bytes();
        raw.partial_load_count = counts[3].to_le_bytes();
        encode(&mut raw.partial_store, programs.partial_store);
        encode(&mut raw.partial_resume, programs.partial_resume);
        encode(&mut raw.partial_load, programs.partial_load);
        raw.depth_bias_array = p.depth_bias_array.to_le_bytes();
        raw.scissor_array = p.scissor_array.to_le_bytes();
        raw.occlusion_query_base = p.occlusion_query_base.to_le_bytes();
        raw.load_pipeline_bind = p.load_pipeline_bind.to_le_bytes();
        raw.load_pipeline = p.load_pipeline.to_le_bytes();
        raw.partial_load_pipeline_bind = p.partial_load_pipeline_bind.to_le_bytes();
        raw.partial_load_pipeline = p.partial_load_pipeline.to_le_bytes();
        raw.depth_flags = p.depth_flags.to_le_bytes();
        raw.depth_buffer = p.depth_buffer.to_le_bytes();
        raw.depth_buffer_repeat = p.depth_buffer.to_le_bytes();
        raw.depth_buffer_repeat2 = p.depth_buffer.to_le_bytes();
        raw.depth_stride = p.depth_stride.to_le_bytes();
        raw.depth_aux_stride = p.depth_aux_stride.to_le_bytes();
        raw.depth_aux_buffer = p.depth_aux_buffer.to_le_bytes();
        raw.stencil_buffer = p.stencil_buffer.to_le_bytes();
        raw.stencil_buffer_repeat = p.stencil_buffer.to_le_bytes();
        raw.stencil_buffer_repeat2 = p.stencil_buffer.to_le_bytes();
        raw.stencil_stride = p.stencil_stride.to_le_bytes();
        raw.stencil_aux_stride = p.stencil_aux_stride.to_le_bytes();
        raw.stencil_aux_buffer = p.stencil_aux_buffer.to_le_bytes();
        raw.tib_encoding = p.tib_encoding.to_le_bytes();
        raw.aux_fb_flags = p.aux_fb_flags.to_le_bytes();
        // The earlier source context_base qword at 0x1f4e is fully overwritten
        // by height and aux_fb_page_count. It is not an independent wire field.
        raw.width = p.width.to_le_bytes();
        raw.height = p.height.to_le_bytes();
        raw.aux_fb_page_count = p.aux_fb_page_count.to_le_bytes();
        raw.tile_config = p.tile_config.to_le_bytes();
        raw.store_pipeline_bind = p.store_pipeline_bind.to_le_bytes();
        raw.store_pipeline = p.store_pipeline.to_le_bytes();
        raw.partial_store_pipeline_bind = p.partial_store_pipeline_bind.to_le_bytes();
        raw.partial_store_pipeline = p.partial_store_pipeline.to_le_bytes();
        raw.depth_clear_value_bits = p.depth_clear_value_bits.to_le_bytes();
        raw.stencil_clear = p.stencil_clear.to_le_bytes();
        raw.sample_size = p.sample_size.to_le_bytes();
        raw.depth_dimensions = p.depth_dimensions.to_le_bytes();
        raw.sync_grow = [p.sync_grow as u8];
        raw.lifecycle_high = (lifecycle >> 32).to_le_bytes();
        raw.sampler_array = p.sampler_array.to_le_bytes();
        raw.sampler_count = p.sampler_count.to_le_bytes();
        raw.sampler_limit = (if p.sampler_count == 0 {
            0
        } else {
            p.sampler_count + 1
        })
        .to_le_bytes();
        raw.process_empty_tiles = [p.process_empty_tiles as u8];
        raw.field_2128 = s.field_2128.to_le_bytes();
        raw.multisample = u32::from(p.samples > 1).to_le_bytes();
        raw.driver_stamp = s.driver_stamp.to_le_bytes();
        raw.firmware_stamp = s.firmware_stamp.to_le_bytes();
        raw.stamp = s.stamp.to_le_bytes();
        raw.event = s.event.to_le_bytes();
        raw.stage_flag = s.stage_flag.to_le_bytes();
        raw.next_work = next.to_le_bytes();
        raw.secondary_stamp = s.secondary_stamp.unwrap_or(s.stamp).to_le_bytes();
        raw.uuid = s.uuid.to_le_bytes();
        raw.descriptor_ordinal = s.descriptor_ordinal.unwrap_or(item).to_le_bytes();
        raw.timestamp_start = timestamp_start.to_le_bytes();
        raw.timestamp_end = timestamp_end.to_le_bytes();
        raw.user_timestamp_start = s.user_timestamp_start.to_le_bytes();
        raw.user_timestamp_end = s.user_timestamp_end.to_le_bytes();
        raw.support = s.support.to_le_bytes();
        raw.field_21d8 = s.field_21d8.to_le_bytes();
        raw.shared_tail = tail.to_le_bytes();
        raw.tail_flags = [s.tail_flags];
        raw.field_222c = s.field_222c.to_le_bytes();
        encode(&mut raw.primary, primary);
        Ok(())
    }
}
