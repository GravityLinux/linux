// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from Niklas Sheth's linux-m4-integration prototype.

//! G16 client roots. The firmware bootstrap root is separate from these
//! roots, even when the hardware context table is using slot zero.

use crate::pgtable::{prot, UatPageTable, UAT_PGSZ};
use core::cell::Cell;
use kernel::{device, page::Page, prelude::*};

const IAS: usize = 42;
const OAS: u32 = 42;
const USC_BASE: u64 = 0x100_0000_0000;

/// Published root addresses. The submission owner must retain their VM until
/// GPU completion; copying these addresses does not transfer page ownership.
#[derive(Clone, Copy)]
pub(crate) struct Roots {
    pub(crate) low: u64,
    pub(crate) high: u64,
}

pub(crate) struct AddressSpace {
    pub(crate) low: UatPageTable,
    pub(crate) high: UatPageTable,
    // Per-Work compute views retain their own table snapshots until this VM
    // is destroyed. Their leaves share this VM's pinned GEM/private backing.
    pub(crate) views: KVec<AddressSpace>,
    // Drop the owned tables before releasing their mapped pages.
    pages: KVec<(u64, Owned<Page>)>,
    snapshots: KVec<Owned<Page>>,
}

impl AddressSpace {
    pub(crate) fn roots(&self) -> Roots {
        Roots {
            low: self.low.ttb(),
            high: self.high.ttb(),
        }
    }

    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            low: UatPageTable::new_with_ias(OAS, IAS)?,
            high: UatPageTable::new_with_ias(OAS, IAS)?,
            pages: KVec::new(),
            snapshots: KVec::new(),
            views: KVec::new(),
        })
    }

    pub(crate) fn fork(&self) -> Result<Self> {
        Ok(Self {
            low: self.low.fork()?,
            high: self.high.fork()?,
            pages: KVec::new(),
            snapshots: KVec::new(),
            views: KVec::new(),
        })
    }

    pub(crate) fn alloc_low(
        &mut self,
        va: u64,
        size: usize,
        access: crate::pgtable::Prot,
    ) -> Result {
        if size == 0 || (va as usize | size) & (UAT_PGSZ - 1) != 0 {
            return Err(EINVAL);
        }
        let end = va.checked_add(size as u64).ok_or(EINVAL)?;
        for v in (va..end).step_by(UAT_PGSZ) {
            if self.low.translate(v)?.is_some() {
                return Err(EEXIST);
            }
        }
        for v in (va..end).step_by(UAT_PGSZ) {
            let page = Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?;
            // These raw pages do not receive the GEM/DMA mapping's initial
            // publication. Clean CPU initialization before exposing them.
            clean_page(&page);
            let phys = page.phys();
            self.pages.push((v, page), GFP_KERNEL)?;
            self.low
                .map_pages(v..v + UAT_PGSZ as u64, phys, access, false)?;
        }
        Ok(())
    }

    /// Build backend-only render storage before this root is published.
    /// Allocation failures leave any acquired pages owned by this address space;
    /// callers discard the unpublished VM rather than reusing a partial graph.
    pub(crate) fn alloc_render_private(&mut self, start: u64, end: u64) -> Result {
        use crate::g16_render as render;
        let mut params = render::Parameters::default();
        params.set_private(start, end).ok_or(EINVAL)?;
        let pool = render::OperandPool::new(start, end).ok_or(EINVAL)?;
        self.alloc_low(start, render::PRIVATE_SIZE, prot::PROT_GPU_SHARED_RW)?;
        for offset in (0..render::PRIVATE_SIZE).step_by(UAT_PGSZ) {
            let va = start + offset as u64;
            let page = &self.pages.iter().find(|(v, _)| *v == va).ok_or(EIO)?.1;
            page.with_pointer_into_page(0, UAT_PGSZ, |ptr| {
                // SAFETY: The closure has exclusive access to the full page
                // owned by this unpublished VM; no GPU root references it.
                let bytes = unsafe { core::slice::from_raw_parts_mut(ptr, UAT_PGSZ) };
                render::private_page(&pool, offset, bytes).ok_or(EINVAL)
            })?;
            clean_page(page);
        }
        for (alias, offset, size) in render::private_aliases(&pool) {
            for page in (0..size).step_by(UAT_PGSZ) {
                let va = alias + page as u64;
                if self.low.translate(va)?.is_some() {
                    return Err(EEXIST);
                }
                let physical = self
                    .low
                    .translate(start + offset + page as u64)?
                    .ok_or(EIO)?;
                self.low.map_pages(
                    va..va + UAT_PGSZ as u64,
                    physical,
                    prot::PROT_GPU_SHARED_RW,
                    false,
                )?;
            }
        }
        self.sync();
        Ok(())
    }

    /// Allocate all ten-block growth pages before changing visible mappings.
    pub(crate) fn alloc_tvb_blocks(&mut self, base: u64, count: usize) -> Result<bool> {
        if count == 0 || count > 10 || base & 0x7fff != 0 {
            return Err(EINVAL);
        }
        let mut pages = KVec::new();
        if pages.reserve(count * 8, GFP_KERNEL).is_err()
            || self.pages.reserve(count * 8, GFP_KERNEL).is_err()
        {
            return Ok(false);
        }
        for block in 0..count {
            let start = base + block as u64 * 0x28000;
            for va in (start..start + 0x28000).step_by(UAT_PGSZ) {
                if self.low.translate(va)?.is_some() {
                    return Err(EEXIST);
                }
            }
            for va in (start..start + 0x20000).step_by(UAT_PGSZ) {
                let page = match Page::alloc_page(GFP_KERNEL | __GFP_ZERO) {
                    Ok(page) => page,
                    Err(_) => return Ok(false),
                };
                clean_page(&page);
                pages.push((va, page), GFP_KERNEL)?;
            }
        }
        self.pages.reserve(pages.len(), GFP_KERNEL)?;
        let first = self.pages.len();
        for page in pages {
            self.pages.push(page, GFP_KERNEL)?;
        }
        for (va, page) in &self.pages[first..] {
            self.low.map_pages(
                *va..*va + UAT_PGSZ as u64,
                page.phys(),
                prot::PROT_GPU_SHARED_RW,
                false,
            )?;
        }
        Ok(true)
    }

    pub(crate) fn write_low(&mut self, mut va: u64, mut bytes: &[u8]) -> Result {
        while !bytes.is_empty() {
            let base = va & !(UAT_PGSZ as u64 - 1);
            let page = &self.pages.iter().find(|(v, _)| *v == base).ok_or(EINVAL)?.1;
            let offset = (va - base) as usize;
            let size = bytes.len().min(UAT_PGSZ - offset);
            page.with_pointer_into_page(offset, size, |p| {
                // SAFETY: This range belongs to an unpublished client allocation.
                unsafe {
                    core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, size);
                }
                Ok(())
            })?;
            clean_page(page);
            va = va.checked_add(size as u64).ok_or(EINVAL)?;
            bytes = &bytes[size..];
        }
        Ok(())
    }

    /// Preserve the caller DVA (its table can contain self-pointers), while
    /// firmware preemption writes go to a fresh context-owned physical page.
    pub(crate) fn snapshot(
        &mut self,
        memory: &impl crate::g16_cdm::Memory,
        address: u64,
    ) -> Result {
        if !self.snapshots.is_empty() {
            return Err(EEXIST);
        }
        let page = Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?;
        page.with_pointer_into_page(0, UAT_PGSZ, |ptr| {
            // SAFETY: The fresh unpublished page is exclusively owned here.
            memory.read(address, unsafe {
                core::slice::from_raw_parts_mut(ptr, UAT_PGSZ)
            })
        })?;
        clean_page(&page);
        let phys = page.phys();
        self.snapshots.push(page, GFP_KERNEL)?;
        // Retain backing before the first possible translation change.
        self.low.unmap_pages(address..address + UAT_PGSZ as u64)?;
        self.low.map_pages(
            address..address + UAT_PGSZ as u64,
            phys,
            prot::PROT_GPU_SHARED_RW,
            false,
        )?;
        self.sync();
        crate::mem::tlbi_all();
        crate::mem::sync();
        Ok(())
    }

    /// Publish page-table edits. Owned client pages are cleaned when initialized;
    /// mapping changes do not need to sweep all existing client allocations.
    /// Firmware-private storage is managed separately by FirmwareSpace.
    pub(crate) fn sync(&self) {
        self.low.clean();
        self.high.clean();
        crate::mem::sync();
    }

    /// Exercise the ownership pattern needed for the firmware bootstrap
    /// root using private pages before touching firmware-owned tables.
    pub(crate) fn check_borrowed_root(&mut self, dev: &device::Device) -> Result {
        let a = Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?;
        let b = Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?;
        let page = UAT_PGSZ as u64;
        let far = 1u64 << 36;
        self.low
            .map_pages(page..2 * page, a.phys(), prot::PROT_GPU_SHARED_RW, false)?;
        let result = (|| {
            let mut borrowed =
                UatPageTable::new_with_ttb_and_ias(self.low.ttb(), 0..1u64 << IAS, OAS, IAS)?;
            // Extend both an existing leaf table and a previously empty L1 slot.
            borrowed.map_pages(
                2 * page..3 * page,
                b.phys(),
                prot::PROT_GPU_SHARED_RW,
                false,
            )?;
            borrowed.map_pages(far..far + page, b.phys(), prot::PROT_GPU_SHARED_RW, false)?;
            let valid = borrowed.translate(page)? == Some(a.phys())
                && borrowed.translate(2 * page)? == Some(b.phys())
                && borrowed.translate(far)? == Some(b.phys());
            borrowed.unmap_pages(2 * page..3 * page)?;
            borrowed.unmap_pages(far..far + page)?;
            drop(borrowed);
            if !valid
                || self.low.translate(page)? != Some(a.phys())
                || self.low.translate(far)?.is_some()
                || self.low.translate(2 * page)?.is_some()
            {
                return Err(EIO);
            }
            Ok(())
        })();
        self.low.unmap_pages(page..2 * page)?;
        result?;
        dev_info!(
            dev,
            "G16: borrowed-root teardown preserved existing mappings\n"
        );
        Ok(())
    }

    /// Bring-up check before either root is made visible to firmware. A
    /// legacy 39-bit mask aliases the two addresses exercised here, which
    /// would silently corrupt client data when installing the USC archive.
    pub(crate) fn check_geometry(&mut self, dev: &device::Device) -> Result {
        let a = Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?;
        let b = Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?;
        let low = UAT_PGSZ as u64;
        let usc = USC_BASE + low;
        self.low
            .map_pages(low..low + low, a.phys(), prot::PROT_GPU_SHARED_RW, false)?;
        if let Err(e) =
            self.low
                .map_pages(usc..usc + low, b.phys(), prot::PROT_GPU_SHARED_RW, false)
        {
            self.low.unmap_pages(low..low + low)?;
            return Err(e);
        }
        let result = (|| {
            if self.low.translate(low + 37)? != Some(a.phys() + 37)
                || self.low.translate(usc + 91)? != Some(b.phys() + 91)
                || self.high.translate(usc)?.is_some()
            {
                return Err(EIO);
            }
            self.low.unmap_pages(usc..usc + low)?;
            if self.low.translate(usc)?.is_some() || self.low.translate(low)? != Some(a.phys()) {
                return Err(EIO);
            }
            Ok(())
        })();
        // Remove both mappings on all paths before their backing pages drop.
        // The upper mapping has already been removed on the successful path.
        if self.low.translate(usc)?.is_some() {
            self.low.unmap_pages(usc..usc + low)?;
        }
        self.low.unmap_pages(low..low + low)?;
        result?;
        dev_info!(
            dev,
            "G16: 42-bit UAT mapping/unmapping verified across USC base; roots {:#x}/{:#x}\n",
            self.low.ttb(),
            self.high.ttb()
        );
        Ok(())
    }
}

use kernel::types::Owned;

struct FirmwareRegion {
    va: u64,
    pages: KVec<Owned<Page>>,
    // One flag per owned page. Live writes publish themselves; only allocation
    // and unpublished initialization need work at the next space-wide sync.
    dirty_pages: KVec<Cell<bool>>,
    pending: Cell<bool>,
}

/// The firmware uses this bootloader root independently of all client slots.
/// Its existing tables remain owned by the firmware reservation.
pub(crate) struct FirmwareSpace {
    table: UatPageTable,
    regions: KVec<FirmwareRegion>,
    external: KVec<core::ops::Range<u64>>,
}

impl FirmwareSpace {
    pub(crate) fn attach(dev: &device::Device) -> Result<Self> {
        let node = dev.of_node().ok_or(EINVAL)?;
        let res = node.reserved_mem_region_to_resource_byname(kernel::c_str!("pagetables"))?;
        let table = UatPageTable::new_with_ttb_and_ias(
            res.start(),
            0xffff_fc00_0000_0000..0xffff_ffff_ffff_c000,
            OAS,
            IAS,
        )?;
        dev_info!(dev, "G16: attached firmware root {:#x}\n", table.ttb());
        Ok(Self {
            table,
            regions: KVec::new(),
            external: KVec::new(),
        })
    }

    /// Borrow a hardware aperture. No ownership of physical pages is implied.
    pub(crate) fn map_external(
        &mut self,
        va: u64,
        pa: u64,
        size: usize,
        prot: crate::pgtable::Prot,
    ) -> Result {
        if size == 0 || (va | pa | size as u64) & (UAT_PGSZ as u64 - 1) != 0 {
            return Err(EINVAL);
        }
        let end = va.checked_add(size as u64).ok_or(EINVAL)?;
        for v in (va..end).step_by(UAT_PGSZ) {
            if self.table.translate(v)?.is_some() {
                return Err(EEXIST);
            }
        }
        // Retain the whole range first so partial mapping failures also unwind.
        self.external.push(va..end, GFP_KERNEL)?;
        self.table.map_pages(va..end, pa, prot, false)
    }

    pub(crate) fn unmap_external(&mut self, range: core::ops::Range<u64>) -> Result {
        self.table.unmap_pages(range.clone())?;
        self.table.sync();
        crate::mem::tlbi_all();
        crate::mem::sync();
        self.external
            .retain(|r| r.start < range.start || r.end > range.end);
        Ok(())
    }

    pub(crate) fn physical(&mut self, va: u64) -> Result<u64> {
        self.table.translate(va)?.ok_or(EINVAL)
    }

    pub(crate) fn alloc(&mut self, va: u64, size: usize, prot: crate::pgtable::Prot) -> Result {
        if size == 0 || (va as usize | size) & (UAT_PGSZ - 1) != 0 {
            return Err(EINVAL);
        }
        let end = va.checked_add(size as u64).ok_or(EINVAL)?;
        for v in (va..end).step_by(UAT_PGSZ) {
            if self.table.translate(v)?.is_some() {
                return Err(EEXIST);
            }
        }
        let mut pages = KVec::new();
        for _ in 0..size / UAT_PGSZ {
            pages.push(Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?, GFP_KERNEL)?;
        }
        let mut dirty_pages = KVec::new();
        dirty_pages.extend_with(pages.len(), Cell::new(true), GFP_KERNEL)?;
        self.regions.push(
            FirmwareRegion {
                va,
                pages,
                dirty_pages,
                pending: Cell::new(true),
            },
            GFP_KERNEL,
        )?;
        let region = self.regions.last().ok_or(EIO)?;
        for (i, page) in region.pages.iter().enumerate() {
            let v = va + (i * UAT_PGSZ) as u64;
            if let Err(e) = self
                .table
                .map_pages(v..v + UAT_PGSZ as u64, page.phys(), prot, false)
            {
                if v != va {
                    self.table.unmap_pages(va..v)?;
                }
                self.regions.pop();
                return Err(e);
            }
        }
        Ok(())
    }

    pub(crate) fn write(&mut self, va: u64, bytes: &[u8]) -> Result {
        let end = va.checked_add(bytes.len() as u64).ok_or(EINVAL)?;
        let region = self
            .regions
            .iter()
            .find(|r| va >= r.va && end <= r.va + (r.pages.len() * UAT_PGSZ) as u64)
            .ok_or(EINVAL)?;
        let mut offset = (va - region.va) as usize;
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let within = offset & (UAT_PGSZ - 1);
            let size = remaining.len().min(UAT_PGSZ - within);
            region.pending.set(true);
            region.dirty_pages[offset / UAT_PGSZ].set(true);
            region.pages[offset / UAT_PGSZ].with_pointer_into_page(within, size, |p| {
                // SAFETY: These owned bytes are not yet published to firmware.
                unsafe {
                    core::ptr::copy_nonoverlapping(remaining.as_ptr(), p, size);
                }
                Ok(())
            })?;
            remaining = &remaining[size..];
            offset += size;
        }
        Ok(())
    }

    /// Initialize a whole owned page before its contents are published.
    pub(crate) fn init_page(
        &mut self,
        va: u64,
        initialize: impl FnOnce(&mut [u8]) -> Result,
    ) -> Result {
        if va & (UAT_PGSZ as u64 - 1) != 0 {
            return Err(EINVAL);
        }
        let region = self
            .regions
            .iter()
            .find(|r| va >= r.va && va < r.va + (r.pages.len() * UAT_PGSZ) as u64)
            .ok_or(EINVAL)?;
        region.pending.set(true);
        region.dirty_pages[((va - region.va) as usize) / UAT_PGSZ].set(true);
        region.pages[((va - region.va) as usize) / UAT_PGSZ].with_pointer_into_page(
            0,
            UAT_PGSZ,
            |ptr| {
                // SAFETY: This page is owned by this space and initialization
                // occurs before it is published to firmware.
                initialize(unsafe { core::slice::from_raw_parts_mut(ptr, UAT_PGSZ) })
            },
        )
    }

    pub(crate) fn alloc_render_support(&mut self) -> Result {
        use crate::g16_render::Addresses;
        let a = Addresses::bootstrap();
        for (base, size) in a.private_regions() {
            self.alloc(base, size, prot::PROT_FW_PRIV_RW)?;
            for va in (base..base + size as u64).step_by(UAT_PGSZ) {
                self.init_page(va, |bytes| a.private_page(va, bytes).ok_or(EINVAL))?;
            }
        }
        for va in [a.tiling_shared_tail, a.fragment_shared_tail] {
            self.alloc(va, UAT_PGSZ, prot::PROT_FW_SHARED_RW)?;
        }
        self.alloc(
            crate::g16_render::SUPPORT_BASE,
            64 * UAT_PGSZ,
            prot::PROT_FW_SHARED_RW,
        )?;
        for i in 0..64 {
            let mut support = Addresses::bootstrap();
            support.support = crate::g16_render::SUPPORT_BASE + i * UAT_PGSZ as u64;
            self.init_page(support.support, |bytes| {
                support.private_page(support.support, bytes).ok_or(EINVAL)
            })?;
        }
        Ok(())
    }

    /// Update host-owned bytes, preserving adjacent firmware-owned cache lines.
    /// Cache maintenance completes once per range, not once per page.
    pub(crate) fn write_live(&mut self, va: u64, bytes: &[u8]) -> Result {
        self.update_live(va, bytes.len(), |ptr, offset, size| {
            // SAFETY: update_live supplies an exclusively host-owned destination
            // and a range within the source slice, with no alias between them.
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr().add(offset), ptr, size) };
        })
    }

    pub(crate) fn zero_live(&mut self, va: u64, size: usize) -> Result {
        self.update_live(va, size, |ptr, _, size| {
            // SAFETY: update_live supplies a valid host-owned byte range.
            unsafe { core::ptr::write_bytes(ptr, 0, size) };
        })
    }

    fn update_live(
        &mut self,
        va: u64,
        size: usize,
        mut update: impl FnMut(*mut u8, usize, usize),
    ) -> Result {
        let end = va.checked_add(size as u64).ok_or(EINVAL)?;
        let region = self
            .regions
            .iter()
            .find(|r| va >= r.va && end <= r.va + (r.pages.len() * UAT_PGSZ) as u64)
            .ok_or(EINVAL)?;
        if size == 0 {
            return Ok(());
        }
        let start = (va - region.va) as usize;
        let end = start + size;
        let first = start / UAT_PGSZ;
        let last = (end - 1) / UAT_PGSZ;
        for index in first..=last {
            // Unpublished initialization is still authoritative in CPU cache.
            if region.dirty_pages[index].get() {
                continue;
            }
            let base = index * UAT_PGSZ;
            let within = start.saturating_sub(base);
            let limit = (end - base).min(UAT_PGSZ);
            region.pages[index].with_page_mapped(|p| {
                for off in (within & !63..limit).step_by(64) {
                    // SAFETY: These cache lines lie within the mapped page.
                    unsafe { core::arch::asm!("dc ivac, {addr}", addr = in(reg) p.add(off)) };
                }
            });
        }
        crate::mem::sync();
        for index in first..=last {
            let base = index * UAT_PGSZ;
            let within = start.saturating_sub(base);
            let limit = (end - base).min(UAT_PGSZ);
            let dirty = region.dirty_pages[index].replace(false);
            region.pages[index].with_page_mapped(|p| {
                // SAFETY: The checked range lies in this mapped page. The
                // caller owns these bytes; neighboring firmware bytes were
                // invalidated before the batch's completion barrier above.
                unsafe { update(p.add(within), base + within - start, limit - within) };
                // Publish all initialization on a dirty page, including zeros
                // outside this write. Published pages need only touched lines.
                let lines = if dirty {
                    0..UAT_PGSZ
                } else {
                    (within & !63)..limit
                };
                for off in lines.step_by(64) {
                    // SAFETY: The cache line lies in this mapped page.
                    unsafe { core::arch::asm!("dc cvac, {addr}", addr = in(reg) p.add(off)) };
                }
            });
        }
        crate::mem::sync();
        Ok(())
    }

    /// Read one firmware-owned status word after invalidating the CPU view.
    /// Host writes to this cache line must have been synced before publication.
    pub(crate) fn read_u64(&self, va: u64) -> Result<u64> {
        Ok(u64::from(self.read_u32(va)?) | (u64::from(self.read_u32(va + 4)?) << 32))
    }

    /// Called only after the context is drained and its hardware roots detached.
    pub(crate) fn release_region(&mut self, va: u64) -> Result {
        let index = self.regions.iter().position(|r| r.va == va).ok_or(EINVAL)?;
        let size = self.regions[index].pages.len() * UAT_PGSZ;
        self.table.unmap_pages(va..va + size as u64)?;
        self.table.sync();
        crate::mem::tlbi_all();
        crate::mem::sync();
        self.regions.swap_remove(index);
        Ok(())
    }

    pub(crate) fn read_u32(&self, va: u64) -> Result<u32> {
        if va & 3 != 0 {
            return Err(EINVAL);
        }
        let end = va.checked_add(4).ok_or(EINVAL)?;
        let region = self
            .regions
            .iter()
            .find(|r| va >= r.va && end <= r.va + (r.pages.len() * UAT_PGSZ) as u64)
            .ok_or(EINVAL)?;
        let offset = (va - region.va) as usize;
        region.pages[offset / UAT_PGSZ].with_pointer_into_page(offset & (UAT_PGSZ - 1), 4, |p| {
            // SAFETY: The aligned word lies in this owned mapped page.
            // Firmware owns it and all CPU writes have reached PoC.
            unsafe {
                core::arch::asm!("dc ivac, {addr}", "dsb sy", addr = in(reg) p);
                Ok(u32::from_le(p.cast::<u32>().read_volatile()))
            }
        })
    }

    pub(crate) fn sync(&self) {
        for region in &self.regions {
            if !region.pending.replace(false) {
                continue;
            }
            for (page, dirty) in region.pages.iter().zip(&region.dirty_pages) {
                if dirty.replace(false) {
                    clean_page(page);
                }
            }
        }
        self.table.sync();
        crate::mem::tlbi_all();
        crate::mem::sync();
    }
}

impl Drop for FirmwareSpace {
    fn drop(&mut self) {
        // Only valid before publishing the graph or after firmware is stopped.
        // Remove leaf mappings before their backing pages are released.
        for region in &self.regions {
            let _ = self
                .table
                .unmap_pages(region.va..region.va + (region.pages.len() * UAT_PGSZ) as u64);
        }
        for range in &self.external {
            let _ = self.table.unmap_pages(range.clone());
        }
        self.table.sync();
        crate::mem::tlbi_all();
        crate::mem::sync();
    }
}

fn clean_page(page: &Page) {
    page.with_page_mapped(|p| {
        for off in (0..UAT_PGSZ).step_by(64) {
            // SAFETY: The cache line lies inside this owned page.
            unsafe {
                core::arch::asm!("dc cvac, {addr}", addr = in(reg) p.add(off));
            }
        }
    });
}
