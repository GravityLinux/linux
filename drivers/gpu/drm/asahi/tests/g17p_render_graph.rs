// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]
#[path = "../g17p_compute.rs"]
mod g17p_compute;
#[path = "../g17p_compute_memory.rs"]
mod g17p_compute_memory;
#[path = "../g17p_render.rs"]
mod g17p_render;
#[path = "../g17p_render_graph.rs"]
mod graph;
use g17p_compute::Error;
use g17p_render::Kind;
use graph::*;
use std::io::Write;
fn emit(body: &[u8]) {
    std::io::stdout().write_all(body).unwrap();
}
fn main() {
    for case in 0..64u32 {
        let pair = case % 4;
        let work = [0, 1, 32767, 8192][case as usize % 4];
        let slots = 0xfffffc2001640004 + case as u64 * 0x4000;
        let shared = 0xfffffc2001698040 + case as u64 * 0x4000;
        let mut a = vec![0xa5; POOL_A_SIZE];
        let mut b = vec![0xa5; POOL_B_SIZE];
        record_array_a(&mut a, slots, work).unwrap();
        emit(&a);
        record_array_b(&mut b, slots, shared, pair, work).unwrap();
        emit(&b);
        let saved = b.clone();
        assert!(record_array_b(&mut b, u64::MAX, shared, pair, work).is_err());
        assert_eq!(b, saved);
        assert!(record_array_b(&mut b, slots, shared, pair, u32::MAX).is_err());
        assert_eq!(b, saved);
        let pointers = [slots + 0x10000, slots + 0x18000, shared, slots + 0x20000];
        let mut s = vec![0xa5; 0x88];
        Shared {
            pointers,
            pair,
            groups: case % 32 + 1,
            work,
        }
        .build(&mut s)
        .unwrap();
        emit(&s);
        let saved = s.clone();
        assert!(Shared {
            pointers,
            pair,
            groups: 0,
            work
        }
        .build(&mut s)
        .is_err());
        assert_eq!(s, saved);
        context2_shared(&mut s, pointers, case).unwrap();
        emit(&s);
        let custom = [(case * 3 + 1, case % 23), (case * 9 + 0x30, case % 32)];
        let ranges = match case % 3 {
            0 => &DEFAULT_INDEX_GROUPS,
            1 => &CONTEXT2_INDEX_GROUPS,
            _ => &custom,
        };
        let mut page = vec![0xa5; 0x4000];
        for leaf_kind in [
            Leaf::PrimaryIndex,
            Leaf::SecondaryIndex,
            Leaf::PoolASlots,
            Leaf::PoolBSlots,
            Leaf::SharedSlots,
            Leaf::Flag,
        ] {
            leaf(&mut page, leaf_kind, pair, ranges, case % 32 + 1, work).unwrap();
            emit(&page);
        }
        let saved = page.clone();
        assert!(leaf(&mut page, Leaf::PrimaryIndex, 0, &[(0, 1025)], 0, 0).is_err());
        assert_eq!(page, saved);
        assert!(leaf(&mut page, Leaf::PrimaryIndex, 0, &[(u32::MAX, 1)], 0, 0).is_err());
        assert_eq!(page, saved);
        for kind in [Kind::Tiling, Kind::Fragment] {
            let overrides = [
                (0x1e, (case + 40) as u16),
                (0x46, (case + 41) as u16),
                (0xbe, 0x5aa5),
            ];
            let mut optional = Optional {
                kind,
                context_scratch: 0x7000000000 + case as u64 * 0x4000,
                firmware_scratch: slots,
                shared_control: shared,
                channel_control: shared + 0x4000,
                tiling_shared: if kind == Kind::Tiling {
                    Some(slots + 0x8000)
                } else {
                    None
                },
                grid: (case % 12) as u16,
                item: (case % 37) as u16,
                ordinal: (case * 173) as u16,
                context: if case % 3 == 0 {
                    None
                } else {
                    Some((case % 4) as u16)
                },
                uuid: if case % 2 == 0 {
                    None
                } else {
                    Some((case * 131) as u16)
                },
                scheduler_class: if case % 3 == 0 {
                    None
                } else {
                    Some((case % 7) as u16)
                },
                context_index: if case % 3 == 0 {
                    None
                } else {
                    Some((case * 307) as u16)
                },
                context_phase: if case % 3 == 0 {
                    None
                } else {
                    Some((case * 317) as u16)
                },
                first: match case % 3 {
                    0 => None,
                    1 => Some(false),
                    _ => Some(true),
                },
                lifecycle: if case % 3 == 0 {
                    None
                } else {
                    Some((case * 337) as u16)
                },
                namespace: if case % 3 == 0 {
                    None
                } else {
                    Some((case * 347) as u16)
                },
                overrides: if case % 2 == 0 { &[] } else { &overrides },
            };
            let mut body = vec![0xa5; 0xc0];
            optional.build(&mut body).unwrap();
            emit(&body);
            let saved = body.clone();
            optional.overrides = &[(1, 0)];
            assert!(optional.build(&mut body).is_err());
            assert_eq!(body, saved);
            optional.overrides = &[];
            optional.context_phase = None;
            optional.item = 256;
            assert!(optional.build(&mut body).is_err());
            assert_eq!(body, saved);
        }
        // Only the authored record changes; subsequent event storage is retained.
        let mut events = vec![0xa5; 0x400];
        event(&mut events[..0x40], case * 37, case * 419, case * 421).unwrap();
        emit(&events);
    }
    for count in [0usize, 1, 7, 8, 9, 21, 28, 29, 256] {
        let base = 0x7000238000 + count as u64 * 0x4000;
        let mut table = vec![0xa5; 0x4000];
        operand_table(&mut table, base, count).unwrap();
        emit(&table);
        let mut directory = vec![0xa5; operand_directory_size(count).unwrap()];
        operand_directory(&mut directory, base, count).unwrap();
        emit(&directory);
    }
    for case in 0..256u32 {
        for kind in [Kind::Tiling, Kind::Fragment] {
            let fragment = kind == Kind::Fragment;
            let mode = case % 8;
            let pair = match mode {
                0 | 1 => case / 8 % 2,
                4 => 2,
                5 => 3,
                _ => 0,
            };
            let item = if mode == 2 || mode == 3 {
                0
            } else {
                [0, 1, 2, 35, 36, 255, 256, 1024, u32::MAX][case as usize / 8 % 9]
            };
            let context = match mode {
                2 | 3 => Some(1),
                4 | 6 | 7 => Some(2),
                5 => Some(3),
                _ => None,
            };
            let base = if fragment {
                0xfffffc20c00b0000
            } else {
                0xfffffc20c0018000
            };
            let points: Vec<(u8, u32)> = (0..case % 32 + 1)
                .map(|i| (((i * 3 + case) % 128) as u8, case * 1000 + i))
                .collect();
            let mut c = Context {
                kind,
                descriptor: if mode == 0 {
                    0
                } else {
                    base + (case % 37) as u64 * if fragment { 0x2240 } else { 0x9c0 }
                },
                queue: 0xfffffc20c0000000 + case as u64 * 0xc0,
                pair,
                item,
                context,
                grid: match mode {
                    5 => Some(12 + fragment as u32),
                    6 => Some(fragment as u32),
                    _ => None,
                },
                locator_context: if mode == 6 { Some(3) } else { None },
                partial_opening: mode == 2,
                dependency_grid: if mode == 3 {
                    Some((case / 8 % 6) * 2 + fragment as u32)
                } else {
                    None
                },
                points: if case % 3 == 0 { None } else { Some(&points) },
                event_slot: if case % 5 == 0 {
                    Some((case % 128) as u8)
                } else {
                    None
                },
                completion: if case % 4 == 0 {
                    None
                } else {
                    Some(case * 503 + 1)
                },
            };
            let mut body = vec![0xa5; CONTEXT_SIZE];
            c.build(&mut body).unwrap();
            emit(&body);
            let mut previous: Vec<u8> = (0..CONTEXT_SIZE)
                .map(|i| (i * 137 + case as usize) as u8)
                .collect();
            update_context(kind, &mut previous, &body).unwrap();
            emit(&previous);
            let mut page = vec![0xa5; 0x4000];
            c.page(&mut page).unwrap();
            emit(&page);
            let saved = body.clone();
            c.completion = Some(0);
            assert_eq!(c.build(&mut body), Err(Error::Invalid));
            assert_eq!(body, saved);
            c.completion = None;
            c.points = Some(&[(128, 0)]);
            assert_eq!(c.build(&mut body), Err(Error::Invalid));
            assert_eq!(body, saved);
            c.points = None;
            c.descriptor = base + 1;
            assert_eq!(c.build(&mut body), Err(Error::Invalid));
            assert_eq!(body, saved);
            for pair in 0..2 {
                let (points, count) = paired_dependencies(kind, pair, case).unwrap();
                emit(&[pair, count as u8]);
                for &(queue, value) in &points[..count] {
                    emit(&[queue]);
                    emit(&value.to_le_bytes());
                }
            }
        }
    }
    assert!(paired_dependencies(Kind::Tiling, 2, 0).is_err());
    assert!(paired_dependencies(Kind::Fragment, 0, (1 << 30) - 1).is_err());
    assert!(operand_directory_size(usize::MAX).is_err());
}
