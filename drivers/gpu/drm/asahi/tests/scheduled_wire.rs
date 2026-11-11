// SPDX-License-Identifier: GPL-2.0-only
#![allow(dead_code)]

extern crate self as kernel;
pub mod macros {
    pub use host_versions::versions;
}

#[path = "../g17p_queue.rs"]
mod legacy;
#[path = "../g17p_queue.rs"]
mod production;
#[path = "../scheduled_wire.rs"]
mod wire;

// Also exercise the untouched legacy macro group in the same expansion crate.
#[host_versions::versions(AGX)]
#[repr(C)]
struct Legacy {
    common: u32,
    #[ver(V >= V13_5)]
    added: u32,
}

#[host_versions::versions(AGX)]
impl Legacy::ver {
    #[ver(G == G13)]
    pub fn marker() -> u32 {
        13
    }

    #[ver(G != G13)]
    pub(crate) fn marker() -> u32 {
        14
    }

    fn after_conditional_methods() -> bool {
        true
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|v| v == "--native-queue") {
        let bytes = std::fs::read(&args[2]).unwrap();
        assert_eq!(bytes.len(), wire::QueueRecordG16C::SIZE);
        let u32_at = |at| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let u64_at = |at| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        let record = wire::QueueRecordG16C::new(wire::QueueParameters {
            pointers: u64_at(0),
            ring: u64_at(8),
            jobs: u64_at(0x10),
            context: u64_at(0x9c),
            uuid: u32_at(0x48),
            priority: u32_at(0x28),
            unk_2c: u32_at(0x2c),
            unk_30: u64_at(0x30),
            unk_38: u32_at(0x38),
            prio5: u32_at(0x40),
            unk_94: u32_at(0x94),
        });
        assert_eq!(record.as_bytes(), bytes);
        return;
    }
    if args.get(1).is_some_and(|v| v == "--native-pointers") {
        let bytes = std::fs::read(&args[2]).unwrap();
        assert_eq!(bytes.len(), wire::QueuePointersG16C::SIZE);
        let write = u32::from_le_bytes(bytes[0x40..0x44].try_into().unwrap());
        let pointers = wire::QueuePointersG16C::with_write(write);
        assert_eq!(pointers.as_bytes(), bytes);
        return;
    }
    assert_eq!(core::mem::size_of::<LegacyG13V12_3>(), 4);
    assert_eq!(core::mem::size_of::<LegacyG14V12_4>(), 4);
    assert_eq!(core::mem::size_of::<LegacyG13V13_5>(), 8);
    assert_eq!(core::mem::size_of::<LegacyG14V13_5>(), 8);
    assert_eq!(core::mem::size_of::<LegacyG14XV13_5>(), 8);
    assert_eq!(LegacyG13V12_3::marker(), 13);
    assert_eq!(LegacyG14V12_4::marker(), 14);
    assert_eq!(LegacyG13V13_5::marker(), 13);
    assert_eq!(LegacyG14V13_5::marker(), 14);
    assert_eq!(LegacyG14XV13_5::marker(), 14);
    assert!(LegacyG13V12_3::after_conditional_methods());
    assert!(LegacyG14XV13_5::after_conditional_methods());

    for case in 0..256u32 {
        let capacity = case.wrapping_mul(0x1020_3041);
        let expected = legacy::pointers(capacity);
        assert_eq!(
            wire::QueuePointersG17P::with_capacity(capacity).as_bytes(),
            expected
        );
        assert_eq!(production::pointers(capacity), expected);
        let base = 0xffff_fc20_0000_0000 + u64::from(case) * 0x20_0000;
        let p = wire::QueueParameters {
            pointers: base + 0x4013,
            ring: base + 0x8029,
            jobs: base + 0xc043,
            context: base + 0x1005b,
            uuid: case.wrapping_mul(0x1020_3041),
            priority: case.wrapping_mul(0x1001_0001),
            unk_2c: case ^ 0x81ff_ff00,
            unk_30: 0xfeed_beef_1234_5678 ^ (u64::from(case) << 24),
            unk_38: case.rotate_left(17),
            prio5: case.rotate_right(9),
            unk_94: !case,
        };
        // Retain every existing Neo sentinel encoding, including an explicit
        // zero and full-width unknown words, rather than just normal defaults.
        for sentinel_size in 0..=6 {
            for explicit in [false, true] {
                let unk_30 = if explicit {
                    p.unk_30
                } else if sentinel_size == 0 {
                    0
                } else {
                    u64::MAX << (8 * (8 - sentinel_size))
                };
                let expected = legacy::Record {
                    pointers: p.pointers,
                    ring: p.ring,
                    job_list: p.jobs,
                    context: p.context,
                    uuid: p.uuid,
                    priority: p.priority,
                    prio5: p.prio5,
                    unk_2c: p.unk_2c,
                    unk_38: p.unk_38,
                    unk_30: explicit.then_some(p.unk_30),
                    unk_94: p.unk_94,
                    sentinel_size,
                }
                .build()
                .unwrap();
                let actual = wire::QueueRecordG17P::new(wire::QueueParameters { unk_30, ..p });
                assert_eq!(
                    actual.as_bytes(),
                    &expected[..wire::QueueRecordG17P::SIZE],
                    "Neo case {case}/{sentinel_size}/{explicit}"
                );
                let adopted = production::Record {
                    pointers: p.pointers,
                    ring: p.ring,
                    job_list: p.jobs,
                    context: p.context,
                    uuid: p.uuid,
                    priority: p.priority,
                    prio5: p.prio5,
                    unk_2c: p.unk_2c,
                    unk_38: p.unk_38,
                    unk_30: explicit.then_some(p.unk_30),
                    unk_94: p.unk_94,
                    sentinel_size,
                }
                .build()
                .unwrap();
                assert_eq!(
                    adopted, expected,
                    "adopted Neo case {case}/{sentinel_size}/{explicit}"
                );
            }
        }

        let priority = case % 4;
        let (mask, first, last) = match priority {
            0 => (0xffff_ffff_ffff_0000, 1, 1),
            1 => (0xffff_ffff_0000_0000, 0, 0),
            2 => (0xffff_0000_0000_0000, 0, 2),
            _ => (0, 0, 3),
        };
        let max = wire::QueueRecordG16C::new(wire::QueueParameters {
            priority,
            unk_2c: priority,
            unk_30: mask,
            unk_38: first,
            prio5: last,
            unk_94: 0,
            ..p
        });
        let hex: String = max.as_bytes().iter().map(|v| format!("{v:02x}")).collect();
        let pointers = wire::QueuePointersG16C::with_write(case * 0x10001);
        let pointer_hex: String = pointers
            .as_bytes()
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect();
        println!("{case} {hex} {pointer_hex}");
    }
}
