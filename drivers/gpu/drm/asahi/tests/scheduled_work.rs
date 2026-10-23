// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]

#[path = "LEGACY_COMPUTE"]
mod legacy;
#[path = "../g17p_compute.rs"]
mod current;
use current::work::*;
use current::wire::Error;
use std::io::{Read, Write};

fn emit(bytes: &[u8]) { std::io::stdout().write_all(bytes).unwrap(); }

fn neo() {
    for i in 0u32..256 {
        let item = [0, 1, 255, 256, (1 << 30) - 2, u32::MAX][i as usize % 6];
        let a = legacy::Optional {
            context_low: 0x70004d8000 + i as u64 * 0x4000,
            context_high: 0xfffffc2000278000 + i as u64 * 0x4000,
            grid: i * 0x10001, ordinal: item,
            shared_control: 0xfffffc20c0998000 + i as u64 * 8,
            channel_control: 0xfffffc20c07b8040 + i as u64 * 0x40,
            uuid: i.wrapping_mul(0x1020304), field_46: i * 0x10003,
            field_1e: i % 4, field_32: i * 0x10001,
            field_56: i * 0x10007, field_5e: i * 0x10009,
            first: i % 2 == 0, item_index: item,
        };
        let b = current::Optional {
            context_low: a.context_low, context_high: a.context_high,
            grid: a.grid, ordinal: a.ordinal, shared_control: a.shared_control,
            channel_control: a.channel_control, uuid: a.uuid, field_46: a.field_46,
            field_1e: a.field_1e, field_32: a.field_32, field_56: a.field_56,
            field_5e: a.field_5e, first: a.first, item_index: a.item_index,
        };
        assert_eq!(a.build(), b.build()); emit(&b.build());

        let values: Vec<_> = (0..i % 32 + 1).map(|j| ((j % 128) as u8, i * 3107 + j)).collect();
        let grid = [0, 1, 127, 65535, (1 << 22) - 1][i as usize % 5];
        let a = legacy::Context {
            descriptor: 0xfffffc20c0358000 + i as u64 * 0x1040,
            queue: 0xfffffc20c0000300, grid, flags: (i as u64) << 55,
            word_220: 0xffff080200000001, word_330: i as u64 * 1234567,
            word_338: i as u64 % 9, word_350: 0x000110038001a084 + i as u64,
            word_358: 0x000020038001a0bd + i as u64, word_378: 0x003fffffffffffff,
            item_index: item, points: if i % 3 == 0 { None } else { Some(&values) },
            event_slot: if i % 2 == 0 { None } else { Some(i as u8) },
            completion: if i % 4 == 0 { None } else { Some(i + 1) },
        };
        let b = current::Context {
            descriptor: a.descriptor, queue: a.queue, grid: a.grid, flags: a.flags,
            word_220: a.word_220, word_330: a.word_330, word_338: a.word_338,
            word_350: a.word_350, word_358: a.word_358, word_378: a.word_378,
            item_index: a.item_index, points: a.points, event_slot: a.event_slot,
            completion: a.completion,
        };
        let mut old = vec![0xa5; 0x200]; let mut new = vec![0x5a; 0x200];
        a.build(&mut old).unwrap(); b.build(&mut new).unwrap(); assert_eq!(old, new); emit(&new);
        let mut reused: Vec<u8> = (0..0x200).map(|j| (j * 137 + i) as u8).collect();
        let mut baseline = reused.clone();
        legacy::update_context(&mut baseline, &old).unwrap();
        current::update_context(&mut reused, &new).unwrap(); assert_eq!(baseline, reused); emit(&reused);
        let mut invalid = b; invalid.points = Some(&[]);
        let mut invalid_old = a; invalid_old.points = Some(&[]);
        assert_eq!(invalid.build(&mut new), Err(current::Error::Invalid));
        assert_eq!(invalid_old.build(&mut old), Err(legacy::Error::Invalid)); assert_eq!(old, new);

        let a = legacy::Scheduler { slot: 0xfffffc2001640000 + i as u64 * 4,
            work_id: i * 77, phase: i % 3, job_list: if i % 2 == 0 { 0 } else { 0xfffffc2000000000 },
            node_id: if i % 3 == 0 { 0 } else { u64::MAX - i as u64 },
            completion_kind: i as u64 % 4 };
        let b = current::Scheduler { slot: a.slot, work_id: a.work_id, phase: a.phase,
            job_list: a.job_list, node_id: a.node_id, completion_kind: a.completion_kind };
        assert_eq!(a.build(), b.build()); emit(&b.build());
    }
}

fn main() {
    neo();
    let mut bytes = Vec::new(); std::io::stdin().read_to_end(&mut bytes).unwrap();
    let mut input = bytes.as_slice();
    while !input.is_empty() {
        let kind = input[0]; let count = input[1] as usize; input = &input[2..];
        let mut w = Vec::new(); for _ in 0..count {
            w.push(u64::from_le_bytes(input[..8].try_into().unwrap())); input = &input[8..];
        }
        match kind {
            1 => {
                let mut p = RecordParametersG16C {
                    descriptor: w[0], queue: w[1], grid: w[2] as u8, item: w[3] as u32,
                    primary_count: w[4] as u16, timeout: w[5] as u16, ready: w[6] != 0,
                    retire_control: w[7], flags: w[8], control: w[9] as u16, control_130: w[10],
                    header_grid: if w[11] == u64::MAX { None } else { Some(w[11] as u8) },
                    header_count: if w[12] == u64::MAX { None } else { Some(w[12] as u32) },
                    control_low: w[13] as u32, field_30: w[14],
                };
                emit(ComputeRecordG16C::new(&p).unwrap().as_bytes());
                let item = p.item; p.item = 0x3fffffff;
                assert!(matches!(ComputeRecordG16C::new(&p), Err(Error::Invalid))); p.item = item;
                let grid = p.grid; p.grid = 128;
                assert!(matches!(ComputeRecordG16C::new(&p), Err(Error::Invalid))); p.grid = grid;
                let desc = p.descriptor; p.descriptor = DESCRIPTOR_BASE_G16C - 0x80;
                assert!(matches!(ComputeRecordG16C::new(&p), Err(Error::Invalid)));
                p.descriptor = u64::MAX;
                assert!(matches!(ComputeRecordG16C::new(&p), Err(Error::Overflow)));
                p.descriptor = desc;
                for count in [0, 128, u16::MAX] { p.primary_count = count;
                    assert!(matches!(ComputeRecordG16C::new(&p), Err(Error::Invalid))); }
            }
            2 => {
                let mut p = WrapperParameters {
                    scratch_low: w[0], scratch_high: w[1], support: w[2], queue_control: w[3],
                    grid: w[4] as u16, item: w[5] as u16, ordinal: w[6] as u16, context: w[7] as u16,
                    support_id: w[8] as u16, uuid: w[9] as u16, scheduler: w[10] as u16,
                    scheduler_hint: w[11] as u16, first: w[12] != 0, field_52: w[13] as u16,
                    field_5e: w[14] as u16, field_62: w[15] as u16,
                };
                emit(ComputeWrapperG16C::new(&p).unwrap().as_bytes());
                let context = p.context; p.context = 0;
                assert!(matches!(ComputeWrapperG16C::new(&p), Err(Error::Invalid)));
                p.context = 64;
                assert!(matches!(ComputeWrapperG16C::new(&p), Err(Error::Invalid))); p.context = context;
                p.queue_control = 0;
                assert!(matches!(ComputeWrapperG16C::new(&p), Err(Error::Invalid)));
            }
            3 => emit(ComputeSchedulerG16C::for_compute(w[0]).as_bytes()),
            _ => panic!("bad host protocol"),
        }
    }
    for (descriptor, offset, count) in [(DESCRIPTOR_BASE_G16C - 0x80, 0x40, 34),
        (DESCRIPTOR_BASE_G16C + 1, 0x40, 34), (DESCRIPTOR_BASE_G16C, 0x40, 128),
        (DESCRIPTOR_BASE_G16C + (1 << 36), 0x40, 34), (u64::MAX, 0x40, 34)] {
        assert!(register_locator_g16c(descriptor, offset, count).is_err());
    }
}
