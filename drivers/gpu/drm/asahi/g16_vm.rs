// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from Niklas Sheth's linux-m4-integration prototype.

//! G16 client roots. The firmware bootstrap root is separate from these
//! roots, even when the hardware context table is using slot zero.

use crate::g16_alloc::Arena;
use crate::pgtable::{prot, UatPageTable, UAT_PGSZ};
use kernel::rbtree::{RBTree, RBTreeNode};
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
    // Drop the owned tables before releasing their mapped pages.
    pages: RBTree<u64, Owned<Page>>,
    compute_arena: Arena,
    render_arena: Arena,
    scratch_free: [KVec<core::ops::Range<u64>>; 2],
    scratch_users: [usize; 2],
}

/// An exclusive suballocation; its backing remains owned by the VM.
#[derive(Debug, PartialEq)]
pub(crate) struct Scratch {
    range: core::ops::Range<u64>,
    render: bool,
}

impl AddressSpace {
    /// Reused Work retains the same alias and backing. Existing aliases need
    /// no PTE edit or invalidation; conflicting client mappings are rejected.
    pub(crate) fn map_work_alias(&mut self, va: u64, pa: u64) -> Result {
        match self.low.translate(va)? {
            Some(existing) if existing == pa => Ok(()),
            Some(_) => Err(EEXIST),
            None => self.low.map_pages(
                va..va + UAT_PGSZ as u64,
                pa,
                prot::PROT_GPU_SHARED_RW,
                false,
            ),
        }
    }
    pub(crate) fn roots(&self) -> Roots {
        Roots {
            low: self.low.ttb(),
            high: self.high.ttb(),
        }
    }

    /// Read a command word only while the execution owns its GEM mappings.
    /// A decode must also wait for the GPU producer dependencies first; the
    /// unwaited first-word peek relies on userspace writing native headers on
    /// the CPU (see `submit_render`). Volatile access also avoids creating Rust
    /// references to memory userspace can still write.
    pub(crate) fn command_word(&mut self, va: u64) -> Result<u32> {
        if va & 3 != 0 {
            return Err(EINVAL);
        }
        let pa = self.low.translate(va)?.ok_or(EFAULT)?;
        let base = pa & !(UAT_PGSZ as u64 - 1);
        // SAFETY: the calling execution pins this VM's external mappings; the
        // runtime/VM mutexes prevent unmapping for the duration of this borrow.
        let page = unsafe { Page::borrow_phys(&base) }.ok_or(EFAULT)?;
        page.with_pointer_into_page((pa - base) as usize, 4, |ptr| {
            // SAFETY: the page helper checked the aligned word's bounds.
            Ok(u32::from_le(unsafe { ptr.cast::<u32>().read_volatile() }))
        })
    }

    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            low: UatPageTable::new_with_ias(OAS, IAS)?,
            high: UatPageTable::new_with_ias(OAS, IAS)?,
            pages: RBTree::new(),
            compute_arena: Arena::default(),
            render_arena: Arena::default(),
            scratch_free: [KVec::new(), KVec::new()],
            scratch_users: [0; 2],
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
            if self.pages.get(&v).is_some() || self.low.translate(v)?.is_some() {
                return Err(EEXIST);
            }
        }
        for v in (va..end).step_by(UAT_PGSZ) {
            let page = Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?;
            let phys = page.phys();
            self.pages.try_create_and_insert(v, page, GFP_KERNEL)?;
            self.low
                .map_pages(v..v + UAT_PGSZ as u64, phys, access, false)?;
        }
        Ok(())
    }

    /// Build backend-only render storage before this root is published.
    /// ENOMEM may leave a partial graph, all owned here. This root has never
    /// rendered: retry can finish missing pages/PTEs and reinitialize storage
    /// before its first publication. Never replace a conflicting mapping.
    pub(crate) fn alloc_render_private(&mut self, start: u64, end: u64) -> Result {
        use crate::g16_render as render;
        let mut params = render::Parameters::default();
        params.set_private(start, end).ok_or(EINVAL)?;
        let pool = render::OperandPool::new(start, end).ok_or(EINVAL)?;
        let remaining = pool.end().ok_or(EINVAL)?..end.min(render::CONTEXT_BASE + (1 << 32));
        if remaining.start > remaining.end {
            return Err(ENOMEM);
        }
        self.render_arena = Arena::new(remaining);
        for offset in (0..render::PRIVATE_SIZE).step_by(UAT_PGSZ) {
            let va = start + offset as u64;
            if self.pages.get(&va).is_none() {
                if self.low.translate(va)?.is_some() {
                    return Err(EEXIST);
                }
                let page = Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?;
                self.pages.try_create_and_insert(va, page, GFP_KERNEL)?;
            }
            let physical = self.pages.get(&va).ok_or(EIO)?.phys();
            self.map_render_page(va, physical)?;
        }
        for offset in (0..render::PRIVATE_SIZE).step_by(UAT_PGSZ) {
            let va = start + offset as u64;
            let page = self.pages.get(&va).ok_or(EIO)?;
            page.with_pointer_into_page(0, UAT_PGSZ, |ptr| {
                // SAFETY: The closure has exclusive access to the full page
                // owned by this unpublished VM; no GPU root references it.
                let bytes = unsafe { core::slice::from_raw_parts_mut(ptr, UAT_PGSZ) };
                render::private_page(&pool, offset, bytes).ok_or(EINVAL)
            })?;
        }
        for (alias, offset, size) in render::private_aliases(&pool) {
            for page in (0..size).step_by(UAT_PGSZ) {
                let va = alias + page as u64;
                let physical = self
                    .low
                    .translate(start + offset + page as u64)?
                    .ok_or(EIO)?;
                self.map_render_page(va, physical)?;
            }
        }
        self.sync();
        Ok(())
    }

    /// A failed first-render allocation can already own this exact PTE.
    fn map_render_page(&mut self, va: u64, physical: u64) -> Result {
        match self.low.translate(va)? {
            Some(existing) if existing == physical => Ok(()),
            Some(_) => Err(EEXIST),
            None => self.low.map_pages(
                va..va + UAT_PGSZ as u64,
                physical,
                prot::PROT_GPU_SHARED_RW,
                false,
            ),
        }
    }

    /// Each active render exclusively leases tile metadata and save storage.
    pub(crate) fn prepare_render(
        &mut self,
        p: &mut crate::g16_render::Parameters,
    ) -> Result<Scratch> {
        let _t = crate::g16_timeline::Timer::start(&crate::g16_timeline::P_PREPARE_RENDER);
        let (tilemap, tpc) = p.scratch_sizes().ok_or(EINVAL)?;
        let size = tilemap
            .checked_add(tpc)
            .and_then(|n| n.checked_add(5 * UAT_PGSZ))
            .and_then(|n| n.checked_add(if p.mesh.is_some() {
                crate::g16_mesh::Profile::PAYLOAD_BYTES
            } else { 0 }))
            .ok_or(ENOMEM)?;
        // Only the trailing metadata pages need clearing.
        let scratch = self.lease_scratch(true, size, tilemap + tpc)?;
        let base = scratch.range.start;
        p.tilemap = base;
        p.tpc = base + tilemap as u64;
        let meta = p.tpc + tpc as u64;
        p.layermeta = meta;
        p.heapmeta = meta + if p.layers > 1 { 0x100 } else { 0 };
        p.deflake_3 = meta + 0x4000;
        p.deflake_2 = p.deflake_3 + 0x20;
        p.deflake_1 = p.deflake_3 + 0x2a0;
        p.ta_status = meta + 0x8000 + 0x240;
        p.fragment_status = meta + 0xc000 + 0x2c0;
        p.aux_fb = meta + 0x10000;
        if let Some(mesh) = &mut p.mesh {
            mesh.payload = meta + 0x14000;
            // The lease lasts through both TA and fragment retirement. Clear
            // the entire payload, not only the metadata pages.
            for offset in (0..crate::g16_mesh::Profile::PAYLOAD_BYTES).step_by(256) {
                self.write_low(mesh.payload + offset as u64, &[0; 256])?;
            }
        }
        self.write_low(p.aux_fb + 0x600, &0x0000035b60000000u64.to_le_bytes())?;
        self.sync();
        Ok(scratch)
    }

    /// Compute commands use distinct private scratch addresses in the caller's
    /// reserved kernel aperture. The persistent root and all backing remain
    /// owned by the VM; allocating a later command never replaces a live PTE.
    pub(crate) fn init_compute_private(&mut self, range: core::ops::Range<u64>) {
        self.compute_arena = Arena::new(range);
    }

    pub(crate) fn prepare_compute(
        &mut self,
        p: &mut crate::g16_compute::Parameters,
    ) -> Result<Scratch> {
        let _t = crate::g16_timeline::Timer::start(&crate::g16_timeline::P_PREPARE_COMPUTE);
        let scratch = self.lease_scratch(false, 0x24000, 0x20000)?;
        let base = scratch.range.start;
        p.scratch = base;
        p.marker = base + 0x20000;
        Ok(scratch)
    }

    /// `metadata`: the lease offset from which a reused lease is cleared.
    fn lease_scratch(&mut self, render: bool, size: usize, metadata: usize) -> Result<Scratch> {
        if size == 0 || size & (UAT_PGSZ - 1) != 0 {
            return Err(EINVAL);
        }
        let index = usize::from(render);
        // Splitting a free range adds at most one future free-list entry.
        // Reserve now; completion and coalescing cannot allocate.
        self.scratch_free[index].reserve(self.scratch_users[index] + 1, GFP_KERNEL)?;
        let free = self.scratch_free[index]
            .iter()
            .position(|r| r.end - r.start >= size as u64);
        let range = if let Some(slot) = free {
            let start = self.scratch_free[index][slot].start;
            let end = start + size as u64;
            if end == self.scratch_free[index][slot].end {
                self.scratch_free[index].remove(slot).map_err(|_| EIO)?;
            } else {
                self.scratch_free[index][slot].start = end;
            }
            // All previous GPU writes have retired. Reinitialize the
            // metadata pages of the leased range (the tilemap, TPC and compute
            // scratch need no clearing; full VK CTS clean, #181) without
            // touching PTEs or neighbouring active jobs.
            let clear_from = (start + metadata as u64).min(end);
            for va in (clear_from..end).step_by(UAT_PGSZ) {
                let page = self.pages.get(&va).ok_or(EIO)?;
                page.with_page_mapped(|ptr| {
                    // SAFETY: The lease owns this full page exclusively.
                    unsafe { core::ptr::write_bytes(ptr, 0, UAT_PGSZ) };
                });
            }
            start..end
        } else {
            let arena = if render {
                &mut self.render_arena
            } else {
                &mut self.compute_arena
            };
            // Partially allocated failures stay quarantined until VM teardown.
            let range = arena.alloc(size as u64, UAT_PGSZ as u64).ok_or(ENOMEM)?;
            self.alloc_low(range.start, size, prot::PROT_GPU_SHARED_RW)?;
            range
        };
        self.scratch_users[index] += 1;
        Ok(Scratch { range, render })
    }

    pub(crate) fn release_scratch(&mut self, scratch: Scratch) -> Result {
        let index = usize::from(scratch.render);
        let mut range = scratch.range;
        let free = &mut self.scratch_free[index];
        let mut slot = 0;
        while slot < free.len() {
            if free[slot].end == range.start || range.end == free[slot].start {
                let old = free.remove(slot).map_err(|_| EIO)?;
                range.start = range.start.min(old.start);
                range.end = range.end.max(old.end);
                slot = 0;
            } else {
                slot += 1;
            }
        }
        free.push(range, GFP_KERNEL)?;
        self.scratch_users[index] -= 1;
        Ok(())
    }

    /// TVB blocks that still fit in the render arena's free VA (each block
    /// reserves 0x28000). Scratch shares the arena from its other end.
    pub(crate) fn tvb_va_blocks(&self) -> u32 {
        (self.render_arena.free() / 0x28000).min(u64::from(u32::MAX)) as u32
    }

    /// Allocate tiler blocks from the shared private aperture. Before mapping,
    /// failure leaves both existing backing and the arena reservation untouched.
    /// `gfp` comes from the caller: TVB growth must not enter reclaim. Page
    /// tables for the new range are still allocated by `map_pages`.
    pub(crate) fn alloc_tvb_blocks(&mut self, count: usize, gfp: kernel::alloc::Flags) -> Result<Option<u64>> {
        if count == 0 || count > crate::g16_tvb::MAX_BLOCKS as usize {
            return Err(EINVAL);
        }
        let mut arena = self.render_arena.clone();
        let Some(range) = arena.alloc_back(count as u64 * 0x28000, 0x8000) else {
            return Ok(None);
        };
        let base = range.start;
        let mut pages = KVec::new();
        if pages.reserve(count * 8, gfp).is_err() {
            return Ok(None);
        }
        for block in 0..count {
            let start = base + block as u64 * 0x28000;
            for va in (start..start + 0x28000).step_by(UAT_PGSZ) {
                if self.pages.get(&va).is_some() || self.low.translate(va)?.is_some() {
                    return Err(EEXIST);
                }
            }
            for va in (start..start + 0x20000).step_by(UAT_PGSZ) {
                let page = match Page::alloc_page(gfp | __GFP_ZERO) {
                    Ok(page) => page,
                    Err(_) => return Ok(None),
                };
                let node = match RBTreeNode::new(va, page, gfp) {
                    Ok(node) => node,
                    Err(_) => return Ok(None),
                };
                pages.push_within_capacity(node).map_err(|_| EIO)?;
            }
        }
        // Transfer every preallocated node before the first fallible PTE edit.
        self.render_arena = arena;
        for node in pages {
            self.pages.insert(node);
        }
        for block in 0..count {
            let start = base + block as u64 * 0x28000;
            for va in (start..start + 0x20000).step_by(UAT_PGSZ) {
                self.low.map_pages(
                    va..va + UAT_PGSZ as u64,
                    self.pages.get(&va).ok_or(EIO)?.phys(),
                    prot::PROT_GPU_SHARED_RW,
                    false,
                )?;
            }
        }
        Ok(Some(base))
    }

    pub(crate) fn write_low(&mut self, mut va: u64, mut bytes: &[u8]) -> Result {
        let _t = crate::g16_timeline::Timer::start(&crate::g16_timeline::P_WRITE_LOW);
        while !bytes.is_empty() {
            let base = va & !(UAT_PGSZ as u64 - 1);
            let page = self.pages.get(&base).ok_or(EINVAL)?;
            let offset = (va - base) as usize;
            let size = bytes.len().min(UAT_PGSZ - offset);
            page.with_pointer_into_page(offset, size, |p| {
                // SAFETY: This range belongs to an unpublished client allocation.
                unsafe {
                    core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, size);
                }
                Ok(())
            })?;
            va = va.checked_add(size as u64).ok_or(EINVAL)?;
            bytes = &bytes[size..];
        }
        Ok(())
    }

    /// Publish page-table edits. Owned client pages are cleaned when initialized;
    /// mapping changes do not need to sweep all existing client allocations.
    /// Firmware-private storage is managed separately by FirmwareSpace.
    pub(crate) fn sync(&self) {
        let _t = crate::g16_timeline::Timer::start(&crate::g16_timeline::P_VM_SYNC);
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
}

/// The firmware uses this bootloader root independently of all client slots.
/// Its existing tables remain owned by the firmware reservation.
pub(crate) struct FirmwareSpace {
    table: UatPageTable,
    regions: RBTree<u64, FirmwareRegion>,
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
            regions: RBTree::new(),
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
        self.table.publish();
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
        if crate::module_parameters::fw_trace.value() & 128 != 0 {
            pr_info!("G16: fwmap {:#x}+{:#x} {:?}\n", va, size, prot);
        }
        let end = va.checked_add(size as u64).ok_or(EINVAL)?;
        // Failed mapping cleanup may retain an owned but partly unmapped
        // region. Its address range must not be replaced or overlapped.
        let previous = match self.regions.cursor_lower_bound(&va) {
            Some(cursor) => {
                if *cursor.current().0 < end {
                    return Err(EEXIST);
                }
                cursor.peek_prev().map(|(base, _)| *base)
            }
            None => self.regions.cursor_back().map(|cursor| *cursor.current().0),
        };
        if previous.is_some_and(|base| {
            let region = self.regions.get(&base).unwrap();
            base + (region.pages.len() * UAT_PGSZ) as u64 > va
        }) {
            return Err(EEXIST);
        }
        for v in (va..end).step_by(UAT_PGSZ) {
            if self.table.translate(v)?.is_some() {
                return Err(EEXIST);
            }
        }
        let mut pages = KVec::new();
        for _ in 0..size / UAT_PGSZ {
            pages.push(Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?, GFP_KERNEL)?;
        }
        let node = RBTreeNode::new(va, FirmwareRegion { va, pages }, GFP_KERNEL)?;
        self.regions.insert(node);
        let region = self.regions.get(&va).ok_or(EIO)?;
        for (i, page) in region.pages.iter().enumerate() {
            let v = va + (i * UAT_PGSZ) as u64;
            if let Err(e) = self
                .table
                .map_pages(v..v + UAT_PGSZ as u64, page.phys(), prot, false)
            {
                if v != va {
                    self.table.unmap_pages(va..v)?;
                }
                self.table.publish();
                self.regions.remove(&va);
                return Err(e);
            }
        }
        Ok(())
    }

    pub(crate) fn write(&mut self, va: u64, bytes: &[u8]) -> Result {
        let region = self.region(va, bytes.len())?;
        let mut offset = (va - region.va) as usize;
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let within = offset & (UAT_PGSZ - 1);
            let size = remaining.len().min(UAT_PGSZ - within);
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
        let region = self.region(va, UAT_PGSZ)?;
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
            prot::PROT_FW_PRIV_RW,
        )?;
        // Each lease's render shared state, which the host advances while
        // firmware runs, stays uncached apart from the cached support page.
        self.alloc(
            crate::g16_render::SUPPORT_SHARED_BASE,
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

    /// Update host-owned bytes in memory firmware may be reading.
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
        self.update_batch(1, |_| (va, size), |_, ptr, offset, size| update(ptr, offset, size))
    }

    /// Several host-owned writes with one barrier pair, the region resolved
    /// again only when a destination leaves the previous one. Nothing in a
    /// batch is visible to firmware before the queue publication that follows
    /// it (flush), which has its own barrier.
    /// `write(n)` is the destination and source of write `n` of `count`.
    pub(crate) fn write_batch<'a>(&mut self, count: usize, write: impl Fn(usize) -> (u64, &'a [u8])) -> Result {
        let range = |n| {
            let (va, bytes) = write(n);
            (va, bytes.len())
        };
        self.update_batch(count, range, |which, ptr, offset, size| {
            let bytes = write(which).1;
            // SAFETY: update_batch supplies an exclusively host-owned
            // destination and a range within this write's source slice.
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr().add(offset), ptr, size) };
        })
    }

    /// Whether `region` holds all of `va..va + size`.
    fn holds(region: &FirmwareRegion, va: u64, size: usize) -> bool {
        va >= region.va
            && va
                .checked_add(size as u64)
                .is_some_and(|end| end <= region.va + (region.pages.len() * UAT_PGSZ) as u64)
    }

    /// Update host-owned bytes in memory firmware may be reading, between one
    /// barrier pair. G16 firmware memory is coherent, as writeback user BOs
    /// are, so no CPU cache maintenance is needed.
    /// `range(n)` is the destination of write `n` of `count`.
    fn update_batch(
        &mut self,
        count: usize,
        range: impl Fn(usize) -> (u64, usize),
        mut update: impl FnMut(usize, *mut u8, usize, usize),
    ) -> Result {
        let _t = crate::g16_timeline::Timer::start(&crate::g16_timeline::P_LIVE);
        // Validate every destination before touching any of them, resolving
        // each distinct region once: a render's writes alternate between a
        // few regions (notifier, Work slot, lane rings).
        // Fixed-size bookkeeping: no allocation per write. Destinations past
        // these bounds are looked up individually (NONE).
        const NONE: u8 = u8::MAX;
        let mut found: [Option<&FirmwareRegion>; 4] = [None; 4];
        let mut owner = [NONE; 32];
        for n in 0..count {
            let (va, size) = range(n);
            let region = match found.iter().flatten().position(|r| Self::holds(r, va, size)) {
                Some(index) => {
                    if let Some(slot) = owner.get_mut(n) {
                        *slot = index as u8;
                    }
                    continue;
                }
                None => self.region(va, size)?,
            };
            if let (Some(slot), Some(free)) = (owner.get_mut(n), found.iter().position(Option::is_none)) {
                found[free] = Some(region);
                *slot = free as u8;
            }
        }
        let resolve = |n: usize, va: u64, size: usize| -> Result<&FirmwareRegion> {
            match owner.get(n).and_then(|&i| found.get(i as usize).copied().flatten()) {
                Some(region) => Ok(region),
                None => self.region(va, size),
            }
        };
        crate::mem::sync();
        for which in 0..count {
            let (va, size) = range(which);
            if size == 0 {
                continue;
            }
            let region = resolve(which, va, size)?;
            let start = (va - region.va) as usize;
            let end = start + size;
            for index in start / UAT_PGSZ..=(end - 1) / UAT_PGSZ {
                let base = index * UAT_PGSZ;
                let within = start.saturating_sub(base);
                let limit = (end - base).min(UAT_PGSZ);
                region.pages[index].with_page_mapped(|p| {
                    // SAFETY: The checked range lies in this mapped page, and
                    // the caller owns these bytes.
                    unsafe { update(which, p.add(within), base + within - start, limit - within) };
                });
            }
        }
        crate::mem::sync();
        Ok(())
    }

    /// Read one firmware-owned 64-bit status value as two 32-bit reads.
    /// Host writes to these words must have been published first.
    pub(crate) fn read_u64(&self, va: u64) -> Result<u64> {
        Ok(u64::from(self.read_u32(va)?) | (u64::from(self.read_u32(va + 4)?) << 32))
    }

    /// Debug (debugfs `fw_mem`): the page-aligned VA and physical address of
    /// each page of the region at `va`, which must be one never released.
    pub(crate) fn region_pages(&self, va: u64) -> Result<KVec<(u64, kernel::addr::PhysicalAddr)>> {
        let region = self.regions.get(&va).ok_or(EINVAL)?;
        let mut pages = KVec::with_capacity(region.pages.len(), GFP_KERNEL)?;
        for (i, page) in region.pages.iter().enumerate() {
            pages.push((va + (i * UAT_PGSZ) as u64, page.phys()), GFP_KERNEL)?;
        }
        Ok(pages)
    }

    /// Called only after the context is drained and its hardware roots detached.
    pub(crate) fn release_region(&mut self, va: u64) -> Result {
        let size = self.regions.get(&va).ok_or(EINVAL)?.pages.len() * UAT_PGSZ;
        self.table.unmap_pages(va..va + size as u64)?;
        self.table.publish();
        self.regions.remove(&va);
        Ok(())
    }

    pub(crate) fn read_u32(&self, va: u64) -> Result<u32> {
        let _t = crate::g16_timeline::Timer::start(&crate::g16_timeline::P_READ);
        if va & 3 != 0 {
            return Err(EINVAL);
        }
        let region = self.region(va, 4)?;
        let offset = (va - region.va) as usize;
        region.pages[offset / UAT_PGSZ].with_pointer_into_page(offset & (UAT_PGSZ - 1), 4, |p| {
            // SAFETY: The aligned word lies in this live owned page, mapped
            // coherently with firmware; the barrier orders the volatile read
            // after the caller's publication, and nothing here cleans caches.
            unsafe {
                core::arch::asm!("dsb sy");
                Ok(u32::from_le(p.cast::<u32>().read_volatile()))
            }
        })
    }

    pub(crate) fn sync(&self) {
        let _t = crate::g16_timeline::Timer::start(&crate::g16_timeline::P_FW_SYNC);
        self.table.publish();
    }

    /// Find the preceding allocation in logarithmic time, then validate the
    /// entire access. Adjacent regions must not hide an out-of-bounds request.
    fn region(&self, va: u64, size: usize) -> Result<&FirmwareRegion> {
        let end = va.checked_add(size as u64).ok_or(EINVAL)?;
        let base = match self.regions.cursor_lower_bound(&va) {
            Some(cursor) if *cursor.current().0 == va => va,
            Some(cursor) => *cursor.peek_prev().ok_or(EINVAL)?.0,
            None => *self.regions.cursor_back().ok_or(EINVAL)?.current().0,
        };
        let region = self.regions.get(&base).ok_or(EINVAL)?;
        if end > base + (region.pages.len() * UAT_PGSZ) as u64 {
            return Err(EINVAL);
        }
        Ok(region)
    }
}

impl Drop for FirmwareSpace {
    fn drop(&mut self) {
        // Only valid before publishing the graph or after firmware is stopped.
        // Remove leaf mappings before their backing pages are released.
        for region in self.regions.values() {
            let _ = self
                .table
                .unmap_pages(region.va..region.va + (region.pages.len() * UAT_PGSZ) as u64);
        }
        for range in &self.external {
            let _ = self.table.unmap_pages(range.clone());
        }
        self.table.publish();
    }
}
