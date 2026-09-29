// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]
#[path = "../g17p_compute.rs"]
mod g17p_compute;
#[path = "../g17p_compute_lifecycle.rs"]
mod lifecycle;
use std::io::Write;
fn emit(body: &[u8]) {
    std::io::stdout().write_all(body).unwrap();
}
fn main() {
    for (number, value) in lifecycle::opening_program(0x30000000000, 0x10000600000).unwrap() {
        emit(&number.to_le_bytes());
        emit(&value.to_le_bytes());
    }
    for n in [
        1, 2, 3, 4, 34, 35, 36, 127, 128, 239, 240, 255, 256, 383, 384, 1024,
    ] {
        for slot in [0, 1, 2] {
            let spec = lifecycle::Retained::new(n).unwrap();
            for word in [
                spec.descriptor,
                spec.descriptor_low,
                spec.optional,
                spec.event,
                spec.context_record,
                spec.scheduler,
                spec.scheduler_slot,
                spec.dispatch[0],
                spec.dispatch[1],
                spec.status[0],
                spec.status[1],
            ] {
                emit(&word.to_le_bytes());
            }
            let regs = spec.program(0x30000000000, 0x10000600000, slot).unwrap();
            for (number, value) in regs {
                emit(&number.to_le_bytes());
                emit(&value.to_le_bytes());
            }
            emit(&spec.scheduler_body());
            let mut page = [0; 0x4000];
            spec.descriptor_body(
                &mut page,
                &regs,
                0x10000600030,
                0,
                0,
                [
                    0xfffffc2181400000 + n as u64 * 16,
                    0xfffffc2181400008 + n as u64 * 16,
                ],
            )
            .unwrap();
            emit(&page);
            emit(&spec.optional_body());
            let mut ctx = [0; 0x200];
            spec.context_body(&mut ctx).unwrap();
            emit(&ctx);
            emit(&lifecycle::channel_control());
        }
    }
    for (number, value) in
        lifecycle::after_render_opening_program(0x30000000000, 0x10000600000).unwrap()
    {
        emit(&number.to_le_bytes());
        emit(&value.to_le_bytes());
    }
    let regs = lifecycle::Retained::new(1)
        .unwrap()
        .program(0x30000000000, 0x10000600000, 1)
        .unwrap();
    let mut page = [0; 0x4000];
    lifecycle::after_render_second_descriptor(
        &mut page,
        &regs,
        0x10000600030,
        0,
        0,
        [0xfffffc2181400010, 0xfffffc2181400018],
        [0xfffffc2001a00010, 0xfffffc2001a00018],
    )
    .unwrap();
    emit(&page);
    emit(&lifecycle::after_render_second_optional());
    let mut ctx = [0; 0x200];
    lifecycle::after_render_second_context(&mut ctx).unwrap();
    emit(&ctx);
    for n in [1,2,3,4,35,36,127,128,239,240,255,256,257] {
        let mut spec = lifecycle::Retained::new(n).unwrap();
        spec.status = [0xfffffc2001a00000 + n as u64 * 16,
                       0xfffffc2001a00008 + n as u64 * 16];
        emit(&spec.after_render_event().to_le_bytes());
        emit(&spec.after_render_context_address().unwrap().to_le_bytes());
        let regs = spec.program(0x30000000000,0x10000600000,n%2).unwrap();
        let mut page = [0;0x4000];
        spec.after_render_descriptor(&mut page,&regs,0x10000600030,0,0,
            [0xfffffc2181400000+n as u64*16,0xfffffc2181400008+n as u64*16]).unwrap();
        emit(&page);
        let mut ctx = [0;0x200];
        spec.after_render_context(&mut ctx).unwrap(); emit(&ctx);
    }
    for case in 0..8u32 {
        let old = [
            0xfffffc200165a870 + case as u64 * 0x4000,
            0xfffffc20c08aa870 + case as u64 * 0x4000,
        ];
        let done = 384 + case * 3;
        let mut record = core::array::from_fn(|i| (i as u32 * 17 + case * 13) as u8);
        record[..8].copy_from_slice(&old[0].to_le_bytes());
        record[8..16].copy_from_slice(&old[1].to_le_bytes());
        record[0x1c..0x20].copy_from_slice(&done.to_le_bytes());
        for (wrong, bad_done, slot) in [
            ([old[0] + 8, old[1]], done, 0),
            (old, done + 1, 0),
            (old, done, 2),
        ] {
            let mut rejected = record;
            assert!(lifecycle::transport_record(&mut rejected, wrong, bad_done, slot).is_err());
            assert_eq!(rejected, record);
        }
        lifecycle::transport_record(&mut record, old, done, case % 2).unwrap();
        emit(&record);
        emit(&lifecycle::transport_pointers());
    }
    for invalid in [0, 0xffffff, u32::MAX] {
        assert!(lifecycle::Retained::new(invalid).is_err());
    }
}
