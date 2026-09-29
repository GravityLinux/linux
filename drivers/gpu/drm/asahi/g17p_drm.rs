// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Native Asahi memory UAPI and synchronous first-compute bring-up.
//! Published mappings and GEM references survive ioctl/file teardown until
//! firmware is stopped. Unimplemented submission features fail explicitly.

use super::{
    g17p_boot::Session, g17p_compute::USC_EXEC_BASE, g17p_image::Image, g17p_user_vm::UserVm,
};
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use kernel::{
    bindings, c_str, device, drm,
    drm::{
        gem::{self, shmem, BaseObject, DriverObject},
        ioctl,
    },
    new_mutex,
    prelude::*,
    sync::{aref::ARef, Arc, Mutex},
    uaccess::{UserPtr, UserSlice},
    uapi,
};

const PAGE: u64 = 0x4000;
const VM_START: u64 = PAGE;
const VM_END: u64 = (1 << 42) - 2 * PAGE;
const VM_KERNEL_MIN: u64 = 0x20000000;
const WRITEBACK: u32 = uapi::drm_asahi_gem_flags_DRM_ASAHI_GEM_WRITEBACK;
const PRIVATE: u32 = uapi::drm_asahi_gem_flags_DRM_ASAHI_GEM_VM_PRIVATE;
const UNBIND: u32 = uapi::drm_asahi_bind_flags_DRM_ASAHI_BIND_UNBIND;
const READ: u32 = uapi::drm_asahi_bind_flags_DRM_ASAHI_BIND_READ;
const WRITE: u32 = uapi::drm_asahi_bind_flags_DRM_ASAHI_BIND_WRITE;
const SINGLE: u32 = uapi::drm_asahi_bind_flags_DRM_ASAHI_BIND_SINGLE_PAGE;

type Device = drm::Device<Driver>;
type DrmFile = drm::File<File>;
pub(crate) type Object = shmem::Object<Bo>;
pub(crate) struct Runtime {
    pub(crate) session: Session,
    pub(crate) image: Image,
}
pub(crate) type RuntimeRef = Arc<Mutex<Option<Runtime>>>;

pub(crate) struct Driver;
#[pin_data]
pub(crate) struct Data {
    params: uapi::drm_asahi_params_global,
    runtime: RuntimeRef,
}
#[vtable]
impl drm::driver::Driver for Driver {
    type Data = Data;
    const DROP_DATA: bool = true;
    type File = File;
    type Object = Object;
    const INFO: drm::driver::DriverInfo = drm::driver::DriverInfo {
        major: 0,
        minor: 0,
        patchlevel: 0,
        name: c_str!("asahi"),
        desc: c_str!("Apple AGX Graphics"),
    };
    const FEATURES: u32 = drm::driver::FEAT_GEM
        | drm::driver::FEAT_RENDER
        | drm::driver::FEAT_SYNCOBJ
        | drm::driver::FEAT_SYNCOBJ_TIMELINE;
    kernel::declare_drm_ioctls! {
        (ASAHI_GET_PARAMS,drm_asahi_get_params,ioctl::RENDER_ALLOW,File::get_params),
        (ASAHI_GET_TIME,drm_asahi_get_time,ioctl::AUTH|ioctl::RENDER_ALLOW,File::get_time),
        (ASAHI_VM_CREATE,drm_asahi_vm_create,ioctl::AUTH|ioctl::RENDER_ALLOW,File::vm_create),
        (ASAHI_VM_DESTROY,drm_asahi_vm_destroy,ioctl::AUTH|ioctl::RENDER_ALLOW,File::vm_destroy),
        (ASAHI_VM_BIND,drm_asahi_vm_bind,ioctl::AUTH|ioctl::RENDER_ALLOW,File::vm_bind),
        (ASAHI_GEM_CREATE,drm_asahi_gem_create,ioctl::AUTH|ioctl::RENDER_ALLOW,File::gem_create),
        (ASAHI_GEM_MMAP_OFFSET,drm_asahi_gem_mmap_offset,ioctl::AUTH|ioctl::RENDER_ALLOW,File::gem_mmap_offset),
        (ASAHI_GEM_BIND_OBJECT,drm_asahi_gem_bind_object,ioctl::AUTH|ioctl::RENDER_ALLOW,File::gem_bind_object),
        (ASAHI_QUEUE_CREATE,drm_asahi_queue_create,ioctl::AUTH|ioctl::RENDER_ALLOW,File::queue_create),
        (ASAHI_QUEUE_DESTROY,drm_asahi_queue_destroy,ioctl::AUTH|ioctl::RENDER_ALLOW,File::queue_destroy),
        (ASAHI_SUBMIT,drm_asahi_submit,ioctl::AUTH|ioctl::RENDER_ALLOW,File::submit),
    }
}

pub(crate) fn register(
    dev: &device::Device<device::Bound>,
    version: u32,
    counts: u32,
    core_mask: u32,
    max_mhz: u32,
    runtime: RuntimeRef,
) -> Result<ARef<Device>> {
    let dies = (counts >> 16) & 15;
    let clusters = ((counts >> 8) & 255) * dies;
    let cores = counts & 255;
    // The supported device has one cluster. Read the enabled mask, never infer
    // enabled cores from the maximum core count.
    if version >> 24 != 10
        || (version >> 16) & 255 != 0
        || dies != 1
        || clusters != 1
        || cores == 0
        || cores > 32
        || core_mask == 0
        || (core_mask as u64) >> cores != 0
    {
        return Err(ENODEV);
    }
    let mut masks = [0u64; uapi::DRM_ASAHI_MAX_CLUSTERS as usize];
    masks[0] = core_mask as u64;
    let params = uapi::drm_asahi_params_global {
        features: 0,
        gpu_generation: 17,
        gpu_variant: b'P' as u32,
        gpu_revision: (version >> 8) & 255,
        chip_id: 0x8140,
        num_dies: dies,
        num_clusters_total: clusters,
        num_cores_per_cluster: cores,
        max_frequency_khz: max_mhz.checked_mul(1000).ok_or(EINVAL)?,
        core_masks: masks,
        vm_start: VM_START,
        vm_end: VM_END,
        vm_kernel_min_size: VM_KERNEL_MIN,
        max_commands_per_submission: 64,
        max_attachments: 16,
        command_timestamp_frequency_hz: 1_000_000_000,
    };
    let drm = Device::new(dev, try_pin_init!(Data { params, runtime }))?;
    drm::driver::Registration::new_foreign_owned(&drm, dev, 0)?;
    dev_info!(
        dev,
        "G17P: Asahi DRM/GEM registered: core mask {:#x}; first-work initialization deferred\n",
        core_mask
    );
    Ok(drm)
}

#[pin_data]
pub(crate) struct Bo {
    owner_file: u64,
    owner_vm: u32,
}
#[vtable]
impl DriverObject for Bo {
    type Driver = Driver;
    type Args = (u64, u32);
    const HAS_EXPORT: bool = true;
    fn new(_dev: &Device, _size: usize, args: Self::Args) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            owner_file: args.0,
            owner_vm: args.1
        })
    }
    fn close(obj: &Object, file: &DrmFile) {
        let inner = file.inner();
        let mut state = inner.state.lock();
        for vm in &mut state.vms {
            let mut failed = false;
            for binding in &vm.bindings {
                if core::ptr::eq(&*binding.bo, obj)
                    && vm.tree.unmap(binding.start, binding.size).is_err()
                {
                    failed = true;
                }
            }
            // A table access error must not drop the last BO reference while
            // a leaf may remain. Keep it until this unpublished VM is dropped.
            if !failed {
                vm.bindings
                    .retain(|binding| !core::ptr::eq(&*binding.bo, obj));
            } else {
                pr_err!("G17P: GEM close retained mapping after table error\n");
            }
        }
        state
            .objects
            .retain(|object| !core::ptr::eq(&*object.bo, obj));
    }
    fn export(obj: &Object, flags: u32) -> Result<gem::DmaBuf<Object>> {
        if obj.owner_vm != 0 {
            return Err(EINVAL);
        }
        obj.prime_export(flags)
    }
}

struct Binding {
    start: u64,
    size: u64,
    offset: u64,
    flags: u32,
    bo: ARef<Object>,
}
impl Binding {
    fn end(&self) -> u64 {
        self.start + self.size
    }
    fn part(&self, start: u64, end: u64) -> Self {
        Self {
            start,
            size: end - start,
            offset: self.offset
                + if self.flags & SINGLE != 0 {
                    0
                } else {
                    start - self.start
                },
            flags: self.flags,
            bo: self.bo.clone(),
        }
    }
}
struct Vm {
    id: u32,
    kernel_start: u64,
    kernel_end: u64,
    tree: UserVm,
    resv: ARef<Object>,
    bindings: KVec<Binding>,
}
impl Vm {
    fn range(&self, start: u64, size: u64) -> Result<u64> {
        let end = start.checked_add(size).ok_or(EINVAL)?;
        if size == 0
            || (start | size) & (PAGE - 1) != 0
            || start < VM_START
            || end > VM_END
            || start < self.kernel_end && self.kernel_start < end
        {
            return Err(EINVAL);
        }
        Ok(end)
    }
    fn cover(&self, start: u64, size: u64, flags: u32) -> Result {
        let end = start.checked_add(size).ok_or(EINVAL)?;
        if size == 0 {
            return Err(EINVAL);
        }
        let mut cursor = start;
        while cursor < end {
            let binding = self
                .bindings
                .iter()
                .find(|b| b.start <= cursor && cursor < b.end() && b.flags & flags == flags)
                .ok_or(EINVAL)?;
            cursor = end.min(binding.end());
        }
        Ok(())
    }
    fn snapshot(&self) -> Result<super::g17p_compute_runtime::Client> {
        let mut root = UserVm::new()?;
        let mut buffers = KVec::new();
        for binding in &self.bindings {
            buffers.push(binding.bo.clone(), GFP_KERNEL)?;
            root.prepare(binding.start, binding.size)?;
            let mut pages = KVec::new();
            let extent = if binding.flags & SINGLE != 0 {
                PAGE
            } else {
                binding.size
            };
            pages.reserve((extent / PAGE) as usize, GFP_KERNEL)?;
            let mut cursor = 0;
            for entry in binding.bo.sg_table()?.iter() {
                let pa = entry.dma_address();
                let length = entry.dma_len() as u64;
                if (pa | length) & (PAGE - 1) != 0 {
                    return Err(EINVAL);
                }
                for offset in (0..length).step_by(PAGE as usize) {
                    if cursor >= binding.offset && cursor < binding.offset + extent {
                        pages.push(pa + offset, GFP_KERNEL)?;
                    }
                    cursor += PAGE;
                }
            }
            if pages.len() as u64 != extent / PAGE {
                return Err(EIO);
            }
            for index in 0..binding.size / PAGE {
                let pa = pages[if binding.flags & SINGLE != 0 {
                    0
                } else {
                    index as usize
                }];
                root.map_page(binding.start + index * PAGE, pa, binding.flags & WRITE != 0)?;
            }
        }
        Ok(super::g17p_compute_runtime::Client { root, buffers })
    }
    fn unbind(&mut self, start: u64, end: u64) -> Result {
        let mut next = KVec::with_capacity(
            self.bindings.len().checked_add(2).ok_or(EOVERFLOW)?,
            GFP_KERNEL,
        )?;
        for binding in &self.bindings {
            if binding.start < end && start < binding.end() {
                if binding.start < start {
                    next.push(binding.part(binding.start, start), GFP_KERNEL)?;
                }
                if end < binding.end() {
                    next.push(binding.part(end, binding.end()), GFP_KERNEL)?;
                }
            } else {
                next.push(binding.part(binding.start, binding.end()), GFP_KERNEL)?;
            }
        }
        self.tree.unmap(start, end - start)?;
        self.bindings = next;
        Ok(())
    }
}
struct Queue {
    id: u32,
    vm: u32,
    priority: u32,
}
struct Timestamp {
    id: u32,
    bo: ARef<Object>,
    offset: u64,
    size: u64,
}
struct State {
    vms: KVec<Vm>,
    queues: KVec<Queue>,
    objects: KVec<Timestamp>,
    next_vm: u32,
    next_queue: u32,
    next_object: u32,
}
#[pin_data]
pub(crate) struct File {
    id: u64,
    raw: AtomicPtr<bindings::drm_file>,
    #[pin]
    state: Mutex<State>,
}
static FILE_ID: AtomicU64 = AtomicU64::new(1);
impl drm::file::DriverFile for File {
    type Driver = Driver;
    fn open(_dev: &Device) -> Result<Pin<KBox<Self>>> {
        let id = FILE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |x| x.checked_add(1))
            .map_err(|_| EOVERFLOW)?;
        KBox::pin_init(
            try_pin_init!(Self {id,raw:AtomicPtr::new(core::ptr::null_mut()),state<-new_mutex!(State {
            vms:KVec::new(),queues:KVec::new(),objects:KVec::new(),next_vm:1,next_queue:1,next_object:1})}),
            GFP_KERNEL,
        )
    }
    fn post_open(&self, file: &DrmFile) {
        self.raw.store(file.as_raw(), Ordering::Release);
    }
    fn as_raw(&self) -> *mut bindings::drm_file {
        self.raw.load(Ordering::Acquire)
    }
}
impl File {
    fn get_params(dev: &Device, data: &uapi::drm_asahi_get_params, _file: &DrmFile) -> Result<u32> {
        if data.param_group != 0 || data.pad != 0 {
            return Err(EINVAL);
        }
        // The UAPI consists of consecutive integer fields, with no padding.
        const {
            assert!(core::mem::size_of::<uapi::drm_asahi_params_global>() == 592);
        }
        let size = (data.size as usize).min(core::mem::size_of_val(&dev.params));
        let mut writer = UserSlice::new(UserPtr::from_addr(data.pointer as usize), size).writer();
        // SAFETY: All 592 bytes are initialized, and size is bounded above.
        writer.write_slice(unsafe {
            core::slice::from_raw_parts(
                (&dev.params as *const uapi::drm_asahi_params_global).cast::<u8>(),
                size,
            )
        })?;
        Ok(0)
    }
    fn get_time(
        _dev: &Device,
        data: &mut uapi::drm_asahi_get_time,
        _file: &DrmFile,
    ) -> Result<u32> {
        if data.flags != 0 {
            return Err(EINVAL);
        }
        let raw: u64;
        let hz: u64;
        // SAFETY: Read-only architectural counter and its frequency.
        unsafe {
            core::arch::asm!("mrs {raw}, cntpct_el0","mrs {hz}, cntfrq_el0",raw=out(reg)raw,hz=out(reg)hz,options(nostack,preserves_flags));
        }
        if hz == 0 || hz > u32::MAX as u64 {
            return Err(EIO);
        }
        data.gpu_timestamp = (raw / hz)
            .checked_mul(1_000_000_000)
            .and_then(|whole| whole.checked_add((raw % hz) * 1_000_000_000 / hz))
            .ok_or(EOVERFLOW)?;
        Ok(0)
    }
    fn vm_create(
        dev: &Device,
        data: &mut uapi::drm_asahi_vm_create,
        file: &DrmFile,
    ) -> Result<u32> {
        if data.pad != 0
            || (data.kernel_start | data.kernel_end) & (PAGE - 1) != 0
            || data.kernel_start < VM_START
            || data.kernel_end > VM_END
            || data
                .kernel_end
                .checked_sub(data.kernel_start)
                .ok_or(EINVAL)?
                < VM_KERNEL_MIN
        {
            return Err(EINVAL);
        }
        let inner = file.inner();
        let mut state = inner.state.lock();
        let id = state.next_vm;
        let next = id.checked_add(1).ok_or(EOVERFLOW)?;
        let tree = UserVm::new()?;
        let resv = Object::new(
            dev,
            PAGE as usize,
            shmem::ObjectConfig {
                map_wc: false,
                parent_resv_obj: None,
            },
            (file.inner().id, id),
        )?;
        let root = tree.root();
        state.vms.push(
            Vm {
                id,
                kernel_start: data.kernel_start,
                kernel_end: data.kernel_end,
                tree,
                resv,
                bindings: KVec::new(),
            },
            GFP_KERNEL,
        )?;
        state.next_vm = next;
        data.vm_id = id;
        dev_info!(
            dev.as_ref(),
            "G17P: file {} VM {} owns unpublished UAT root {:#x}\n",
            file.inner().id,
            id,
            root
        );
        Ok(0)
    }
    fn vm_destroy(_dev: &Device, data: &uapi::drm_asahi_vm_destroy, file: &DrmFile) -> Result<u32> {
        if data.pad != 0 {
            return Err(EINVAL);
        }
        let inner = file.inner();
        let mut state = inner.state.lock();
        let index = state
            .vms
            .iter()
            .position(|vm| vm.id == data.vm_id)
            .ok_or(ENOENT)?;
        if state.queues.iter().any(|q| q.vm == data.vm_id) || !state.vms[index].bindings.is_empty()
        {
            return Err(EBUSY);
        }
        state.vms.swap_remove(index);
        Ok(0)
    }
    fn gem_create(
        dev: &Device,
        data: &mut uapi::drm_asahi_gem_create,
        file: &DrmFile,
    ) -> Result<u32> {
        if data.pad != 0
            || data.flags & !(WRITEBACK | PRIVATE) != 0
            || data.size == 0
            || data.flags & PRIVATE == 0 && data.vm_id != 0
        {
            return Err(EINVAL);
        }
        let size: usize = data
            .size
            .checked_add(PAGE - 1)
            .ok_or(EINVAL)?
            .try_into()
            .map_err(|_| EINVAL)?;
        let size = size & !(PAGE as usize - 1);
        let inner = file.inner();
        let state = inner.state.lock();
        let parent = if data.flags & PRIVATE != 0 {
            Some(
                &*state
                    .vms
                    .iter()
                    .find(|vm| vm.id == data.vm_id)
                    .ok_or(ENOENT)?
                    .resv,
            )
        } else {
            None
        };
        let object = Object::new(
            dev,
            size,
            shmem::ObjectConfig {
                map_wc: data.flags & WRITEBACK == 0,
                parent_resv_obj: parent,
            },
            (file.inner().id, data.vm_id),
        )?;
        drop(state);
        data.handle = object.create_handle(file)?;
        Ok(0)
    }
    fn gem_mmap_offset(
        _dev: &Device,
        data: &mut uapi::drm_asahi_gem_mmap_offset,
        file: &DrmFile,
    ) -> Result<u32> {
        if data.flags != 0 {
            return Err(EINVAL);
        }
        let object = Object::lookup_handle(file, data.handle)?;
        data.offset = object.create_mmap_offset()?;
        Ok(0)
    }
    fn vm_bind(_dev: &Device, data: &uapi::drm_asahi_vm_bind, file: &DrmFile) -> Result<u32> {
        let stride = data.stride as usize;
        let op_size = core::mem::size_of::<uapi::drm_asahi_gem_bind_op>();
        if data.pad != 0 || stride < op_size {
            return Err(EINVAL);
        }
        let size = stride.checked_mul(data.num_binds as usize).ok_or(EINVAL)?;
        let mut bytes = KVVec::new();
        UserSlice::new(UserPtr::from_addr(data.userptr as usize), size)
            .reader()
            .read_all(&mut bytes, GFP_KERNEL)?;
        let inner = file.inner();
        let mut state = inner.state.lock();
        let vm = state
            .vms
            .iter_mut()
            .find(|vm| vm.id == data.vm_id)
            .ok_or(ENOENT)?;
        for raw in bytes.chunks_exact(stride) {
            if raw[op_size..].iter().any(|b| *b != 0) {
                return Err(E2BIG);
            }
            // SAFETY: Checked full-sized copied user input. This UAPI is only
            // integers (every bit pattern valid); alignment is not assumed.
            let op = unsafe {
                core::ptr::read_unaligned(raw.as_ptr().cast::<uapi::drm_asahi_gem_bind_op>())
            };
            let end = vm.range(op.addr, op.range)?;
            if op.offset & (PAGE - 1) != 0 {
                return Err(EINVAL);
            }
            if op.flags & UNBIND != 0 {
                if op.flags != UNBIND || op.handle != 0 || op.offset != 0 {
                    return Err(EINVAL);
                }
                vm.unbind(op.addr, end)?;
                continue;
            }
            if op.flags & !(READ | WRITE | SINGLE) != 0
                || op.flags & (READ | WRITE) == 0
                || vm
                    .bindings
                    .iter()
                    .any(|b| op.addr < b.end() && b.start < end)
            {
                return Err(EINVAL);
            }
            let object = Object::lookup_handle(file, op.handle)?;
            if object.owner_vm != 0
                && (object.owner_vm != vm.id || object.owner_file != file.inner().id)
            {
                return Err(EINVAL);
            }
            let accessed = if op.flags & SINGLE != 0 {
                PAGE
            } else {
                op.range
            };
            if op.offset.checked_add(accessed).ok_or(EINVAL)? > object.size() as u64 {
                return Err(EINVAL);
            }
            // Pin the real shmem pages, preserving their actual DMA addresses.
            let mut pages = KVec::new();
            pages.reserve((accessed / PAGE).try_into()?, GFP_KERNEL)?;
            let mut offset = 0u64;
            for entry in object.sg_table()?.iter() {
                let base = entry.dma_address();
                let len = entry.dma_len() as u64;
                if (base | len) & (PAGE - 1) != 0 {
                    return Err(EINVAL);
                }
                for index in (0..len).step_by(PAGE as usize) {
                    if offset >= op.offset && offset < op.offset + accessed {
                        pages.push(base + index, GFP_KERNEL)?;
                    }
                    offset = offset.checked_add(PAGE).ok_or(EINVAL)?;
                }
            }
            if pages.len() as u64 != accessed / PAGE {
                return Err(EIO);
            }
            vm.bindings.reserve(1, GFP_KERNEL)?;
            vm.tree.prepare(op.addr, op.range)?;
            for index in 0..op.range / PAGE {
                let pa = pages[if op.flags & SINGLE != 0 {
                    0
                } else {
                    index as usize
                }];
                if let Err(error) =
                    vm.tree
                        .map_page(op.addr + index * PAGE, pa, op.flags & WRITE != 0)
                {
                    vm.tree.unmap(op.addr, op.range)?;
                    return Err(error);
                }
            }
            vm.bindings.push(
                Binding {
                    start: op.addr,
                    size: op.range,
                    offset: op.offset,
                    flags: op.flags,
                    bo: object,
                },
                GFP_KERNEL,
            )?;
        }
        Ok(0)
    }
    fn gem_bind_object(
        _dev: &Device,
        data: &mut uapi::drm_asahi_gem_bind_object,
        file: &DrmFile,
    ) -> Result<u32> {
        if data.vm_id != 0 || data.pad != 0 {
            return Err(EINVAL);
        }
        let inner = file.inner();
        let mut state = inner.state.lock();
        if data.op == uapi::drm_asahi_bind_object_op_DRM_ASAHI_BIND_OBJECT_OP_UNBIND {
            if data.flags != 0 || data.handle != 0 || data.offset != 0 || data.range != 0 {
                return Err(EINVAL);
            }
            let index = state
                .objects
                .iter()
                .position(|o| o.id == data.object_handle)
                .ok_or(ENOENT)?;
            state.objects.swap_remove(index);
            return Ok(0);
        }
        if data.op != uapi::drm_asahi_bind_object_op_DRM_ASAHI_BIND_OBJECT_OP_BIND
            || data.flags
                != uapi::drm_asahi_bind_object_flags_DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS
        {
            return Err(EINVAL);
        }
        let bo = Object::lookup_handle(file, data.handle)?;
        if data.range == 0
            || (data.offset | data.range) & (PAGE - 1) != 0
            || data.offset.checked_add(data.range).ok_or(EINVAL)? > bo.size() as u64
        {
            return Err(EINVAL);
        }
        let id = state.next_object;
        let next = id.checked_add(1).ok_or(EOVERFLOW)?;
        state.objects.push(
            Timestamp {
                id,
                bo,
                offset: data.offset,
                size: data.range,
            },
            GFP_KERNEL,
        )?;
        state.next_object = next;
        data.object_handle = id;
        Ok(0)
    }
    fn queue_create(
        _dev: &Device,
        data: &mut uapi::drm_asahi_queue_create,
        file: &DrmFile,
    ) -> Result<u32> {
        // Enforce the operator's immutable execution base before allocation.
        if data.usc_exec_base != USC_EXEC_BASE || data.flags != 0 || data.priority > 1 {
            return Err(EINVAL);
        }
        let inner = file.inner();
        let mut state = inner.state.lock();
        let vm = state
            .vms
            .iter()
            .find(|vm| vm.id == data.vm_id)
            .ok_or(ENOENT)?;
        vm.range(USC_EXEC_BASE, 1 << 32)?;
        let id = state.next_queue;
        let next = id.checked_add(1).ok_or(EOVERFLOW)?;
        state.queues.push(
            Queue {
                id,
                vm: data.vm_id,
                priority: data.priority,
            },
            GFP_KERNEL,
        )?;
        state.next_queue = next;
        data.queue_id = id;
        Ok(0)
    }
    fn queue_destroy(
        _dev: &Device,
        data: &uapi::drm_asahi_queue_destroy,
        file: &DrmFile,
    ) -> Result<u32> {
        if data.pad != 0 {
            return Err(EINVAL);
        }
        let inner = file.inner();
        let mut state = inner.state.lock();
        let index = state
            .queues
            .iter()
            .position(|q| q.id == data.queue_id)
            .ok_or(ENOENT)?;
        state.queues.swap_remove(index);
        Ok(0)
    }
    fn submit(dev: &Device, data: &uapi::drm_asahi_submit, file: &DrmFile) -> Result<u32> {
        use super::g17p_compute_runtime as compute;
        if data.flags != 0 || data.pad != 0 || data.cmdbuf_size == 0 {
            return Err(EINVAL);
        }
        if data.in_sync_count != 0 || data.out_sync_count != 0 {
            return Err(Error::from_errno(-(bindings::EOPNOTSUPP as i32)));
        }
        let mut bytes = KVVec::new();
        UserSlice::new(
            UserPtr::from_addr(data.cmdbuf as usize),
            data.cmdbuf_size as usize,
        )
        .reader()
        .read_all(&mut bytes, GFP_KERNEL)?;
        let inner = file.inner();
        let state = inner.state.lock();
        let queue = state
            .queues
            .iter()
            .find(|q| q.id == data.queue_id)
            .ok_or(ENOENT)?;
        let vm = state.vms.iter().find(|v| v.id == queue.vm).ok_or(ENOENT)?;
        let mut offset = 0;
        let mut command = None;
        while offset < bytes.len() {
            let header = bytes.get(offset..offset + 8).ok_or(EINVAL)?;
            let kind = u16::from_le_bytes(header[0..2].try_into().unwrap());
            let size = u16::from_le_bytes(header[2..4].try_into().unwrap()) as usize;
            let barriers = [
                u16::from_le_bytes(header[4..6].try_into().unwrap()),
                u16::from_le_bytes(header[6..8].try_into().unwrap()),
            ];
            offset += 8;
            let payload = bytes.get(offset..offset + size).ok_or(EINVAL)?;
            offset += size;
            if kind == 4 {
                if barriers != [u16::MAX; 2] || size % 24 != 0 || size / 24 > 16 {
                    return Err(EINVAL);
                }
                for row in payload.chunks_exact(24) {
                    let address = u64::from_le_bytes(row[0..8].try_into().unwrap());
                    let extent = u64::from_le_bytes(row[8..16].try_into().unwrap());
                    if row[16..].iter().any(|v| *v != 0) {
                        return Err(EINVAL);
                    }
                    vm.cover(address, extent, WRITE)?;
                }
                continue;
            }
            if kind != 1 || command.is_some() {
                return Err(Error::from_errno(-(bindings::EOPNOTSUPP as i32)));
            }
            if barriers.iter().any(|b| *b != 0 && *b != u16::MAX) {
                return Err(EINVAL);
            }
            if payload
                .get(64..)
                .unwrap_or_default()
                .iter()
                .any(|b| *b != 0)
            {
                return Err(E2BIG);
            }
            let mut body = [0u8; 64];
            let size = size.min(64);
            body[..size].copy_from_slice(&payload[..size]);
            // SAFETY: Every integer bit pattern is valid; UAPI size is checked.
            const {
                assert!(core::mem::size_of::<uapi::drm_asahi_cmd_compute>() == 64);
            }
            let cmd = unsafe {
                core::ptr::read_unaligned(body.as_ptr().cast::<uapi::drm_asahi_cmd_compute>())
            };
            if cmd.flags != 0
                || cmd.helper.binary != 0
                || cmd.helper.cfg != 0
                || cmd.helper.data != 0
            {
                return Err(EINVAL);
            }
            if cmd.ts.start.handle != 0 || cmd.ts.end.handle != 0 {
                return Err(Error::from_errno(-(bindings::EOPNOTSUPP as i32)));
            }
            let base = cmd.cdm_ctrl_stream_base;
            let end = cmd.cdm_ctrl_stream_end;
            if (base | end) & 3 != 0
                || end <= base
                || cmd.sampler_count > 1024
                || (cmd.sampler_heap == 0) != (cmd.sampler_count == 0)
                || cmd.sampler_heap & 7 != 0
            {
                return Err(EINVAL);
            }
            vm.cover(base, end - base, READ)?;
            if cmd.sampler_count != 0 {
                vm.cover(cmd.sampler_heap, cmd.sampler_count as u64 * 8, READ)?;
            }
            command = Some(compute::Parameters {
                preempt: vm.kernel_start,
                cdm: base,
                end,
                sampler: cmd.sampler_heap,
                sampler_count: cmd.sampler_count,
            });
        }
        let parameters = command.ok_or(EINVAL)?;
        let mut runtime = dev.runtime.lock();
        let runtime = Option::as_mut(&mut *runtime).ok_or(ENODEV)?;
        runtime.session.require_first_work()?;
        let client = vm.snapshot()?;
        runtime
            .session
            .submit_compute(dev.as_ref(), &runtime.image, client, &parameters)?;
        Ok(0)
    }
}
