// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Cold source-built firmware address space. This is not a userspace VM yet.
//! Fixed placements are confined to the validated DT reservations; all other
//! backing belongs to Memory. Firmware-private top-table entries survive.

use super::{
    g17p_image::Image, g17p_initgraph, g17p_layout, g17p_memory::Memory, g17p_opening as opening,
    g17p_platform::Platform, g17p_topology as topology,
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
            for index in 0..(if delta == 0 { 3 } else { PAGE / 8 }) {
                let word = memory.read64(from + index * 8)?;
                if index < 3 && word & 3 != 3 {
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
        // The Python context constructor allocates each page independently,
        // before extent filling. The render view is distinct and remains blank
        // at publication; context 0 later aliases the generated high view.
        for (kind, &(high, low)) in opening::CONTEXTS.iter().enumerate() {
            for index in 0..opening::CONTEXT_PAGES {
                for (group, first, flags) in [
                    (2, high, 0x00c0_0000_0000_044b),
                    (1, low, 0x00c0_0000_0000_0c8b),
                ] {
                    let pa = memory.allocate(PAGE as usize)?;
                    if index == 0 && group == 2 {
                        memory.write(pa, &opening::context(kind).ok_or(EINVAL)?)?;
                    }
                    memory.clean(pa, PAGE as usize)?;
                    vm.span(
                        memory,
                        group,
                        first + index as u64 * PAGE,
                        pa,
                        PAGE as usize,
                        flags,
                    )?;
                }
            }
        }
        let channel = memory.allocate(PAGE as usize)?;
        memory.write(channel, &opening::channel_control())?;
        memory.clean(channel, PAGE as usize)?;
        vm.span(
            memory,
            2,
            opening::CHANNEL_CONTROL,
            channel,
            PAGE as usize,
            0x00c0_0000_0000_0443,
        )?;

        // Zero-backed render address-space shape. No fixture workload bytes
        // are installed. Actual caller BOs replace their own mappings later.
        let mut render = 0;
        for &(first, count, flags) in topology::RENDER_RUNS {
            for index in 0..count {
                let va = first + index as u64 * PAGE;
                if vm.lookup(memory, 1, va)?.is_none() {
                    let pa = memory.allocate(PAGE as usize)?;
                    memory.clean(pa, PAGE as usize)?;
                    vm.span(memory, 1, va, pa, PAGE as usize, flags)?;
                }
                render += 1;
            }
        }
        for va in opening::EXTRA_RENDER {
            let pa = memory.allocate(PAGE as usize)?;
            memory.clean(pa, PAGE as usize)?;
            vm.span(memory, 1, va, pa, PAGE as usize, 0x00c0_0000_0000_0c8b)?;
            render += 1;
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
        vm.write(memory, 2, opening::SUPPORT, &opening::support())?;
        vm.write(memory, 2, opening::STATE, &2u32.to_le_bytes())?;
        // The Python compute bootstrap keeps these cold render-owner records
        // even while both render queues are empty. Omitting them permits the
        // first CL command but later CL work retires without command status.
        vm.write(memory, 0, 0x7001838000, &opening::resource_record())?;
        vm.write(memory, 0, 0x7001840000, &opening::dispatch_record())?;
        vm.write(memory, 2, 0xfffffc20015e8000, &opening::dispatch_record())?;
        for (index, address) in [0xfffffc20c07d0000, 0xfffffc20c07f8000]
            .into_iter()
            .enumerate()
        {
            vm.write(memory, 2, address, &opening::current_jobs(index == 1))?;
        }
        // Stage both control rings before publishing initdata, with the same
        // already-consumed prefix as the current partial Python entrypoint.
        for (slot, channels) in image.graph.channels.iter().enumerate() {
            let control = &channels[12];
            vm.write(memory, 2, control.ring, &opening::message(slot == 1))?;
            for va in control.states {
                vm.write(memory, 2, va, &1u32.to_le_bytes())?;
            }
        }
        for index in 2..64 {
            let word = memory.read64(vm.roots[2] + index * 8)?;
            if word != 0 {
                memory.write64(vm.roots[2] + SECONDARY + index * 8, word)?;
            }
        }
        for table in &vm.tables {
            memory.clean(*table, PAGE as usize)?;
        }
        // bind_contexts(macos_table=True), then the separate context-0 root:
        // slots 0/1 tagged 0/1. Capture root enumeration is not a HW slot ID.
        memory.zero(platform.regions[0].base, 64 * 16)?;
        for (slot, low) in [(0u64, vm.roots[0]), (1, vm.roots[1])] {
            memory.write64(platform.regions[0].base + slot * 16, (slot << 48) | low | 1)?;
            memory.write64(
                platform.regions[0].base + slot * 16 + 8,
                (slot << 48) | vm.roots[2] | 1,
            )?;
        }
        memory.clean(platform.regions[0].base, 64 * 16)?;
        dev_info!(dev, "G17P: source VM ready: {} leaves, {} tables, {} extent pages, {} context-0 aliases, {} render pages; roots {:#x}/{:#x}/{:#x}\n",
            vm.leaves, vm.tables.len(), extent, aliases, render, vm.roots[0], vm.roots[1], vm.roots[2]);
        Ok(vm)
    }

    pub(crate) fn physical(&self, memory: &Memory, group: usize, va: u64) -> Result<u64> {
        let offset = va & (PAGE - 1);
        Ok((self.lookup(memory, group, va - offset)?.ok_or(EINVAL)? & ADDRESS) + offset)
    }

    /// Driver-owned RAM fields only. Each page is separately translated and
    /// bounded by Memory, then cleaned before a mailbox can expose the write.
    pub(crate) fn write(&self, memory: &mut Memory, group: usize, va: u64, bytes: &[u8]) -> Result {
        va.checked_add(bytes.len() as u64).ok_or(EINVAL)?;
        let mut offset = 0;
        while offset < bytes.len() {
            let at = va + offset as u64;
            let size = (bytes.len() - offset).min((PAGE - (at & (PAGE - 1))) as usize);
            let pa = self.physical(memory, group, at)?;
            // Every earlier host store through this API was cleaned. Preserve
            // adjacent firmware-owned bytes when a partial cache line is now
            // updated, particularly split counters and scheduler slots.
            memory.invalidate(pa, size)?;
            memory.write(pa, &bytes[offset..offset + size])?;
            memory.clean(pa, size)?;
            offset += size;
        }
        Ok(())
    }

    pub(crate) fn flush_tables(&self, memory: &Memory) -> Result {
        for &table in &self.tables {
            memory.clean(table, PAGE as usize)?;
        }
        super::g17p_memory::sync();
        Ok(())
    }

    /// Extend driver-owned firmware storage before first-work publication.
    pub(crate) fn ensure_firmware(&mut self, memory: &mut Memory, va: u64, size: usize) -> Result {
        let first = va & !(PAGE - 1);
        let end = va
            .checked_add(size as u64)
            .and_then(|v| v.checked_add(PAGE - 1))
            .ok_or(EINVAL)?
            & !(PAGE - 1);
        for at in (first..end).step_by(PAGE as usize) {
            if self.lookup(memory, 2, at)?.is_some() {
                continue;
            }
            let pa = memory.allocate(PAGE as usize)?;
            memory.clean(pa, PAGE as usize)?;
            let flags = if (0xfffffc20c0000000..0xfffffc20d0000000).contains(&at) {
                0x00c0000000000443
            } else {
                0x00c000000000044b
            };
            self.span(memory, 2, at, pa, PAGE as usize, flags)?;
        }
        Ok(())
    }

    /// Explicit first-work replacement of the source-owned context-0 alias.
    /// This is never used for caller mappings or after a firmware publication.
    pub(crate) fn alias_firmware(
        &mut self,
        memory: &mut Memory,
        high: u64,
        low: u64,
        size: usize,
    ) -> Result {
        if (high ^ low) & (PAGE - 1) != 0 {
            return Err(EINVAL);
        }
        self.ensure_firmware(memory, high, size)?;
        let first = high & !(PAGE - 1);
        let end = high
            .checked_add(size as u64)
            .and_then(|v| v.checked_add(PAGE - 1))
            .ok_or(EINVAL)?
            & !(PAGE - 1);
        for at in (first..end).step_by(PAGE as usize) {
            let pa = self.physical(memory, 2, at)?;
            self.span_inner(
                memory,
                0,
                (low & !(PAGE - 1)) + at - first,
                pa,
                PAGE as usize,
                0x0080000000000c8b,
                true,
            )?;
        }
        Ok(())
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
        self.span_inner(memory, group, va, pa, size, flags, false)
    }
    fn span_inner(
        &mut self,
        memory: &mut Memory,
        group: usize,
        va: u64,
        pa: u64,
        size: usize,
        flags: u64,
        replace: bool,
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
            if !replace && prior & 1 != 0 && prior != expected {
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
