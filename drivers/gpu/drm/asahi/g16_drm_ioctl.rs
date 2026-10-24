// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! The DRM ioctls other than SUBMIT: parameters, time, VM and queue
//! lifecycle, VM_BIND and GEM objects.

use super::*;

impl File {
    pub(super) fn get_time(
        _device: &RegDevice,
        _reg: &RegData<'_>,
        data: &mut uapi::drm_asahi_get_time,
        _file: &DrmFile,
    ) -> Result<u32> {
        if data.flags != 0 {
            return Err(EINVAL);
        }
        let (counter, frequency): (u64, u64);
        // SAFETY: These instructions only read architectural timer registers.
        unsafe {
            core::arch::asm!("mrs {c}, cntpct_el0", "mrs {f}, cntfrq_el0",
                                 c = out(reg) counter, f = out(reg) frequency);
        }
        if frequency == 0 {
            return Err(EIO);
        }
        data.gpu_timestamp =
            counter / frequency * 1_000_000_000 + (counter % frequency) * 1_000_000_000 / frequency;
        Ok(0)
    }

    pub(super) fn vm_create(
        device: &RegDevice,
        _reg: &RegData<'_>,
        data: &mut uapi::drm_asahi_vm_create,
        file: &DrmFile,
    ) -> Result<u32> {
        let device: &Device = device;
        if data.pad != 0
            || data.kernel_start < 0x4000
            || data.kernel_end > VM_END
            || data
                .kernel_end
                .checked_sub(data.kernel_start)
                .ok_or(EINVAL)?
                < 0x20000000
            || (data.kernel_start | data.kernel_end) & 0x3fff != 0
        {
            return Err(EINVAL);
        }
        let inner = file.inner();
        let mut state = inner.state.lock();
        let id = state.next_vm;
        let next = id.checked_add(1).ok_or(ENOSPC)?;
        let reservation = Object::new(
            device,
            0x4000,
            shmem::ObjectConfig {
                map_wc: false,
                parent_resv_obj: None,
            },
            (inner.id, id),
        )?;
        let space = Arc::pin_init(
            kernel::new_mutex!(crate::g16_vm::AddressSpace::new()?),
            GFP_KERNEL,
        )?;
        let compute_space = Arc::pin_init(
            kernel::new_mutex!(crate::g16_vm::AddressSpace::new()?),
            GFP_KERNEL,
        )?;
        // Debug: name the process behind each VM so firmware context slots in
        // hang reports (logged by root) can be traced to a client.
        if crate::module_parameters::debug_ctx.value() != 0 {
            let task = kernel::current!();
            let leader = task.group_leader();
            let comm = crate::util::task_comm(leader);
            pr_info!(
                "G16: file {} vm {} by tgid {} ({}): render root {:#x} compute root {:#x}\n",
                inner.id,
                id,
                leader.pid(),
                crate::util::comm_str(&comm),
                space.lock().roots().low,
                compute_space.lock().roots().low
            );
        }
        state.vms.push(
            Vm {
                id,
                visible: true,
                kernel: data.kernel_start..data.kernel_end,
                space,
                compute_space,
                mappings: KVVec::new(),
                snapshot: None,
                pending: KVec::new(),
                draining: 0,
                reservation,
                render_support: None,
                // The module parameter was validated at probe and debugfs
                // writes are bounded, so the mode always parses.
                native_mode: crate::g16_mesh::Mode::from_parameter(crate::g16_mesh::snapshot().1)
                    .unwrap_or(crate::g16_mesh::Mode::Disabled),
                render_ready: false,
                tvb_ready: false,
                compute_ready: false,
                render_broken: false,
                deferred: KVec::new(),
            },
            GFP_KERNEL,
        )?;
        state.next_vm = next;
        data.vm_id = id;
        Ok(0)
    }

    pub(super) fn vm_destroy(
        device: &RegDevice,
        _reg: &RegData<'_>,
        data: &mut uapi::drm_asahi_vm_destroy,
        file: &DrmFile,
    ) -> Result<u32> {
        let device: &Device = device;
        if data.pad != 0 {
            return Err(EINVAL);
        }
        let inner = file.inner();
        let mut state = inner.state.lock();
        let index = state
            .vms
            .iter()
            .position(|v| v.id == data.vm_id && v.visible)
            .ok_or(ENOENT)?;
        if state.queues.iter().any(|q| q.vm == data.vm_id) {
            // Removing the public handle must not invalidate existing queues.
            // They retain this VM until the last queue is destroyed.
            state.vms[index].visible = false;
        } else {
            // No queue can submit to this VM any more, so only its own
            // pending work can still use its tables. Other clients' work
            // never touches them: don't wait for the whole device.
            let (relocked, result, _) = drain_vm(state, data.vm_id, 35000, false, &inner.drained);
            state = relocked;
            result?;
            let index = state
                .vms
                .iter()
                .position(|v| v.id == data.vm_id && v.visible)
                .ok_or(ENOENT)?;
            state.vms[index].detach(&mut device.runtime.lock())?;
            state.vms.swap_remove(index);
        }
        Ok(0)
    }

    pub(super) fn get_params(
        device: &RegDevice,
        _reg: &RegData<'_>,
        data: &mut uapi::drm_asahi_get_params,
        _file: &DrmFile,
    ) -> Result<u32> {
        let device: &Device = device;
        if data.param_group != 0 || data.pad != 0 {
            return Err(EINVAL);
        }
        let mut params = uapi::drm_asahi_params_global {
            features: u64::from(uapi::drm_asahi_feature_DRM_ASAHI_FEATURE_FRAGMENT_BARRIER)
                | u64::from(uapi::drm_asahi_feature_DRM_ASAHI_FEATURE_FEW_PRIMITIVES)
                | if crate::module_parameters::fault_control.value() == 3
                    && crate::module_parameters::soft_faults.value() != 0
                {
                    u64::from(uapi::drm_asahi_feature_DRM_ASAHI_FEATURE_SOFT_FAULTS)
                } else {
                    0
                },
            gpu_generation: 16,
            gpu_variant: b'G' as u32,
            gpu_revision: device.gpu_revision,
            chip_id: 0x8132,
            num_dies: 1,
            num_clusters_total: 1,
            num_cores_per_cluster: 10,
            max_frequency_khz: device.maximum_frequency_khz,
            core_masks: [0; uapi::DRM_ASAHI_MAX_CLUSTERS as usize],
            vm_start: 0x4000,
            vm_end: (1 << 42) - 0x8000,
            vm_kernel_min_size: 0x20000000,
            max_commands_per_submission: 64,
            max_attachments: 16,
            command_timestamp_frequency_hz: 1_000_000_000,
        };
        params.core_masks[0] = u64::from(device.core_mask);
        let size = core::mem::size_of_val(&params).min(data.size.try_into()?);
        let mut writer = UserSlice::new(UserPtr::from_addr(data.pointer as usize), size).writer();
        // SAFETY: The UAPI object has fully initialized fields, and size is
        // bounded by its extent. This copies once into the caller's buffer.
        writer.write_slice(unsafe {
            core::slice::from_raw_parts(
                (&params as *const uapi::drm_asahi_params_global).cast(),
                size,
            )
        })?;
        Ok(0)
    }

    pub(super) fn queue_create(
        device: &RegDevice,
        _reg: &RegData<'_>,
        data: &mut uapi::drm_asahi_queue_create,
        file: &DrmFile,
    ) -> Result<u32> {
        let device: &Device = device;
        if data.flags != 0 || data.priority > 1 || data.usc_exec_base != 1 << 40 {
            return Err(EINVAL);
        }
        let inner = file.inner();
        // A later record may drain with state dropped, exposing earlier edits.
        // The list is not atomic across that wait (nor was it on error);
        // userspace must synchronize sparse binds with its submissions.
        let mut state = inner.state.lock();
        if !state.vms.iter().any(|vm| vm.id == data.vm_id && vm.visible) {
            return Err(ENOENT);
        }
        let id = state.next_queue;
        let next = id.checked_add(1).ok_or(ENOSPC)?;
        let priority = if data.priority == 0 {
            sched::Priority::Low
        } else {
            sched::Priority::Normal
        };
        let entity = sched::Entity::new(&device.scheduler, priority)?;
        state.queues.push(
            Queue {
                firmware: Arc::pin_init(
                    // Firmware priority runs in the opposite direction:
                    // public low/medium select native classes 3/2.
                    kernel::new_mutex!(crate::g16::FirmwareQueues::new(3 - data.priority)),
                    GFP_KERNEL,
                )?,
                id,
                vm: data.vm_id,
                entity,
            },
            GFP_KERNEL,
        )?;
        state.next_queue = next;
        data.queue_id = id;
        Ok(0)
    }

    pub(super) fn queue_destroy(
        device: &RegDevice,
        _reg: &RegData<'_>,
        data: &mut uapi::drm_asahi_queue_destroy,
        file: &DrmFile,
    ) -> Result<u32> {
        let device: &Device = device;
        if data.pad != 0 {
            return Err(EINVAL);
        }
        let inner = file.inner();
        let queue = {
            let mut state = inner.state.lock();
            let index = state
                .queues
                .iter()
                .position(|q| q.id == data.queue_id)
                .ok_or(ENOENT)?;
            state.queues.swap_remove(index)
        };
        // Cancel dependency-blocked jobs. Scheduler destruction must run
        // outside the file-state lock. The queue's firmware storage needs no
        // wait here: its in-flight Executions hold it until they retire
        // (QueueStorage), and no other queue references it.
        let vm_id = queue.vm;
        drop(queue);
        let mut state = inner.state.lock();
        let last = |state: &FileState| {
            !state.queues.iter().any(|q| q.vm == vm_id)
                && state.vms.iter().any(|v| v.id == vm_id && !v.visible)
        };
        if last(&state) {
            // The last queue of an already destroyed VM: detach it once the
            // VM's own work has finished, never waiting for other clients.
            let (relocked, result, _) = drain_vm(state, vm_id, 35000, false, &inner.drained);
            state = relocked;
            result?;
            if last(&state) {
                let index = state.vms.iter().position(|v| v.id == vm_id).ok_or(ENOENT)?;
                state.vms[index].detach(&mut device.runtime.lock())?;
                state.vms.swap_remove(index);
            }
        }
        Ok(0)
    }

    pub(super) fn bind_object(
        device: &RegDevice,
        reg: &RegData<'_>,
        data: &mut uapi::drm_asahi_gem_bind_object,
        file: &DrmFile,
    ) -> Result<u32> {
        let device: &Device = device;
        if data.pad != 0 || data.vm_id != 0 {
            return Err(EINVAL);
        }
        // A new timestamp mapping is unused, so binding never waits. Unbinding
        // waits only for this file's own work (below), never other clients'.
        let inner = file.inner();
        let mut state = inner.state.lock();
        match data.op {
            uapi::drm_asahi_bind_object_op_DRM_ASAHI_BIND_OBJECT_OP_BIND => {
                if data.flags
                    != uapi::drm_asahi_bind_object_flags_DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS
                    || data.range == 0
                    || (data.offset | data.range) & 0x3fff != 0
                {
                    return Err(EINVAL);
                }
                let bo = Object::lookup_handle(file, data.handle)?;
                if data.offset.checked_add(data.range).ok_or(EINVAL)? > bo.size() as u64 {
                    return Err(EINVAL);
                }
                let id = state.next_timestamp;
                let next = id.checked_add(1).ok_or(ENOSPC)?;
                let sgt = bo.sg_table(reg.pdev.as_ref())?;
                let first = data.offset & !0x3fff;
                let extent = ((data.offset + data.range + 0x3fff) & !0x3fff) - first;
                state.timestamps.reserve(1, GFP_KERNEL)?;
                state.retained_timestamps.reserve(1, GFP_KERNEL)?;
                let mut runtime = device.runtime.lock();
                let base = runtime.reserve_timestamp(extent)?;
                let timestamp = TimestampBuffer {
                    id,
                    _obj: bo.clone(),
                    base,
                    extent,
                    address: base + (data.offset - first),
                    size: data.range,
                };
                // Keep backing even if mapping installation or rollback fails.
                state
                    .retained_timestamps
                    .push(timestamp.clone(), GFP_KERNEL)?;
                let mut offset = first;
                let mut address = base;
                let result = (|| {
                    for segment in sgt.iter() {
                        let length = u64::from(segment.dma_len());
                        if offset >= length {
                            offset -= length;
                            continue;
                        }
                        let size = (length - offset).min(base + extent - address);
                        runtime.map_timestamp(
                            address,
                            segment.dma_address() + offset,
                            size.try_into()?,
                        )?;
                        address += size;
                        offset = 0;
                        if address == base + extent {
                            break;
                        }
                    }
                    if address != base + extent {
                        return Err(EIO);
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    runtime.release_timestamp(base..base + extent)?;
                    state.retained_timestamps.pop();
                    return Err(error);
                }
                state.timestamps.push(timestamp, GFP_KERNEL)?;
                state.next_timestamp = next;
                data.object_handle = id;
            }
            uapi::drm_asahi_bind_object_op_DRM_ASAHI_BIND_OBJECT_OP_UNBIND => {
                if data.flags != 0 || data.offset != 0 || data.range != 0 || data.handle != 0 {
                    return Err(EINVAL);
                }
                let index = state
                    .timestamps
                    .iter()
                    .position(|t| t.id == data.object_handle)
                    .ok_or(ENOENT)?;
                // Unpublish first: no later submission can resolve it, while
                // the retained copy keeps the firmware mapping and backing.
                let timestamp = state.timestamps.swap_remove(index);
                // Only this file's submissions can reference the binding. Wait
                // for each of its VMs' own work (progress-based), not for
                // every client's. On failure the handle is already gone: the
                // mapping stays retained until file teardown, and a retry of
                // this UNBIND returns ENOENT.
                let mut ids = KVec::new();
                for vm in state.vms.iter() {
                    ids.push(vm.id, GFP_KERNEL)?;
                }
                for id in ids {
                    let (relocked, result, _) = drain_vm(state, id, 35000, false, &inner.drained);
                    state = relocked;
                    match result {
                        Err(error) if error != ENOENT => return Err(error),
                        _ => {}
                    }
                }
                // Failed BIND rollback can retain an unpublished allocation
                // without advancing next_timestamp. Match the reserved range,
                // which stays unique even if that public ID is reused.
                let retained = state
                    .retained_timestamps
                    .iter()
                    .position(|t| t.base == timestamp.base && t.extent == timestamp.extent)
                    .ok_or(EIO)?;
                // Keep the retained backing reference if unmapping fails, just
                // as at file teardown, so a stale PTE never reaches freed RAM.
                device.runtime.lock().release_timestamp(
                    timestamp.base..timestamp.base + timestamp.extent,
                )?;
                state.retained_timestamps.swap_remove(retained);
            }
            _ => return Err(EINVAL),
        }
        Ok(0)
    }

    pub(super) fn vm_bind(device: &RegDevice, reg: &RegData<'_>, data: &mut uapi::drm_asahi_vm_bind, file: &DrmFile) -> Result<u32> {
        let device: &Device = device;
        if data.pad != 0 || data.num_binds > 4096 || data.stride == 0 {
            return Err(EINVAL);
        }
        let byte_count = (data.stride as usize)
            .checked_mul(data.num_binds as usize)
            .ok_or(EINVAL)?;
        if byte_count > 1024 * 1024 {
            return Err(EINVAL);
        }
        let mut records = KVVec::new();
        UserSlice::new(UserPtr::from_addr(data.userptr.try_into()?), byte_count)
            .reader()
            .read_all(&mut records, GFP_KERNEL)?;
        let mut reader = crate::util::Reader::new(&records);
        if device.failed.load(Ordering::Acquire) {
            return Err(EIO);
        }
        let inner = file.inner();
        let mut state = inner.state.lock();
        if !state.vms.iter().any(|vm| vm.id == data.vm_id && vm.visible) {
            return Err(ENOENT);
        }
        if inner.reaper.armed.load(Ordering::Relaxed) {
            reap(&mut state, device, &inner.reaper.armed);
        }
        for _ in 0..data.num_binds {
            let bind: uapi::drm_asahi_gem_bind_op = reader.read_up_to(data.stride as usize)?;
            loop {
                if device.failed.load(Ordering::Acquire) {
                    return Err(EIO);
                }
                let vm = state.vms.iter_mut()
                    .find(|vm| vm.id == data.vm_id && vm.visible).ok_or(ENOENT)?;
                let (flags, handle, offset, size, address) =
                    (bind.flags, bind.handle, bind.offset, bind.range, bind.addr);
                let end = address.checked_add(size).ok_or(EINVAL)?;
                if size == 0
                    || (address | size | offset) & 0x3fff != 0
                    || address < 0x4000
                    || end > VM_END
                    || (address < vm.kernel.end && end > vm.kernel.start)
                    || (address < 0x73_0000_0000 && end > 0x71_0000_0000)
                    || [
                        (0x7000000000, 0x7000400000),
                        (0x7000408000, 0x7000418000),
                        (0x7000420000, 0x7002cb8000),
                        (0x7003108000, 0x7005180000),
                    ]
                    .iter()
                    .any(|(base, limit)| address < *limit && end > *base)
                    || compute::private_ranges()
                        .iter()
                        .any(|(base, len)| address < base + *len as u64 && end > *base)
                    || render::fixed_ranges()
                        .iter()
                        .any(|(base, len)| address < base + *len as u64 && end > *base)
                {
                    return Err(EINVAL);
                }
                if flags == uapi::drm_asahi_bind_flags_DRM_ASAHI_BIND_UNBIND {
                    if handle != 0 || offset != 0 {
                        return Err(EINVAL);
                    }
                    // Only this VM can reference the removed mapping. Fresh
                    // binds never wait for hardware, including on this VM.
                    if !vm.pending.is_empty() {
                        let (relocked, result, _) = drain_vm(state, data.vm_id, 35000, false, &inner.drained);
                        state = relocked;
                        result?;
                        // State stayed locked after the idle check. Revalidate
                        // visibility and mappings changed by concurrent ioctls.
                        continue;
                    }
                    let runtime = device.runtime.lock();
                    vm.unmap(address..end, &runtime)?;
                    break;
                }
                let read = flags & uapi::drm_asahi_bind_flags_DRM_ASAHI_BIND_READ != 0;
                let write = flags & uapi::drm_asahi_bind_flags_DRM_ASAHI_BIND_WRITE != 0;
                let single = flags & uapi::drm_asahi_bind_flags_DRM_ASAHI_BIND_SINGLE_PAGE != 0;
                if flags & !0xe != 0 || (!read && !write) {
                    return Err(EINVAL);
                }
                let bo = Object::lookup_handle(file, handle)?;
                if bo.vm != 0 && (bo.vm != vm.id || bo.owner != inner.id) {
                    return Err(EINVAL);
                }
                if offset
                    .checked_add(if single { 0x4000 } else { size })
                    .ok_or(EINVAL)?
                    > bo.size() as u64
                {
                    return Err(EINVAL);
                }
                let access = match (read, write) {
                    (true, true) => crate::pgtable::prot::PROT_GPU_SHARED_RW,
                    (true, false) => crate::pgtable::prot::PROT_GPU_SHARED_RO,
                    _ => crate::pgtable::prot::PROT_GPU_SHARED_WO,
                };
                // A bind replaces whatever the range held, as sparse residency
                // and address replay rebind reserved ranges. Remove only the
                // overlapping parts of existing mappings.
                let mut overlaps = KVec::new();
                for m in vm.mappings.iter() {
                    if address < m.range.end && end > m.range.start {
                        overlaps.push(
                            m.range.start.max(address)..m.range.end.min(end),
                            GFP_KERNEL,
                        )?;
                    }
                }
                if crate::g16_deferred::overlap(&vm.mappings, address, end)
                    == crate::g16_deferred::Overlap::Deferred
                {
                    // Only closed objects overlap (an address reused after
                    // GEM_CLOSE): wait for their own snapshots, not for the
                    // whole VM, then reap and revalidate. Closing mappings no
                    // entry tracks (none expected) take the rule below.
                    let mut fences = KVec::new();
                    for entry in vm.deferred.iter() {
                        if vm.mappings.iter().any(|m| {
                            m.closing && core::ptr::eq(&*m.obj, &*entry.obj)
                                && address < m.range.end && end > m.range.start
                        }) {
                            fences.extend_from_slice(&entry.fences, GFP_KERNEL)?;
                        }
                    }
                    if !fences.is_empty() {
                        drop(state);
                        let (result, _) = drain_within(&mut fences, 35000, crate::g16_drain::CAP_MS, false);
                        state = inner.state.lock();
                        result?;
                        reap(&mut state, device, &inner.reaper.armed);
                        continue;
                    }
                }
                if !overlaps.is_empty() {
                    if !vm.pending.is_empty() {
                        let (relocked, result, _) = drain_vm(state, data.vm_id, 35000, false, &inner.drained);
                        state = relocked;
                        result?;
                        // State stayed locked after the idle check. Revalidate
                        // visibility and mappings changed by concurrent ioctls.
                        continue;
                    }
                    let runtime = device.runtime.lock();
                    for range in overlaps.iter() {
                        vm.unmap(range.clone(), &runtime)?;
                    }
                }
                let runtime = device.runtime.lock();
                vm.map(
                    address..end,
                    offset,
                    bo.clone(),
                    bo.sg_table(reg.pdev.as_ref())?,
                    access,
                    flags & 6,
                    single,
                    &runtime,
                )?;
                break;
            }
        }
        Ok(0)
    }

    pub(super) fn gem_create(
        device: &RegDevice,
        _reg: &RegData<'_>,
        data: &mut uapi::drm_asahi_gem_create,
        file: &DrmFile,
    ) -> Result<u32> {
        let device: &Device = device;
        let private = data.flags & uapi::drm_asahi_gem_flags_DRM_ASAHI_GEM_VM_PRIVATE != 0;
        if data.flags
            & !(uapi::drm_asahi_gem_flags_DRM_ASAHI_GEM_WRITEBACK
                | uapi::drm_asahi_gem_flags_DRM_ASAHI_GEM_VM_PRIVATE)
            != 0
            || (!private && data.vm_id != 0)
            || data.size == 0
            || data.pad != 0
        {
            return Err(EINVAL);
        }
        let size: usize = data.size.try_into()?;
        let size = size.checked_add(0x3fff).ok_or(EINVAL)? & !0x3fff;
        let inner = file.inner();
        let state = inner.state.lock();
        let parent = if private {
            Some(
                &*state
                    .vms
                    .iter()
                    .find(|vm| vm.id == data.vm_id && vm.visible)
                    .ok_or(ENOENT)?
                    .reservation,
            )
        } else {
            None
        };
        let bo = Object::new(
            device,
            size,
            shmem::ObjectConfig {
                map_wc: data.flags & uapi::drm_asahi_gem_flags_DRM_ASAHI_GEM_WRITEBACK == 0,
                parent_resv_obj: parent,
            },
            (inner.id, if private { data.vm_id } else { 0 }),
        )?;
        data.handle = bo.create_handle(file)?;
        Ok(0)
    }

    pub(super) fn gem_mmap_offset(
        _device: &RegDevice,
        _reg: &RegData<'_>,
        data: &mut uapi::drm_asahi_gem_mmap_offset,
        file: &DrmFile,
    ) -> Result<u32> {
        if data.flags != 0 {
            return Err(EINVAL);
        }
        data.offset = Object::lookup_handle(file, data.handle)?.create_mmap_offset()?;
        Ok(0)
    }
}
