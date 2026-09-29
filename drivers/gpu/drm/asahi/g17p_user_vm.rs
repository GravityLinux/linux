// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Owned 42-bit user UAT tree, including quiescent caller-binding updates.

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
        Self::store(page, index, word);
        Ok(())
    }
    // Only checked, owned table pages and indices reach the publication pass.
    fn store(page: &Page, index: usize, word: u64) {
        page.with_page_mapped(|p| {
            // SAFETY: Exclusive VM access and checked aligned table index.
            unsafe {p.cast::<u64>().add(index).write_volatile(word.to_le());
                core::arch::asm!("dc cvac, {p}",p=in(reg)p.add(index*8),options(nostack,preserves_flags));}
        });
        super::g17p_memory::sync();
    }
    pub(crate) fn pte(&self, va: u64) -> Result<u64> {
        if va >= 1 << 42 || va & (PAGE - 1) != 0 {
            return Err(EINVAL);
        }
        let mut table = self.root();
        for index in [((va >> 36) & 63) as usize, ((va >> 25) & 2047) as usize] {
            let entry = self.read(table, index)?;
            if entry == 0 {
                return Ok(0);
            }
            if entry & 3 != 3 {
                return Err(EINVAL);
            }
            table = entry & ADDRESS;
        }
        self.read(table, ((va >> 14) & 2047) as usize)
    }

    fn invalidate(context: u16) {
        super::g17p_memory::sync();
        // SAFETY: Called under the runtime lock with the source's quiescent
        // execution context. T8140 implements FEAT_TLBIOS.
        unsafe {
            core::arch::asm!(
                ".inst 0xd5088140", // TLBI ASIDE1OS, X0
                "dsb sy", "isb",
                in("x0") (context as u64) << 48,
                options(nostack, preserves_flags)
            );
        }
    }

    /// Replace caller leaves after completion, preserving private/grown pages.
    /// Each change names its expected old leaf (zero for an addition). Allocate
    /// and validate the complete plan before breaking any live mapping. Failed
    /// preparation leaves active tables intact, including all parent entries.
    pub(crate) fn rebind(&mut self, changes: &[(u64, u64, u64)], contexts: &[u16]) -> Result {
        if contexts.is_empty()
            || contexts.iter().enumerate().any(|(i, &context)| {
                context == 0 || context >= 64 || contexts[..i].contains(&context)
            })
        {
            return Err(EINVAL);
        }
        let mut writes: KVec<(usize, u64, usize, u64)> = KVec::new();
        for &(va, old, new) in changes {
            if va >= 1 << 42
                || va & (PAGE - 1) != 0
                || [old, new].into_iter().any(|pte| {
                    pte != 0
                        && (pte & ADDRESS == 0
                            || pte & !(ADDRESS | 0x00c0000000000fff) != 0
                            || pte & 3 != 3)
                })
                || self.pte(va)? != old
            {
                return Err(EINVAL);
            }
            let mut table = self.root();
            for (depth, index) in [((va >> 36) & 63) as usize, ((va >> 25) & 2047) as usize]
                .into_iter()
                .enumerate()
            {
                let mut entry =
                    if let Some(row) = writes.iter().find(|r| r.1 == table && r.2 == index) {
                        row.3
                    } else {
                        self.read(table, index)?
                    };
                if entry == 0 {
                    entry = self.table()? | 3;
                    writes.push((depth, table, index, entry), GFP_KERNEL)?;
                }
                if entry & 3 != 3 {
                    return Err(EINVAL);
                }
                table = entry & ADDRESS;
                self.page(table)?;
            }
            let index = ((va >> 14) & 2047) as usize;
            if writes.iter().any(|r| r.1 == table && r.2 == index) {
                return Err(EINVAL);
            }
            writes.push((2, table, index, new), GFP_KERNEL)?;
        }
        // Resolve every owned page reference before publication. The following
        // two store passes have no allocation, lookup or other fallible step.
        let mut stores = KVec::new();
        for &(depth, table, index, new) in &writes {
            let old = self.read(table, index)?;
            if old != new {
                stores.push((depth, self.page(table)?, index, old, new), GFP_KERNEL)?;
            }
        }
        for &(depth, page, index, old, _) in &stores {
            if depth == 2 && old != 0 {
                Self::store(page, index, 0);
            }
        }
        for &context in contexts {
            Self::invalidate(context);
        }
        for depth in [2, 1, 0] {
            for &(_, page, index, _, new) in stores.iter().filter(|r| r.0 == depth) {
                Self::store(page, index, new);
            }
            super::g17p_memory::sync();
        }
        for &context in contexts {
            Self::invalidate(context);
        }
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

    /// Add disjoint growth pages to a live retained root. Validate every leaf
    /// and allocate every child first; an allocation failure publishes nothing.
    /// Each new child is populated/cleaned before its parent link is visible.
    pub(crate) fn grow(&mut self, pages: &[(u64, u64)]) -> Result {
        if pages.is_empty() {
            return Err(EINVAL);
        }
        let mut writes: KVec<(usize, u64, usize, u64)> = KVec::new();
        for &(va, physical) in pages {
            if va >= 1 << 42 || va & (PAGE - 1) != 0 || physical == 0 || physical & !ADDRESS != 0 {
                return Err(EINVAL);
            }
            let mut table = self.root();
            for (depth, index) in [((va >> 36) & 63) as usize, ((va >> 25) & 2047) as usize]
                .into_iter()
                .enumerate()
            {
                let mut entry =
                    if let Some(row) = writes.iter().find(|r| r.1 == table && r.2 == index) {
                        row.3
                    } else {
                        self.read(table, index)?
                    };
                if entry == 0 {
                    entry = self.table()? | 3;
                    writes.push((depth, table, index, entry), GFP_KERNEL)?;
                }
                if entry & 3 != 3 {
                    return Err(EINVAL);
                }
                table = entry & ADDRESS;
                self.page(table)?;
            }
            let index = ((va >> 14) & 2047) as usize;
            if self.read(table, index)? != 0 || writes.iter().any(|r| r.1 == table && r.2 == index)
            {
                return Err(EBUSY);
            }
            writes.push((2, table, index, physical | 0x00c0000000000c8b), GFP_KERNEL)?;
        }
        // No allocation or user-controlled validation after this boundary.
        for depth in [2, 1, 0] {
            for &(_, table, index, entry) in writes.iter().filter(|r| r.0 == depth) {
                self.write(table, index, entry)?;
            }
            super::g17p_memory::sync();
        }
        Ok(())
    }
}
