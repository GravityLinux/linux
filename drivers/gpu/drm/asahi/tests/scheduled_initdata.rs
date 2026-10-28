// SPDX-License-Identifier: GPL-2.0-only
#![allow(dead_code)]

#[path = "../scheduled_initdata.rs"]
mod wire;

use std::{env, fs, path::Path};

fn save(directory: &Path, name: &str, bytes: &[u8]) {
    fs::write(directory.join(name), bytes).unwrap();
}

fn main() {
    let arguments: Vec<String> = env::args().collect();
    if arguments.get(1).is_some_and(|v| v == "--native-root") {
        let bytes = fs::read(&arguments[2]).unwrap();
        assert_eq!(bytes.len(), wire::InitRootG16C::SIZE);
        let qword = |at| bytes[at..at + 8].try_into().unwrap();
        let root = wire::InitRootG16C {
            region_a: qword(8),
            main: qword(0x18),
            region_c: qword(0x20),
            status_a: qword(0xa8),
            status_b: qword(0xb0),
            extra_0: qword(0xb8),
            extra_1: qword(0xc0),
            ..Default::default()
        };
        assert_eq!(root.as_bytes(), bytes);
        return;
    }
    if arguments.get(1).is_some_and(|v| v == "--native-main") {
        let bytes = fs::read(&arguments[2]).unwrap();
        assert_eq!(bytes.len(), wire::MainConfigG16C::SIZE);
        let qword = |at| bytes[at..at + 8].try_into().unwrap();
        let main = wire::MainConfigG16C {
            hardware: qword(0),
            repeated: qword(8),
            repeated_2: qword(0x10),
            channels: core::array::from_fn(|i| wire::Channel {
                states: core::array::from_fn(|j| qword(0x20 + i * 0x20 + j * 8)),
                ring: qword(0x20 + i * 0x20 + 0x18),
            }),
            auxiliary: core::array::from_fn(|i| qword(0x24c + i * 8)),
            regions: core::array::from_fn(|i| qword(0x2d0 + i * 8)),
            unaligned: qword(0x471),
            ..Default::default()
        };
        assert_eq!(main.as_bytes(), bytes);
        return;
    }
    let directory = arguments.get(1).expect("output directory");
    let directory = Path::new(&directory);
    fs::create_dir_all(directory).unwrap();
    assert_eq!(wire::G16C_ROLES, &[wire::FirmwareRole::Primary]);
    assert_eq!(
        wire::G17P_ROLES,
        &[wire::FirmwareRole::Primary, wire::FirmwareRole::Secondary]
    );
    assert_eq!(wire::InitRootG17P::default().kind, 0u32.to_le_bytes());
    assert_eq!(
        wire::SecondaryInitRootG17P::default().root.kind,
        1u32.to_le_bytes()
    );
    let primary = wire::MainConfigG17P::for_role(wire::FirmwareRole::Primary);
    let secondary = wire::MainConfigG17P::for_role(wire::FirmwareRole::Secondary);
    assert_eq!(primary.kind, 0u32.to_le_bytes());
    assert_eq!(primary.byte_mask, 0xffu32.to_le_bytes());
    assert_eq!(primary.interval, 0x16u32.to_le_bytes());
    assert_eq!(secondary.kind, 4u32.to_le_bytes());
    assert_eq!(secondary.byte_mask, 0u32.to_le_bytes());
    assert_eq!(secondary.interval, 0x2au32.to_le_bytes());
    // A kind word of four does not universally mean a secondary instance.
    assert_eq!(wire::MainConfigG16C::default().kind, 4u32.to_le_bytes());
    for case in 0..64u32 {
        let address = |index: u64| 0xffff_fc20_0000_0000 + (u64::from(case) << 24) + index * 0x4143;
        let root = wire::InitRootG16C {
            region_a: address(1).to_le_bytes(),
            main: address(2).to_le_bytes(),
            region_c: address(3).to_le_bytes(),
            status_a: address(4).to_le_bytes(),
            status_b: address(5).to_le_bytes(),
            extra_0: if case % 3 == 0 {
                [0; 8]
            } else {
                address(6).to_le_bytes()
            },
            extra_1: if case % 3 == 1 {
                [0; 8]
            } else {
                address(7).to_le_bytes()
            },
            ..Default::default()
        };
        save(directory, &format!("root{case}.bin"), root.as_bytes());
        let mut main = wire::MainConfigG16C {
            hardware: address(70).to_le_bytes(),
            repeated: address(71).to_le_bytes(),
            repeated_2: address(71).to_le_bytes(),
            auxiliary: core::array::from_fn(|i| address(72 + i as u64).to_le_bytes()),
            regions: core::array::from_fn(|i| address(80 + i as u64).to_le_bytes()),
            unaligned: address(90).to_le_bytes(),
            ..Default::default()
        };
        for (index, channel) in main.channels.iter_mut().enumerate() {
            let base = index as u64 * 4;
            *channel = wire::Channel {
                states: [
                    address(base).to_le_bytes(),
                    address(base + 1).to_le_bytes(),
                    address(base + 2).to_le_bytes(),
                ],
                ring: address(base + 3).to_le_bytes(),
            };
        }
        save(directory, &format!("main{case}.bin"), main.as_bytes());
        let region = wire::HardwareRegion {
            lead: case.to_le_bytes(),
            low: (case * 0x4000).to_le_bytes(),
            high: (0xffff_fc20_c000_0000 + u64::from(case) * 0x4000).to_le_bytes(),
            count: (case + 1).to_le_bytes(),
            stride: 0x40u32.to_le_bytes(),
            trail: case.rotate_left(9).to_le_bytes(),
            ..Default::default()
        };
        save(directory, &format!("region{case}.bin"), region.as_bytes());
        let mmio = wire::MmioRegion {
            physical: (0x28800_0000 + u64::from(case) * 0x8000).to_le_bytes(),
            address: address(91).to_le_bytes(),
            size: ((case + 1) * 0x1000).to_le_bytes(),
            stride: (0x4000u32 << (case & 1)).to_le_bytes(),
            relative: (u64::from(case) * 0x1000).to_le_bytes(),
            flags: (case & 3).to_le_bytes(),
            ..Default::default()
        };
        save(directory, &format!("mmio{case}.bin"), mmio.as_bytes());
    }
}
