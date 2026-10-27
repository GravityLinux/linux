// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! Compute submission: dependency resolution and publishing a compute Work.

use super::*;

impl Bootstrap {
    pub(crate) fn map_compute_work(
        &mut self,
        space: &mut g16_vm::AddressSpace,
        private: core::ops::Range<u64>,
    ) -> Result {
        let opening = self._opening_client.as_mut().ok_or(EIO)?;
        for (base, size) in compute::private_ranges() {
            // Scratch and marker are private to each command at fresh VAs.
            // Only the device-owned FList directory and spill pool are shared.
            if base == compute::SCRATCH || base == compute::MARKER {
                continue;
            }
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
        space.init_compute_private(private);
        space.sync();
        Ok(())
    }

    /// Order a compute Work against other queues in the kernel rather than
    /// with firmware dependency items, which can stall the firmware when a
    /// burst of compute Works carries them (Dota 2's loading screen); Metal's
    /// compute Works carry none either. The queue's own compute lane is
    /// ordered by its ring. Any other awaited stamp must already be reached:
    /// otherwise return EBUSY, and the submission worker publishes what it
    /// has staged and retries after the next completion, holding back the
    /// rest of this queue.
    pub(super) fn resolve_compute_dependencies(
        &self,
        queue: &mut FirmwareQueues,
        dependencies: &[Option<Stamp>; 2],
        tally: &mut DepTally,
    ) -> Result<[Option<Stamp>; 2]> {
        let own_lane = queue.lanes.as_ref().map(|lanes| lanes[2].firmware_stamp());
        // A wait on this queue's own render lanes (the
        // render -> compute edge inside one submission) stays a firmware
        // item, so the firmware starts the compute right after the render
        // instead of after a host round trip. Other queues stay kernel-side.
        let own_render = queue
            .lanes
            .as_ref()
            .map(|lanes| [lanes[0].firmware_stamp(), lanes[1].firmware_stamp()]);
        let mut kept = [None; 2];
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        for (slot, dep) in dependencies.iter().enumerate() {
            let Some(dep) = dep else { continue };
            if own_render.is_some_and(|r| r.contains(&dep.address)) {
                if !self.elide_item(queue, 2, dep, false, tally)? {
                    kept[slot] = Some(*dep);
                }
                continue;
            }
            if own_lane != Some(dep.address) {
                let now = fw.read_u32(dep.address)?;
                let done = g16_stamp::reached(now, dep.value);
                self.hold_check("compute", dep, now, done)?;
                if !done {
                    crate::g16_timeline::DEFERRED_CPU_HOLD.fetch_add(1, Ordering::Relaxed);
                    return Err(EBUSY);
                }
            }
        }
        Ok(kept)
    }

    pub(crate) fn submit_compute(
        &mut self,
        queues: Arc<Mutex<FirmwareQueues>>,
        owner: Arc<Mutex<g16_vm::AddressSpace>>,
        params: &compute::Parameters,
        timestamps: [u64; 4],
        dependencies: &[Option<Stamp>; 2],
    ) -> Result<Receipt> {
        let _t = crate::g16_timeline::Timer::start(&crate::g16_timeline::P_SUBMIT_COMPUTE);
        let mut queue = queues.lock();
        let mut client = owner.lock();
        let mut tally = DepTally::default();
        let resolved = self.resolve_compute_dependencies(&mut queue, dependencies, &mut tally)?;
        let dependencies = &resolved;
        self.reserve_flight(
            &queue,
            false,
            client.roots().low,
            [0, 0, 1 + dependencies.iter().flatten().count() as u32],
        )?;
        self.prepare_queues(&mut queue)?;
        let result = self.submit_compute_queue(
            &mut queue,
            queues.clone(),
            &mut client,
            owner.clone(),
            params,
            timestamps,
            dependencies,
        );
        if queue.pending == 0 {
            queue.events = None;
        }
        if result.is_ok() {
            tally.publish();
        }
        result
    }

    #[inline(never)]
    pub(super) fn submit_compute_queue(
        &mut self,
        queue: &mut FirmwareQueues,
        queues: Arc<Mutex<FirmwareQueues>>,
        client: &mut g16_vm::AddressSpace,
        owner: Arc<Mutex<g16_vm::AddressSpace>>,
        params: &compute::Parameters,
        timestamps: [u64; 4],
        dependencies: &[Option<Stamp>; 2],
    ) -> Result<Receipt> {
        let lane = queue.lanes.as_ref().ok_or(EIO)?[2];
        let priority = queue.priority;

        let mut params = *params;
        let scratch = client.prepare_compute(&mut params)?;
        let space = client.roots();
        if self.render_failed {
            return Err(EIO);
        }
        if !(2..=3).contains(&priority) {
            return Err(EINVAL);
        }
        let ordinal = queue.compute_count;
        let (work, aliased) = self.allocate_work(false)?;
        let alias = 0x71_0000_0000 + (work - WORK_BASE);
        if alias + 0x4000 > 0x72_0000_0000 {
            return Err(ENOSPC);
        }
        // Firmware save state is independent of shader resource tables.
        // The upper half of the zeroed Work page is GPU-addressable storage.
        params.save_area = alias + 0x2000;
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        // As for renders: reused slots skip aliases that already exist.
        if aliased.is_none() || aliased != Some(space.low) {
            let pa = fw.physical(work)?;
            if aliased.is_none() {
                self._address_space
                    .as_mut()
                    .ok_or(EIO)?
                    .map_work_alias(alias, pa)?;
            }
            client.map_work_alias(alias, pa)?;
        }
        fw.zero_live(work + 0x2000, 0x2000)?;
        let mut a = compute::Addresses::publication(self.compute_publication, ordinal, work, alias);
        a.queue = lane.queue;
        a.driver_stamp = work + 0x4000;
        a.firmware_stamp = lane.firmware_stamp();
        a.event = queue.event(2)?;
        self._address_space.as_ref().ok_or(EIO)?.sync();
        fw.sync();
        client.sync();
        self.invalidate_client(self._address_space.as_ref().ok_or(EIO)?);
        self.invalidate_client(client);
        a.context = self.acquire_context(space, 1)?;
        a.identity = (u64::from(a.context) << 32) | (a.identity & 0xffff_ffff);
        a.register_identity = (u64::from(a.context) << 32) | 1;
        self.render_failed = true;
        let own = queue.lanes.map_or([0; 3], |l| l.map(|lane| lane.firmware_stamp()));
        self.note_dependency_events(dependencies, own);
        a.cdm_entry = Some(alias + compute::ENTRY_OFFSET);
        let work = &mut self.encodings.compute;
        work.fill(0);
        if params.work(&a, work).is_none() {
            pr_err!("G16: compute Work encoding failed for ioctl-validated parameters\n");
            return Err(EIO);
        }
        // Timestamp's user destinations must lie in the validated special-
        // object aperture. Firmware writes nanoseconds directly, as on M1/M2.
        // Firmware consumes this pointer pair, so it lives in the Work's
        // uncached shared page: in the cached private page, stale firmware
        // cache lines lost timestamps once the Work was reused.
        let user_timestamps = a.work + 0x4040;
        let count = self.compute_publication.wrapping_add(1);
        let count32 = count as u32;

        let first = lane.head;
        let head = (first + 1 + dependencies.iter().flatten().count() as u32) % 0x500;
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        fw.write_live(compute::SHARED_STATE, &count32.to_le_bytes())?;
        fw.write_live(a.work, &self.encodings.compute)?;
        fw.write_live(a.work + compute::ENTRY_OFFSET, &compute::entry(params.cdm))?;
        let mut microsequence = params.microsequence(&a);
        for base in [0x1bc, 0x20c] {
            microsequence[base + 0x24..base + 0x2c].copy_from_slice(&user_timestamps.to_le_bytes());
        }
        fw.write_live(user_timestamps, &timestamps[0].to_le_bytes())?;
        fw.write_live(user_timestamps + 8, &timestamps[1].to_le_bytes())?;
        fw.write_live(a.microsequence, &microsequence)?;
        let notifier = a.notifier(0);
        // Retained notifier list links are firmware-owned even after completion.
        fw.write_live(a.notifier, &notifier[..0x14])?;
        fw.write_live(a.notifier + 0x24, &notifier[0x24..0x28])?;
        fw.write_live(a.threshold, &1u32.to_le_bytes())?;
        fw.write_live(a.driver_stamp, &g16_stamp::previous(a.stamp).to_le_bytes())?;
        let mut pos = first;
        for (index, dep) in dependencies.iter().flatten().enumerate() {
            // Between the threshold/timestamps and the CDM entry trampoline.
            let address = a.work + 0x1900 + index as u64 * 0x40;
            fw.write_live(
                address,
                &g16_fw::queue::dependency(
                    dep.address,
                    dep.value,
                    dep.event,
                    a.stamp,
                    a.identity as u32,
                    1,
                ),
            )?;
            fw.write_live(lane.ring + u64::from(pos) * 8, &address.to_le_bytes())?;
            pos = (pos + 1) % 0x500;
        }
        fw.write_live(lane.ring + u64::from(pos) * 8, &a.work.to_le_bytes())?;
        let channel = priority as usize * 3 + 2;
        let mut pending_lane = lane;
        pending_lane.head = head;
        let epoch =
            self.stage_queue(channel, pending_lane, a.event, count)?;
        self.compute_publication = count;
        queue.compute_count = ordinal.wrapping_add(1);
        let lane = &mut queue.lanes.as_mut().ok_or(EIO)?[2];
        lane.head = head;
        lane.new = false;
        let stamp = Stamp {
            address: a.firmware_stamp,
            value: a.stamp,
            event: a.event,
        };
        let pointers = lane.pointers;
        queue.last[1] = Some(stamp);
        for dep in dependencies.iter().flatten() {
            FirmwareQueues::note_wait(&mut queue.waited, 2, *dep);
        }
        queue.pending += 1;
        self.flights.push_within_capacity(
            Flight {
                id: a.work,
                owner: queues.clone(),
                context: a.context,
                stream: params.cdm,
                ordinal,
                tgid: params.tgid,
                comm: params.comm,
                started: Instant::now(),
                notifier_count: None,
                job_notifier: None,
                stamps_seen: None,
                blocked: 0,
                dependencies: *dependencies,
                stamps: [Some(stamp), None],
                driver_stamps: [
                    Some(Stamp {
                        address: a.driver_stamp,
                        ..stamp
                    }),
                    None,
                ],
                scratch,
                client: owner,
                queues: [(pointers, head), (0, 0)],
                lane_items: [dependencies.iter().flatten().count() as u32, 0],
                channels: [(channel, (epoch + 1) & 255), (usize::MAX, 0)],
                render: None,
            },
        )
        .map_err(|_| EIO)?;
        // Capacity was reserved with the flight in reserve_flight.
        let flight = self.flights.last().ok_or(EIO)?;
        self.ledger.add(flight)?;
        self.render_failed = false;
        self.batch_works += 1;
        self.trace_publication("compute", a.work)?;
        self.trace_dependencies("compute", a.work, a.context, dependencies)?;
        Ok(Receipt { id: a.work, stamp })
    }
}
