// SPDX-License-Identifier: GPL-2.0-only

//! G16C register configuration and firmware device mappings.
//! Like Asahi's SoC IOMapping tables, fixed apertures belong to the hardware
//! profile. m1n1 supplies the interrupt and MCC selection as typed FDT cells.
//! Linux never interprets raw ADT data here.

use super::scheduled_initdata::MmioRegion;
use kernel::{c_str, of, prelude::*};

const PAGE: u64 = 0x4000;
const SGX: u64 = 0x288000000;
const REGISTER_BASE: u64 = 0xfffffc2180200000;
const REGISTER_LIMIT: u64 = 0xfffffc2181000000;
const REGISTERS: usize = 25;
const MAX_MAPPINGS: usize = 60;
const MCC_BASE: u64 = 0x2201c4000;
const MCC_SIZE: u64 = 0x18000;

#[derive(Clone, Copy, Default)]
pub(crate) struct Mapping {
    pub(crate) address: u64,
    pub(crate) physical: u64,
    pub(crate) size: usize,
    pub(crate) pte_flags: u64,
}

pub(crate) struct MmioG16C {
    registers: [(usize, MmioRegion); REGISTERS],
    mappings: [Mapping; MAX_MAPPINGS],
    count: usize,
}

// SAFETY: Only integer/byte-array records and a zero count; no pointers,
// references or destructors. A zero image is valid empty CPU staging.
unsafe impl Zeroable for MmioG16C {}

impl MmioG16C {
    pub(crate) fn from_fdt(node: &of::Node) -> Result<Self> {
        Self::build(
            node.get_property(c_str!("apple,firmware-sw-interrupt"))?,
            node.get_property(c_str!("apple,active-mcc-mask"))?,
        )
    }

    pub(crate) fn from_fdt_boxed(node: &of::Node) -> Result<KBox<Self>> {
        let mut out = KBox::init(Self::init_zeroed(), GFP_KERNEL)?;
        out.populate(
            node.get_property(c_str!("apple,firmware-sw-interrupt"))?,
            node.get_property(c_str!("apple,active-mcc-mask"))?,
        )?;
        Ok(out)
    }

    fn build(software_interrupt: u64, mcc_mask: u32) -> Result<Self> {
        let mut out = Self::zeroed();
        out.populate(software_interrupt, mcc_mask)?;
        Ok(out)
    }

    fn populate(&mut self, software_interrupt: u64, mcc_mask: u32) -> Result {
        if !(PAGE..1 << 48).contains(&software_interrupt) || mcc_mask == 0 {
            return Err(EINVAL);
        }
        let first_mcc = MCC_BASE + ((mcc_mask.trailing_zeros() as u64) << 25);
        // slot, first PA, total bytes, element size, SGX-relative offset, access.
        // Slots 3 and 20 are strided physical arrays, not flat MMIO apertures.
        let rows: [(usize, u64, u64, u64, u64, u32); REGISTERS] = [
            (0, software_interrupt & !(PAGE - 1), PAGE, PAGE, 0, 2),
            (
                3,
                first_mcc,
                mcc_mask.count_ones() as u64 * MCC_SIZE,
                MCC_SIZE,
                0,
                2,
            ),
            (4, 0x50244c000, PAGE, PAGE, 0, 0),
            (9, 0x5003d0000, PAGE, PAGE, 0, 2),
            (10, 0x5003c0000, PAGE, PAGE, 0, 0),
            (11, 0x5003d8000, PAGE, PAGE, 0, 2),
            (12, 0x48565c000, PAGE, PAGE, 0, 2),
            (14, 0x500400000, 0x10000, 0x10000, 0, 0),
            (15, 0x50cdc0000, PAGE, PAGE, 0, 0),
            (17, SGX, 0x21400, 0x21400, 0, 2),
            (20, SGX + 0x3000000, 0x100000, 0x40000, 0, 2),
            (22, SGX + 0x1000000, 0x8000, 0x8000, 0, 2),
            (26, SGX + 0xd04000, 0x8000, 0x8000, 0xd04000, 2),
            (28, SGX + 0xd58000, 0x8000, 0x8000, 0xd58000, 2),
            (29, SGX + 0xd10000, PAGE, PAGE, 0xd10000, 2),
            (31, SGX + 0xd40000, PAGE, PAGE, 0xd40000, 2),
            (32, SGX + 0xd60000, 0x20000, 0x20000, 0xd60000, 2),
            (33, SGX + 0xd44000, PAGE, PAGE, 0xd44000, 2),
            (34, SGX + 0xd4c000, 0x200, 0x200, 0xd4c000, 2),
            (35, SGX + 0xe00000, PAGE, PAGE, 0xe00000, 2),
            (37, SGX + 0xe40000, PAGE, PAGE, 0xe40000, 2),
            (39, SGX + 0xe08000, 0x8000, 0x8000, 0, 2),
            (40, SGX + 0xe1c000, PAGE, PAGE, 0xe1c000, 2),
            (41, SGX + 0xe1f800, PAGE, PAGE, 0, 2),
            (43, SGX + 0xe60000, 0x58, 0x58, 0xe60000, 2),
        ];
        let mut cursor = REGISTER_BASE;
        for (index, (slot, physical, size, stride, relative, flags)) in rows.into_iter().enumerate()
        {
            let offset = physical & (PAGE - 1);
            let address = cursor + offset;
            self.registers[index] = (
                slot,
                MmioRegion {
                    physical: physical.to_le_bytes(),
                    address: address.to_le_bytes(),
                    size: (size as u32).to_le_bytes(),
                    stride: (stride as u32).to_le_bytes(),
                    relative: relative.to_le_bytes(),
                    flags: (flags as u32).to_le_bytes(),
                    ..Default::default()
                },
            );
            let pte_flags = if flags & 2 != 0 {
                0x00c0_0000_0000_0447
            } else {
                0x0080_0000_0000_0447
            };
            if slot == 3 {
                let mut window = 0;
                for bit in 0..32 {
                    if mcc_mask & (1 << bit) != 0 {
                        self.push(
                            address + window * MCC_SIZE,
                            MCC_BASE + ((bit as u64) << 25),
                            MCC_SIZE,
                            pte_flags,
                        )?;
                        window += 1;
                    }
                }
            } else if slot == 20 {
                for cluster in 0..4 {
                    self.push(
                        address + cluster * stride,
                        physical + cluster * 0x200000,
                        stride,
                        pte_flags,
                    )?;
                }
            } else {
                self.push(
                    cursor,
                    physical - offset,
                    (offset + size + PAGE - 1) & !(PAGE - 1),
                    pte_flags,
                )?;
            }
            cursor += ((offset + size + PAGE - 1) & !(PAGE - 1)) + PAGE;
        }
        if cursor > REGISTER_LIMIT {
            return Err(EINVAL);
        }
        // Hardware status is a separate shared-NC alias of slot 32's aperture.
        // Only install PTEs: never CPU-map or zero these bytes as RAM.
        self.push(
            0xfffffc218100c000,
            SGX + 0xd60000,
            0x20000,
            0x00c0_0000_0000_044b,
        )?;
        Ok(())
    }

    fn push(&mut self, address: u64, physical: u64, size: u64, pte_flags: u64) -> Result {
        let slot = self.mappings.get_mut(self.count).ok_or(EINVAL)?;
        *slot = Mapping {
            address,
            physical,
            size: size.try_into()?,
            pte_flags,
        };
        self.count += 1;
        Ok(())
    }

    pub(crate) fn registers(&self) -> &[(usize, MmioRegion)] {
        &self.registers
    }
    pub(crate) fn mappings(&self) -> &[Mapping] {
        &self.mappings[..self.count]
    }
}
