// SPDX-License-Identifier: GPL-2.0-only OR MIT
// Host-side byte oracle adapter: compile the actual kernel serializers.
#![allow(dead_code)]

#[path = "../g17p_abi.rs"]
mod abi;

use std::{env, fs, path::Path};

fn save(path: &Path, name: &str, data: &[u8]) {
    fs::write(path.join(name), data).unwrap();
}

fn main() {
    let directory = env::args().nth(1).expect("output directory");
    let directory = Path::new(&directory);
    fs::create_dir_all(directory).unwrap();
    for case in 0..64u32 {
        let address = |index: u64| 0xffff_fc20_0000_0000 + (u64::from(case) << 24) + index * 0x4143;
        let root = abi::Root {
            version: [0x04c0 ^ case as u16, 0x0396, 0xa322, 0x0c8a],
            region_a: address(1),
            main: address(2),
            region_c: address(3),
            kind: case & 1,
            status: [address(4), address(5)],
            secondary_extra: match case % 3 {
                0 => [0, 0],
                1 => [address(6), 0],
                _ => [0, address(7)],
            },
        };
        let mut out = vec![0xa5; root.size()];
        root.build(&mut out).unwrap();
        save(directory, &format!("root{case}.bin"), &out);
        let mut wrong = vec![0xa5; root.size() - 1];
        assert_eq!(root.build(&mut wrong), Err(abi::InvalidSize));
        assert!(wrong.iter().all(|b| *b == 0xa5));

        let mut channels = [abi::Channel::default(); abi::CHANNELS];
        for (index, channel) in channels.iter_mut().enumerate() {
            let base = index as u64 * 4;
            *channel = abi::Channel {
                states: [address(base), address(base + 1), address(base + 2)],
                ring: address(base + 3),
            };
        }
        let main = abi::MainConfig {
            hardware: address(70),
            repeated: address(71),
            channels,
            addresses: [
                address(72),
                address(73),
                address(74),
                address(75),
                address(76),
            ],
            region_views: [
                (u64::MAX, Some(case)),
                (address(77), Some(0x01840000)),
                (address(78), None),
            ],
            secondary: case & 1 != 0,
            secondary_extra: if case & 2 != 0 { address(79) } else { 0 },
        };
        let mut out = vec![0xa5; abi::MAIN_SIZE];
        main.build(&mut out).unwrap();
        save(directory, &format!("main{case}.bin"), &out);
        let mut out = [0xa5; 0x20];
        channels[case as usize % abi::CHANNELS]
            .build(&mut out)
            .unwrap();
        save(directory, &format!("channel{case}.bin"), &out);

        let mut out = [0xa5; 0x28];
        abi::Register {
            physical: 0x480000000 + u64::from(case) * 0x1800,
            address: address(80),
            size: 0x21400 + case,
            relative: u64::MAX - u64::from(case),
            flags: case,
        }
        .build(&mut out)
        .unwrap();
        save(directory, &format!("register{case}.bin"), &out);
        let mut out = [0xa5; 0x40];
        abi::RegionRecord {
            lead: 0x100 + case,
            value: 0x1848000 + case,
            address: address(81),
            size_a: 0x800 + case,
            size_b: 0x40,
            trail: case & 3,
        }
        .build(&mut out)
        .unwrap();
        save(directory, &format!("region{case}.bin"), &out);

        let ladder = |column: u32| std::array::from_fn(|i| case * 1000 + column * 100 + i as u32);
        let perf = abi::Performance {
            freq_a: ladder(0),
            freq_b: ladder(1),
            core_voltage: ladder(2),
            memory_voltage: ladder(3),
            scale_b: ladder(4),
            relative_a: ladder(5),
            relative_b: ladder(6),
            index_a: ladder(7),
            index_b: ladder(8),
        };
        let registers = [
            (
                17,
                abi::Register {
                    physical: 0x480000000,
                    address: address(80),
                    size: 0x21400,
                    relative: u64::from(case),
                    flags: 2,
                },
            ),
            (
                41,
                abi::Register {
                    physical: 0x480e1f800,
                    address: address(81),
                    size: 0x4000,
                    relative: u64::MAX,
                    flags: 2,
                },
            ),
        ];
        let regions = [abi::RegionRecord {
            lead: 0x100,
            value: case,
            address: address(82),
            size_a: 0x800,
            size_b: 0x40,
            trail: 2,
        }];
        let opaque: &[(usize, &[u8])] = &[(0x20, &[0x12, 0x34, 0x56, 0x78])];
        let hwdata = abi::HardwareData {
            registers: &registers,
            flags: &[(0, case), (52, 2)],
            performance: &perf,
            chip: if case & 1 != 0 { Some(0x8140) } else { None },
            regions: &regions,
            opaque: match case % 3 {
                0 => None,
                1 => Some(&[]),
                _ => Some(opaque),
            },
        };
        let mut out = vec![0xa5; abi::HWDATA_SIZE];
        hwdata.build(&mut out).unwrap();
        save(directory, &format!("hardware{case}.bin"), &out);
        let mut out = vec![0xa5; 0x5000];
        abi::primary_status(
            &mut out,
            address(83),
            address(84),
            0x100,
            if case & 1 != 0 { 0x4000 } else { 0x4900 },
            opaque,
        )
        .unwrap();
        save(directory, &format!("primary_status{case}.bin"), &out);
    }
    let mut out = [0xa5; 0x20];
    abi::compute_dispatch(&mut out).unwrap();
    save(directory, "dispatch.bin", &out);
    let mut out = vec![0xa5; abi::REGION_C_SIZE];
    abi::region_c(&mut out).unwrap();
    save(directory, "region_c.bin", &out);
    for acknowledged in [false, true] {
        let mut out = [0xa5; 0x80];
        abi::status(&mut out, acknowledged).unwrap();
        save(
            directory,
            &format!("status{}.bin", u8::from(acknowledged)),
            &out,
        );
    }
}
