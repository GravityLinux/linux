// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]
#[path = "../g17p_compute.rs"]
mod compute;
use compute::*;
use std::io::Write;
fn emit(out: &[u8]) {
    std::io::stdout().write_all(out).unwrap();
}
fn main() {
    for case in 0u32..96 {
        let ordinal = [0, 1, 3, 255, 256, 1024, u32::MAX][case as usize % 7];
        let mut program = Program {
            preempt: 0x10000200000 + case as u64 * 0x4000,
            cdm: 0x10003000000 + case as u64 * 0x8000,
            identity: 0x0200034503000346 + case as u64,
            context: case % 64,
            ordinal,
            robustness: 0x10004000000,
            operand_state: 0x7000220000,
            usc_exec_base: USC_EXEC_BASE,
            helper_binary: case as u64 * 0x4000 | 5,
            helper_data: 0x10000300000 + case as u64 * 0x4000,
            helper_cfg: (case as u64) << 16,
            execution_gate: (case % 3) as u64,
        };
        let mut regs = program.build().unwrap().to_vec();
        for base in [0, USC_EXEC_BASE + 0x4000, u64::MAX] {
            program.usc_exec_base = base;
            assert_eq!(program.build(), Err(Error::UnsupportedExecBase));
        }
        if case % 3 == 1 {
            regs.push((0x1a440, 0xdeadbeef));
        }
        if case % 3 == 2 {
            regs.extend((40..128).map(|i| (0x20000 + i, i as u64 * 9821)));
        }
        let mut regbytes = Vec::new();
        for (n, v) in &regs {
            regbytes.extend(n.to_le_bytes());
            regbytes.extend(v.to_le_bytes());
        }
        emit(&(regs.len() as u32).to_le_bytes());
        emit(&regbytes);
        let mut d = Descriptor {
            scheduler: 0xfffffc20c0900100,
            low_alias: 0x7002000000 + case as u64 * 0x4000,
            cdm_terminator: program.cdm + 0x100,
            sequence: (case as u64) << 40 | case as u64,
            context: case % 64,
            grid: case % 12,
            dispatch: [0xfffffc20001c8028, 0xfffffc20c07c0028],
            status: [0xfffffc2000024c68, 0xfffffc2000024c70],
            timestamps: [case as u64 * 0x4000, 0x7000500000 + case as u64 * 8],
            shared_control: 0xfffffc20c0998000,
            zero_page: 0xfffffc2001710000,
            support_control: 0x21000001 + case,
            support_flags: case % 4,
            ordinal,
            queue_submission: case + 1,
            queue_ordinal: ordinal,
            submission_index: case * 3 + 1,
            sampler_array: if case % 2 == 0 {
                0
            } else {
                0x10000800000 + case as u64 * 8
            },
            sampler_count: if case % 2 == 0 { 0 } else { case + 1 },
        };
        let mut body = vec![0x5a; PAGE];
        d.build(&mut body, &regs).unwrap();
        emit(&body);
        if case == 0 {
            let saved = body.clone();
            assert_eq!(d.build(&mut body, &regs[..6]), Err(Error::MissingRegister));
            assert_eq!(body, saved);
            d.cdm_terminator = program.cdm - 1;
            assert_eq!(d.build(&mut body, &regs), Err(Error::Invalid));
            assert_eq!(body, saved);
            d.cdm_terminator = program.cdm + 0x100;
            let low = d.low_alias;
            d.low_alias = u64::MAX;
            assert_eq!(d.build(&mut body, &regs), Err(Error::Overflow));
            assert_eq!(body, saved);
            d.low_alias = low;
            d.queue_submission = 0;
            assert_eq!(d.build(&mut body, &regs), Err(Error::Invalid));
            assert_eq!(body, saved);
            d.queue_submission = 1;
            d.sampler_array = 0x10000800003;
            d.sampler_count = 1;
            assert_eq!(d.build(&mut body, &regs), Err(Error::Invalid));
            assert_eq!(body, saved);
            d.sampler_array = 0;
        }
        d.sampler_count = u32::MAX;
        let saved = body.clone();
        assert!(d.build(&mut body, &regs).is_err());
        assert_eq!(body, saved);
        let optional = Optional {
            context_low: 0x70004d8000,
            context_high: 0xfffffc2000278000,
            grid: case % 12,
            ordinal,
            shared_control: 0xfffffc20c0998000,
            channel_control: 0xfffffc20c07b8040,
            uuid: case * 257,
            field_46: case % 5,
            field_1e: case % 3,
            field_32: case + 1,
            field_56: case + 2,
            field_5e: case + 3,
            first: case % 2 == 0,
            item_index: ordinal,
        };
        emit(&optional.build());
        let mut event_body = vec![0; 0x400];
        event(&mut event_body, case * 171, case % 12, case * 257).unwrap();
        emit(&event_body);
        let values: Vec<(u8, u32)> = (0..(case % 32 + 1))
            .map(|i| ((i % 128) as u8, case * 19000 + i))
            .collect();
        let c = Context {
            descriptor: 0xfffffc20c0358000 + case as u64 * 0x1040,
            queue: 0xfffffc20c0000300,
            grid: case % 12,
            flags: 0x1000000000000000,
            word_220: 0xffff080100000001,
            word_330: case as u64 % 5,
            word_338: case as u64 * 123,
            word_350: 0x000110038001a002,
            word_358: 0x000020038001a03b,
            word_378: 0x003fffffffffffff,
            item_index: ordinal,
            points: if case % 3 == 0 { None } else { Some(&values) },
            event_slot: if case % 2 == 0 {
                None
            } else {
                Some((case % 128) as u8)
            },
            completion: if case % 4 == 0 { None } else { Some(case + 1) },
        };
        let mut current = vec![0; 0x200];
        c.build(&mut current).unwrap();
        emit(&current);
        let mut prior: Vec<u8> = (0..0x200).map(|i| ((i * 137 + case) & 255) as u8).collect();
        update_context(&mut prior, &current).unwrap();
        emit(&prior);
        for records in [2, 3, 128, 255, 256] {
            emit(&(context_offset(ordinal, records).unwrap() as u32).to_le_bytes());
        }
        let scheduler = Scheduler {
            slot: 0xfffffc2001640000 + case as u64 * 4,
            work_id: case * 77,
            phase: case % 3,
            job_list: if case % 2 == 0 { 0 } else { 0xfffffc2000000000 },
            node_id: if case % 3 == 0 { 0 } else { case as u64 * 123 },
            completion_kind: case as u64 % 4,
        };
        emit(&scheduler.build());
        emit(&scheduler_slot(case as u64 * 17, case as usize % 9).unwrap());
    }
    let mut out = vec![0; 0x200];
    assert!(points(&mut out[..0x110], 8, 0, &[]).is_err());
    assert!(points(&mut out[..0x110], 8, 0, &[(128, 1)]).is_err());
    assert!(completion_header(0, 0).is_err());
    assert!(completion_header(0, 1 << 30).is_err());
    assert!(context_offset(0, 1).is_err());
    assert!(context_offset(0, 257).is_err());
    assert!(update_context(&mut out, &[0; 0x200]).is_err());
    assert!(scheduler_slot(1, 9).is_err());
}
