// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from Niklas Sheth's linux-m4-integration prototype.

//! Synchronous G16 graphics publication and retirement.

use super::Bootstrap;
use crate::{g16_compute as compute, g16_fw, g16_render as render, g16_vm};
use core::sync::atomic::{AtomicU64, Ordering};
use kernel::time::{delay::fsleep, Delta};
use kernel::{c_str, io, prelude::*, sync::Arc};

/// A per-VM firmware support object. The submit mutex and file-state lock keep
/// this lease live until all GPU accesses from the VM have completed.
pub(crate) struct Support {
    slots: Arc<AtomicU64>,
    index: u32,
    growth_base: u64,
    submissions: AtomicU64,
}
impl Support {
    pub(crate) fn address(&self) -> u64 {
        render::SUPPORT_BASE + u64::from(self.index) * 0x4000
    }
}
impl Drop for Support {
    fn drop(&mut self) {
        self.slots.fetch_and(!(1 << self.index), Ordering::AcqRel);
    }
}

impl Bootstrap {
    pub(crate) fn reserve_timestamp(&mut self, size: u64) -> Result<u64> {
        if size == 0 || size & 0x3fff != 0 {
            return Err(EINVAL);
        }
        // Match the shim's firmware-shared special-object aperture.
        let mut base = 0xfffffc2181400000u64;
        for range in &self.timestamp_ranges {
            if base.checked_add(size).ok_or(ENOSPC)? <= range.start {
                break;
            }
            base = base.max(range.end);
        }
        let end = base.checked_add(size).ok_or(ENOSPC)?;
        if end > 0xfffffc2185400000 {
            return Err(ENOSPC);
        }
        self.timestamp_ranges.push(base..end, GFP_KERNEL)?;
        self.timestamp_ranges.sort_unstable_by_key(|r| r.start);
        Ok(base)
    }

    pub(crate) fn map_timestamp(&mut self, address: u64, physical: u64, size: usize) -> Result {
        self._firmware_space.as_mut().ok_or(EIO)?.map_external(
            address,
            physical,
            size,
            crate::pgtable::prot::PROT_FW_SHARED_RW,
        )?;
        self._firmware_space.as_ref().ok_or(EIO)?.sync();
        Ok(())
    }
    pub(crate) fn release_timestamp(&mut self, range: core::ops::Range<u64>) -> Result {
        self._firmware_space
            .as_mut()
            .ok_or(EIO)?
            .unmap_external(range.clone())?;
        self.timestamp_ranges.retain(|r| *r != range);
        Ok(())
    }

    pub(crate) fn render_failed(&self) -> bool {
        self.render_failed
    }

    pub(crate) fn acquire_render_support(&mut self, pool: &render::OperandPool) -> Result<Support> {
        if self.render_failed {
            return Err(EIO);
        }
        let mut used = self.render_support.load(Ordering::Acquire);
        let index = loop {
            let index = (!used).trailing_zeros();
            if index >= 57 {
                return Err(ENOSPC);
            }
            match self.render_support.compare_exchange_weak(
                used,
                used | (1 << index),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break index,
                Err(value) => used = value,
            }
        };
        let lease = Support {
            slots: self.render_support.clone(),
            index,
            growth_base: pool.growth_base().ok_or(EINVAL)?,
            submissions: AtomicU64::new(0),
        };
        let mut a = render::Addresses::bootstrap();
        a.support = lease.address();
        a.render_shared_state = lease.address() + 0x1000;
        let mut page = KVec::new();
        page.extend_with(0x4000, 0u8, GFP_KERNEL)?;
        page[..0x100].copy_from_slice(&pool.support(3 + u64::from(index), a.render_shared_state));
        self._firmware_space
            .as_mut()
            .ok_or(EIO)?
            .write_live(a.support, &page)?;
        Ok(lease)
    }

    fn render_context(&mut self, space: Option<g16_vm::Roots>) -> Result {
        let dev = self.dev.as_ref();
        let res = dev
            .of_node()
            .ok_or(EINVAL)?
            .reserved_mem_region_to_resource_byname(c_str!("ttbs"))?;
        // SAFETY: The validated loader-owned table has 64 two-root slots.
        let table = unsafe { io::mem::Mem::try_new(res, io::mem::MemFlag::WB.into())? };
        let next_root = space.map(|s| s.low).unwrap_or(0);
        // Preserve the previous ASID while removing its valid roots, matching
        // the working SPTM UAT transition before the old backing is released.
        unsafe {
            let ptr = table.ptr().add(9 * 16);
            if self.render_root != 0 && self.render_root != next_root {
                ptr.cast::<u64>()
                    .write_volatile(ptr.cast::<u64>().read_volatile() & !1);
                ptr.add(8)
                    .cast::<u64>()
                    .write_volatile(ptr.add(8).cast::<u64>().read_volatile() & !1);
                core::arch::asm!("dc cvac, {addr}", "dsb oshst", addr = in(reg) ptr);
                crate::mem::tlbi_asid(9);
                core::arch::asm!("dsb osh", "isb");
            }
            if let Some(space) = space {
                ptr.cast::<u64>().write_volatile(space.low | (9 << 48) | 1);
                ptr.add(8)
                    .cast::<u64>()
                    .write_volatile(space.high | (9 << 48) | 1);
                core::arch::asm!("dc cvac, {addr}", addr = in(reg) ptr);
            }
        }
        crate::mem::sync();
        crate::mem::tlbi_all();
        crate::mem::sync();
        if self.render_root != next_root {
            self.render_queue_root = 0;
        }
        self.render_root = space.map(|s| s.low).unwrap_or(0);
        Ok(())
    }

    fn compute_context(&mut self, roots: Option<g16_vm::Roots>) -> Result {
        let next = roots.map(|s| s.low).unwrap_or(0);
        if self.compute_queue_root == next {
            return Ok(());
        }
        let dev = self.dev.as_ref();
        let res = dev
            .of_node()
            .ok_or(EINVAL)?
            .reserved_mem_region_to_resource_byname(c_str!("ttbs"))?;
        // SAFETY: Slot one is exclusively managed under the runtime mutex.
        let table = unsafe { io::mem::Mem::try_new(res, io::mem::MemFlag::WB.into())? };
        unsafe {
            let ptr = table.ptr().add(16);
            ptr.cast::<u64>()
                .write_volatile(ptr.cast::<u64>().read_volatile() & !1);
            ptr.add(8)
                .cast::<u64>()
                .write_volatile(ptr.add(8).cast::<u64>().read_volatile() & !1);
            core::arch::asm!("dc cvac, {addr}", "dsb oshst", addr=in(reg) ptr);
            crate::mem::tlbi_asid(1);
            core::arch::asm!("dsb osh", "isb");
            if let Some(roots) = roots {
                ptr.cast::<u64>().write_volatile(roots.low | (1 << 48) | 1);
                ptr.add(8)
                    .cast::<u64>()
                    .write_volatile(roots.high | (1 << 48) | 1);
                core::arch::asm!("dc cvac, {addr}", addr=in(reg) ptr);
            }
        }
        crate::mem::sync();
        crate::mem::tlbi_all();
        crate::mem::sync();
        self.compute_queue_root = next;
        Ok(())
    }

    pub(crate) fn release_render_vm(&mut self, space: g16_vm::Roots) -> Result {
        if self.render_failed {
            return Err(EIO);
        }
        if self.compute_queue_root == space.low {
            self.compute_context(None)?;
        }
        if self.render_root == space.low {
            self.render_context(None)?;
        }
        if let Some(index) = self.tvb_pools.iter().position(|p| p.root == space.low) {
            let a = self.tvb_pools[index].addresses;
            let fw = self._firmware_space.as_mut().ok_or(EIO)?;
            fw.write_live(0xfffffc2000128000 + a.buffer_manager_slot * 16, &[0; 16])?;
            fw.release_region(a.buffer_manager)?;
            self.tvb_pools.swap_remove(index);
        }
        Ok(())
    }

    fn drain_render_rx(&mut self) -> Result {
        self.drain_render_rx_inner(true)
    }

    fn drain_render_rx_inner(&mut self, events: bool) -> Result {
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        for (state, capacity) in [
            (0xffff_fc20_0003_f1c0, 256),
            (0xffff_fc20_0003_f3c0, 512),
            (0xffff_fc20_0003_f400, 256),
            (0xffff_fc20_0003_f200, 256),
            (0xffff_fc20_0003_f230, 256),
            (0xffff_fc20_0003_f260, 256),
            (0xffff_fc20_0003_f290, 256),
            (0xffff_fc20_0003_f2c0, 256),
            (0xffff_fc20_0003_f2f0, 256),
        ] {
            if !events && state == 0xfffffc200003f1c0 {
                continue;
            }
            let read = fw.read_u32(state)?;
            let write = fw.read_u32(state + 0x20)?;
            if read >= capacity || write >= capacity {
                return Err(EIO);
            }
            if read != write {
                if state == 0xffff_fc20_0003_f1c0 {
                    let mut cursor = read;
                    while cursor != write {
                        let base = 0xffff_fc20_0003_f440 + u64::from(cursor) * 0x48;
                        let kind = fw.read_u32(base)?;
                        if kind == 13
                            && self.opening_notification_pending
                            && fw.read_u32(base + 4)? == 1
                            && fw.read_u32(base + 8)? == 0
                            && fw.read_u64(base + 12)? == g16_fw::CONTROL_DATA
                            && fw.read_u64(base + 20)? == g16_fw::OPERAND_TABLE
                            && fw.read_u64(base + 28)? == g16_fw::OPERAND_TABLE
                            && fw.read_u32(base + 36)? == 20 * 8
                        {
                            // Completion of our boot-time opcode-0x20 admission.
                            // Only the authored prefix is meaningful; the tail
                            // is opaque firmware stack storage.
                            self.opening_notification_pending = false;
                        } else if kind != 1 {
                            pr_err!(
                                "G16: unexpected firmware event {} at render {}\n",
                                kind,
                                self.render_publication
                            );
                            return Err(EIO);
                        }
                        cursor = (cursor + 1) % capacity;
                    }
                }
                if state == 0xffff_fc20_0003_f3c0 {
                    let mut cursor = read;
                    while cursor != write {
                        let base = 0xffff_fc20_000e_5c40 + u64::from(cursor) * 0x48;
                        if fw.read_u32(base)? == 5
                            && fw.read_u32(base + 0x2c)? & 0xffff == 4
                            && fw.read_u64(base + 0x30)? == 0
                            && fw.read_u64(base + 0x14)? != 0
                        {
                            pr_info!(
                                "G16: firmware Start3D partial render queue {:#x}\n",
                                fw.read_u64(base + 0x24)?
                            );
                        }
                        cursor = (cursor + 1) % capacity;
                    }
                }
                fw.write_live(state, &write.to_le_bytes())?;
            }
        }
        Ok(())
    }

    /// The caller retains all client mappings throughout this call. On failure
    /// after publication it must quarantine those mappings until reset.
    pub(crate) fn submit_render(
        &mut self,
        client: &mut g16_vm::AddressSpace,
        params: &render::Parameters,
        support: &Support,
        priority: u32,
        timestamps: &[u64; 4],
    ) -> Result<[u64; 4]> {
        let root = client.roots().low;
        let index = match self.tvb_pools.iter().position(|p| p.root == root) {
            Some(index) => index,
            None => {
                self.tvb_pools.reserve(1, GFP_KERNEL)?;
                let base = self.next_work_va;
                self.next_work_va = base.checked_add(0x44000).ok_or(ENOSPC)?;
                let pool = crate::g16_tvb::Tvb::new(
                    self._firmware_space.as_mut().ok_or(EIO)?,
                    client,
                    base,
                    7 + u64::from(support.index),
                    support.growth_base,
                )?;
                self.tvb_pools.push(pool, GFP_KERNEL)?;
                self.tvb_pools.len() - 1
            }
        };
        let mut pool = self.tvb_pools.swap_remove(index);
        let result =
            self.submit_render_pool(client, params, support, priority, &mut pool, timestamps);
        self.tvb_pools.push(pool, GFP_KERNEL)?;
        result
    }

    fn submit_render_pool(
        &mut self,
        client: &mut g16_vm::AddressSpace,
        params: &render::Parameters,
        support: &Support,
        priority: u32,
        pool: &mut crate::g16_tvb::Tvb,
        timestamps: &[u64; 4],
    ) -> Result<[u64; 4]> {
        if self.render_failed {
            return Err(EIO);
        }
        if !params.valid() || !(1..=2).contains(&priority) {
            return Err(EINVAL);
        }
        let space = client.roots();
        let mut params = *params;
        params.tiling_admission = pool.addresses.buffer_manager_slot;
        params.fragment_admission = pool.addresses.buffer_manager_slot;
        let mut a = render::Addresses::bootstrap()
            .publication(self.render_publication)
            .ok_or(ENOSPC)?;
        pool.work_addresses(&mut a, self.render_publication);
        // Timestamp microcommands write private per-Work counter slots. The
        // frontend copies the converted values to caller BOs before its fence.
        let _ = timestamps;
        a.tiling_queue = 0xffff_fc20_c000_0000;
        a.fragment_queue = g16_fw::queue::QUEUE;
        a.support = support.address();
        a.render_shared_state = support.address() + 0x1000;
        a.tiling_status_page = params.ta_status & !0x3fff;
        a.fragment_status_page = params.fragment_status & !0x3fff;
        let queue_ordinal = self.render_queue_publication;
        a.fragment_counter = queue_ordinal * 2;
        a.tiling_counter = queue_ordinal * 2 + 1;
        let va = self.next_work_va;
        self.next_work_va = va.checked_add(0x14000).ok_or(ENOSPC)?;
        let alias = 0x72_0000_0000 + self.render_publication * 0x8000;
        if alias + 0x8000 > 0x73_0000_0000 {
            return Err(ENOSPC);
        }
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        fw.alloc(va, 0x10000, crate::pgtable::prot::PROT_FW_PRIV_RW)?;
        for stage in 0..2u64 {
            let work_page = va + stage * 0x8000;
            let register_page = alias + stage * 0x4000;
            let pa = fw.physical(work_page)?;
            for view in [self._address_space.as_mut().ok_or(EIO)?, &mut *client] {
                view.low.map_pages(
                    register_page..register_page + 0x4000,
                    pa,
                    crate::pgtable::prot::PROT_GPU_SHARED_RW,
                    false,
                )?;
                view.sync();
            }
            if stage == 0 {
                a.tiling_work = work_page + (a.tiling_work & 0x3fff);
                a.tiling_microsequence = work_page + 0x4000 + (a.tiling_microsequence & 0x3fff);
                a.tiling_prelude = work_page + 0x4000;
                a.tiling_register_alias = register_page + (a.tiling_register_alias & 0x3fff);
            } else {
                a.fragment_work = work_page + (a.fragment_work & 0x3fff);
                a.fragment_microsequence = work_page + 0x4000 + (a.fragment_microsequence & 0x3fff);
                a.fragment_prelude = work_page + 0x4000;
                a.fragment_register_alias = register_page + (a.fragment_register_alias & 0x3fff);
            }
        }
        fw.sync();
        crate::mem::tlbi_all();
        crate::mem::sync();
        // Encode everything before any client root or producer is published.
        let mut ta = params.tiling_work(&a).ok_or(EINVAL)?;
        let mut frag = params.fragment_work(&a).ok_or(EINVAL)?;
        let raw_timestamps = [va + 0x100, va + 0x108, va + 0x110, va + 0x118];
        ta[0x8c8..0x8d0].copy_from_slice(&raw_timestamps[0].to_le_bytes());
        ta[0x8d0..0x8d8].copy_from_slice(&raw_timestamps[1].to_le_bytes());
        frag[0xc10..0xc18].copy_from_slice(&raw_timestamps[2].to_le_bytes());
        frag[0xc18..0xc20].copy_from_slice(&raw_timestamps[3].to_le_bytes());
        // The tiling buffer slot contains VM-relative backing. Replacing its
        // root requires a fresh bind even though the firmware queues persist.
        let rebind = self.render_publication == 0;
        let initbm = !pool.initialized;
        self.render_context(Some(space))?;
        // The shared count covers individual Work items, not render pairs.
        // Firmware retires one tiling and one fragment item per submission.
        let submitted = support.submissions.load(Ordering::Relaxed);
        let next_submitted = submitted.checked_add(2).ok_or(ENOSPC)?;
        let use_count = u32::try_from(next_submitted).map_err(|_| ENOSPC)?;
        // From here an error conservatively quarantines the client root.
        self.render_failed = true;
        self._firmware_space
            .as_mut()
            .ok_or(EIO)?
            .write_live(a.render_shared_state, &use_count.to_le_bytes())?;
        let result = self.publish_render(&a, &ta, &frag, priority, rebind, initbm, client, pool);
        if result.is_ok() {
            pool.initialized = true;
            support.submissions.store(next_submitted, Ordering::Relaxed);
            self.render_queue_root = space.low;
            self.render_publication += 1;
            self.render_queue_publication = queue_ordinal + 1;
            self.render_channel_publications[priority as usize] += 1;
            self.render_failed = false;
        }
        if result? {
            return Err(ENOMEM);
        }
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        let mut out = [0; 4];
        for (i, address) in raw_timestamps.iter().enumerate() {
            out[i] = timestamp_ns(fw.read_u64(*address)?)?;
        }
        Ok(out)
    }

    fn publish_render(
        &mut self,
        a: &render::Addresses,
        ta: &[u8],
        frag: &[u8],
        priority: u32,
        rebind: bool,
        initbm: bool,
        client: &mut g16_vm::AddressSpace,
        pool: &mut crate::g16_tvb::Tvb,
    ) -> Result<bool> {
        use g16_fw::queue as q;
        let publication = self.render_publication;
        let epoch = (self.render_channel_publications[priority as usize] & 255) as u32;
        let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
        let routes = [
            &config.channels[priority as usize * 3],
            &config.channels[priority as usize * 3 + 1],
        ];
        let state_offset = |address: u64| 0xd64000 + (address - 0xffff_fc20_0002_8000) as usize;
        let producer_offset = |address: u64| 0xd60000 + (address - 0xffff_fc20_0002_0000) as usize;
        let next = (epoch + 1) & 255;
        let firsts = if rebind {
            [0, 0]
        } else {
            self.render_queue_heads
        };
        let heads = [
            (firsts[0] + 1 + u32::from(initbm)) % 0x500,
            (firsts[1] + 2) % 0x500,
        ];
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        if publication == 0 {
            // The event control covers both stages of the initial render.
            // Seed its submitted count before firmware observes either Work.
            fw.write_live(a.event_count_array + 4, &2u32.to_le_bytes())?;
        }
        // The queue lanes persist independently of the physical work channel.
        for stage in 0..2 {
            let queue = if stage == 0 {
                a.tiling_queue
            } else {
                a.fragment_queue
            };
            let pointers = 0xffff_fc20_0001_0000 + stage as u64 * 0x2860;
            let ring = if stage == 0 {
                0xffff_fc20_c001_0000
            } else {
                q::RING
            };
            if rebind {
                let zeros = [0u8; 0x100];
                for offset in (0..0x2410usize).step_by(zeros.len()) {
                    let size = zeros.len().min(0x2410 - offset);
                    fw.write_live(queue + 0xb0 + offset as u64, &zeros[..size])?;
                }
                let jobs = q::JOBS;
                let context = q::CONTEXT;
                fw.write_live(
                    queue,
                    &q::Queue {
                        pointers,
                        ring,
                        jobs,
                        private: queue + 0xb0,
                        context,
                        uuid: (publication + 2) as u32,
                    }
                    .encode_priority(priority)
                    .ok_or(EINVAL)?,
                )?;
                fw.write_live(pointers, &q::pointers(0x500, 0))?;
                fw.write_live(jobs, &q::jobs(jobs))?;
                fw.write_live(context, &q::context())?;
            } else {
                for off in [0, 0x30, 0x40] {
                    if fw.read_u32(pointers + off)? != firsts[stage] {
                        return Err(EBUSY);
                    }
                }
                // Only priority fields are host-owned after queue admission.
                let bytes = q::Queue {
                    pointers,
                    ring,
                    jobs: 0,
                    private: 0,
                    context: 0,
                    uuid: 0,
                }
                .encode_priority(priority)
                .ok_or(EINVAL)?;
                fw.write_live(queue + 0x30, &bytes[0x30..0x4c])?;
            }
            let prelude = if stage == 0 {
                a.tiling_prelude
            } else {
                a.fragment_prelude
            };
            let work = if stage == 0 {
                a.tiling_work
            } else {
                a.fragment_work
            };
            let micro = if stage == 0 {
                a.tiling_microsequence
            } else {
                a.fragment_microsequence
            };
            fw.write_live(
                prelude,
                &if stage == 0 {
                    a.tiling_prelude(pool.blocks)
                } else {
                    a.fragment_prelude()
                },
            )?;
            fw.write_live(work, if stage == 0 { ta } else { frag })?;
            if stage == 0 {
                fw.write_live(micro, &a.tiling_microsequence())?;
            } else {
                fw.write_live(micro, &a.fragment_microsequence())?;
            }
            let mut pos = firsts[stage];
            if stage == 1 || initbm {
                fw.write_live(ring + u64::from(pos) * 8, &prelude.to_le_bytes())?;
                pos = (pos + 1) % 0x500;
            }
            fw.write_live(ring + u64::from(pos) * 8, &work.to_le_bytes())?;
            fw.write_live(pointers + 0x40, &heads[stage].to_le_bytes())?;
        }
        if publication != 0 {
            let event_count = fw.read_u32(a.event_count_array + 4)?;
            fw.write_live(
                a.event_count_array + 4,
                &event_count.wrapping_add(2).to_le_bytes(),
            )?;
        }
        crate::mem::sync();
        self.drain_render_rx()?;
        let dev = self.dev.clone();
        let sgx_guard = self._sgx.try_access().ok_or(ENODEV)?;
        let sgx = &*sgx_guard;
        // The selected fragment channel is published before tiling; its barrier
        // waits for the matching TA completion stamp.
        for stage in [1usize, 0] {
            let route = routes[stage];
            let state = state_offset(route.state[0]);
            let done = state_offset(route.state[1]);
            let producer = producer_offset(route.state[2]);
            if (
                sgx.try_read32(state)?,
                sgx.try_read32(done)?,
                sgx.try_read32(producer)?,
            ) != (epoch, epoch, epoch)
            {
                return Err(EBUSY);
            }
            let ring = route.ring;
            let (queue, event) = if stage == 0 {
                (a.tiling_queue, a.tiling_event)
            } else {
                (a.fragment_queue, a.fragment_event)
            };
            self._firmware_space.as_mut().ok_or(EIO)?.write_live(
                ring + u64::from(epoch) * 0x18,
                &q::channel(
                    queue,
                    heads[stage] as u16,
                    event as u8,
                    stage as u32,
                    rebind,
                    publication + 1,
                ),
            )?;
            sgx.try_write32(next, producer)?;
        }
        crate::mem::sync();
        drop(sgx_guard);
        self._rtkit
            .as_mut()
            .send_message(0x21, 0x0083_0000_0000_0000 | (u64::from(priority) << 2))?;
        let mut service = RenderEvents::default();
        for _ in 0..10000 {
            self.service_render_events(client, pool, a, &mut service)?;
            self.drain_render_rx_inner(false)?;
            let sgx_guard = self._sgx.try_access().ok_or(ENODEV)?;
            let sgx = &*sgx_guard;
            let fw = self._firmware_space.as_mut().ok_or(EIO)?;
            let mut channels = true;
            for route in routes {
                channels &= sgx.try_read32(state_offset(route.state[0]))? == next
                    && sgx.try_read32(state_offset(route.state[1]))? == next;
            }
            let queues = fw.read_u32(0xffff_fc20_0001_0000)? == heads[0]
                && fw.read_u32(q::POINTERS)? == heads[1]
                && fw.read_u32(0xffff_fc20_0001_0030)? == heads[0]
                && fw.read_u32(q::POINTERS + 0x30)? == heads[1];
            let stamps = fw.read_u32(a.tiling_firmware_stamp)? == a.tiling_stamp as u32
                && fw.read_u32(a.fragment_firmware_stamp)? == a.fragment_stamp as u32;
            let control_idle = sgx.try_read32(0xd640c0)? == sgx.try_read32(0xd60060)?
                && sgx.try_read32(0xd640c8)? == sgx.try_read32(0xd60060)?;
            if channels && queues && stamps && control_idle {
                if fw.read_u32(pool.addresses.buffer_manager + 0x3c)? != pool.blocks {
                    return Err(EIO);
                }
                self.render_queue_heads = heads;
                return Ok(service.memory_limit);
            }
            drop(sgx_guard);
            fsleep(Delta::from_millis(1));
        }
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        dev_err!(
            dev.as_ref(),
            "G16: render {} timeout; TA read/done {}/{} 3D {}/{} stamps {:#x}/{:#x}\n",
            publication,
            fw.read_u32(0xffff_fc20_0001_0030)?,
            fw.read_u32(0xffff_fc20_0001_0000)?,
            fw.read_u32(q::POINTERS + 0x30)?,
            fw.read_u32(q::POINTERS)?,
            fw.read_u32(a.tiling_firmware_stamp)?,
            fw.read_u32(a.fragment_firmware_stamp)?
        );
        let sgx_guard = self._sgx.try_access().ok_or(ENODEV)?;
        let sgx = &*sgx_guard;
        dev_err!(
            dev.as_ref(),
            "G16: fault {:#x} address {:#x}\n",
            sgx.read64(0xd8c0),
            sgx.read64(0xd8c8)
        );
        Err(ETIMEDOUT)
    }
}

impl Bootstrap {
    pub(crate) fn map_compute_work(&mut self, space: &mut g16_vm::AddressSpace) -> Result {
        let opening = self._opening_client.as_mut().ok_or(EIO)?;
        for (base, size) in compute::private_ranges() {
            for va in (base..base + size as u64).step_by(0x4000) {
                if space.low.translate(va)?.is_some() {
                    return Err(EEXIST);
                }
                let pa = opening.low.translate(va)?.ok_or(EIO)?;
                space.low.map_pages(
                    va..va + 0x4000,
                    pa,
                    crate::pgtable::prot::PROT_GPU_SHARED_RW,
                    false,
                )?;
            }
        }
        space.sync();
        Ok(())
    }

    pub(crate) fn submit_compute(
        &mut self,
        client: &mut g16_vm::AddressSpace,
        params: &compute::Parameters,
        priority: u32,
        memory: &impl crate::g16_cdm::Memory,
        _timestamps: &[u64; 4],
    ) -> Result<[u64; 4]> {
        let space = client.roots();
        use g16_fw::queue as q;
        if self.render_failed {
            return Err(EIO);
        }
        if !(1..=2).contains(&priority) {
            return Err(EINVAL);
        }
        let rebind = self.compute_publication == 0;
        let ordinal = if rebind {
            0
        } else {
            self.compute_queue_publication
        };
        let resource = crate::g16_cdm::resource(memory, params.cdm, params.cdm_end)?;
        client.snapshot(memory, resource)?;
        let mut params = *params;
        params.resource = resource;
        let work = self.next_work_va;
        let alias = 0x71_0000_0000 + self.compute_publication * 0x4000;
        if alias + 0x4000 > 0x72_0000_0000 {
            return Err(ENOSPC);
        }
        self.next_work_va = work.checked_add(0x8000).ok_or(ENOSPC)?;
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        fw.alloc(work, 0x4000, crate::pgtable::prot::PROT_FW_PRIV_RW)?;
        let pa = fw.physical(work)?;
        self._address_space.as_mut().ok_or(EIO)?.low.map_pages(
            alias..alias + 0x4000,
            pa,
            crate::pgtable::prot::PROT_GPU_SHARED_RW,
            false,
        )?;
        client.low.map_pages(
            alias..alias + 0x4000,
            pa,
            crate::pgtable::prot::PROT_GPU_SHARED_RW,
            false,
        )?;
        let a = compute::Addresses::publication(self.compute_publication, ordinal, work, alias)
            .ok_or(ENOSPC)?;
        self._address_space.as_ref().ok_or(EIO)?.sync();
        fw.sync();
        client.sync();
        crate::mem::tlbi_all();
        crate::mem::sync();
        let work = params.work(&a).ok_or(EINVAL)?;
        let count = self.compute_publication.checked_add(1).ok_or(ENOSPC)?;
        let count32 = u32::try_from(count).map_err(|_| ENOSPC)?;
        self.compute_context(Some(space))?;
        self.render_failed = true;
        let first = if rebind { 0 } else { self.compute_queue_head };
        let head = (first + 1) % 0x500;
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        let queue = q::Queue {
            pointers: compute::POINTERS,
            ring: compute::RING,
            jobs: compute::JOBS,
            private: compute::QUEUE + 0xb0,
            context: compute::CONTEXT,
            uuid: (self.compute_publication + 2) as u32,
        }
        .encode_priority(priority)
        .ok_or(EINVAL)?;
        if rebind {
            let zeros = [0u8; 0x100];
            for offset in (0..0x2410usize).step_by(zeros.len()) {
                fw.write_live(
                    compute::QUEUE + 0xb0 + offset as u64,
                    &zeros[..zeros.len().min(0x2410 - offset)],
                )?;
            }
            fw.write_live(compute::QUEUE, &queue)?;
            fw.write_live(compute::POINTERS, &q::pointers(0x500, 0))?;
            fw.write_live(compute::JOBS, &q::jobs(compute::JOBS))?;
            fw.write_live(compute::CONTEXT, &q::context())?;
        } else {
            for off in [0, 0x30, 0x40] {
                if fw.read_u32(compute::POINTERS + off)? != first {
                    return Err(EBUSY);
                }
            }
            fw.write_live(compute::QUEUE + 0x30, &queue[0x30..0x4c])?;
        }
        fw.write_live(compute::SHARED_STATE, &count32.to_le_bytes())?;
        fw.write_live(a.work, &work)?;
        fw.write_live(a.microsequence, &params.microsequence(&a))?;
        let notifier = a.notifier(count32);
        // Retained notifier list links are firmware-owned even after completion.
        fw.write_live(a.notifier, &notifier[..0x14])?;
        fw.write_live(a.notifier + 0x24, &notifier[0x24..0x28])?;
        fw.write_live(a.threshold, &count32.to_le_bytes())?;
        let previous = a.stamp.checked_sub(0x100).ok_or(EINVAL)?;
        fw.write_live(a.driver_stamp, &previous.to_le_bytes())?;
        fw.write_live(a.firmware_stamp, &previous.to_le_bytes())?;
        // Optional resume state is null, matching ordinary shim compute.
        fw.write_live(compute::RING + u64::from(first) * 8, &a.work.to_le_bytes())?;
        fw.write_live(compute::POINTERS + 0x40, &head.to_le_bytes())?;
        crate::mem::sync();
        self.drain_render_rx()?;
        let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
        let route = &config.channels[2];
        let state = 0xd64000 + (route.state[0] - 0xffff_fc20_0002_8000) as usize;
        let done = 0xd64000 + (route.state[1] - 0xffff_fc20_0002_8000) as usize;
        let producer = 0xd60000 + (route.state[2] - 0xffff_fc20_0002_0000) as usize;
        let epoch = (self.compute_channel_publications[0] & 255) as u32;
        let next = (epoch + 1) & 255;
        let guard = self._sgx.try_access().ok_or(ENODEV)?;
        let sgx = &*guard;
        if (
            sgx.try_read32(state)?,
            sgx.try_read32(done)?,
            sgx.try_read32(producer)?,
        ) != (epoch, epoch, epoch)
        {
            return Err(EBUSY);
        }
        self._firmware_space.as_mut().ok_or(EIO)?.write_live(
            route.ring + u64::from(epoch) * 0x18,
            &q::channel(
                a.queue,
                head as u16,
                a.event as u8,
                2,
                rebind,
                self.compute_publication + 1,
            ),
        )?;
        crate::mem::sync();
        sgx.try_write32(next, producer)?;
        crate::mem::sync();
        drop(guard);
        self._rtkit
            .as_mut()
            .send_message(0x21, 0x0083_0000_0000_0002)?;
        for _ in 0..10000 {
            self.drain_render_rx()?;
            let guard = self._sgx.try_access().ok_or(ENODEV)?;
            let sgx = &*guard;
            let fw = self._firmware_space.as_ref().ok_or(EIO)?;
            if sgx.try_read32(state)? == next
                && sgx.try_read32(done)? == next
                && fw.read_u32(compute::POINTERS)? == head
                && fw.read_u32(compute::POINTERS + 0x30)? == head
                && fw.read_u32(a.firmware_stamp)? == a.stamp
            {
                self.compute_publication += 1;
                self.compute_queue_publication = ordinal + 1;
                self.compute_channel_publications[0] += 1;
                self.compute_queue_head = head;
                self.compute_queue_root = space.low;
                self.render_failed = false;
                return Ok([
                    timestamp_ns(fw.read_u64(a.timestamp_start)?)?,
                    timestamp_ns(fw.read_u64(a.timestamp_end)?)?,
                    0,
                    0,
                ]);
            }
            drop(guard);
            fsleep(Delta::from_millis(1));
        }
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        let guard = self._sgx.try_access().ok_or(ENODEV)?;
        let sgx = &*guard;
        dev_err!(self.dev.as_ref(), "G16: compute {} timeout; channel {}/{}/{} queue {}/{} stamp {:#x}; fault {:#x} at {:#x}\n",
            self.compute_publication, sgx.try_read32(state)?, sgx.try_read32(done)?, sgx.try_read32(producer)?,
            fw.read_u32(compute::POINTERS)?, fw.read_u32(compute::POINTERS + 0x30)?,
            fw.read_u32(a.firmware_stamp)?, sgx.read64(0xd8c0), sgx.read64(0xd8c8));
        Err(ETIMEDOUT)
    }
}

#[derive(Default)]
struct RenderEvents {
    replies: u32,
    refused: bool,
    memory_limit: bool,
}

impl Bootstrap {
    /// Consume only events owned by this publication. Growth transfers retained
    /// pages -> block list -> host counts -> reply -> producer, as in the shim.
    fn service_render_events(
        &mut self,
        client: &mut g16_vm::AddressSpace,
        pool: &mut crate::g16_tvb::Tvb,
        a: &render::Addresses,
        service: &mut RenderEvents,
    ) -> Result {
        const STATE: u64 = 0xfffffc200003f1c0;
        const RING: u64 = 0xfffffc200003f440;
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        let mut cursor = fw.read_u32(STATE)?;
        let end = fw.read_u32(STATE + 0x20)?;
        if cursor >= 256 || end >= 256 {
            return Err(EIO);
        }
        while cursor != end {
            let base = RING + u64::from(cursor) * 0x48;
            let kind = fw.read_u32(base)?;
            let next = (cursor + 1) & 255;
            if kind == 1 {
                fw.write_live(STATE, &next.to_le_bytes())?;
                cursor = next;
                continue;
            }
            if !matches!(kind, 6 | 7) {
                dev_err!(self.dev.as_ref(), "G16: unexpected render Event {}\n", kind);
                return Err(EIO);
            }
            let guard = self._sgx.try_access().ok_or(ENODEV)?;
            let sgx = &*guard;
            let head = sgx.try_read32(0xd640c0)?;
            let ack = sgx.try_read32(0xd640c8)?;
            let tail = sgx.try_read32(0xd60060)?;
            if head >= 256 || ack >= 256 || tail >= 256 {
                return Err(EIO);
            }
            let successor = (tail + 1) & 255;
            if successor == head || successor == ack {
                break;
            }
            let mut reply = [0u8; 64];
            if kind == 6 {
                if service.memory_limit
                    || service.replies >= 256
                    || fw.read_u32(base + 4)? != a.context_id as u32
                    || fw.read_u32(base + 8)? != pool.addresses.buffer_manager_slot as u32
                    || fw.read_u32(base + 12)? != pool.counter
                    || fw.read_u32(base + 16)? != 0
                    || fw.read_u64(base + 20)? != 0
                {
                    return Err(EIO);
                }
                let grown = pool.grow(fw, client)?;
                dev_info!(
                    self.dev.as_ref(),
                    "G16: TVB slot {} blocks {} growth {}\n",
                    pool.addresses.buffer_manager_slot,
                    pool.blocks,
                    grown
                );
                for (i, word) in [
                    8,
                    u32::from(grown),
                    pool.addresses.buffer_manager_slot as u32,
                    a.context_id as u32,
                    pool.counter,
                ]
                .iter()
                .enumerate()
                {
                    reply[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
                }
                service.refused |= !grown;
                service.replies += 1;
                pool.counter = pool.counter.checked_add(1).ok_or(EIO)?;
            } else {
                if !service.refused
                    || service.memory_limit
                    || fw.read_u32(base + 4)? != 0
                    || fw.read_u32(base + 8)? != 1
                    || fw.read_u32(base + 12)? != a.fragment_event as u32
                    || fw.read_u32(base + 16)? != a.fragment_stamp as u32
                {
                    return Err(EIO);
                }
                reply[..4].copy_from_slice(&9u32.to_le_bytes());
                reply[4..8].copy_from_slice(&1u32.to_le_bytes());
                reply[8..16].copy_from_slice(&a.fragment_queue.to_le_bytes());
                reply[16..20].copy_from_slice(&(a.fragment_stamp as u32).to_le_bytes());
                service.memory_limit = true;
            }
            crate::mem::sync();
            let control = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS).channels[12].ring;
            fw.write_live(control + u64::from(tail) * 64, &reply)?;
            crate::mem::sync();
            fw.write_live(STATE, &next.to_le_bytes())?;
            sgx.try_write32(successor, 0xd60060)?;
            crate::mem::sync();
            self._rtkit
                .as_mut()
                .send_message(0x21, 0x0084_0000_0000_0011)?;
            cursor = next;
        }
        Ok(())
    }
}

fn timestamp_ns(raw: u64) -> Result<u64> {
    // Firmware profiling timestamps use the 24 MHz always-on clock. M4's
    // architectural CNTFRQ can instead describe the extended 1 GHz counter.
    const FREQUENCY: u64 = 24_000_000;
    (raw / FREQUENCY)
        .checked_mul(1_000_000_000)
        .and_then(|n| n.checked_add((raw % FREQUENCY) * 1_000_000_000 / FREQUENCY))
        .ok_or(EOVERFLOW)
}
