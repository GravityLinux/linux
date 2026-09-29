// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]
#[path = "../g17p_dependency.rs"]
mod dependency;
#[path = "../g17p_compute.rs"]
mod g17p_compute;
#[path = "../g17p_compute_memory.rs"]
mod g17p_compute_memory;
#[path = "../g17p_queue.rs"]
mod g17p_queue;
#[path = "../g17p_render.rs"]
mod g17p_render;
#[path = "../g17p_render_graph.rs"]
mod g17p_render_graph;
use dependency as d;
use g17p_compute::{Error, Register};
use std::io::{Read, Write};
struct Transition {
    values: std::collections::BTreeMap<u64, u32>,
    writes: Vec<(u64, u32)>,
}
impl d::ReleaseWriter for Transition {
    type Error = ();
    fn read32(&mut self, address: u64) -> Result<u32, ()> {
        self.values.get(&address).copied().ok_or(())
    }
    fn write32(&mut self, address: u64, value: u32) -> Result<(), ()> {
        self.values.insert(address, value);
        self.writes.push((address, value));
        Ok(())
    }
}
fn emit(body: &[u8]) {
    std::io::stdout().write_all(body).unwrap();
}
fn regs(values: &[Register]) {
    for (number, value) in values {
        emit(&number.to_le_bytes());
        emit(&value.to_le_bytes());
    }
}
fn main() {
    for t in d::LAYOUTS {
        for word in [
            t.queue,
            t.pointers,
            t.ring,
            t.grid as u64,
            t.job_list,
            t.control,
            t.context_low,
            t.context_high,
        ] {
            emit(&word.to_le_bytes());
        }
        emit(&t.record());
        emit(&d::pointers());
        emit(&d::channel_control());
    }
    for i in 0..3 {
        emit(&g17p_queue::job_list(d::JOB_LIST + i * 0x18));
        emit(&d::scheduler(i as usize).unwrap());
    }
    assert_eq!(d::scheduler(3), Err(Error::Invalid));
    let mut page = [0; 0x4000];
    d::compute_support(&mut page).unwrap();
    emit(&page);
    d::render_support(&mut page).unwrap();
    emit(&page);
    d::operand_table(&mut page).unwrap();
    emit(&page);
    for leaf in [
        g17p_render_graph::Leaf::PrimaryIndex,
        g17p_render_graph::Leaf::SecondaryIndex,
        g17p_render_graph::Leaf::PoolASlots,
        g17p_render_graph::Leaf::PoolBSlots,
        g17p_render_graph::Leaf::SharedSlots,
        g17p_render_graph::Leaf::Flag,
    ] {
        d::leaf(&mut page, leaf).unwrap();
        emit(&page);
    }
    let mut a = [0; g17p_render_graph::POOL_A_SIZE];
    d::pool_a(&mut a).unwrap();
    emit(&a);
    let mut b = [0; g17p_render_graph::POOL_B_SIZE];
    d::pool_b(&mut b).unwrap();
    emit(&b);
    let mut shared = [0; 0x88];
    d::shared(&mut shared).unwrap();
    emit(&shared);
    for case in 0..16 {
        let caller_cdm = 0x10000600000 + case * 0x78000;
        for spec in [d::Compute::Opening, d::Compute::Closing] {
            let program = spec
                .program(caller_cdm, g17p_compute::USC_EXEC_BASE)
                .unwrap();
            regs(&program);
            let end = if spec.index() == 0 {
                0x100000b0030
            } else {
                caller_cdm + 0x30
            };
            spec.descriptor_body(
                &mut page,
                &program,
                end,
                if case % 2 == 0 {
                    0
                } else {
                    caller_cdm + 0x10000
                },
                if case % 2 == 0 { 0 } else { case as u32 },
                [
                    0xfffffc2181400000 + case * 32 + spec.index() as u64 * 16,
                    0xfffffc2181400008 + case * 32 + spec.index() as u64 * 16,
                ],
            )
            .unwrap();
            emit(&page);
            emit(&spec.optional_body());
            emit(&spec.event_body());
            let mut context = [0; 0x200];
            spec.context_body(&mut context, case as u32 + 1).unwrap();
            emit(&context);
            assert_eq!(
                spec.program(caller_cdm, 0x20000000000),
                Err(Error::UnsupportedExecBase)
            );
            assert_eq!(
                spec.program(u64::MAX, 0x20000000000),
                Err(Error::UnsupportedExecBase)
            );
            assert_eq!(
                spec.program(u64::MAX, g17p_compute::USC_EXEC_BASE),
                Err(Error::Overflow)
            );
            for invalid in [0, 1 << 30, u32::MAX] {
                context.fill(0xa5);
                assert_eq!(
                    spec.context_body(&mut context, invalid),
                    Err(Error::Invalid)
                );
                assert!(context.iter().all(|v| *v == 0xa5));
            }
        }
        for kind in [g17p_render::Kind::Tiling, g17p_render::Kind::Fragment] {
            let mut optional = [0; 0xc0];
            d::render_optional(&mut optional, kind).unwrap();
            emit(&optional);
            let mut context = [0; g17p_render_graph::CONTEXT_SIZE];
            d::render_context(&mut context, kind, case as u32 + 1).unwrap();
            emit(&context);
        }
        let p = g17p_render::Parameters {
            width: [1, 32, 128, 129][case as usize % 4],
            height: [129, 128, 32, 1][case as usize % 4],
            context_base: 0x1000000000,
            tilemap: 0x10001b0000,
            heapmeta: 0x10001b1000,
            tpc: 0x10001d8000,
            ta_status: 0x1000078000,
            fragment_status: 0x10001a8000,
            deflake_1: 0x10000682a0,
            deflake_2: 0x1000068020,
            deflake_3: 0x1000068000,
            encoder: 0x1000000000 + case * 0x4000,
            aux_fb: 0x10000300000,
            sampler_array: if case % 2 == 0 {
                0
            } else {
                0x10000080000 + case * 8
            },
            sampler_count: if case % 2 == 0 { 0 } else { case },
            emit_uapi_fields: true,
            reactive_tvb_growth: true,
            ..Default::default()
        };
        for kind in [g17p_render::Kind::Tiling, g17p_render::Kind::Fragment] {
            let mut descriptor = vec![0; kind.size()];
            d::render_descriptor(&mut descriptor, kind, &p).unwrap();
            emit(&descriptor);
        }
        emit(&d::render_registration(case as u32));
        emit(&d::render_receipt(case as u32));
        let mut receipt = d::Receipt::new(case as u32);
        let mut body = [0; 0x48];
        body[..0x28].copy_from_slice(&d::render_receipt(case as u32));
        for at in 0..0x28 {
            body[at] ^= 1;
            emit(&[receipt.consume(0, &body) as u8]);
            body[at] ^= 1;
        }
        emit(&[receipt.consume(1, &body) as u8]);
        for len in [0, 0x27, 0x28, 0x47] {
            emit(&[receipt.consume(0, &body[..len]) as u8]);
        }
        body[0x28..].fill(case as u8);
        emit(&[receipt.consume(0, &body) as u8]);
        emit(&[receipt.consume(0, &body) as u8]);
        assert!(!receipt.pending());
    }
    for counter in 0..3 {
        emit(&d::tick(counter).unwrap());
    }
    for engine in [2, 1, 0] {
        emit(&d::engine_owner(engine).unwrap());
    }
    assert_eq!(d::tick(3), Err(Error::Invalid));
    assert_eq!(d::engine_owner(3), Err(Error::Invalid));
    for case in 0..16 {
        let mut t = Transition {
            values: Default::default(),
            writes: Vec::new(),
        };
        for offset in (0..0x20).step_by(4) {
            t.values
                .insert(d::LEAVES[0] + 0x60 + offset, (case * 0x100 + offset) as u32);
        }
        t.values.insert(
            d::LEAVES[1] + 0x30,
            if case == 15 {
                u32::MAX
            } else {
                case as u32 + 10
            },
        );
        t.values.insert(d::LEAVES[1] + 0x38, case as u32 + 20);
        d::render_transition(&mut t).unwrap();
        for (at, value) in t.writes {
            emit(&at.to_le_bytes());
            emit(&value.to_le_bytes());
        }
    }
    // Source-generated register lists include every changed register, duplicates,
    // and unrelated entries. Verify only the selected engine's fields change.
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    let mut words = input
        .chunks_exact(12)
        .map(|r| {
            (
                u32::from_le_bytes(r[..4].try_into().unwrap()),
                u64::from_le_bytes(r[4..].try_into().unwrap()),
            )
        })
        .collect::<Vec<_>>();
    let half = words.len() / 2;
    d::render_registers(g17p_render::Kind::Tiling, &mut words[..half]);
    d::render_registers(g17p_render::Kind::Fragment, &mut words[half..]);
    regs(&words);
}
