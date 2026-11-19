// SPDX-License-Identifier: GPL-2.0-only

//! G16C's owned, unpublished single-instance boot graph.
//!
//! Addresses, permissions and aliases follow the output-qualified single-user
//! source builder. Allocation and publication remain separate: these are CPU
//! staging buffers, not references into memory which firmware can mutate.
//! The control ring starts inside MainConfig, so it shares the bundle owner.

use super::{
    scheduled_board::{BoardG16C, HardwarePointers},
    scheduled_boot_records::{PrimaryStatusG16C, RegionCG16C},
    scheduled_hwdata::{self as hardware, HardwareDataG16C},
    scheduled_initdata::{
        Channel, FirmwareRole, HardwareRegion, InitRootG16C, MainConfigG16C, MmioRegion,
    },
    scheduled_power::{self as power, PowerControllerG16C, PowerLinks, SettingsG16C},
};

pub(crate) const PAGE: usize = 0x4000;
pub(crate) const BASE: u64 = 0xffff_fc20_0000_0000;
pub(crate) const BUNDLE_OFFSET: usize = 0xc04d8000;
pub(crate) const MAIN_OFFSET: usize = 0x1e100;
pub(crate) const POWER_OFFSET: usize = 0x5240;
pub(crate) const OBJECTS: usize = 36;
const STATUS: usize = 2;
const REGION_C: usize = 3;
const ROOT: usize = 4;
const RESUME_FLAG: usize = 14;
const RESUME_TABLE: usize = 15;
const POLICY: usize = 19;
const BUNDLE: usize = 25;

/// Layout only; encoders copy each component's static Default template
/// directly into staging, without placing this large aggregate on the stack.
#[repr(C)]
pub(crate) struct FirmwareBundleG16C {
    pub(crate) hardware: HardwareDataG16C,
    pub(crate) pad_3db4: [u8; POWER_OFFSET - hardware::SIZE],
    pub(crate) power: PowerControllerG16C,
    pub(crate) entropy: [u8; 4],
    // Additional observed scalar locations. Their producer/meaning is not
    // established. The output-qualified source startup initializes them to
    // zero; never universalize this device's captured calibration values.
    pub(crate) pad_b618: [u8; 0x48],
    pub(crate) field_b660: [u8; 4],
    pub(crate) pad_b664: [u8; 8],
    pub(crate) fields_b66c: [[u8; 4]; 4],
    pub(crate) pad_b67c: [u8; 0x70],
    pub(crate) fields_b6ec: [[u8; 4]; 4],
    pub(crate) pad_b6fc: [u8; 0x40],
    pub(crate) field_b73c: [u8; 4],
    pub(crate) pad_b740: [u8; 4],
    pub(crate) field_b744: [u8; 4],
    pub(crate) unmodeled_b748: [u8; MAIN_OFFSET - 0xb748],
    pub(crate) main: MainConfigG16C,
    pub(crate) pad_1e600: [u8; 0x20000 - 0x1e600],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Object {
    pub(crate) offset: usize,
    pub(crate) size: usize,
    pub(crate) pte_flags: u64,
}

const SHARED: u64 = 0x00c0_0000_0000_044b;
const CACHED: u64 = 0x00c0_0000_0000_0443;
const ROOT_SHARED: u64 = 0x0080_0000_0000_044b;
const fn object(offset: usize, size: usize, pte_flags: u64) -> Object {
    Object {
        offset,
        size,
        pte_flags,
    }
}

// Coalesce adjacent RAM pages with equal permissions, keeping holes unmapped.
// MMIO (including the hardware-status alias) is never a RAM object to clear.
pub(crate) const OBJECT_LAYOUT: [Object; OBJECTS] = [
    object(0x20000, PAGE, SHARED),
    object(0x28000, PAGE, SHARED),
    object(0x30000, 0xc4000, SHARED),
    object(0xf8000, PAGE, ROOT_SHARED),
    object(0x100000, PAGE, ROOT_SHARED),
    object(0x10c000, PAGE, ROOT_SHARED),
    object(0x110000, PAGE, SHARED),
    object(0x118000, PAGE, SHARED),
    object(0x120000, PAGE, SHARED),
    object(0x128000, PAGE, SHARED),
    object(0x130000, PAGE, SHARED),
    object(0x140000, PAGE, SHARED),
    object(0x148000, PAGE, SHARED),
    object(0x150000, PAGE, SHARED),
    object(0x1b0000, PAGE, SHARED),
    object(0x1b8000, PAGE, ROOT_SHARED),
    object(0x1c0000, 0x10000, SHARED),
    object(0x1540000, PAGE, SHARED),
    object(0x1548000, PAGE, SHARED),
    object(0x1550000, PAGE, SHARED),
    object(0xc0018000, 0x94000, CACHED),
    object(0xc00b0000, 0x2a4000, CACHED),
    object(0xc0358000, 0x12c000, CACHED),
    object(0xc0488000, 0x34000, CACHED),
    object(0xc04c0000, 0x10000, CACHED),
    object(BUNDLE_OFFSET, 0x24000, CACHED),
    object(0xc0500000, PAGE, CACHED),
    object(0xc0510000, PAGE, CACHED),
    object(0xc0524000, PAGE, SHARED),
    object(0xc0528000, PAGE, CACHED),
    object(0xc0530000, PAGE, CACHED),
    object(0xc05e8000, 0x8000, CACHED),
    object(0xc4000000, 0x20000, CACHED),
    object(0xc4028000, 0x20000, CACHED),
    object(0x181000000, PAGE, SHARED),
    object(0x181008000, PAGE, SHARED),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Alias {
    pub(crate) low: u64,
    pub(crate) high: u64,
    pub(crate) size: usize,
    pub(crate) pte_flags: u64,
}
const fn alias(low: u64, offset: u64, size: usize, writable: bool) -> Alias {
    Alias {
        low,
        high: BASE + offset,
        size,
        pte_flags: if writable {
            0x00c0_0000_0000_0c8b
        } else {
            0x0080_0000_0000_0c8b
        },
    }
}

// Install these in context zero using the physical backing of the high view.
// Allocating a second zeroed buffer for a low view is not an alias.
pub(crate) const ALIASES: [Alias; 8] = [
    alias(0x7000000000, 0xc0018000, 0x94000, false),
    alias(0x7000098000, 0xc00b0000, 0x204000, false),
    alias(0x70002a0000, 0xc02b8000, 0x98000, false),
    alias(0x7000340000, 0xc0358000, 0xf4000, false),
    alias(0x7001838000, 0x1548000, PAGE, true),
    alias(0x7001840000, 0x1550000, PAGE, true),
    alias(0x7001848000, 0xc4000000, 0x20000, true),
    alias(0x7001870000, 0xc4028000, 0x20000, true),
];

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InvalidGraph;

/// Each index must return stable, exclusive storage for its complete owner.
/// Callers discard the unpublished image if construction returns an error.
pub(crate) trait Storage {
    fn object(&mut self, index: usize) -> Result<&mut [u8], InvalidGraph>;
}

pub(crate) struct Graph {
    pub(crate) channels: [Channel; 17],
}

impl Graph {
    pub(crate) fn root(&self, role: FirmwareRole) -> Option<u64> {
        match role {
            FirmwareRole::Primary => Some(BASE + 0x100000),
            FirmwareRole::Secondary => None,
        }
    }
}

fn region(lead: u32, low: u32, high: u64, trail: u32) -> HardwareRegion {
    HardwareRegion {
        lead: lead.to_le_bytes(),
        low: low.to_le_bytes(),
        high: high.to_le_bytes(),
        count: 0x800u32.to_le_bytes(),
        stride: 0x40u32.to_le_bytes(),
        trail: trail.to_le_bytes(),
        ..Default::default()
    }
}

pub(crate) fn build(
    storage: &mut impl Storage,
    board: &BoardG16C,
    registers: &[(usize, MmioRegion)],
    settings: &SettingsG16C,
    entropy: u32,
) -> Result<Graph, InvalidGraph> {
    if board.cs_count == 0 || board.cs_count > board.cs.len() {
        return Err(InvalidGraph);
    }
    let regions = [
        region(0x100, 0x1848000, BASE + 0xc4000000, 0),
        region(0, 0x1870000, BASE + 0xc4028000, 2),
    ];
    let hardware = board.hardware(
        registers,
        HardwarePointers {
            status: BASE + 0x18100c000,
            computed: BASE + 0x1540000,
            timestamp: BASE + 0x181400000,
        },
        Some(&regions),
    );
    hardware.validate().map_err(|_| InvalidGraph)?;
    let power = power::ParametersG16C::derive(board, settings).map_err(|_| InvalidGraph)?;
    // Reject all caller-controlled geometry/configuration before clearing RAM.
    for (index, object) in OBJECT_LAYOUT.iter().enumerate() {
        if storage.object(index)?.len() != object.size {
            return Err(InvalidGraph);
        }
    }
    for index in 0..OBJECTS {
        storage.object(index)?.fill(0);
    }

    let mut root = InitRootG16C::default();
    root.region_a = (BASE + 0x10c000).to_le_bytes();
    root.main = (BASE + BUNDLE_OFFSET as u64 + MAIN_OFFSET as u64).to_le_bytes();
    root.region_c = (BASE + 0xf8000).to_le_bytes();
    root.status_a = (BASE + 0x3f180).to_le_bytes();
    root.status_b = (BASE + 0x30000).to_le_bytes();
    root.extra_0 = (BASE + 0x3e440).to_le_bytes();
    root.extra_1 = (BASE + 0xf3440).to_le_bytes();
    storage.object(ROOT)?[..InitRootG16C::SIZE].copy_from_slice(root.as_bytes());
    RegionCG16C::encode(storage.object(REGION_C)?, board.maximum_power)
        .map_err(|_| InvalidGraph)?;
    let status = storage.object(STATUS)?;
    PrimaryStatusG16C::encode(status, BASE + 0x150000, BASE + 0x150040)
        .map_err(|_| InvalidGraph)?;
    status[0xf184..0xf188].copy_from_slice(&1u32.to_le_bytes());

    let mut main = MainConfigG16C::default();
    let bundle_address = BASE + BUNDLE_OFFSET as u64;
    main.hardware = bundle_address.to_le_bytes();
    main.repeated = (bundle_address + 0xc040).to_le_bytes();
    main.repeated_2 = main.repeated;
    let rings = [
        0x10240, 0x16240, 0x1c240, 0xea40, 0x14a40, 0x1aa40, 0xd240, 0x13240, 0x19240, 0xba40,
        0x11a40, 0x17a40, 0x1df00,
    ];
    for (i, ring) in rings.into_iter().enumerate() {
        let row = if i == 12 {
            0x300
        } else {
            (i % 3) * 0x100 + (3 - i / 3) * 0x40
        };
        let state = BASE + 0x20000 + row as u64;
        main.channels[i] = Channel {
            states: [
                state.to_le_bytes(),
                (state + 0x10).to_le_bytes(),
                (state + 0x20).to_le_bytes(),
            ],
            ring: (bundle_address + ring + 0x6c0).to_le_bytes(),
        };
    }
    for (i, values) in [
        (13, [0x3f1c0, 0x3f440, 0x3f200, 0x43c40]),
        (14, [0x3f3c0, 0xe5c40, 0x3f400, 0xeec40]),
    ] {
        main.channels[i] = Channel {
            states: core::array::from_fn(|j| (BASE + values[j]).to_le_bytes()),
            ring: (BASE + values[3]).to_le_bytes(),
        };
    }
    main.channels[15].states[0] = (BASE + 0x6c440).to_le_bytes();
    main.auxiliary = [0, 0x2740, 0x3380, 0x4400, 0xb7c0, 0xb840].map(|offset| {
        if offset == 0 {
            [0; 8]
        } else {
            (bundle_address + offset).to_le_bytes()
        }
    });
    main.regions = [
        BASE + 0xc0518000,
        0x7001838000,
        BASE + 0x1548000,
        0x7001840000,
        BASE + 0x1550000,
        0,
    ]
    .map(u64::to_le_bytes);
    main.unaligned = (bundle_address + POWER_OFFSET as u64).to_le_bytes();
    let bundle = storage.object(BUNDLE)?;
    hardware.build(bundle).map_err(|_| InvalidGraph)?;
    power
        .encode(
            &mut bundle[POWER_OFFSET..],
            PowerLinks {
                thermal_context: BASE + 0xf34c0,
                auxiliary: BASE + 0xc0500000,
            },
        )
        .map_err(|_| InvalidGraph)?;
    bundle[0xb614..0xb618].copy_from_slice(&entropy.to_le_bytes());
    bundle[MAIN_OFFSET..MAIN_OFFSET + MainConfigG16C::SIZE].copy_from_slice(main.as_bytes());
    // The opening opcode 0x16 is main.interval at +0x4c0. The rest of its
    // 0x40-byte slot is already zero; it must not get a separate allocation.
    storage.object(RESUME_FLAG)?[..4].copy_from_slice(&1u32.to_le_bytes());
    let resume = storage.object(RESUME_TABLE)?;
    for i in 0..96 {
        resume[i * 0x40..i * 0x40 + 8].copy_from_slice(&0x7000001822u64.to_le_bytes());
    }
    let policy = storage.object(POLICY)?;
    policy[..8].copy_from_slice(&(0x4000_0000_0000_0000u64 | (0x7000000000u64 >> 7)).to_le_bytes());
    policy[12..16].copy_from_slice(&(20u32 * 2048 * 2).to_le_bytes());
    policy[16..20].copy_from_slice(&(20u32 * 2048).to_le_bytes());
    Ok(Graph {
        channels: main.channels,
    })
}

const _: () = {
    assert!(core::mem::align_of::<FirmwareBundleG16C>() == 1);
    assert!(core::mem::size_of::<FirmwareBundleG16C>() == 0x20000);
    assert!(core::mem::offset_of!(FirmwareBundleG16C, power) == POWER_OFFSET);
    assert!(core::mem::offset_of!(FirmwareBundleG16C, entropy) == 0xb614);
    assert!(core::mem::offset_of!(FirmwareBundleG16C, field_b660) == 0xb660);
    assert!(core::mem::offset_of!(FirmwareBundleG16C, fields_b66c) == 0xb66c);
    assert!(core::mem::offset_of!(FirmwareBundleG16C, fields_b6ec) == 0xb6ec);
    assert!(core::mem::offset_of!(FirmwareBundleG16C, field_b73c) == 0xb73c);
    assert!(core::mem::offset_of!(FirmwareBundleG16C, field_b744) == 0xb744);
    assert!(core::mem::offset_of!(FirmwareBundleG16C, main) == MAIN_OFFSET);
    assert!(MAIN_OFFSET + core::mem::offset_of!(MainConfigG16C, interval) == 0x1df00 + 0x6c0);
    let mut i = 0;
    while i < OBJECTS {
        let object = OBJECT_LAYOUT[i];
        assert!(object.offset % PAGE == 0 && object.size > 0 && object.size % PAGE == 0);
        if i > 0 {
            assert!(OBJECT_LAYOUT[i - 1].offset + OBJECT_LAYOUT[i - 1].size <= object.offset);
        }
        i += 1;
    }
};
