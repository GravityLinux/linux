// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Source-owned Max render pair lowering in the common scheduler/runtime.

use super::super::super::g17p_render::{self as command, program, wire as body};
use super::*;
const BASE: u64 = WORK_BASE + 0x40000;
const FLAGS: [u64; 7] = [
    PRIVATE,
    PRIVATE,
    PRIVATE,
    PRIVATE,
    0x008000000000044b,
    0x008000000000044b,
    SHARED,
];

impl State {
    fn render_storage(&mut self, memory: &mut Memory, vm: &mut Vm) -> StageResult<()> {
        let alias = 0x7000000000 + BASE - work::DESCRIPTOR_BASE_G16C;
        if self.render_work_pages.is_none() {
            let (staged, pages) = stage_work_pages(memory, vm, BASE, FLAGS, &[0, 2])
                .map_err(StageError::allocation)?;
            initial_allocation_checkpoint(6, &mut self.initial_allocation_faulted, &staged)?;
            memory.absorb(staged).map_err(StageError::allocation)?;
            install_work_pages(memory, vm, BASE, FLAGS, &[0, 2], pages)?;
            self.render_work_pages = Some(pages);
        }
        let pages = self.render_work_pages.ok_or(EIO)?;
        for (i, flags) in FLAGS.into_iter().enumerate() {
            if vm.pte(memory, 2, BASE + i as u64 * PAGE)? != pages[i] | flags {
                return Err(EIO.into());
            }
        }
        for i in [0, 2] {
            if vm.pte(memory, 0, alias + i as u64 * PAGE)? != pages[i] | GPU_READ {
                return Err(EIO.into());
            }
        }
        for pa in pages {
            memory.zero(pa, PAGE as usize)?;
            memory.clean(pa, PAGE as usize)?;
        }
        Ok(())
    }
    fn stage_render(
        &mut self,
        session: &mut Session,
        key: Key,
        client: &mut Option<compute::Client>,
        input: &command::Parameters,
        priority: u32,
        kernel_start: u64,
        kernel_end: u64,
    ) -> StageResult<Option<Arc<RenderReceipt>>> {
        if !self.idle() {
            return Ok(None);
        }
        self.compute_mode=false;
        self.compute_bound=[false;2];
        self.release_requested(session)?;
        let qi = self.prepare_queue(session, key, client, kernel_start, kernel_end, priority, None)?;
        let oi = self.queues[qi].owner;
        // Vertex scratch uses the owned queue operand directory at the VM's
        // requested origin. Idle above includes every GPU and CPU root lease;
        // build_owner maps all 28 backing tranches before this admission.
        if self.owners[oi].directory != kernel_start {
            return Err(EIO.into());
        }
        if self.owners[oi].render.is_none() {
            // PB identities belong to the render cache, not an idle compute
            // owner's ASID. Detached resources have completed owned-root and
            // control retirement; all GPU/CPU leases are quiescent above.
            if let Some(from) = self
                .owners
                .iter()
                .position(|o| o.render.as_ref().is_some_and(|r| r.dormant()
                    && r.private_end.checked_sub(r.directory)
                        .and_then(|span| self.owners[oi].private_next.checked_add(span))
                        .is_some_and(|end| end <= self.owners[oi].private_limit)))
            {
                let slot = self.owners[from].render.as_ref().ok_or(EIO)?.parameter.slot;
                self.owners[oi].render = self.owners[from].render.take();
                kernel::dev_info!(session.peers[0].data.dev.as_ref(),
                    "G16C: dormant PB {} transferred context {} -> {} with no retained render root\n",
                    slot, self.owners[from].context, self.owners[oi].context);
            }
        }
        if self.owners[oi].render.is_none() {
            let slot = (0..15u32)
                .find(|slot| {
                    !self
                        .owners
                        .iter()
                        .any(|o| o.render.as_ref().is_some_and(|r| r.parameter.slot == *slot))
                })
                .ok_or(ENOSPC)?;
            let o = &mut self.owners[oi];
            let memory = session.memory.as_mut().ok_or(EIO)?;
            let vm = session.vm.as_mut().ok_or(EIO)?;
            if render::ResourcesG16C::private_end(o.private_next)? > o.private_limit {
                return Err(StageError::Rejected(ENOSPC));
            }
            let (staged, resource) = render::ResourcesG16C::prepare(
                memory,
                vm,
                &mut o.client.as_mut().ok_or(EIO)?.root,
                slot,
                slot,
                o.private_next,
                o.private_limit,
            ).map_err(StageError::allocation)?;
            initial_allocation_checkpoint(4, &mut self.initial_allocation_faulted, &staged)?;
            memory.absorb(staged).map_err(StageError::allocation)?;
            resource.install(memory, vm, &mut o.client.as_mut().ok_or(EIO)?.root)?;
            if resource.private_end > o.private_limit {
                return Err(ENOSPC.into());
            }
            o.private_next = resource.private_end;
            o.render = Some(resource);
            kernel::dev_info!(
                session.peers[0].data.dev.as_ref(),
                "G16C: PB {} allocated for context {}\n",
                slot,
                o.context
            );
        }
        if !self.owners[oi].render.as_ref().ok_or(EIO)?.attached() {
            let o = &mut self.owners[oi];
            let r = o.render.as_mut().ok_or(EIO)?;
            let span = r.private_end.checked_sub(r.directory).ok_or(EIO)?;
            if o.private_next.checked_add(span).ok_or(EOVERFLOW)? > o.private_limit {
                return Err(StageError::Rejected(ENOSPC));
            }
            r.rebind(
                session.memory.as_mut().ok_or(EIO)?,
                session.vm.as_mut().ok_or(EIO)?,
                &mut o.client.as_mut().ok_or(EIO)?.root,
                o.private_next,
                o.private_limit,
            )?;
            o.private_next = r.private_end;
        }
        let n = self.render_ordinal;
        let first_event = if self.compute_first == Some(true) && Self::compute_slots()?==2 {2} else {u8::from(self.compute_first == Some(true))};
        if n >= 0x3fffffff {
            return Err(EOVERFLOW.into());
        }
        let local = self.queues[qi].render_count;
        let mut p = *input;
        p.context_base = 0x1000000000;
        p.deflake_1 = 0x1000068da0;
        p.deflake_2 = 0x1000068120;
        p.deflake_3 = 0x1000068000;
        p.ta_status = 0x1000078000;
        p.fragment_status = 0x1000490000;
        p.aux_fb = 0x10004d0000;
        // Owned kind-6 requests are serviced by the common runtime before polling retirement.
        p.reactive_tvb_growth = true;
        if *crate::module_parameters::g16c_parameter_oom_test.value()==n {
            // Explicit one-shot qualification matches the source OOM7 gate.
            p.reactive_tvb_growth=false;
            p.fragment_sync_grow=Some(true);
            kernel::dev_info!(session.peers[0].data.dev.as_ref(),"G16C: test-only parameter OOM policy at render ordinal={} TA892=0 FG2100=1\n",n);
        }
        p.aux_fb_flags = 0xc000 | (p.aux_fb_flags & (1 << 18));
        let (_, _, tpc) = p.scratch_layout().map_err(|_| EINVAL)?;
        let tile = p.scratch_layout().map_err(|_| EINVAL)?.0;
        let tpc_size = tpc.checked_mul(4).ok_or(EOVERFLOW)?;
        let o = &mut self.owners[oi];
        let r = o.render.as_mut().ok_or(EIO)?;
        let geometry = r.prepare_geometry(
            &mut o.client.as_mut().ok_or(EIO)?.root,
            [tile, tpc_size, 0x200 + if p.layers > 1 { 0x100 } else { 0 }],
        ).map_err(StageError::allocation)?;
        let geo = if let Some((staged, prepared)) = geometry {
            initial_allocation_checkpoint(7, &mut self.initial_allocation_faulted, &staged)?;
            session.memory.as_mut().ok_or(EIO)?.absorb(staged).map_err(StageError::allocation)?;
            r.install_geometry(&mut o.client.as_mut().ok_or(EIO)?.root, prepared)?
        } else { r.geometry_addresses() };
        p.tilemap = geo[0];
        p.tpc = geo[1];
        p.heapmeta = geo[2] + if p.layers > 1 { 0x100 } else { 0 };
        let scene_slot = r.sequence % 79 + 1;
        let context = u64::from(o.context);
        let ps = program::ProgramStateG16C {
            layermeta: geo[2],
            cycle: 0x420000 + u64::from(scene_slot % 36) * 0x70,
            scene_slot: u64::from(scene_slot),
            tiling_admission: u64::from(r.parameter.slot),
            fragment_admission: u64::from(r.parameter.slot),
            tiling_gate: 0x7f00400020,
            fragment_gate: 0xa000400020,
            tiling_lifecycle: 0xbc010000e1 + u64::from(r.sequence) * 2,
            fragment_lifecycle: 0xbc010000e0 + u64::from(r.sequence) * 2,
            tiling_work_stamp: context << 8 | u64::from(r.sequence & 255),
            fragment_work_stamp: context << 8 | u64::from(r.sequence & 255),
            stencil_clear_flags: 0x100,
            partial_load_depth_flags: Some(p.depth_flags | ((p.depth_flags & (3 << 18)) >> 4)),
            partial_resume_depth_flags: Some(p.depth_flags | ((p.depth_flags & (3 << 18)) >> 4)),
            ..Default::default()
        };
        let parameters = program::descriptor_parameters(&p, &ps, tpc_size).map_err(|_| EINVAL)?;
        let receipt = RenderReceipt::new(n).map_err(StageError::allocation)?;
        let addresses = [
            p.ta_user_timestamp_start,
            p.ta_user_timestamp_end,
            p.fragment_user_timestamp_start,
            p.fragment_user_timestamp_end,
        ];
        let completion = Completion {
            client: o.client.as_ref().ok_or(EIO)?.lease().map_err(StageError::allocation)?,
            stamps: session
                .timestamps
                .as_ref()
                .ok_or(EIO)?
                .cache_seed(&addresses).map_err(StageError::allocation)?,
            fence: receipt.fence.clone(),
            grid: first_event + 1,
            error: None,
        };
        let mut leases = KVec::new();
        leases.push(o.client.as_ref().ok_or(EIO)?.lease().map_err(StageError::allocation)?, GFP_KERNEL).map_err(StageError::allocation)?;
        o.client.as_ref().ok_or(EIO)?.cache(false).map_err(StageError::allocation)?;
        completion.stamps.run(false).map_err(StageError::allocation)?;
        let slot = r.parameter.slot;
        let support = r.support;
        let manager = r.manager;
        let scene = r.scene + u64::from(r.sequence % 79) * 0x80;
        let empty = r.empty_scene;
        let auxiliary = r.auxiliary;
        let resource_count = (r.sequence_base.checked_add(r.sequence).ok_or(EOVERFLOW)? + 1)
            .checked_mul(2)
            .ok_or(EOVERFLOW)?;
        self.pending.reserve(1, GFP_KERNEL).map_err(StageError::allocation)?;
        self.render_storage(
            session.memory.as_mut().ok_or(EIO)?,
            session.vm.as_mut().ok_or(EIO)?,
        )?;
        let stamp = n.wrapping_add(1).wrapping_shl(8);
        let mut drivers = KVec::new();
        for stage in 0..2 {
            drivers.push(
                Stamp {
                    address: BASE + 0x18000 + stage * 0x40,
                    value: stamp,
                },
                GFP_KERNEL,
            ).map_err(StageError::allocation)?;
        }
        let mut counters = KVec::new();
        counters.push(
            Stamp {
                address: BASE + 0x20c,
                value: 2,
            },
            GFP_KERNEL,
        ).map_err(StageError::allocation)?;
        let next_local = self.queues[qi].render_count.checked_add(1).ok_or(EOVERFLOW)?;
        let mut descriptor = bytes(body::FragmentDescriptorG16C::SIZE).map_err(StageError::allocation)?;
        flight_allocation_checkpoint(&mut self.initial_allocation_faulted)?;
        let memory = session.memory.as_mut().ok_or(EIO)?;
        let vm = session.vm.as_mut().ok_or(EIO)?;
        for stage in 0..2usize {
            let fragment = stage == 1;
            let lane = &self.queues[qi].storage.lanes[stage];
            let address = BASE + if fragment { 0x8000 + 0x1b40 } else { 0x2940 };
            let prelude = BASE + if fragment { 0xc000 } else { 0x4000 };
            let state = body::State {
                context: context as u32,
                pool: slot,
                event_control: BASE + 0x200,
                manager,
                scene,
                empty_scene: empty,
                alias: 0x7000000000 + address - work::DESCRIPTOR_BASE_G16C,
                shared_tail: BASE + 0x10000 + stage as u64 * PAGE - u64::from(n) * 0x40,
                driver_stamp: BASE + 0x18000 + stage as u64 * 0x40,
                firmware_stamp: lane.queue + 0x1040,
                support,
                counter: if n != local {
                    u64::from(local) * 2 + 1 - stage as u64
                } else {
                    u64::from(n) * 2 + 1 - stage as u64
                },
                stamp,
                event: u32::from(first_event) + stage as u32,
                uuid: 0x010000e1 + n * 2 - stage as u32,
                timestamp: BASE + 0x18100,
                timestamp_end: if fragment { None } else { Some(0) },
                user_timestamp_start: addresses[stage * 2],
                user_timestamp_end: addresses[stage * 2 + 1],
                fragment_event: u32::from(first_event) + 1,
                fragment_stamp: stamp,
                stage_flag: 0,
                field_93c: 0x305800001,
                field_2128: 0,
                field_21d8: 0x3be00,
                field_222c: 0,
                secondary_stamp: Some(if n != local {
                    (local + 1).wrapping_shl(8)
                } else {
                    stamp & 65535
                }),
                descriptor_ordinal: Some(if n != local {
                    local
                } else if fragment {
                    n
                } else {
                    0
                }),
                work_index: Some(if n != local {
                    if fragment {
                        local + 1
                    } else {
                        local + local / 2
                    }
                } else if fragment {
                    (n + 1) & 255
                } else {
                    n
                }),
                tail_flags: if n != local { 3 } else { 1 },
                tiling_submission_tag: ((n + 1) & 255) * 0x101,
                tiling_queue_ordinal: n,
            };
            let primary_count;
            if fragment {
                let primary = program::FragmentProgramG16C::new(&p, &ps).map_err(|_| EINVAL)?;
                let store = program::partial_store(&p, &ps).map_err(|_| EINVAL)?;
                let resume = program::partial_resume(&p, &ps).map_err(|_| EINVAL)?;
                let load = program::partial_load(&p, &ps).map_err(|_| EINVAL)?;
                body::FragmentDescriptorG16C::write(
                    &mut descriptor,
                    &parameters,
                    &state,
                    &body::Programs {
                        primary: primary.registers(),
                        partial_store: &store,
                        partial_resume: &resume,
                        partial_load: &load,
                    },
                    n,
                    u32::from(first_event) + 1,
                )
                .map_err(|_| EINVAL)?;
                primary_count = primary.registers().len() as u16 - 4;
                vm.write(memory, 2, address, &descriptor)?;
            } else {
                let primary = program::TilingProgramG16C::new(&p, &ps).map_err(|_| EINVAL)?;
                body::TilingDescriptorG16C::write(
                    &mut descriptor[..body::TilingDescriptorG16C::SIZE],
                    &parameters,
                    &state,
                    primary.registers(),
                    n,
                    u32::from(first_event),
                )
                .map_err(|_| EINVAL)?;
                primary_count = primary.registers().len() as u16 - 2;
                vm.write(
                    memory,
                    2,
                    address,
                    &descriptor[..body::TilingDescriptorG16C::SIZE],
                )?;
            }
            let arrays = if fragment {
                [
                    (0xa0, primary_count),
                    (0x7c0, 16),
                    (0xee0, 23),
                    (0x1600, 10),
                ]
            } else {
                [(0x60, primary_count), (0, 0), (0, 0), (0, 0)]
            };
            let record = work::WorkRecordG16C::for_render(&work::RenderRecordParametersG16C {
                descriptor: address,
                queue: lane.queue,
                grid: first_event + stage as u8,
                item: n,
                fragment,
                timeout: 0xfa0,
                ready: true,
                arrays: if fragment { &arrays } else { &arrays[..1] },
                control: Some(if fragment { 0x1800 } else { 0xc00 }),
                control_low: None,
                field_30: None,
                field_38: 0,
            })
            .map_err(|_| EINVAL)?;
            vm.write(
                memory,
                2,
                lane.scratch_high + u64::from((n + 1) & 255) * 0x200,
                record.as_bytes(),
            )?;
            let wrapper = work::WorkWrapperG16C::for_render(&work::RenderWrapperParameters {
                common: work::WrapperParameters {
                    scratch_low: lane.scratch_low,
                    scratch_high: lane.scratch_high,
                    support,
                    queue_control: self.queues[qi].storage.context,
                    grid: u16::from(first_event) + stage as u16,
                    item: 0,
                    ordinal: n as u16,
                    context: context as u16,
                    support_id: slot as u16,
                    uuid: self.queues[qi].storage.uuid,
                    scheduler: lane.priority as u16,
                    scheduler_hint: 2,
                    first: n == 0,
                    field_52: if n == local { u16::from(n == 0) } else { 1 },
                    field_5e: lane.priority as u16,
                    field_62: if n == local { u16::from(n == 0) } else { 1 },
                },
                fragment,
                buffer_manager: manager,
                lifecycle: n as u16,
                namespace: slot as u16,
                item: n,
                item_control: if n == local {
                    ((n & 255) << 8) as u16
                } else {
                    0
                },
            })
            .map_err(|_| EINVAL)?;
            vm.write(memory, 2, prelude, wrapper.as_bytes())?;
            let mut end = [0u8; 0x40];
            end[..4].copy_from_slice(&16u32.to_le_bytes());
            end[4..8].copy_from_slice(
                &(((n + 1) & 65535) << 16 | (u32::from(first_event) + stage as u32)).to_le_bytes(),
            );
            end[8..16]
                .copy_from_slice(&(0x00fe000000000000u64 | (u64::from(n + 1) >> 16)).to_le_bytes());
            vm.write(memory, 2, prelude + 0xc0, &end)?;
            vm.write(
                memory,
                2,
                state.driver_stamp,
                &stamp.wrapping_sub(0x100).to_le_bytes(),
            )?;
        }
        let mut event = [0u8; 0x28];
        event[..8].copy_from_slice(&(BASE + 0x18200).to_le_bytes());
        event[0x10..0x14].copy_from_slice(&0x50u32.to_le_bytes());
        vm.write(memory, 2, BASE + 0x200, &event)?;
        vm.write(memory, 2, BASE + 0x18200, &2u32.to_le_bytes())?;
        vm.write(memory, 2, auxiliary, &resource_count.to_le_bytes())?;
        let [ta, fr, _] = &mut self.queues[qi].storage.lanes;
        let prepared = session.prepare_g16c_admission(
            &mut [
                Request {
                    lane: ta,
                    works: &[BASE + 0x4000, BASE + 0x40c0],
                    event: first_event,
                    tag: 0,
                },
                Request {
                    lane: fr,
                    works: &[BASE + 0xc000, BASE + 0xc0c0],
                    event: first_event + 1,
                    tag: 0,
                },
            ],
            Requirements {
                firmware: KVec::new(),
                driver: drivers,
                notifiers: KVec::new(),
                drains: KVec::new(),
                counters,
            },
            leases,
        )?;
        self.activate(session, oi)?;
        if !self.render_status {
            boot::activate_render_status(
                session.memory.as_mut().ok_or(EIO)?,
                session.vm.as_ref().ok_or(EIO)?,
            )?;
            self.render_status = true;
        }
        let o = &mut self.owners[oi];
        o.render.as_mut().ok_or(EIO)?.admit(
            session,
            &o.client.as_ref().ok_or(EIO)?.root,
            o.context,
            o.high_root,
            0,
        )?;
        session.configure_g16c_engines(
            &BindingG16C::render(
                first_event,
                self.queues[qi].storage.lanes[0].scratch_low,
                self.queues[qi].storage.lanes[1].scratch_low,
            )
            .ok_or(EINVAL)?, false, true,
        )?;
        let [ta, fr, _] = &mut self.queues[qi].storage.lanes;
        let id = session.publish_g16c_submission(&mut [
            Request { lane: ta, works: &[BASE + 0x4000, BASE + 0x40c0], event: first_event, tag: 0 },
            Request { lane: fr, works: &[BASE + 0xc000, BASE + 0xc0c0], event: first_event + 1, tag: 0 },
        ], prepared)?;
        self.compute_first.get_or_insert(false);
        receipt.publication_ready.store(true, Ordering::Release);
        self.pending.push(
            Pending {
                id,
                receipt: ReceiptG16C::Render(receipt.clone()),
                render_owner: Some(oi),
                growth_tokens: KVec::new(),
                render_work: Some(parameter_error::RenderOwnerG16C {
                    context:u32::from(self.owners[oi].context),slot,event:u32::from(first_event)+1,stamp,
                    descriptor:BASE+0x9b40,manager,queue:self.queues[qi].storage.lanes[1].queue,
                    driver_stamps:[BASE+0x18000,BASE+0x18040],
                }),
                parameter_error: None,
                compute_slot: None,
                completion,
                started: kernel::time::Instant::now(),
            },
            GFP_KERNEL,
        )?;
        self.queues[qi].render_count = next_local;
        self.owners[oi].render.as_mut().ok_or(EIO)?.sequence += 1;
        self.render_ordinal += 1;
        Ok(Some(receipt))
    }
}
impl Session {
    pub(crate) fn stage_g16c_render(
        &mut self,
        key: Key,
        client: &mut Option<compute::Client>,
        p: &command::Parameters,
        priority: u32,
        kernel_start: u64,
        kernel_end: u64,
    ) -> Result<Option<Arc<RenderReceipt>>> {
        if self.phase != Phase::Running
            || self
                .peers
                .iter()
                .any(|p| p.data.crashed.load(Ordering::Acquire))
        {
            return Err(EIO);
        }
        let mut state = self.g16c_compute.take().ok_or(EIO)?;
        let result = state.stage_render(self, key, client, p, priority, kernel_start, kernel_end);
        self.g16c_compute = Some(state);
        self.finish_g16c_admission(result)
    }
}
