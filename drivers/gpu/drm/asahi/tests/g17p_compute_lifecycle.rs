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
    for invalid in [0, 0xffffff, u32::MAX] {
        assert!(lifecycle::Retained::new(invalid).is_err());
    }
}
