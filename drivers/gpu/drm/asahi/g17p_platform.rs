// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! T8140 boot resources and live platform inputs for the synchronous shim port.
//!
//! The loader preserves the original SGX ADT properties as little-endian bytes.
//! Its resource references and explicitly authored properties use DT endianness.
//! Validate this boundary before allocating UAT tables or starting either ASC.

use kernel::{c_str, device, of, prelude::*};

const PAGE: u64 = 0x4000;
const RAW_STATES: usize = 16;
const STATES: usize = 11;

#[derive(Clone, Copy, Default)]
pub(crate) struct Region {
    pub(crate) base: u64,
    pub(crate) size: u64,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct PerfState {
    pub(crate) frequency_a_mhz: u32,
    pub(crate) frequency_b_mhz: u32,
    pub(crate) index_a: u32,
    pub(crate) index_b: u32,
    pub(crate) core_mv: u32,
    pub(crate) memory_mv: u32,
}

pub(crate) struct Platform {
    pub(crate) regions: [Region; 6],
    pub(crate) private_vm: [u64; 2],
    pub(crate) performance: [PerfState; STATES],
    pub(crate) period_ms: u32,
}

pub(crate) fn adt_u32(node: &of::Node, name: &CStr) -> Result<u32> {
    let bytes: KVec<u8> = node.get_property(name)?;
    Ok(u32::from_le_bytes(
        bytes.as_slice().try_into().map_err(|_| EINVAL)?,
    ))
}

fn adt_u64(node: &of::Node, name: &CStr) -> Result<u64> {
    let bytes: KVec<u8> = node.get_property(name)?;
    Ok(u64::from_le_bytes(
        bytes.as_slice().try_into().map_err(|_| EINVAL)?,
    ))
}

fn performance(node: &of::Node) -> Result<[PerfState; STATES]> {
    if adt_u32(node, c_str!("apple,sgx-perf-state-count"))? != RAW_STATES as u32
        || adt_u32(node, c_str!("apple,sgx-perf-state-table-count"))? != 1
    {
        return Err(EINVAL);
    }
    let core: KVec<u8> = node.get_property(c_str!("apple,sgx-perf-states"))?;
    let memory: KVec<u8> = node.get_property(c_str!("apple,sgx-perf-states-sram"))?;
    if core.len() != RAW_STATES * 8 || memory.len() != core.len() {
        return Err(EINVAL);
    }
    let word = |bytes: &[u8], row: usize, column: usize| {
        let offset = row * 8 + column * 4;
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    };
    let mut states = [PerfState::default(); STATES];
    let mut count = 0;
    let mut first = 0;
    while first < RAW_STATES {
        if count == STATES {
            return Err(EINVAL);
        }
        let core_mv = word(&core, first, 1);
        let memory_mv = word(&memory, first, 1);
        let mut last = first;
        while last + 1 < RAW_STATES && word(&core, last + 1, 1) == core_mv {
            last += 1;
        }
        let mut low = first;
        let mut high = first;
        for row in first..=last {
            let frequency = word(&core, row, 0);
            if frequency != word(&memory, row, 0)
                || frequency % 1_000_000 != 0
                || memory_mv != word(&memory, row, 1)
            {
                return Err(EINVAL);
            }
            if frequency < word(&core, low, 0) {
                low = row;
            }
            if frequency > word(&core, high, 0) {
                high = row;
            }
        }
        let state = PerfState {
            frequency_a_mhz: word(&core, high, 0) / 1_000_000,
            frequency_b_mhz: word(&core, low, 0) / 1_000_000,
            index_a: high as u32,
            index_b: low as u32,
            core_mv,
            memory_mv,
        };
        if core_mv == 0 || memory_mv == 0 {
            return Err(EINVAL);
        }
        if count == 0 {
            if state.frequency_a_mhz != 0 || state.frequency_b_mhz != 0 {
                return Err(EINVAL);
            }
        } else if core_mv <= states[count - 1].core_mv
            || state.frequency_a_mhz <= states[count - 1].frequency_a_mhz
            || state.frequency_b_mhz <= states[count - 1].frequency_b_mhz
        {
            return Err(EINVAL);
        }
        states[count] = state;
        count += 1;
        first = last + 1;
    }
    if count != STATES {
        return Err(EINVAL);
    }
    Ok(states)
}

impl Platform {
    /// Keep the shim's qualified calibration until a new profile is validated.
    /// Frequency/voltage and index inputs must agree with the live ADT tables;
    /// the remaining scale/relative ladders have no established derivation yet.
    pub(crate) fn firmware_performance(&self) -> Result<super::g17p_abi::Performance> {
        let expected = super::g17p_layout::PERFORMANCE;
        for (index, state) in self.performance.iter().enumerate() {
            if state.frequency_a_mhz != expected.freq_a[index]
                || state.frequency_b_mhz != expected.freq_b[index]
                || state.core_mv != expected.core_voltage[index]
                || state.memory_mv != expected.memory_voltage[index]
                || state.index_a != expected.index_a[index]
                || state.index_b != expected.index_b[index]
            {
                return Err(ENOTSUPP);
            }
        }
        Ok(expected)
    }

    pub(crate) fn new(dev: &device::Device) -> Result<Self> {
        let node = dev.of_node().ok_or(ENODEV)?;
        let abi: u32 = node.get_property(c_str!("apple,neo-boot-abi"))?;
        let firmware: KVec<u8> = node.get_property(c_str!("apple,firmware-build"))?;
        if abi != 1 || firmware.as_slice() != b"mBoot-18000.161.9\0" {
            dev_err!(dev, "G17P: unsupported loader/firmware ABI\n");
            return Err(ENOTSUPP);
        }
        let private_vm: KVec<u64> = node.get_property(c_str!("apple,rtkit-private-vm-region"))?;
        let private_vm: [u64; 2] = private_vm.as_slice().try_into().map_err(|_| EINVAL)?;
        if private_vm != [0xffff_fc00_0000_0000, 0x20_0000_0000] {
            return Err(EINVAL);
        }

        let mut regions = [Region::default(); 6];
        for (index, (name, base_property, size_property, minimum)) in [
            (
                c_str!("ttbs"),
                c_str!("apple,sgx-gpu-region-base"),
                c_str!("apple,sgx-gpu-region-size"),
                PAGE,
            ),
            (
                c_str!("pagetables"),
                c_str!("apple,sgx-gfx-shared-region-base"),
                c_str!("apple,sgx-gfx-shared-region-size"),
                0x80000,
            ),
            (
                c_str!("l2"),
                c_str!("apple,sgx-gfx-shared-l2-region-base"),
                c_str!("apple,sgx-gfx-shared-l2-region-size"),
                PAGE,
            ),
            (
                c_str!("handoff"),
                c_str!("apple,sgx-gfx-handoff-base"),
                c_str!("apple,sgx-gfx-handoff-size"),
                PAGE,
            ),
            (
                c_str!("firmware"),
                c_str!("apple,sgx-gfx-data-base"),
                c_str!("apple,sgx-gfx-data-size"),
                PAGE,
            ),
            (
                c_str!("firmware-secondary"),
                c_str!("apple,sgx-gfx1-data-base"),
                c_str!("apple,sgx-gfx1-data-size"),
                PAGE,
            ),
        ]
        .iter()
        .enumerate()
        {
            let resource = node.reserved_mem_region_to_resource_byname(name)?;
            let base = resource.start() as u64;
            let size = resource.size() as u64;
            let end = base.checked_add(size).ok_or(EINVAL)?;
            if base == 0
                || size < *minimum
                || (base | size) & (PAGE - 1) != 0
                || base != adt_u64(&node, base_property)?
                || size != adt_u64(&node, size_property)?
            {
                dev_err!(dev, "G17P: invalid {} reservation\n", name);
                return Err(EINVAL);
            }
            for other in &regions[..index] {
                if base < other.base + other.size && other.base < end {
                    return Err(EINVAL);
                }
            }
            regions[index] = Region { base, size };
            dev_info!(dev, "G17P: {} {:#x}+{:#x}\n", name, base, size);
        }
        let performance = performance(&node)?;
        let period_ms = adt_u32(&node, c_str!("apple,sgx-gpu-power-sample-period"))?;
        if period_ms == 0 || period_ms.checked_mul(24_000).is_none() {
            return Err(EINVAL);
        }
        dev_info!(dev, "G17P: boot resources verified, two firmware instances, {} performance states, max {} MHz, period {} ms\n",
            performance.len(), performance[STATES - 1].frequency_a_mhz, period_ms);
        Ok(Self {
            regions,
            private_vm,
            performance,
            period_ms,
        })
    }
}
