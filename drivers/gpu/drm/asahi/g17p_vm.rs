// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Cold source-built firmware address space. This is not a userspace VM yet.
//! Fixed placements are confined to the validated DT reservations; all other
//! backing belongs to Memory. Firmware-private top-table entries survive.

use super::{
    g17p_image::Image, g17p_initgraph, g17p_layout, g17p_memory::Memory, g17p_platform::Platform,
    g17p_topology as topology,
};
use kernel::{device, prelude::*};

const PAGE: u64 = 0x4000;
const ADDRESS: u64 = 0x0000_ffff_ffff_c000;
const SECONDARY: u64 = 0x40000;

pub(crate) struct Vm {
    roots: [u64; 3],
    tables: KVec<u64>,
    claims: KVec<(u64, usize)>,
    leaves: usize,
}

fn target(group: usize, path: &[usize]) -> Option<u64> {
    topology::TABLE_TARGETS
        .iter()
        .find(|(g, p, _)| *g == group && *p == path)
        .map(|(_, _, pa)| *pa)
}

fn native(va: u64) -> Option<u64> {
    topology::LEAF_RUNS
        .iter()
        .find_map(|&(first, pa, count, stride)| {
            let index = va.checked_sub(first)? / PAGE;
            (index < count as u64).then(|| (pa as i64 + index as i64 * stride) as u64)
        })
}

impl Vm {
    pub(crate) fn build(
        dev: &device::Device,
        memory: &mut Memory,
        platform: &Platform,
        image: &Image,
    ) -> Result<Self> {
        let mut vm = Self {
            roots: [
                target(0, &[]).ok_or(EINVAL)?,
                target(1, &[]).ok_or(EINVAL)?,
                target(2, &[]).ok_or(EINVAL)?,
            ],
            tables: KVec::new(),
            claims: KVec::new(),
            leaves: 0,
        };
        for group in 0..2 {
            vm.table(memory, group, &[])?;
        }
        // Preserve each firmware instance's own code/data mappings and shared
        // L2 link. Match UAT.clear_stale_kernel_roots() for G15 and later.
        for delta in [0, SECONDARY] {
            let from = platform.regions[1].base + delta;
            let to = vm.roots[2] + delta;
            memory.zero(to, PAGE as usize)?;
            for index in 0..3 {
                let word = memory.read64(from + index * 8)?;
                if word & 3 != 3 {
                    return Err(EINVAL);
                }
                if index == 2 && word & ADDRESS != topology::SHARED_L2 {
                    return Err(EINVAL);
                }
                memory.write64(to + index * 8, word)?;
            }
            memory.clean(to, PAGE as usize)?;
            vm.tables.push(to, GFP_KERNEL)?;
        }
        vm.tables.push(topology::SHARED_L2, GFP_KERNEL)?;
        for &(group, path, _) in topology::TABLE_TARGETS {
            if group == 2
                && path.len() == 2
                && memory.read64(topology::SHARED_L2 + path[1] as u64 * 8)? != 0
            {
                return Err(EBUSY);
            }
        }
        // Arena allocation order is part of the native placement policy.
        for (index, object) in g17p_initgraph::OBJECT_LAYOUT.iter().enumerate() {
            let va = image.graph.addresses[index];
            let pa = vm.backing(memory, va, object.size)?;
            memory.write(pa, image.object(index)?)?;
            memory.clean(pa, object.size)?;
            vm.span(memory, 2, va, pa, object.size, object.pte_flags)?;
            dev_info!(
                dev,
                "G17P: {} DVA {:#x} PA {:#x}+{:#x}\n",
                object.name,
                va,
                pa,
                object.size
            );
        }
        for (_, register) in g17p_layout::REGISTERS {
            let offset = register.address & (PAGE - 1);
            if offset != register.physical & (PAGE - 1) {
                return Err(EINVAL);
            }
            let size = (offset + register.size as u64 + PAGE - 1) & !(PAGE - 1);
            // Install device PTEs only; never CPU-map or clear these registers.
            vm.span(
                memory,
                2,
                register.address - offset,
                register.physical - offset,
                size as usize,
                0x00c0_0000_0000_0447,
            )?;
        }
        // map_firmware_extent(): retain existing objects; fill the remaining
        // source shape with fresh zeroes. Split at native PA discontinuities.
        let mut extent = 0;
        for &(first, count, flags) in topology::FIRMWARE_RUNS {
            let mut index = 0;
            while index < count {
                let va = first + index as u64 * PAGE;
                if vm.lookup(memory, 2, va)?.is_some() {
                    index += 1;
                    continue;
                }
                let mut pages = 1;
                while index + pages < count {
                    let next = va + pages as u64 * PAGE;
                    if vm.lookup(memory, 2, next)?.is_some() {
                        break;
                    }
                    match (native(next - PAGE), native(next)) {
                        (None, None) => (),
                        (Some(a), Some(b)) if b == a + PAGE => (),
                        _ => break,
                    }
                    pages += 1;
                }
                let size = pages * PAGE as usize;
                let pa = vm.backing(memory, va, size)?;
                memory.clean(pa, size)?;
                let attributes = 0x0080_0000_0000_0443 | (flags & ((7 << 2) | (1 << 54)));
                vm.span(memory, 2, va, pa, size, attributes)?;
                extent += pages;
                index += pages;
            }
        }
        let mut aliases = 0;
        for &(first, count, high) in topology::CONTEXT0_PEERS {
            for index in 0..count {
                let low = first + index as u64 * PAGE;
                let high = high + index as u64 * PAGE;
                let pa = vm.lookup(memory, 2, high)?.ok_or(EINVAL)? & ADDRESS;
                let flags = topology::CONTEXT0_RUNS
                    .iter()
                    .find(|(start, count, _)| low >= *start && low < *start + *count as u64 * PAGE)
                    .ok_or(EINVAL)?
                    .2;
                vm.span(memory, 0, low, pa, PAGE as usize, flags)?;
                if vm.lookup(memory, 0, low)? != Some(pa | flags) {
                    return Err(EIO);
                }
                aliases += 1;
            }
        }
        for (low, high) in image.graph.primary_aliases() {
            if vm.lookup(memory, 0, low)?.ok_or(EINVAL)? & ADDRESS
                != vm.lookup(memory, 2, high)?.ok_or(EINVAL)? & ADDRESS
            {
                return Err(EIO);
            }
        }
        for index in 3..64 {
            memory.write64(
                vm.roots[2] + SECONDARY + index * 8,
                memory.read64(vm.roots[2] + index * 8)?,
            )?;
        }
        for table in &vm.tables {
            memory.clean(*table, PAGE as usize)?;
        }
        // Bind the same source context tags as Python. Render backing and
        // control staging will be added before the first workload doorbell.
        for (slot, tag, low) in [
            (0u64, 0u64, vm.roots[0]),
            (1, 64, vm.roots[1]),
            (2, 1, vm.roots[1]),
        ] {
            memory.write64(platform.regions[0].base + slot * 16, (tag << 48) | low | 1)?;
            memory.write64(
                platform.regions[0].base + slot * 16 + 8,
                (tag << 48) | vm.roots[2] | 1,
            )?;
        }
        memory.clean(platform.regions[0].base, 64 * 16)?;
        dev_info!(dev, "G17P: source VM ready: {} leaves, {} tables, {} extent pages, {} context-0 aliases; roots {:#x}/{:#x}/{:#x}\n",
            vm.leaves, vm.tables.len(), extent, aliases, vm.roots[0], vm.roots[1], vm.roots[2]);
        Ok(vm)
    }

    fn backing(&mut self, memory: &mut Memory, va: u64, size: usize) -> Result<u64> {
        if va & (PAGE - 1) != 0 || size == 0 || size % PAGE as usize != 0 {
            return Err(EINVAL);
        }
        let candidate = native(va).filter(|&pa| {
            (0..size)
                .step_by(PAGE as usize)
                .all(|offset| native(va + offset as u64) == Some(pa + offset as u64))
                && topology::RESERVATIONS
                    .iter()
                    .any(|&(base, len)| pa >= base && pa + size as u64 <= base + len)
                && !self
                    .claims
                    .iter()
                    .any(|&(base, len)| pa < base + len as u64 && base < pa + size as u64)
                && !topology::TABLE_TARGETS
                    .iter()
                    .any(|&(_, _, table)| table >= pa && table < pa + size as u64)
                && !(self.roots[2] + SECONDARY >= pa
                    && self.roots[2] + SECONDARY < pa + size as u64)
        });
        let pa = if let Some(pa) = candidate {
            self.claims.push((pa, size), GFP_KERNEL)?;
            memory.zero(pa, size)?;
            pa
        } else {
            memory.allocate(size)?
        };
        Ok(pa)
    }

    fn table(&mut self, memory: &mut Memory, group: usize, path: &[usize]) -> Result<u64> {
        let pa = if let Some(pa) = target(group, path) {
            pa
        } else {
            memory.allocate(PAGE as usize)?
        };
        if self.tables.contains(&pa) {
            return Err(EEXIST);
        }
        memory.zero(pa, PAGE as usize)?;
        memory.clean(pa, PAGE as usize)?;
        self.tables.push(pa, GFP_KERNEL)?;
        Ok(pa)
    }

    fn indices(group: usize, va: u64) -> Result<[usize; 3]> {
        if group > 2
            || va & (PAGE - 1) != 0
            || (if group == 2 {
                va < 0xffff_fc20_0000_0000 || va >= 0xffff_fc30_0000_0000
            } else {
                va >= 1 << 42
            })
        {
            return Err(EINVAL);
        }
        Ok([
            ((va >> 36) & 63) as usize,
            ((va >> 25) & 2047) as usize,
            ((va >> 14) & 2047) as usize,
        ])
    }

    fn lookup(&self, memory: &Memory, group: usize, va: u64) -> Result<Option<u64>> {
        let indices = Self::indices(group, va)?;
        let mut table = self.roots[group];
        for (level, index) in indices.into_iter().enumerate() {
            if !self.tables.contains(&table) {
                return Err(EINVAL);
            }
            let pte = memory.read64(table + index as u64 * 8)?;
            if pte & 1 == 0 {
                return Ok(None);
            }
            if pte & 3 != 3 {
                return Err(EINVAL);
            }
            if level == 2 {
                return Ok(Some(pte));
            }
            table = pte & ADDRESS;
        }
        Err(EINVAL)
    }

    fn span(
        &mut self,
        memory: &mut Memory,
        group: usize,
        va: u64,
        pa: u64,
        size: usize,
        flags: u64,
    ) -> Result {
        if size == 0
            || size % PAGE as usize != 0
            || pa & !ADDRESS != 0
            || flags & ADDRESS != 0
            || flags & 3 != 3
        {
            return Err(EINVAL);
        }
        va.checked_add(size as u64).ok_or(EINVAL)?;
        pa.checked_add(size as u64)
            .filter(|end| *end <= 1 << 42)
            .ok_or(EINVAL)?;
        for offset in (0..size).step_by(PAGE as usize) {
            let indices = Self::indices(group, va + offset as u64)?;
            let mut table = self.roots[group];
            for level in 0..2 {
                if !self.tables.contains(&table) {
                    return Err(EINVAL);
                }
                let at = table + indices[level] as u64 * 8;
                let mut pte = memory.read64(at)?;
                if pte & 1 == 0 {
                    let child = self.table(memory, group, &indices[..level + 1])?;
                    pte = child | 3;
                    memory.write64(at, pte)?;
                }
                if pte & 3 != 3 {
                    return Err(EINVAL);
                }
                table = pte & ADDRESS;
            }
            if !self.tables.contains(&table) {
                return Err(EINVAL);
            }
            let at = table + indices[2] as u64 * 8;
            let expected = (pa + offset as u64) | flags;
            let prior = memory.read64(at)?;
            if prior & 1 != 0 && prior != expected {
                return Err(EEXIST);
            }
            memory.write64(at, expected)?;
            if memory.read64(at)? != expected {
                return Err(EIO);
            }
            self.leaves += usize::from(prior & 1 == 0);
        }
        Ok(())
    }
}
