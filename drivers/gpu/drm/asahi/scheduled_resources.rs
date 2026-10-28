// SPDX-License-Identifier: GPL-2.0-only

//! Typed, reserved-memory contract for the single G16C firmware instance.
//! All original ADT interpretation belongs to m1n1. Addresses here come from
//! Linux resources backed by explicit reserved-memory phandles and no-map.

use kernel::{c_str, of, prelude::*};

pub(crate) const PAGE: u64 = 0x4000;
pub(crate) const REGIONS: usize = 5;

#[derive(Clone, Copy, Default)]
pub(crate) struct Region {
    pub(crate) base: u64,
    pub(crate) size: u64,
}

impl Region {
    pub(crate) fn contains_page(&self, page: u64) -> bool {
        page & (PAGE - 1) == 0
            && page >= self.base
            && page
                .checked_add(PAGE)
                .zip(self.base.checked_add(self.size))
                .is_some_and(|(end, limit)| end <= limit)
    }
}

pub(crate) struct ResourcesG16C {
    pub(crate) regions: [Region; REGIONS],
    pub(crate) private_vm: [u64; 2],
}

impl ResourcesG16C {
    pub(crate) fn from_fdt(node: &of::Node) -> Result<Self> {
        let firmware: KVec<u8> = node.get_property(c_str!("apple,firmware-build"))?;
        let names: KVec<u8> = node.get_property(c_str!("memory-region-names"))?;
        let private: KVec<u64> = node.get_property(c_str!("apple,rtkit-private-vm-region"))?;
        let handles: KVec<u32> = node.get_property(c_str!("memory-region"))?;
        if firmware.as_slice() != b"mBoot-18000.161.10\0"
            || names.as_slice() != b"ttbs\0pagetables\0l2\0handoff\0firmware\0"
            || private.as_slice() != [0xfffffc0000000000, 0x2000000000]
            || handles.len() != REGIONS
        {
            return Err(EINVAL);
        }
        let mut regions = [Region::default(); REGIONS];
        for (index, name) in [
            c_str!("ttbs"),
            c_str!("pagetables"),
            c_str!("l2"),
            c_str!("handoff"),
            c_str!("firmware"),
        ]
        .iter()
        .enumerate()
        {
            let region = node
                .parse_phandle(c_str!("memory-region"), index)
                .ok_or(ENODEV)?;
            let no_map: KVec<u8> = region.get_property(c_str!("no-map"))?;
            let status: KVec<u8> = region.get_property(c_str!("status"))?;
            if !no_map.is_empty() || status.as_slice() != b"okay\0" {
                return Err(EINVAL);
            }
            let resource = node.reserved_mem_region_to_resource_byname(name)?;
            let base = resource.start() as u64;
            let size = resource.size() as u64;
            let end = base.checked_add(size).ok_or(EINVAL)?;
            if base == 0 || size == 0 || (base | size) & (PAGE - 1) != 0 || end > 1 << 42 {
                return Err(EINVAL);
            }
            for previous in &regions[..index] {
                if base < previous.base + previous.size && previous.base < end {
                    return Err(EINVAL);
                }
            }
            regions[index] = Region { base, size };
        }
        Ok(Self {
            regions,
            private_vm: [private[0], private[1]],
        })
    }

    pub(crate) fn boot_table(&self, page: u64) -> bool {
        self.regions[1].contains_page(page) || self.regions[2].contains_page(page)
    }
}
