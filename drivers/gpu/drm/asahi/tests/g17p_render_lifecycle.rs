// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]
#[path = "../g17p_compute.rs"]
mod g17p_compute;
#[path = "../g17p_compute_memory.rs"]
mod g17p_compute_memory;
#[path = "../g17p_dependency.rs"]
mod g17p_dependency;
#[path = "../g17p_opening.rs"]
mod g17p_opening;
#[path = "../g17p_queue.rs"]
mod g17p_queue;
#[path = "../g17p_render.rs"]
mod g17p_render;
#[path = "../g17p_render_graph.rs"]
mod g17p_render_graph;
#[path = "../g17p_render_lifecycle.rs"]
mod life;
use g17p_render::{Kind, Parameters};
use std::io::Write;
fn emit(body: &[u8]) {
    std::io::stdout().write_all(body).unwrap();
}
fn main() {
    assert!(life::Item::new(0).is_err());
    assert!(life::Item::new(65535).is_ok());
    assert!(life::Item::new(u32::MAX).is_ok());
    assert_eq!(life::STORAGE_SUBMISSIONS, 256);
    assert!(life::Item::new(254).is_ok());
    assert!(life::Item::new(255).is_ok());
    assert!(life::Item::retained(254, 254, life::ORDINARY).is_ok());
    assert!(life::Item::retained(254, 255, life::ORDINARY).is_ok());
    for ordinal in 2..255 {
        emit(&life::control_tick(ordinal).unwrap());
    }
    emit(&life::control_prestate());
    for count in [32, 72, 112, 0, 1312, u32::MAX] {
        let body = life::index_registration(0x1000190000, count).unwrap();
        emit(&[u8::from(body.is_some())]);
        if let Some(body) = body {
            emit(&body);
        }
    }
    assert!(life::index_registration(0, 32).is_err());
    assert!(life::index_registration(0x1000190001, 32).is_err());
    assert!(life::index_registration(0x2000000000, 32).is_err());
    let extended = [255, 256, 257, 510, 511, 512, 1022, 1023, 1024, 2047, 2048, 4094, 4095, 4096, 65534, 65535, 65536, 131071, 131072];
    for case in 0..256u64 + extended.len() as u64 {
        let item = life::Item::new(if case < 256 { (case % 254 + 1) as u32 } else { extended[(case-256) as usize] }).unwrap();
        let p = Parameters {
            width: 128 + case,
            height: 128,
            context_base: 0x1000000000,
            tilemap: 0x10001b0000,
            heapmeta: 0x10001b1000,
            tpc: 0x10001d8000,
            deflake_1: 0x10000682a0,
            deflake_2: 0x1000068020,
            deflake_3: 0x1000068000,
            ta_status: 0x1000078000,
            fragment_status: 0x10001a8000,
            aux_fb: 0x10000300000,
            encoder: 0x1000200000 + case * 0x4000,
            store_pipeline: 0x10000000140,
            load_pipeline: 0x10000000240,
            ta_user_timestamp_start: 0xfffffc2181400040 + case * 32,
            ta_user_timestamp_end: 0xfffffc2181400048 + case * 32,
            fragment_user_timestamp_start: 0xfffffc2181400050 + case * 32,
            fragment_user_timestamp_end: 0xfffffc2181400058 + case * 32,
            reactive_tvb_growth: true,
            emit_uapi_fields: true,
            ..Default::default()
        };
        for kind in [Kind::Tiling, Kind::Fragment] {
            for value in [item.descriptor_address(kind), item.descriptor_alias(kind), item.optional_address(kind), item.event_address(kind), item.status(kind), item.context_address(kind)] {
                emit(&value.to_le_bytes());
            }
            let mut body = vec![0xa5; kind.size()];
            item.descriptor(kind, &p, &mut body).unwrap();
            item.scheduler_node(kind, &mut body).unwrap();
            let node = item.ordinal + item.ordinal / 2;
            for &at in if kind == Kind::Tiling { &[0x370, 0x37c, 0x388][..] } else { &[0x470, 0x47c][..] } {
                assert_eq!(u32::from_le_bytes(body[at..at+4].try_into().unwrap()), 0x100 + (node & 0xff));
            }
            emit(&body);
            let mut optional = [0xa5; 0xc0];
            item.optional(kind, &mut optional).unwrap();
            emit(&optional);
            let mut context = [0xa5; 0x180];
            item.context(kind, &mut context).unwrap();
            emit(&context);
        }
        let reset = item.tilemap_reset(&p);
        emit(&[u8::from(reset.is_some())]);
        if let Some(address) = reset { emit(&address.to_le_bytes()); }
    }
    for pair in 0..2 {
        for index in [0, 1, 35, 70, 79, 254] {
            let mut layout = life::ORDINARY;
            layout.pair = pair;
            let item = life::Item::retained(1, index, layout).unwrap();
            for (kind, offsets) in [
                (Kind::Tiling, &[0x310, 0x31c, 0x328][..]),
                (Kind::Fragment, &[0x464][..]),
            ] {
                let mut body = vec![0xa5; kind.size()];
                item.pool_b_mirrors(kind, &mut body, 0x12345678, 0xabcdef12);
                for &at in offsets {
                    emit(&body[at..at + 4]);
                }
            }
        }
    }
    for pair in 0..2 {
        for index in [0, 1, 35, 70, 79, 254] {
            let mut layout = life::ORDINARY;
            layout.pair = pair;
            let item = life::Item::retained(1, index, layout).unwrap();
            let leaf = item.retirement_leaf();
            emit(&[u8::from(leaf.is_some())]);
            if let Some(address) = leaf {
                emit(&address.to_le_bytes());
                emit(&0x13u32.to_le_bytes());
            }
        }
    }
    for pair in 0..2 {
        for index in [0, 1, 7, 8, 35, 64, 254] {
            let mut layout = life::ORDINARY;
            layout.pair = pair;
            layout.independent = true;
            let item = life::Item::retained(1, index, layout).unwrap();
            let reset = item.tilemap_reset(&Parameters { tilemap: 0x10001b0000, ..Default::default() });
            emit(&[u8::from(reset.is_some())]);
            if let Some(address) = reset { emit(&address.to_le_bytes()); }
        }
    }
    for current in [0, 1, 2, 4, 6, 14, 0xfffffffd, 0xfffffffe, 0xffffffff] {
        for reused in [false, true] {
            let values = life::scheduler_phases(current, reused);
            emit(&[u8::from(values.is_ok())]);
            for value in values.unwrap_or([0; 3]) { emit(&value.to_le_bytes()); }
        }
    }
}
