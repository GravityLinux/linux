// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! The two source-built descriptor graphs, before any firmware publication.
//!
//! Port of agx_g17p_boot.py::build_initdata (the current, non-legacy path).
//! Storage belongs to the caller. No CPU pointer or physical address is embedded
//! in these objects: every pointer names a GPU virtual address. The runtime must
//! install the mappings and aliases below before publishing either root.

use super::{g17p_abi as abi, g17p_layout as layout};
use abi::InvalidSize;
use layout::*;

pub(crate) const PAGE: usize = 0x4000;
pub(crate) const OBJECTS: usize = 12;
const PRIVATE: usize = 0;
const FWCTL: usize = 1;
const BUNDLE: usize = 2;
const HWDATA_REGIONS: usize = 3;
const ROOTS: usize = 5;
const REGION_C: usize = 7;
const REGION_A: usize = 8;
const COMPUTED: usize = 9;
const PRIMARY_REGIONS: usize = 10;

#[derive(Clone, Copy)]
pub(crate) struct Object {
    pub(crate) name: &'static str,
    pub(crate) offset: usize,
    pub(crate) size: usize,
    pub(crate) pte_flags: u64,
}

// OS=1, AP=1, AF=1, valid page; private firmware objects are UXN.
const SHARED: u64 = 0x00c0_0000_0000_044b;
const CACHED: u64 = 0x00c0_0000_0000_0443;
const ROOT_SHARED: u64 = 0x0080_0000_0000_044b;

pub(crate) const OBJECT_LAYOUT: [Object; OBJECTS] = [
    Object {
        name: "private",
        offset: NATIVE_PRIVATE_CLUSTER_OFFSET,
        size: NATIVE_PRIVATE_CLUSTER_SIZE,
        pte_flags: SHARED,
    },
    Object {
        name: "fwctl",
        offset: NATIVE_FWCTL_OFFSET,
        size: PAGE,
        pte_flags: SHARED,
    },
    Object {
        name: "bundle",
        offset: NATIVE_HWDATA_OFFSET,
        size: NATIVE_SHARED_CLUSTER_SIZE,
        pte_flags: CACHED,
    },
    Object {
        name: "hwregion0",
        offset: NATIVE_HWDATA_OFFSET + NATIVE_HWDATA_REGION_OFFSETS[0],
        size: PAGE,
        pte_flags: CACHED,
    },
    Object {
        name: "hwregion1",
        offset: NATIVE_HWDATA_OFFSET + NATIVE_HWDATA_REGION_OFFSETS[1],
        size: PAGE,
        pte_flags: CACHED,
    },
    Object {
        name: "root0",
        offset: NATIVE_ROOT_OFFSET,
        size: PAGE,
        pte_flags: ROOT_SHARED,
    },
    Object {
        name: "root1",
        offset: NATIVE_ROOT_OFFSET + SECONDARY_ROOT_DELTA,
        size: PAGE,
        pte_flags: ROOT_SHARED,
    },
    Object {
        name: "region_c",
        offset: NATIVE_ROOT_OFFSET - SECONDARY_ROOT_DELTA,
        size: PAGE,
        pte_flags: ROOT_SHARED,
    },
    Object {
        name: "region_a",
        offset: NATIVE_ROOT_OFFSET + 2 * SECONDARY_ROOT_DELTA,
        size: PAGE,
        pte_flags: ROOT_SHARED,
    },
    Object {
        name: "computed",
        offset: NATIVE_PRIMARY_COMPUTED_PAGE_OFFSET,
        size: PAGE,
        pte_flags: SHARED,
    },
    Object {
        name: "primary_region0",
        offset: PRIMARY_VIEWS[1].0,
        size: PAGE,
        pte_flags: SHARED,
    },
    Object {
        name: "primary_region1",
        offset: PRIMARY_VIEWS[2].0,
        size: PAGE,
        pte_flags: SHARED,
    },
];

pub(crate) trait Storage {
    fn object(&mut self, index: usize) -> Result<&mut [u8], InvalidSize>;
}

pub(crate) struct Graph {
    pub(crate) base: u64,
    pub(crate) addresses: [u64; OBJECTS],
    pub(crate) channels: [[abi::Channel; abi::CHANNELS]; 2],
}

impl Graph {
    pub(crate) fn roots(&self) -> [u64; 2] {
        [self.addresses[ROOTS], self.addresses[ROOTS + 1]]
    }

    /// Required context-0 low aliases of the two primary region objects.
    pub(crate) fn primary_aliases(&self) -> [(u64, u64); 2] {
        [
            (0x7001838000, self.addresses[PRIMARY_REGIONS]),
            (0x7001840000, self.addresses[PRIMARY_REGIONS + 1]),
        ]
    }
}

fn overlay(out: &mut [u8], runs: &[(usize, &[u8])]) -> Result<(), InvalidSize> {
    for (offset, bytes) in runs {
        let end = offset.checked_add(bytes.len()).ok_or(InvalidSize)?;
        out.get_mut(*offset..end)
            .ok_or(InvalidSize)?
            .copy_from_slice(bytes);
    }
    Ok(())
}

pub(crate) fn build(
    storage: &mut impl Storage,
    base: u64,
    performance: &abi::Performance,
) -> Result<Graph, InvalidSize> {
    if base & (PAGE as u64 - 1) != 0 {
        return Err(InvalidSize);
    }
    let mut graph = Graph {
        base,
        addresses: [0; OBJECTS],
        channels: [[abi::Channel::default(); abi::CHANNELS]; 2],
    };
    // Check the complete address range and storage geometry before changing bytes.
    for (index, object) in OBJECT_LAYOUT.iter().enumerate() {
        let address = base.checked_add(object.offset as u64).ok_or(InvalidSize)?;
        address.checked_add(object.size as u64).ok_or(InvalidSize)?;
        if storage.object(index)?.len() != object.size {
            return Err(InvalidSize);
        }
        graph.addresses[index] = address;
    }
    for index in 0..OBJECTS {
        storage.object(index)?.fill(0);
    }
    let bundle_address = graph.addresses[BUNDLE];
    let regions = core::array::from_fn::<_, 2, _>(|index| abi::RegionRecord {
        lead: REGION_SCALARS[index].0,
        value: REGION_SCALARS[index].1,
        address: graph.addresses[HWDATA_REGIONS + index],
        size_a: 0x800,
        size_b: 0x40,
        trail: REGION_SCALARS[index].2,
    });
    let bundle = storage.object(BUNDLE)?;
    abi::HardwareData {
        registers: REGISTERS,
        flags: REGISTER_FLAGS,
        performance,
        chip: Some(0x8140),
        regions: &regions,
        opaque: None,
    }
    .build(&mut bundle[..abi::HWDATA_SIZE])?;
    overlay(bundle, BUNDLE_RUNS)?;
    // The legacy views overlap transport rings: clear them AFTER the overlays.
    for offset in NATIVE_WORK_RING_OFFSETS {
        bundle[offset..offset + RING_STRIDE].fill(0);
    }
    abi::region_c(&mut storage.object(REGION_C)?[..abi::REGION_C_SIZE])?;

    for slot in 0..2 {
        let main_offset = if slot == 0 {
            NATIVE_PRIMARY_MAIN_OFFSET
        } else {
            NATIVE_SECONDARY_MAIN_OFFSET
        };
        let main_address = bundle_address + main_offset as u64;
        let state_offset = if slot == 0 {
            NATIVE_PRIMARY_WORK_STATE_OFFSET
        } else {
            NATIVE_SECONDARY_WORK_STATE_OFFSET
        };
        let status_a_offset = if slot == 0 {
            NATIVE_PRIMARY_STATUS_A_OFFSET
        } else {
            NATIVE_SECONDARY_STATUS_A_OFFSET
        };
        let state_address = base + state_offset as u64;
        let status_a = base + status_a_offset as u64;
        let status_b = if slot == 0 {
            state_address + NATIVE_STATUS_B_OFFSET as u64
        } else {
            0
        };
        let channels = &mut graph.channels[slot];
        for (index, channel) in channels.iter_mut().enumerate() {
            if index <= 12 {
                channel.states = core::array::from_fn(|state| {
                    state_address + (NATIVE_WORK_STATE_OFFSETS[index] + state * 0x10) as u64
                });
            } else if let Some((_, offsets)) = TRAILING_STATES.iter().find(|(i, _)| *i == index) {
                channel.states = offsets.map(|offset| offset.map_or(0, |o| status_a + o as u64));
            }
            channel.ring = if index < 12 {
                bundle_address + NATIVE_WORK_RING_OFFSETS[index] as u64
            } else if index == 12 {
                main_address + 0x4c0
            } else {
                TRAILING_RINGS
                    .iter()
                    .find(|(i, _)| *i == index)
                    .map_or(0, |(_, offset)| status_a + *offset as u64)
            };
        }
        let addresses = MAIN_ADDR_OBJECT_OFFSETS.map(|offset| bundle_address + offset as u64);
        abi::MainConfig {
            hardware: bundle_address,
            repeated: bundle_address + MAIN_REPEATED_ADDR_OFFSET as u64,
            channels: *channels,
            addresses,
            region_views: if slot == 0 {
                PRIMARY_VIEWS.map(|(offset, value)| (base + offset as u64, value))
            } else {
                SECONDARY_VIEWS
            },
            secondary: slot == 1,
            secondary_extra: if slot == 1 {
                addresses[SECONDARY_EXTRA_ADDR_OBJECT] + SECONDARY_EXTRA_ADDR_OFFSET as u64
            } else {
                0
            },
        }
        .build(&mut storage.object(BUNDLE)?[main_offset..main_offset + abi::MAIN_SIZE])?;

        let private = storage.object(PRIVATE)?;
        let status_offset = status_a_offset - NATIVE_PRIVATE_CLUSTER_OFFSET;
        abi::status(&mut private[status_offset..status_offset + 0x80], false)?;
        if slot == 0 {
            let offset = state_offset + NATIVE_STATUS_B_OFFSET - NATIVE_PRIVATE_CLUSTER_OFFSET;
            let fwctl = graph.addresses[FWCTL];
            abi::primary_status(
                &mut private[offset..offset + NATIVE_PRIMARY_STATUS_B_SIZE],
                fwctl,
                fwctl + 0x40,
                NATIVE_PRIMARY_STATUS_B_CONFIG_HEADER,
                NATIVE_PRIMARY_STATUS_B_CONFIG_OFFSET,
                NATIVE_PRIMARY_STATUS_B_CONFIG_RUNS,
            )?;
        }
        let secondary_extra = if slot == 1 {
            let offset = NATIVE_SECONDARY_ROOT_EXTRA_OFFSETS[1] - NATIVE_PRIVATE_CLUSTER_OFFSET;
            private[offset..offset + 0x80].fill(0);
            overlay(
                &mut private[offset..offset + 0x80],
                NATIVE_SECONDARY_ROOT_EXTRA_1_RUNS,
            )?;
            NATIVE_SECONDARY_ROOT_EXTRA_OFFSETS.map(|offset| base + offset as u64)
        } else {
            [0; 2]
        };
        let root = abi::Root {
            version: ROOT_VERSION_VALUES,
            region_a: graph.addresses[REGION_A],
            main: main_address,
            region_c: graph.addresses[REGION_C],
            kind: slot as u32,
            status: [status_a, status_b],
            secondary_extra,
        };
        root.build(&mut storage.object(ROOTS + slot)?[..root.size()])?;
    }
    let placed = graph.channels[1][12].states[0] + HWDATA_STATE_AFTER_CONTROL_STATE as u64;
    storage.object(BUNDLE)?[HWDATA_BUNDLE_STATE_PTR..HWDATA_BUNDLE_STATE_PTR + 8]
        .copy_from_slice(&placed.to_le_bytes());
    Ok(graph)
}
