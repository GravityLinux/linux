// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Caller timestamp BO aliases in the source adapter's firmware aperture.
//! Aliases and backing stay owned until both firmware instances stop. This
//! deliberately does not recycle addresses while firmware could retain them.

use super::{g17p_drm::Object, g17p_memory::Memory, g17p_vm::Vm};
use kernel::{drm::gem::BaseObject, prelude::*, sync::aref::ARef};

pub(crate) const BASE: u64 = 0xfffffc2181400000;
pub(crate) const SIZE: u64 = 0x4000000;
const PAGE: u64 = 0x4000;

struct Alias {
    bo: ARef<Object>,
    offset: u64,
    size: u64,
    address: u64,
}
pub(crate) struct Registry {
    next: u64,
    aliases: KVec<Alias>,
}
impl Registry {
    pub(crate) fn new() -> Self {
        Self {
            next: BASE,
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
        let address = self.next;
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
            },
            GFP_KERNEL,
        )?;
        self.next = end;
        for (index, pa) in pages.into_iter().enumerate() {
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

    pub(crate) fn cache(&self, addresses: [u64; 2], invalidate: bool) -> Result {
        super::g17p_memory::sync();
        for address in addresses {
            if address == 0 {
                continue;
            }
            let alias = self
                .aliases
                .iter()
                .find(|a| {
                    address >= a.address
                        && address
                            .checked_add(8)
                            .is_some_and(|e| e <= a.address + a.size)
                })
                .ok_or(EINVAL)?;
            let map = alias.bo.vmap::<u8>()?;
            if map.is_iomem() {
                return Err(EINVAL);
            }
            let offset = (alias.offset + address - alias.address) as usize & !63;
            let pointer = map.ptr_from_index(offset)?;
            // SAFETY: The selected complete cache line lies in pinned,
            // page-aligned GEM RAM. Invalidate only after a clean and verified
            // GPU completion. Unrelated BO lines are not discarded.
            unsafe {
                if invalidate {
                    core::arch::asm!("dc ivac, {p}", p=in(reg)pointer, options(nostack,preserves_flags));
                } else {
                    core::arch::asm!("dc cvac, {p}", p=in(reg)pointer, options(nostack,preserves_flags));
                }
            }
        }
        super::g17p_memory::sync();
        Ok(())
    }
}
