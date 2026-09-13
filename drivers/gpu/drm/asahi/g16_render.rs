// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from Niklas Sheth's linux-m4-integration prototype.

//! T8132 render register ABI. Ordered writes are significant, including duplicates.
//! Ported from the open m1n1 G16/G17 register model. Addresses and lifecycle
//! values are supplied by the runtime; geometry is derived from each render.

#[derive(Clone, Copy, Default)]
pub(crate) struct Parameters {
    pub(crate) request_tvb_growth: bool,
    pub(crate) fragment_sync_grow: bool,
    pub(crate) process_empty_tiles: bool,
    pub(crate) partial_load_pipeline: u64,
    pub(crate) partial_load_pipeline_bind: u64,
    pub(crate) partial_store_pipeline: u64,
    pub(crate) partial_store_pipeline_bind: u64,
    pub(crate) sample_size: u64,
    pub(crate) sampler_array: u64,
    pub(crate) sampler_count: u64,

    pub(crate) aux_fb: u64,
    pub(crate) aux_fb_flags: u64,
    pub(crate) aux_fb_page_count: u64,
    pub(crate) completion_control: u64,
    pub(crate) context_base: u64,
    pub(crate) cycle: u64,
    pub(crate) deflake_1: u64,
    pub(crate) deflake_2: u64,
    pub(crate) deflake_3: u64,
    pub(crate) depth_aux_buffer: u64,
    pub(crate) depth_aux_stride: u64,
    pub(crate) depth_bias_array: u64,
    pub(crate) depth_buffer: u64,
    pub(crate) depth_clear_value_bits: u64,
    pub(crate) depth_dimensions: u64,
    pub(crate) depth_flags: u64,
    pub(crate) depth_stride: u64,
    pub(crate) encoder: u64,
    pub(crate) fragment_admission: u64,
    pub(crate) fragment_gate: u64,
    pub(crate) fragment_lifecycle: u64,
    pub(crate) fragment_status: u64,
    pub(crate) fragment_work_stamp: u64,
    pub(crate) heapmeta: u64,
    pub(crate) height: u64,
    pub(crate) layermeta: u64,
    pub(crate) layers: u64,
    pub(crate) load_pipeline: u64,
    pub(crate) load_pipeline_bind: u64,
    pub(crate) merge_upper_x_bits: u64,
    pub(crate) merge_upper_y_bits: u64,
    pub(crate) multisample_control: u64,
    pub(crate) occlusion_query_base: u64,
    pub(crate) ppp_control: u64,
    pub(crate) queue_item_index: u64,
    pub(crate) samples: u64,
    pub(crate) scissor_array: u64,
    pub(crate) stencil_aux_buffer: u64,
    pub(crate) stencil_aux_stride: u64,
    pub(crate) stencil_buffer: u64,
    pub(crate) stencil_clear_value: u64,
    pub(crate) stencil_stride: u64,
    pub(crate) store_pipeline: u64,
    pub(crate) store_pipeline_bind: u64,
    pub(crate) ta_status: u64,
    pub(crate) tib_blocks: u64,
    pub(crate) tile_config: u64,
    pub(crate) tilemap: u64,
    pub(crate) tiling_admission: u64,
    pub(crate) tiling_gate: u64,
    pub(crate) tiling_lifecycle: u64,
    pub(crate) tiling_work_stamp: u64,
    pub(crate) tpc: u64,
    pub(crate) utile_config: u64,
    pub(crate) utile_height: u64,
    pub(crate) utile_width: u64,
    pub(crate) width: u64,
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
    pub(crate) fn valid(&self) -> bool {
        (1..=16384).contains(&self.width)
            && (1..=16384).contains(&self.height)
            && (1..=2048).contains(&self.layers)
            && matches!(
                (self.utile_width, self.utile_height),
                (32, 32) | (32, 16) | (16, 16)
            )
            && matches!(self.samples, 1 | 2 | 4)
            && self.tib_blocks <= 64
            && self.queue_item_index <= 0xffff
            && (self.sampler_array == 0) == (self.sampler_count == 0)
            && self.sampler_array & 7 == 0
            && self.sampler_count < u32::MAX as u64
            && [
                self.tilemap,
                self.heapmeta,
                self.layer_metadata(),
                self.tpc,
                self.deflake_1,
                self.deflake_2,
                self.deflake_3,
                self.encoder,
            ]
            .iter()
            .all(|a| {
                a.checked_sub(self.context_base)
                    .is_some_and(|v| v <= u32::MAX as u64)
            })
    }

    fn ta_offset(&self, address: u64) -> u64 {
        address - self.context_base
    }
    fn layer_metadata(&self) -> u64 {
        if self.layermeta == 0 {
            self.heapmeta
        } else {
            self.layermeta
        }
    }
    fn tib_encoding(&self) -> u64 {
        // T8132's 1..8 RGBA16F attachment sweep covers the full 128 KiB
        // allocation. The low field wraps at 64 blocks; multisample renders
        // double their bank tag above 64 KiB. Bit 6 is not an allocation bit.
        let blocks = self.tib_blocks & 0x3f;
        if self.samples == 1 {
            blocks
        } else {
            let banks = if self.tib_blocks > 32 { 2 } else { 1 };
            ((self.samples * banks) << 16) | blocks
        }
    }
    fn geometry(&self) -> Geometry {
        let tx = self.width.div_ceil(32);
        let ty = self.height.div_ceil(32);
        let mx = (tx.div_ceil(4) + 3) & !3;
        let my = (ty.div_ceil(4) + 3) & !3;
        let ux = 32 / self.utile_width;
        let uy = 32 / self.utile_height;
        Geometry {
            tiles_x: tx,
            tiles_y: ty,
            size1: (5 * mx * my * ux * uy).div_ceil(4),
            size2: mx * my,
            size3: 2 * mx * my * ux * uy,
            x_blocks: 3 * mx | ((2 * mx) << 9) | (mx << 18),
            y_blocks: 3 * my | ((2 * my) << 9) | (my << 18),
            screen: ((ty - 1) << 12) | (tx - 1),
            pixels: (self.width - 1) | ((self.height - 1) << 16),
            macro_size: my * uy | ((mx * ux) << 16),
        }
    }

    pub(crate) fn tiling_registers(&self) -> Option<[(u32, u64); 71]> {
        if !self.valid() {
            return None;
        }
        let g = self.geometry();
        let tilemap = self.ta_offset(self.tilemap);
        let heapmeta = self.ta_offset(self.heapmeta);
        let layermeta = self.ta_offset(self.layer_metadata());
        let target_layers = if self.layers > 1 {
            0xe000 | (self.layers - 1)
        } else {
            0x8000
        };
        let deflake_1 = self.ta_offset(self.deflake_1);
        let record_index = 0x80005 + self.queue_item_index * 4;
        Some([
            (0x01748, 1),
            (0x10141, 512),
            (0x1c039, tilemap),
            (0x1c9c8, tilemap),
            (0x1c0a1, self.ta_offset(self.tpc)),
            (0x1c031, heapmeta | 0x8000000000000000),
            (0x1c9c0, heapmeta | 0x8000000000000000),
            (0x1c051, 0x3a0012006b0003),
            (0x1c061, 1),
            (0x10149, self.utile_config),
            (0x10139, self.multisample_control),
            (0x10111, deflake_1),
            (0x1c9b0, deflake_1),
            (0x10119, self.ta_offset(self.deflake_2)),
            (0x1c9b8, self.ta_offset(self.deflake_2)),
            (0x1c958, 1),
            (0x1c950, self.ta_offset(self.deflake_3) | 0x4000000000000),
            (0x1c930, 0),
            (0x1c880, self.ta_offset(self.encoder)),
            (0x1c079, layermeta),
            (0x1c9d8, layermeta),
            (0x10151, 0),
            (0x1c199, 0),
            (0x1c1a1, 0),
            (0x1c1a9, 0),
            (0x1c1b1, 0),
            (0x1c1b9, 0),
            (0x1c8f8, 0x8860),
            (0x1c0b1, g.size1),
            (0x1c850, g.size1),
            (0x10131, 0x88),
            (0x10121, self.ppp_control),
            (0x10129, g.pixels),
            (0x101b9, g.screen),
            (0x1c069, g.x_blocks),
            (0x1c071, g.y_blocks),
            (0x1c081, g.size2),
            (0x1c0a9, g.size3),
            (0x10171, 256),
            (0x10169, target_layers),
            (0x0a309, 0),
            (0x1c8e0, 0xffffffffffffffff),
            (0x1c8e8, 0xffffffffffffffff),
            (0x1c898, 0),
            (0x101e1, 28),
            (0x1c9e8, target_layers & 0x4fff),
            (0x1a099, 0),
            (0x1a0a1, 0),
            (0x1a069, 0),
            (0x1a071, 0),
            (0x1a0c9, 0),
            (0x1a0d1, 0),
            (0x101c9, 0),
            (0x0d471, 0),
            (0x1a0f1, 8),
            (0x10799, 0xff0000),
            (0x1c830, self.tiling_admission),
            (0x1ca30, self.cycle),
            (0x16c39, self.cycle),
            (0x1c910, record_index),
            (0x0a5a1, self.tiling_gate),
            (0x0d419, 0x200000001),
            (0x1ca10, self.tiling_lifecycle),
            (0x014a1, self.tiling_lifecycle),
            (0x0a349, self.tiling_lifecycle),
            (0x10209, self.tiling_work_stamp),
            (0x1c9f0, self.tiling_work_stamp),
            (0x14320, self.tiling_work_stamp),
            (0x14308, self.completion_control),
            (0x14318, self.ta_status | 1),
            (0x01740, 1),
        ])
    }

    pub(crate) fn fragment_registers(&self) -> Option<[(u32, u64); 87]> {
        if !self.valid() {
            return None;
        }
        let g = self.geometry();
        let tile_state = 0x3717f
            | ((g.tiles_x - 1) << 44)
            | ((g.tiles_y - 1) << 53)
            | 0x2000000000
            | if self.layers > 1 { 0x100000000 } else { 0 }
            | ((self.utile_config & 0xf000) << 28);
        Some([
            (0x01739, 1),
            (0x10009, self.utile_config),
            (0x15379, self.store_pipeline_bind),
            (0x15381, self.store_pipeline),
            (0x15369, self.load_pipeline_bind),
            (0x15371, self.load_pipeline),
            (0x15131, self.merge_upper_x_bits),
            (0x15139, self.merge_upper_y_bits),
            (0x100a1, 0),
            (0x15069, 0),
            (0x15071, 0),
            (0x16058, 0),
            (0x10019, self.multisample_control),
            (0x100b1, g.macro_size),
            (0x16030, g.macro_size),
            (0x100d9, g.screen),
            (0x0a301, 0),
            (0x10791, 0xff0200),
            (0x16098, self.heapmeta),
            (0x15109, self.scissor_array),
            (0x15101, self.depth_bias_array),
            (0x15021, self.aux_fb_flags),
            (0x15211, self.height << 32 | self.width),
            (0x15049, self.aux_fb_page_count),
            (0x10051, self.tib_encoding()),
            (0x15321, self.depth_dimensions),
            (0x15301, self.depth_clear_value_bits),
            (0x15309, self.stencil_clear_value | 0x300),
            (0x15311, self.occlusion_query_base),
            (0x15319, self.depth_flags),
            (0x15349, 0x4040404),
            (0x15351, 0),
            (0x15329, self.depth_buffer),
            (0x15331, self.depth_buffer),
            (0x15339, self.stencil_buffer),
            (0x15341, self.stencil_buffer),
            (0x15231, 0),
            (0x15221, 0),
            (0x15239, 0),
            (0x15229, 0),
            (0x15401, self.depth_stride),
            (0x15421, self.depth_stride),
            (0x15409, self.stencil_stride),
            (0x15429, self.stencil_stride),
            (0x153c1, self.depth_aux_buffer),
            (0x15411, self.depth_aux_stride),
            (0x153c9, self.depth_aux_buffer),
            (0x15431, self.depth_aux_stride),
            (0x153d1, self.stencil_aux_buffer),
            (0x15419, self.stencil_aux_stride),
            (0x153d9, self.stencil_aux_buffer),
            (0x15439, self.stencil_aux_stride),
            (0x16429, self.tilemap),
            (0x16060, self.layer_metadata()),
            (0x16431, 4 * g.size1 << 24),
            (0x10039, self.tile_config),
            (0x16020, 0),
            (0x16451, 0),
            (0x15359, 0),
            (0x100b8, 0x8860),
            (0x16461, self.aux_fb),
            (0x16090, self.aux_fb),
            (0x101e9, 28),
            (0x160a8, 0),
            (0x16068, tile_state),
            (0x1a0a9, 0),
            (0x1a0b1, 0),
            (0x1a079, 0),
            (0x1a081, 0),
            (0x1a0d9, 0),
            (0x1a0e1, 0),
            (0x101c1, 0),
            (0x0d469, 0),
            (0x1a0f9, 8),
            (0x0a5a9, self.fragment_gate),
            (0x0d429, 0x200000001),
            (0x160e0, self.fragment_lifecycle),
            (0x01499, self.fragment_lifecycle),
            (0x0a341, self.fragment_lifecycle),
            (0x1c838, self.fragment_admission),
            (0x1ca28, self.cycle),
            (0x10211, self.fragment_work_stamp),
            (0x10420, self.fragment_work_stamp),
            (0x14048, self.completion_control),
            (0x14080, self.fragment_status | 1),
            (0x01731, 1),
            (0x01731, 1),
        ])
    }
}

pub(crate) fn pack_registers(records: &[(u32, u64)]) -> Option<[u8; 0x600]> {
    if records.len() > 0x600 / 12 {
        return None;
    }
    let mut out = [0; 0x600];
    for (slot, (number, value)) in out.chunks_exact_mut(12).zip(records) {
        slot[..4].copy_from_slice(&number.to_le_bytes());
        slot[4..].copy_from_slice(&value.to_le_bytes());
    }
    Some(out)
}

fn u32_at(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn u64_at(out: &mut [u8], offset: usize, value: u64) {
    out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Addresses {
    pub(crate) buffer_manager_block_control: u64,
    pub(crate) buffer_manager_block_list: u64,
    pub(crate) buffer_manager_counter: u64,
    pub(crate) buffer_manager_page_list: u64,
    pub(crate) buffer_manager_scene_list: u64,
    pub(crate) event_count_array: u64,
    pub(crate) fragment_prelude: u64,
    pub(crate) pool_slots: u64,
    pub(crate) render_shared_state: u64,
    pub(crate) tiling_prelude: u64,

    pub(crate) empty_buffer: u64,
    pub(crate) event_control: u64,
    pub(crate) fragment_counter: u64,
    pub(crate) fragment_driver_stamp: u64,
    pub(crate) fragment_register_alias: u64,
    pub(crate) fragment_shared_tail: u64,
    pub(crate) tiling_driver_stamp: u64,
    pub(crate) tiling_register_alias: u64,
    pub(crate) tiling_shared_tail: u64,
    pub(crate) timestamp_end: u64,
    pub(crate) timestamp_start: u64,

    pub(crate) buffer_manager: u64,
    pub(crate) buffer_manager_slot: u64,
    pub(crate) buffer_thing: u64,
    pub(crate) context_id: u64,
    pub(crate) fragment_event: u64,
    pub(crate) fragment_firmware_stamp: u64,
    pub(crate) fragment_microsequence: u64,
    pub(crate) fragment_queue: u64,
    pub(crate) fragment_stamp: u64,
    pub(crate) fragment_stats: u64,
    pub(crate) fragment_status_page: u64,
    pub(crate) fragment_user_timestamp_end: u64,
    pub(crate) fragment_user_timestamp_start: u64,
    pub(crate) fragment_uuid: u64,
    pub(crate) fragment_work: u64,
    pub(crate) support: u64,
    pub(crate) tiling_counter: u64,
    pub(crate) tiling_event: u64,
    pub(crate) tiling_firmware_stamp: u64,
    pub(crate) tiling_microsequence: u64,
    pub(crate) tiling_queue: u64,
    pub(crate) tiling_stamp: u64,
    pub(crate) tiling_stats: u64,
    pub(crate) tiling_status_page: u64,
    pub(crate) tiling_user_timestamp_end: u64,
    pub(crate) tiling_user_timestamp_start: u64,
    pub(crate) tiling_uuid: u64,
    pub(crate) tiling_work: u64,
}

impl Addresses {
    pub(crate) fn tiling_microsequence(&self) -> [u8; 0x300] {
        let mut out = [0; 0x300];
        let work = self.tiling_work;
        let queue = self.tiling_queue;
        let user_timestamps =
            if self.tiling_user_timestamp_start != 0 || self.tiling_user_timestamp_end != 0 {
                work + 0x8d8
            } else {
                0
            };
        u32_at(&mut out, 0x0, 5);
        for (offset, value) in [
            (0x14, work + 0x40),
            (0x1c, self.buffer_manager),
            (0x24, self.buffer_thing),
            (0x2c, self.tiling_stats),
            (0x34, queue),
            (0x3c, work + 0x85c),
        ] {
            u64_at(&mut out, offset, value);
        }
        u32_at(&mut out, 0x44, self.context_id as u32);
        u32_at(&mut out, 0x48, self.tiling_counter as u32);
        u64_at(&mut out, 0x50, self.buffer_manager_slot);
        u64_at(&mut out, 0x64, work + 0x784);
        u64_at(&mut out, 0x6c, work + 0x8a8);
        u32_at(&mut out, 0x7c, self.tiling_uuid as u32);
        u64_at(&mut out, 0x190, self.support);
        u64_at(&mut out, 0x1a8, self.tiling_status_page);
        u64_at(&mut out, 0x1b8, 1);
        u32_at(&mut out, 0x1c0, self.tiling_event as u32);
        u32_at(&mut out, 0x1cc, 0x80000003);
        for (offset, value) in [
            (0x1d0, work + 0x8c0),
            (0x1d8, work + 0x8c8),
            (0x1e0, work + 0x8c8),
            (0x1e8, queue),
            (0x1f0, user_timestamps),
            (0x1f8, work + 0x928),
            (0x200, work + 0x83c),
        ] {
            u64_at(&mut out, offset, value);
        }
        u32_at(&mut out, 0x210, self.tiling_uuid as u32);
        u32_at(&mut out, 0x218, 1);
        u32_at(&mut out, 0x21c, 3);
        for (offset, value) in [
            (0x220, work + 0x8c0),
            (0x228, work + 0x8c8),
            (0x230, work + 0x8d0),
            (0x238, queue),
            (0x240, user_timestamps),
            (0x248, work + 0x928),
            (0x250, work + 0x83c),
        ] {
            u64_at(&mut out, offset, value);
        }
        u32_at(&mut out, 0x260, self.tiling_uuid as u32);
        u32_at(&mut out, 0x268, 6);
        for (offset, value) in [
            (0x26c, self.buffer_thing),
            (0x274, self.buffer_manager),
            (0x27c, self.tiling_stats),
            (0x284, queue),
            (0x28c, work + 0x85c),
        ] {
            u64_at(&mut out, offset, value);
        }
        u32_at(&mut out, 0x294, self.context_id as u32);
        u64_at(&mut out, 0x29c, work + 0x784);
        u32_at(&mut out, 0x2a8, self.tiling_uuid as u32);
        u64_at(&mut out, 0x2b0, self.tiling_firmware_stamp);
        u32_at(&mut out, 0x2b8, self.tiling_stamp as u32);
        u64_at(&mut out, 0x2e0, self.tiling_microsequence + 0x190);
        u32_at(&mut out, 0x2e8, (-0x268i32) as u32);
        u64_at(&mut out, 0x2ed, work + 0x918);
        u64_at(&mut out, 0x2f5, 0x200000000000001);
        u32_at(&mut out, 0x2fc, 0x40000002);
        out
    }
    pub(crate) fn fragment_microsequence(&self) -> [u8; 0x380] {
        let mut out = [0; 0x380];
        let work = self.fragment_work;
        let queue = self.fragment_queue;
        let user_timestamps =
            if self.fragment_user_timestamp_start != 0 || self.fragment_user_timestamp_end != 0 {
                work + 0xc20
            } else {
                0
            };
        u32_at(&mut out, 0x0, 7);
        for (offset, value) in [
            (0x14, work + 0x80),
            (0x1c, self.buffer_thing),
            (0x24, self.fragment_stats),
            (0x2c, work + 0xb70),
            (0x34, self.buffer_thing + 0x54),
            (0x3c, work + 0xbb0),
            (0x44, work + 0xbb4),
            (0x4c, queue),
            (0x54, work),
        ] {
            u64_at(&mut out, offset, value);
        }
        u32_at(&mut out, 0x5c, self.context_id as u32);
        u32_at(&mut out, 0x60, self.tiling_counter as u32);
        u64_at(&mut out, 0x68, self.buffer_manager_slot);
        u64_at(&mut out, 0x7c, work + 0xa58);
        u64_at(&mut out, 0x84, work + 0xbf0);
        u32_at(&mut out, 0xa0, self.fragment_uuid as u32);
        u64_at(&mut out, 0x1b0, self.support);
        u64_at(&mut out, 0x1c8, self.fragment_status_page);
        u32_at(&mut out, 0x1e0, self.fragment_event as u32);
        u32_at(&mut out, 0x1e4, 1);
        u32_at(&mut out, 0x1ec, 0x80000003);
        for (offset, value) in [
            (0x1f0, work + 0xc08),
            (0x1f8, work + 0xc10),
            (0x200, work + 0xc10),
            (0x208, queue),
            (0x210, user_timestamps),
            (0x218, work + 0xc70),
            (0x220, work + 0xb84),
        ] {
            u64_at(&mut out, offset, value);
        }
        u32_at(&mut out, 0x230, self.fragment_uuid as u32);
        u32_at(&mut out, 0x238, 1);
        u32_at(&mut out, 0x23c, 3);
        for (offset, value) in [
            (0x240, work + 0xc08),
            (0x248, work + 0xc10),
            (0x250, work + 0xc18),
            (0x258, queue),
            (0x260, user_timestamps),
            (0x268, work + 0xc70),
            (0x270, work + 0xb84),
        ] {
            u64_at(&mut out, offset, value);
        }
        u32_at(&mut out, 0x280, self.fragment_uuid as u32);
        u32_at(&mut out, 0x288, 8);
        u32_at(&mut out, 0x28c, self.fragment_uuid as u32);
        u64_at(&mut out, 0x294, self.fragment_firmware_stamp);
        u32_at(&mut out, 0x29c, self.fragment_stamp as u32);
        for (offset, value) in [
            (0x2a4, self.buffer_thing),
            (0x2ac, self.buffer_manager),
            (0x2b4, 1),
            (0x2b8, self.fragment_stats),
            (0x2c0, work + 0xbb0),
            (0x2c8, work + 0xbb4),
            (0x2d0, work + 0xb70),
            (0x2d8, queue),
            (0x2e0, work),
        ] {
            u64_at(&mut out, offset, value);
        }
        u32_at(&mut out, 0x2e8, self.context_id as u32);
        u64_at(&mut out, 0x2ec, work + 0xa58);
        u64_at(&mut out, 0x320, self.fragment_microsequence + 0x1b0);
        u32_at(&mut out, 0x328, (-0x288i32) as u32);
        u64_at(&mut out, 0x32d, work + 0xc60);
        u64_at(&mut out, 0x335, 0x110100000001);
        u32_at(&mut out, 0x344, 0x40000002);
        out
    }
}

fn tagged_page(address: u64) -> u64 {
    (1 << 56) | (address >> 8)
}

impl Addresses {
    fn valid(&self) -> bool {
        (self.support | self.tiling_shared_tail | self.fragment_shared_tail) & 0xff == 0
    }
}

impl Parameters {
    pub(crate) fn tiling_work(&self, a: &Addresses) -> Option<[u8; 0x940]> {
        if !self.valid() || !a.valid() {
            return None;
        }
        let mut out = [0; 0x940];
        let registers = pack_registers(&self.tiling_registers()?)?;
        let g = self.geometry();
        let sampler_array = self.sampler_array;
        let sampler_count = self.sampler_count;
        let sampler_max = if sampler_count == 0 {
            0
        } else {
            sampler_count + 1
        };
        u32_at(&mut out, 0x0, 0x0);
        u64_at(&mut out, 0x4, a.tiling_counter);
        u32_at(&mut out, 0xc, a.context_id as u32);
        u64_at(&mut out, 0x10, a.event_control);
        u64_at(&mut out, 0x18, a.buffer_manager_slot);
        u64_at(&mut out, 0x20, a.buffer_manager);
        u64_at(&mut out, 0x28, a.buffer_thing);
        u64_at(&mut out, 0x30, a.empty_buffer);
        out[0x40..0x640].copy_from_slice(&registers);
        u64_at(&mut out, 0x740, a.tiling_register_alias);
        out[0x748..0x74a].copy_from_slice(&(0x41u16).to_le_bytes());
        out[0x74a..0x74c].copy_from_slice(&((0x41 * 0xc) as u16).to_le_bytes());
        u64_at(&mut out, 0x760, self.tpc);
        u64_at(&mut out, 0x768, (g.size3 << 0x6) * self.layers);
        u64_at(&mut out, 0x770, a.tiling_microsequence);
        u32_at(&mut out, 0x778, 0x300);
        u32_at(&mut out, 0x77c, 0x11);
        u32_at(&mut out, 0x780, a.fragment_stamp as u32);
        u64_at(&mut out, 0x7a8, self.ta_offset(self.deflake_1));
        out[0x864] = u8::from(self.request_tvb_growth);
        u64_at(&mut out, 0x840, self.tiling_lifecycle >> 0x20);
        u32_at(&mut out, 0x848, 0xffffffff);
        u64_at(&mut out, 0x84c, sampler_array);
        u32_at(&mut out, 0x84c + 0x8, sampler_count as u32);
        u32_at(&mut out, 0x84c + 0xc, sampler_max as u32);
        u64_at(&mut out, 0x878, a.tiling_driver_stamp);
        u64_at(&mut out, 0x880, a.tiling_firmware_stamp);
        u32_at(&mut out, 0x888, a.tiling_stamp as u32);
        u32_at(&mut out, 0x88c, a.tiling_event as u32);
        u32_at(&mut out, 0x898, a.tiling_uuid as u32);
        u64_at(&mut out, 0x8a0, self.layers - 0x1);
        u64_at(&mut out, 0x8c8, a.timestamp_start);
        u64_at(&mut out, 0x8d8, a.tiling_user_timestamp_start);
        u64_at(&mut out, 0x8e0, a.tiling_user_timestamp_end);
        u64_at(&mut out, 0x8ff, tagged_page(a.support));
        u64_at(
            &mut out,
            0x908,
            0xdaa0 | if self.layers > 1 { 1 << 62 } else { 0 },
        );
        u64_at(&mut out, 0x910, tagged_page(a.tiling_shared_tail));
        Some(out)
    }
    pub(crate) fn fragment_work(&self, a: &Addresses) -> Option<[u8; 0xcc0]> {
        if !self.valid() || !a.valid() {
            return None;
        }
        let mut out = [0; 0xcc0];
        let registers = pack_registers(&self.fragment_registers()?)?;
        let g = self.geometry();
        let sampler_array = self.sampler_array;
        let sampler_count = self.sampler_count;
        let sampler_max = if sampler_count == 0 {
            0
        } else {
            sampler_count + 1
        };
        let tiles_x = g.tiles_x;
        let tiles_y = g.tiles_y;
        let merge_x = if self.merge_upper_x_bits != 0 {
            self.merge_upper_x_bits
        } else {
            (crate::float::F32::from_bits(0x3fddb3d9) / crate::float::F32::from(self.width as u32))
                .to_bits() as u64
        };
        let merge_y = if self.merge_upper_y_bits != 0 {
            self.merge_upper_y_bits
        } else {
            (crate::float::F32::from_bits(0x3fddb3d9) / crate::float::F32::from(self.height as u32))
                .to_bits() as u64
        };
        let compact_store = (self.store_pipeline & 0xffffffff) << 32;
        let compact_partial_store = (self.partial_store_pipeline & 0xffffffff) << 32;
        u32_at(&mut out, 0x0, 0x1);
        u64_at(&mut out, 0x4, a.fragment_counter);
        u32_at(&mut out, 0xc, a.context_id as u32);
        u32_at(&mut out, 0x10, 0x0);
        u64_at(&mut out, 0x14, a.fragment_microsequence);
        u32_at(&mut out, 0x1c, 0x380);
        u64_at(&mut out, 0x20, a.event_control);
        u64_at(&mut out, 0x28, a.buffer_manager);
        u64_at(&mut out, 0x30, a.buffer_thing);
        u64_at(&mut out, 0x38, a.empty_buffer);
        u64_at(&mut out, 0x40, self.tilemap);
        u64_at(&mut out, 0x48, self.multisample_control);
        u32_at(&mut out, 0x50, self.samples as u32);
        u32_at(&mut out, 0x54, g.macro_size as u32);
        u32_at(&mut out, 0x68, merge_x as u32);
        u32_at(&mut out, 0x6c, merge_y as u32);
        u64_at(&mut out, 0x78, tiles_x * tiles_y);
        out[0x80..0x680].copy_from_slice(&registers);
        u64_at(&mut out, 0x680, 0x8c020001dddc00);
        u64_at(&mut out, 0x688, 0xa0001de1800);
        u64_at(&mut out, 0x690, 0x8c020200029800);
        u64_at(&mut out, 0x698, 0x220200039520);
        u64_at(&mut out, 0x780, a.fragment_register_alias);
        out[0x788..0x78a].copy_from_slice(&(0x51u16).to_le_bytes());
        out[0x78a..0x78c].copy_from_slice(&((0x51 * 0xc) as u16).to_le_bytes());
        u64_at(&mut out, 0x7a0, self.depth_bias_array);
        u64_at(&mut out, 0x7b0, self.scissor_array);
        u64_at(&mut out, 0x7c0, self.occlusion_query_base);
        u64_at(&mut out, 0x8f8, self.load_pipeline_bind);
        u64_at(&mut out, 0x900, self.load_pipeline);
        u64_at(&mut out, 0x928, self.partial_load_pipeline_bind);
        u64_at(&mut out, 0x930, self.partial_load_pipeline);
        u64_at(&mut out, 0x938, self.depth_flags);
        u32_at(&mut out, 0x940, 0x4040404);
        u64_at(&mut out, 0x948, self.depth_buffer);
        u64_at(&mut out, 0x950, self.depth_stride);
        u64_at(&mut out, 0x958, self.depth_aux_stride);
        u64_at(&mut out, 0x960, self.depth_buffer);
        u64_at(&mut out, 0x968, self.depth_buffer);
        u64_at(&mut out, 0x970, self.depth_aux_buffer);
        u64_at(&mut out, 0x978, self.stencil_buffer);
        u64_at(&mut out, 0x980, self.stencil_stride);
        u64_at(&mut out, 0x988, self.stencil_aux_stride);
        u64_at(&mut out, 0x990, self.stencil_buffer);
        u64_at(&mut out, 0x998, self.stencil_buffer);
        u64_at(&mut out, 0x9a0, self.stencil_aux_buffer);
        u32_at(&mut out, 0x9b8, self.tib_encoding() as u32);
        u64_at(&mut out, 0x9c0, self.aux_fb_flags);
        u64_at(&mut out, 0x9c8, (self.height << 0x20) | self.width);
        u64_at(&mut out, 0x9d0, self.aux_fb_page_count);
        u64_at(&mut out, 0x9d8, self.tile_config);
        // The compact EOT records retain the common firmware layout:
        // rsrc_spec at +0x0c, USC address at +0x14 (fw/fragment.rs::EotProgram).
        u32_at(&mut out, 0x9f4, self.store_pipeline_bind as u32);
        u64_at(&mut out, 0x9f8, compact_store);
        u64_at(&mut out, 0x9fc, self.store_pipeline);
        u32_at(&mut out, 0xa14, self.partial_store_pipeline_bind as u32);
        u64_at(&mut out, 0xa18, compact_partial_store);
        u64_at(&mut out, 0xa1c, self.partial_store_pipeline);
        u32_at(&mut out, 0xa28, self.depth_clear_value_bits as u32);
        u32_at(
            &mut out,
            0xa2c,
            ((self.stencil_clear_value & 0xff) | 0x300) as u32,
        );
        u64_at(&mut out, 0xa30, self.sample_size);
        u64_at(&mut out, 0xa50, self.depth_dimensions);
        u32_at(&mut out, 0xb80, u32::from(self.fragment_sync_grow));
        u64_at(&mut out, 0xb88, self.fragment_lifecycle >> 0x20);
        u32_at(&mut out, 0xb90, 0xffffffff);
        u64_at(&mut out, 0xb94, sampler_array);
        u32_at(&mut out, 0xb94 + 0x8, sampler_count as u32);
        u32_at(&mut out, 0xb94 + 0xc, sampler_max as u32);
        u32_at(&mut out, 0xba4, u32::from(self.process_empty_tiles));
        u32_at(&mut out, 0xba8, 0x1);
        u32_at(&mut out, 0xbac, u32::from(self.samples > 1));
        u64_at(&mut out, 0xbc0, a.fragment_driver_stamp);
        u64_at(&mut out, 0xbc8, a.fragment_firmware_stamp);
        u32_at(&mut out, 0xbd0, a.fragment_stamp as u32);
        u32_at(&mut out, 0xbd4, a.fragment_event as u32);
        u32_at(&mut out, 0xbe0, a.fragment_uuid as u32);
        u64_at(&mut out, 0xbe8, self.layers - 0x1);
        u64_at(&mut out, 0xc10, a.timestamp_start);
        u64_at(&mut out, 0xc18, a.timestamp_end);
        u64_at(&mut out, 0xc20, a.fragment_user_timestamp_start);
        u64_at(&mut out, 0xc28, a.fragment_user_timestamp_end);
        u64_at(&mut out, 0xc47, tagged_page(a.support));
        u64_at(
            &mut out,
            0xc50,
            0x113a0 | if self.layers > 1 { 1 << 62 } else { 0 },
        );
        u64_at(&mut out, 0xc58, tagged_page(a.fragment_shared_tail));
        u64_at(&mut out, 0xc80, 0x100000000);
        Some(out)
    }
}

/// Per-VM firmware operand storage, placed in the caller kernel carveout.
pub(crate) const CONTEXT_BASE: u64 = 0x10_0000_0000;
pub(crate) const PRIVATE_SIZE: usize = 0x35d4000;
pub(crate) const HEAPMETA_OFFSET: u64 = 0x35d0000;
const OPERAND_STRIDE: u64 = 0x208000;

#[derive(Clone, Copy)]
pub(crate) struct OperandPool {
    pub(crate) directory: u64,
    pub(crate) table: u64,
    buffer: u64,
}
impl OperandPool {
    pub(crate) fn new(start: u64, end: u64) -> Option<Self> {
        let directory = start.checked_add(PRIVATE_SIZE as u64 + 0x4000)?;
        let table = directory.checked_add(0x408000)?;
        let buffer = table.checked_add(0x18000)?;
        let pool = Self {
            directory,
            table,
            buffer,
        };
        if start & 0x3fff != 0 || pool.end()? > end {
            return None;
        }
        Some(pool)
    }
    pub(crate) fn end(&self) -> Option<u64> {
        self.buffer.checked_add(24 * OPERAND_STRIDE)
    }
    pub(crate) fn growth_base(&self) -> Option<u64> {
        Some(self.end()?.checked_add(0x7fff)? & !0x7fff)
    }
    pub(crate) fn growth_end(&self) -> Option<u64> {
        self.growth_base()?.checked_add(0x3200000)
    }
    pub(crate) fn support(&self, id: u64, shared: u64) -> [u8; 0x100] {
        let mut out = [0; 0x100];
        u64_at(&mut out, 0, id);
        u64_at(&mut out, 0x14, self.directory);
        u32_at(&mut out, 0x1c, 0x400000 / 8);
        u32_at(&mut out, 0x24, 24 * 512);
        u32_at(&mut out, 0x2c, 24 * 512);
        u64_at(&mut out, 0x30, self.table);
        u32_at(&mut out, 0x48, 24 * 0x40 / 8);
        u64_at(&mut out, 0x4c, shared);
        out
    }
}

/// Only accelerator-derived aliases are outside the VM's kernel carveout.
pub(crate) fn fixed_ranges() -> [(u64, usize); 14] {
    let mut out = [(0, 0); 14];
    out[..3].copy_from_slice(&[
        (0x1000080000, 0x4000),
        (0x1000258000, 0x1c000),
        (0x1000240000, 0x4000),
    ]);
    for i in 0..11 {
        out[3 + i] = (0x1000088000 + i as u64 * 0x28000, 0x20000);
    }
    out
}

/// (accelerator alias, backing offset in the kernel carveout, extent).
pub(crate) fn private_aliases(pool: &OperandPool) -> [(u64, u64, usize); 40] {
    let mut out = [(0, 0, 0); 40];
    out[..5].copy_from_slice(&[
        (0x10_0008_0000, 0x3c000, 0x4000),
        (0x10_0025_8000, 0x3450000, 0x1c000),
        (0x10_0024_0000, 0x346c000, 0x4000),
        (pool.directory, 0x40000, 0x400000),
        (pool.table, 0x440000, 0x10000),
    ]);
    for i in 0..11 {
        out[5 + i] = (
            0x10_0008_8000 + i as u64 * 0x28000,
            0x3470000 + i as u64 * 0x20000,
            0x20000,
        );
    }
    for i in 0..24 {
        out[16 + i] = (
            pool.buffer + i as u64 * OPERAND_STRIDE,
            0x450000 + i as u64 * 0x200000,
            0x200000,
        );
    }
    out
}

/// Return one initialized backing page, without allocating a contiguous 54 MiB
/// CPU buffer. All unmapped padding and scratch begin at zero.
pub(crate) fn private_page(pool: &OperandPool, offset: usize, out: &mut [u8]) -> Option<()> {
    if offset & 0x3fff != 0 || offset >= PRIVATE_SIZE || out.len() != 0x4000 {
        return None;
    }
    out.fill(0);
    if offset == 0x38000 {
        u32_at(out, 0x600, 0x60000000);
        u32_at(out, 0x604, 0x35b);
    } else if (0x40000..0x58000).contains(&offset) {
        let first = (offset - 0x40000) / 8;
        for i in 0..0x800 {
            let page = first + i;
            u64_at(
                out,
                i * 8,
                pool.buffer + (page / 512) as u64 * OPERAND_STRIDE + (page % 512) as u64 * 0x1000,
            );
        }
    } else if offset == 0x440000 {
        for i in 0..24 {
            u64_at(
                out,
                i * 0x40,
                (pool.buffer + i as u64 * OPERAND_STRIDE) | (1 << 61),
            );
        }
    } else if offset == 0x3450000 {
        for block in 0..11 {
            for page in 0..4 {
                u32_at(
                    out,
                    (block * 4 + page) * 4,
                    (0x11 + block * 5 + page) as u32,
                );
            }
        }
    }
    Some(())
}

impl Parameters {
    pub(crate) fn set_private(&mut self, start: u64, end: u64) -> Option<()> {
        if start & 0x3fff != 0
            || start < CONTEXT_BASE
            || start.checked_add(PRIVATE_SIZE as u64)? > end.min(CONTEXT_BASE + (1 << 32))
        {
            return None;
        }
        OperandPool::new(start, end)?;
        self.context_base = CONTEXT_BASE;
        self.tilemap = start;
        self.heapmeta = start + HEAPMETA_OFFSET;
        self.tpc = start + 0x28000;
        self.deflake_1 = start + 0x2c2a0;
        self.deflake_2 = start + 0x2c020;
        self.deflake_3 = start + 0x2c000;
        self.ta_status = start + 0x30240;
        self.fragment_status = start + 0x342c0;
        self.aux_fb = start + 0x38000;
        Some(())
    }
}

impl Addresses {
    /// Initial device-owned render transport placement. All serialized links
    /// are derived from these allocations, independently of client resources.
    pub(crate) fn bootstrap() -> Self {
        Self {
            buffer_manager: 0xfffffc20c0678000,
            buffer_manager_block_control: 0xfffffc2000340000,
            buffer_manager_block_list: 0xfffffc20c0878000,
            buffer_manager_counter: 0xfffffc2000350000,
            buffer_manager_page_list: 0xfffffc20c0890000,
            buffer_manager_scene_list: 0xfffffc2000348000,
            buffer_manager_slot: 0x7,
            buffer_thing: 0xfffffc20c0600080,
            context_id: 0x9,
            empty_buffer: 0xfffffc20c0602800,
            event_control: 0xfffffc20c0588100,
            event_count_array: 0xfffffc20001b0000,
            fragment_counter: 0x0,
            fragment_driver_stamp: 0xfffffc2000118044,
            fragment_event: 0x11,
            fragment_firmware_stamp: 0xfffffc20c0510044,
            fragment_microsequence: 0xfffffc20c0469680,
            fragment_prelude: 0xfffffc20c029fbc0,
            fragment_queue: 0xfffffc20c0824980,
            fragment_register_alias: 0x7000141bc0,
            fragment_shared_tail: 0xfffffc2000358000,
            fragment_stamp: 0x100,
            fragment_stats: 0xfffffc20c04db2c8,
            fragment_status_page: 0x1000278000,
            fragment_user_timestamp_end: 0x0,
            fragment_user_timestamp_start: 0x0,
            fragment_uuid: 0x900a6ba,
            fragment_work: 0xfffffc20c0161b40,
            pool_slots: 0x1000080000,
            render_shared_state: 0xfffffc20001b8000,
            support: 0xfffffc20c05b8000,
            tiling_counter: 0x1,
            tiling_driver_stamp: 0xfffffc2000118040,
            tiling_event: 0x10,
            tiling_firmware_stamp: 0xfffffc20c0510040,
            tiling_microsequence: 0xfffffc20c03a2340,
            tiling_prelude: 0xfffffc20c0350600,
            tiling_queue: 0xfffffc20c08224c0,
            tiling_register_alias: 0x700004a980,
            tiling_shared_tail: 0xfffffc2000300000,
            tiling_stamp: 0xc00,
            tiling_stats: 0xfffffc20c04da684,
            tiling_status_page: 0x1000078000,
            tiling_user_timestamp_end: 0x0,
            tiling_user_timestamp_start: 0x0,
            tiling_uuid: 0x900a6bb,
            tiling_work: 0xfffffc20c006a940,
            timestamp_end: 0xfffffc2000034fe0,
            timestamp_start: 0xfffffc2000034fd8,
        }
    }
}

impl Addresses {
    pub(crate) fn private_regions(&self) -> [(u64, usize); 9] {
        [
            (self.buffer_manager, 0x4000),
            (self.buffer_manager_block_control, 0x4000),
            (self.buffer_manager_scene_list, 0x4000),
            (self.buffer_manager_counter, 0x4000),
            (self.buffer_thing & !0x3fff, 0x4000),
            (self.buffer_manager_block_list, 0x10000),
            (self.buffer_manager_page_list, 0x1c000),
            (self.event_control & !0x3fff, 0x4000),
            (self.support, 0x4000),
        ]
    }

    pub(crate) fn private_page(&self, address: u64, out: &mut [u8]) -> Option<()> {
        if out.len() != 0x4000 || address & 0x3fff != 0 {
            return None;
        }
        out.fill(0);
        if address == self.buffer_manager {
            for (off, value) in [
                (0xc, self.buffer_manager_slot as u32),
                (0x28, 0x258000),
                (0x2c, 0x10),
                (0x30, 0x1c000),
                (0x34, 0x2c),
                (0x38, 0x19b3),
                (0x3c, 0xb),
                (0x54, 0x2b),
                (0x58, 0x20000),
                (0x7c, 0x66cc),
                (0x80, 0x2244),
                (0x84, 0x248000),
            ] {
                u32_at(out, off, value);
            }
            for (off, value) in [
                (0x20, self.buffer_manager_page_list),
                (0x44, self.buffer_manager_block_list),
                (0x4c, self.buffer_manager_block_control),
                (0x64, self.buffer_manager_counter),
            ] {
                u64_at(out, off, value);
            }
        } else if address == self.buffer_manager_block_control {
            u32_at(out, 0, 11);
            u32_at(out, 4, 11);
            u32_at(out, 0x60, 1);
        } else if address == self.buffer_manager_counter {
            u32_at(out, 0, 1);
        } else if address == self.buffer_manager_block_list {
            for i in 0..11 {
                u32_at(out, i * 8, (0x11 + i * 5) as u32);
            }
        } else if address == self.buffer_manager_page_list {
            for i in 0..11 {
                for j in 0..4 {
                    u32_at(out, (i * 4 + j) * 4, (0x11 + i * 5 + j) as u32);
                }
            }
        } else if address == self.buffer_thing & !0x3fff {
            for i in 0..80 {
                for (off, value) in [
                    (0, self.pool_slots + i as u64 * 4),
                    (8, self.buffer_manager_scene_list + i as u64 * 4),
                    (0x28, 0x240000 + (i as u64 % 36) * 0x30),
                    (0x40, self.buffer_manager_block_control + 0x40),
                    (0x48, 0x100000000),
                ] {
                    u64_at(out, i * 0x80 + off, value);
                }
            }
        } else if address == self.event_control & !0x3fff {
            for i in 0..36 {
                u64_at(out, i * 0x100, self.event_count_array + i as u64 * 4);
            }
            u32_at(out, (self.event_control as usize & 0x3fff) + 0x10, 0x50);
        }
        Some(())
    }
}

/// Reserved support leases; Work storage is allocated per publication.
pub(crate) const SUPPORT_BASE: u64 = 0xffff_fc20_c300_0000;

impl Addresses {
    pub(crate) fn publication(&self, ordinal: u64) -> Option<Self> {
        if ordinal > 0x00ff_fff0 {
            return None;
        }
        let mut a = *self;
        a.tiling_counter += ordinal * 2;
        a.fragment_counter += ordinal * 2;
        a.tiling_stamp += ordinal * 0x100;
        a.fragment_stamp += ordinal * 0x100;
        a.buffer_thing = (self.buffer_thing & !0x3fff) + ((ordinal + 1) % 80) * 0x80;
        Some(a)
    }
}

impl Addresses {
    pub(crate) fn tiling_prelude(&self, blocks: u32) -> [u8; 0x40] {
        let mut out = [0; 0x40];
        for (off, value) in [
            (0, 6),
            (4, self.context_id as u32),
            (8, self.buffer_manager_slot as u32),
            (0x10, blocks),
            (0x1c, self.tiling_stamp as u32),
        ] {
            u32_at(&mut out, off, value);
        }
        u64_at(&mut out, 0x14, self.buffer_manager);
        out
    }
    pub(crate) fn fragment_prelude(&self) -> [u8; 0x40] {
        let mut out = [0; 0x40];
        u32_at(&mut out, 0, 4);
        u64_at(&mut out, 4, self.tiling_firmware_stamp);
        u64_at(&mut out, 0xc, self.tiling_firmware_stamp);
        for (off, value) in [
            (0x14, self.tiling_stamp),
            (0x20, self.tiling_event),
            (0x24, self.fragment_stamp),
            (0x28, self.fragment_uuid),
        ] {
            u32_at(&mut out, off, value as u32);
        }
        out
    }
}

impl Parameters {
    /// Bytes of tilemap and TPC storage required by this pass. The 16
    /// macrotiles each carry their own region and pointer strides.
    pub(crate) fn scratch_sizes(&self) -> Option<(usize, usize)> {
        if !self.valid() {
            return None;
        }
        let g = self.geometry();
        Some((
            ((g.size1 * 64 * self.layers + 0x3fff) & !0x3fff) as usize,
            ((g.size3 * 64 * self.layers + 0x3fff) & !0x3fff) as usize,
        ))
    }
}
