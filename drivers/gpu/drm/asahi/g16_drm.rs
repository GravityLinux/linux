// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from Niklas Sheth's linux-m4-integration prototype.

//! G16 DRM frontend. Firmware initialization and queue ownership stay in the
//! G16 runtime; GEM storage uses the common DRM shmem implementation.

use crate::{g16_compute as compute, g16_render as render};
use core::{
    ops::Range,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use kernel::dma_fence::{self, RawDmaFence};
use kernel::drm::sched;
use kernel::drm::{
    gem::{shmem, BaseObject, DriverObject},
    ioctl,
};
use kernel::sync::{Arc, CondVar, Mutex, MutexGuard};
use kernel::sync::aref::ARef;
use kernel::uaccess::{UserPtr, UserSlice};
use kernel::workqueue::{self, impl_has_work, new_work, Work, WorkItem};
use kernel::{bindings, c_str, device::{Bound, Core}, drm, platform, prelude::*, scatterlist, uapi};

enum Command {
    Render(uapi::drm_asahi_cmd_render),
    Compute(uapi::drm_asahi_cmd_compute),
}
#[derive(Clone, Copy)]
enum Parameters {
    Render(render::Parameters),
    Compute(compute::Parameters),
}

pub(crate) struct Driver;
pub(crate) type Device = drm::Device<Driver>;
pub(crate) type DeviceRef = ARef<Device>;
pub(crate) type RegDevice = drm::Device<Driver, drm::Registered>;

/// Registration-scoped data: the bound parent device, for DMA mappings.
pub(crate) struct RegData<'a> {
    pdev: &'a platform::Device<Bound>,
}
type DrmFile = drm::File<File>;
type Object = shmem::Object<Buffer>;
const VM_END: u64 = (1 << 42) - 0x8000;
static NEXT_FILE: AtomicU64 = AtomicU64::new(1);
static NEXT_EXECUTION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct Mapping {
    range: Range<u64>,
    permissions: u32,
    offset: u64,
    single: bool,
    obj: ARef<Object>,
}

impl Mapping {
    fn cover(mappings: &[Self], start: u64, size: u64, permissions: u32) -> Result {
        let end = start.checked_add(size).ok_or(EINVAL)?;
        if size == 0 {
            return Err(EINVAL);
        }
        // `mappings` is sorted by address and never overlaps, so the only
        // candidate for `cursor` is the first mapping ending after it.
        let mut cursor = start;
        while cursor < end {
            let index = mappings.partition_point(|m| m.range.end <= cursor);
            let mapping = mappings
                .get(index)
                .filter(|m| m.range.contains(&cursor) && m.permissions & permissions == permissions)
                .ok_or(EINVAL)?;
            cursor = end.min(mapping.range.end);
        }
        Ok(())
    }
}

struct Vm {
    id: u32,
    visible: bool,
    kernel: Range<u64>,
    // Tables must be removed before the references keeping GEM pages pinned.
    space: Arc<Mutex<crate::g16_vm::AddressSpace>>,
    compute_space: Arc<Mutex<crate::g16_vm::AddressSpace>>,
    // Large address spaces hold thousands of mappings; allow vmalloc.
    // Sorted by address; mappings never overlap.
    mappings: KVVec<Mapping>,
    /// Shared copy of `mappings` retained by submissions. Rebuilt only after
    /// the mappings change, so a submit costs one reference, not one per
    /// mapping.
    snapshot: Option<Arc<KVVec<Mapping>>>,
    pending: KVec<dma_fence::Fence>,
    // Count overlapping drains. Submit to this VM sleeps until all finish;
    // unrelated file ioctls and other VMs can still acquire file state.
    draining: u32,
    reservation: ARef<Object>,
    render_support: Option<Arc<crate::g16::Support>>,
    render_ready: bool,
    compute_ready: bool,
    render_broken: bool,
}

/// Retain submit-time GEM and timestamp bindings until actual completion.
/// Command preparation never reads caller CDM or shader resource data.
struct Bindings {
    _mappings: Arc<KVVec<Mapping>>,
    _timestamps: KVec<TimestampBuffer>,
}

struct FileState {
    next_vm: u32,
    next_queue: u32,
    next_timestamp: u32,
    queues: KVec<Queue>,
    timestamps: KVec<TimestampBuffer>,
    retained_timestamps: KVec<TimestampBuffer>,
    vms: KVec<Vm>,
}

struct Queue {
    id: u32,
    vm: u32,
    entity: sched::Entity<SubmissionJob>,
    firmware: Arc<Mutex<crate::g16::FirmwareQueues>>,
}
#[derive(Clone)]
struct TimestampBuffer {
    id: u32,
    /// Keeps the object, and so its pinned pages and DMA mapping, alive while mapped.
    _obj: ARef<Object>,
    base: u64,
    extent: u64,
    address: u64,
    size: u64,
}

#[pin_data]
pub(crate) struct Data {
    gpu_revision: u32,
    core_mask: u32,
    maximum_frequency_khz: u32,
    scheduler: sched::Scheduler<SubmissionJob>,
    #[pin]
    admission: Mutex<KVec<dma_fence::Fence>>,
    failed: AtomicBool,
    notifications: Arc<crate::g16::Notifications>,
    /// Debug view of the firmware's memory; declared before `runtime` so it
    /// is removed before the firmware regions can be freed.
    _fw_mem: Pin<KBox<kernel::debugfs::File<crate::g16_timeline::FwMem>>>,
    #[pin]
    runtime: Mutex<crate::g16::Bootstrap>,
    #[pin]
    engine: Mutex<Engine>,
    /// Jobs drm_sched has run but the pump has not yet taken. Scheduler
    /// callbacks hold only this lock, briefly, never `engine`: the pump
    /// keeps `engine` through a whole publication pass.
    #[pin]
    inbox: Mutex<Inbox>,
    /// timeline=1/2: the record ring and its debugfs file (dropped before the
    /// directory).
    timeline: Option<Pin<KBox<kernel::debugfs::File<crate::g16_timeline::Ring>>>>,
    _profile: Pin<KBox<kernel::debugfs::File<crate::g16_timeline::Profile>>>,
    _maint: Pin<KBox<kernel::debugfs::File<crate::g16_timeline::MaintKnob>>>,
    _queue_cap: Pin<KBox<kernel::debugfs::File<crate::g16_timeline::CapKnob>>>,
    _flags: KVec<Pin<KBox<kernel::debugfs::File<crate::g16_timeline::FlagKnob>>>>,
    _dep_stats: Pin<KBox<kernel::debugfs::File<crate::g16_timeline::DepStats>>>,
    _lane_cap: Pin<KBox<kernel::debugfs::File<crate::g16_timeline::CapKnob>>>,
    _debugfs: kernel::debugfs::Dir,
}

#[vtable]
impl drm::driver::Driver for Driver {
    type Data = Data;
    type RegistrationData<'a> = RegData<'a>;
    type File = File;
    type Object = Object;
    type ParentDevice<Ctx: kernel::device::DeviceContext> = platform::Device<Ctx>;
    const INFO: drm::driver::DriverInfo = drm::driver::DriverInfo {
        major: 0,
        minor: 0,
        patchlevel: 0,
        name: c_str!("asahi"),
        desc: c_str!("Apple G16 Graphics"),
    };
    const FEAT_RENDER: bool = true;
    const FEAT_SYNCOBJ: bool = true;
    const FEAT_SYNCOBJ_TIMELINE: bool = true;

    kernel::declare_drm_ioctls! {
        (ASAHI_GET_PARAMS, drm_asahi_get_params, ioctl::RENDER_ALLOW, File::get_params),
        (ASAHI_GET_TIME, drm_asahi_get_time, ioctl::RENDER_ALLOW, File::get_time),
        (ASAHI_VM_CREATE, drm_asahi_vm_create, ioctl::RENDER_ALLOW, File::vm_create),
        (ASAHI_VM_DESTROY, drm_asahi_vm_destroy, ioctl::RENDER_ALLOW, File::vm_destroy),
        (ASAHI_VM_BIND, drm_asahi_vm_bind, ioctl::RENDER_ALLOW, File::vm_bind),
        (ASAHI_GEM_CREATE, drm_asahi_gem_create, ioctl::RENDER_ALLOW, File::gem_create),
        (ASAHI_GEM_MMAP_OFFSET, drm_asahi_gem_mmap_offset, ioctl::RENDER_ALLOW, File::gem_mmap_offset),
        (ASAHI_GEM_BIND_OBJECT, drm_asahi_gem_bind_object, ioctl::RENDER_ALLOW, File::bind_object),
        (ASAHI_QUEUE_CREATE, drm_asahi_queue_create, ioctl::RENDER_ALLOW, File::queue_create),
        (ASAHI_QUEUE_DESTROY, drm_asahi_queue_destroy, ioctl::RENDER_ALLOW, File::queue_destroy),
        (ASAHI_SUBMIT, drm_asahi_submit, ioctl::RENDER_ALLOW, File::submit),
    }
}

pub(crate) fn register<'a>(
    pdev: &'a platform::Device<Core<'_>>,
) -> Result<drm::Registration<'a, Driver>> {
    use kernel::dma::{Device as _, DmaMask};
    // SAFETY: G16's hardware output address width was validated at bootstrap.
    unsafe {
        pdev.dma_set_mask_and_coherent(DmaMask::try_new(42)?)?;
    }
    let runtime = crate::g16::Bootstrap::new(pdev)?;
    crate::g16_timeline::FW_CACHE_MAINT.store(
        crate::module_parameters::fw_cache_maint.value(),
        Ordering::Relaxed,
    );
    crate::g16_timeline::init_caps(
        crate::module_parameters::max_queue_entries.value(),
        crate::module_parameters::max_lane_entries.value(),
    )?;
    let dir = kernel::debugfs::Dir::new(c_str!("asahi_m4"));
    let queue_cap = KBox::pin_init(
        dir.read_write_file(c_str!("queue_cap"), crate::g16_timeline::CapKnob::QUEUE),
        GFP_KERNEL,
    )?;
    let lane_cap = KBox::pin_init(
        dir.read_write_file(c_str!("lane_cap"), crate::g16_timeline::CapKnob::LANE),
        GFP_KERNEL,
    )?;
    use crate::g16_timeline::FlagKnob;
    let knobs = [
        (c_str!("profile"), FlagKnob::PROFILE, crate::module_parameters::profile.value()),
        (c_str!("read_opt"), FlagKnob::READ_OPT, crate::module_parameters::read_opt.value()),
        (c_str!("safety_poll_ms"), FlagKnob::SAFETY_POLL_MS, crate::module_parameters::safety_poll_ms.value()),
        (c_str!("scratch_zero"), FlagKnob::SCRATCH_ZERO, crate::module_parameters::scratch_zero.value()),
    ];
    let mut flags = KVec::with_capacity(knobs.len(), GFP_KERNEL)?;
    for (name, knob, value) in knobs {
        knob.init(value);
        flags.push(KBox::pin_init(dir.read_write_file(name, knob), GFP_KERNEL)?, GFP_KERNEL)?;
    }
    let dep_stats = KBox::pin_init(
        dir.read_only_file(c_str!("dep_stats"), crate::g16_timeline::DepStats),
        GFP_KERNEL,
    )?;
    let maint = KBox::pin_init(
        dir.read_write_file(c_str!("fw_cache_maint"), crate::g16_timeline::MaintKnob),
        GFP_KERNEL,
    )?;
    let timeline = if crate::module_parameters::timeline.value() != 0 {
        let ring = crate::g16_timeline::Ring::new(
            crate::module_parameters::timeline_records.value() as usize,
        )?;
        Some(KBox::pin_init(dir.read_binary_file(c_str!("timeline"), ring), GFP_KERNEL)?)
    } else {
        None
    };
    let profile = KBox::pin_init(
        dir.read_only_file(c_str!("publish_profile"), crate::g16_timeline::Profile),
        GFP_KERNEL,
    )?;
    let fw_mem = KBox::pin_init(
        dir.read_binary_file(
            c_str!("fw_mem"),
            crate::g16_timeline::FwMem::new(runtime.firmware_pages()?),
        ),
        GFP_KERNEL,
    )?;
    let data = try_pin_init!(Data {
        gpu_revision: runtime.gpu_revision,
        core_mask: runtime.core_mask, maximum_frequency_khz: runtime.maximum_frequency_khz,
        scheduler: sched::Scheduler::new(pdev.as_ref(), 4, 128, 0, 30000, c_str!("asahi_m4"))?,
        admission <- kernel::new_mutex!(KVec::new()),
        failed: AtomicBool::new(false),
        notifications: runtime.notifications.clone(),
        _fw_mem: fw_mem,
        runtime <- kernel::new_mutex!(runtime),
        engine <- kernel::new_mutex!(Engine { cursor: 0, jobs: KVec::new(), issued: KVec::new() }),
        inbox <- kernel::new_mutex!(Inbox { running: false, jobs: KVec::new() }),
        timeline,
        _profile: profile,
        _maint: maint,
        _queue_cap: queue_cap,
        _flags: flags,
        _dep_stats: dep_stats,
        _lane_cap: lane_cap,
        _debugfs: dir, });
    let device = drm::UnregisteredDevice::<Driver>::new(pdev, data)?;
    let bound: &'a platform::Device<Bound> = pdev;
    // SAFETY: The registration is stored in the platform driver data and dropped on unbind.
    unsafe {
        drm::Registration::new(pdev.as_ref(), device, Ok::<_, Error>(RegData { pdev: bound }), 0)
    }
}

pub(crate) struct Buffer {
    owner: u64,
    vm: u32,
}
#[vtable]
impl DriverObject for Buffer {
    type Driver = Driver;
    type Args = (u64, u32);
    fn new(_dev: &Device, _size: usize, args: Self::Args) -> impl PinInit<Self, Error> {
        Ok(Self {
            owner: args.0,
            vm: args.1,
        })
    }

    /// Imported DMA-BUFs are shared, not private to a VM.
    fn import_args() -> Self::Args {
        (0, 0)
    }

    fn export(obj: &Object, flags: u32) -> Result<drm::gem::DmaBuf<Object>> {
        if obj.vm != 0 {
            return Err(EINVAL);
        }
        obj.prime_export(flags)
    }

    fn close(obj: &Object, file: &DrmFile) {
        // Mesa may reuse this virtual address after GEM_CLOSE. Retire only
        // VMs using this BO; hold file state for the final idle check and PTE
        // edits, but release it while waiting for completion.
        let inner = file.inner();
        if inner.device.failed.load(Ordering::Acquire) {
            return;
        }
        let exiting = current_exiting();
        let mut state = inner.state.lock();
        let mut cursor = 0;
        loop {
            // IDs never repeat. Re-find after each wait: another ioctl may
            // destroy a VM or swap_remove an earlier vector entry meanwhile.
            let Some(id) = state.vms.iter()
                .filter(|vm| vm.id > cursor && vm.mappings.iter()
                    .any(|m| core::ptr::eq(&*m.obj, obj)))
                .map(|vm| vm.id).min()
            else { break };
            cursor = id;
            let vm = state.vms.iter_mut().find(|vm| vm.id == id).unwrap();
            if vm.render_broken {
                // Already failed: its mappings stay until the file goes
                // away, and waiting again per object would only multiply
                // the stall (every handle is closed on exit).
                continue;
            }
            if exiting && vm.pending.iter().any(|f| {
                // SAFETY: The vector retains a live reference while queried.
                !unsafe { bindings::dma_fence_is_signaled(f.raw()) }
            }) {
                // DRM closes every handle before the file's teardown, which
                // destroys the queues (cancelling dependency-blocked jobs)
                // and then waits once. Don't wait here: keep the mappings
                // and leave the VM to that teardown.
                vm.render_broken = true;
                continue;
            }
            // A live close: wait at most CLOSE_DRAIN_MS. Only a fatal signal
            // ends the wait early (the process is then exiting anyway); any
            // other signal is ignored, so a healthy client's VM stays usable.
            let (relocked, result) = drain_vm(state, id, CLOSE_DRAIN_MS, true /* killable */, &inner.drained);
            state = relocked;
            let Some(vm) = state.vms.iter_mut().find(|vm| vm.id == id) else {
                continue;
            };
            if let Err(error) = result {
                // This VM's own work hasn't completed: typically a job still
                // waiting on an input fence that never signaled, or a hung
                // client. Its mappings keep the backing pinned, so late DMA
                // stays inside this client's pages. Only this VM becomes
                // unusable; other clients keep running.
                vm.render_broken = true;
                let task = kernel::current!();
                let comm = crate::util::task_comm(&task);
                // Where the unfinished work is: in the pump (issued or held on
                // a stamp) or still in drm_sched waiting for its input fences.
                let unsignaled = vm.pending.iter().filter(|f| {
                    // SAFETY: The vector retains a live reference while queried.
                    !unsafe { bindings::dma_fence_is_signaled(f.raw()) }
                }).count();
                let (mut pumped, mut issued) = (0usize, 0usize);
                {
                    let engine = inner.device.engine.lock();
                    let inbox = inner.device.inbox.lock();
                    for job in Iterator::chain(engine.jobs.iter(), inbox.jobs.iter()) {
                        let e = &job.execution;
                        if Arc::ptr_eq(&e.space, &vm.space) || Arc::ptr_eq(&e.space, &vm.compute_space) {
                            pumped += 1;
                            if job.next > 0 {
                                issued += 1;
                            }
                        }
                    }
                }
                pr_err!(
                    "G16: closing a GEM object: VM {} work didn't complete ({:?}) in {}[{}]; keeping its mappings and failing that VM's further submissions\n",
                    vm.id, error, crate::util::comm_str(&comm), task.pid()
                );
                pr_err!(
                    "G16: VM {}: {} of {} submissions unfinished: {} in the pump ({} started), {} waiting for input fences\n",
                    vm.id, unsignaled, vm.pending.len(), pumped, issued, unsignaled.saturating_sub(pumped)
                );
                continue;
            }
            let runtime = inner.device.runtime.lock();
            let result: Result = (|| {
                while let Some(range) = vm.mappings.iter()
                    .find(|m| core::ptr::eq(&*m.obj, obj))
                    .map(|m| m.range.clone())
                {
                    vm.unmap(range, &runtime)?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                // Page tables may be partly edited: the hardware state is
                // unknown, so stop the device as before.
                inner.device.failed.store(true, Ordering::Release);
                pr_err!("G16: failed to unbind closed GEM object: {:?}\n", error);
                return;
            }
        }
    }
}

#[pin_data(PinnedDrop)]
pub(crate) struct File {
    device: DeviceRef,
    id: u64,
    state: Arc<Mutex<FileState>>,
    #[pin]
    drained: CondVar,
}
impl drm::file::DriverFile for File {
    type Driver = Driver;
    fn open(device: &Device) -> Result<Pin<KBox<Self>>> {
        KBox::pin_init(
            try_pin_init!(Self {
                device: device.into(),
                id: NEXT_FILE.fetch_add(1, Ordering::Relaxed),
                drained <- kernel::new_condvar!("m4-file-drain"),
                state: Arc::pin_init(
                    kernel::new_mutex!(FileState {
                        next_vm: 1,
                        next_queue: 1,
                        next_timestamp: 1,
                        queues: KVec::new(),
                        timestamps: KVec::new(),
                        retained_timestamps: KVec::new(),
                        vms: KVec::new()
                    }),
                    GFP_KERNEL
                )?,
            }),
            GFP_KERNEL,
        )
    }
}

#[pinned_drop]
impl PinnedDrop for File {
    fn drop(self: Pin<&mut Self>) {
        // Entity destruction cancels dependency-blocked jobs. Never hold the
        // state lock while DRM waits for scheduler callbacks.
        let queues = core::mem::take(&mut self.state.lock().queues);
        drop(queues);
        // Wait only for this file's own work. Other clients' fences don't
        // protect anything torn down here.
        let mut state = self.state.lock();
        let mut drained = true;
        for vm in state.vms.iter_mut() {
            // Not interruptible: a killed process has a fatal signal
            // pending, and giving up early would leak its pages.
            if drain_within(&mut vm.pending, CLOSE_DRAIN_MS, false).is_err() {
                drained = false;
            }
        }
        if !drained {
            // Completion was not established. Retain this file's roots and
            // every pinned GEM page until reboot, so late DMA can't reach
            // reused RAM. Other clients keep running.
            pr_err!("G16: closing a client whose GPU work didn't complete; leaking its address spaces\n");
            drop(state);
            core::mem::forget(self.state.clone());
            return;
        }
        let mut runtime = self.device.runtime.lock();
        for vm in state.vms.iter().rev() {
            if vm.detach(&mut runtime).is_err() {
                // Retain GPU-visible mappings if the installed root cannot
                // be detached safely; device reset releases the hardware.
                drop(runtime);
                drop(state);
                core::mem::forget(self.state.clone());
                return;
            }
        }
        for timestamp in &state.retained_timestamps {
            if runtime
                .release_timestamp(timestamp.base..timestamp.base + timestamp.extent)
                .is_err()
            {
                core::mem::forget(self.state.clone());
                break;
            }
        }
    }
}

impl File {
    fn get_time(
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

    fn vm_create(
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
                render_ready: false,
                compute_ready: false,
                render_broken: false,
            },
            GFP_KERNEL,
        )?;
        state.next_vm = next;
        data.vm_id = id;
        Ok(0)
    }

    fn vm_destroy(
        device: &RegDevice,
        _reg: &RegData<'_>,
        data: &mut uapi::drm_asahi_vm_destroy,
        file: &DrmFile,
    ) -> Result<u32> {
        let device: &Device = device;
        if data.pad != 0 {
            return Err(EINVAL);
        }
        let mut admission = device.admission.lock();
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
            drain(&mut admission)?;
            state.vms[index].detach(&mut device.runtime.lock())?;
            state.vms.swap_remove(index);
        }
        Ok(0)
    }

    fn get_params(
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

    fn queue_create(
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

    fn queue_destroy(
        device: &RegDevice,
        _reg: &RegData<'_>,
        data: &mut uapi::drm_asahi_queue_destroy,
        file: &DrmFile,
    ) -> Result<u32> {
        let device: &Device = device;
        if data.pad != 0 {
            return Err(EINVAL);
        }
        let mut admission = device.admission.lock();
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
        // Cancel dependency-blocked jobs before waiting on their lifetime
        // fences. Scheduler destruction must run outside the file-state lock.
        let vm_id = queue.vm;
        drop(queue);
        drain(&mut admission)?;
        let mut state = inner.state.lock();
        if !state.queues.iter().any(|q| q.vm == vm_id) {
            if let Some(index) = state.vms.iter().position(|v| v.id == vm_id && !v.visible) {
                state.vms[index].detach(&mut device.runtime.lock())?;
                state.vms.swap_remove(index);
            }
        }
        Ok(0)
    }

    fn bind_object(
        device: &RegDevice,
        reg: &RegData<'_>,
        data: &mut uapi::drm_asahi_gem_bind_object,
        file: &DrmFile,
    ) -> Result<u32> {
        let device: &Device = device;
        if data.pad != 0 || data.vm_id != 0 {
            return Err(EINVAL);
        }
        let mut admission = device.admission.lock();
        drain(&mut admission)?;
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
                let timestamp = &state.timestamps[index];
                // Failed BIND rollback can retain an unpublished allocation
                // without advancing next_timestamp. Match the reserved range,
                // which stays unique even if that public ID is reused.
                let retained = state
                    .retained_timestamps
                    .iter()
                    .position(|t| t.base == timestamp.base && t.extent == timestamp.extent)
                    .ok_or(EIO)?;
                // Admission is held and drained above: no submitted work can
                // still write this firmware mapping, or acquire it anew.
                // Keep both backing references if unmapping fails, just as
                // at file teardown, so a stale PTE never reaches freed RAM.
                device.runtime.lock().release_timestamp(
                    timestamp.base..timestamp.base + timestamp.extent,
                )?;
                state.timestamps.swap_remove(index);
                state.retained_timestamps.swap_remove(retained);
            }
            _ => return Err(EINVAL),
        }
        Ok(0)
    }

    fn vm_bind(device: &RegDevice, reg: &RegData<'_>, data: &mut uapi::drm_asahi_vm_bind, file: &DrmFile) -> Result<u32> {
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
                        let (relocked, result) = drain_vm(state, data.vm_id, 35000, false, &inner.drained);
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
                if !overlaps.is_empty() {
                    if !vm.pending.is_empty() {
                        let (relocked, result) = drain_vm(state, data.vm_id, 35000, false, &inner.drained);
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

    fn gem_create(
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

    fn gem_mmap_offset(
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

impl Vm {
    fn detach(&self, runtime: &mut crate::g16::Bootstrap) -> Result {
        {
            let space = self.space.lock();
            runtime.flush_render_ring(space.roots().low);
            runtime.release_render_vm(space.roots())?;
        }
        let compute = self.compute_space.lock();
        runtime.release_render_vm(compute.roots())?;
        Ok(())
    }

    fn map(
        &mut self,
        range: Range<u64>,
        mut offset: u64,
        obj: ARef<Object>,
        sgt: &scatterlist::SGTable,
        access: crate::pgtable::Prot,
        permissions: u32,
        single: bool,
        runtime: &crate::g16::Bootstrap,
    ) -> Result {
        self.snapshot = None;
        if self
            .mappings
            .iter()
            .any(|m| range.start < m.range.end && range.end > m.range.start)
        {
            return Err(EEXIST);
        }
        let bo_offset = offset;
        let mut chunks = KVVec::new();
        let mut va = range.start;
        for segment in sgt.iter() {
            let length = u64::from(segment.dma_len());
            if offset >= length {
                offset -= length;
                continue;
            }
            let pa = segment.dma_address().checked_add(offset).ok_or(EINVAL)?;
            let length = (length - offset).min(range.end - va);
            offset = 0;
            if (va | pa | length) & 0x3fff != 0 {
                return Err(EINVAL);
            }
            let end = if single { range.end } else { va + length };
            chunks.push((va, end, pa), GFP_KERNEL)?;
            va = end;
            if va == range.end {
                break;
            }
        }
        if va != range.end {
            return Err(EINVAL);
        }
        self.mappings.push(
            Mapping {
                range: range.clone(),
                permissions,
                offset: bo_offset,
                single,
                obj,
            },
            GFP_KERNEL,
        )?;
        // Keep the list sorted by address for lookups.
        let last = self.mappings.len() - 1;
        let index = self.mappings[..last].partition_point(|m| m.range.start < range.start);
        self.mappings[index..].rotate_right(1);
        let result = (|| {
            // The shim uses distinct roots for the fixed render and compute
            // private namespaces. Both views share the caller's GEM pages.
            for view in [&self.space, &self.compute_space] {
                let mut space = view.lock();
                for &(start, end, pa) in &chunks {
                    space.low.map_pages(start..end, pa, access, single)?;
                    let last = if single {
                        pa
                    } else {
                        pa + end - start - 0x4000
                    };
                    if space.low.translate(start)? != Some(pa)
                        || space.low.translate(end - 0x4000)? != Some(last)
                    {
                        return Err(EIO);
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            // Keep the backing pinned if rollback itself cannot finish.
            self.space.lock().low.unmap_pages(range.clone())?;
            self.compute_space.lock().low.unmap_pages(range)?;
            self.publish_mappings(runtime);
            self.mappings[index..].rotate_left(1);
            self.mappings.pop();
            return Err(e);
        }
        self.publish_mappings(runtime);
        Ok(())
    }

    /// Publish translation changes before another job can use this VM.
    fn publish_mappings(&self, runtime: &crate::g16::Bootstrap) {
        for view in [&self.space, &self.compute_space] {
            let space = view.lock();
            space.sync();
            runtime.invalidate_client(&space);
        }
    }

    fn unmap(&mut self, range: Range<u64>, runtime: &crate::g16::Bootstrap) -> Result {
        self.snapshot = None;
        // Callers drain this VM's jobs and hold file state against new uses.
        // A failure may have signaled those fences while we waited. The
        // runtime lock prevents a concurrent failure during this teardown.
        if runtime.render_failed() {
            return Err(EIO);
        }
        // Mappings do not overlap, so removing one interval can split at most
        // one mapping into two. Reserve that one extra record before editing
        // PTEs. Whole-mapping teardown needs no new ownership allocation,
        // especially when GEM close follows an allocation failure.
        let right = self.mappings.iter().find(|m| {
            m.range.start < range.start && range.end < m.range.end
        }).map(|m| Mapping {
            range: range.end..m.range.end,
            offset: m.offset + if m.single { 0 } else { range.end - m.range.start },
            permissions: m.permissions,
            single: m.single,
            obj: m.obj.clone(),
        });
        if right.is_some() {
            self.mappings.reserve(1, GFP_KERNEL)?;
        }
        self.space.lock().low.reserve_invalidation()?;
        self.compute_space.lock().low.reserve_invalidation()?;
        // Unbinds may span holes, such as unbound sparse pages. Clear only
        // the parts which mappings cover.
        for m in self.mappings.iter() {
            let start = m.range.start.max(range.start);
            let end = m.range.end.min(range.end);
            if start < end {
                self.space.lock().low.unmap_pages(start..end)?;
                self.compute_space.lock().low.unmap_pages(start..end)?;
            }
        }
        self.publish_mappings(runtime);
        // Only release backing after both views and their invalidations are
        // complete. Any earlier failure preserves all original references.
        self.mappings.retain(|m| {
            if m.range.end <= range.start || m.range.start >= range.end {
                return true;
            }
            if m.range.start < range.start {
                m.range.end = range.start;
                return true;
            }
            if m.range.end > range.end {
                if !m.single {
                    m.offset += range.end - m.range.start;
                }
                m.range.start = range.end;
                return true;
            }
            false
        });
        if let Some(right) = right {
            let start = right.range.start;
            self.mappings.push_within_capacity(right)
                .unwrap_or_else(|_| unreachable!("reserved split mapping"));
            let last = self.mappings.len() - 1;
            let index = self.mappings[..last].partition_point(|m| m.range.start < start);
            self.mappings[index..].rotate_right(1);
        }
        Ok(())
    }
}

static FENCE_KEY: Pin<&kernel::sync::LockClassKey> = kernel::static_lock_class!();

/// The calling process's name, for hang reports.
fn current_comm() -> [u8; 16] {
    let task = kernel::current!();
    let leader = task.group_leader();
    crate::util::task_comm(leader)
}

fn drain(pending: &mut KVec<dma_fence::Fence>) -> Result {
    drain_within(pending, 35000, false)
}

/// Wait for every fence in `pending` within one overall budget of `ms`.
/// With `killable`, a fatal signal ends the wait early with EINTR; other
/// signals are ignored, since a GEM close can't be restarted (DRM has
/// already removed the handle) and failing a live client's VM because of
/// SIGIO or a timer would break a healthy process.
fn drain_within(pending: &mut KVec<dma_fence::Fence>, ms: u32, killable: bool) -> Result {
    let mut left = kernel::time::msecs_to_jiffies(ms) as core::ffi::c_long;
    // Re-check for a fatal signal this often while waiting.
    let slice = if killable {
        kernel::time::msecs_to_jiffies(100) as core::ffi::c_long
    } else {
        left
    };
    for fence in pending.iter() {
        loop {
            let step = left.min(slice).max(1);
            // SAFETY: Each entry owns its fence reference throughout the wait.
            let result = unsafe { bindings::dma_fence_wait_timeout(fence.raw(), false, step as _) };
            if result < 0 {
                return Err(Error::from_errno(result as i32));
            }
            if result > 0 {
                // dma_fence_wait_timeout returns the jiffies of `step` left
                // (at least 1), so `left` stays positive.
                left -= step - result as core::ffi::c_long;
                break;
            }
            left -= step;
            if left <= 0 {
                return Err(ETIMEDOUT);
            }
            if killable && fatal_signal_pending() {
                return Err(EINTR);
            }
        }
    }
    pending.clear();
    Ok(())
}

/// Block new submissions to this VM before dropping file state for waits.
/// Overlapping drains share a counter: only the last one releases submitters.
/// Return with state held through the caller's PTE edits, so clearing the
/// barrier cannot let a new submit race those edits. No VM reference or vector
/// index survives an unlock; all retries share one overall timeout.
fn drain_vm<'a>(
    mut state: MutexGuard<'a, FileState>,
    id: u32,
    ms: u32,
    killable: bool,
    changed: &CondVar,
) -> (MutexGuard<'a, FileState>, Result) {
    let Some(vm) = state.vms.iter_mut().find(|vm| vm.id == id) else {
        return (state, Err(ENOENT));
    };
    let Some(draining) = vm.draining.checked_add(1) else {
        return (state, Err(EOVERFLOW));
    };
    vm.draining = draining;
    let mutex = state.lock_ref();
    let start = kernel::time::Instant::<kernel::time::Monotonic>::now();
    let result = loop {
        let Some(vm) = state.vms.iter_mut().find(|vm| vm.id == id) else {
            break Err(ENOENT);
        };
        vm.pending.retain(|f| {
            // SAFETY: The vector owns each fence reference during the query.
            !unsafe { bindings::dma_fence_is_signaled(f.raw()) }
        });
        let Some(fence) = vm.pending.first().cloned() else {
            break Ok(());
        };
        let left = i64::from(ms) - start.elapsed().as_millis();
        if left <= 0 {
            break Err(ETIMEDOUT);
        }
        let step = kernel::time::msecs_to_jiffies(
            if killable { left.min(100) } else { left } as u32,
        );
        drop(state);
        // SAFETY: The local clone owns the fence throughout the unlocked wait.
        let result = unsafe { bindings::dma_fence_wait_timeout(fence.raw(), false, step as _) };
        state = mutex.lock();
        if result < 0 {
            break Err(Error::from_errno(result as i32));
        }
        if result == 0 && killable && fatal_signal_pending() {
            break Err(EINTR);
        }
    };
    if let Some(vm) = state.vms.iter_mut().find(|vm| vm.id == id) {
        vm.draining -= 1;
        if vm.draining != 0 {
            return (state, result);
        }
    }
    // Also wake after VM removal: submit must re-find its queue and VM.
    // Success, timeout and signal exits all release this drain's barrier.
    changed.notify_all();
    (state, result)
}

/// Whether the calling task has SIGKILL pending (fatal_signal_pending()).
fn fatal_signal_pending() -> bool {
    let task = kernel::current!();
    // SAFETY: `current` is valid for the duration of this call. Like
    // __fatal_signal_pending(), this is an unlocked read of the task's own
    // pending set; SIGKILL stays set once delivered.
    unsafe {
        (*task.as_ptr()).pending.signal.sig[0] & (1 << (bindings::SIGKILL - 1)) != 0
    }
}

/// A healthy issued Work finishes within the 10 s firmware watchdog, or the
/// device fails anyway; past this only unissued (stuck) work can remain.
const CLOSE_DRAIN_MS: u32 = 12000;

/// Whether the calling task is exiting (DRM releases its GEM handles).
fn current_exiting() -> bool {
    let task = kernel::current!();
    // SAFETY: `current` is valid for the duration of this call.
    unsafe { (*task.as_ptr()).flags & bindings::PF_EXITING != 0 }
}

/// Owns copied commands and the VM storage until hardware completion. The
/// worker never acquires admission or the file-state lock, so another SUBMIT
/// can publish pending fences while the current job executes.
#[pin_data(PinnedDrop)]
struct Execution {
    id: u64,
    firmware: Arc<Mutex<crate::g16::FirmwareQueues>>,
    // UAPI subqueue indices, resolved against previous submissions and this batch.
    barriers: KVec<[u16; 2]>,
    device: DeviceRef,
    _owner: Arc<Mutex<FileState>>,
    render_space: Arc<Mutex<crate::g16_vm::AddressSpace>>,
    space: Arc<Mutex<crate::g16_vm::AddressSpace>>,
    _bindings: Bindings,
    support: Option<Arc<crate::g16::Support>>,
    parameters: KVec<Parameters>,
    timestamps: KVec<[u64; 4]>,
    completion: dma_fence::Fence,
    started: AtomicBool,
    /// timeline=1: CNTVCT at the ioctl and when the scheduler ran the job,
    /// the public queue, and input fences in total and still pending then.
    trace: Trace,
    #[pin]
    work: Work<Execution>,
}

impl_has_work! { impl HasWork<Self> for Execution { self.work } }

#[derive(Default)]
struct Trace {
    ioctl: u64,
    run: AtomicU64,
    queue: u32,
    inputs: u32,
    pending: u32,
}

/// The CPU's virtual counter (1 GHz on T8132).
pub(crate) fn ticks() -> u64 {
    let value: u64;
    // SAFETY: Reading CNTVCT_EL0 has no side effects.
    unsafe { core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) value) };
    value
}

fn trace_record(ring: &crate::g16_timeline::Ring, job: &Running) {
    let t = &job.execution.trace;
    let pid = match job.execution.parameters.first() {
        Some(Parameters::Render(p)) => p.tgid,
        Some(Parameters::Compute(p)) => p.tgid,
        None => 0,
    };
    ring.push(&crate::g16_timeline::Record {
        exec: job.execution.id,
        pid: pid as u32,
        // A per-file id: muvm guests all submit from the one krun process.
        file: (Arc::as_ptr(&job.execution._owner) as usize >> 4) as u64 & 0xffff_ffff,
        queue: t.queue,
        cmds: job.execution.parameters.len() as u32,
        pending: t.pending,
        inputs: t.inputs,
        busy: job.busy,
        error: job.error.is_some(),
        why: job.why,
        ticks: [
            t.ioctl,
            t.run.load(Ordering::Relaxed),
            job.first,
            job.last,
            ticks(),
            job.kick,
            job.irq,
            job.rx,
            job.wake,
            job.seen,
        ],
    });
}

struct Running {
    execution: Arc<Execution>,
    next: usize,
    /// timeline=1: publication times, deferrals and their reasons
    /// (backpressure bits; 0x100 a dependency hold).
    first: u64,
    last: u64,
    busy: u32,
    why: u64,
    /// timeline=2: doorbell after the first publication, and for the
    /// notification that retired the last Work: IRQ, RTKit delivery, pump
    /// wake and the retire scan that saw it.
    kick: u64,
    irq: u64,
    rx: u64,
    wake: u64,
    seen: u64,
    pending: usize,
    error: Option<Error>,
    history: [KVec<Option<crate::g16::Stamp>>; 2],
}

fn resolve_barriers(
    history: &[KVec<Option<crate::g16::Stamp>>; 2],
    barriers: [u16; 2],
) -> Result<[Option<crate::g16::Stamp>; 2]> {
    let mut dependencies = [None; 2];
    for (stage, barrier) in barriers.into_iter().enumerate() {
        if barrier != 0xffff {
            dependencies[stage] = *history[stage].get(barrier as usize).ok_or(EIO)?;
        }
    }
    Ok(dependencies)
}
struct Issued {
    id: u64,
    owner: u64,
}
struct Engine {
    cursor: usize,
    jobs: KVec<Running>,
    issued: KVec<Issued>,
}
struct Inbox {
    /// A pump is queued or running; it rechecks the inbox before stopping.
    running: bool,
    jobs: KVec<Running>,
}

impl Engine {
    /// Collect without removing anything if allocation fails, as before.
    fn take_finished(&mut self) -> KVec<Running> {
        let mut finished = KVec::new();
        if finished.reserve(self.jobs.len(), GFP_KERNEL).is_err() {
            return finished;
        }
        let mut index = 0;
        while index < self.jobs.len() {
            let job = &self.jobs[index];
            if job.pending == 0
                && (job.error.is_some() || job.next == job.execution.parameters.len())
            {
                let job = self.jobs.remove(index).expect("index in range");
                finished.push_within_capacity(job).map_err(|_| ())
                    .expect("reserved for every job");
            } else {
                index += 1;
            }
        }
        // Removal shifts later entries; keep the admission cursor valid.
        if self.cursor > self.jobs.len() {
            self.cursor = 0;
        }
        finished
    }

    /// Take the jobs drm_sched has run since the last pass, in order.
    /// On failure the jobs stay in the inbox for the error path to fail.
    fn admit(&mut self, inbox: &Mutex<Inbox>) -> Result {
        let mut inbox = inbox.lock();
        self.jobs.reserve(inbox.jobs.len(), GFP_KERNEL)?;
        let incoming = core::mem::take(&mut inbox.jobs);
        drop(inbox);
        for job in incoming {
            self.jobs.push_within_capacity(job).map_err(|_| ENOMEM)?;
        }
        Ok(())
    }
}

impl Execution {
    /// Call only after dropping the engine and runtime locks.
    fn signal_finished(&self, job: Running) {
        if let Some(ring) = &self.device.timeline {
            trace_record(ring, &job);
        }
        if let Some(error) = job.error {
            job.execution.completion.set_error(error);
        }
        let _ = job.execution.completion.signal();
    }
}

impl WorkItem for Execution {
    type Pointer = Arc<Self>;

    fn run(this: Arc<Self>) {
        // Exactly one pump owns publication order. It holds these locks only
        // while preparing/publishing or inspecting retirement, never asleep.
        // Scheduler callbacks append ready jobs even while hardware is busy.
        let mut wake_result = Ok(());
        loop {
            let generation = this.device.notifications.snapshot();
            let mut engine = this.device.engine.lock();
            let taken = engine.admit(&this.device.inbox);
            let mut runtime = this.device.runtime.lock();
            let result = (|| -> Result<bool> {
                let _t = crate::g16_timeline::Timer::start(&crate::g16_timeline::P_PUMP);
                taken?;
                // No notification within the watchdog window: poll the stamps.
                // Retirement's watchdog reports a real hang; a lost
                // notification simply retires the finished work. Report what
                // the poll found: retired work means a notification was late
                // or lost, nothing retired means slow work.
                let timed_out = match wake_result {
                    Err(e) if e == ETIMEDOUT => Some(runtime.oldest_flight_ms()),
                    Err(e) => return Err(e),
                    Ok(()) => None,
                };
                generation?;
                if this.device.failed.load(Ordering::Acquire) {
                    return Err(EIO);
                }
                let mut progress = false;
                let mut polled_retired = 0u32;
                while let Some((id, result)) = runtime.retire().inspect_err(|error| {
                    pr_err!("G16: firmware retirement failed: {:?}\n", error);
                })? {
                    let index = engine.issued.iter().position(|r| r.id == id).ok_or(EIO)?;
                    let issued = engine.issued.remove(index).map_err(|_| EIO)?;
                    let job = engine
                        .jobs
                        .iter_mut()
                        .find(|r| r.execution.id == issued.owner)
                        .ok_or(EIO)?;
                    job.pending -= 1;
                    if job.pending == 0 && crate::module_parameters::timeline.value() >= 2 {
                        let n = &this.device.notifications;
                        job.seen = ticks();
                        job.irq = n.last_irq.load(Ordering::Relaxed);
                        job.rx = n.last_rx.load(Ordering::Relaxed);
                        job.wake = n.last_wake.load(Ordering::Relaxed);
                    }
                    if let Err(error) = result {
                        job.error = Some(error);
                    }
                    progress = true;
                    polled_retired += 1;
                }
                if let Some(age) = timed_out {
                    use crate::g16_timeline as t;
                    t::POLL_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
                    t::POLL_RETIRED.fetch_add(u64::from(polled_retired), Ordering::Relaxed);
                    // A safety poll every few ms is routine; report only polls
                    // that found finished work, or long waits.
                    if polled_retired != 0 || age >= 1000 {
                        pr_warn!(
                            "G16: notification wait timed out; polling retired {} Work(s), oldest flight {} ms\n",
                            polled_retired,
                            age
                        );
                    }
                }
                Ok(progress)
            })();
            // Signal every finished job before admitting more work, with the
            // locks dropped: a completed fence must not wait behind a
            // publication pass. Upstream signals from its event path.
            let mut finished = if result.is_ok() {
                engine.take_finished()
            } else {
                KVec::new()
            };
            let signalled = !finished.is_empty();
            if signalled {
                drop(runtime);
                drop(engine);
                for job in finished.drain_all() {
                    this.signal_finished(job);
                }
                engine = this.device.engine.lock();
                runtime = this.device.runtime.lock();
            }
            let result = result.and_then(|progress| (|| -> Result<bool> {
                let mut progress = progress || signalled;
                let mut admitted = 0;
                loop {
                    let previous = admitted;
                    let start = engine.cursor;
                    for offset in 0..engine.jobs.len() {
                        let index = (start + offset) % engine.jobs.len();
                        // Preserve submit-relative barrier index zero: all earlier
                        // submissions on this public queue must have published.
                        if engine.jobs[..index].iter().any(|earlier| {
                            earlier.error.is_none()
                                && earlier.next < earlier.execution.parameters.len()
                                && Arc::ptr_eq(
                                    &earlier.execution.firmware,
                                    &engine.jobs[index].execution.firmware,
                                )
                        }) {
                            continue;
                        }
                        let job = &mut engine.jobs[index];
                        if job.error.is_some() || job.next == job.execution.parameters.len() {
                            continue;
                        }
                        if job.history[0].is_empty() {
                            let previous = job.execution.firmware.lock().last;
                            for (history, stamp) in job.history.iter_mut().zip(previous) {
                                history.push(stamp, GFP_KERNEL)?;
                            }
                        }
                        let dependencies =
                            resolve_barriers(&job.history, job.execution.barriers[job.next])?;
                        let execution = job.execution.clone();
                        let compute =
                            matches!(execution.parameters[job.next], Parameters::Compute(_));
                        // Intra-queue UAPI dependencies are encoded into firmware
                        // rings; preparation does not read GPU-produced data.
                        engine.issued.reserve(1, GFP_KERNEL)?;
                        let job = &mut engine.jobs[index];
                        crate::mem::sync();
                        let receipt = match &execution.parameters[job.next] {
                            Parameters::Render(p) => runtime.submit_render(
                                execution.firmware.clone(),
                                execution.render_space.clone(),
                                p,
                                execution.support.as_deref().ok_or(EIO)?,
                                execution.timestamps[job.next],
                                &dependencies,
                            ),
                            Parameters::Compute(p) => runtime.submit_compute(
                                execution.firmware.clone(),
                                execution.space.clone(),
                                p,
                                execution.timestamps[job.next],
                                &dependencies,
                            ),
                        };
                        let receipt = match receipt {
                            Ok(receipt) => receipt,
                            Err(EBUSY) if !runtime.render_failed() => {
                                if crate::module_parameters::timeline.value() != 0 {
                                    job.busy += 1;
                                    job.why |= match runtime.take_pressure() {
                                        0 => 0x100,
                                        bits => bits,
                                    };
                                }
                                continue;
                            }
                            Err(error) if !runtime.render_failed() => {
                                pr_err!(
                                    "G16: command {} preparation failed: {:?}\n",
                                    job.next,
                                    error
                                );
                                job.error = Some(error);
                                progress = true;
                                continue;
                            }
                            Err(error) => return Err(error),
                        };
                        job.history[usize::from(compute)].push(Some(receipt.stamp), GFP_KERNEL)?;
                        let issued = Issued {
                            id: receipt.id,
                            owner: execution.id,
                        };
                        if crate::module_parameters::timeline.value() != 0 {
                            let now = ticks();
                            if job.next == 0 {
                                job.first = now;
                            }
                            job.last = now;
                        }
                        job.next += 1;
                        job.pending += 1;
                        engine.issued.push(issued, GFP_KERNEL)?;
                        // Resume after the last admitted job when a scarce slot
                        // becomes free, rather than repeatedly favoring index zero.
                        engine.cursor = index + 1;
                        progress = true;
                        admitted += 1;
                        if admitted == 64 {
                            break;
                        }
                    }
                    if admitted == previous || admitted == 64 {
                        break;
                    }
                }
                runtime.flush()?;
                if crate::module_parameters::timeline.value() >= 2 {
                    let now = ticks();
                    for job in engine.jobs.iter_mut().filter(|j| j.first != 0 && j.kick == 0) {
                        job.kick = now;
                    }
                }
                Ok(progress)
            })());
            if let Err(error) = result {
                pr_err!(
                    "G16: submission pipeline failed: {:?}, {} jobs, {} issued\n",
                    error,
                    engine.jobs.len(),
                    engine.issued.len()
                );
                runtime.poison();
                this.device.failed.store(true, Ordering::Release);
                let mut jobs = core::mem::take(&mut engine.jobs);
                engine.issued.clear();
                let queued = {
                    let mut inbox = this.device.inbox.lock();
                    inbox.running = false;
                    core::mem::take(&mut inbox.jobs)
                };
                drop(runtime);
                drop(engine);
                // Quarantined context roots remain installed and retained;
                // a failed fence never authorizes reuse of their backing.
                for job in Iterator::chain(jobs.drain_all(), queued) {
                    job.execution.completion.set_error(error);
                    let _ = job.execution.completion.signal();
                }
                return;
            }
            if let Some(index) = engine.jobs.iter().position(|j| {
                j.pending == 0 && (j.error.is_some() || j.next == j.execution.parameters.len())
            }) {
                let job = engine
                    .jobs
                    .remove(index)
                    .expect("index found under engine lock");
                drop(runtime);
                drop(engine);
                this.signal_finished(job);
                continue;
            }
            if engine.jobs.is_empty() {
                // Stop only if no job arrived since this pass drained the
                // inbox; run_job sees `running` cleared and queues a new pump.
                let mut inbox = this.device.inbox.lock();
                if inbox.jobs.is_empty() {
                    inbox.running = false;
                    return;
                }
                drop(inbox);
                drop(runtime);
                drop(engine);
                continue;
            }
            let timeout_ms = runtime.watchdog_remaining_ms();
            drop(runtime);
            drop(engine);
            if !result.unwrap_or(false) {
                wake_result = this.device.notifications.wait(
                    generation.expect("checked before servicing jobs"),
                    timeout_ms,
                );
                if crate::module_parameters::timeline.value() >= 2 {
                    this.device.notifications.last_wake.store(ticks(), Ordering::Relaxed);
                }
            }
        }
    }
}

#[pinned_drop]
impl PinnedDrop for Execution {
    fn drop(self: Pin<&mut Self>) {
        // An entity can discard a job still waiting for its dependencies.
        // Such a job never published to firmware, but its fence must finish.
        if !self.started.load(Ordering::Acquire) {
            self.completion.set_error(ECANCELED);
            let _ = self.completion.signal();
        }
    }
}

struct SubmissionJob {
    execution: Arc<Execution>,
}

impl sched::JobImpl for SubmissionJob {
    fn run(job: &mut sched::Job<Self>) -> Result<Option<dma_fence::Fence>> {
        let execution = &job.execution;
        if crate::module_parameters::timeline.value() != 0 {
            execution.trace.run.store(ticks(), Ordering::Relaxed);
        }
        // Firmware jobs cannot be replayed after a fault without a reboot.
        if execution.started.swap(true, Ordering::AcqRel) {
            return Err(EIO);
        }
        let fence = execution.completion.clone();
        // Allocate before taking the inbox lock: it is held only to append.
        let running = (|| -> Result<Running> {
            let mut history = [KVec::new(), KVec::new()];
            for events in &mut history {
                events.reserve(65, GFP_KERNEL)?;
            }
            Ok(Running {
                execution: execution.clone(),
                next: 0,
                first: 0,
                last: 0,
                busy: 0,
                why: 0,
                kick: 0,
                irq: 0,
                rx: 0,
                wake: 0,
                seen: 0,
                pending: 0,
                error: None,
                history,
            })
        })();
        let mut inbox = execution.device.inbox.lock();
        if let Err(error) = running.and_then(|r| inbox.jobs.push(r, GFP_KERNEL).map_err(Into::into)) {
            drop(inbox);
            execution.completion.set_error(error);
            let _ = execution.completion.signal();
            return Err(error);
        }
        if !inbox.running {
            inbox.running = true;
            let _ = workqueue::system_dfl().enqueue(execution.clone());
        }
        drop(inbox);
        // A running worker may be asleep waiting for firmware on another
        // queue. New ready work must wake it even if no GPU event arrives.
        execution.device.notifications.notify(false);
        Ok(Some(fence))
    }

    fn timed_out(job: &mut sched::Job<Self>) -> sched::Status {
        job.execution.device.failed.store(true, Ordering::Release);
        job.execution.device.notifications.notify(false);
        // Wake the worker to quarantine firmware mappings and fail the jobs.
        // Do not signal its lifetime fence before that cleanup here.
        sched::Status::NoDevice
    }

    fn cancel(job: &mut sched::Job<Self>) {
        if !job.execution.started.load(Ordering::Acquire) {
            job.execution.completion.set_error(ECANCELED);
            let _ = job.execution.completion.signal();
        }
    }
}

struct Completion;
#[vtable]
impl dma_fence::FenceOps for Completion {
    fn get_driver_name<'a>(self: &'a dma_fence::FenceObject<Self>) -> &'a kernel::str::CStr {
        c_str!("asahi")
    }
    fn get_timeline_name<'a>(self: &'a dma_fence::FenceObject<Self>) -> &'a kernel::str::CStr {
        c_str!("m4-execution")
    }
}
struct OutputSync {
    object: drm::syncobj::SyncObj,
    chain: Option<dma_fence::FenceChain>,
    point: u64,
}

impl File {
    fn submit(device: &RegDevice, _reg: &RegData<'_>, data: &mut uapi::drm_asahi_submit, file: &DrmFile) -> Result<u32> {
        let device: &Device = device;
        if data.flags != 0
            || data.pad != 0
            || data.cmdbuf_size == 0
            || data.cmdbuf_size > 1024 * 1024
            || data.in_sync_count > 4096
            || data.out_sync_count > 4096
        {
            return Err(EINVAL);
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
            offset = offset.checked_add(8).ok_or(EINVAL)?;
            let end = offset.checked_add(header.size as usize).ok_or(EINVAL)?;
            let payload = bytes.get(offset..end).ok_or(EINVAL)?;
            offset = end;
            match u32::from(header.cmd_type) {
                uapi::drm_asahi_cmd_type_DRM_ASAHI_CMD_RENDER => {
                    if (header.vdm_barrier != 0xffff && header.vdm_barrier > renders)
                        || (header.cdm_barrier != 0xffff && header.cdm_barrier > computes)
                        || renders + computes == 64
                    {
                        return Err(EINVAL);
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
                        return Err(EINVAL);
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
                        return Err(EINVAL);
                    }
                    let mut reader = crate::util::Reader::new(payload);
                    while !reader.is_empty() {
                        let attachment: uapi::drm_asahi_attachment = reader.read()?;
                        if attachment.flags != 0
                            || attachment.pad != 0
                            || attachment.pointer.checked_add(attachment.size).is_none()
                        {
                            return Err(EINVAL);
                        }
                        attachments.push(attachment, GFP_KERNEL)?;
                    }
                }
                _ => return Err(ENOTSUPP),
            }
        }
        if renders + computes == 0 {
            return Err(EINVAL);
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
                return Err(EINVAL);
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
                let fence = object.fence_get().ok_or(EINVAL)?;
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
        let vm_id = queue.vm;
        let firmware = queue.firmware.clone();
        let mut timestamps = KVec::new();
        let resolve = |stamp: &uapi::drm_asahi_timestamp| -> Result<u64> {
            if stamp.handle == 0 {
                return if stamp.offset == 0 {
                    Ok(0)
                } else {
                    Err(EINVAL)
                };
            }
            let binding = state
                .timestamps
                .iter()
                .find(|t| t.id == stamp.handle)
                .ok_or(ENOENT)?;
            let offset = u64::from(stamp.offset);
            if offset & 7 != 0 || offset.checked_add(8).ok_or(EINVAL)? > binding.size {
                return Err(EINVAL);
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
                    render::OperandPool::new(vm.kernel.start, vm.kernel.end).ok_or(EINVAL)?;
                // Acquire scarce support and its Arc before editing this
                // root. ENOSPC/ENOMEM here must not quarantine a healthy VM.
                let support = Arc::new(
                    runtime.acquire_render_support(&pool)?,
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
                device: device.into(), _owner: inner.state.clone(),
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
        let mut job = queue.entity.new_job(1, SubmissionJob { execution })?;
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
    fn cover(&self, start: u64, size: u64, permissions: u32) -> Result {
        Mapping::cover(&self.mappings, start, size, permissions)
    }

    fn compute_parameters(&self, c: &uapi::drm_asahi_cmd_compute) -> Result<compute::Parameters> {
        if c.flags != 0 || c.helper.binary != 0 || c.helper.cfg != 0 || c.helper.data != 0 {
            return Err(ENOTSUPP);
        }
        let size = c
            .cdm_ctrl_stream_end
            .checked_sub(c.cdm_ctrl_stream_base)
            .ok_or(EINVAL)?;
        if (c.cdm_ctrl_stream_base | c.cdm_ctrl_stream_end) & 3 != 0
            || size < 4
            || c.sampler_heap & 7 != 0
            || c.sampler_count > 1024
            || (c.sampler_count == 0) != (c.sampler_heap == 0)
        {
            return Err(EINVAL);
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

    fn render_parameters(&self, c: &uapi::drm_asahi_cmd_render) -> Result<render::Parameters> {
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
            return Err(ENOTSUPP);
        }
        let mut p = render::Parameters::default();
        p.tgid = kernel::current!().group_leader().pid();
        p.comm = current_comm();
        p.vdm_barrier_fragment = c.flags & VDM_BARRIER_FRAGMENT != 0;
        p.set_private(self.kernel.start, self.kernel.end)
            .ok_or(EINVAL)?;
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
            return Err(EINVAL);
        }
        self.cover(p.encoder, 4, 2)?;
        p.utile_config = (p.utile_width / 16) << 12
            | (p.utile_height / 16) << 14
            | match p.samples {
                1 => 0,
                2 => 1,
                4 => 2,
                _ => return Err(EINVAL),
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
            return Err(EINVAL);
        }
        for (address, access) in [
            (p.scissor_array, 2),
            (p.depth_bias_array, 2),
            (p.occlusion_query_base, 4),
        ] {
            if address != 0 {
                if address & 7 != 0 {
                    return Err(EINVAL);
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
                return Err(EINVAL);
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
            return Err(EINVAL);
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
                    return Err(EINVAL);
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

// SAFETY: These UAPI structures contain only integers and integer arrays.
unsafe impl crate::util::AnyBitPattern for uapi::drm_asahi_cmd_header {}
unsafe impl crate::util::AnyBitPattern for uapi::drm_asahi_cmd_render {}
unsafe impl crate::util::AnyBitPattern for uapi::drm_asahi_cmd_compute {}
unsafe impl crate::util::AnyBitPattern for uapi::drm_asahi_attachment {}

// SAFETY: VM bind records contain only integer fields.
unsafe impl crate::util::AnyBitPattern for uapi::drm_asahi_gem_bind_op {}
