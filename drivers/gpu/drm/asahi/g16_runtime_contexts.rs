// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! Firmware context slots, render support leases, timestamp apertures and
//! the firmware trace receive ring.

use super::*;

impl Bootstrap {
    pub(crate) fn reserve_timestamp(&mut self, size: u64) -> Result<u64> {
        if size == 0 || size & 0x3fff != 0 {
            return Err(EINVAL);
        }
        // Match the shim's firmware-shared special-object aperture.
        let mut base = g16_fw::TIMESTAMP_BASE;
        for range in &self.timestamp_ranges {
            if base.checked_add(size).ok_or(ENOSPC)? <= range.start {
                break;
            }
            base = base.max(range.end);
        }
        let end = base.checked_add(size).ok_or(ENOSPC)?;
        if end > g16_fw::TIMESTAMP_BASE + g16_fw::TIMESTAMP_SIZE {
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

    /// `mode`: the VM's native mesh mode, latched when it was created.
    pub(crate) fn acquire_render_support(&mut self, pool: &render::OperandPool, mode: g16_mesh::Mode) -> Result<Support> {
        if self.render_failed {
            return Err(EIO);
        }
        let native = mode.enabled();
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
            submissions: AtomicU64::new(
                crate::module_parameters::fw_counter_start.value().wrapping_mul(2),
            ),
            native,
            mode,
        };
        let mut a = render::Addresses::bootstrap();
        a.support = lease.address();
        a.render_shared_state = lease.shared_state();
        let mut page = KVec::new();
        page.extend_with(0x4000, 0u8, GFP_KERNEL)?;
        page[..0x100].copy_from_slice(&pool.support(3 + u64::from(index), a.render_shared_state));
        if lease.native {
            render::initialize_native_support(&mut page[..0x100]);
        }
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        fw.write_live(a.support, &page)?;
        // A reused lease starts its submitted-Work count afresh.
        fw.zero_live(a.render_shared_state, 0x4000)?;
        Ok(lease)
    }

    pub(super) fn set_context(&mut self, index: usize, roots: Option<g16_vm::Roots>) -> Result {
        let res = self
            .dev
            .as_ref()
            .of_node()
            .ok_or(EINVAL)?
            .reserved_mem_region_to_resource_byname(c_str!("ttbs"))?;
        // SAFETY: The reserved table has 64 two-root slots. A slot is changed
        // only with no outstanding Work using it, under the runtime mutex.
        let table = unsafe { io::mem::Mem::try_new(res, io::mem::MemFlag::WB.into())? };
        unsafe {
            let ptr = table.ptr().add(index * 16);
            ptr.cast::<u64>()
                .write_volatile(ptr.cast::<u64>().read_volatile() & !1);
            ptr.add(8)
                .cast::<u64>()
                .write_volatile(ptr.add(8).cast::<u64>().read_volatile() & !1);
            core::arch::asm!("dc cvac, {addr}", "dsb oshst", addr=in(reg) ptr);
            crate::mem::tlbi_asid(index as u8);
            core::arch::asm!("dsb osh", "isb");
            if let Some(roots) = roots {
                ptr.cast::<u64>()
                    .write_volatile(roots.low | ((index as u64) << 48) | 1);
                ptr.add(8)
                    .cast::<u64>()
                    .write_volatile(roots.high | ((index as u64) << 48) | 1);
                core::arch::asm!("dc cvac, {addr}", addr=in(reg) ptr);
            }
        }
        crate::mem::sync();
        if let Some(next) = roots {
            self.diagnostics.handovers[index] = Some(Handover {
                at: Instant::now(),
                previous: self.context_roots[index].map_or(0, |r| r.low),
                next: next.low,
            });
        }
        self.context_roots[index] = roots;
        Ok(())
    }

    /// Invalidate only slots actually bound to this root. New roots need no
    /// shootdown: set_context invalidates the ASID before installing them.
    pub(crate) fn invalidate_client(&self, client: &g16_vm::AddressSpace) {
        let root = client.roots().low;
        if self
            ._address_space
            .as_ref()
            .is_some_and(|s| s.roots().low == root)
        {
            client.low.invalidate(Some(0));
            client.high.invalidate(Some(0));
        }
        for (index, roots) in self.context_roots.iter().enumerate().skip(1) {
            if roots.is_some_and(|r| r.low == root) {
                client.low.invalidate(Some(index as u8));
                client.high.invalidate(Some(index as u8));
            }
        }
        crate::mem::sync();
        client.low.clear_invalidations();
        client.high.clear_invalidations();
    }

    pub(super) fn acquire_context(&mut self, roots: g16_vm::Roots, preferred: usize) -> Result<u32> {
        let existing = (1..64).find(|&i| self.context_roots[i].is_some_and(|r| r.low == roots.low));
        let slot = existing
            .or_else(|| {
                if self.context_users[preferred] == 0 {
                    Some(preferred)
                } else {
                    (1..64).find(|&i| self.context_users[i] == 0)
                }
            })
            .ok_or(EBUSY)?;
        if existing.is_none() {
            let previous = self.context_roots[slot].map(|r| r.low);
            self.set_context(slot, Some(roots))?;
            if crate::module_parameters::debug_ctx.value() != 0 {
                pr_info!(
                    "G16: context {} bound to root {:#x} (previous {:#x})\n",
                    slot,
                    roots.low,
                    previous.unwrap_or(0)
                );
            }
        }
        self.context_users[slot] += 1;
        Ok(slot as u32)
    }

    pub(crate) fn release_render_vm(&mut self, space: g16_vm::Roots) -> Result {
        if self.render_failed {
            return Err(EIO);
        }
        // A later VM can reuse this root's address: forget which free slots
        // have their alias mapped in it.
        for slots in &mut self.free_work {
            for slot in slots.iter_mut().filter(|(_, root)| *root == space.low) {
                slot.1 = 0;
            }
        }
        for index in 1..64 {
            if self.context_roots[index].is_some_and(|r| r.low == space.low) {
                if self.context_users[index] != 0 {
                    return Err(EBUSY);
                }
                self.set_context(index, None)?;
            }
        }
        if let Some(index) = self.tvb_pools.iter().position(|p| p.root == space.low) {
            let a = self.tvb_pools[index].addresses;
            let fw = self._firmware_space.as_mut().ok_or(EIO)?;
            fw.write_live(0xfffffc2000128000 + a.buffer_manager_slot * 16, &[0; 16])?;
            // The pool is three regions: control pages, the shared counter
            // and the block and page lists (see Tvb::new).
            fw.release_region(a.buffer_manager)?;
            fw.release_region(a.buffer_manager_counter)?;
            fw.release_region(a.buffer_manager_block_list)?;
            self.tvb_pools.swap_remove(index);
        }
        Ok(())
    }

    pub(super) fn drain_trace_rx(&mut self) -> Result {
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        for (state, capacity) in [
            (0xffff_fc20_0003_f3c0, 512),
            (0xffff_fc20_0003_f400, 256),
            (0xffff_fc20_0003_f200, 256),
            (0xffff_fc20_0003_f230, 256),
            (0xffff_fc20_0003_f260, 256),
            (0xffff_fc20_0003_f290, 256),
            (0xffff_fc20_0003_f2c0, 256),
            (0xffff_fc20_0003_f2f0, 256),
        ] {
            let read = fw.read_u32(state)?;
            let write = fw.read_u32(state + 0x20)?;
            if read >= capacity || write >= capacity {
                dev_err!(
                    self.dev.as_ref(),
                    "G16: invalid RX cursors at {:#x}: {}/{} capacity {}\n",
                    state,
                    read,
                    write,
                    capacity
                );
                return Err(EIO);
            }
            if read != write && crate::module_parameters::fw_trace.value() & 64 != 0 {
                // SAFETY: ktime_get() has no preconditions.
                let now = unsafe { kernel::bindings::ktime_get() };
                let entry = if state == 0xffff_fc20_0003_f3c0 {
                    0xffff_fc20_000e_5c40 + u64::from(read) * 0x48
                } else {
                    0
                };
                let (w0, w1) = if entry != 0 {
                    (fw.read_u32(entry)?, fw.read_u32(entry + 4)?)
                } else {
                    (0, 0)
                };
                pr_info!(
                    "G16: rxring {} {:#x} {}..{} first={:#x}/{:#x}\n",
                    now, state, read, write, w0, w1
                );
            }
            if read != write {
                if state == 0xffff_fc20_0003_f3c0 {
                    let mut cursor = read;
                    while cursor != write {
                        let base = 0xffff_fc20_000e_5c40 + u64::from(cursor) * 0x48;
                        if fw.read_u32(base)? == 5
                            && fw.read_u32(base + 0x2c)? & 0xffff == 4
                            && fw.read_u64(base + 0x30)? == 0
                            && fw.read_u64(base + 0x14)? != 0
                        {
                            crate::g16_timeline::PARTIAL_RENDERS
                                .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
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
}
