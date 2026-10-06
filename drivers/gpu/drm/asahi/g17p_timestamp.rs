// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Caller timestamp BO aliases in the source adapter's firmware aperture.
//! Bound tokens retain their backing and per-command fences. Only an unbound
//! token whose work retired successfully can release its alias for reuse.

use super::{
    g17p_drm::Object,
    g17p_memory::Memory,
    g17p_vm::{Vm, TIMESTAMP_BASE as BASE, TIMESTAMP_SIZE as SIZE},
};
use kernel::{
    bindings,
    dma_fence::{Fence, RawDmaFence},
    drm::gem::BaseObject,
    prelude::*,
    sync::aref::ARef,
};

const PAGE: u64 = 0x4000;

struct Alias {
    bo: ARef<Object>,
    offset: u64,
    size: u64,
    address: u64,
    pages: KVec<u64>,
    fences: KVec<Fence>,
    unbound: bool,
}
/// Exact BO/cache-line ownership, detached from mutable alias registration.
/// The accepted command fence keeps the firmware alias leased until this
/// CPU visibility work finishes, including logical unbind and GEM close.
pub(crate) struct Cache {
    lines: KVec<(ARef<Object>, usize)>,
}
impl Cache {
    pub(crate) fn run(&self, invalidate: bool) -> Result {
        super::g17p_memory::sync();
        for (bo, offset) in &self.lines {
            let map = bo.vmap::<u8>()?;
            if map.is_iomem() { return Err(EINVAL); }
            let pointer = map.ptr_from_index(*offset)?;
            // SAFETY: The pinned GEM owns this checked complete cache line.
            // Invalidation occurs only after this command's verified GPU
            // retirement; no unsubmitted or unrelated BO line is discarded.
            unsafe {
                if invalidate {
                    core::arch::asm!("dc ivac, {p}", p=in(reg)pointer, options(nostack,preserves_flags));
                } else {
                    core::arch::asm!("dc civac, {p}", p=in(reg)pointer, options(nostack,preserves_flags));
                }
            }
        }
        super::g17p_memory::sync();
        Ok(())
    }
}

pub(crate) struct Registry {
    aliases: KVec<Alias>,
}
impl Registry {
    pub(crate) fn new() -> Self {
        Self {
            aliases: KVec::new(),
        }
    }

    pub(crate) fn bind(
        &mut self,
        memory: &mut Memory,
        vm: &mut Vm,
        bo: ARef<Object>,
        offset: u64,
        size: u64,
    ) -> Result<u64> {
        if size == 0
            || (offset | size) & (PAGE - 1) != 0
            || offset.checked_add(size).ok_or(EINVAL)? > bo.size() as u64
        {
            return Err(EINVAL);
        }
        self.reap(memory, vm)?;
        // Source _timestamp_address: first fit over all retained allocations,
        // including unbound tokens with pending or failed work.
        let mut address = BASE;
        loop {
            let next = self
                .aliases
                .iter()
                .filter(|a| a.address >= address)
                .min_by_key(|a| a.address);
            match next {
                Some(alias) if address.checked_add(size).ok_or(EOVERFLOW)? > alias.address => {
                    address = alias.address.checked_add(alias.size).ok_or(EOVERFLOW)?;
                }
                _ => break,
            }
        }
        let end = address.checked_add(size).ok_or(EOVERFLOW)?;
        if end > BASE + SIZE {
            return Err(ENOSPC);
        }
        let mut pages = KVec::with_capacity((size / PAGE) as usize, GFP_KERNEL)?;
        let mut cursor = 0;
        for entry in bo.sg_table()?.iter() {
            let pa = entry.dma_address();
            let length = entry.dma_len() as u64;
            if (pa | length) & (PAGE - 1) != 0 {
                return Err(EINVAL);
            }
            for index in (0..length).step_by(PAGE as usize) {
                if cursor >= offset && cursor < offset + size {
                    pages.push(pa + index, GFP_KERNEL)?;
                }
                cursor += PAGE;
            }
        }
        if pages.len() as u64 != size / PAGE {
            return Err(EIO);
        }
        // Retain ownership before the first page-table store, including a
        // partial mapping failure. Failed aliases are never returned/reused.
        self.aliases.push(
            Alias {
                bo,
                offset,
                size,
                address,
                pages,
                fences: KVec::new(),
                unbound: false,
            },
            GFP_KERNEL,
        )?;
        for (index, &pa) in self.aliases.last().ok_or(EIO)?.pages.iter().enumerate() {
            vm.timestamp_page(memory, address + index as u64 * PAGE, pa)?;
        }
        vm.flush_tables(memory)?;
        // SAFETY: Match map_firmware_existing_at's invalidation after adding
        // owned aliases. T8140 supports FEAT_TLBIOS. Runtime mutex is held.
        unsafe {
            core::arch::asm!(
                ".inst 0xd508811f",
                "dsb sy",
                "isb",
                options(nostack, preserves_flags)
            );
        }
        Ok(address)
    }

    fn reap(&mut self, memory: &Memory, vm: &Vm) -> Result {
        let mut index = 0;
        while index < self.aliases.len() {
            let alias = &mut self.aliases[index];
            // SAFETY: The token owns each fence reference. An error signal
            // does not prove that firmware released this timestamp backing.
            alias
                .fences
                .retain(|f| unsafe { bindings::dma_fence_get_status(f.raw()) } <= 0);
            if alias.unbound && alias.fences.is_empty() {
                vm.unmap_timestamp(memory, alias.address, &alias.pages)?;
                self.aliases.swap_remove(index);
            } else {
                index += 1;
            }
        }
        Ok(())
    }

    pub(crate) fn unbind(&mut self, memory: &Memory, vm: &Vm, address: u64, lost: bool) -> Result {
        self.aliases
            .iter_mut()
            .find(|a| a.address == address)
            .ok_or(ENOENT)?
            .unbound = true;
        if !lost {
            self.reap(memory, vm)?;
        }
        Ok(())
    }

    /// Acquire the alias lease while the frontend still owns its bound token.
    pub(crate) fn admit(&mut self, addresses: &[u64], fence: &Fence) -> Result {
        self.retain_checked(addresses, fence, true)
    }

    /// Accepted work may register after logical unbind. Its admission fence
    /// prevents this alias from being removed or recycled in the meantime.
    pub(crate) fn retain(&mut self, addresses: &[u64], fence: &Fence) -> Result {
        self.retain_checked(addresses, fence, false)
    }

    fn retain_checked(&mut self, addresses: &[u64], fence: &Fence, bound: bool) -> Result {
        let mut indices = KVec::new();
        for &address in addresses {
            if address == 0 {
                continue;
            }
            let index = self
                .aliases
                .iter()
                .position(|a| {
                    (!bound || !a.unbound)
                        && address >= a.address
                        && address
                            .checked_add(8)
                            .is_some_and(|end| end <= a.address + a.size)
                })
                .ok_or(EINVAL)?;
            if !indices.contains(&index) {
                indices.push(index, GFP_KERNEL)?;
            }
        }
        for &index in &indices {
            self.aliases[index].fences.reserve(1, GFP_KERNEL)?;
        }
        for index in indices {
            if !self.aliases[index].fences.iter().any(|f| f.raw() == fence.raw()) {
                self.aliases[index].fences.push(fence.clone(), GFP_KERNEL)?;
            }
        }
        Ok(())
    }

    pub(crate) fn fail_pending(&mut self, error: Error) {
        for alias in &self.aliases {
            for fence in &alias.fences {
                // SAFETY: The alias owns this live reference throughout.
                if unsafe { bindings::dma_fence_get_status(fence.raw()) } == 0 {
                    fence.set_error(error);
                    fence.signal();
                }
            }
        }
    }

    pub(crate) fn cache_seed(&self, addresses: &[u64]) -> Result<Cache> {
        let mut lines = KVec::with_capacity(addresses.len(), GFP_KERNEL)?;
        for &address in addresses {
            if address == 0 { continue; }
            let alias = self.aliases.iter().find(|a| address >= a.address
                && address.checked_add(8).is_some_and(|e| e <= a.address + a.size))
                .ok_or(EINVAL)?;
            let offset = (alias.offset + address - alias.address) as usize & !63;
            if !lines.iter().any(|(bo, at): &(ARef<Object>, usize)|
                core::ptr::eq(&**bo, &*alias.bo) && *at == offset) {
                lines.push((alias.bo.clone(), offset), GFP_KERNEL)?;
            }
        }
        Ok(Cache { lines })
    }
    pub(crate) fn cache(&self, addresses: [u64; 2], invalidate: bool) -> Result {
        self.cache_seed(&addresses)?.run(invalidate)
    }
}
