// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Source port of g17p_render.py and G17PWorkBuilder descriptor bodies.
//! Ordered register writes (including duplicates) are firmware programs.
//! All firmware buffers are supplied by the caller; no captured templates.

use super::g17p_compute::{add, clear, u32_at, u64_at, Error, Register, Result, USC_EXEC_BASE};

pub(crate) const TA_SIZE: usize = 0x9c0;
pub(crate) const FRAGMENT_SIZE: usize = 0x2240;
pub(crate) const SUPPORT_SIZE: usize = 0x70;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Parameters {
    pub(crate) width: u64,
    pub(crate) height: u64,
    pub(crate) context_base: u64,
    pub(crate) tilemap: u64,
    pub(crate) heapmeta: u64,
    pub(crate) tpc: u64,
    pub(crate) deflake_1: u64,
    pub(crate) deflake_2: u64,
    pub(crate) deflake_3: u64,
    pub(crate) encoder: u64,
    pub(crate) ta_status: u64,
    pub(crate) store_pipeline_bind: u64,
    pub(crate) store_pipeline: u64,
    pub(crate) load_pipeline_bind: u64,
    pub(crate) load_pipeline: u64,
    pub(crate) scissor_array: u64,
    pub(crate) depth_bias_array: u64,
    pub(crate) aux_fb: u64,
    pub(crate) fragment_status: u64,
    pub(crate) layers: u64,
    pub(crate) utile_width: u64,
    pub(crate) utile_height: u64,
    pub(crate) samples: u64,
    pub(crate) sample_size: u64,
    pub(crate) occlusion_query_base: u64,
    pub(crate) depth_stride: u64,
    pub(crate) stencil_stride: u64,
    pub(crate) depth_aux_stride: u64,
    pub(crate) stencil_aux_stride: u64,
    pub(crate) merge_upper_x_bits: u64,
    pub(crate) merge_upper_y_bits: u64,
    pub(crate) partial_load_pipeline_bind: u64,
    pub(crate) partial_load_pipeline: u64,
    pub(crate) partial_store_pipeline_bind: u64,
    pub(crate) partial_store_pipeline: u64,
    pub(crate) sampler_array: u64,
    pub(crate) sampler_count: u64,
    pub(crate) process_empty_tiles: bool,
    pub(crate) fragment_sync_grow: Option<bool>,
    pub(crate) reactive_tvb_growth: bool,
    pub(crate) tvb_pool_id: Option<u64>,
    pub(crate) emit_uapi_fields: bool,
    pub(crate) vertex_store_flag: bool,
    pub(crate) fragment_store_flag: bool,
    pub(crate) usc_exec_base: u64,
    pub(crate) timestamp_a: u64,
    pub(crate) timestamp_b: u64,
    pub(crate) ta_timestamp_end: u64,
    pub(crate) fragment_timestamp_start: u64,
    pub(crate) fragment_timestamp_end: u64,
    pub(crate) ta_user_timestamp_start: u64,
    pub(crate) ta_user_timestamp_end: u64,
    pub(crate) fragment_user_timestamp_start: u64,
    pub(crate) fragment_user_timestamp_end: u64,
    pub(crate) depth_buffer: u64,
    pub(crate) stencil_buffer: u64,
    pub(crate) depth_aux_buffer: u64,
    pub(crate) stencil_aux_buffer: u64,
    pub(crate) depth_clear_value_bits: u64,
    pub(crate) stencil_clear_value: u64,
    pub(crate) depth_flags: u64,
    pub(crate) depth_dimensions: u64,
    pub(crate) utile_config: u64,
    pub(crate) multisample_control: u64,
    pub(crate) ppp_control: u64,
    pub(crate) tib_blocks: u64,
    pub(crate) tile_config: u64,
    pub(crate) aux_fb_flags: u64,
    pub(crate) aux_fb_page_count: u64,
    pub(crate) lifecycle_ordinal: u64,
    pub(crate) native_context_slot: Option<u64>,
    pub(crate) queue_pair: u64,
    pub(crate) queue_item_index: u64,
    pub(crate) status_queue_pair: Option<u64>,
    pub(crate) status_item_index: Option<u64>,
    pub(crate) native_cycle_registers: bool,
    pub(crate) pair_resource_stride: u64,
    pub(crate) native_record_index_register: bool,
    pub(crate) native_pair_registers: bool,
    pub(crate) native_status_registers: bool,
    pub(crate) local_item_registers: bool,
    pub(crate) native_item_fields: bool,
}

impl Default for Parameters {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            context_base: 0,
            tilemap: 0,
            heapmeta: 0,
            tpc: 0,
            deflake_1: 0,
            deflake_2: 0,
            deflake_3: 0,
            encoder: 0,
            ta_status: 0,
            store_pipeline_bind: 0,
            store_pipeline: 0,
            load_pipeline_bind: 0,
            load_pipeline: 0,
            scissor_array: 0,
            depth_bias_array: 0,
            aux_fb: 0,
            fragment_status: 0,
            layers: 1,
            utile_width: 32,
            utile_height: 32,
            samples: 1,
            sample_size: 0,
            occlusion_query_base: 0,
            depth_stride: 0,
            stencil_stride: 0,
            depth_aux_stride: 0,
            stencil_aux_stride: 0,
            merge_upper_x_bits: 0,
            merge_upper_y_bits: 0,
            partial_load_pipeline_bind: 0,
            partial_load_pipeline: 0,
            partial_store_pipeline_bind: 0,
            partial_store_pipeline: 0,
            sampler_array: 0,
            sampler_count: 0,
            process_empty_tiles: true,
            fragment_sync_grow: None,
            reactive_tvb_growth: false,
            tvb_pool_id: None,
            emit_uapi_fields: false,
            vertex_store_flag: false,
            fragment_store_flag: false,
            usc_exec_base: 1099511627776,
            timestamp_a: 0xfffffc2000024c68,
            timestamp_b: 0xfffffc2000024c70,
            ta_timestamp_end: 0,
            fragment_timestamp_start: 0xfffffc2000024c68,
            fragment_timestamp_end: 0xfffffc2000024c70,
            ta_user_timestamp_start: 0,
            ta_user_timestamp_end: 0,
            fragment_user_timestamp_start: 0,
            fragment_user_timestamp_end: 0,
            depth_buffer: 0,
            stencil_buffer: 0,
            depth_aux_buffer: 0,
            stencil_aux_buffer: 0,
            depth_clear_value_bits: 1065353216,
            stencil_clear_value: 0,
            depth_flags: 0,
            depth_dimensions: 0,
            utile_config: 40960,
            multisample_control: 136,
            ppp_control: 514,
            tib_blocks: 8,
            tile_config: 66176,
            aux_fb_flags: 49153,
            aux_fb_page_count: 1048576,
            lifecycle_ordinal: 0,
            native_context_slot: None,
            queue_pair: 0,
            queue_item_index: 0,
            status_queue_pair: None,
            status_item_index: None,
            native_cycle_registers: false,
            pair_resource_stride: 6160384,
            native_record_index_register: false,
            native_pair_registers: false,
            native_status_registers: false,
            local_item_registers: false,
            native_item_fields: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Tiling,
    Fragment,
}
impl Kind {
    pub(crate) fn size(self) -> usize {
        match self {
            Self::Tiling => TA_SIZE,
            Self::Fragment => FRAGMENT_SIZE,
        }
    }
    fn index(self) -> u32 {
        match self {
            Self::Tiling => 0,
            Self::Fragment => 1,
        }
    }
}

struct Geometry {
    tiles_x: u64,
    tiles_y: u64,
    size1: u64,
    size2: u64,
    size3: u64,
    x_blocks: u64,
    y_blocks: u64,
    screen: u64,
    pixels: u64,
    macro_size: u64,
}

impl Parameters {
    pub(crate) fn validate(&self) -> Result {
        // Check this first, before deriving any address or modifying output.
        if self.usc_exec_base != USC_EXEC_BASE {
            return Err(Error::UnsupportedExecBase);
        }
        // Bounds are the existing g17p_modern.py userspace contract.
        if !(1..=16384).contains(&self.width)
            || !(1..=16384).contains(&self.height)
            || !(1..=2048).contains(&self.layers)
            || !matches!(
                (self.utile_width, self.utile_height),
                (32, 32) | (32, 16) | (16, 16)
            )
            || !matches!(self.samples, 1 | 2 | 4)
            || self.queue_pair > 3
            || self.status_queue_pair.is_some_and(|v| v > 3)
            || self
                .native_context_slot
                .is_some_and(|v| !matches!(v, 1 | 2))
            || self.tvb_pool_id.is_some_and(|v| v > 1)
            || self.lifecycle_ordinal > u32::MAX as u64 * 2 / 3
            || self.sampler_count >= u32::MAX as u64
            || (self.sampler_array == 0) != (self.sampler_count == 0)
            || self.sampler_array & 7 != 0
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    fn ta_offset(&self, address: u64) -> Result<u64> {
        address.checked_sub(self.context_base).ok_or(Error::Invalid)
    }
    fn geometry(&self) -> Result<Geometry> {
        self.validate()?;
        let (tx, ty) = ((self.width + 31) / 32, (self.height + 31) / 32);
        let (ux, uy) = (32 / self.utile_width, 32 / self.utile_height);
        let (mx, my) = (((tx + 3) / 4 + 3) & !3, ((ty + 3) / 4 + 3) & !3);
        Ok(Geometry {
            tiles_x: tx,
            tiles_y: ty,
            size1: (5 * mx * my * ux * uy + 3) / 4,
            size2: mx * my,
            size3: 2 * mx * my * ux * uy,
            x_blocks: 3 * mx | ((2 * mx) << 9) | (mx << 18),
            y_blocks: 3 * my | ((2 * my) << 9) | (my << 18),
            screen: ((ty - 1) << 12) | (tx - 1),
            pixels: (self.width - 1) | ((self.height - 1) << 16),
            macro_size: my * uy | ((mx * ux) << 16),
        })
    }
    fn work_stamp(&self) -> u64 {
        (self.native_context_slot.unwrap_or(1) << 8)
            | (self.lifecycle_ordinal + self.lifecycle_ordinal / 2)
    }
    fn cycle(&self) -> Result<u64> {
        let pair =
            self.native_item_fields || self.native_pair_registers || self.native_cycle_registers;
        let mut value = 0x178020;
        if pair {
            value = add(
                value,
                self.queue_pair
                    .checked_mul(self.pair_resource_stride)
                    .ok_or(Error::Overflow)?,
            )?;
        }
        if pair || self.local_item_registers {
            value = add(
                value,
                self.queue_item_index
                    .checked_mul(0x20)
                    .ok_or(Error::Overflow)?,
            )?;
        }
        Ok(value)
    }
    fn record_index(&self) -> Result<u64> {
        let pair = self.native_item_fields
            || self.native_pair_registers
            || self.native_record_index_register;
        let mut value = 0x80005;
        if pair {
            value = add(value, self.queue_pair * 0x140)?;
        }
        if pair || self.local_item_registers {
            value = add(
                value,
                self.queue_item_index
                    .checked_mul(4)
                    .ok_or(Error::Overflow)?,
            )?;
        }
        Ok(value)
    }
    fn status(&self, kind: Kind) -> Result<u64> {
        let bases = match kind {
            Kind::Tiling => [0x1000078000, 0x1000660000, 0x1000c40000, 0x1000078000],
            Kind::Fragment => [0x10001a8000, 0x1000788000, 0x1000d68000, 0x10001a8000],
        };
        let base = if self.native_item_fields || self.native_status_registers {
            bases[self.status_queue_pair.unwrap_or(self.queue_pair) as usize]
        } else {
            match kind {
                Kind::Tiling => self.ta_status,
                Kind::Fragment => self.fragment_status,
            }
        };
        add(
            base,
            self.status_item_index
                .unwrap_or(self.queue_item_index)
                .checked_mul(0x40)
                .ok_or(Error::Overflow)?,
        )
    }
}

/// Round the source Python constant 1.732051 / dimension to binary32 without
/// using floating point in the kernel. The binary64 constant is exactly
/// 7800464238186695 / 2^52. All admitted dimensions are checked by the oracle.
fn merge_bits(dimension: u64) -> u64 {
    const NUMERATOR: u64 = 7800464238186695;
    let log = 63 - dimension.leading_zeros() as i32;
    let mut exponent = -log;
    if dimension > (NUMERATOR >> (52 - log)) {
        exponent -= 1;
    }
    let denominator = dimension << (29 + exponent);
    let mut mantissa = NUMERATOR / denominator;
    let remainder = NUMERATOR % denominator;
    if remainder * 2 > denominator || (remainder * 2 == denominator && mantissa & 1 != 0) {
        mantissa += 1;
    }
    if mantissa == 1 << 24 {
        mantissa >>= 1;
        exponent += 1;
    }
    ((exponent + 127) as u64) << 23 | (mantissa & 0x7fffff)
}

pub(crate) fn tiling_registers(p: &Parameters) -> Result<[Register; 73]> {
    let g = p.geometry()?;
    let tilemap = p.ta_offset(p.tilemap)?;
    let heapmeta = p.ta_offset(p.heapmeta)?;
    let deflake_1 = p.ta_offset(p.deflake_1)?;
    let lifecycle = 0;
    let cycle = p.cycle()?;
    let record_index = p.record_index()?;
    let status = p.status(Kind::Tiling)?;
    let work_stamp = p.work_stamp();
    Ok([
        (0x1748, 0x1),
        (0x10141, 0x200),
        (0x1c039, tilemap),
        (0x1c9c8, tilemap),
        (0x1c0a1, p.ta_offset(p.tpc)?),
        (0x1c031, (heapmeta | 0x8000000000000000)),
        (0x1c9c0, (heapmeta | 0x8000000000000000)),
        (0x1c051, 0x3a0012006b0003),
        (0x1c061, 0x1),
        (0x10149, p.utile_config),
        (0x10139, p.multisample_control),
        (0x10111, deflake_1),
        (0x1c9b0, deflake_1),
        (0x10119, p.ta_offset(p.deflake_2)?),
        (0x1c9b8, p.ta_offset(p.deflake_2)?),
        (0x1c958, 0x1),
        (0x1c950, (p.ta_offset(p.deflake_3)? | 0x4000000000000)),
        (0x1c930, 0x0),
        (0x1c880, p.ta_offset(p.encoder)?),
        (0x1c079, heapmeta),
        (0x1c9d8, heapmeta),
        (0x10151, 0x0),
        (0x1c199, 0x0),
        (0x1c1a1, 0x0),
        (0x1c1a9, 0x0),
        (0x1c1b1, 0x0),
        (0x1c1b9, 0x0),
        (0x1c8f8, 0x8860),
        (0x1c0b1, g.size1),
        (0x1c850, g.size1),
        (0x10131, p.multisample_control),
        (0x10121, p.ppp_control),
        (0x10129, g.pixels),
        (0x101b9, g.screen),
        (0x1c069, g.x_blocks),
        (0x1c071, g.y_blocks),
        (0x1c081, g.size2),
        (0x1c0a9, g.size3),
        (0x10171, 0x100),
        (
            0x10169,
            if p.layers > 0x1 {
                0xe000 | (p.layers - 0x1)
            } else {
                0x8000
            },
        ),
        (0xa309, 0x0),
        (0x1c8e0, 0xffffffffffffffff),
        (0x1c8e8, 0xffffffffffffffff),
        (0x1c898, 0x0),
        (0x101e1, 0x1c),
        (0x1c9e8, 0x0),
        (0x1a099, 0x0),
        (0x1a0a1, 0x0),
        (0x1a069, 0x0),
        (0x1a071, 0x0),
        (0x1a0c9, 0x0),
        (0x1a0d1, 0x0),
        (0x101c9, 0x0),
        (0xd471, 0x0),
        (0x1a0f1, 0x8),
        (0x10799, 0xff0000),
        (0x1c830, 0x0),
        (0x1ca30, cycle),
        (0x16c39, cycle),
        (0x1c910, record_index),
        (0xa5a1, 0xfe00400020),
        (0xd419, 0x200000001),
        (0x1ca10, lifecycle),
        (0x14a1, lifecycle),
        (0xa349, lifecycle),
        (0x10209, work_stamp),
        (0x1c9f0, work_stamp),
        (0x14320, work_stamp),
        (0x14308, 0x0),
        (0x14318, (status | 0x1)),
        (0x1740, 0x1),
        (0x1c880, deflake_1),
        (0x1c898, 0x1),
    ])
}

pub(crate) fn fragment_registers(p: &Parameters) -> Result<[Register; 89]> {
    let g = p.geometry()?;
    let lifecycle = 0;
    let cycle = p.cycle()?;
    let status = p.status(Kind::Fragment)?;
    let work_stamp = p.work_stamp();
    let tile_state = 0x3717f
        | ((g.tiles_x - 1) << 44)
        | ((g.tiles_y - 1) << 53)
        | 0x2000000000
        | if p.layers > 1 { 0x100000000 } else { 0 }
        | ((p.utile_config & 0xf000) << 28);
    let merge_upper_x = if p.emit_uapi_fields {
        p.merge_upper_x_bits
    } else {
        merge_bits(p.width)
    };
    let merge_upper_y = if p.emit_uapi_fields {
        p.merge_upper_y_bits
    } else {
        merge_bits(p.height)
    };
    Ok([
        (0x1739, 0x1),
        (0x10009, p.utile_config),
        (0x15379, p.store_pipeline_bind),
        (0x15381, p.store_pipeline),
        (0x15369, p.load_pipeline_bind),
        (0x15371, p.load_pipeline),
        (0x15131, merge_upper_x),
        (0x15139, merge_upper_y),
        (0x100a1, 0x0),
        (0x15069, 0x0),
        (0x15071, 0x0),
        (0x16058, 0x0),
        (0x10019, p.multisample_control),
        (0x100b1, g.macro_size),
        (0x16030, g.macro_size),
        (0x100d9, g.screen),
        (0xa301, 0x0),
        (0x10791, 0xff0200),
        (0x16098, p.heapmeta),
        (0x15109, p.scissor_array),
        (0x15101, p.depth_bias_array),
        (0x15021, p.aux_fb_flags),
        (0x15211, ((p.height << 0x20) | p.width)),
        (0x15049, p.aux_fb_page_count),
        (0x10051, p.tib_blocks),
        (0x15321, p.depth_dimensions),
        (0x15301, p.depth_clear_value_bits),
        (0x15309, (p.stencil_clear_value | 0x300)),
        (0x15311, p.occlusion_query_base),
        (0x15319, p.depth_flags),
        (0x15349, 0x4040404),
        (0x15351, 0x0),
        (0x15329, p.depth_buffer),
        (0x15331, p.depth_buffer),
        (0x15339, p.stencil_buffer),
        (0x15341, p.stencil_buffer),
        (0x15231, 0x0),
        (0x15221, 0x0),
        (0x15239, 0x0),
        (0x15229, 0x0),
        (0x15401, p.depth_stride),
        (0x15421, p.depth_stride),
        (0x15409, p.stencil_stride),
        (0x15429, p.stencil_stride),
        (0x153c1, p.depth_aux_buffer),
        (0x15411, p.depth_aux_stride),
        (0x153c9, p.depth_aux_buffer),
        (0x15431, p.depth_aux_stride),
        (0x153d1, p.stencil_aux_buffer),
        (0x15419, p.stencil_aux_stride),
        (0x153d9, p.stencil_aux_buffer),
        (0x15439, p.stencil_aux_stride),
        (0x16429, p.tilemap),
        (0x16060, p.heapmeta),
        (0x16431, ((0x4 * g.size1) << 0x18)),
        (0x10039, p.tile_config),
        (0x16020, 0x0),
        (0x16451, 0x0),
        (0x15359, 0x0),
        (0x100b8, 0x8860),
        (0x16461, p.aux_fb),
        (0x16090, p.aux_fb),
        (0x101e9, 0x1c),
        (0x160a8, 0x0),
        (0x16068, tile_state),
        (0x1a0a9, 0x0),
        (0x1a0b1, 0x0),
        (0x1a079, 0x0),
        (0x1a081, 0x0),
        (0x1a0d9, 0x0),
        (0x1a0e1, 0x0),
        (0x101c1, 0x0),
        (0xd469, 0x0),
        (0x1a0f9, 0x8),
        (0xa5a9, 0x14600400020),
        (0xd429, 0x200000001),
        (0x160e0, lifecycle),
        (0x1499, lifecycle),
        (0xa341, lifecycle),
        (0x1c838, 0x0),
        (0x1ca28, cycle),
        (0x10211, work_stamp),
        (0x10420, work_stamp),
        (0x14048, 0x0),
        (0x14080, (status | 0x1)),
        (0x1731, 0x1),
        (0x16020, 0x1),
        (0x16020, 0x0),
        (0x16068, 0x40000),
    ])
}

pub(crate) fn partial_store_registers(p: &Parameters) -> Result<[Register; 16]> {
    p.validate()?;
    Ok([
        (0x15379, p.partial_store_pipeline_bind),
        (0x15381, p.partial_store_pipeline),
        (0x10039, p.tile_config),
        (0x15359, 0x20),
        (0x15331, p.depth_buffer),
        (0x153c9, p.depth_aux_buffer),
        (0x15341, p.stencil_buffer),
        (0x153d9, p.stencil_aux_buffer),
        (0x15421, p.depth_stride),
        (0x15431, p.depth_aux_stride),
        (0x15429, p.stencil_stride),
        (0x15439, p.stencil_aux_stride),
        (0x15221, 0x0),
        (0x15229, 0x0),
        (0x15319, p.depth_flags),
        (0x15349, 0x4040404),
    ])
}

pub(crate) fn partial_resume_registers(p: &Parameters) -> Result<[Register; 23]> {
    p.validate()?;
    Ok([
        (0x15379, p.partial_store_pipeline_bind),
        (0x15381, p.partial_store_pipeline),
        (0x15369, p.partial_load_pipeline_bind),
        (0x15371, p.partial_load_pipeline),
        (0x10039, (p.tile_config & 0xffff)),
        (0x15359, 0x20),
        (0x15331, p.depth_buffer),
        (0x153c9, p.depth_aux_buffer),
        (0x15341, p.stencil_buffer),
        (0x153d9, p.stencil_aux_buffer),
        (0x15421, p.depth_stride),
        (0x15431, p.depth_aux_stride),
        (0x15429, p.stencil_stride),
        (0x15439, p.stencil_aux_stride),
        (0x15221, 0x0),
        (0x15229, 0x0),
        (0x15309, (p.stencil_clear_value | 0x300)),
        (0x15329, p.depth_buffer),
        (0x153c1, p.depth_aux_buffer),
        (0x15339, p.stencil_buffer),
        (0x153d1, p.stencil_aux_buffer),
        (0x15319, p.depth_flags),
        (0x15349, 0x4040404),
    ])
}

pub(crate) fn partial_load_registers(p: &Parameters) -> Result<[Register; 10]> {
    p.validate()?;
    Ok([
        (0x15369, p.partial_load_pipeline_bind),
        (0x15371, p.partial_load_pipeline),
        (0x10039, (p.tile_config & 0xffff)),
        (0x15309, (p.stencil_clear_value | 0x300)),
        (0x15329, p.depth_buffer),
        (0x153c1, p.depth_aux_buffer),
        (0x15339, p.stencil_buffer),
        (0x153d1, p.stencil_aux_buffer),
        (0x15319, p.depth_flags),
        (0x15349, 0x4040404),
    ])
}

fn encode_registers(out: &mut [u8], registers: &[Register]) {
    for (entry, &(number, value)) in out.chunks_exact_mut(12).zip(registers) {
        u32_at(entry, 0, number);
        u64_at(entry, 4, value);
    }
}
fn last(registers: &[Register], number: u32) -> Option<u64> {
    registers.iter().rev().find(|r| r.0 == number).map(|r| r.1)
}
fn narrow(value: u64) -> Result<u32> {
    value.try_into().map_err(|_| Error::Overflow)
}

const CLASS4_REGISTERS: [u32; 32] = [
    0x15401, 0x15421, 0x15409, 0x15429, 0x153c1, 0x15411, 0x153c9, 0x15431, 0x153d1, 0x15419,
    0x153d9, 0x15439, 0x16429, 0x16060, 0x16431, 0x10039, 0x16020, 0x16451, 0x15359, 0x100b8,
    0x16461, 0x16090, 0x101e9, 0x160a8, 0x16068, 0x1a0a9, 0x1a0b1, 0x1a079, 0x1a081, 0x1a0d9,
    0x1a0e1, 0x101c1,
];
pub(crate) fn class4_program(out: &mut [u8], registers: &[Register]) -> Result {
    if out.len() != 32 * 12 {
        return Err(Error::Invalid);
    }
    let selected = registers
        .windows(32)
        .find(|r| r.iter().zip(CLASS4_REGISTERS).all(|(r, n)| r.0 == n))
        .ok_or(Error::MissingRegister)?;
    encode_registers(out, selected);
    Ok(())
}
pub(crate) fn class2_prestate(
    out: &mut [u8],
    operands: u64,
    firmware: u64,
    low_object: u64,
    context: u32,
) -> Result {
    if operands >> 32 != low_object >> 32 {
        return Err(Error::Invalid);
    }
    clear(out, SUPPORT_SIZE)?;
    support_fields(out, operands, firmware, context, false);
    u32_at(out, 0, 2);
    u32_at(out, 0x14, low_object as u32);
    u64_at(out, 0x20, 0x0000170000000000);
    u64_at(out, 0x28, 0x0000170000000000);
    u32_at(out, 0x48, 0xb8);
    Ok(())
}
pub(crate) fn class4_state(
    out: &mut [u8],
    operands: u64,
    firmware: u64,
    context: u32,
    active: bool,
) -> Result {
    clear(out, SUPPORT_SIZE)?;
    support_fields(out, operands, firmware, context, active);
    Ok(())
}
fn support_fields(out: &mut [u8], operands: u64, firmware: u64, context: u32, active: bool) {
    for (at, value) in [
        (0, 6),
        (8, context),
        (0x10, 2),
        (0x40, 4),
        (0x44, active as u32),
        (0x48, 0xb0),
        (0x60, 3),
    ] {
        u32_at(out, at, value);
    }
    for (at, value) in [
        (0x18, 0x0004000000000070),
        (0x20, 0x0000160000000000),
        (0x28, 0x0000160000000000),
        (0x30, operands),
        (0x4c, firmware),
    ] {
        u64_at(out, at, value);
    }
}
pub(crate) fn aux_fb(out: &mut [u8]) -> Result {
    clear(out, 0x4000)?;
    u32_at(out, 0x480, 0x60000000);
    u32_at(out, 0x484, 0x35b);
    Ok(())
}

/// Addresses are owned by the runtime. Explicit record indices let the paired
/// allocator wrap only after it has proved both physical records reusable.
/// Tail and item overrides correspond to the Python builder's named maps.
pub(crate) struct Descriptor<'a> {
    pub(crate) kind: Kind,
    pub(crate) index: u32,
    pub(crate) sequence: u64,
    pub(crate) ordinal: u32,
    pub(crate) context: u32,
    pub(crate) queue_pair: u32,
    pub(crate) pool_bases: [u64; 2],
    pub(crate) record_indices: [u32; 2],
    pub(crate) shared: [u64; 2],
    pub(crate) low_alias: Option<u64>,
    pub(crate) status_base: Option<u64>,
    pub(crate) grid: Option<u32>,
    pub(crate) write_tail: bool,
    pub(crate) write_lifecycle: bool,
    pub(crate) write_item: bool,
    pub(crate) write_structural: bool,
    pub(crate) pointer_overrides: &'a [(usize, u64)],
    pub(crate) item_overrides: &'a [(usize, u32)],
}
impl Descriptor<'_> {
    pub(crate) fn build(
        &self,
        out: &mut [u8],
        registers: &[Register],
        parameters: Option<&Parameters>,
    ) -> Result {
        if let Some(p) = parameters {
            p.validate()?;
        }
        if out.len() != self.kind.size() || registers.len() > 128 || self.queue_pair > 3 {
            return Err(Error::Invalid);
        }
        let native_slot = parameters.and_then(|p| p.native_context_slot);
        if native_slot.is_some_and(|slot| !matches!((self.context, slot), (1, 1) | (1, 2) | (2, 2)))
        {
            return Err(Error::Invalid);
        }
        let work = self
            .ordinal
            .checked_add(self.ordinal / 2)
            .ok_or(Error::Overflow)?;
        let stamp = narrow((native_slot.unwrap_or(self.context.max(1) as u64) << 8) + work as u64)?;
        let pool = parameters
            .and_then(|p| p.tvb_pool_id)
            .unwrap_or(self.queue_pair as u64) as u32;
        let objects = [
            add(
                self.pool_bases[0],
                (self.record_indices[0] % 35) as u64 * 0x100,
            )?,
            self.shared[0],
            add(
                self.pool_bases[1],
                (self.record_indices[1] % 79) as u64 * 0x80,
            )?,
            self.shared[1],
        ];
        let item = self.index.checked_add(1).ok_or(Error::Overflow)?;
        let grid = self.grid.unwrap_or(self.queue_pair * 2 + self.kind.index());
        let structural = self.write_tail && self.write_structural && parameters.is_some();
        let lifecycle = if self.write_tail && self.write_lifecycle {
            last(
                registers,
                if self.kind == Kind::Tiling {
                    0x1ca10
                } else {
                    0x160e0
                },
            )
            .ok_or(Error::MissingRegister)?
        } else {
            0
        };
        let deflake = if structural && self.kind == Kind::Tiling {
            last(registers, 0x10111).ok_or(Error::MissingRegister)?
        } else {
            0
        };
        if self.write_tail && self.write_item {
            let max = if self.kind == Kind::Tiling {
                0xff
            } else {
                0xffffff
            };
            if item > max
                || grid == u32::MAX
                || self
                    .item_overrides
                    .iter()
                    .any(|&(at, _)| at > out.len() - 4)
            {
                return Err(Error::Invalid);
            }
        }
        // Preflight the narrow structural fields before touching caller bytes.
        if structural {
            let p = parameters.unwrap();
            for v in [
                p.width,
                p.height,
                p.store_pipeline_bind,
                p.partial_store_pipeline_bind,
                p.depth_clear_value_bits,
            ] {
                narrow(v)?;
            }
            if p.tib_blocks > u64::MAX >> 33 {
                return Err(Error::Overflow);
            }
        }
        let mut pointers = [(0usize, 0u64); 11];
        let count = if self.write_tail {
            self.tail_pointers(&mut pointers)?
        } else {
            0
        };
        clear(out, self.kind.size())?;
        u32_at(out, 0, self.kind.index());
        u64_at(out, 4, self.sequence);
        u32_at(out, 0xc, self.context);
        let (po, ro) = if self.kind == Kind::Tiling {
            (0x10, 0x60)
        } else {
            (0x20, 0xa0)
        };
        for (i, value) in objects.into_iter().enumerate() {
            let gap = if self.kind == Kind::Tiling && i != 0 {
                8
            } else {
                0
            };
            u64_at(out, po + i * 8 + gap, value);
        }
        encode_registers(&mut out[ro..ro + registers.len() * 12], registers);
        if self.kind == Kind::Fragment {
            for (at, n, width) in [
                (0x40, 0x16429, 8),
                (0x48, 0x10019, 8),
                (0x54, 0x100b1, 4),
                (0x68, 0x15131, 4),
                (0x6c, 0x15139, 4),
            ] {
                if let Some(v) = last(registers, n) {
                    if width == 8 {
                        u64_at(out, at, v);
                    } else {
                        u32_at(out, at, v as u32);
                    }
                }
            }
            if let Some(screen) = last(registers, 0x100d9) {
                u64_at(
                    out,
                    0x78,
                    ((screen & 0xfff) + 1) * (((screen >> 12) & 0xfff) + 1),
                );
            }
            u32_at(out, 0x458, pool);
            for at in [0x470, 0x47c] {
                u32_at(out, at, stamp);
            }
            for (at, v) in [
                (0x50, 1),
                (0x80, 0x56),
                (0x82, 0x57),
                (0x84, 0x57),
                (0x88, 0x59),
            ] {
                out[at] = v;
            }
            u32_at(out, 0x90, item);
        } else {
            for at in [0x18, 0x304] {
                u32_at(out, at, pool);
            }
            u32_at(out, 0x48, work);
            for at in [0x370, 0x37c, 0x388] {
                u32_at(out, at, stamp);
            }
            for (at, v) in [(0x38, 0x47), (0x3a, 0x49), (0x3c, 0x49)] {
                out[at] = v;
            }
        }
        for &(at, v) in &pointers[..count] {
            u64_at(out, at, v);
        }
        if self.write_tail {
            let (grid_at, high, low) = if self.kind == Kind::Tiling {
                (0x8ba, 0x86e, 0x8ce)
            } else {
                (0x2154, 0x2108, 0x2168)
            };
            u32_at(out, grid_at, grid);
            if self.write_lifecycle {
                u64_at(out, high, lifecycle >> 32);
                u64_at(out, low, lifecycle & 0xffffffff);
            }
            if self.write_item {
                self.item_fields(out, item, grid);
                for &(at, v) in self.item_overrides {
                    u32_at(out, at, v);
                }
            }
            if structural {
                self.structural_tail(out, registers, parameters.unwrap(), deflake);
            }
        }
        Ok(())
    }
    fn tail_pointers(&self, out: &mut [(usize, u64); 11]) -> Result<usize> {
        // Role: 0 fixed, 1 self alias, 2 pair dispatch slot, 3 status record.
        let ta = [
            (0x760, 0x7000000060, 1),
            (0x780, 0x1000240000, 0),
            (0x8a6, 0xfffffc20001c8000, 2),
            (0x8ae, 0xfffffc20c07c0000, 2),
            (0x8fe, 0xfffffc2000024c68, 0),
            (0x934, 0xfffffc20c0830000, 0),
            (0x945, 0, 3),
        ];
        let frag = [
            (0x7a0, 0x70000980a0, 1),
            (0xec0, 0x70000987c0, 1),
            (0x15e0, 0x7000098ee0, 1),
            (0x1d00, 0x7000099600, 1),
            (0x1f4e, 0x1000000000, 0),
            (0x2140, 0xfffffc20001c8004, 2),
            (0x2148, 0xfffffc20c07c0004, 2),
            (0x2198, 0xfffffc2000024c68, 0),
            (0x21a0, 0xfffffc2000024c70, 0),
            (0x21ce, 0xfffffc20c0830000, 0),
            (0x21df, 0, 3),
        ];
        let status_bases = if self.kind == Kind::Tiling {
            [
                0xfffffc2001610000,
                0xfffffc2001638000,
                0xfffffc2001690000,
                0xfffffc20c1698000,
            ]
        } else {
            [
                0xfffffc2001630000,
                0xfffffc2001650000,
                0xfffffc20016b0000,
                0xfffffc20c16c8000,
            ]
        };
        let fields = if self.kind == Kind::Tiling {
            &ta[..]
        } else {
            &frag[..]
        };
        for (i, &(at, v, role)) in fields.iter().enumerate() {
            let value = if let Some(&(_, value)) = self
                .pointer_overrides
                .iter()
                .rev()
                .find(|&&(offset, _)| offset == at)
            {
                value
            } else {
                match role {
                    1 => {
                        if let Some(alias) = self.low_alias {
                            add(alias, v & 0x3fff)?
                        } else {
                            add(v, self.ordinal as u64 * self.kind.size() as u64)?
                        }
                    }
                    2 => add(v, self.queue_pair as u64 * 8)?,
                    3 => add(
                        self.status_base
                            .unwrap_or(status_bases[self.queue_pair as usize]),
                        self.index as u64 * 0x40,
                    )?,
                    _ => v,
                }
            };
            out[i] = (at, value);
        }
        Ok(fields.len())
    }
    fn item_fields(&self, out: &mut [u8], item: u32, grid: u32) {
        if self.kind == Kind::Tiling {
            for (at, v) in [
                (0x79c, grid + 1),
                (0x7a0, item * 0x100),
                (0x7a8, item),
                (0x7b0, item * 0x101),
                (0x8b4, (item << 24) | 0xffff),
                (0x8c4, item << 16),
                (0x8c8, item << 24),
                (0x8d4, self.index << 16),
            ] {
                u32_at(out, at, v);
            }
        } else {
            for (at, v) in [
                (0x2150, item << 8),
                (0x215c, 1),
                (0x2160, item),
                (0x2164, item << 8),
                (0x2170, self.index),
            ] {
                u32_at(out, at, v);
            }
        }
    }
    fn structural_tail(
        &self,
        out: &mut [u8],
        registers: &[Register],
        p: &Parameters,
        deflake: u64,
    ) {
        let sampler_max = if p.sampler_count == 0 {
            0
        } else {
            p.sampler_count as u32 + 1
        };
        if self.kind == Kind::Tiling {
            for (at, v) in [
                (0x768, 0x036c0049),
                (0x7d6, deflake as u32),
                (0x876, u32::MAX),
                (0x882, p.sampler_count as u32),
                (0x886, sampler_max),
            ] {
                u32_at(out, at, v);
            }
            for (at, v) in [
                (0x780, p.tpc),
                (0x87a, p.sampler_array),
                (0x8fe, p.timestamp_a),
                (0x906, p.ta_timestamp_end),
                (0x90e, p.ta_user_timestamp_start),
                (0x916, p.ta_user_timestamp_end),
            ] {
                u64_at(out, at, v);
            }
            for (at, v) in [
                (0x789, 0x78),
                (0x892, p.reactive_tvb_growth as u8),
                (0x89a, p.vertex_store_flag as u8),
                (0x932, 0x44),
                (0x93c, 1),
                (0x94d, 1),
            ] {
                out[at] = v;
            }
            return;
        }
        // Parameters were validated before any bytes were written. Each
        // embedded program is encoded independently to bound stack use.
        u32_at(
            out,
            0x7a8,
            ((registers.len() * 12) as u32) << 16 | registers.len() as u32,
        );
        Self::embedded(out, 0xec8, 0x7c0, &partial_store_registers(p).unwrap());
        Self::embedded(out, 0x15e8, 0xee0, &partial_resume_registers(p).unwrap());
        Self::embedded(out, 0x1d08, 0x1600, &partial_load_registers(p).unwrap());
        for (at, v) in [
            (0x1d20, p.depth_bias_array),
            (0x1d30, p.scissor_array),
            (0x1d40, p.occlusion_query_base),
            (0x1e78, p.load_pipeline_bind),
            (0x1e80, p.load_pipeline),
            (0x1ea8, p.partial_load_pipeline_bind),
            (0x1eb0, p.partial_load_pipeline),
            (0x1f38, p.tib_blocks),
            (0x1f40, p.aux_fb_flags),
            (0x1f50, p.aux_fb_page_count),
            (0x1f58, p.tile_config),
            (0x1f7c, p.store_pipeline),
            (0x1f9c, p.partial_store_pipeline),
            (0x1fac, (p.tib_blocks << 33) | 0x300),
            (0x2114, p.sampler_array),
            (0x2198, p.fragment_timestamp_start),
            (0x21a0, p.fragment_timestamp_end),
            (0x21a8, p.fragment_user_timestamp_start),
            (0x21b0, p.fragment_user_timestamp_end),
        ] {
            u64_at(out, at, v);
        }
        for (at, v) in [
            (0x1ec0, 0x04040404),
            (0x1f48, p.width as u32),
            (0x1f4c, p.height as u32),
            (0x1f78, p.store_pipeline_bind as u32),
            (0x1f98, p.partial_store_pipeline_bind as u32),
            (0x1fa8, p.depth_clear_value_bits as u32),
            (0x2110, u32::MAX),
            (0x211c, p.sampler_count as u32),
            (0x2120, sampler_max),
        ] {
            u32_at(out, at, v);
        }
        for (at, v) in [
            (
                0x2100,
                p.fragment_sync_grow.unwrap_or(!p.process_empty_tiles) as u8,
            ),
            (0x20fc, p.fragment_store_flag as u8),
            (0x2124, p.process_empty_tiles as u8),
            (0x2128, 1),
            (0x21cc, 0x53),
            (0x21d6, 1),
            (0x21e7, 1),
            (0x2208, 1),
            (0x220c, 1),
            (0x222d, 1),
        ] {
            out[at] = v;
        }
    }
    fn embedded(out: &mut [u8], header: usize, at: usize, registers: &[Register]) {
        u32_at(
            out,
            header,
            ((registers.len() * 12) as u32) << 16 | registers.len() as u32,
        );
        encode_registers(&mut out[at..at + registers.len() * 12], registers);
    }
}
