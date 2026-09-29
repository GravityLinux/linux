// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]
#[path = "../g17p_compute.rs"]
mod g17p_compute;
#[path = "../g17p_compute_memory.rs"]
mod memory;
use g17p_compute::PAGE;
use memory::*;
use std::io::Write;
fn emit(bytes: &[u8]) {
    std::io::stdout().write_all(bytes).unwrap();
}
fn main() {
    for case in 0u32..32 {
        let mut out = vec![0; PAGE];
        let entries = [0, 1, 8, 21, 28, 255, 256][case as usize % 7];
        let base = 0x7000220000 + case as u64 * 0x4000;
        shared_state(&mut out, case).unwrap();
        emit(&out);
        operand_table_contiguous(&mut out, base, entries).unwrap();
        emit(&out);
        let bases: Vec<u64> = (0..entries).map(|i| base + i as u64 * 0x123000).collect();
        operand_table(&mut out, &bases).unwrap();
        emit(&out);
        let lists = PageLists {
            base,
            entries: [1, 8, 21, 28, 256][case as usize % 5],
            buffer_size: [0x100000, 0x30000, 0x8000][case as usize % 3],
            buffer_stride: 0x108000,
            page_size: [0x1000, 0x4000][case as usize % 2],
        };
        let mut body = vec![0; lists.size().unwrap()];
        lists.build(&mut body).unwrap();
        emit(&(body.len() as u32).to_le_bytes());
        emit(&body);
        let full = PageLists {
            base,
            entries: 8,
            buffer_size: 0x100000,
            buffer_stride: 0x108000,
            page_size: 0x1000,
        };
        full.build_full_page(&mut out).unwrap();
        emit(&out);
        let mut support = Support {
            compact: None,
            header: case as u64 + 1,
            word_08: case as u64 % 3,
            word_10: 0x0200800000000001,
            resource_class: 0x13 + case,
            word_20: if case % 2 == 0 {
                None
            } else {
                Some(0xfeed1234)
            },
            word_28: if case % 3 == 0 {
                None
            } else {
                Some(0xdead2345)
            },
            client_state: base,
            firmware_state: 0xfffffc2001630000 + case as u64 * 0x4000,
            cursor: case * 8 + 0x98,
            field_54: case % 4,
            field_5c: case % 5,
            final_kind: case % 3,
        };
        support.build(&mut out).unwrap();
        emit(&out);
        support.compact = Some((case % 3 + 1, 0x7000250000 + case as u64 * 0x1000));
        support.build(&mut out).unwrap();
        emit(&out);
        let active: Vec<u32> = (0..case % 18)
            .map(|i| [5, 3, 7, 1][i as usize % 4])
            .collect();
        let pool = Class2Pool {
            low_slots: base,
            high_slots: 0xfffffc2001630000,
            shared_state: 0xfffffc2001640000,
            records: [1, 35, 36, 80, 128][case as usize % 5],
            index_base: 0x808000 + case as u64 * 0x100,
            active: &active,
        };
        pool.build(&mut out).unwrap();
        emit(&out);
        pool_state(&mut out, case + 8, case % 6).unwrap();
        emit(&out);
        for profile in [Predecessor::Seed, Predecessor::Active, Predecessor::Minimal] {
            predecessor(
                &mut out,
                0xfffffc2001640000 + case as u64 * 0x4000,
                0xfffffc2000000000,
                profile,
            )
            .unwrap();
            emit(&out);
        }
        for minimal in [false, true] {
            predecessor_slots(&mut out, minimal).unwrap();
            emit(&out);
        }
    }
    let mut out = vec![0x5a; PAGE];
    let saved = out.clone();
    assert!(operand_table_contiguous(&mut out, 0, 257).is_err());
    assert_eq!(out, saved);
    assert!(operand_table_contiguous(&mut out, u64::MAX, 2).is_err());
    assert_eq!(out, saved);
    let mut lists = PageLists {
        base: 0,
        entries: 0,
        buffer_size: 0x100000,
        buffer_stride: 0x108000,
        page_size: 0x1000,
    };
    assert!(lists.size().is_err());
    lists.entries = 1;
    lists.page_size = 0;
    assert!(lists.size().is_err());
    lists.page_size = 0x1000;
    assert!(lists.build_full_page(&mut out).is_err());
    assert_eq!(out, saved);
    lists.base = u64::MAX;
    assert!(lists.size().is_err());
}
