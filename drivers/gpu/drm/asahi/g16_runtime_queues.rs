// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! Firmware queue lifecycle: Work arena slots, queue contexts, and preparing
//! a userspace queue's lanes, events and TVB pool before its first command.

use super::*;

impl Bootstrap {
    /// Work storage must stay inside the 4 GiB windows that alias it for the
    /// GPU (0x71/0x72_0000_0000 + offset): Work arenas grow from WORK_BASE,
    /// and every other firmware object (queue identities, TVB pools) takes
    /// its VA from OBJECT_BASE above the window. Before #174 they shared one
    /// cursor, so context churn (about 0.4 MiB per queue and VM) pushed new
    /// Work past the window after enough contexts in one boot: CTS on #173
    /// failed renders with ENOSPC at offset 5.3 GiB.
    pub(super) fn work_space_left(&self, render: bool) -> bool {
        let index = usize::from(render);
        let stride = if render { 0x1c000 } else { 0x8000 };
        !self.free_work[index].is_empty()
            || !self.work_arenas[index].is_empty()
            || self.next_work_va + 16 * stride <= WORK_BASE + WORK_WINDOW
    }

    /// Lease Work storage until notifier/driver stamps and transport retire.
    /// Backing stays mapped. Queue-owned wrapping stamps outlive reused Work,
    /// so a pending dependency never follows a recycled completion address.
    /// A Work slot and, for a reused one, the client root its alias is
    /// already mapped into; the device alias exists for every reused slot.
    pub(super) fn allocate_work(&mut self, render: bool) -> Result<(u64, Option<u64>)> {
        use crate::pgtable::prot::{PROT_FW_PRIV_RW, PROT_FW_SHARED_RW};
        let index = usize::from(render);
        if let Some((va, root)) = self.free_work[index].pop() {
            return Ok((va, Some(root)));
        }
        let stride = if render { 0x1c000 } else { 0x8000 };
        if self.work_arenas[index].is_empty() {
            // Retirement must not allocate, so the free list keeps room for
            // every slot ever carved. Counting flights instead misses slots
            // still leased after their own flight retired (a submission's
            // notifier owner, held for its later renders).
            let slots = self.work_slots[index] + 16;
            self.free_work[index].reserve(slots - self.free_work[index].len(), GFP_KERNEL)?;
            let base = self.next_work_va;
            let end = base.checked_add(16 * stride).ok_or(ENOSPC)?;
            if end > WORK_BASE + WORK_WINDOW {
                return Err(ENOSPC);
            }
            // Work VA only grows; a steady rise under constant load means
            // leaked Work allocations.
            pr_info!(
                "G16: work arena refill render={} base={:#x} free=[{}, {}] flights={}\n",
                render,
                base,
                self.free_work[0].len(),
                self.free_work[1].len(),
                self.flights.len()
            );
            // Failed refills retain their partial allocations, but no part of
            // that range becomes available for a later Work or another arena.
            self.next_work_va = end;
            let fw = self._firmware_space.as_mut().ok_or(EIO)?;
            for va in (base..end).step_by(stride as usize) {
                fw.alloc(va, if render { 0x10000 } else { 0x4000 }, PROT_FW_PRIV_RW)?;
                if render {
                    fw.alloc(va + 0x10000, 0xc000, PROT_FW_SHARED_RW)?;
                } else {
                    fw.alloc(va + 0x4000, 0x4000, PROT_FW_SHARED_RW)?;
                }
            }
            fw.sync();
            self.work_arenas[index] = base..end;
            self.work_slots[index] = slots;
        }
        let va = self.work_arenas[index].start;
        self.work_arenas[index].start += stride;
        Ok((va, None))
    }

    /// Collect storage whose context destruction firmware has consumed. Retire
    /// polls this too, so the half-ring comparison sees each target promptly.
    /// It must not allocate: publication reserved each free-list slot.
    pub(super) fn collect_destroyed_queues(&mut self) -> Result {
        if self.destroying_queues.is_empty() {
            return Ok(());
        }
        let guard = self._sgx.try_access().ok_or(ENODEV)?;
        let read = guard.try_read32(0xd640c0)?;
        let done = guard.try_read32(0xd640c8)?;
        drop(guard);
        if read >= 256 || done >= 256 {
            return Err(EIO);
        }
        let mut i = 0;
        while i < self.destroying_queues.len() {
            let (base, target) = self.destroying_queues[i];
            if reached(read, target, 256) && reached(done, target, 256) {
                self.destroying_queues.swap_remove(i);
                self.free_queues
                    .push_within_capacity(base)
                    .map_err(|_| EIO)?;
            } else {
                i += 1;
            }
        }
        Ok(())
    }

    /// Firmware may still hold a retired queue's context ID binding, so the
    /// context is destroyed before its storage serves another queue.
    pub(super) fn recycle_queues(&mut self) -> Result {
        self.collect_destroyed_queues()?;
        loop {
            self.destroying_queues.reserve(1, GFP_KERNEL)?;
            self.free_queues
                .reserve(self.destroying_queues.len() + 1, GFP_KERNEL)?;
            let Some(base) = self.retired_queues.lock().pop() else {
                return Ok(());
            };
            match self.destroy_queue_context(base)? {
                Some(target) => self.destroying_queues.push((base, target), GFP_KERNEL)?,
                None => {
                    // The control ring is full. Its slot was just vacated.
                    let _ = self.retired_queues.lock().push_within_capacity(base);
                    return Ok(());
                }
            }
        }
    }

    /// Publish a context destruction without waiting, as growth replies are.
    /// Returns the control cursor that acknowledges it, or None when full.
    pub(super) fn destroy_queue_context(&mut self, base: u64) -> Result<Option<u32>> {
        let context = base + 0x4140;
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        let ids = fw.read_u32(context)?;
        let fields = [
            (fw.read_u32(context + 0x24)? >> 16) as u8,
            ids as u8,
            (ids >> 8) as u8,
            fw.read_u32(context + 4)? as u8,
        ];
        let guard = self._sgx.try_access().ok_or(ENODEV)?;
        let head = guard.try_read32(0xd640c0)?;
        let ack = guard.try_read32(0xd640c8)?;
        let tail = guard.try_read32(0xd60060)?;
        drop(guard);
        if head >= 256 || ack >= 256 || tail >= 256 {
            return Err(EIO);
        }
        let successor = (tail + 1) & 255;
        if successor == head || successor == ack {
            return Ok(None);
        }
        let control = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS).channels[12].ring;
        fw.write_live(
            control + u64::from(tail) * 64,
            &g16_fw::destroy_context(context, fields),
        )?;
        crate::mem::sync();
        let guard = self._sgx.try_access().ok_or(ENODEV)?;
        guard.try_write32(successor, 0xd60060)?;
        drop(guard);
        crate::mem::sync();
        self._rtkit
            .as_mut()
            .send_message(0x21, 0x0084_0000_0000_0011)?;
        Ok(Some(successor))
    }

    /// Initialize three lanes in `base`, which is newly allocated or recycled.
    /// `identity` is a fresh address whose page index names the queues, so a
    /// recycled queue never reuses a destroyed queue's identity.
    pub(super) fn init_queues(
        &mut self,
        owner: &FirmwareQueues,
        base: u64,
        identity: u64,
        fresh: bool,
    ) -> Result<[Lane; 3]> {
        use crate::pgtable::prot::{PROT_FW_PRIV_RW, PROT_FW_SHARED_RW};
        use g16_fw::queue as q;

        let lanes: [Lane; 3] = core::array::from_fn(|stage| {
            let queue = base + stage as u64 * 0xc000;
            Lane {
                queue,
                pointers: queue + 0x4000,
                ring: queue + 0x8000,
                head: 0,
                new: true,
            }
        });
        if fresh {
            // Reserve before the region exists, so it can always be retired.
            let total = self.queue_regions + 1;
            let mut retired = self.retired_queues.lock();
            let additional = total - retired.len();
            retired.reserve(additional, GFP_KERNEL)?;
            drop(retired);
            self.queue_regions = total;
        }
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        for lane in &lanes {
            if fresh {
                fw.alloc(lane.queue, 0x4000, PROT_FW_PRIV_RW)?;
                fw.alloc(lane.pointers, 0x8000, PROT_FW_SHARED_RW)?;
            } else {
                // Restore a new allocation's zeroed state, including the
                // firmware-private words after each queue descriptor.
                fw.zero_live(lane.queue, 0x4000)?;
                fw.zero_live(lane.pointers, 0x8000)?;
            }
        }
        // A notifier list and GPU context belong to the public queue,
        // shared only by its own vertex, fragment and compute lanes.
        let jobs = base + 0x4100;
        let context = base + 0x4140;
        for (stage, lane) in lanes.iter().enumerate() {
            let named = identity + stage as u64 * 0xc000;
            let mut descriptor = q::Queue {
                pointers: lane.pointers,
                ring: lane.ring,
                jobs,
                private: lane.queue + 0xb0,
                context,
                uuid: ((named - 0xffff_fc22_0000_0000) >> 14) as u32 + 2,
            }
            .encode_priority(owner.priority)
            .ok_or(EINVAL)?;
            // Native TA/3D profiles allow firmware to suspend and restart
            // renders. Their Work/scratch leases survive until the final
            // notifier/driver stamp, including intervening partial runs.
            // Compute remains non-preemptible, matching the M1/M2 driver;
            // Metal's compute queues use the native preemptible profile.
            let preemptible = stage != 2;
            if !preemptible {
                descriptor[0x30..0x34].copy_from_slice(&0u32.to_le_bytes());
                descriptor[0x48..0x4c].copy_from_slice(&1u32.to_le_bytes());
            }
            fw.write_live(lane.queue, &descriptor)?;
            fw.write_live(lane.pointers, &q::pointers(0x500, 0))?;
            // A newly created timeline starts immediately before its first
            // target. This also permits qualification near stamp rollover.
            let (ordinal, first) = match stage {
                0 => (owner.render_count, 0xc00),
                1 => (owner.render_count, 0x100),
                _ => (owner.compute_count, 0x100),
            };
            let current = g16_stamp::previous(g16_stamp::value(ordinal, first));
            fw.write_live(lane.firmware_stamp(), &current.to_le_bytes())?;
            if stage < 2 {
                fw.write_live(lane.driver_stamp(), &current.to_le_bytes())?;
            }
        }
        fw.write_live(jobs, &q::jobs(jobs))?;
        fw.write_live(context, &q::context())?;
        let notifier = render_notifier(&lanes);
        let count = render_notifier_count(&lanes);
        fw.write_live(notifier, &[0u8; 0x100])?;
        fw.write_live(notifier, &count.to_le_bytes())?;
        fw.write_live(notifier + 0x10, &0x50u32.to_le_bytes())?;
        fw.write_live(count, &0u32.to_le_bytes())?;
        fw.sync();
        Ok(lanes)
    }

    /// For the SUBMIT ioctl: allocate the queue's firmware rings now rather
    /// than at its first publication.
    pub(crate) fn provision_queue(&mut self, owner: &mut FirmwareQueues) -> Result {
        if self.render_failed {
            return Err(EIO);
        }
        self.prepare_lanes(owner)
    }

    /// For the SUBMIT ioctl: create the VM root's TVB pool now rather than
    /// at its first render publication.
    pub(crate) fn provision_tvb(&mut self, client: &mut g16_vm::AddressSpace, support: &Support) -> Result {
        if self.render_failed {
            return Err(EIO);
        }
        self.tvb_pool(client, support).map(|_| ())
    }

    /// The index of the client root's TVB pool, created on first use. Not
    /// inlined: its creation path would otherwise enlarge the job worker's frame.
    #[inline(never)]
    pub(super) fn tvb_pool(&mut self, client: &mut g16_vm::AddressSpace, support: &Support) -> Result<usize> {
        let root = client.roots().low;
        if let Some(index) = self.tvb_pools.iter().position(|p| p.root == root) {
            return Ok(index);
        }
        self.tvb_pools.reserve(1, GFP_KERNEL)?;
        let base = self.next_object_va;
        self.next_object_va = base.checked_add(0x44000).ok_or(ENOSPC)?;
        let pool = crate::g16_tvb::Tvb::new(
            self._firmware_space.as_mut().ok_or(EIO)?,
            client,
            base,
            7 + u64::from(support.index),
        )?;
        self.tvb_pools.push_within_capacity(pool).map_err(|_| ENOMEM)?;
        Ok(self.tvb_pools.len() - 1)
    }

    pub(super) fn prepare_queues(&mut self, owner: &mut FirmwareQueues) -> Result {
        self.prepare_lanes(owner)?;
        self.prepare_events(owner)
    }

    /// The queue's firmware rings, allocated once per public queue. Not
    /// inlined, like `tvb_pool`: it is cold and its frame is large.
    #[inline(never)]
    pub(super) fn prepare_lanes(&mut self, owner: &mut FirmwareQueues) -> Result {
        if owner.lanes.is_none() {
            self.recycle_queues()?;
            let identity = self.next_object_va;
            self.next_object_va = identity.checked_add(0x24000).ok_or(ENOSPC)?;
            let recycled = self.free_queues.pop();
            let base = recycled.unwrap_or(identity);
            let lanes = match self.init_queues(owner, base, identity, recycled.is_none()) {
                Ok(lanes) => lanes,
                Err(error) => {
                    // A failed fresh allocation keeps any partial regions
                    // owned, like a failed Work refill; recycled storage returns.
                    if let Some(base) = recycled {
                        let _ = self.free_queues.push_within_capacity(base);
                    }
                    return Err(error);
                }
            };
            owner.storage = Some(QueueStorage {
                base,
                retired: self.retired_queues.clone(),
            });
            owner.lanes = Some(lanes);
            for c in &owner.cursors {
                c.set((u32::MAX, u32::MAX));
            }
            owner.notified_works = 0;
            if crate::module_parameters::fw_trace.value() & 1 != 0 {
                dev_info!(
                    self.dev.as_ref(),
                    "G16: queues vertex={:#x} fragment={:#x} compute={:#x} priority={} recycled={}\n",
                    lanes[0].queue,
                    lanes[1].queue,
                    lanes[2].queue,
                    owner.priority,
                    recycled.is_some()
                );
            }
        }
        Ok(())
    }

    /// An event index for each busy period of the queue (released when its
    /// last Work retires). EBUSY while all are in use: admission retries.
    pub(super) fn prepare_events(&mut self, owner: &mut FirmwareQueues) -> Result {
        if owner.events.is_none() {
            // Bits 0-31 are the event indices in use; a queue takes the
            // lowest free one.
            let mut picked = 0u32;
            self.queue_events
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    let free = !(value as u32);
                    if free == 0 {
                        return None;
                    }
                    picked = free.trailing_zeros();
                    Some(value | (1 << picked))
                })
                .map_err(|_| {
                    self.report_pressure(8, "events");
                    EBUSY
                })?;
            if crate::module_parameters::fw_trace.value() & 256 != 0 && owner.last_event != Some(picked) {
                pr_info!("G16: queue event index {:?} -> {}\n", owner.last_event, picked);
            }
            owner.last_event = Some(picked);
            if let Some(lanes) = owner.lanes {
                for (stage, lane) in lanes.iter().enumerate() {
                    self.set_lane_event(lane.firmware_stamp(), 32 + picked * 3 + stage as u32);
                }
            }
            owner.events = Some(QueueEvents {
                slots: self.queue_events.clone(),
                index: picked,
            });
        }
        Ok(())
    }
}
