// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Ordered G16C render programs. Inputs reuse the common render command;
//! firmware lifecycle/gate/partial-pass scalars stay explicit owner state.
//! Register pairs are CPU staging, not repr(C) firmware structs.

use super::{Error, Parameters, Register};

#[derive(Clone, Copy)]
pub(crate) struct ProgramStateG16C {
    pub(crate) layermeta: u64,
    pub(crate) cycle: u64,
    pub(crate) scene_slot: u64,
    pub(crate) tiling_record_index: Option<u64>,
    pub(crate) tiling_admission: u64,
    pub(crate) fragment_admission: u64,
    pub(crate) tiling_gate: u64,
    pub(crate) fragment_gate: u64,
    pub(crate) tiling_lifecycle: u64,
    pub(crate) fragment_lifecycle: u64,
    pub(crate) tiling_work_stamp: u64,
    pub(crate) fragment_work_stamp: u64,
    pub(crate) tiling_control: u64,
    pub(crate) partial_load_tile_config: Option<u64>,
    pub(crate) partial_load_depth_flags: Option<u64>,
    pub(crate) partial_resume_depth_flags: Option<u64>,
    pub(crate) stencil_clear_flags: u64,
    pub(crate) partial_stencil_clear_flags: u64,
    pub(crate) vertex_helper_binary: u64,
    pub(crate) vertex_helper_data: u64,
    pub(crate) vertex_helper_config: u64,
    pub(crate) fragment_helper_binary: u64,
    pub(crate) fragment_helper_data: u64,
    pub(crate) fragment_helper_config: u64,
}
impl Default for ProgramStateG16C {
    fn default() -> Self {
        Self {
            layermeta: 0,
            cycle: 0,
            scene_slot: 0,
            tiling_record_index: None,
            tiling_admission: 0,
            fragment_admission: 0,
            tiling_gate: 0,
            fragment_gate: 0,
            tiling_lifecycle: 0,
            fragment_lifecycle: 0,
            tiling_work_stamp: 0,
            fragment_work_stamp: 0,
            tiling_control: 1,
            partial_load_tile_config: None,
            partial_load_depth_flags: None,
            partial_resume_depth_flags: None,
            stencil_clear_flags: 0x300,
            partial_stencil_clear_flags: 0x300,
            vertex_helper_binary: 0,
            vertex_helper_data: 0,
            vertex_helper_config: 0,
            fragment_helper_binary: 0,
            fragment_helper_data: 0,
            fragment_helper_config: 0,
        }
    }
}

fn offset(p: &Parameters, address: u64) -> Result<u64, Error> {
    let value = address.checked_sub(p.context_base).ok_or(Error::Invalid)?;
    if value > u32::MAX as u64 {
        return Err(Error::Invalid);
    }
    Ok(value)
}
fn validate(p: &Parameters, s: &ProgramStateG16C) -> Result<(), Error> {
    p.validate()?;
    if p.tib_blocks > 128
        || s.scene_slot >= 80
        || (p.samples == 4 && (p.utile_width, p.utile_height) == (32, 32))
        || (!p.emit_uapi_fields
            && [
                s.vertex_helper_binary,
                s.vertex_helper_data,
                s.vertex_helper_config,
                s.fragment_helper_binary,
                s.fragment_helper_data,
                s.fragment_helper_config,
            ]
            .iter()
            .any(|&value| value != 0))
    {
        return Err(Error::Invalid);
    }
    for address in [
        p.tilemap,
        p.heapmeta,
        if s.layermeta == 0 {
            p.heapmeta
        } else {
            s.layermeta
        },
        p.tpc,
        p.deflake_1,
        p.deflake_2,
        p.deflake_3,
        p.encoder,
    ] {
        offset(p, address)?;
    }
    Ok(())
}
pub(crate) fn tib_encoding(p: &Parameters) -> u64 {
    let blocks = p.tib_blocks & 0x3f;
    if p.samples == 1 {
        return blocks;
    }
    let quantum = ((p.utile_width * p.utile_height * p.samples * p.samples + 2047) / 2048).max(1);
    let groups = ((p.tib_blocks * p.samples + 32 * quantum - 1) / (32 * quantum)).max(1);
    quantum * groups << 16 | blocks
}

pub(crate) struct TilingProgramG16C {
    registers: [Register; 87],
    count: usize,
}
impl Default for TilingProgramG16C {
    fn default() -> Self {
        Self {
            count: 87,
            registers: [
                (0x10141, 0x200),
                (0x1c039, 0x0),
                (0x1c9c8, 0x0),
                (0x1c041, 0x0),
                (0x1c9d0, 0x0),
                (0x1c0a1, 0x0),
                (0x1c031, 0x0),
                (0x1c9c0, 0x0),
                (0x1c051, 0x3a0012006b0003),
                (0x1c061, 0x1),
                (0x10149, 0x0),
                (0x10139, 0x0),
                (0x10111, 0x0),
                (0x1c9b0, 0x0),
                (0x10119, 0x0),
                (0x1c9b8, 0x0),
                (0x1c958, 0x1),
                (0x1c950, 0x0),
                (0x1c930, 0x0),
                (0x1c880, 0x0),
                (0x1c948, 0x0),
                (0x1c888, 0x0),
                (0x1c890, 0x0),
                (0x1c918, 0x0),
                (0x1c079, 0x0),
                (0x1c9d8, 0x0),
                (0x1c089, 0x0),
                (0x1c9e0, 0x0),
                (0x16c41, 0x0),
                (0x1ca40, 0x0),
                (0x1c9a8, 0x5c),
                (0x1c920, 0x0),
                (0x10151, 0x0),
                (0x1c199, 0x0),
                (0x1c1a1, 0x0),
                (0x1c1a9, 0x0),
                (0x1c1b1, 0x0),
                (0x1c1b9, 0x0),
                (0x1c8f8, 0x8860),
                (0x10061, 0x0),
                (0x11801, 0x0),
                (0x11809, 0x0),
                (0x11f71, 0x0),
                (0x1c0b1, 0x0),
                (0x1c850, 0x0),
                (0x10131, 0x88),
                (0x10121, 0x0),
                (0x10129, 0x0),
                (0x101b9, 0x0),
                (0x1c069, 0x0),
                (0x1c071, 0x0),
                (0x1c081, 0x0),
                (0x1c0a9, 0x0),
                (0x10171, 0x100),
                (0x10169, 0x0),
                (0x0a309, 0x0),
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
                (0x0d471, 0x0),
                (0x1a0f1, 0x8),
                (0x10799, 0xff0000),
                (0x1c830, 0x0),
                (0x1ca30, 0x0),
                (0x16c39, 0x0),
                (0x1c910, 0x0),
                (0x0a5a1, 0x0),
                (0x0d419, 0x200000001),
                (0x1ca10, 0x0),
                (0x014a1, 0x0),
                (0x0a349, 0x0),
                (0x10209, 0x0),
                (0x1c9f0, 0x0),
                (0x14320, 0x0),
                (0x14308, 0x0),
                (0x14318, 0x0),
                (0x1c880, 0x0),
                (0x1c898, 0x1),
            ],
        }
    }
}
impl TilingProgramG16C {
    pub(crate) fn new(p: &Parameters, s: &ProgramStateG16C) -> Result<Self, Error> {
        validate(p, s)?;
        let g = p.geometry()?;
        let layermeta = if s.layermeta == 0 {
            p.heapmeta
        } else {
            s.layermeta
        };
        let tilemap = offset(p, p.tilemap)?;
        let heapmeta = offset(p, p.heapmeta)?;
        let layermeta = offset(p, layermeta)?;
        let deflake_1 = offset(p, p.deflake_1)?;
        let target_layers = if p.layers > 1 {
            0xd000 | p.layers - 1
        } else {
            0x8000
        };
        let record_index = s.tiling_record_index.unwrap_or(0x80001 + s.scene_slot * 4);
        let mut raw = Self::default();
        raw.registers[1].1 = tilemap;
        raw.registers[2].1 = tilemap;
        raw.registers[5].1 = offset(p, p.tpc)?;
        raw.registers[6].1 = heapmeta | 9223372036854775808;
        raw.registers[7].1 = heapmeta | 9223372036854775808;
        raw.registers[10].1 = p.utile_config;
        raw.registers[11].1 = p.multisample_control;
        raw.registers[12].1 = deflake_1;
        raw.registers[13].1 = deflake_1;
        raw.registers[14].1 = offset(p, p.deflake_2)?;
        raw.registers[15].1 = offset(p, p.deflake_2)?;
        raw.registers[17].1 = offset(p, p.deflake_3)? | 1125899906842624;
        raw.registers[19].1 = offset(p, p.encoder)?;
        raw.registers[22].1 = s.tiling_control;
        raw.registers[24].1 = layermeta;
        raw.registers[25].1 = layermeta;
        raw.registers[39].1 = p.usc_exec_base;
        raw.registers[40].1 = s.vertex_helper_binary;
        raw.registers[41].1 = s.vertex_helper_data;
        raw.registers[42].1 = s.vertex_helper_config;
        raw.registers[43].1 = g.size1;
        raw.registers[44].1 = g.size1;
        raw.registers[46].1 = p.ppp_control;
        raw.registers[47].1 = g.pixels;
        raw.registers[48].1 = g.screen;
        raw.registers[49].1 = g.x_blocks;
        raw.registers[50].1 = g.y_blocks;
        raw.registers[51].1 = g.size2;
        raw.registers[52].1 = g.size3;
        raw.registers[54].1 = target_layers;
        raw.registers[60].1 = target_layers & 20479;
        raw.registers[71].1 = s.tiling_admission;
        raw.registers[72].1 = s.cycle;
        raw.registers[73].1 = s.cycle;
        raw.registers[74].1 = record_index;
        raw.registers[75].1 = s.tiling_gate;
        raw.registers[77].1 = s.tiling_lifecycle;
        raw.registers[78].1 = s.tiling_lifecycle;
        raw.registers[79].1 = s.tiling_lifecycle;
        raw.registers[80].1 = s.tiling_work_stamp;
        raw.registers[81].1 = s.tiling_work_stamp;
        raw.registers[82].1 = s.tiling_work_stamp;
        raw.registers[83].1 = p.completion_control;
        raw.registers[84].1 = p.ta_status | 1;
        raw.registers[85].1 = deflake_1;
        if !p.emit_uapi_fields {
            raw.registers.copy_within(43..87, 39);
            raw.registers[83..].fill((0, 0));
            raw.count -= 4;
        }
        Ok(raw)
    }
    pub(crate) fn registers(&self) -> &[Register] {
        &self.registers[..self.count]
    }
}

pub(crate) struct FragmentProgramG16C {
    registers: [Register; 95],
    count: usize,
}
impl Default for FragmentProgramG16C {
    fn default() -> Self {
        Self {
            count: 95,
            registers: [
                (0x01739, 0x1),
                (0x10009, 0x0),
                (0x15379, 0x0),
                (0x15381, 0x0),
                (0x15369, 0x0),
                (0x15371, 0x0),
                (0x15131, 0x0),
                (0x15139, 0x0),
                (0x100a1, 0x0),
                (0x15069, 0x0),
                (0x15071, 0x0),
                (0x16058, 0x0),
                (0x10019, 0x0),
                (0x100b1, 0x0),
                (0x16030, 0x0),
                (0x100d9, 0x0),
                (0x0a301, 0x0),
                (0x10791, 0xff0200),
                (0x16098, 0x0),
                (0x15109, 0x0),
                (0x15101, 0x0),
                (0x15021, 0x0),
                (0x15211, 0x0),
                (0x15049, 0x0),
                (0x10051, 0x0),
                (0x15321, 0x0),
                (0x15301, 0x0),
                (0x15309, 0x0),
                (0x15311, 0x0),
                (0x15319, 0x0),
                (0x15349, 0x4040404),
                (0x15351, 0x0),
                (0x15329, 0x0),
                (0x15331, 0x0),
                (0x15339, 0x0),
                (0x15341, 0x0),
                (0x15231, 0x0),
                (0x15221, 0x0),
                (0x15239, 0x0),
                (0x15229, 0x0),
                (0x15401, 0x0),
                (0x15421, 0x0),
                (0x15409, 0x0),
                (0x15429, 0x0),
                (0x153c1, 0x0),
                (0x15411, 0x0),
                (0x153c9, 0x0),
                (0x15431, 0x0),
                (0x153d1, 0x0),
                (0x15419, 0x0),
                (0x153d9, 0x0),
                (0x15439, 0x0),
                (0x16429, 0x0),
                (0x16060, 0x0),
                (0x16431, 0x0),
                (0x10039, 0x0),
                (0x16020, 0x0),
                (0x10069, 0x0),
                (0x11821, 0x0),
                (0x11829, 0x0),
                (0x11f79, 0x0),
                (0x16451, 0x0),
                (0x15359, 0x0),
                (0x100b8, 0x8860),
                (0x16461, 0x0),
                (0x16090, 0x0),
                (0x101e9, 0x1c),
                (0x160a8, 0x0),
                (0x16068, 0x0),
                (0x160b8, 0x0),
                (0x1a0a9, 0x0),
                (0x1a0b1, 0x0),
                (0x1a079, 0x0),
                (0x1a081, 0x0),
                (0x1a0d9, 0x0),
                (0x1a0e1, 0x0),
                (0x101c1, 0x0),
                (0x0d469, 0x0),
                (0x1a0f9, 0x8),
                (0x0a5a9, 0x0),
                (0x0d429, 0x200000001),
                (0x160e0, 0x0),
                (0x01499, 0x0),
                (0x0a341, 0x0),
                (0x1c838, 0x0),
                (0x1ca28, 0x0),
                (0x10211, 0x0),
                (0x10420, 0x0),
                (0x14048, 0x0),
                (0x14080, 0x0),
                (0x01731, 0x1),
                (0x16020, 0x1),
                (0x16020, 0x0),
                (0x160a8, 0x20),
                (0x16068, 0x40000),
            ],
        }
    }
}
impl FragmentProgramG16C {
    pub(crate) fn new(p: &Parameters, s: &ProgramStateG16C) -> Result<Self, Error> {
        validate(p, s)?;
        let g = p.geometry()?;
        let layermeta = if s.layermeta == 0 {
            p.heapmeta
        } else {
            s.layermeta
        };
        let tile_state = 0x3717f
            | (g.tiles_x - 1) << 44
            | (g.tiles_y - 1) << 53
            | 0x2000000000
            | if p.layers > 1 { 0x100000000 } else { 0 }
            | (p.utile_config & 0xf000) << 28;
        let mut raw = Self::default();
        raw.registers[1].1 = p.utile_config;
        raw.registers[2].1 = p.store_pipeline_bind;
        raw.registers[3].1 = p.store_pipeline;
        raw.registers[4].1 = p.load_pipeline_bind;
        raw.registers[5].1 = p.load_pipeline;
        raw.registers[6].1 = p.merge_upper_x_bits;
        raw.registers[7].1 = p.merge_upper_y_bits;
        raw.registers[12].1 = p.multisample_control;
        raw.registers[13].1 = g.macro_size;
        raw.registers[14].1 = g.macro_size;
        raw.registers[15].1 = g.screen;
        raw.registers[18].1 = p.heapmeta;
        raw.registers[19].1 = p.scissor_array;
        raw.registers[20].1 = p.depth_bias_array;
        raw.registers[21].1 = p.aux_fb_flags;
        raw.registers[22].1 = (p.height << 32) | p.width;
        raw.registers[23].1 = p.aux_fb_page_count;
        raw.registers[24].1 = tib_encoding(p);
        raw.registers[25].1 = p.depth_dimensions;
        raw.registers[26].1 = p.depth_clear_value_bits;
        raw.registers[27].1 = p.stencil_clear_value | s.stencil_clear_flags;
        raw.registers[28].1 = p.occlusion_query_base;
        raw.registers[29].1 = p.depth_flags;
        raw.registers[32].1 = p.depth_buffer;
        raw.registers[33].1 = p.depth_buffer;
        raw.registers[34].1 = p.stencil_buffer;
        raw.registers[35].1 = p.stencil_buffer;
        raw.registers[40].1 = p.depth_stride;
        raw.registers[41].1 = p.depth_stride;
        raw.registers[42].1 = p.stencil_stride;
        raw.registers[43].1 = p.stencil_stride;
        raw.registers[44].1 = p.depth_aux_buffer;
        raw.registers[45].1 = p.depth_aux_stride;
        raw.registers[46].1 = p.depth_aux_buffer;
        raw.registers[47].1 = p.depth_aux_stride;
        raw.registers[48].1 = p.stencil_aux_buffer;
        raw.registers[49].1 = p.stencil_aux_stride;
        raw.registers[50].1 = p.stencil_aux_buffer;
        raw.registers[51].1 = p.stencil_aux_stride;
        raw.registers[52].1 = p.tilemap;
        raw.registers[53].1 = layermeta;
        raw.registers[54].1 = (4 * g.size1) << 24;
        raw.registers[55].1 = p.tile_config;
        raw.registers[57].1 = p.usc_exec_base;
        raw.registers[58].1 = s.fragment_helper_binary;
        raw.registers[59].1 = s.fragment_helper_data;
        raw.registers[60].1 = s.fragment_helper_config;
        raw.registers[64].1 = p.aux_fb;
        raw.registers[65].1 = p.aux_fb;
        raw.registers[68].1 = (tile_state & !131071) | 90129;
        raw.registers[79].1 = s.fragment_gate;
        raw.registers[81].1 = s.fragment_lifecycle;
        raw.registers[82].1 = s.fragment_lifecycle;
        raw.registers[83].1 = s.fragment_lifecycle;
        raw.registers[84].1 = s.fragment_admission;
        raw.registers[85].1 = s.cycle;
        raw.registers[86].1 = s.fragment_work_stamp;
        raw.registers[87].1 = s.fragment_work_stamp;
        raw.registers[88].1 = p.completion_control;
        raw.registers[89].1 = p.fragment_status | 1;
        if !p.emit_uapi_fields {
            raw.registers.copy_within(61..95, 57);
            raw.registers[91..].fill((0, 0));
            raw.count -= 4;
        }
        Ok(raw)
    }
    pub(crate) fn registers(&self) -> &[Register] {
        &self.registers[..self.count]
    }
}

struct PartialTemplatesG16C {
    store: [Register; 16],
    resume: [Register; 23],
    load: [Register; 10],
}
impl PartialTemplatesG16C {
    const fn template() -> Self {
        Self {
            store: [
                (0x15379, 0x0),
                (0x15381, 0x0),
                (0x10039, 0x0),
                (0x15359, 0x20),
                (0x15331, 0x0),
                (0x153c9, 0x0),
                (0x15341, 0x0),
                (0x153d9, 0x0),
                (0x15421, 0x0),
                (0x15431, 0x0),
                (0x15429, 0x0),
                (0x15439, 0x0),
                (0x15221, 0x0),
                (0x15229, 0x0),
                (0x15319, 0x0),
                (0x15349, 0x4040404),
            ],
            resume: [
                (0x15379, 0x0),
                (0x15381, 0x0),
                (0x15369, 0x0),
                (0x15371, 0x0),
                (0x10039, 0x0),
                (0x15359, 0x20),
                (0x15331, 0x0),
                (0x153c9, 0x0),
                (0x15341, 0x0),
                (0x153d9, 0x0),
                (0x15421, 0x0),
                (0x15431, 0x0),
                (0x15429, 0x0),
                (0x15439, 0x0),
                (0x15221, 0x0),
                (0x15229, 0x0),
                (0x15309, 0x0),
                (0x15329, 0x0),
                (0x153c1, 0x0),
                (0x15339, 0x0),
                (0x153d1, 0x0),
                (0x15319, 0x0),
                (0x15349, 0x4040404),
            ],
            load: [
                (0x15369, 0x0),
                (0x15371, 0x0),
                (0x10039, 0x0),
                (0x15309, 0x0),
                (0x15329, 0x0),
                (0x153c1, 0x0),
                (0x15339, 0x0),
                (0x153d1, 0x0),
                (0x15319, 0x0),
                (0x15349, 0x4040404),
            ],
        }
    }
}
impl Default for PartialTemplatesG16C {
    fn default() -> Self {
        Self::template()
    }
}
static PARTIAL_TEMPLATES_G16C: PartialTemplatesG16C = PartialTemplatesG16C::template();
pub(crate) fn partial_store(p: &Parameters, s: &ProgramStateG16C) -> Result<[Register; 16], Error> {
    validate(p, s)?;
    let mut out = PARTIAL_TEMPLATES_G16C.store;
    out[0].1 = p.partial_store_pipeline_bind;
    out[1].1 = p.partial_store_pipeline;
    out[2].1 = p.tile_config;
    out[4].1 = p.depth_buffer;
    out[5].1 = p.depth_aux_buffer;
    out[6].1 = p.stencil_buffer;
    out[7].1 = p.stencil_aux_buffer;
    out[8].1 = p.depth_stride;
    out[9].1 = p.depth_aux_stride;
    out[10].1 = p.stencil_stride;
    out[11].1 = p.stencil_aux_stride;
    out[14].1 = p.depth_flags;
    Ok(out)
}

pub(crate) fn partial_resume(
    p: &Parameters,
    s: &ProgramStateG16C,
) -> Result<[Register; 23], Error> {
    validate(p, s)?;
    let mut out = PARTIAL_TEMPLATES_G16C.resume;
    out[0].1 = p.partial_store_pipeline_bind;
    out[1].1 = p.partial_store_pipeline;
    out[2].1 = p.partial_load_pipeline_bind;
    out[3].1 = p.partial_load_pipeline;
    out[4].1 = p.tile_config & 65535;
    out[6].1 = p.depth_buffer;
    out[7].1 = p.depth_aux_buffer;
    out[8].1 = p.stencil_buffer;
    out[9].1 = p.stencil_aux_buffer;
    out[10].1 = p.depth_stride;
    out[11].1 = p.depth_aux_stride;
    out[12].1 = p.stencil_stride;
    out[13].1 = p.stencil_aux_stride;
    out[16].1 = p.stencil_clear_value | s.partial_stencil_clear_flags;
    out[17].1 = p.depth_buffer;
    out[18].1 = p.depth_aux_buffer;
    out[19].1 = p.stencil_buffer;
    out[20].1 = p.stencil_aux_buffer;
    out[21].1 = s.partial_resume_depth_flags.unwrap_or(p.depth_flags);
    Ok(out)
}

pub(crate) fn partial_load(p: &Parameters, s: &ProgramStateG16C) -> Result<[Register; 10], Error> {
    validate(p, s)?;
    let mut out = PARTIAL_TEMPLATES_G16C.load;
    out[0].1 = p.partial_load_pipeline_bind;
    out[1].1 = p.partial_load_pipeline;
    out[2].1 = s.partial_load_tile_config.unwrap_or(p.tile_config & 65535);
    out[3].1 = p.stencil_clear_value | s.partial_stencil_clear_flags;
    out[4].1 = p.depth_buffer;
    out[5].1 = p.depth_aux_buffer;
    out[6].1 = p.stencil_buffer;
    out[7].1 = p.stencil_aux_buffer;
    out[8].1 = s.partial_load_depth_flags.unwrap_or(p.depth_flags);
    Ok(out)
}

/// Lower the common command into checked Max body inputs. Explicit TPC size
/// zero selects geometry-derived storage; nonzero values remain caller policy.
pub(crate) fn descriptor_parameters(
    p: &Parameters,
    s: &ProgramStateG16C,
    tpc_size: u64,
) -> Result<super::wire::Parameters, Error> {
    validate(p, s)?;
    let g = p.geometry()?;
    let word = |value: u64| u32::try_from(value).map_err(|_| Error::Overflow);
    Ok(super::wire::Parameters {
        width: word(p.width)?,
        height: word(p.height)?,
        samples: word(p.samples)?,
        tiles: g.tiles_x * g.tiles_y,
        tpc: p.tpc,
        tpc_size: if tpc_size == 0 {
            p.scratch_layout()?.2
        } else {
            tpc_size
        },
        sampler_array: p.sampler_array,
        sampler_count: word(p.sampler_count)?,
        request_growth: p.reactive_tvb_growth,
        sync_grow: p.fragment_sync_grow.unwrap_or(false),
        process_empty_tiles: p.process_empty_tiles,
        depth_bias_array: p.depth_bias_array,
        scissor_array: p.scissor_array,
        occlusion_query_base: p.occlusion_query_base,
        load_pipeline_bind: p.load_pipeline_bind,
        load_pipeline: p.load_pipeline,
        partial_load_pipeline_bind: p.partial_load_pipeline_bind,
        partial_load_pipeline: p.partial_load_pipeline,
        depth_flags: p.depth_flags,
        depth_buffer: p.depth_buffer,
        depth_stride: p.depth_stride,
        depth_aux_stride: p.depth_aux_stride,
        depth_aux_buffer: p.depth_aux_buffer,
        stencil_buffer: p.stencil_buffer,
        stencil_stride: p.stencil_stride,
        stencil_aux_stride: p.stencil_aux_stride,
        stencil_aux_buffer: p.stencil_aux_buffer,
        tib_encoding: tib_encoding(p),
        aux_fb_flags: p.aux_fb_flags,
        aux_fb_page_count: p.aux_fb_page_count,
        tile_config: p.tile_config,
        store_pipeline_bind: word(p.store_pipeline_bind)?,
        store_pipeline: p.store_pipeline,
        partial_store_pipeline_bind: word(p.partial_store_pipeline_bind)?,
        partial_store_pipeline: p.partial_store_pipeline,
        depth_clear_value_bits: word(p.depth_clear_value_bits)?,
        stencil_clear: word(p.stencil_clear_value | s.stencil_clear_flags)?,
        sample_size: word(if p.sample_size == 0 {
            p.tib_blocks * 2
        } else {
            p.sample_size
        })?,
        depth_dimensions: p.depth_dimensions,
    })
}
