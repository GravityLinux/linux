// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]
#[path = "../g17p_compute.rs"]
mod g17p_compute;
#[path = "../g17p_compute_memory.rs"]
mod g17p_compute_memory;
#[path = "../g17p_opening.rs"]
mod g17p_opening;
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
    assert!(life::Item::new(life::SUBMISSIONS).is_err());
    assert!(life::Item::new(u32::MAX).is_err());
    for case in 0..64u64 {
        let item = life::Item::new(1).unwrap();
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
            let mut body = vec![0xa5; kind.size()];
            item.descriptor(kind, &p, &mut body).unwrap();
            emit(&body);
            let mut optional = [0xa5; 0xc0];
            item.optional(kind, &mut optional).unwrap();
            emit(&optional);
            let mut context = [0xa5; 0x180];
            item.context(kind, &mut context).unwrap();
            emit(&context);
        }
    }
}
