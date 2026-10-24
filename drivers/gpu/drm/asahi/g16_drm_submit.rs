// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! SUBMIT: parsing and validating commands into render and compute
//! parameters, and queueing them as scheduler jobs.

use super::*;

impl File {
    pub(super) fn submit(device: &RegDevice, _reg: &RegData<'_>, data: &mut uapi::drm_asahi_submit, file: &DrmFile) -> Result<u32> {
        let device: &Device = device;
        if data.flags != 0
            || data.pad != 0
            || data.cmdbuf_size == 0
            || data.cmdbuf_size > 1024 * 1024
            || data.in_sync_count > 4096
            || data.out_sync_count > 4096
        {
            return Err(reject(format_args!("bad flags/pad, cmdbuf size {} or sync counts {}/{}", data.cmdbuf_size, data.in_sync_count, data.out_sync_count)));
        }
        let mut bytes = KVVec::new();
        UserSlice::new(
            UserPtr::from_addr(data.cmdbuf.try_into()?),
            data.cmdbuf_size as usize,
        )
        .reader()
        .read_all(&mut bytes, GFP_KERNEL)?;
        let mut commands = KVec::new();
        let mut barriers = KVec::new();
        let mut attachments = KVec::new();
        let mut offset = 0usize;
        let mut renders = 0u16;
        let mut computes = 0u16;
        while offset < bytes.len() {
            let mut reader = crate::util::Reader::new(&bytes[offset..]);
            let header: uapi::drm_asahi_cmd_header = reader.read()?;
            offset = offset.checked_add(8).ok_or_else(|| reject(format_args!("command header past the buffer")))?;
            let end = offset.checked_add(header.size as usize).ok_or_else(|| reject(format_args!("command size overflows")))?;
            let payload = bytes.get(offset..end).ok_or_else(|| reject(format_args!("command payload past the buffer ({} > {})", end, bytes.len())))?;
            offset = end;
            match u32::from(header.cmd_type) {
                uapi::drm_asahi_cmd_type_DRM_ASAHI_CMD_RENDER => {
                    if (header.vdm_barrier != 0xffff && header.vdm_barrier > renders)
                        || (header.cdm_barrier != 0xffff && header.cdm_barrier > computes)
                        || renders + computes == 64
                    {
                        return Err(reject(format_args!("render barrier {}/{} beyond {} renders/{} computes, or 64 commands", header.vdm_barrier, header.cdm_barrier, renders, computes)));
                    }
                    let command = crate::util::Reader::new(payload)
                        .read_up_to::<uapi::drm_asahi_cmd_render>(payload.len())?;
                    commands.push(Command::Render(command), GFP_KERNEL)?;
                    barriers.push([header.vdm_barrier, header.cdm_barrier], GFP_KERNEL)?;
                    renders += 1;
                }
                uapi::drm_asahi_cmd_type_DRM_ASAHI_CMD_COMPUTE => {
                    if (header.vdm_barrier != 0xffff && header.vdm_barrier > renders)
                        || (header.cdm_barrier != 0xffff && header.cdm_barrier > computes)
                        || renders + computes == 64
                    {
                        return Err(reject(format_args!("compute barrier {}/{} beyond {} renders/{} computes, or 64 commands", header.vdm_barrier, header.cdm_barrier, renders, computes)));
                    }
                    let command = crate::util::Reader::new(payload)
                        .read_up_to::<uapi::drm_asahi_cmd_compute>(payload.len())?;
                    commands.push(Command::Compute(command), GFP_KERNEL)?;
                    barriers.push([header.vdm_barrier, header.cdm_barrier], GFP_KERNEL)?;
                    computes += 1;
                }
                uapi::drm_asahi_cmd_type_DRM_ASAHI_SET_VERTEX_ATTACHMENTS
                | uapi::drm_asahi_cmd_type_DRM_ASAHI_SET_FRAGMENT_ATTACHMENTS
                | uapi::drm_asahi_cmd_type_DRM_ASAHI_SET_COMPUTE_ATTACHMENTS => {
                    if header.vdm_barrier != 0xffff
                        || header.cdm_barrier != 0xffff
                        || payload.len() % 24 != 0
                        || payload.len() / 24 > 16
                    {
                        return Err(reject(format_args!("attachment command with barriers, or {} bytes (not 24 x 1..16)", payload.len())));
                    }
                    let mut reader = crate::util::Reader::new(payload);
                    while !reader.is_empty() {
                        let attachment: uapi::drm_asahi_attachment = reader.read()?;
                        if attachment.flags != 0
                            || attachment.pad != 0
                            || attachment.pointer.checked_add(attachment.size).is_none()
                        {
                            return Err(reject(format_args!("attachment flags/pad nonzero or range overflows")));
                        }
                        attachments.push(attachment, GFP_KERNEL)?;
                    }
                }
                kind => return Err(unsupported(format_args!("command type {}", kind))),
            }
        }
        if renders + computes == 0 {
            return Err(reject(format_args!("no render or compute command")));
        }
        let count = data.in_sync_count + data.out_sync_count;
        let mut sync_bytes = KVec::new();
        UserSlice::new(
            UserPtr::from_addr(data.syncs.try_into()?),
            count as usize * 16,
        )
        .reader()
        .read_all(&mut sync_bytes, GFP_KERNEL)?;
        let mut outputs = KVec::new();
        let mut dependencies = KVec::new();
        for (index, bytes) in sync_bytes.chunks_exact(16).enumerate() {
            let kind = u32::from_ne_bytes(bytes[0..4].try_into().map_err(|_| EINVAL)?);
            let handle = u32::from_ne_bytes(bytes[4..8].try_into().map_err(|_| EINVAL)?);
            let point = u64::from_ne_bytes(bytes[8..16].try_into().map_err(|_| EINVAL)?);
            if kind > 1 || (kind == 0 && point != 0) {
                return Err(reject(format_args!("sync {} kind {} point {}", index, kind, point)));
            }
            let object = drm::syncobj::SyncObj::lookup_handle(file, handle)?;
            if index >= data.in_sync_count as usize {
                outputs.push(
                    OutputSync {
                        object,
                        point,
                        chain: if kind == 1 {
                            Some(dma_fence::FenceChain::new()?)
                        } else {
                            None
                        },
                    },
                    GFP_KERNEL,
                )?;
            } else {
                let fence = object.fence_get().ok_or_else(|| reject(format_args!("in-sync {} (handle {}) has no fence: waited before any signal was submitted", index, handle)))?;
                let fence = if kind == 1 {
                    fence.chain_find_seqno(point)?
                } else {
                    Some(fence)
                };
                if let Some(fence) = fence {
                    dependencies.push(fence, GFP_KERNEL)?;
                }
            }
        }
        let mut trace = Trace::default();
        if crate::module_parameters::timeline.value() != 0 {
            trace.ioctl = ticks();
            trace.queue = data.queue_id;
            trace.inputs = dependencies.len() as u32;
            trace.pending = dependencies
                .iter()
                // SAFETY: The vector holds a live reference to each fence.
                .filter(|f| !unsafe { bindings::dma_fence_is_signaled(f.raw()) })
                .count() as u32;
        }
        let inner = file.inner();
        let (mut admission, mut state) = loop {
            let admission = device.admission.lock();
            if device.failed.load(Ordering::Acquire) {
                return Err(EIO);
            }
            let mut state = inner.state.lock();
            let queue = state.queues.iter()
                .find(|q| q.id == data.queue_id).ok_or(ENOENT)?;
            let vm = state.vms.iter().find(|vm| vm.id == queue.vm).ok_or(ENOENT)?;
            if vm.draining == 0 {
                break (admission, state);
            }
            // Wait atomically with state dropped. Drop admission as well:
            // holding it would block submissions to unrelated VMs and other
            // files and teardown paths that need admission before state.
            drop(admission);
            // No submission state has been published yet; let a signalled
            // submitter leave without waiting for the drainer's whole budget.
            if inner.drained.wait_interruptible(&mut state) {
                return Err(ERESTARTSYS);
            }
            drop(state);
            // Reacquire in admission -> state order and re-find both IDs.
        };
        // Completed records no longer need lifetime bookkeeping.
        admission.retain(|f| {
            // SAFETY: The vector retains a live reference while queried.
            !unsafe { bindings::dma_fence_is_signaled(f.raw()) }
        });
        admission.reserve(1, GFP_KERNEL)?;
        let queue = state
            .queues
            .iter()
            .find(|q| q.id == data.queue_id)
            .ok_or(ENOENT)?;
        // Render and compute on even one public queue may complete out of
        // order. Give each submission its own timeline: a later independent
        // completion must never subsume an earlier dependency in DRM.
        let unique = dma_fence::FenceContexts::new(1, c_str!("asahi_m4"), FENCE_KEY)?
            .new_fence(0, Completion)?;
        // A queue that has failed work stays failed: refuse new work instead
        // of accepting jobs that can only complete with ECANCELED.
        if queue.firmware.lock().rejected {
            return Err(ECANCELED);
        }
        let vm_id = queue.vm;
        let firmware = queue.firmware.clone();
        let mut timestamps = KVec::new();
        let resolve = |stamp: &uapi::drm_asahi_timestamp| -> Result<u64> {
            if stamp.handle == 0 {
                return if stamp.offset == 0 {
                    Ok(0)
                } else {
                    Err(reject(format_args!("timestamp offset {} without a handle", stamp.offset)))
                };
            }
            let binding = state
                .timestamps
                .iter()
                .find(|t| t.id == stamp.handle)
                .ok_or(ENOENT)?;
            let offset = u64::from(stamp.offset);
            if offset & 7 != 0 || offset.checked_add(8).ok_or_else(|| reject(format_args!("timestamp offset overflows")))? > binding.size {
                return Err(reject(format_args!("timestamp offset {} unaligned or beyond {}", offset, binding.size)));
            }
            Ok(binding.address + offset)
        };
        for command in &commands {
            let resolved = match command {
                Command::Render(c) => [
                    resolve(&c.ts_vtx.start)?,
                    resolve(&c.ts_vtx.end)?,
                    resolve(&c.ts_frag.start)?,
                    resolve(&c.ts_frag.end)?,
                ],
                Command::Compute(c) => [resolve(&c.ts.start)?, resolve(&c.ts.end)?, 0, 0],
            };
            timestamps.push(resolved, GFP_KERNEL)?;
        }
        let mut timestamp_bindings = KVec::new();
        for binding in &state.timestamps {
            timestamp_bindings.push(binding.clone(), GFP_KERNEL)?;
        }
        let index = state.vms.iter().position(|v| v.id == vm_id).ok_or(ENOENT)?;
        let vm = &mut state.vms[index];
        if vm.render_broken {
            return Err(EIO);
        }
        for attachment in &attachments {
            vm.cover(attachment.pointer, attachment.size, 4)?;
        }
        let mut parameters = KVec::new();
        for command in &commands {
            parameters.push(
                match command {
                    Command::Render(c) => Parameters::Render(vm.render_parameters(c)?),
                    Command::Compute(c) => Parameters::Compute(vm.compute_parameters(c)?),
                },
                GFP_KERNEL,
            )?;
        }
        let needs_mapping =
            (renders != 0 && !vm.render_ready) || (computes != 0 && !vm.compute_ready);
        if needs_mapping {
            // These reserved namespaces are initialized once before this
            // VM's first command on the corresponding engine.
            let mut runtime = device.runtime.lock();
            if runtime.render_failed() {
                return Err(EIO);
            }
            if renders != 0 && !vm.render_ready {
                let pool =
                    render::OperandPool::new(vm.kernel.start, vm.kernel.end).ok_or_else(|| reject(format_args!("VM kernel VA {:#x}..{:#x} too small for the render operand pool", vm.kernel.start, vm.kernel.end)))?;
                // Acquire scarce support and its Arc before editing this
                // root. ENOSPC/ENOMEM here must not quarantine a healthy VM.
                let support = Arc::new(
                    runtime.acquire_render_support(&pool, vm.native_mode)?,
                    GFP_KERNEL,
                )?;
                if let Err(error) = vm.space.lock()
                    .alloc_render_private(vm.kernel.start, vm.kernel.end)
                {
                    // The unpublished graph owns every partial allocation
                    // and resumes on ENOMEM. Structural/PTE inconsistencies
                    // still quarantine the VM; do not mask those failures.
                    vm.render_broken = error != ENOMEM;
                    return Err(error);
                }
                vm.render_support = Some(support);
                vm.render_ready = true;
            }
            if computes != 0 && !vm.compute_ready {
                vm.render_broken = true;
                runtime.map_compute_work(&mut vm.compute_space.lock(), vm.kernel.clone())?;
                vm.compute_ready = true;
                vm.render_broken = false;
            }
        }
        // Provision what this queue's or VM root's first publication would
        // otherwise allocate in the job worker, so a failure returns here with
        // nothing queued instead of failing the queue later.
        let tvb = renders != 0 && !vm.tvb_ready;
        if tvb || !firmware.lock().provisioned() {
            let mut runtime = device.runtime.lock();
            if runtime.render_failed() {
                return Err(EIO);
            }
            runtime.provision_queue(&mut firmware.lock())?;
            if tvb {
                let support = vm.render_support.as_deref().ok_or(EIO)?;
                runtime.provision_tvb(&mut vm.space.lock(), support)?;
                vm.tvb_ready = true;
            }
        }
        // Every submission retains the VM's mappings until it completes.
        // Share one copy until the mappings change: CS2-sized address spaces
        // hold thousands of mappings and submit hundreds of times a frame.
        let mappings = match &vm.snapshot {
            Some(snapshot) => snapshot.clone(),
            None => {
                let mut copy = KVVec::with_capacity(vm.mappings.len(), GFP_KERNEL)?;
                for mapping in &vm.mappings {
                    copy.push(mapping.clone(), GFP_KERNEL)?;
                }
                let snapshot = Arc::new(copy, GFP_KERNEL)?;
                vm.snapshot = Some(snapshot.clone());
                snapshot
            }
        };
        let bindings = Bindings {
            _mappings: mappings,
            _timestamps: timestamp_bindings,
        };
        vm.pending.retain(|f| {
            // SAFETY: Each entry owns its live fence reference.
            !unsafe { bindings::dma_fence_is_signaled(f.raw()) }
        });
        vm.pending.reserve(1, GFP_KERNEL)?;
        let completion = dma_fence::Fence::from_fence(&unique);
        let worker_completion = completion.clone();
        let execution = Arc::pin_init(
            try_pin_init!(Execution {
                id: NEXT_EXECUTION.fetch_add(1, Ordering::Relaxed), firmware, barriers,
                device: device.into(), _owner: inner.state.clone(), reaper: inner.reaper.clone(),
                render_space: vm.space.clone(), space: vm.compute_space.clone(), _bindings: bindings,
                support: vm.render_support.clone(),
                parameters, timestamps, completion: worker_completion,
                started: AtomicBool::new(false), trace, work <- new_work!("m4-execution"),
            }),
            GFP_KERNEL,
        )?;
        state.vms[index]
            .pending
            .push(completion.clone(), GFP_KERNEL)?;
        let queue = state
            .queues
            .iter_mut()
            .find(|q| q.id == data.queue_id)
            .ok_or(ENOENT)?;
        let mut history = [KVec::new(), KVec::new()];
        // One entry per command (at most 64, checked above) plus the queue's
        // previous stamp: run_job and the worker never grow them.
        for events in &mut history {
            events.reserve(65, GFP_KERNEL)?;
        }
        let mut job = queue.entity.new_job(1, SubmissionJob { execution, history: Some(history) })?;
        for fence in dependencies {
            job.add_dependency(fence)?;
        }
        // No fallible operation follows output-fence publication.
        // Publish the hardware lifetime fence directly. Scheduler finished
        // fences can be downgraded to scheduled dependencies by DRM; our
        // external inputs must wait for actual hardware completion.
        let fence = completion.clone();
        admission.push(completion, GFP_KERNEL)?;
        let job = job.arm();
        for output in outputs {
            if let Some(chain) = output.chain {
                output.object.add_point(chain, &fence, output.point);
            } else {
                output.object.replace_fence(Some(&fence));
            }
        }
        job.push();
        Ok(0)
    }
}

impl Vm {
    pub(super) fn cover(&self, start: u64, size: u64, permissions: u32) -> Result {
        Mapping::cover(&self.mappings, start, size, permissions)
    }

    pub(super) fn compute_parameters(&self, c: &uapi::drm_asahi_cmd_compute) -> Result<compute::Parameters> {
        if c.flags != 0 || c.helper.binary != 0 || c.helper.cfg != 0 || c.helper.data != 0 {
            return Err(unsupported(format_args!("compute flags {:#x} or a helper program", c.flags)));
        }
        let size = c
            .cdm_ctrl_stream_end
            .checked_sub(c.cdm_ctrl_stream_base)
            .ok_or_else(|| reject(format_args!("compute stream end before base")))?;
        if (c.cdm_ctrl_stream_base | c.cdm_ctrl_stream_end) & 3 != 0
            || size < 4
            || c.sampler_heap & 7 != 0
            || c.sampler_count > 1024
            || (c.sampler_count == 0) != (c.sampler_heap == 0)
        {
            return Err(reject(format_args!("compute stream {:#x}..{:#x} or sampler heap {:#x} x {}", c.cdm_ctrl_stream_base, c.cdm_ctrl_stream_end, c.sampler_heap, c.sampler_count)));
        }
        self.cover(c.cdm_ctrl_stream_base, size, 2)?;
        if c.sampler_count != 0 {
            self.cover(c.sampler_heap, u64::from(c.sampler_count) * 8, 2)?;
        }
        Ok(compute::Parameters {
            tgid: kernel::current!().group_leader().pid(),
            comm: current_comm(),
            cdm: c.cdm_ctrl_stream_base,
            cdm_end: c.cdm_ctrl_stream_end,
            sampler: c.sampler_heap,
            sampler_count: c.sampler_count,
            scratch: compute::SCRATCH,
            marker: compute::MARKER,
            save_area: 0,
        })
    }

    pub(super) fn render_parameters(&self, c: &uapi::drm_asahi_cmd_render) -> Result<render::Parameters> {
        use uapi::{
            drm_asahi_render_flags_DRM_ASAHI_RENDER_DBIAS_IS_INT as DBIAS_IS_INT,
            drm_asahi_render_flags_DRM_ASAHI_RENDER_FEW_PRIMITIVES as FEW_PRIMITIVES,
            drm_asahi_render_flags_DRM_ASAHI_RENDER_PROCESS_EMPTY_TILES as PROCESS_EMPTY_TILES,
            drm_asahi_render_flags_DRM_ASAHI_RENDER_VDM_BARRIER_FRAGMENT as VDM_BARRIER_FRAGMENT,
        };

        if c.flags & !(PROCESS_EMPTY_TILES | DBIAS_IS_INT | VDM_BARRIER_FRAGMENT | FEW_PRIMITIVES) != 0
            || c.vertex_helper.binary != 0
            || c.vertex_helper.cfg != 0
            || c.vertex_helper.data != 0
            || c.fragment_helper.binary != 0
            || c.fragment_helper.cfg != 0
            || c.fragment_helper.data != 0
        {
            return Err(unsupported(format_args!(
                "render flags {:#x} (known {:#x}) or a vertex/fragment helper program",
                c.flags,
                PROCESS_EMPTY_TILES | DBIAS_IS_INT | VDM_BARRIER_FRAGMENT | FEW_PRIMITIVES
            )));
        }
        let mut p = render::Parameters::default();
        p.tgid = kernel::current!().group_leader().pid();
        p.comm = current_comm();
        p.vdm_barrier_fragment = c.flags & VDM_BARRIER_FRAGMENT != 0;
        p.set_private(self.kernel.start, self.kernel.end)
            .ok_or_else(|| reject(format_args!("render private area outside the kernel VA")))?;
        p.width = u64::from(c.width_px);
        p.height = u64::from(c.height_px);
        p.layers = u64::from(c.layers);
        p.utile_width = u64::from(c.utile_width_px);
        p.utile_height = u64::from(c.utile_height_px);
        p.samples = u64::from(c.samples);
        p.sample_size = u64::from(c.sample_size_B);
        p.tib_blocks = (p.sample_size * p.utile_width * p.utile_height * p.samples).div_ceil(2048);
        p.encoder = c.vdm_ctrl_stream_base;
        if !p.valid() || p.encoder == 0 || p.encoder & 3 != 0 {
            return Err(reject(format_args!("render parameters or encoder {:#x}", p.encoder)));
        }
        self.cover(p.encoder, 4, 2)?;
        p.utile_config = (p.utile_width / 16) << 12
            | (p.utile_height / 16) << 14
            | match p.samples {
                1 => 0,
                2 => 1,
                4 => 2,
                _ => return Err(reject(format_args!("render samples {}", p.samples))),
            };
        p.process_empty_tiles = c.flags & PROCESS_EMPTY_TILES != 0;
        p.tile_config =
            0x280 | u64::from(p.layers > 1) | if p.process_empty_tiles { 0x10000 } else { 0 };
        p.multisample_control = c.ppp_multisamplectl;
        p.ppp_control = u64::from(c.ppp_ctrl);
        p.merge_upper_x_bits = u64::from(c.isp_merge_upper_x);
        p.merge_upper_y_bits = u64::from(c.isp_merge_upper_y);
        // ISP control: bit 18 is the integer depth-bias formula (the UAPI flag
        // matches it). Bit 0 selects a faster fragment tile dispatch that is
        // only safe for passes with very few primitives; macOS sets it for at
        // most four statically known primitives per pass, and userspace
        // asserts the same bound with FEW_PRIMITIVES.
        p.aux_fb_flags = 0xc000
            | u64::from(c.flags & DBIAS_IS_INT)
            | u64::from(c.flags & FEW_PRIMITIVES != 0);
        p.aux_fb_page_count = 0x100000;
        p.scissor_array = c.isp_scissor_base;
        p.depth_bias_array = c.isp_dbias_base;
        p.occlusion_query_base = c.isp_oclqry_base;
        if p.scissor_array == 0 {
            return Err(reject(format_args!("render without a scissor array")));
        }
        for (address, access) in [
            (p.scissor_array, 2),
            (p.depth_bias_array, 2),
            (p.occlusion_query_base, 4),
        ] {
            if address != 0 {
                if address & 7 != 0 {
                    return Err(reject(format_args!("render array {:#x} unaligned", address)));
                }
                self.cover(address, 8, access)?;
            }
        }
        for zls in [&c.depth, &c.stencil] {
            if (zls.base == 0 && (zls.comp_base != 0 || zls.stride != 0 || zls.comp_stride != 0))
                || (zls.comp_base == 0 && zls.comp_stride != 0)
                || (p.layers > 1 && zls.base != 0 && zls.stride == 0)
                || (zls.stride != 0 && zls.stride & 0x3fff != 1)
                || zls.comp_stride & 0x3fff != 0
            {
                return Err(reject(format_args!("render depth/stencil base {:#x} stride {:#x} comp stride {:#x} layers {}", zls.base, zls.stride, zls.comp_stride, p.layers)));
            }
            if zls.base != 0 {
                let stride = if zls.stride == 0 {
                    0
                } else {
                    (u64::from(zls.stride >> 14) + 1) * 0x4000
                };
                self.cover(zls.base, stride * (p.layers - 1) + 1, 6)?;
            }
            if zls.comp_base != 0 {
                self.cover(
                    zls.comp_base,
                    (u64::from(zls.comp_stride >> 14) + 1) * 0x80 * (p.layers - 1) + 1,
                    6,
                )?;
            }
        }
        p.depth_buffer = c.depth.base;
        p.depth_aux_buffer = c.depth.comp_base;
        p.depth_stride = u64::from(c.depth.stride);
        p.depth_aux_stride = u64::from(c.depth.comp_stride);
        p.stencil_buffer = c.stencil.base;
        p.stencil_aux_buffer = c.stencil.comp_base;
        p.stencil_stride = u64::from(c.stencil.stride);
        p.stencil_aux_stride = u64::from(c.stencil.comp_stride);
        p.depth_flags = c.zls_ctrl;
        p.depth_dimensions = u64::from(c.isp_zls_pixels);
        p.depth_clear_value_bits = u64::from(c.isp_bgobjdepth);
        p.stencil_clear_value = u64::from(c.isp_bgobjvals);
        p.sampler_array = c.sampler_heap;
        p.sampler_count = u64::from(c.sampler_count);
        if !p.valid() || p.sampler_count > 1024 {
            return Err(reject(format_args!("render parameters or {} samplers", p.sampler_count)));
        }
        if p.sampler_count != 0 {
            self.cover(p.sampler_array, p.sampler_count * 8, 2)?;
        }
        for (program, optional) in [
            (&c.bg, true),
            (&c.eot, false),
            (&c.partial_bg, c.bg.usc == 0),
            (&c.partial_eot, false),
        ] {
            if program.usc == 0 {
                if !optional || program.rsrc_spec != 0 {
                    return Err(reject(format_args!("render background/end-of-tile program missing or with resources")));
                }
            } else {
                self.cover((1 << 40) + u64::from(program.usc & !7), 4, 2)?;
            }
        }
        let pipeline = |v: u32| if v == 0 { 0 } else { (1 << 40) + u64::from(v) };
        let load_bind = |v: &uapi::drm_asahi_bg_eot| {
            if v.usc == 0 {
                0
            } else {
                0xffff800000000000 | u64::from(v.rsrc_spec)
            }
        };
        p.load_pipeline = pipeline(c.bg.usc);
        p.load_pipeline_bind = load_bind(&c.bg);
        p.partial_load_pipeline = pipeline(c.partial_bg.usc);
        p.partial_load_pipeline_bind = load_bind(&c.partial_bg);
        p.store_pipeline = pipeline(c.eot.usc);
        p.store_pipeline_bind = u64::from(c.eot.rsrc_spec);
        p.partial_store_pipeline = pipeline(c.partial_eot.usc);
        p.partial_store_pipeline_bind = u64::from(c.partial_eot.rsrc_spec);
        p.request_tvb_growth = true;
        p.tiling_admission = 7;
        p.fragment_admission = 7;
        p.cycle = 0x240030;
        p.tiling_gate = 0xa900400020;
        p.fragment_gate = 0xd500400020;
        p.tiling_lifecycle = 0x08009f520900a6bb;
        p.fragment_lifecycle = 0x08009f520900a6ba;
        p.tiling_work_stamp = 0x120;
        p.fragment_work_stamp = 0x121;
        // macOS writes 0-3 (3 solo, mostly 1 under contention); this driver's
        // original value is 4.
        p.completion_control = 4;
        p.layermeta = p.heapmeta;
        if p.layers > 1 {
            p.heapmeta += 0x100;
        }
        Ok(p)
    }
}
