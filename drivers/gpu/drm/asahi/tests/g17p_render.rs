// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]
#[path = "../g17p_compute.rs"]
mod g17p_compute;
#[path = "../g17p_render.rs"]
mod g17p_render;
use g17p_compute::{Error, Register};
use g17p_render::*;
use std::io::{Read, Write};
fn emit(body: &[u8]) {
    std::io::stdout().write_all(body).unwrap();
}
fn regs(values: &[Register]) {
    for &(n, v) in values {
        emit(&n.to_le_bytes());
        emit(&v.to_le_bytes());
    }
}
fn parameters(words: &mut impl Iterator<Item = u64>) -> Parameters {
    Parameters {
        width: words.next().unwrap(),
        height: words.next().unwrap(),
        context_base: words.next().unwrap(),
        tilemap: words.next().unwrap(),
        heapmeta: words.next().unwrap(),
        tpc: words.next().unwrap(),
        deflake_1: words.next().unwrap(),
        deflake_2: words.next().unwrap(),
        deflake_3: words.next().unwrap(),
        encoder: words.next().unwrap(),
        ta_status: words.next().unwrap(),
        store_pipeline_bind: words.next().unwrap(),
        store_pipeline: words.next().unwrap(),
        load_pipeline_bind: words.next().unwrap(),
        load_pipeline: words.next().unwrap(),
        scissor_array: words.next().unwrap(),
        depth_bias_array: words.next().unwrap(),
        aux_fb: words.next().unwrap(),
        fragment_status: words.next().unwrap(),
        layers: words.next().unwrap(),
        utile_width: words.next().unwrap(),
        utile_height: words.next().unwrap(),
        samples: words.next().unwrap(),
        sample_size: words.next().unwrap(),
        occlusion_query_base: words.next().unwrap(),
        depth_stride: words.next().unwrap(),
        stencil_stride: words.next().unwrap(),
        depth_aux_stride: words.next().unwrap(),
        stencil_aux_stride: words.next().unwrap(),
        merge_upper_x_bits: words.next().unwrap(),
        merge_upper_y_bits: words.next().unwrap(),
        partial_load_pipeline_bind: words.next().unwrap(),
        partial_load_pipeline: words.next().unwrap(),
        partial_store_pipeline_bind: words.next().unwrap(),
        partial_store_pipeline: words.next().unwrap(),
        sampler_array: words.next().unwrap(),
        sampler_count: words.next().unwrap(),
        process_empty_tiles: words.next().unwrap() != 0,
        fragment_sync_grow: match words.next().unwrap() {
            u64::MAX => None,
            v => Some(v != 0),
        },
        reactive_tvb_growth: words.next().unwrap() != 0,
        tvb_pool_id: match words.next().unwrap() {
            u64::MAX => None,
            v => Some(v),
        },
        emit_uapi_fields: words.next().unwrap() != 0,
        vertex_store_flag: words.next().unwrap() != 0,
        fragment_store_flag: words.next().unwrap() != 0,
        usc_exec_base: words.next().unwrap(),
        timestamp_a: words.next().unwrap(),
        timestamp_b: words.next().unwrap(),
        ta_timestamp_end: words.next().unwrap(),
        fragment_timestamp_start: words.next().unwrap(),
        fragment_timestamp_end: words.next().unwrap(),
        ta_user_timestamp_start: words.next().unwrap(),
        ta_user_timestamp_end: words.next().unwrap(),
        fragment_user_timestamp_start: words.next().unwrap(),
        fragment_user_timestamp_end: words.next().unwrap(),
        depth_buffer: words.next().unwrap(),
        stencil_buffer: words.next().unwrap(),
        depth_aux_buffer: words.next().unwrap(),
        stencil_aux_buffer: words.next().unwrap(),
        depth_clear_value_bits: words.next().unwrap(),
        stencil_clear_value: words.next().unwrap(),
        depth_flags: words.next().unwrap(),
        depth_dimensions: words.next().unwrap(),
        utile_config: words.next().unwrap(),
        multisample_control: words.next().unwrap(),
        ppp_control: words.next().unwrap(),
        tib_blocks: words.next().unwrap(),
        tile_config: words.next().unwrap(),
        aux_fb_flags: words.next().unwrap(),
        aux_fb_page_count: words.next().unwrap(),
        lifecycle_ordinal: words.next().unwrap(),
        native_context_slot: match words.next().unwrap() {
            u64::MAX => None,
            v => Some(v),
        },
        queue_pair: words.next().unwrap(),
        queue_item_index: words.next().unwrap(),
        status_queue_pair: match words.next().unwrap() {
            u64::MAX => None,
            v => Some(v),
        },
        status_item_index: match words.next().unwrap() {
            u64::MAX => None,
            v => Some(v),
        },
        native_cycle_registers: words.next().unwrap() != 0,
        pair_resource_stride: words.next().unwrap(),
        native_record_index_register: words.next().unwrap() != 0,
        native_pair_registers: words.next().unwrap() != 0,
        native_status_registers: words.next().unwrap() != 0,
        local_item_registers: words.next().unwrap() != 0,
        native_item_fields: words.next().unwrap() != 0,
    }
}
fn main() {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    let mut words = input
        .chunks_exact(8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()));
    let count = words.next().unwrap();
    for case in 0..count {
        let p = parameters(&mut words);
        let ta = tiling_registers(&p).unwrap();
        let frag = fragment_registers(&p).unwrap();
        regs(&ta);
        regs(&frag);
        regs(&partial_store_registers(&p).unwrap());
        regs(&partial_resume_registers(&p).unwrap());
        regs(&partial_load_registers(&p).unwrap());
        let mut program = vec![0xa5; 32 * 12];
        class4_program(&mut program, &frag).unwrap();
        emit(&program);
        let saved = program.clone();
        assert_eq!(
            class4_program(&mut program, &frag[..60]),
            Err(Error::MissingRegister)
        );
        assert_eq!(program, saved);
        let operand = 0x7000048000 + case * 0x4000;
        let firmware = 0xfffffc20c0998000 + case * 0x4000;
        let mut support = vec![0xa5; SUPPORT_SIZE];
        class2_prestate(
            &mut support,
            operand,
            firmware,
            0x7000058080 + case * 16,
            (case % 4) as u32,
        )
        .unwrap();
        emit(&support);
        let saved = support.clone();
        assert_eq!(
            class2_prestate(&mut support, operand, firmware, 0x7100058000, 3),
            Err(Error::Invalid)
        );
        assert_eq!(support, saved);
        for active in [false, true] {
            class4_state(&mut support, operand, firmware, (case % 3) as u32, active).unwrap();
            emit(&support);
        }
        for kind in [Kind::Tiling, Kind::Fragment] {
            let mut registers = if kind == Kind::Tiling {
                ta.to_vec()
            } else {
                frag.to_vec()
            };
            if case % 3 == 1 {
                registers.push((
                    if kind == Kind::Tiling {
                        0x1ca10
                    } else {
                        0x160e0
                    },
                    0x123456789abcdef0,
                ));
            }
            if case % 7 == 2 {
                registers.push((
                    if kind == Kind::Tiling {
                        0x10111
                    } else {
                        0x15131
                    },
                    0xfeed00881234,
                ));
            }
            let pointers = [
                (if kind == Kind::Tiling { 0x934 } else { 0x21ce }, firmware),
                (
                    if kind == Kind::Tiling { 0x8a6 } else { 0x2140 },
                    0xfffffc2000120000 + case * 8,
                ),
            ];
            let items = [(
                if kind == Kind::Tiling { 0x8c4 } else { 0x2160 },
                (case * 513 + 1) as u32,
            )];
            let mut d = Descriptor {
                kind,
                index: (case % 37) as u32,
                sequence: case << 40 | case * 2 + u64::from(kind == Kind::Tiling),
                ordinal: (case * 3) as u32,
                context: if p.native_context_slot == Some(1) {
                    1
                } else if p.native_context_slot == Some(2) {
                    2
                } else {
                    (case % 4) as u32
                },
                queue_pair: p.queue_pair as u32,
                pool_bases: [0xfffffc20c0834000, 0xfffffc20c0858000],
                record_indices: [(case * 2) as u32, (case * 3) as u32],
                shared: [0xfffffc20c0920000, 0xfffffc20c0924000],
                low_alias: if case % 3 == 0 {
                    None
                } else {
                    Some(0x7002000000 + case * 0x4000)
                },
                status_base: if case % 2 == 0 {
                    None
                } else {
                    Some(0xfffffc2001770000 + case * 0x4000)
                },
                grid: if case % 3 == 0 {
                    None
                } else {
                    Some((case % 12) as u32)
                },
                write_tail: case % 11 != 0,
                write_lifecycle: case % 4 != 1,
                write_item: case % 4 != 2,
                write_structural: case % 4 != 3,
                pointer_overrides: if case % 2 == 0 { &[] } else { &pointers },
                item_overrides: if case % 2 == 0 { &[] } else { &items },
            };
            let mut body = vec![0xa5; kind.size()];
            d.build(
                &mut body,
                &registers,
                if case % 5 == 0 { None } else { Some(&p) },
            )
            .unwrap();
            emit(&body);
            let saved = body.clone();
            d.queue_pair = 4;
            assert_eq!(
                d.build(&mut body, &registers, Some(&p)),
                Err(Error::Invalid)
            );
            assert_eq!(body, saved);
            d.queue_pair = p.queue_pair as u32;
            d.write_tail = true;
            d.low_alias = Some(u64::MAX);
            assert_eq!(
                d.build(&mut body, &registers, Some(&p)),
                Err(Error::Overflow)
            );
            assert_eq!(body, saved);
            d.low_alias = Some(0x7002000000);
            d.write_lifecycle = true;
            assert_eq!(
                d.build(&mut body, &[], Some(&p)),
                Err(Error::MissingRegister)
            );
            assert_eq!(body, saved);
            d.write_lifecycle = false;
            d.write_structural = true;
            if kind == Kind::Tiling {
                assert_eq!(
                    d.build(&mut body, &[], Some(&p)),
                    Err(Error::MissingRegister)
                );
                assert_eq!(body, saved);
            }
            let mut bad = p;
            bad.usc_exec_base += 0x1000;
            bad.width = 0;
            assert_eq!(
                d.build(&mut body, &registers, Some(&bad)),
                Err(Error::UnsupportedExecBase)
            );
            assert_eq!(body, saved);
        }
        for dimension in [0, 16385, u64::MAX] {
            let mut bad = p;
            bad.width = dimension;
            assert!(tiling_registers(&bad).is_err());
            assert!(fragment_registers(&bad).is_err());
        }
        let mut bad = p;
        bad.tpc = p.context_base - 1;
        assert!(tiling_registers(&bad).is_err());
        for base in [0, 0x10000004000, u64::MAX] {
            bad = p;
            bad.usc_exec_base = base;
            assert_eq!(tiling_registers(&bad), Err(Error::UnsupportedExecBase));
            assert_eq!(fragment_registers(&bad), Err(Error::UnsupportedExecBase));
            assert_eq!(
                partial_store_registers(&bad),
                Err(Error::UnsupportedExecBase)
            );
            assert_eq!(
                partial_resume_registers(&bad),
                Err(Error::UnsupportedExecBase)
            );
            assert_eq!(
                partial_load_registers(&bad),
                Err(Error::UnsupportedExecBase)
            );
        }
    }
    assert!(words.next().is_none());
    let mut p = Parameters {
        width: 1,
        height: 1,
        ..Default::default()
    };
    for dimension in 1..=16384 {
        p.width = dimension;
        p.height = 16385 - dimension;
        let r = fragment_registers(&p).unwrap();
        emit(&(r[6].1 as u32).to_le_bytes());
        emit(&(r[7].1 as u32).to_le_bytes());
    }
    let mut page = vec![0xa5; 0x4000];
    aux_fb(&mut page).unwrap();
    emit(&page);
}
