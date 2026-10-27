// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! Render submission: dependency resolution and item elision, then building
//! and publishing a render's TA and 3D Works.

use super::*;

impl Bootstrap {
    /// Count a firmware dependency item and decide whether to drop it as
    /// redundant: when this lane's previous Work already waited on that stamp
    /// or a later one (Works on a lane start in order), or when the stamp has
    /// already been reached at publish. An item on the same lane's earlier
    /// Work is only counted: lane order implies it, but it is kept.
    pub(super) fn elide_item(
        &self,
        queue: &FirmwareQueues,
        lane: usize,
        dep: &Stamp,
        same_lane: bool,
        tally: &mut DepTally,
    ) -> Result<bool> {
        use crate::g16_timeline as t;
        tally.items += 1;
        let drop = if same_lane {
            tally.same_lane += 1;
            false
        } else if queue.waited[lane]
            .iter()
            .flatten()
            .any(|w| w.address == dep.address && g16_stamp::reached(w.value, dep.value))
        {
            tally.repeat += 1;
            true
        } else {
            let before = t::reads();
            // The last retire scan's value: stamps only advance, so a value
            // that already reached the target still does.
            let now = self.last_scan.peek(dep.address);
            t::attribute(&t::P_RD_ELIDE, before);
            let reached = now.is_some_and(|v| g16_stamp::reached(v, dep.value));
            if reached {
                tally.reached += 1;
            }
            reached
        };
        if drop {
            tally.dropped += 1;
        }
        Ok(drop)
    }

    /// The lane a render's dependency item sits on: slot 0 (render) on the
    /// fragment lane with a fragment barrier, else the tiling lane.
    pub(super) fn render_item_lane(params: &render::Parameters, slot: usize) -> usize {
        usize::from(slot == 0 && params.vdm_barrier_fragment)
    }

    pub(super) fn resolve_render_dependencies(
        &self,
        queue: &mut FirmwareQueues,
        params: &render::Parameters,
        dependencies: &[Option<Stamp>; 2],
        tally: &mut DepTally,
    ) -> Result<[Option<Stamp>; 2]> {
        let own_fragment = queue.lanes.as_ref().map(|lanes| lanes[1].firmware_stamp());
        let mut kept = *dependencies;
        for (slot, dep) in dependencies.iter().enumerate() {
            let Some(dep) = dep else { continue };
            let lane = Self::render_item_lane(params, slot);
            let same_lane = lane == 1 && own_fragment == Some(dep.address);
            if self.elide_item(queue, lane, dep, same_lane, tally)? {
                kept[slot] = None;
            }
        }
        Ok(kept)

    }

    pub(crate) fn submit_render(
        &mut self,
        queues: Arc<Mutex<FirmwareQueues>>,
        owner: Arc<Mutex<g16_vm::AddressSpace>>,
        params: &render::Parameters,
        support: &Support,
        timestamps: [u64; 4],
        dependencies: &[Option<Stamp>; 2],
        job: u64,
    ) -> Result<Receipt> {
        let _t = crate::g16_timeline::Timer::start(&crate::g16_timeline::P_SUBMIT_RENDER);
        if self.render_failed {
            return Err(EIO);
        }
        let mut mesh_params;
        // The VM's latched mode, not the current one: a VM created native keeps
        // decoding its streams after a runtime switch to 0.
        let mesh_mode = support.mode;
        // Peek at the first word without waiting for producers: an ordinary
        // header publishes as at native_mesh=0, with no stall behind earlier
        // work. Userspace writes a native header on the CPU before SUBMIT, and
        // no GPU job rewrites a render's first word (Honeykrisp audit,
        // 2026-10-24), so a stale read can only see a native header that is
        // gone by now: that case still waits and decodes, which is
        // authoritative. An unreadable first word also waits and then fails
        // in the decode, as before.
        let params = if mesh_mode.enabled()
            && owner.lock().command_word(params.encoder).map_or(true, g16_mesh::may_be_native)
        {
            mesh_params = *params;
            let params = &mut mesh_params;
            // These are the raw producer dependencies, before same-lane wait
            // elision. A firmware barrier alone cannot protect a CPU read.
            let fw = self._firmware_space.as_ref().ok_or(EIO)?;
            for dep in dependencies.iter().flatten() {
                if !g16_stamp::reached(fw.read_u32(dep.address)?, dep.value) {
                    crate::g16_timeline::DEFERRED_CPU_HOLD.fetch_add(1, Ordering::Relaxed);
                    return Err(EBUSY);
                }
            }
            let mut client = owner.lock();
            params.mesh = g16_mesh::decode(params.encoder, mesh_mode, |va| client.command_word(va).ok())
                .map_err(|error| match error {
                    g16_mesh::DecodeError::Unmapped => EFAULT,
                    g16_mesh::DecodeError::Unsupported => EOPNOTSUPP,
                })?;
            drop(client);
            if params.mesh.is_some() && (params.samples != 1 || params.layers != 1) {
                return Err(EOPNOTSUPP);
            }
            // A VM that first rendered with native_mesh=0 keeps its ordinary
            // support header, which cannot run a native stream.
            if params.mesh.is_some() && !support.native {
                return Err(EOPNOTSUPP);
            }
            // The VM already owns and advertises all 24 operand buffers.
            // This initialized pool serves both ordinary and native Work.
            &mesh_params
        } else {
            params
        };
        let mut queue = queues.lock();
        let mut tally = DepTally::default();
        let resolved = self.resolve_render_dependencies(&mut queue, params, dependencies, &mut tally)?;
        let dependencies = &resolved;
        let root = owner.lock().roots().low;
        let initbm = !self
            .tvb_pools
            .iter()
            .any(|p| p.root == root && p.initialized);
        self.reserve_flight(
            &queue,
            true,
            root,
            [
                1 + u32::from(initbm) + dependencies.iter().flatten().count() as u32,
                2,
                0,
            ],
        )?;
        self.prepare_queues(&mut queue)?;
        let result = self.submit_render_queue(
            &mut queue,
            queues.clone(),
            owner,
            params,
            support,
            timestamps,
            dependencies,
            job,
        );
        if queue.pending == 0 {
            queue.events = None;
        }
        if result.is_ok() {
            tally.publish();
        }
        result
    }

    pub(super) fn submit_render_queue(
        &mut self,
        queue: &mut FirmwareQueues,
        queues: Arc<Mutex<FirmwareQueues>>,
        owner: Arc<Mutex<g16_vm::AddressSpace>>,
        params: &render::Parameters,
        support: &Support,
        timestamps: [u64; 4],
        dependencies: &[Option<Stamp>; 2],
        job: u64,
    ) -> Result<Receipt> {
        let mut guard = owner.lock();
        let client = &mut *guard;
        // The SUBMIT ioctl created the pool (provision_tvb); this finds it.
        let index = self.tvb_pool(client, support)?;
        let scene = self.tvb_pools[index].reserve_scene().inspect_err(|_| {
            self.report_pressure(4, "scenes");
        })?;
        let mut pool = self.tvb_pools.swap_remove(index);
        // Out of `self` while publication also borrows it; always returned.
        let mut encodings = core::mem::take(&mut self.encodings);
        let result = self.submit_render_pool(
            client,
            params,
            support,
            queue,
            queues,
            &mut pool,
            &mut encodings,
            owner.clone(),
            scene,
            timestamps,
            dependencies,
            job,
        );
        self.encodings = encodings;
        if result.is_err() && !self.render_failed {
            pool.release_scene(scene);
        }
        // Back into the slot swap_remove freed: no allocation.
        self.tvb_pools.push_within_capacity(pool).map_err(|_| EIO)?;
        result
    }

    #[inline(never)]
    pub(super) fn submit_render_pool(
        &mut self,
        client: &mut g16_vm::AddressSpace,
        params: &render::Parameters,
        support: &Support,
        queue: &mut FirmwareQueues,
        queues: Arc<Mutex<FirmwareQueues>>,
        pool: &mut crate::g16_tvb::Tvb,
        encodings: &mut Encodings,
        owner: Arc<Mutex<g16_vm::AddressSpace>>,
        scene: u32,
        timestamps: [u64; 4],
        dependencies: &[Option<Stamp>; 2],
        job: u64,
    ) -> Result<Receipt> {
        if self.render_failed {
            return Err(EIO);
        }
        if !params.valid() || !(2..=3).contains(&queue.priority) {
            return Err(EINVAL);
        }
        let space = client.roots();
        let mut params = *params;
        params.tiling_admission = pool.addresses.buffer_manager_slot;
        params.fragment_admission = pool.addresses.buffer_manager_slot;
        // These accelerator pointers must select the same leased scene as
        // BufferThing. Reusing scene 1 here corrupts other in-flight renders.
        params.scene_slot = u64::from(scene);
        params.cycle = 0x240000 + u64::from(scene) * 0x30;
        let scratch = client.prepare_render(&mut params)?;
        let mut a = render::Addresses::bootstrap().publication(self.render_publication);
        pool.work_addresses(&mut a, scene);
        // Keep profiling storage private to the Work. Public timestamp BOs
        // are written by the firmware through its separate user destinations.
        let lanes = queue.lanes.as_ref().ok_or(EIO)?;
        a.tiling_queue = lanes[0].queue;
        a.fragment_queue = lanes[1].queue;
        a.tiling_event = u64::from(queue.event(0)?);
        a.fragment_event = u64::from(queue.event(1)?);
        a.support = support.address();
        a.render_shared_state = support.shared_state();
        a.tiling_status_page = params.ta_status & !0x3fff;
        a.fragment_status_page = params.fragment_status & !0x3fff;
        let queue_ordinal = queue.render_count;
        a.fragment_counter = queue_ordinal.wrapping_mul(2);
        a.tiling_counter = a.fragment_counter.wrapping_add(1);
        // Firmware's dependency graph requires consecutive stamp updates
        // within each lane, even when other queues publish between them.
        a.fragment_stamp = u64::from(g16_stamp::value(queue_ordinal, 0x100));
        a.tiling_stamp = u64::from(g16_stamp::value(queue_ordinal, 0xc00));
        let (va, aliased) = self.allocate_work(true)?;
        a.tiling_shared_tail = va + 0x10000;
        a.fragment_shared_tail = va + 0x14000;
        // The notifier pass stores driver stamps, then sends the IRQ after
        // a DSB. These destinations must be firmware-uncached: a DSB does
        // not clean a cached stamp, leaving Linux asleep on stale memory.
        // Each render has its own driver stamps and, with shared_notifier=0,
        // its own notifier and count. A notifier shared by the whole queue
        // with a cumulative count (2026-10-03 to 10-27) was announced only
        // once every Work published to it had completed, so a busy queue
        // waited for the safety poll; one per submission (below) is not.
        // The Work count lives with the stamps in the firmware-uncached page:
        // a shared notifier's count is raised while the firmware reads it.
        a.event_control = va + 0x200;
        a.event_count_array = va + 0x18080;
        a.tiling_driver_stamp = va + 0x18000;
        a.fragment_driver_stamp = va + 0x18040;
        // shared_notifier=1: the submission's renders share the notifier of
        // its first published render (the owner), as Metal shares one per
        // command buffer. Like Metal, each render raises the notifier's Work
        // count by its own two Works as it is published: the owner writes
        // the header with count 2, the others the new count and their driver
        // stamps. The count never runs ahead of what was published. A render
        // published after the owner has retired (a submission held back)
        // starts a fresh notifier rather than re-arming one that may have
        // drained, which Metal, publishing a command buffer at once, never does.
        let job_owner = if crate::g16_timeline::shared_notifier() {
            let owner = self
                .job_notifiers
                .iter()
                .find(|j| j.job == job && j.owner_root.is_none())
                .map(|j| j.owner);
            let owner_va = owner.unwrap_or(va);
            a.event_control = owner_va + 0x200;
            a.event_count_array = owner_va + 0x18080;
            Some((owner_va, owner.is_none()))
        } else {
            None
        };
        // What this render writes into the notifier: the full header (its
        // own notifier, or a submission's first) and the Work count.
        let notifier_write = match job_owner {
            None | Some((_, true)) => (true, 2u32),
            Some((owner_va, false)) => {
                let entry = self.job_notifiers.iter().find(|j| j.owner == owner_va).ok_or(EIO)?;
                (false, entry.works + 2)
            }
        };
        // Fail before publication: past it, a render must have its Flight.
        if matches!(job_owner, Some((_, true))) && self.job_notifiers.len() == self.job_notifiers.capacity() {
            return Err(EIO);
        }

        a.tiling_firmware_stamp = lanes[0].firmware_stamp();
        a.fragment_firmware_stamp = lanes[1].firmware_stamp();
        let alias = 0x72_0000_0000 + (va - WORK_BASE);
        if alias + 0x8000 > 0x73_0000_0000 {
            return Err(ENOSPC);
        }
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        // A reused slot keeps its device alias, and its client alias when it
        // returns to the same root: skip those page-table walks.
        let client_mapped = aliased == Some(space.low);
        for stage in 0..2u64 {
            let work_page = va + stage * 0x8000;
            let register_page = alias + stage * 0x4000;
            if aliased.is_none() || !client_mapped {
                let pa = fw.physical(work_page)?;
                if aliased.is_none() {
                    self._address_space.as_mut().ok_or(EIO)?.map_work_alias(register_page, pa)?;
                }
                if !client_mapped {
                    client.map_work_alias(register_page, pa)?;
                }
            }
            for view in [self._address_space.as_mut().ok_or(EIO)?, &mut *client] {
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
        self.invalidate_client(self._address_space.as_ref().ok_or(EIO)?);
        self.invalidate_client(client);
        // A live context lease keeps these roots fixed through retirement.
        a.context_id = u64::from(self.acquire_context(space, 9)?);
        a.tiling_user_timestamp_start = timestamps[0];
        a.tiling_user_timestamp_end = timestamps[1];
        a.fragment_user_timestamp_start = timestamps[2];
        a.fragment_user_timestamp_end = timestamps[3];
        // From here any publication failure quarantines the roots.
        self.render_failed = true;
        let (ta, frag) = (&mut encodings.tiling, &mut encodings.fragment);
        ta.fill(0);
        frag.fill(0);
        // The ioctl validated the parameters and every address here is the
        // kernel's own, so a failed encoding is a driver bug, not bad input.
        if params.tiling_work(&a, ta).is_none() || params.fragment_work(&a, frag).is_none() {
            pr_err!("G16: render Work encoding failed for ioctl-validated parameters\n");
            return Err(EIO);
        }
        // No per-Work start or tiling-end profiling stamps; fragment end
        // uses one location shared by the queue, separate from UAPI timestamps.
        ta[0x8c8..0x8d8].fill(0);
        frag[0xc10..0xc18].fill(0);
        frag[0xc18..0xc20].copy_from_slice(&(lanes[1].pointers + 0x2080).to_le_bytes());
        if crate::module_parameters::fw_trace.value() & 128 != 0
            && !CONFIG_DUMPED.swap(true, Ordering::Relaxed)
        {
            // Live firmware configuration, for comparison with macOS.
            let fw = self._firmware_space.as_ref().ok_or(EIO)?;
            for (base, size) in [
                (0xffff_fc20_0010_0000u64, 0x4000u64),
                (0xffff_fc20_0010_8000, 0x4000),
                (0xffff_fc20_000f_8000, 0x4000),
                (0xffff_fc20_c04d_8000, 0x20000),
                (0xffff_fc20_0003_e000, 0x2000),
            ] {
                for line in (base..base + size).step_by(32) {
                    let words = [
                        fw.read_u64(line)?,
                        fw.read_u64(line + 8)?,
                        fw.read_u64(line + 16)?,
                        fw.read_u64(line + 24)?,
                    ];
                    if words != [0; 4] {
                        pr_info!(
                            "G16: cfgdump {:#x}: {:016x} {:016x} {:016x} {:016x}\n",
                            line, words[0], words[1], words[2], words[3]
                        );
                    }
                }
            }
        }
        if crate::module_parameters::fw_trace.value() & 32 != 0 && queue_ordinal == 10 {
            pr_info!(
                "G16: work dump TA={:#x} 3D={:#x} queues={:#x}/{:#x} bm={:#x} thing={:#x} support={:#x} micro={:#x}/{:#x}\n",
                a.tiling_work, a.fragment_work, a.tiling_queue, a.fragment_queue,
                a.buffer_manager, a.buffer_thing, a.support,
                a.tiling_microsequence, a.fragment_microsequence
            );
            for (name, bytes) in [("TA", &ta[..]), ("3D", &frag[..])] {
                for (i, chunk) in bytes.chunks(32).enumerate() {
                    pr_info!("G16: work {} {:#06x}: {:02x?}\n", name, i * 32, chunk);
                }
            }
        }
        // TVB initialization and first publication of a firmware queue are
        // independent: several public queues can share one VM's TVB pool.
        let initbm = !pool.initialized;

        // The shared count covers individual Work items, not render pairs.
        // Firmware retires one tiling and one fragment item per submission.
        let submitted = support.submissions.load(Ordering::Relaxed);
        let next_submitted = submitted.wrapping_add(2);
        // Firmware observes the low word of this cumulative count.
        let use_count = next_submitted as u32;
        // From here an error conservatively quarantines the client root.
        self.render_failed = true;
        self._firmware_space
            .as_mut()
            .ok_or(EIO)?
            .write_live(a.render_shared_state, &use_count.to_le_bytes())?;
        pool.increment(self._firmware_space.as_mut().ok_or(EIO)?)?;
        let own = queue.lanes.map_or([0; 3], |l| l.map(|lane| lane.firmware_stamp()));
        self.note_dependency_events(dependencies, own);
        let (heads, epochs, lane_items) = self.publish_render(
            &a, &ta, &frag, queue, initbm, pool.blocks, dependencies,
            params.vdm_barrier_fragment, notifier_write,
        )?;
        pool.initialized = true;
        support.submissions.store(next_submitted, Ordering::Relaxed);
        self.render_publication = self.render_publication.wrapping_add(1);
        queue.render_count = queue_ordinal.wrapping_add(1);
        // A shared notifier's completed count (+0xc) reaches this render's
        // running total within the submission once both its Works are done.
        let notifier_count = match job_owner {
            None => None,
            Some((owner_va, first)) => {
                if first {
                    self.job_notifiers
                        .push_within_capacity(JobNotifier {
                            job,
                            owner: owner_va,
                            owner_root: None,
                            works: 0,
                            refs: 0,
                        })
                        .map_err(|_| EIO)?;
                }
                let entry = self.job_notifiers.iter_mut().find(|j| j.owner == owner_va).ok_or(EIO)?;
                entry.works += 2;
                entry.refs += 1;
                Some((a.event_control + 0xc, entry.works))
            }
        };
        let job_notifier = job_owner.map(|(owner_va, _)| owner_va);
        let lanes = queue.lanes.as_mut().ok_or(EIO)?;
        for stage in 0..2 {
            lanes[stage].head = heads[stage];
            lanes[stage].new = false;
        }
        let stamp = Stamp {
            address: a.fragment_firmware_stamp,
            value: a.fragment_stamp as u32,
            event: a.fragment_event as u32,
        };
        let receipt = Receipt { id: va, stamp };
        queue.last[0] = Some(stamp);
        for (slot, dep) in dependencies.iter().enumerate() {
            if let Some(dep) = dep {
                FirmwareQueues::note_wait(&mut queue.waited, Self::render_item_lane(&params, slot), *dep);
            }
        }
        queue.pending += 1;
        self.flights.push_within_capacity(
            Flight {
                id: va,
                owner: queues,
                context: a.context_id as u32,
                stream: params.encoder,
                ordinal: queue_ordinal,
                tgid: params.tgid,
                comm: params.comm,
                started: Instant::now(),
                notifier_count,
                job_notifier,
                stamps_seen: None,
                blocked: 0,
                dependencies: *dependencies,
                stamps: [
                    Some(Stamp {
                        address: a.tiling_firmware_stamp,
                        value: a.tiling_stamp as u32,
                        event: a.tiling_event as u32,
                    }),
                    Some(stamp),
                ],
                driver_stamps: [
                    Some(Stamp {
                        address: a.tiling_driver_stamp,
                        value: a.tiling_stamp as u32,
                        event: a.tiling_event as u32,
                    }),
                    Some(Stamp {
                        address: a.fragment_driver_stamp,
                        ..stamp
                    }),
                ],
                scratch,
                client: owner.clone(),
                queues: [(lanes[0].pointers, heads[0]), (lanes[1].pointers, heads[1])],
                lane_items,
                channels: [
                    (queue.priority as usize * 3, (epochs[0] + 1) & 255),
                    (queue.priority as usize * 3 + 1, (epochs[1] + 1) & 255),
                ],
                render: Some(RenderFlight {
                    addresses: a,
                    client: owner,
                    root: space.low,
                    events: RenderEvents::default(),
                    scene,
                }),
            },
        )
        .map_err(|_| EIO)?;
        // Capacity was reserved with the flight in reserve_flight.
        let flight = self.flights.last().ok_or(EIO)?;
        self.ledger.add(flight)?;
        self.render_failed = false;
        self.batch_works += 1;
        self.trace_publication("render", va)?;
        self.trace_dependencies("render", va, a.context_id as u32, dependencies)?;
        Ok(receipt)
    }

    pub(super) fn publish_render(
        &mut self,
        a: &render::Addresses,
        ta: &[u8],
        frag: &[u8],
        queue: &FirmwareQueues,
        initbm: bool,
        blocks: u32,
        dependencies: &[Option<Stamp>; 2],
        vdm_barrier_fragment: bool,
        notifier_write: (bool, u32),
    ) -> Result<([u32; 2], [u32; 2], [u32; 2])> {
        let priority = queue.priority;
        let lanes = queue.lanes.as_ref().ok_or(EIO)?;
        let publication = self.render_publication;
        let firsts = [lanes[0].head, lanes[1].head];
        // Only the render dependency can move to fragment. Compute output
        // may feed the vertex stage, and input fences were already consumed
        // by the scheduler before either lane was published.
        let stage_dependencies = if vdm_barrier_fragment {
            [[None, dependencies[1]], [dependencies[0], None]]
        } else {
            [*dependencies, [None, None]]
        };
        let heads = [
            (firsts[0] + 1 + u32::from(initbm)
                + stage_dependencies[0].iter().flatten().count() as u32)
                % 0x500,
            (firsts[1] + 2 + stage_dependencies[1].iter().flatten().count() as u32) % 0x500,
        ];
        // Every byte this render publishes, written as one batch: one region
        // lookup per destination region and one barrier pair, rather than a
        // lookup and two barriers for each of about twenty writes.
        let context = (a.context_id as u32).to_le_bytes();
        // The notifier header: the full header when this render owns the
        // notifier (its own, or its submission's first), else only the raised
        // Work count; then this render's driver stamps.
        let count_array = a.event_count_array.to_le_bytes();
        let fragment_stamp = (a.fragment_stamp as u32).to_le_bytes();
        let zero = 0u32.to_le_bytes();
        let flags = 0x50u32.to_le_bytes();
        let count = notifier_write.1.to_le_bytes();
        let tiling_previous = g16_stamp::previous(a.tiling_stamp as u32).to_le_bytes();
        let fragment_previous = g16_stamp::previous(a.fragment_stamp as u32).to_le_bytes();
        let own: [(u64, &[u8]); 8] = [
            (a.event_control, &count_array[..]),
            (a.event_control + 8, &fragment_stamp[..]),
            (a.event_control + 12, &zero[..]),
            (a.event_control + 0x10, &flags[..]),
            (a.event_control + 0x24, &context[..]),
            (a.event_count_array, &count[..]),
            (a.tiling_driver_stamp, &tiling_previous[..]),
            (a.fragment_driver_stamp, &fragment_previous[..]),
        ];
        let (own, header): (&[(u64, &[u8])], usize) = if notifier_write.0 {
            (&own[..], own.len())
        } else {
            (&own[5..], 3)
        };
        let preludes = [a.tiling_prelude(blocks), a.fragment_prelude()];
        let tiling_micro = a.tiling_microsequence();
        let fragment_micro = a.fragment_microsequence();
        // At most two dependency items and four ring entries per stage.
        let mut items = [[[0u8; 0x40]; 2]; 2];
        let mut entries = [[[0u8; 8]; 4]; 2];
        let mut item_counts = [0usize; 2];
        let mut entry_counts = [0usize; 2];
        let render_kind = crate::module_parameters::dependency_kind.value();
        for stage in 0..2 {
            let (prelude, work) = if stage == 0 {
                (a.tiling_prelude, a.tiling_work)
            } else {
                (a.fragment_prelude, a.fragment_work)
            };
            let (stamp, uuid) = if stage == 0 {
                (a.tiling_stamp, a.tiling_uuid)
            } else {
                (a.fragment_stamp, a.fragment_uuid)
            };
            // The prelude page leaves +0x40..+0xc0 free before its
            // microsequence. All barrier bytes share the Work's lifetime.
            let deps = stage_dependencies[stage]
                .iter()
                .enumerate()
                .filter_map(|(slot, dep)| dep.map(|d| (if slot == 0 { render_kind } else { 1 }, d)));
            for (index, (kind, dep)) in deps.enumerate() {
                items[stage][index] = g16_fw::queue::dependency(
                    dep.address, dep.value, dep.event, stamp as u32, uuid as u32, kind,
                );
                entries[stage][entry_counts[stage]] = (prelude + 0x40 + index as u64 * 0x40).to_le_bytes();
                item_counts[stage] += 1;
                entry_counts[stage] += 1;
            }
            if stage == 1 || initbm {
                entries[stage][entry_counts[stage]] = prelude.to_le_bytes();
                entry_counts[stage] += 1;
            }
            entries[stage][entry_counts[stage]] = work.to_le_bytes();
            entry_counts[stage] += 1;
        }
        // Write `n` of the batch, in order: the shared notifier's two words
        // (firmware counts completions at +0xc and writes the lanes' driver
        // stamps as they retire), then per stage the prelude, Work,
        // microsequence, dependency items and ring entries (contiguous except
        // where the ring wraps). Computed on demand: no list on the job
        // worker's stack, and nothing to allocate.
        let (ta, frag): (&[u8], &[u8]) = (ta, frag);
        let preludes = &preludes;
        let (tiling_micro, fragment_micro) = (&tiling_micro, &fragment_micro);
        let (items, entries) = (&items, &entries);
        let rings = [lanes[0].ring, lanes[1].ring];
        let per_stage = move |stage: usize| 3 + item_counts[stage] + entry_counts[stage];
        let count = header + per_stage(0) + per_stage(1);
        let write = move |n: usize| {
            if n < header {
                return own[n];
            }
            let n = n - header + 2;
            let (stage, k) = if n - 2 < per_stage(0) { (0, n - 2) } else { (1, n - 2 - per_stage(0)) };
            let (prelude, work, micro) = if stage == 0 {
                (a.tiling_prelude, a.tiling_work, a.tiling_microsequence)
            } else {
                (a.fragment_prelude, a.fragment_work, a.fragment_microsequence)
            };
            match k {
                0 => (prelude, &preludes[stage][..]),
                1 => (work, if stage == 0 { ta } else { frag }),
                2 => (micro, if stage == 0 { &tiling_micro[..] } else { &fragment_micro[..] }),
                k if k - 3 < item_counts[stage] => {
                    let index = k - 3;
                    (prelude + 0x40 + index as u64 * 0x40, &items[stage][index][..])
                }
                k => {
                    let index = k - 3 - item_counts[stage];
                    let pos = (firsts[stage] + index as u32) % 0x500;
                    (rings[stage] + u64::from(pos) * 8, &entries[stage][index][..])
                }
            }
        };
        self._firmware_space.as_mut().ok_or(EIO)?.write_batch(count, write)?;
        let mut epochs = [0; 2];
        for stage in [1usize, 0] {
            let mut lane = lanes[stage];
            lane.head = heads[stage];
            epochs[stage] = self.stage_queue(
                priority as usize * 3 + stage,
                lane,
                if stage == 0 {
                    a.tiling_event
                } else {
                    a.fragment_event
                } as u32,
                publication.wrapping_add(1),
            )?;
        }
        let lane_items = stage_dependencies.map(|d| d.iter().flatten().count() as u32);
        Ok((heads, epochs, lane_items))
    }
}
