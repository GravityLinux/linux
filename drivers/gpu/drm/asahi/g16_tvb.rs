// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! Context-owned TVB lists and retained growth backing, following g16g_tvb.py.
use crate::{
    g16_fw, g16_render as render,
    g16_vm::{AddressSpace, FirmwareSpace},
    pgtable::prot,
};
use kernel::prelude::*;

pub(crate) const GROW_BLOCKS: u32 = 10;
pub(crate) const MAX_BLOCKS: u32 = 11 + 32 * GROW_BLOCKS;

pub(crate) struct Tvb {
    pub(crate) root: u64,
    pub(crate) addresses: render::Addresses,
    pub(crate) initialized: bool,
    pub(crate) blocks: u32,
    pub(crate) counter: u32,
    pub(crate) refused: bool,
    scenes: u64,
    next: u64,
}
impl Tvb {
    pub(crate) fn new(
        fw: &mut FirmwareSpace,
        client: &mut AddressSpace,
        base: u64,
        slot: u64,
        growth_base: u64,
    ) -> Result<Self> {
        let mut a = render::Addresses::bootstrap();
        a.buffer_manager_slot = slot;
        a.buffer_manager = base;
        a.buffer_thing = base + 0x4080;
        a.buffer_manager_block_control = base + 0x8000;
        a.buffer_manager_scene_list = base + 0xc000;
        a.buffer_manager_counter = base + 0x10000;
        a.buffer_manager_block_list = base + 0x14000;
        a.buffer_manager_page_list = base + 0x24000;
        fw.alloc(base, 0x40000, prot::PROT_FW_PRIV_RW)?;
        for va in (base..base + 0x40000).step_by(0x4000) {
            fw.init_page(va, |bytes| a.private_page(va, bytes).ok_or(EINVAL))?;
        }
        // Firmware and the accelerator must mutate the same page-list backing.
        // The retired packed initialization pages remain owned by this context.
        for offset in (0..0x1c000u64).step_by(0x4000) {
            let va = 0x1000258000 + offset;
            let pa = fw.physical(a.buffer_manager_page_list + offset)?;
            client.low.unmap_pages(va..va + 0x4000)?;
            client
                .low
                .map_pages(va..va + 0x4000, pa, prot::PROT_GPU_SHARED_RW, false)?;
        }
        fw.write_live(
            0xfffffc2000128000 + slot * 16,
            &g16_fw::parameter_buffer_descriptor(0x1000258000, 44).ok_or(EINVAL)?,
        )?;
        fw.sync();
        client.sync();
        Ok(Self {
            root: client.roots().low,
            addresses: a,
            initialized: false,
            blocks: 11,
            counter: 0,
            refused: false,
            scenes: 0,
            next: growth_base,
        })
    }

    pub(crate) fn reserve_scene(&mut self) -> Result<u32> {
        // The firmware's accelerator scene records have 36 slots. Lease one
        // until both stages and their event replies have retired.
        let index = (!self.scenes).trailing_zeros();
        if index >= 36 {
            return Err(EBUSY);
        }
        self.scenes |= 1 << index;
        Ok(index)
    }

    pub(crate) fn release_scene(&mut self, index: u32) {
        self.scenes &= !(1 << index);
    }

    pub(crate) fn work_addresses(&self, a: &mut render::Addresses, scene: u32) {
        let p = &self.addresses;
        a.buffer_manager_slot = p.buffer_manager_slot;
        a.buffer_manager = p.buffer_manager;
        a.buffer_manager_block_control = p.buffer_manager_block_control;
        a.buffer_manager_scene_list = p.buffer_manager_scene_list;
        a.buffer_manager_counter = p.buffer_manager_counter;
        a.buffer_manager_block_list = p.buffer_manager_block_list;
        a.buffer_manager_page_list = p.buffer_manager_page_list;
        a.buffer_thing = (p.buffer_thing & !0x3fff) + u64::from(scene) * 0x80;
    }

    pub(crate) fn grow(
        &mut self,
        fw: &mut FirmwareSpace,
        client: &mut AddressSpace,
        asid: u8,
    ) -> Result<bool> {
        let a = &self.addresses;
        let old = self.blocks;
        if client.roots().low != self.root
            // Another queued request can arrive before firmware consumes the
            // previous growth reply. Its mirror may lag our published lists.
            || fw.read_u32(a.buffer_manager + 0x3c)? > old
            || fw.read_u32(a.buffer_manager_block_control)? != old
            || fw.read_u32(a.buffer_manager_block_control + 4)? != old
            || fw.read_u64(a.buffer_manager + 0x44)? != a.buffer_manager_block_list
            || fw.read_u64(a.buffer_manager + 0x4c)? != a.buffer_manager_block_control
            || fw.read_u32(a.buffer_manager + 0x58)? != 0x20000
        {
            return Err(EIO);
        }
        let limit = (*crate::module_parameters::tvb_max_blocks.value()).min(MAX_BLOCKS);
        let new = (old + GROW_BLOCKS).min(limit).max(old);
        if new > fw.read_u32(a.buffer_manager + 0x38)? || new * 8 > 0x10000 || new * 16 > 0x1c000 {
            return Err(EIO);
        }
        if new == old {
            return Ok(false);
        }
        // Allocation is all-or-nothing before publishing any leaf or list entry.
        // After mapping starts, any error requires retaining the context/reset.
        if !client.alloc_tvb_blocks(self.next, (new - old) as usize)? {
            return Ok(false);
        }
        client.sync();
        client.low.invalidate(Some(asid));
        client.high.invalidate(Some(asid));
        crate::mem::sync();
        client.low.clear_invalidations();
        client.high.clear_invalidations();
        let mut entries = [0u8; GROW_BLOCKS as usize * 8];
        for index in 0..new - old {
            let dva = self.next + u64::from(index) * 0x28000;
            let id = (dva - render::CONTEXT_BASE) / 0x8000;
            let offset = index as usize * 8;
            entries[offset..offset + 8].copy_from_slice(&id.to_le_bytes());
        }
        fw.write_live(
            a.buffer_manager_block_list + u64::from(old) * 8,
            &entries[..(new - old) as usize * 8],
        )?;
        crate::mem::sync();
        let counts = (u64::from(new) | (u64::from(new) << 32)).to_le_bytes();
        fw.write_live(a.buffer_manager_block_control, &counts)?;
        crate::mem::sync();
        // Firmware expands its page list and counts after the type-8 reply.
        self.blocks = new;
        self.next += u64::from(new - old) * 0x28000;
        Ok(true)
    }
}
