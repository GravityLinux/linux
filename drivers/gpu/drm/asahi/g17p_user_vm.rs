// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Owned, initially unpublished 42-bit user UAT tree. These pages are separate
//! from the fixed firmware/source graph. Runtime binding/invalidation is added
//! at the first submission boundary; no hardware slot points here yet.

use kernel::{
    alloc::flags::__GFP_ZERO,
    page::{self, Page},
    prelude::*,
    types::Owned,
};
const PAGE: u64 = 0x4000;
const ADDRESS: u64 = 0x000003ffffffc000;

pub(crate) struct UserVm {
    tables: KVec<Owned<Page>>,
}
impl UserVm {
    pub(crate) fn new() -> Result<Self> {
        if page::PAGE_SIZE != PAGE as usize {
            return Err(EINVAL);
        }
        let mut vm = Self {
            tables: KVec::new(),
        };
        vm.table()?;
        Ok(vm)
    }
    pub(crate) fn root(&self) -> u64 {
        self.tables[0].phys()
    }
    fn table(&mut self) -> Result<u64> {
        self.tables.reserve(1, GFP_KERNEL)?;
        let page = Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?;
        let pa = page.phys();
        if pa & !ADDRESS != 0 {
            return Err(EINVAL);
        }
        Self::clean(&page);
        self.tables.push(page, GFP_KERNEL)?;
        Ok(pa)
    }
    fn page(&self, pa: u64) -> Result<&Page> {
        self.tables
            .iter()
            .find(|p| p.phys() == pa)
            .map(|p| &**p)
            .ok_or(EINVAL)
    }
    fn clean(page: &Page) {
        page.with_page_mapped(|p| {
            for offset in (0..PAGE as usize).step_by(64) {
                // SAFETY: Every complete line lies inside this owned page.
                unsafe {core::arch::asm!("dc cvac, {p}",p=in(reg)p.add(offset),options(nostack,preserves_flags))};
            }
        });
        super::g17p_memory::sync();
    }
    fn read(&self, pa: u64, index: usize) -> Result<u64> {
        if index >= 2048 {
            return Err(EINVAL);
        }
        Ok(self.page(pa)?.with_page_mapped(|p| {
            // SAFETY: Aligned qword within the owned table, serialized by File.
            u64::from_le(unsafe { p.cast::<u64>().add(index).read_volatile() })
        }))
    }
    fn write(&mut self, pa: u64, index: usize, word: u64) -> Result {
        if index >= 2048 {
            return Err(EINVAL);
        }
        let page = self.page(pa)?;
        page.with_page_mapped(|p| {
            // SAFETY: Exclusive VM access and checked aligned table index.
            unsafe {p.cast::<u64>().add(index).write_volatile(word.to_le());
                core::arch::asm!("dc cvac, {p}",p=in(reg)p.add(index*8),options(nostack,preserves_flags));}
        });
        super::g17p_memory::sync();
        Ok(())
    }
    fn leaf(&mut self, va: u64, create: bool) -> Result<Option<(u64, usize)>> {
        if va >= 1 << 42 || va & (PAGE - 1) != 0 {
            return Err(EINVAL);
        }
        let mut pa = self.root();
        for index in [((va >> 36) & 63) as usize, ((va >> 25) & 2047) as usize] {
            let mut entry = self.read(pa, index)?;
            if entry == 0 {
                if !create {
                    return Ok(None);
                }
                let next = self.table()?;
                entry = next | 3;
                self.write(pa, index, entry)?;
            }
            if entry & 3 != 3 {
                return Err(EINVAL);
            }
            pa = entry & ADDRESS;
        }
        self.page(pa)?;
        Ok(Some((pa, ((va >> 14) & 2047) as usize)))
    }
    /// Preallocate the table paths before modifying leaves. Failed allocation
    /// leaves only empty tables; the previous bindings remain valid.
    pub(crate) fn prepare(&mut self, va: u64, size: u64) -> Result {
        if size == 0 || size & (PAGE - 1) != 0 || va.checked_add(size).ok_or(EINVAL)? > 1 << 42 {
            return Err(EINVAL);
        }
        for offset in (0..size).step_by(PAGE as usize) {
            let (table, index) = self.leaf(va + offset, true)?.ok_or(EINVAL)?;
            if self.read(table, index)? != 0 {
                return Err(EBUSY);
            }
        }
        Ok(())
    }
    pub(crate) fn map_page(&mut self, va: u64, pa: u64, writable: bool) -> Result {
        let flags = 0x0080000000000c8b | if writable { 1 << 54 } else { 0 };
        self.map_owned_page(va, pa, flags)
    }
    /// Preserve the source attributes of driver-owned private render pages.
    /// This is an internal mapping API; userspace cannot supply PTE flags.
    pub(crate) fn map_owned_page(&mut self, va: u64, pa: u64, flags: u64) -> Result {
        if pa & !ADDRESS != 0 {
            return Err(EINVAL);
        }
        if flags & !0x00c0000000000fff != 0 || flags & 3 != 3 {
            return Err(EINVAL);
        }
        let (table, index) = self.leaf(va, false)?.ok_or(EINVAL)?;
        if self.read(table, index)? != 0 {
            return Err(EBUSY);
        }
        // Current source uses Shared/AP=2/nG, with UXN for writable resources.
        let pte = pa | flags;
        self.write(table, index, pte)?;
        if self.read(table, index)? != pte {
            return Err(EIO);
        }
        Ok(())
    }
    pub(crate) fn unmap(&mut self, va: u64, size: u64) -> Result {
        if size == 0 || size & (PAGE - 1) != 0 || va.checked_add(size).ok_or(EINVAL)? > 1 << 42 {
            return Err(EINVAL);
        }
        for offset in (0..size).step_by(PAGE as usize) {
            if let Some((table, index)) = self.leaf(va + offset, false)? {
                self.write(table, index, 0)?;
            }
        }
        Ok(())
    }
}
