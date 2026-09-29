// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Native Asahi memory UAPI and synchronous compute submissions.
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
    fn snapshot(
        &self,
        owner: (u64, u32),
        render: bool,
    ) -> Result<super::g17p_compute_runtime::Client> {
        let mut root = UserVm::new()?;
        let mut buffers = KVec::new();
        let mut bindings = KVec::new();
        for binding in &self.bindings {
            buffers.push(binding.bo.clone(), GFP_KERNEL)?;
            bindings.push(
                (binding.start, binding.size, binding.offset, binding.flags),
                GFP_KERNEL,
            )?;
            let address = if render && binding.start < 0x1000000000 {
                binding.start + 0x1000000000
            } else {
                binding.start
            };
            root.prepare(address, binding.size)?;
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
                root.map_page(address + index * PAGE, pa, binding.flags & WRITE != 0)?;
            }
        }
        Ok(super::g17p_compute_runtime::Client {
            root,
            buffers,
            bindings,
            owner,
        })
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
enum Command {
    Compute(super::g17p_compute_runtime::Parameters),
    Render(super::g17p_render::Parameters),
}

struct Queue {
    id: u32,
    vm: u32,
    priority: u32,
}
struct Timestamp {
    id: u32,
    address: u64,
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
        dev: &Device,
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
        state.objects.reserve(1, GFP_KERNEL)?;
        let mut runtime = dev.runtime.lock();
        let address = Option::as_mut(&mut *runtime)
            .ok_or(ENODEV)?
            .session
            .bind_timestamp(bo.clone(), data.offset, data.range)?;
        state.objects.push(
            Timestamp {
                id,
                address,
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
    fn render_parameters(
        vm: &Vm,
        objects: &[Timestamp],
        payload: &[u8],
        fragment_count: usize,
    ) -> Result<super::g17p_render::Parameters> {
        if payload
            .get(240..)
            .unwrap_or_default()
            .iter()
            .any(|b| *b != 0)
        {
            return Err(E2BIG);
        }
        let mut body = [0u8; 240];
        let size = payload.len().min(body.len());
        body[..size].copy_from_slice(&payload[..size]);
        const {
            assert!(core::mem::size_of::<uapi::drm_asahi_cmd_render>() == 240);
        }
        // SAFETY: The copied ABI body contains integer fields only.
        let cmd = unsafe {
            core::ptr::read_unaligned(body.as_ptr().cast::<uapi::drm_asahi_cmd_render>())
        };
        if cmd.flags & !(2 | (1 << 18)) != 0
            || cmd.vertex_helper.binary != 0
            || cmd.vertex_helper.cfg != 0
            || cmd.vertex_helper.data != 0
            || cmd.fragment_helper.binary != 0
            || cmd.fragment_helper.cfg != 0
            || cmd.fragment_helper.data != 0
            || !(1..=16384).contains(&cmd.width_px)
            || !(1..=16384).contains(&cmd.height_px)
            || !(1..=2048).contains(&cmd.layers)
            || !matches!(
                (cmd.utile_width_px, cmd.utile_height_px),
                (32, 32) | (32, 16) | (16, 16)
            )
        {
            return Err(EINVAL);
        }
        let sample_bits = match cmd.samples {
            1 => 0,
            2 => 1,
            4 => 2,
            _ => return Err(EINVAL),
        };
        let utile_bytes = cmd.sample_size_B as u64
            * cmd.utile_width_px as u64
            * cmd.utile_height_px as u64
            * cmd.samples as u64;
        if utile_bytes > 32768 || cmd.vdm_ctrl_stream_base == 0 || cmd.vdm_ctrl_stream_base & 3 != 0
        {
            return Err(EINVAL);
        }
        vm.cover(cmd.vdm_ctrl_stream_base, 4, READ)?;
        for (address, flags) in [
            (cmd.isp_scissor_base, READ),
            (cmd.isp_dbias_base, READ),
            (cmd.isp_oclqry_base, WRITE),
        ] {
            if address != 0 {
                if address & 7 != 0 {
                    return Err(EINVAL);
                }
                vm.cover(address, 8, flags)?;
            }
        }
        if cmd.isp_scissor_base == 0 {
            return Err(EINVAL);
        }
        for zls in [&cmd.depth, &cmd.stencil] {
            if (zls.base == 0 && (zls.comp_base != 0 || zls.stride != 0 || zls.comp_stride != 0))
                || (zls.comp_base == 0 && zls.comp_stride != 0)
                || (cmd.layers > 1 && zls.base != 0 && zls.stride == 0)
                || (zls.stride != 0 && zls.stride & 0x3fff != 1)
                || zls.comp_stride & 0x3fff != 0
            {
                return Err(EINVAL);
            }
            if zls.base != 0 {
                let stride = if zls.stride != 0 {
                    ((zls.stride as u64 >> 14) + 1) * PAGE
                } else {
                    0
                };
                vm.cover(zls.base, stride * (cmd.layers as u64 - 1) + 1, READ | WRITE)?;
            }
            if zls.comp_base != 0 {
                vm.cover(
                    zls.comp_base,
                    ((zls.comp_stride as u64 >> 14) + 1) * 0x80 * (cmd.layers as u64 - 1) + 1,
                    READ | WRITE,
                )?;
            }
        }
        if cmd.sampler_count > 1024
            || (cmd.sampler_heap == 0) != (cmd.sampler_count == 0)
            || cmd.sampler_heap & 7 != 0
        {
            return Err(EINVAL);
        }
        if cmd.sampler_count != 0 {
            vm.cover(cmd.sampler_heap, cmd.sampler_count as u64 * 8, READ)?;
        }
        for program in [&cmd.bg, &cmd.eot, &cmd.partial_bg, &cmd.partial_eot] {
            if program.usc == 0 {
                return Err(EINVAL);
            }
            vm.cover(USC_EXEC_BASE + (program.usc as u64 & !7), 4, READ)?;
        }
        let resolve = |ts: &uapi::drm_asahi_timestamp| -> Result<u64> {
            if ts.handle == 0 {
                return if ts.offset == 0 { Ok(0) } else { Err(EINVAL) };
            }
            let object = objects.iter().find(|o| o.id == ts.handle).ok_or(ENOENT)?;
            let offset = ts.offset as u64;
            if offset & 7 != 0 || offset + 8 > object.size {
                return Err(EINVAL);
            }
            Ok(object.address + offset)
        };
        let mut p = super::g17p_render_runtime::first_parameters();
        p.width = cmd.width_px as u64;
        p.height = cmd.height_px as u64;
        p.layers = cmd.layers as u64;
        p.encoder = if cmd.vdm_ctrl_stream_base < 0x1000000000 {
            cmd.vdm_ctrl_stream_base + 0x1000000000
        } else {
            cmd.vdm_ctrl_stream_base
        };
        p.scissor_array = cmd.isp_scissor_base;
        p.depth_bias_array = cmd.isp_dbias_base;
        p.occlusion_query_base = cmd.isp_oclqry_base;
        p.depth_buffer = cmd.depth.base;
        p.depth_aux_buffer = cmd.depth.comp_base;
        p.depth_stride = cmd.depth.stride as u64;
        p.depth_aux_stride = cmd.depth.comp_stride as u64;
        p.stencil_buffer = cmd.stencil.base;
        p.stencil_aux_buffer = cmd.stencil.comp_base;
        p.stencil_stride = cmd.stencil.stride as u64;
        p.stencil_aux_stride = cmd.stencil.comp_stride as u64;
        p.depth_flags = cmd.zls_ctrl;
        p.depth_dimensions = cmd.isp_zls_pixels as u64;
        p.multisample_control = cmd.ppp_multisamplectl;
        p.ppp_control = cmd.ppp_ctrl as u64;
        p.utile_width = cmd.utile_width_px as u64;
        p.utile_height = cmd.utile_height_px as u64;
        p.samples = cmd.samples as u64;
        p.sample_size = cmd.sample_size_B as u64;
        p.utile_config = ((p.utile_width / 16) << 12) | ((p.utile_height / 16) << 14) | sample_bits;
        p.tib_blocks = utile_bytes.div_ceil(2048);
        p.process_empty_tiles = cmd.flags & 2 != 0;
        p.tile_config =
            0x280 | u64::from(p.layers > 1) | if p.process_empty_tiles { 0x10000 } else { 0 };
        p.merge_upper_x_bits = cmd.isp_merge_upper_x as u64;
        p.merge_upper_y_bits = cmd.isp_merge_upper_y as u64;
        p.store_pipeline = USC_EXEC_BASE + cmd.eot.usc as u64;
        p.store_pipeline_bind = cmd.eot.rsrc_spec as u64;
        p.load_pipeline = USC_EXEC_BASE + cmd.bg.usc as u64;
        p.load_pipeline_bind = 0xffff800000000000 | cmd.bg.rsrc_spec as u64;
        p.partial_store_pipeline = USC_EXEC_BASE + cmd.partial_eot.usc as u64;
        p.partial_store_pipeline_bind = cmd.partial_eot.rsrc_spec as u64;
        p.partial_load_pipeline = USC_EXEC_BASE + cmd.partial_bg.usc as u64;
        p.partial_load_pipeline_bind = 0xffff800000000000 | cmd.partial_bg.rsrc_spec as u64;
        p.depth_clear_value_bits = cmd.isp_bgobjdepth as u64;
        p.stencil_clear_value = cmd.isp_bgobjvals as u64;
        p.sampler_array = cmd.sampler_heap;
        p.sampler_count = cmd.sampler_count as u64;
        p.aux_fb_flags =
            (if fragment_count > 1 { 0xc000 } else { 0xc001 }) | (cmd.flags as u64 & (1 << 18));
        p.ta_user_timestamp_start = resolve(&cmd.ts_vtx.start)?;
        p.ta_user_timestamp_end = resolve(&cmd.ts_vtx.end)?;
        p.fragment_user_timestamp_start = resolve(&cmd.ts_frag.start)?;
        p.fragment_user_timestamp_end = resolve(&cmd.ts_frag.end)?;
        p.validate().map_err(|_| EINVAL)?;
        Ok(p)
    }

    fn parameters(vm: &Vm, objects: &[Timestamp], bytes: &[u8]) -> Result<KVec<Command>> {
        use super::g17p_compute_runtime as compute;
        let mut offset = 0;
        let mut commands = KVec::new();
        let mut counts = [0usize; 2];
        let mut fragment_count = 0;
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
            if matches!(kind, 2 | 3 | 4) {
                if kind == 3 {
                    fragment_count = size / 24;
                }
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
            if kind > 1 {
                return Err(Error::from_errno(-(bindings::EOPNOTSUPP as i32)));
            }
            if commands.len() == 64
                || (barriers[0] != u16::MAX && barriers[0] as usize > counts[0])
                || (barriers[1] != u16::MAX && barriers[1] as usize > counts[1])
            {
                return Err(EINVAL);
            }
            counts[kind as usize] += 1;
            if kind == 0 {
                commands.push(
                    Command::Render(Self::render_parameters(
                        vm,
                        objects,
                        payload,
                        fragment_count,
                    )?),
                    GFP_KERNEL,
                )?;
                continue;
            }
            // All previous commands finish before the next one starts in
            // this synchronous port, satisfying each validated barrier.
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
            let resolve = |ts: &uapi::drm_asahi_timestamp| -> Result<u64> {
                if ts.handle == 0 {
                    return if ts.offset == 0 { Ok(0) } else { Err(EINVAL) };
                }
                let object = objects.iter().find(|o| o.id == ts.handle).ok_or(ENOENT)?;
                let offset = ts.offset as u64;
                if offset & 7 != 0 || offset.checked_add(8).ok_or(EINVAL)? > object.size {
                    return Err(EINVAL);
                }
                Ok(object.address + offset)
            };
            let timestamps = [resolve(&cmd.ts.start)?, resolve(&cmd.ts.end)?];
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
            commands.push(
                Command::Compute(compute::Parameters {
                    preempt: vm.kernel_start,
                    cdm: base,
                    end,
                    sampler: cmd.sampler_heap,
                    sampler_count: cmd.sampler_count,
                    timestamps,
                }),
                GFP_KERNEL,
            )?;
        }
        if commands.is_empty() {
            return Err(EINVAL);
        }
        Ok(commands)
    }
    fn submit(dev: &Device, data: &uapi::drm_asahi_submit, file: &DrmFile) -> Result<u32> {
        if data.flags != 0
            || data.pad != 0
            || data.cmdbuf_size == 0
            || data.cmdbuf_size > 1024 * 1024
        {
            return Err(EINVAL);
        }
        let sync = super::g17p_sync::Plan::read(file, data)?;
        let mut bytes = KVVec::new();
        UserSlice::new(
            UserPtr::from_addr(data.cmdbuf.try_into()?),
            data.cmdbuf_size as usize,
        )
        .reader()
        .read_all(&mut bytes, GFP_KERNEL)?;
        let inner = file.inner();
        {
            let state = inner.state.lock();
            let queue = state
                .queues
                .iter()
                .find(|q| q.id == data.queue_id)
                .ok_or(ENOENT)?;
            let vm = state.vms.iter().find(|v| v.id == queue.vm).ok_or(ENOENT)?;
            Self::parameters(vm, &state.objects, &bytes)?;
        }
        // No file/runtime lock across waits. A concurrent producer may need
        // this very file. Revalidate mappings after the wait before staging.
        sync.wait_inputs()?;
        let state = inner.state.lock();
        let queue = state
            .queues
            .iter()
            .find(|q| q.id == data.queue_id)
            .ok_or(ENOENT)?;
        let vm = state.vms.iter().find(|v| v.id == queue.vm).ok_or(ENOENT)?;
        let parameters = Self::parameters(vm, &state.objects, &bytes)?;
        let mut runtime = dev.runtime.lock();
        let runtime = Option::as_mut(&mut *runtime).ok_or(ENODEV)?;
        if parameters.iter().any(|p| matches!(p, Command::Render(_))) {
            // Admit the entire synchronous render batch before publishing a
            // prefix. Each command completes before the next one's barriers.
            if parameters.iter().any(|p| !matches!(p, Command::Render(_))) {
                return Err(Error::from_errno(-(bindings::EOPNOTSUPP as i32)));
            }
            if parameters.len() > runtime.session.render_remaining()? as usize {
                return Err(Error::from_errno(-(bindings::EOPNOTSUPP as i32)));
            }
            let mut replacement = if let Some(client) = runtime.session.render_client()? {
                if client.owner != (inner.id, vm.id) {
                    return Err(Error::from_errno(-(bindings::EOPNOTSUPP as i32)));
                }
                if client.bindings.len() != vm.bindings.len()
                    || !vm.bindings.iter().enumerate().all(|(i, b)| {
                        client.bindings[i] == (b.start, b.size, b.offset, b.flags)
                            && core::ptr::eq(&*client.buffers[i], &*b.bo)
                    })
                {
                    Some(vm.snapshot((inner.id, vm.id), true)?)
                } else {
                    None
                }
            } else {
                None
            };
            for command in &parameters {
                let Command::Render(render) = command else {
                    return Err(EINVAL);
                };
                if runtime.session.render_client()?.is_some() {
                    runtime.session.submit_next_render(
                        dev.as_ref(),
                        &runtime.image,
                        replacement.take(),
                        render,
                    )?;
                } else {
                    let client = vm.snapshot((inner.id, vm.id), true)?;
                    runtime
                        .session
                        .submit_render(dev.as_ref(), &runtime.image, client, render)?;
                }
            }
            sync.complete();
            return Ok(0);
        }
        runtime.session.require_compute_owner((inner.id, vm.id))?;
        let mut replacement = if let Some(client) = runtime.session.compute_client()? {
            if client.owner != (inner.id, vm.id)
                || client.bindings.len() != vm.bindings.len()
                || !vm.bindings.iter().enumerate().all(|(i, b)| {
                    client.bindings[i] == (b.start, b.size, b.offset, b.flags)
                        && core::ptr::eq(&*client.buffers[i], &*b.bo)
                })
            {
                Some(vm.snapshot((inner.id, vm.id), false)?)
            } else {
                None
            }
        } else {
            None
        };
        // Admit the complete batch before its first publication. In
        // particular, exhaustion may never execute an accepted prefix.
        if parameters.len() > runtime.session.compute_remaining()? {
            return Err(Error::from_errno(-(bindings::EOPNOTSUPP as i32)));
        }
        for command in &parameters {
            let Command::Compute(parameters) = command else {
                return Err(EINVAL);
            };
            if runtime.session.compute_client()?.is_some() {
                runtime.session.submit_next_compute(
                    dev.as_ref(),
                    &runtime.image,
                    replacement.take(),
                    parameters,
                )?;
            } else {
                let client = vm.snapshot((inner.id, vm.id), false)?;
                runtime
                    .session
                    .submit_compute(dev.as_ref(), &runtime.image, client, parameters)?;
            }
        }
        sync.complete();
        Ok(0)
    }
}
