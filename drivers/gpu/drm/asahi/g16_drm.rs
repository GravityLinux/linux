// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from Niklas Sheth's linux-m4-integration prototype.

//! G16 DRM frontend. Firmware initialization and queue ownership stay in the
//! G16 runtime; GEM storage uses the common DRM shmem implementation.
//! This file holds the driver, file, VM and job types; the submodules hold
//! the ioctls (`ioctl_impl`), VM mappings (`bind`), SUBMIT (`submit_impl`)
//! and the submission pump (`pump`).

use crate::{g16_compute as compute, g16_render as render};
use core::{
    ops::Range,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering},
};
use kernel::dma_fence::{self, RawDmaFence};
use kernel::drm::sched;
use kernel::drm::{
    gem::{shmem, BaseObject, DriverObject},
    ioctl,
};
use kernel::sync::{Arc, CondVar, Mutex, MutexGuard};
use kernel::error::to_result;
use kernel::sync::aref::ARef;
use kernel::uaccess::{UserPtr, UserSlice};
use kernel::workqueue::{self, impl_has_work, new_work, Work, WorkItem};
use kernel::{bindings, c_str, device::{Bound, Core}, drm, platform, prelude::*, scatterlist, uapi};

#[path = "g16_drm_ioctl.rs"]
mod ioctl_impl;
#[path = "g16_drm_bind.rs"]
mod bind;
#[path = "g16_drm_submit.rs"]
mod submit_impl;
#[path = "g16_drm_pump.rs"]
mod pump;

// Downstream UAPI values (asahi_drm.h) are single bits allocated from bit 30
// downwards; upstream's start at bit 0. Keep the two ranges apart.
const _: () = {
    use uapi::*;
    let upstream = drm_asahi_feature_DRM_ASAHI_FEATURE_SOFT_FAULTS as u64
        | drm_asahi_render_flags_DRM_ASAHI_RENDER_VERTEX_SCRATCH as u64
        | drm_asahi_render_flags_DRM_ASAHI_RENDER_PROCESS_EMPTY_TILES as u64
        | drm_asahi_render_flags_DRM_ASAHI_RENDER_NO_VERTEX_CLUSTERING as u64
        | drm_asahi_render_flags_DRM_ASAHI_RENDER_DBIAS_IS_INT as u64;
    let downstream = [
        drm_asahi_feature_DRM_ASAHI_FEATURE_FEW_PRIMITIVES as u64,
        drm_asahi_feature_DRM_ASAHI_FEATURE_FRAGMENT_BARRIER as u64,
        drm_asahi_render_flags_DRM_ASAHI_RENDER_FEW_PRIMITIVES as u64,
        drm_asahi_render_flags_DRM_ASAHI_RENDER_VDM_BARRIER_FRAGMENT as u64,
    ];
    assert!(upstream < 1 << 24);
    let mut i = 0;
    while i < downstream.len() {
        let bit = downstream[i];
        assert!(bit.is_power_of_two() && bit >= 1 << 24 && bit <= 1 << 30);
        i += 1;
    }
};

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
    /// Its object was closed while work was queued: the PTEs stay until the
    /// reaper unmaps them (see g16_deferred), but nothing new may use it.
    closing: bool,
}

impl crate::g16_deferred::Span for Mapping {
    fn range(&self) -> Range<u64> {
        self.range.clone()
    }
    fn permissions(&self) -> u32 {
        self.permissions
    }
    fn closing(&self) -> bool {
        self.closing
    }
}

impl Mapping {
    fn cover(mappings: &[Self], start: u64, size: u64, permissions: u32) -> Result {
        if crate::g16_deferred::covered(mappings, start, size, permissions) {
            Ok(())
        } else {
            Err(reject(format_args!("{:#x}+{:#x} not mapped with access {:#x}", start, size, permissions)))
        }
    }
}

/// A GEM close deferred until every fence pending at the close has signaled.
struct Deferred {
    obj: ARef<Object>,
    fences: KVec<dma_fence::Fence>,
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
    /// The native mesh mode when this VM was created. Its support header and
    /// every decode of its streams use it, whatever the runtime mode is later.
    native_mode: crate::g16_mesh::Mode,
    render_ready: bool,
    /// The runtime holds this VM root's TVB pool (created by its first render
    /// submission, released with the VM).
    tvb_ready: bool,
    compute_ready: bool,
    render_broken: bool,
    /// Closes waiting for their snapshot to complete (see g16_deferred).
    deferred: KVec<Deferred>,
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
    /// Set by file teardown: the reaper must no longer touch the VMs.
    closed: bool,
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
    _queue_cap: Pin<KBox<kernel::debugfs::File<crate::g16_timeline::CapKnob>>>,
    _flags: KVec<Pin<KBox<kernel::debugfs::File<crate::g16_timeline::FlagKnob>>>>,
    _native_mesh: Pin<KBox<kernel::debugfs::File<crate::g16_timeline::NativeMeshKnob>>>,
    _native_mesh_attribute: NativeMeshAttribute,
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

/// sysfs `asahi_native_mesh` on the GPU platform device: the native-index
/// advertisement (`g16_mesh::advertise`), readable by anyone (0444).
struct AttributeCell(core::cell::UnsafeCell<bindings::device_attribute>);

// SAFETY: The attribute is written only here, at compile time; the device core
// only reads it.
unsafe impl Sync for AttributeCell {}

static NATIVE_MESH_ATTRIBUTE: AttributeCell = AttributeCell(core::cell::UnsafeCell::new(bindings::device_attribute {
    attr: bindings::attribute {
        name: c"asahi_native_mesh".as_ptr(),
        mode: 0o444,
        // SAFETY: Zero is valid for the remaining (lockdep) fields.
        ..unsafe { core::mem::zeroed() }
    },
    __bindgen_anon_1: bindings::device_attribute__bindgen_ty_1 { show: Some(show_native_mesh) },
    __bindgen_anon_2: bindings::device_attribute__bindgen_ty_2 { store: None },
}));

/// A bounded line buffer for the advertisement.
struct Line {
    bytes: [u8; crate::g16_mesh::ADVERTISEMENT_MAX],
    len: usize,
}

impl core::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len.checked_add(s.len()).filter(|&end| end <= self.bytes.len()).ok_or(core::fmt::Error)?;
        self.bytes[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

unsafe extern "C" fn show_native_mesh(
    _dev: *mut bindings::device,
    _attr: *mut bindings::device_attribute,
    buf: *mut core::ffi::c_char,
) -> isize {
    let (generation, mode) = crate::g16_mesh::snapshot();
    let mut line = Line { bytes: [0; crate::g16_mesh::ADVERTISEMENT_MAX], len: 0 };
    if crate::g16_mesh::advertise(generation, mode, &mut line).is_err() {
        return EIO.to_errno() as isize;
    }
    // SAFETY: sysfs passes a PAGE_SIZE buffer; the line is at most ADVERTISEMENT_MAX bytes.
    unsafe { core::ptr::copy_nonoverlapping(line.bytes.as_ptr(), buf.cast(), line.len) };
    line.len as isize
}

/// Keeps `asahi_native_mesh` on the device until the driver unbinds.
struct NativeMeshAttribute(*mut bindings::device);

// SAFETY: The pointer is only passed to device_remove_file, which may run on any thread.
unsafe impl Send for NativeMeshAttribute {}
// SAFETY: No method takes `&self`.
unsafe impl Sync for NativeMeshAttribute {}

impl NativeMeshAttribute {
    fn new<C: kernel::device::DeviceContext>(dev: &kernel::device::Device<C>) -> Result<Self> {
        // `Device` is a transparent wrapper of the C `struct device`.
        let raw = core::ptr::from_ref(dev).cast_mut().cast::<bindings::device>();
        // SAFETY: The device is bound; the attribute is static.
        to_result(unsafe { bindings::device_create_file(raw, NATIVE_MESH_ATTRIBUTE.0.get()) })?;
        Ok(Self(raw))
    }
}

impl Drop for NativeMeshAttribute {
    fn drop(&mut self) {
        // SAFETY: Dropped with the driver data at unbind, while the platform
        // device (which outlives its driver data) still exists.
        unsafe { bindings::device_remove_file(self.0, NATIVE_MESH_ATTRIBUTE.0.get()) };
    }
}

pub(crate) fn register<'a>(
    pdev: &'a platform::Device<Core<'_>>,
) -> Result<drm::Registration<'a, Driver>> {
    use kernel::dma::{Device as _, DmaMask};
    // SAFETY: This is the initial probe, before bootstrap, DRM registration
    // or any client DMA, so no DMA allocation or mapping runs concurrently
    // with the mask change. T8132's GPU addresses 42 bits.
    unsafe {
        pdev.dma_set_mask_and_coherent(DmaMask::try_new(42)?)?;
    }
    let runtime = crate::g16::Bootstrap::new(pdev)?;
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
        (c_str!("shared_notifier"), FlagKnob::SHARED_NOTIFIER, crate::module_parameters::shared_notifier.value()),
        (c_str!("fail_prep"), FlagKnob::FAIL_PREP, 0),
        (c_str!("fail_retire"), FlagKnob::FAIL_RETIRE, 0),
        (c_str!("fail_skip"), FlagKnob::FAIL_SKIP, 0),
    ];
    let mut flags = KVec::with_capacity(knobs.len(), GFP_KERNEL)?;
    for (name, knob, value) in knobs {
        knob.init(value);
        flags.push(KBox::pin_init(dir.read_write_file(name, knob), GFP_KERNEL)?, GFP_KERNEL)?;
    }
    crate::g16_mesh::init_mode(crate::module_parameters::native_mesh.value());
    let native_mesh_attribute = NativeMeshAttribute::new(pdev.as_ref())?;
    let native_mesh = KBox::pin_init(
        dir.read_write_file(c_str!("native_mesh"), crate::g16_timeline::NativeMeshKnob),
        GFP_KERNEL,
    )?;
    let dep_stats = KBox::pin_init(
        dir.read_only_file(c_str!("dep_stats"), crate::g16_timeline::DepStats),
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
        scheduler: sched::Scheduler::new(pdev.as_ref(), 4, JOB_CREDITS, 0, 30000, c_str!("asahi_m4"))?,
        admission <- kernel::new_mutex!(KVec::new()),
        failed: AtomicBool::new(false),
        notifications: runtime.notifications.clone(),
        _fw_mem: fw_mem,
        runtime <- kernel::new_mutex!(runtime),
        engine <- kernel::new_mutex!(Engine {
            cursor: 0,
            jobs: KVec::with_capacity(JOB_CREDITS as usize, GFP_KERNEL)?,
            finished: KVec::with_capacity(JOB_CREDITS as usize, GFP_KERNEL)?,
            issued: KVec::with_capacity(crate::g16::MAX_FLIGHTS, GFP_KERNEL)?,
        }),
        inbox <- kernel::new_mutex!(Inbox {
            running: false,
            jobs: KVec::with_capacity(JOB_CREDITS as usize, GFP_KERNEL)?,
        }),
        timeline,
        _profile: profile,
        _queue_cap: queue_cap,
        _flags: flags,
        _native_mesh: native_mesh,
        _native_mesh_attribute: native_mesh_attribute,
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
    fn create_imported(_dev: &Device, _size: usize) -> impl PinInit<Self, Error> {
        Ok(Self { owner: 0, vm: 0 })
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
        if inner.reaper.armed.load(Ordering::Relaxed) {
            reap(&mut state, &inner.device, &inner.reaper.armed);
        }
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
            // A live close with work queued: don't wait. Keep the object's
            // PTEs (its pages are already kept by the queued submissions)
            // until everything pending now has completed, then the reaper
            // unmaps them. Nothing new may use them meanwhile (closing).
            vm.pending.retain(|f| {
                // SAFETY: The vector owns each fence reference during the query.
                !unsafe { bindings::dma_fence_is_signaled(f.raw()) }
            });
            if !vm.pending.is_empty() && crate::g16_deferred::admit(vm.deferred.len()) {
                let mut fences = KVec::new();
                let object = vm.mappings.iter()
                    .find(|m| core::ptr::eq(&*m.obj, obj))
                    .map(|m| m.obj.clone());
                if let Some(object) = object {
                    if fences.extend_from_slice(&vm.pending, GFP_KERNEL).is_ok()
                        && vm.deferred.reserve(1, GFP_KERNEL).is_ok()
                    {
                        for m in vm.mappings.iter_mut().filter(|m| core::ptr::eq(&*m.obj, obj)) {
                            m.closing = true;
                        }
                        // Reserved above: cannot fail.
                        let _ = vm.deferred.push_within_capacity(Deferred { obj: object, fences });
                        // Pairs with signal_finished: it signals (a fully
                        // ordered bit set), then loads `armed`. If the snapshot
                        // completed before this store, it may have missed us;
                        // check once ourselves. The fence orders the store
                        // before the plain loads in dma_fence_is_signaled
                        // (store buffering), so one side always sees the other.
                        inner.reaper.armed.store(true, Ordering::SeqCst);
                        core::sync::atomic::fence(Ordering::SeqCst);
                        let done = vm.deferred.last().is_some_and(|e| crate::g16_deferred::ready(e.fences.iter().map(|f| {
                            // SAFETY: The entry owns each fence reference during the query.
                            unsafe { bindings::dma_fence_is_signaled(f.raw()) }
                        })));
                        if done {
                            let _ = workqueue::system_dfl().enqueue(inner.reaper.clone());
                        }
                        continue;
                    }
                }
                // Out of memory or the entry cap: fall back to waiting.
            }
            // Fallback: wait while the VM's work keeps retiring, failing
            // once none of its fences signals for CLOSE_DRAIN_MS (see
            // drain_within). Only a fatal signal ends the wait early (the
            // process is then exiting anyway); any other signal is ignored,
            // so a healthy client's VM stays usable.
            let (relocked, result, stats) = drain_vm(state, id, CLOSE_DRAIN_MS, true /* killable */, &inner.drained);
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
                pr_err!(
                    "G16: VM {}: drain gave up after {} ms: {} of its fences signaled during the wait, none in the last {} ms\n",
                    vm.id, stats.waited_ms, stats.signaled, stats.idle_ms
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
    raw: AtomicPtr<bindings::drm_file>,
    id: u64,
    state: Arc<Mutex<FileState>>,
    reaper: Arc<Reaper>,
    #[pin]
    drained: CondVar,
}
impl drm::file::DriverFile for File {
    type Driver = Driver;
    fn open(device: &RegDevice) -> Result<Pin<KBox<Self>>> {
        let device: &Device = device;
        let state = Arc::pin_init(
            kernel::new_mutex!(FileState {
                next_vm: 1,
                next_queue: 1,
                next_timestamp: 1,
                queues: KVec::new(),
                timestamps: KVec::new(),
                retained_timestamps: KVec::new(),
                vms: KVec::new(),
                closed: false,
            }),
            GFP_KERNEL,
        )?;
        let reaper_state = state.clone();
        let reaper = Arc::pin_init(
            try_pin_init!(Reaper {
                device: device.into(),
                state: reaper_state,
                armed: AtomicBool::new(false),
                work <- new_work!("m4-reaper"),
            }),
            GFP_KERNEL,
        )?;
        KBox::pin_init(
            try_pin_init!(Self {
                device: device.into(),
                raw: AtomicPtr::new(core::ptr::null_mut()),
                id: NEXT_FILE.fetch_add(1, Ordering::Relaxed),
                drained <- kernel::new_condvar!("m4-file-drain"),
                state,
                reaper,
            }),
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

/// Unmaps a file's deferred GEM closes once their snapshots complete. Every
/// fence in a snapshot is one of this file's job fences, so the pump queues
/// this work when it signals a job of an armed file (signal_finished); file
/// ioctls also reap opportunistically. Holds the file state, not the File.
#[pin_data]
struct Reaper {
    device: DeviceRef,
    state: Arc<Mutex<FileState>>,
    /// Some VM of this file has deferred closes. Changed under file state.
    armed: AtomicBool,
    #[pin]
    work: Work<Reaper>,
}

impl_has_work! { impl HasWork<Self> for Reaper { self.work } }

impl WorkItem for Reaper {
    type Pointer = Arc<Self>;

    fn run(this: Arc<Self>) {
        let mut state = this.state.lock();
        reap(&mut state, &this.device, &this.armed);
    }
}

/// Unmap every deferred close whose snapshot has completed; disarm the
/// reaper once none remain. Called with file state held.
fn reap(state: &mut FileState, device: &Device, armed: &AtomicBool) {
    if state.closed || device.failed.load(Ordering::Acquire) {
        return;
    }
    let mut runtime = None;
    let mut left = false;
    for vm in state.vms.iter_mut() {
        let mut index = 0;
        while index < vm.deferred.len() {
            let done = crate::g16_deferred::ready(vm.deferred[index].fences.iter().map(|f| {
                // SAFETY: The entry owns each fence reference during the query.
                unsafe { bindings::dma_fence_is_signaled(f.raw()) }
            }));
            if !done {
                left = true;
                index += 1;
                continue;
            }
            let entry = vm.deferred.swap_remove(index);
            let runtime = runtime.get_or_insert_with(|| device.runtime.lock());
            if let Err(error) = vm.unmap_object(&entry.obj, runtime) {
                // Page tables may be partly edited: stop the device, as a
                // failed close-time unbind always has.
                device.failed.store(true, Ordering::Release);
                pr_err!("G16: failed to unbind a deferred closed GEM object: {:?}\n", error);
                return;
            }
        }
    }
    if !left {
        armed.store(false, Ordering::Relaxed);
    }
}

#[pinned_drop]
impl PinnedDrop for File {
    fn drop(self: Pin<&mut Self>) {
        // From here on a queued reaper leaves the VMs alone: teardown below
        // drains every pending fence, so deferred closes need no reaping.
        // Entity destruction cancels dependency-blocked jobs. Never hold the
        // state lock while DRM waits for scheduler callbacks.
        let queues = {
            let mut state = self.state.lock();
            state.closed = true;
            self.reaper.armed.store(false, Ordering::Relaxed);
            core::mem::take(&mut state.queues)
        };
        drop(queues);
        // Wait only for this file's own work. Other clients' fences don't
        // protect anything torn down here.
        let mut state = self.state.lock();
        let mut drained = true;
        // One deadline for the whole file: VMs that keep retiring can't add
        // up to more than CAP_MS of uninterruptible exit.
        let start = kernel::time::Instant::<kernel::time::Monotonic>::now();
        for vm in state.vms.iter_mut() {
            // Not interruptible: a killed process has a fatal signal
            // pending, and giving up early would leak its pages.
            let cap = crate::g16_drain::CAP_MS - since_ms(&start);
            let (result, stats) = drain_within(&mut vm.pending, CLOSE_DRAIN_MS, cap, false);
            if let Err(error) = result {
                pr_err!(
                    "G16: closing a client: VM {} drain {:?} after {} ms: {} fences signaled, {} still pending, none in the last {} ms\n",
                    vm.id, error, stats.waited_ms, stats.signaled, stats.remaining, stats.idle_ms
                );
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

static FENCE_KEY: Pin<&kernel::sync::LockClassKey> = kernel::static_lock_class!();

/// SUBMIT rejections logged per boot; all are counted in dep_stats
/// (submit_rejected).
const REJECT_LOG: u64 = 32;

/// EINVAL for a submission, naming the check that failed. A bare EINVAL
/// reached userspace as device loss with nothing in the kernel log.
fn reject(why: core::fmt::Arguments<'_>) -> Error {
    log_rejection(EINVAL, "EINVAL", why)
}

/// ENOTSUPP for a submission using something this driver doesn't implement.
fn unsupported(why: core::fmt::Arguments<'_>) -> Error {
    log_rejection(ENOTSUPP, "ENOTSUPP", why)
}

fn log_rejection(error: Error, name: &str, why: core::fmt::Arguments<'_>) -> Error {
    let n = crate::g16_timeline::SUBMIT_REJECTED.fetch_add(1, Ordering::Relaxed);
    if n < REJECT_LOG {
        let comm = current_comm();
        let len = comm.iter().position(|&b| b == 0).unwrap_or(comm.len());
        pr_warn!(
            "G16: SUBMIT rejected with {}: {} ({} pid {})\n",
            name,
            why,
            core::str::from_utf8(&comm[..len]).unwrap_or("?"),
            kernel::current!().group_leader().pid()
        );
    } else if n == REJECT_LOG {
        pr_warn!("G16: further SUBMIT rejections are not logged (debugfs dep_stats submit_rejected counts them)\n");
    }
    error
}

/// The calling process's name, for hang reports.
fn current_comm() -> [u8; 16] {
    let task = kernel::current!();
    let leader = task.group_leader();
    crate::util::task_comm(leader)
}

/// Milliseconds since `start`.
fn since_ms(start: &kernel::time::Instant<kernel::time::Monotonic>) -> i64 {
    start.elapsed().as_millis()
}

/// Wait for every fence in `pending`, failing with ETIMEDOUT once none has
/// signaled for `ms` (or after `cap_ms` overall), not after `ms` in total:
/// slow work that keeps retiring is not a failure. With `killable`, a fatal
/// signal ends the wait early with EINTR; other signals are ignored,
/// since a GEM close can't be restarted (DRM has already removed the handle)
/// and failing a live client's VM because of SIGIO or a timer would break a
/// healthy process.
fn drain_within(
    pending: &mut KVec<dma_fence::Fence>,
    ms: u32,
    cap_ms: i64,
    killable: bool,
) -> (Result, crate::g16_drain::Stats) {
    use crate::g16_drain::{Clock, Verdict};
    let start = kernel::time::Instant::<kernel::time::Monotonic>::now();
    let mut clock = Clock::new(pending.len(), ms, cap_ms);
    let result = loop {
        pending.retain(|f| {
            // SAFETY: The vector owns each fence reference during the query.
            !unsafe { bindings::dma_fence_is_signaled(f.raw()) }
        });
        let left = match clock.update(since_ms(&start), pending.len()) {
            Verdict::Done => break Ok(()),
            Verdict::NoProgress | Verdict::Capped => break Err(ETIMEDOUT),
            Verdict::Wait(left) => left,
        };
        // Re-check the whole set at least once a second (another fence may
        // signal first) and for a fatal signal every 100 ms.
        let step = kernel::time::msecs_to_jiffies(left.min(if killable { 100 } else { 1000 }) as u32);
        let Some(fence) = pending.first() else { break Ok(()) };
        // SAFETY: The vector owns the fence reference throughout the wait.
        let result = unsafe { bindings::dma_fence_wait_timeout(fence.raw(), false, step.max(1) as _) };
        if result < 0 {
            break Err(Error::from_errno(result as i32));
        }
        if result == 0 && killable && fatal_signal_pending() {
            break Err(EINTR);
        }
    };
    (result, clock.stats())
}

/// Block new submissions to this VM before dropping file state for waits.
/// Overlapping drains share a counter: only the last one releases submitters.
/// Return with state held through the caller's PTE edits, so clearing the
/// barrier cannot let a new submit race those edits. No VM reference or vector
/// index survives an unlock. The drain fails once none of the VM's fences has
/// signaled for `ms` (see drain_within); retries share one progress clock.
fn drain_vm<'a>(
    mut state: MutexGuard<'a, FileState>,
    id: u32,
    ms: u32,
    killable: bool,
    changed: &CondVar,
) -> (MutexGuard<'a, FileState>, Result, crate::g16_drain::Stats) {
    use crate::g16_drain::{Clock, Stats, Verdict};
    let Some(vm) = state.vms.iter_mut().find(|vm| vm.id == id) else {
        return (state, Err(ENOENT), Stats::default());
    };
    let Some(draining) = vm.draining.checked_add(1) else {
        return (state, Err(EOVERFLOW), Stats::default());
    };
    vm.draining = draining;
    let mutex = state.lock_ref();
    let start = kernel::time::Instant::<kernel::time::Monotonic>::now();
    let mut clock = None;
    let result = loop {
        let Some(vm) = state.vms.iter_mut().find(|vm| vm.id == id) else {
            break Err(ENOENT);
        };
        vm.pending.retain(|f| {
            // SAFETY: The vector owns each fence reference during the query.
            !unsafe { bindings::dma_fence_is_signaled(f.raw()) }
        });
        let remaining = vm.pending.len();
        let clock = clock.get_or_insert_with(|| Clock::new(remaining, ms, crate::g16_drain::CAP_MS));
        let left = match clock.update(since_ms(&start), remaining) {
            Verdict::Done => break Ok(()),
            Verdict::NoProgress | Verdict::Capped => break Err(ETIMEDOUT),
            Verdict::Wait(left) => left,
        };
        let Some(fence) = vm.pending.first().cloned() else {
            break Ok(());
        };
        // Re-check the whole set at least once a second (another fence may
        // signal first) and for a fatal signal every 100 ms.
        let step = kernel::time::msecs_to_jiffies(left.min(if killable { 100 } else { 1000 }) as u32);
        drop(state);
        // SAFETY: The local clone owns the fence throughout the unlocked wait.
        let result = unsafe { bindings::dma_fence_wait_timeout(fence.raw(), false, step.max(1) as _) };
        state = mutex.lock();
        if result < 0 {
            break Err(Error::from_errno(result as i32));
        }
        if result == 0 && killable && fatal_signal_pending() {
            break Err(EINTR);
        }
    };
    let stats = clock.map_or_else(Stats::default, |c| c.stats());
    if let Some(vm) = state.vms.iter_mut().find(|vm| vm.id == id) {
        vm.draining -= 1;
        if vm.draining != 0 {
            return (state, result, stats);
        }
    }
    // Also wake after VM removal: submit must re-find its queue and VM.
    // Success, timeout and signal exits all release this drain's barrier.
    changed.notify_all();
    (state, result, stats)
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

/// No-progress budget for close-time drains. A healthy issued Work finishes
/// within the 10 s firmware watchdog, or the device fails anyway; a VM none of
/// whose fences signals for longer is stuck or starved. Deep queues of slow
/// work may take longer in total while they keep retiring.
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
    /// This job's file reaper, queued after its fence signals when armed.
    reaper: Arc<Reaper>,
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
/// CNTVCT ticks per second.
fn tick_hz() -> u64 {
    let value: u64;
    // SAFETY: Reading CNTFRQ_EL0 has no side effects.
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) value) };
    value
}

pub(crate) fn ticks() -> u64 {
    let value: u64;
    // SAFETY: Reading CNTVCT_EL0 has no side effects.
    unsafe { core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) value) };
    value
}

fn execution_tgid(execution: &Execution) -> i32 {
    match execution.parameters.first() {
        Some(Parameters::Render(p)) => p.tgid,
        Some(Parameters::Compute(p)) => p.tgid,
        None => 0,
    }
}

fn trace_record(ring: &crate::g16_timeline::Ring, job: &Running) {
    let t = &job.execution.trace;
    let pid = execution_tgid(&job.execution);
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

/// Scheduler credits; each job takes one, so at most this many jobs are ever
/// between run_job and their fence signalling. The inbox and the engine's job
/// lists are preallocated to it: run_job and the worker never allocate.
const JOB_CREDITS: u32 = 128;

impl<T> crate::g16_finished::Bounded<T> for KVec<T> {
    fn len(&self) -> usize {
        KVec::len(self)
    }
    fn capacity(&self) -> usize {
        KVec::capacity(self)
    }
    fn get_at(&self, index: usize) -> Option<&T> {
        self.get(index)
    }
    fn remove_at(&mut self, index: usize) -> Option<T> {
        self.remove(index).ok()
    }
    fn push_within(&mut self, value: T) -> core::result::Result<(), T> {
        self.push_within_capacity(value).map_err(|e| e.0)
    }
    fn insert_within(&mut self, index: usize, value: T) -> core::result::Result<(), T> {
        use kernel::alloc::kvec::InsertError;
        self.insert_within_capacity(index, value).map_err(|e| match e {
            InsertError::IndexOutOfBounds(value) | InsertError::OutOfCapacity(value) => value,
        })
    }
}

static FINISHED_FULL: AtomicBool = AtomicBool::new(false);
static ADMIT_FULL: AtomicBool = AtomicBool::new(false);

/// A bookkeeping bound was exceeded: report it once, and let the caller defer
/// the work rather than fail the device.
fn warn_once(flag: &AtomicBool, what: &str) {
    if !flag.swap(true, Ordering::Relaxed) {
        pr_warn!("G16: {} (bound JOB_CREDITS = {})\n", what, JOB_CREDITS);
    }
}

struct Engine {
    cursor: usize,
    jobs: KVec<Running>,
    /// take_finished's reusable output (empty between passes).
    finished: KVec<Running>,
    /// One entry per published Work, bounded like the flights (MAX_FLIGHTS).
    issued: KVec<Issued>,
}
struct Inbox {
    /// A pump is queued or running; it rechecks the inbox before stopping.
    running: bool,
    jobs: KVec<Running>,
}

struct SubmissionJob {
    execution: Arc<Execution>,
    /// The job's publication history, allocated by SUBMIT so run_job does not
    /// allocate (it is in the fence-signalling path).
    history: Option<[KVec<Option<crate::g16::Stamp>>; 2]>,
}

struct Completion;
struct OutputSync {
    object: drm::syncobj::SyncObj,
    chain: Option<dma_fence::FenceChain>,
    point: u64,
}

// SAFETY: These UAPI structures contain only integers and integer arrays.
unsafe impl crate::util::AnyBitPattern for uapi::drm_asahi_cmd_header {}
unsafe impl crate::util::AnyBitPattern for uapi::drm_asahi_cmd_render {}
unsafe impl crate::util::AnyBitPattern for uapi::drm_asahi_cmd_compute {}
unsafe impl crate::util::AnyBitPattern for uapi::drm_asahi_attachment {}

// SAFETY: VM bind records contain only integer fields.
unsafe impl crate::util::AnyBitPattern for uapi::drm_asahi_gem_bind_op {}
