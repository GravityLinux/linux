// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Dual-ASC management boot for the synchronous G17P shim port.
//! Both transports exist before either CPU starts. Descriptor publication is
//! separate: successful RTKit boot is not proof that GPU work can execute.

use super::{
    g17p_compute_runtime as compute,
    g17p_image::Image,
    g17p_layout as layout,
    g17p_memory::{self, Memory},
    g17p_opening as opening,
    g17p_platform::Platform,
    g17p_queue as queue, g17p_render_runtime as render,
    g17p_vm::Vm,
};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use kernel::{
    c_str,
    device::Core,
    devres::Devres,
    dma_fence::RawDmaFence,
    io::{self, mem::IoMem, Io},
    iosys_map::IoSysMapRef,
    platform,
    prelude::*,
    rbtree::{RBTree, RBTreeNode},
    soc::apple::rtkit,
    sync::{aref::ARef, Arc},
    types::ForeignOwnable,
};

struct Data {
    dev: ARef<platform::Device>,
    name: &'static CStr,
    firmware_region: &'static CStr,
    crashed: AtomicBool,
    last_message: AtomicU64,
    acknowledged: AtomicBool,
    wakeups: AtomicU64,
    events: Arc<super::g17p_drm::asynchronous::Events>,
}

struct CrashBuffer {
    memory: io::mem::Mem,
    offset: usize,
    size: usize,
    address: usize,
}

impl rtkit::Buffer for CrashBuffer {
    fn iova(&self) -> Result<usize> {
        Ok(self.address)
    }
    fn buf(&mut self) -> Result<IoSysMapRef<'_, u8>> {
        self.memory.as_iosys_map(self.offset, self.size)
    }
}

struct Operations;

#[vtable]
impl rtkit::Operations for Operations {
    type Data = Arc<Data>;
    type Buffer = CrashBuffer;

    fn shmem_map(
        data: <Self::Data as ForeignOwnable>::Borrowed<'_>,
        address: usize,
        size: usize,
    ) -> Result<Self::Buffer> {
        let node = data.dev.as_ref().of_node().ok_or(ENODEV)?;
        let resource = node.reserved_mem_region_to_resource_byname(data.firmware_region)?;
        let offset = address
            .checked_sub(resource.start() as usize)
            .ok_or(EINVAL)?;
        if size == 0 || offset.checked_add(size).ok_or(EINVAL)? > resource.size() as usize {
            return Err(EINVAL);
        }
        // SAFETY: This is the preallocated buffer advertised by this ASC,
        // bounded inside its own loader-reserved firmware RAM. It introduces
        // no new DMA address or allocator-owned memory.
        let memory = unsafe { io::mem::Mem::try_new(resource, io::mem::MemFlag::WB.into())? };
        dev_info!(
            data.dev.as_ref(),
            "G17P: {} preallocated RTKit buffer {:#x}+{:#x}\n",
            data.name,
            address,
            size
        );
        Ok(CrashBuffer {
            memory,
            offset,
            size,
            address,
        })
    }

    fn recv_message(
        data: <Self::Data as ForeignOwnable>::Borrowed<'_>,
        endpoint: u8,
        message: u64,
    ) {
        data.last_message.store(message, Ordering::Release);
        data.events.wake();
        if endpoint == 0x20 && message >> 48 == 0x09 {
            data.acknowledged.store(true, Ordering::Release);
        }
        if endpoint == 0x20 && message >> 48 == 0x42 {
            let count = data.wakeups.fetch_add(1, Ordering::Relaxed) + 1;
            // A wakeup is neither a work ID nor evidence of completion. Bound
            // console traffic while keeping every callback counted.
            if count > 4 && !count.is_power_of_two() {
                return;
            }
        }
        dev_info!(
            data.dev.as_ref(),
            "G17P: {} endpoint {:#x} message {:#018x}\n",
            data.name,
            endpoint,
            message
        );
    }

    fn crashed(data: <Self::Data as ForeignOwnable>::Borrowed<'_>, _crashlog: Option<&[u8]>) {
        data.crashed.store(true, Ordering::Release);
        data.events.fail(EIO);
        dev_err!(data.dev.as_ref(), "G17P: {} firmware crashed\n", data.name);
    }
}

struct Peer {
    asc: Pin<KBox<Devres<IoMem<0x4000>>>>,
    rtkit: Option<Pin<KBox<rtkit::RtKit<Operations>>>>,
    data: Arc<Data>,
    control: u32,
    started: bool,
}

#[path = "g17p_bootstrap.rs"]
mod bootstrap;

#[path = "g17p_cleanup_session.rs"]
#[allow(dead_code)]
mod cleanup;
#[path = "g17p_render_fault_session.rs"]
#[allow(dead_code)]
mod fault;
#[path = "g17p_dependency_session.rs"]
#[allow(dead_code)]
mod native;
#[path = "g17p_relocation_session.rs"]
mod relocation;
#[path = "g17p_render_context_session.rs"]
mod render_contexts;
pub(crate) use render_contexts::PreparedPoolRebind;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prepared,
    Starting,
    Running,
    Failed,
}

pub(crate) struct Session {
    phase: Phase,
    ttbs: u64,
    independent_compute: super::g17p_compute_queues::Queues,
    independent_pending: KVec<IndependentPending>,
    independent_count: u32,
    defer_cache: bool,
    completions: KVec<Completion>,
    render_cpu_leases: KVec<(u32, kernel::dma_fence::Fence)>,
    render_prepare_leases: KVec<(u32, Arc<AtomicBool>)>,
    compute: Option<compute::Submission>,
    compute_contexts: Option<super::g17p_context::NativeComputeContexts>,
    render_contexts: Option<super::g17p_context::NativeRenderContexts>,
    render_clients: KVec<compute::Client>,
    // Each ordinary pool has an independently installed root and ASID.
    // The selected pool's client is in render.client; others stay parked.
    render_pool_clients: KVec<(u32, compute::Client)>,
    render_pool_asids: [u16; super::g17p_render_lifecycle::POOL_SLOTS as usize],
    render_fallback_leaves: Option<Arc<KVec<(u64, u64)>>>,
    render_gate: Option<kernel::dma_fence::Fence>,
    render: Option<render::Submission>,
    dormant_render: Option<render::Submission>,
    native: Option<native::State>,
    bootstrapped: bool,
    faults: fault::State,
    cleanup: cleanup::Receipts,
    submission_error: Option<Error>,
    submissions: usize,
    // Published backing remains session-owned even after GEM close/unmap.
    // Submission paths may append temporary duplicate references; retiring
    // those duplicates must preserve the permanent owners established below.
    retained_buffers: KVec<kernel::sync::aref::ARef<super::g17p_drm::Object>>,
    // Compare-only identity keys for the permanent owners above. No backing
    // reference lives in this index; retained_buffers owns the allocations.
    retained_buffer_ids: RBTree<usize, ()>,
    timestamps: Option<super::g17p_timestamp::Registry>,
    peers: KVec<Peer>,
    memory: Option<Memory>,
    vm: Option<Vm>,
    pending: Option<PendingWork>,
    compute_pending: Option<ComputePending>,
    render_control: Option<KBox<RenderControl>>,
    events: Arc<super::g17p_drm::asynchronous::Events>,
}

enum PendingWork {
    Render(RenderWave),
    Control(KBox<RenderControl>),
    Native,
}
struct RenderControl {
    ordinal: u32,
    announced: bool,
    before: u8,
    target: u8,
    parameters: super::g17p_render::Parameters,
    prepared: Option<KBox<render::PreparedAppend>>,
    receipt: Arc<RenderReceipt>,
    started: kernel::time::Instant<kernel::time::Monotonic>,
}
struct RenderWave {
    frames: KVec<RenderPending>,
    next: usize,
}
pub(crate) struct RenderPublication {
    pub(crate) ordinal: u32,
    pub(crate) milestones: [render::Milestone; 2],
    pub(crate) fence: kernel::dma_fence::Fence,
}
/// Accepted publication identity. Its fence is completion, independent of
/// public submission fences; publication readiness does not imply retirement.
pub(crate) struct RenderReceipt {
    pub(crate) ordinal: u32,
    pub(crate) fence: kernel::dma_fence::Fence,
    publication_ready: AtomicBool,
    milestones: [core::sync::atomic::AtomicU64; 2],
}
impl RenderReceipt {
    fn new(ordinal: u32) -> Result<Arc<Self>> {
        Ok(Arc::new(Self { ordinal, fence: super::g17p_sync::work_fence()?,
            publication_ready: AtomicBool::new(false),
            milestones: [core::sync::atomic::AtomicU64::new(0), core::sync::atomic::AtomicU64::new(0)] }, GFP_KERNEL)?)
    }
    pub(crate) fn is_published(&self) -> bool {
        self.publication_ready.load(Ordering::Acquire)
    }
    pub(crate) fn milestone(&self, engine: usize) -> Option<render::Milestone> {
        if !self.is_published() { return None; }
        let value = self.milestones[engine].load(Ordering::Relaxed);
        if value == 0 { return None; } // Special/native serialized profile.
        Some(render::Milestone { event_slot: (value >> 40) as u8,
            grid: (value >> 32) as u8, value: value as u32 })
    }
    fn fail(&self, error: Error) {
        // Runtime mutex serializes completion/error publication.
        if unsafe { kernel::bindings::dma_fence_get_status(self.fence.raw()) } == 0 {
            self.fence.set_error(error);
            self.fence.signal();
        }
    }
}
/// One accepted CL command owns both publication and retirement identity.
pub(crate) struct ComputeReceipt {
    pub(crate) ordinal: u32,
    pub(crate) fence: kernel::dma_fence::Fence,
    publication_ready: AtomicBool,
    point: core::sync::atomic::AtomicU64,
}
impl ComputeReceipt {
    fn new(ordinal: u32) -> Result<Arc<Self>> {
        Ok(Arc::new(Self { ordinal, fence: super::g17p_sync::work_fence()?,
            publication_ready: AtomicBool::new(false),
            point: core::sync::atomic::AtomicU64::new(0) }, GFP_KERNEL)?)
    }
    pub(crate) fn is_published(&self) -> bool { self.publication_ready.load(Ordering::Acquire) }
    pub(crate) fn milestone(&self) -> Option<render::Milestone> {
        if !self.is_published() { return None; }
        let p = self.point.load(Ordering::Relaxed);
        if p == 0 { return None; }
        Some(render::Milestone { event_slot: (p >> 40) as u8, grid: (p >> 32) as u8, value: p as u32 })
    }
    fn fail(&self, error: Error) {
        if unsafe { kernel::bindings::dma_fence_get_status(self.fence.raw()) } == 0 {
            self.fence.set_error(error); self.fence.signal();
        }
    }
}
struct RenderPending {
    receipt: Arc<RenderReceipt>,
    ticket: render::Ticket,
    gate: kernel::dma_fence::Fence,
    startup: [Report; 2],
    terminal_baseline: u32,
    control_done: bool,
    started: kernel::time::Instant<kernel::time::Monotonic>,
}
struct IndependentPending {
    ticket: super::g17p_compute_queues::Ticket,
    receipt: Arc<ComputeReceipt>,
    started: kernel::time::Instant<kernel::time::Monotonic>,
    outer_done: [bool; 2],
}
/// GPU retirement has been verified under owner/state locking. These exact
/// caller/root/alias leases travel to the calling worker; CPU maps and cache
/// visibility happen without the device mutex. Fence success follows them.
pub(crate) struct Completion {
    client: compute::ClientLease,
    stamps: super::g17p_timestamp::Cache,
    fence: kernel::dma_fence::Fence,
    grid: u8,
    error: Option<Error>,
}
static CACHE_PAUSED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
impl Completion {
    pub(crate) fn cache(&mut self) -> Result {
        if *crate::module_parameters::cpu_cache_pause_grid.value() == u32::from(self.grid)
            && !CACHE_PAUSED.swap(true, Ordering::AcqRel) {
            pr_info!("G17P: CPU_CACHE_PAUSE_BEGIN grid {} owner {:?} outside_runtime_lock\n", self.grid, self.client.owner);
            kernel::time::delay::fsleep(kernel::time::Delta::from_millis(2000));
            pr_info!("G17P: CPU_CACHE_PAUSE_END grid {} owner {:?} outside_runtime_lock\n", self.grid, self.client.owner);
        }
        let result = self.client.cache(true).and_then(|()| self.stamps.run(true));
        self.error = result.err();
        result
    }
    /// Finish under the brief device state lock, which also serializes fatal
    /// receipt failure. Bulk CPU work above has already released that lock.
    pub(crate) fn signal(self) {
        if unsafe { kernel::bindings::dma_fence_get_status(self.fence.raw()) } == 0 {
            if let Some(error) = self.error { self.fence.set_error(error); }
            self.fence.signal();
        }
    }
}

struct ComputePending {
    frames: KVec<compute::Pending>,
    gates: KVec<kernel::dma_fence::Fence>,
    receipts: KVec<Arc<ComputeReceipt>>,
    terminal_baselines: KVec<u32>,
    startup: [Report; 2],
    next: usize,
    started: kernel::time::Instant<kernel::time::Monotonic>,
}
struct Report {
    host: u32,
    firmware: u32,
    records: KVec<[u8; 0x48]>,
    peer_credits: [u32; 2],
}
fn configure_growth_limits(service: &mut super::g17p_growth_runtime::Service) -> Result {
    service.set_source_pool_limits(
        *crate::module_parameters::tvb_max_blocks.value(),
        [
            *crate::module_parameters::tvb_max_blocks_pool0.value(),
            *crate::module_parameters::tvb_max_blocks_pool1.value(),
        ],
    )
}

fn source_fragment_parameters(
    p: &super::g17p_render::Parameters,
    first: bool,
) -> Result<super::g17p_render::Parameters> {
    let selector = if first {
        *crate::module_parameters::first_render_fragment_sync_grow.value()
    } else {
        *crate::module_parameters::repeat_fragment_sync_grow.value()
    };
    let mut configured = *p;
    configured.fragment_sync_grow = match selector {
        0 => Some(false),
        1 => Some(true),
        2 => p.fragment_sync_grow,
        _ => return Err(EINVAL),
    };
    Ok(configured)
}
impl Report {
    fn new() -> Self {
        Self {
            host: 0,
            firmware: 0,
            records: KVec::new(),
            peer_credits: [0; 2],
        }
    }
}

impl Session {
    /// The synchronous source adapter allocates BO physical backing once and
    /// does not return it on GEM close. Keep the same physical lifetime: queue
    /// retirement permits unmapping, but does not certify that a firmware
    /// execution context has released all cached references to the allocation.
    /// Session shutdown stops both peers before releasing these owners.
    pub(crate) fn retain_source_backing(&mut self, bo: &ARef<super::g17p_drm::Object>) -> Result {
        let identity = &**bo as *const super::g17p_drm::Object as usize;
        if self.retained_buffer_ids.get(&identity).is_some() {
            return Ok(());
        }
        // Allocate before recording membership. The permanent ARef prevents
        // this address from being reused while its key remains in the index.
        // Live tickets retain independent execution ARefs. This permanent
        // vector is never truncated by retirement, including prefix retirement
        // when a newer accepted job has added a different permanent owner.
        let node = RBTreeNode::new(identity, (), GFP_KERNEL)?;
        self.retained_buffers.reserve(1, GFP_KERNEL)?;
        self.retained_buffers.push(bo.clone(), GFP_KERNEL)?;
        self.retained_buffer_ids.insert(node);
        Ok(())
    }

    pub(crate) fn prepare(
        pdev: &platform::Device<Core>,
        platform: &Platform,
        sgx: &Devres<IoMem<0x4000000>>,
        image: &Image,
    ) -> Result<KBox<Self>> {
        let dev = pdev.as_ref();
        let events = super::g17p_drm::asynchronous::Events::new()?;
        // The combined source owners exceed the probe's stack budget when
        // moved through Runtime/Mutex by value. Keep their lifetime unchanged
        // while placing the session on the kernel heap.
        let mut session = KBox::new(
            Self {
                phase: Phase::Prepared,
                ttbs: platform.regions[0].base,
                independent_compute: super::g17p_compute_queues::Queues::new(),
                independent_pending: KVec::new(),
                independent_count: 0,
                defer_cache: false,
                completions: KVec::new(),
                render_cpu_leases: KVec::new(),
                render_prepare_leases: KVec::new(),
                compute: None,
                compute_contexts: None,
                render_contexts: None,
                render_clients: KVec::new(),
                render_pool_clients: KVec::new(),
                render_pool_asids: [0; super::g17p_render_lifecycle::POOL_SLOTS as usize],
                render_fallback_leaves: None,
                render_gate: None,
                render: None,
                dormant_render: None,
                native: None,
                bootstrapped: false,
                faults: fault::State::new(),
                cleanup: cleanup::Receipts::new(),
                submission_error: None,
                submissions: 0,
                retained_buffers: KVec::new(),
                retained_buffer_ids: RBTree::new(),
                timestamps: Some(super::g17p_timestamp::Registry::new()),
                peers: KVec::new(),
                memory: Some(Memory::new(dev, platform)?),
                vm: None,
                pending: None,
                compute_pending: None,
                render_control: None,
                events: events.clone(),
            },
            GFP_KERNEL,
        )?;
        session.peers.reserve(2, GFP_KERNEL)?;
        for (name, registers, firmware_region) in [
            (c_str!("primary"), c_str!("asc"), c_str!("firmware")),
            (
                c_str!("secondary"),
                c_str!("asc-secondary"),
                c_str!("firmware-secondary"),
            ),
        ] {
            let asc = KBox::pin_init(
                pdev.io_request_by_name(registers)
                    .ok_or(ENODEV)?
                    .iomap_sized::<0x4000>(),
                GFP_KERNEL,
            )?;
            let regs = asc.try_access().ok_or(ENODEV)?;
            let control = regs.read32(0x44);
            let status = regs.read32(0x48);
            dev_info!(
                dev,
                "G17P: {} cold ASC control={:#x} status={:#x}\n",
                name,
                control,
                status
            );
            if control & 0x10 != 0 || status & 3 != 2 {
                return Err(EBUSY);
            }
            drop(regs);
            let data = Arc::new(
                Data {
                    dev: pdev.into(),
                    name,
                    firmware_region,
                    crashed: AtomicBool::new(false),
                    last_message: AtomicU64::new(0),
                    acknowledged: AtomicBool::new(false),
                    wakeups: AtomicU64::new(0),
                    events: events.clone(),
                },
                GFP_KERNEL,
            )?;
            session.peers.push(
                Peer {
                    asc,
                    rtkit: None,
                    data,
                    control,
                    started: false,
                },
                GFP_KERNEL,
            )?;
        }
        {
            let regs = sgx.try_access().ok_or(ENODEV)?;
            for offset in [0x1000104, 0x1000108] {
                regs.write32(regs.read32(offset) | 1, offset);
            }
            dev_info!(
                dev,
                "G17P: AXI transition registers {:#x}/{:#x}\n",
                regs.read32(0x1000104),
                regs.read32(0x1000108)
            );
        }
        session
            .memory
            .as_mut()
            .ok_or(EINVAL)?
            .prepare_cold(platform)?;
        // Match Python's construction order; it initializes both mailbox
        // transports before starting the primary and then the secondary CPU.
        for peer in &mut session.peers {
            let mut rtkit = KBox::pin(
                rtkit::RtKit::<Operations>::new(dev, Some(peer.data.name), 0, peer.data.clone())?,
                GFP_KERNEL,
            )?;
            rtkit.as_mut().set_early_crashlog();
            peer.rtkit = Some(rtkit);
        }
        for peer in &mut session.peers {
            let regs = peer.asc.try_access().ok_or(ENODEV)?;
            peer.started = true;
            regs.write32(peer.control | 0x10, 0x44);
            drop(regs);
            g17p_memory::sync();
            let rtkit = peer.rtkit.as_mut().ok_or(EINVAL)?;
            rtkit.as_mut().boot()?;
            if peer.data.crashed.load(Ordering::Acquire) {
                return Err(EIO);
            }
            for endpoint in [0x20, 0x21] {
                if !rtkit.as_mut().has_endpoint(endpoint) {
                    return Err(ENODEV);
                }
            }
            dev_info!(
                dev,
                "G17P: {} RTKit management boot complete; graphics endpoints advertised\n",
                peer.data.name
            );
        }
        let Self { memory, peers, .. } = &mut *session;
        let memory = memory.as_mut().ok_or(EINVAL)?;
        memory.write_handoff(platform)?;
        for (index, peer) in peers.iter().enumerate() {
            let root = platform.regions[1].base + index as u64 * 0x40000;
            dev_info!(
                dev,
                "G17P: {} firmware root {:#x}: {:#018x} {:#018x} {:#018x}\n",
                peer.data.name,
                root,
                memory.read64(root)?,
                memory.read64(root + 8)?,
                memory.read64(root + 16)?
            );
        }
        let vm = Vm::build(dev, memory, platform, image)?;
        if *crate::module_parameters::compute_queues.value()==1
            && *crate::module_parameters::native_compute_vms.value()==0
            && *crate::module_parameters::native_render_vms.value()==0
            && *crate::module_parameters::native_barriers.value()==0 {
            let mut dispatch=[0;0x20];
            super::g17p_abi::compute_dispatch(&mut dispatch).map_err(|_|EINVAL)?;
            vm.write(memory,2,0xfffffc20015e8020,&dispatch)?;
        }
        session.render_fallback_leaves = Some(Arc::new(vm.render_fallback_leaves(memory)?, GFP_KERNEL)?);
        session.vm = Some(vm);
        Ok(session)
    }

    /// Stage all first-work objects before exposing either root to firmware.
    /// Preparation failure leaves the session unpublished and cannot trigger
    /// a GPU access to a partially constructed graph.
    pub(crate) fn stage<T>(
        &mut self,
        prepare: impl FnOnce(&mut Memory, &mut Vm) -> Result<T>,
    ) -> Result<T> {
        if self.phase != Phase::Prepared {
            return Err(EBUSY);
        }
        prepare(
            self.memory.as_mut().ok_or(EINVAL)?,
            self.vm.as_mut().ok_or(EINVAL)?,
        )
    }

    pub(crate) fn require_first_work(&self) -> Result {
        if self.phase == Phase::Failed {
            return Err(EIO);
        }
        if self.phase != Phase::Prepared || self.compute.is_some() || self.render.is_some() {
            return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
        }
        Ok(())
    }

    pub(crate) fn bind_timestamp(
        &mut self,
        bo: ARef<super::g17p_drm::Object>,
        offset: u64,
        size: u64,
    ) -> Result<u64> {
        if self.phase == Phase::Failed {
            return Err(EIO);
        }
        if self.phase != Phase::Prepared && self.phase != Phase::Running {
            return Err(EBUSY);
        }
        self.timestamps.as_mut().ok_or(EINVAL)?.bind(
            self.memory.as_mut().ok_or(EINVAL)?,
            self.vm.as_mut().ok_or(EINVAL)?,
            bo,
            offset,
            size,
        )
    }

    pub(crate) fn unbind_timestamp(&mut self, address: u64) -> Result {
        self.timestamps.as_mut().ok_or(EINVAL)?.unbind(
            self.memory.as_ref().ok_or(EINVAL)?,
            self.vm.as_ref().ok_or(EINVAL)?,
            address,
            self.phase == Phase::Failed,
        )
    }

    pub(crate) fn compute_client(&self) -> Result<Option<&compute::Client>> {
        self.cleanup.require_idle()?;
        if self.phase == Phase::Failed {
            return Err(EIO);
        }
        if self.native.as_ref().is_some_and(|owner| !owner.complete()) {
            return Err(EBUSY);
        }
        if self.phase != Phase::Prepared && self.phase != Phase::Running {
            return Err(EBUSY);
        }
        Ok(self.compute.as_ref().map(|work| &work.client))
    }

    pub(crate) fn require_compute_owner(&self, _owner: (u64, u32)) -> Result {
        // Ordinary source compute has its own admitted 2/3 root. Its logical
        // VM can differ from the quiescent render VM in registered slot one.
        self.compute_client()?;
        Ok(())
    }

    pub(crate) fn unbind_native_compute(
        &mut self,
        owner: (u64, u32),
        start: u64,
        size: u64,
        expected: &[(u64, u64)],
    ) -> Result {
        let Some(contexts) = self.compute_contexts.as_mut() else {
            return Ok(());
        };
        if self.phase != Phase::Running {
            return Err(EIO);
        }
        self.cleanup.require_idle()?;
        let work = self.compute.as_mut().ok_or(EINVAL)?;
        compute::idle(
            self.memory.as_ref().ok_or(EINVAL)?,
            self.vm.as_ref().ok_or(EINVAL)?,
            work,
        )?;
        contexts.unbind_vm(owner, &mut work.client.root, start, size, expected)
    }

    pub(crate) fn unbind_vm(
        &mut self,
        owner: (u64, u32),
        start: u64,
        size: u64,
        expected: &[(u64, u64)],
    ) -> Result {
        if self.phase == Phase::Failed {
            return Ok(());
        }
        self.restore_owned_render_fault()?;
        self.cleanup.require_idle()?;
        if let Some(contexts) = self.render_contexts.as_mut() {
            contexts.require_idle(
                owner,
                self.render
                    .as_ref()
                    .ok_or(EINVAL)?
                    .growth
                    .as_ref()
                    .ok_or(EINVAL)?,
            )?;
        }
        self.independent_compute.unbind(owner,start,size)?;
        if self.compute_contexts.is_some() {
            self.unbind_native_compute(owner, start, size, expected)?;
        } else if let Some(work) = self.compute.as_mut() {
            if work.client.owner == owner {
                compute::idle(
                    self.memory.as_ref().ok_or(EINVAL)?,
                    self.vm.as_ref().ok_or(EINVAL)?,
                    work,
                )?;
                work.client
                    .unbind(start, size, expected, false, &[1, 2, 3])?;
            }
        }
        if let Some(work) = self.render.as_mut() {
            if work.client.owner == owner {
                work.client
                    .unbind(start, size, expected, true, &[work.layout.context as u16])?;
            }
        }
        for (pair, client) in &mut self.render_pool_clients {
            if client.owner == owner {
                client.unbind(start, size, expected, true, &[self.render_pool_asids[*pair as usize]])?;
            }
        }
        if let Some(client) = self.render_clients.iter_mut().find(|c| c.owner == owner) {
            let slot = if let Some(contexts) = self.render_contexts.as_ref() {
                contexts
                    .contexts
                    .iter()
                    .find(|c| c.owner == owner)
                    .ok_or(EINVAL)?
                    .slot
            } else {
                // Ordinary logical roots take turns in the admitted ASID-one
                // slot. An inactive owner's checked leaves still belong to it.
                1
            };
            client.unbind(start, size, expected, true, &[slot])?;
        }
        Ok(())
    }

    pub(crate) fn compute_remaining(&self, _render_first: bool) -> Result<usize> {
        self.compute_client()?;
        Ok(self.compute.as_ref().map_or(
            (compute::MAX_SUBMISSIONS - u32::from(!_render_first)) as usize,
            |work| (work.capacity() - work.ordinal - 1) as usize,
        ))
    }

    /// Source retained_compute_only skips logical activation for a direct
    /// compute bootstrap, including after its dormant render graph is adopted.
    pub(crate) fn compute_selects_logical_context(&self) -> bool {
        self.render.is_some() && self.compute.as_ref().is_none_or(|work| work.after_render)
    }

    fn prepare_next_compute(
        &mut self,
        dev: &kernel::device::Device,
        replacement: Option<compute::Client>,
        parameters: &compute::Parameters,
        pending: bool,
        dependencies: &[(u8, u32)],
    ) -> Result {
        if self.phase != Phase::Running {
            return Err(EIO);
        }
        self.cleanup.require_idle()?;
        let replacement = if *crate::module_parameters::native_compute_vms.value() == 1 {
            if self.render.is_some() || self.native.is_some() {
                return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
            }
            if let Some(client) = replacement {
                let work = self.compute.as_mut().ok_or(EINVAL)?;
                compute::idle(
                    self.memory.as_ref().ok_or(EINVAL)?,
                    self.vm.as_ref().ok_or(EINVAL)?,
                    work,
                )?;
                if self.compute_contexts.is_none() {
                    if work.client.primer_aliases.is_some() {
                        work.client.retire_primer()?;
                    }
                    self.compute_contexts = Some(super::g17p_context::NativeComputeContexts::new(
                        &work.client.root,
                    ));
                }
                let contexts = self.compute_contexts.as_mut().ok_or(EINVAL)?;
                contexts.reap();
                let index = contexts.bind_vm(
                    &client,
                    &mut work.client.root,
                    parameters.preempt,
                    work.preempt,
                )?;
                work.native_context = Some(contexts.contexts[index].slot);
                contexts.publish_roots(self.memory.as_ref().ok_or(EINVAL)?, self.ttbs)?;
                // Contexts retain every GEM reachable from any installed root.
                // This view supplies only the current command's cache work.
                work.client.buffers = client.buffers;
                work.client.bindings = client.bindings;
                work.client.owner = client.owner;
            }
            None
        } else {
            replacement
        };
        let moving_private = self.compute.as_ref().ok_or(EINVAL)?.preempt != parameters.preempt;
        if moving_private {
            // No accepted live wave or unvalidated pending report may borrow
            // the old private addresses during a quiescent ownership transfer.
            if pending || self.pending.is_some() || self.compute_pending.is_some() { return Err(EBUSY); }
            if *crate::module_parameters::native_compute_vms.value() == 1 {
                return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
            }
            if let Some(render) = &self.render {
                render::quiesce(self.memory.as_mut().ok_or(EINVAL)?,
                    self.vm.as_ref().ok_or(EINVAL)?, render)?;
            }
        }
        let work = self.compute.as_mut().ok_or(EINVAL)?;
        if work.ordinal + 1 >= work.capacity() {
            return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
        }
        if moving_private && replacement.is_none() { return Err(EINVAL); }
        if let Some(client) = replacement {
            compute::idle(
                self.memory.as_ref().ok_or(EINVAL)?,
                self.vm.as_ref().ok_or(EINVAL)?,
                work,
            )?;
            let previous_owner = work.client.owner;
            if moving_private {
                let old_base = work.preempt;
                let old_aliases = work.robustness_aliases()?;
                let new_aliases = work.private_robustness_aliases(parameters.preempt)?;
                let mut private = work.private_carveout_changes(&client, parameters.preempt)?;
                work.append_compatibility_retirements(&mut private)?;
                let owner = client.owner;
                // Preparation resolves every owned page/store in every root.
                // No publication, mapping removal, or state change occurs if
                // a collision, allocation, cache clean, or lookup fails here.
                let compute_plan = work.client.prepare_rebind(
                    &client, false, &[1, 2, 3], &private)?;
                let mut mirrors = KVec::new();
                if let Some(render) = &mut self.render {
                    if let Some(plan) = render_contexts::prepare_robustness_handoff(
                        &mut render.client, &old_aliases, &new_aliases, owner)? {
                        mirrors.push(plan, GFP_KERNEL)?;
                    }
                }
                if let Some(render) = &mut self.dormant_render {
                    if let Some(plan) = render_contexts::prepare_robustness_handoff(
                        &mut render.client, &old_aliases, &new_aliases, owner)? {
                        mirrors.push(plan, GFP_KERNEL)?;
                    }
                }
                for logical in &mut self.render_clients {
                    if let Some(plan) = render_contexts::prepare_robustness_handoff(
                        logical, &old_aliases, &new_aliases, owner)? {
                        mirrors.push(plan, GFP_KERNEL)?;
                    }
                }
                // Every following operation is infallible. Physical backing
                // and firmware-written contents remain unchanged; only DVAs
                // and caller metadata move, followed by native ASID TLBIs.
                for plan in mirrors { plan.commit(); }
                compute_plan.commit();
                work.client.adopt_rebound(client);
                work.adopt_private_carveout(parameters.preempt, new_aliases[0].0);
                work.compatibility_aliases = [None; 2];
                dev_info!(dev, "G17P: retained compute private carveout {:#x} -> {:#x}; eight physical pages retained\n",
                    old_base, work.preempt);
            } else {
                // The source's compute mirror uses this same low root in
                // native slots 2/3. Flush before releasing old GEM references.
                let mut private = KVec::new();
                work.append_compatibility_retirements(&mut private)?;
                work.client.prepare_rebind(&client, false, &[1, 2, 3], &private)?.commit();
                work.client.adopt_rebound(client);
                work.compatibility_aliases = [None; 2];
            }
            dev_info!(dev, "G17P: retained compute caller mappings refreshed\n");
            if previous_owner != work.client.owner {
                dev_info!(
                    dev,
                    "G17P: synchronous compute VM handoff {:?} -> {:?}\n",
                    previous_owner,
                    work.client.owner
                );
            }
        }
        self.mirror_compute_robustness()?;
        let work = self.compute.as_mut().ok_or(EINVAL)?;
        let old_pointers = work.pointers;
        let staged = compute::stage_next(
            self.memory.as_mut().ok_or(EINVAL)?,
            self.vm.as_mut().ok_or(EINVAL)?,
            work,
            parameters,
            pending,
            dependencies,
        );
        if staged.is_ok() && old_pointers != work.pointers {
            dev_info!(
                dev,
                "G17P: retained compute transport handoff at ordinal {}: {:#x} -> {:#x}\n",
                work.ordinal,
                old_pointers,
                work.pointers
            );
        }
        let result = staged;
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result
    }

    fn prepare_compute(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        client: compute::Client,
        parameters: &compute::Parameters,
    ) -> Result {
        let after_render = self.render.is_some();
        self.require_compute_owner(client.owner)?;
        if after_render {
            if self.phase != Phase::Running || self.compute.is_some() {
                return Err(EIO);
            }
            let memory = self.memory.as_mut().ok_or(EINVAL)?;
            let vm = self.vm.as_ref().ok_or(EINVAL)?;
            render::quiesce(memory, vm, self.render.as_ref().ok_or(EINVAL)?)?;
            let control = image.graph.channels[0][12];
            let mut counts = [0; 3];
            for (value, address) in counts.iter_mut().zip(control.states) {
                *value = memory.read_firmware32(vm.physical(memory, 2, address)?)?;
            }
            if counts[0] != counts[1] || counts[1] != counts[2] {
                return Err(EBUSY);
            }
            dev_info!(
                dev,
                "G17P: post-render compute retains control history {:?}\n",
                counts
            );
        } else {
            self.require_first_work()?;
        }
        let ttbs = self.ttbs;
        if !after_render && self.dormant_render.is_none() {
            // Source compute-first partial startup owns an empty render graph.
            // Construct it before firmware can cache a queue identity. There
            // are no caller programs or resources and no render publication.
            let client = compute::Client {
                root: super::g17p_user_vm::UserVm::new()?,
                buffers: KVec::new(),
                bindings: KVec::new(),
                owner: client.owner,
                cpu_maps: crate::g17p_compute_runtime::CpuMaps::new()?,
                primer_aliases: None,
            };
            let p = super::g17p_render::Parameters {
                width: 1,
                height: 1,
                // Empty source-owned address, replaced before publication.
                encoder: 0x1000000000,
                ..render::first_parameters()
            };
            self.dormant_render = Some(render::build(
                self.memory.as_mut().ok_or(EINVAL)?,
                self.vm.as_mut().ok_or(EINVAL)?,
                image,
                ttbs,
                client,
                &p,
            )?);
        }
        let built = compute::build(
            self.memory.as_mut().ok_or(EINVAL)?,
            self.vm.as_mut().ok_or(EINVAL)?,
            image,
            ttbs,
            client,
            parameters,
            after_render,
        );
        if built.is_err() && after_render {
            self.phase = Phase::Failed;
        }
        let mut work = built.inspect_err(|error| {
            dev_err!(
                dev,
                "G17P: first compute graph preparation failed: {:?}\n",
                error
            );
        })?;
        // Paired-profile diagnostic: retain the previously qualified ordinary
        // post-render CS gate0 alongside retained render completion1. This is
        // a profile comparison, not a claim about the hardware bit meaning.
        // Opening and cold compute serializers do not consume this field.
        work.retained_execution_gate = 0;
        // Ownership precedes the first mailbox publication. Even an initdata
        // timeout or firmware error keeps every reachable client page pinned.
        self.compute = Some(work);
        self.mirror_compute_robustness()
    }

    pub(crate) fn render_client(&self) -> Result<Option<&compute::Client>> {
        self.cleanup.require_idle()?;
        if self.phase == Phase::Failed {
            return Err(EIO);
        }
        if self.native.as_ref().is_some_and(|owner| !owner.complete()) {
            return Err(EBUSY);
        }
        Ok(self.render.as_ref().map(|work| &work.client))
    }
    pub(crate) fn render_remaining(&self) -> Result<u32> {
        self.render_client()?;
        // Serialized ordinary submissions reuse retired backing. Admission is
        // per command; no lifetime count is derived from physical ring size.
        Ok(self.render.as_ref().map_or(u32::MAX, |work| {
            if work.layout.native {
                super::g17p_render_lifecycle::NATIVE_SUBMISSIONS - work.ordinal - 1
            } else {
                u32::MAX
            }
        }))
    }
    /// Latest Source publication, including an already retired one. Public
    /// queue history retains this value; its ticket retains physical owners.
    pub(crate) fn latest_render_publication(&self) -> Result<Option<RenderPublication>> {
        let Some(PendingWork::Render(wave)) = self.pending.as_ref() else {
            return Ok(None); // A control announcement has not published work.
        };
        let frame = wave.frames.last().ok_or(EIO)?;
        Ok(Some(RenderPublication { ordinal: frame.ticket.item.ordinal,
            milestones: render::milestones(frame.ticket.item)?, fence: frame.gate.clone() }))
    }
    pub(crate) fn live_render_count(&self) -> usize {
        match &self.pending {
            Some(PendingWork::Render(wave)) => wave.frames.len() - wave.next,
            _ => 0,
        }
    }

    pub(crate) fn render_publication_receipt(&self) -> Result<[render::Milestone; 2]> {
        let work = self.render.as_ref().ok_or(EINVAL)?;
        render::milestones(super::g17p_render_lifecycle::Item {
            ordinal: work.ordinal, storage: work.storage, index: work.item_index, layout: work.layout,
        })
    }

    /// Capture under a short lock, then prepare outside the runtime mutex.
    /// ahead=1 is next; further ordered seeds are checked again at commit.
    pub(crate) fn render_scratch_seed(&self, p: &super::g17p_render::Parameters)
        -> Result<Option<render::ScratchSeed>> {
        if !self.independent_render_roots() || self.phase != Phase::Running { return Ok(None); }
        self.render.as_ref().map_or(Ok(None), |work| work.scratch_seed(p))
    }
    pub(crate) fn commit_render_scratch(&mut self, prepared: render::PreparedScratch) -> Result<bool> {
        self.render.as_mut().ok_or(EIO)?.commit_scratch(self.memory.as_mut().ok_or(EIO)?, prepared)
    }
    pub(crate) fn render_preparation_seed(&self, p: &super::g17p_render::Parameters,
                                         ahead: u32) -> Result<Option<KBox<render::PreparationSeed>>> {
        self.render_preparation_seed_for(p, ahead, None)
    }
    pub(crate) fn render_preparation_seed_for(&self, p: &super::g17p_render::Parameters,
        ahead: u32, incoming: Option<&compute::Client>) -> Result<Option<KBox<render::PreparationSeed>>> {
        let Some(work) = self.render.as_ref() else { return Ok(None); };
        if self.native.is_some() || self.phase != Phase::Running { return Ok(None); }
        let mut p = source_fragment_parameters(p, false)?;
        // Native independent-context overlap keeps render completion control
        // zero together with compute gate one, from the opening render.
        p.completion_control = u64::from(self.compute.is_some() && !self.independent_compute_enabled());
        let Some(pair) = self.next_render_pool_for_geometry(incoming, &p)? else { return Ok(None); };
        let Some(storage) = self.next_render_storage()? else { return Ok(None); };
        let installed = self.render_pool_clients.iter().find(|(pool, client)|
            *pool == pair && Self::same_client(client, &work.client)).map(|(_, client)| client);
        let context = if self.independent_render_roots() {
            let asid = self.render_pool_asids[pair as usize];
            Some(if asid != 0 { u32::from(asid) } else {
                (4..64).rev().find(|asid| self.independent_compute.asid_mask() & (1u64 << asid) == 0).ok_or(EBUSY)?
            })
        } else { None };
        let mut seed = work.preparation_seed(&p, ahead, pair, storage, incoming.or(installed), context)?;
        if let Some(seed) = &mut seed {
            seed.set_timestamp_cache(self.timestamp_cache_seed(&[
                p.ta_user_timestamp_start, p.ta_user_timestamp_end,
                p.fragment_user_timestamp_start, p.fragment_user_timestamp_end])?);
        }
        Ok(seed)
    }
    fn render_preparing(&self, pair: u32) -> bool {
        self.render_prepare_leases.iter().any(|(pool, active)| *pool == pair && active.load(Ordering::Acquire))
    }
    pub(crate) fn claim_render_preparation(&mut self, p: &super::g17p_render::Parameters,
        incoming: &compute::Client) -> Result<Option<KBox<render::PreparationSeed>>> {
        let Some(mut seed) = self.render_preparation_seed_for(p, 1, Some(incoming))? else { return Ok(None); };
        let pair = seed.pair();
        if !self.independent_render_roots() { return Ok(Some(seed)); }
        let spans = self.render.as_ref().ok_or(EIO)?.clearing_seed(self.memory.as_ref().ok_or(EIO)?, &seed)?;
        let active = if let Some((_, active)) = self.render_prepare_leases.iter().find(|(pool,_)| *pool == pair) {
            active.clone()
        } else {
            let active = Arc::new(AtomicBool::new(false), GFP_KERNEL)?;
            self.render_prepare_leases.push((pair, active.clone()), GFP_KERNEL)?;
            active
        };
        if active.swap(true, Ordering::AcqRel) { return Err(EBUSY); }
        seed.set_clearing(spans, render::PreparationReservation(active));
        Ok(Some(seed))
    }
    pub(crate) fn render_prepared_matches(&self, plan: &render::PreparedAppend,
        incoming: &compute::Client, p: &super::g17p_render::Parameters) -> Result<bool> {
        Ok(self.render_preparation_seed_for(p, 1, Some(incoming))?
            .is_some_and(|seed| plan.matches_seed(&seed)))
    }
    /// Registered private pool owners are the current live-resource bound.
    /// Descriptor/status/context/ring conflicts remain separate checks.
    pub(crate) fn render_ticket_capacity(&self) -> usize {
        self.render.as_ref().and_then(|work| work.growth.as_ref())
            .map_or(1, |service| service.pools.len())
    }

    /// An idle ordinary post-render compute owner may coexist with render
    /// tickets in the separately admitted ASID-one caller root. The retained
    /// ASID-two/three compute client need not have the same logical owner.
    /// Active, cold-start and native compute keep their serial lifecycle.
    /// No queue/root mutation here; render joins still require an exact caller.
    fn render_compute_owner_compatible(&self) -> Result<bool> {
        let Some(compute) = self.compute.as_ref() else { return Ok(true); };
        let Some(render) = self.render.as_ref() else { return Ok(false); };
        if (!compute.after_render && !(self.independent_compute_enabled() && self.bootstrapped)) || compute.native_context.is_some()
            || self.compute_contexts.is_some() || self.render_contexts.is_some()
            || compute.retained_execution_gate != 0 || !render.layout.independent
            || render.layout.native {
            return Ok(false);
        }
        Ok(true)
    }
    fn render_compute_owner_idle(&self) -> Result<bool> {
        if !self.render_compute_owner_compatible()? { return Ok(false); }
        let Some(compute) = self.compute.as_ref() else { return Ok(true); };
        match compute::idle(self.memory.as_ref().ok_or(EIO)?, self.vm.as_ref().ok_or(EIO)?, compute) {
            Ok(_) => Ok(true), Err(error) if error == EBUSY => Ok(false), Err(error) => Err(error),
        }
    }

    /// Nonmutating admission: free private owners or a compatible queued
    /// owner with unused finite scratch/transport slots. Special profiles keep
    /// their qualified serial lifecycle; unrelated roots remain installed.
    pub(crate) fn can_append_render(&self, p: &super::g17p_render::Parameters) -> Result<bool> {
        self.can_append_render_for(None,p)
    }
    fn can_append_render_for(&self, incoming: Option<&compute::Client>, p: &super::g17p_render::Parameters) -> Result<bool> {
        if self.phase != Phase::Running || self.render_control.is_some() { return Ok(false); }
        if matches!(self.pending, Some(PendingWork::Control(_) | PendingWork::Native)) {
            return Ok(false);
        }
        let Some(work) = self.render.as_ref() else { return Ok(false); };
        if self.live_render_count() == 0 {
            return Ok(self.compute_pending.is_none() || self.render_compute_owner_compatible()?);
        }
        if self.native.is_some() || !self.render_compute_owner_compatible()? || work.layout.native
            || *crate::module_parameters::partial_independent_owner.value() != 1
            || *crate::module_parameters::alternate_queue_pairs.value() != 1
            || (!self.independent_render_roots() && !work.same_geometry(p)) { return Ok(false); }
        Ok(self.next_render_pool_for_geometry(incoming, p)?.is_some() && self.next_render_storage()?.is_some())
    }
    fn next_render_pool(&self) -> Result<Option<u32>> {
        self.next_render_pool_for(None)
    }
    fn next_render_pool_for(&self, incoming: Option<&compute::Client>) -> Result<Option<u32>> {
        let work = self.render.as_ref().ok_or(EINVAL)?;
        let next = work.ordinal.checked_add(1).ok_or(EOVERFLOW)?;
        let count = if work.layout.independent && !work.layout.native
            && *crate::module_parameters::alternate_queue_pairs.value() == 1 {
            work.pool_count()
        } else { return Ok(Some(work.layout.pair)); };
        let frames = match &self.pending {
            Some(PendingWork::Render(wave)) => &wave.frames[wave.next..],
            _ => &[],
        };
        let available = |pair: &u32| !self.render_preparing(*pair) && frames.iter().all(|frame| frame.ticket.item.layout.pair != *pair)
            && !self.render_cpu_leases.iter().any(|(pool, fence)| pool == pair
                && unsafe { kernel::bindings::dma_fence_get_status(fence.raw()) } == 0)
            && (!self.independent_render_roots() || *pair == 0
                || self.render_pool_asids[*pair as usize] != 0
                || self.independent_compute.asid_mask() != u64::MAX);
        if self.independent_render_roots() {
            let desired = incoming.unwrap_or(&work.client);
            // Reuse an idle exact snapshot before rebuilding caller mappings
            // in another pool. A live matching pool never blocks a free pool.
            if let Some(pair) = (0..count).map(|offset| (next + offset) % count).find(|pair| {
                available(pair) && if *pair == work.layout.pair {
                    Self::same_client(desired, &work.client)
                } else {
                    self.render_pool_clients.iter().any(|(pool, client)|
                        pool == pair && Self::same_client(desired, client))
                }
            }) { return Ok(Some(pair)); }
        }
        Ok((0..count).map(|offset| (next + offset) % count).find(available))
    }

    fn next_render_pool_for_geometry(&self, incoming: Option<&compute::Client>,
        p: &super::g17p_render::Parameters) -> Result<Option<u32>> {
        let idle = self.next_render_pool_for(incoming)?;
        if !self.independent_render_roots() { return Ok(idle); }
        let work = self.render.as_ref().ok_or(EIO)?;
        let frames = match &self.pending {
            Some(PendingWork::Render(wave)) => &wave.frames[wave.next..], _ => &[],
        };
        let desired = incoming.unwrap_or(&work.client);
        let memory = self.memory.as_ref().ok_or(EIO)?;
        let vm = self.vm.as_ref().ok_or(EIO)?;
        let mut best = None;
        let mut least = usize::MAX;
        let mut owned = false;
        for pair in 0..work.pool_count() {
            if self.render_preparing(pair) { continue; }
            let own = if pair == work.layout.pair { Some(&work.client) } else {
                self.render_pool_clients.iter().find(|(pool, _)| *pool == pair).map(|(_, client)| client)
            };
            if !own.is_some_and(|own| Self::same_client(desired, own)) || !work.pair_same_geometry(pair, p) { continue; }
            let mut count = 0;
            let mut transport = None;
            let next = work.pair_next_index(pair)?;
            let mut slot_free = true;
            for frame in frames.iter().filter(|frame| frame.ticket.item.layout.pair == pair) {
                count += 1;
                let index = frame.ticket.item.index;
                // Count alone does not establish ownership if a slow ticket
                // survives later retirements. Lease every reused pool-local
                // tilemap, scheduler record, status and context slot exactly.
                slot_free &= next % 8 != index % 8
                    && (2 * next) % 35 != (2 * index) % 35
                    && next % 79 != index % 79
                    // Pool-B's 79 records alias 36 physical cycle blocks.
                    // Record wrap can reuse a block while its earlier record
                    // remains live (e.g. records 72 and 0 both use phase 0).
                    && (next % 79) % 36 != (index % 79) % 36
                    && next % super::g17p_render_lifecycle::STORAGE_SUBMISSIONS
                        != index % super::g17p_render_lifecycle::STORAGE_SUBMISSIONS;
                transport = Some(frame.ticket.item.layout);
            }
            // Eight tilemap blocks are real private ring storage. They cannot
            // alias a live generation. Other matching pools stay admissible.
            let preferred = p.queue_owner.is_some() && work.pair_queue_owner(pair) == p.queue_owner;
            if count == 0 || !slot_free || (owned && !preferred)
                || (preferred == owned && count >= least) { continue; }
            let layout = transport.ok_or(EIO)?;
            let mut room = true;
            if let Some(priority) = p.firmware_priority {
                for queue in layout.queues {
                    room &= memory.read_firmware32(vm.physical(memory,2,queue+0x28)?)? == priority;
                }
            }
            for ptr in layout.pointers {
                let capacity = memory.read_firmware32(vm.physical(memory,2,ptr+0x60)?)?.min(0x2870/8);
                let tail = memory.read_firmware32(vm.physical(memory,2,ptr+0x40)?)?;
                room &= tail.checked_add(3).ok_or(EOVERFLOW)? <= capacity;
            }
            if room { best=Some(pair); least=count; owned=preferred; }
        }
        // Keep a public queue's backlog in its installed firmware owner.
        // A different public queue prefers its own/free pool for fairness.
        if owned { Ok(best) } else { Ok(idle.or(best)) }
    }

    fn next_render_storage(&self) -> Result<Option<u32>> {
        use super::g17p_render_lifecycle::{storage_conflicts, STORAGE_SUBMISSIONS};
        let work = self.render.as_ref().ok_or(EINVAL)?;
        let next = work.ordinal.checked_add(1).ok_or(EOVERFLOW)? % STORAGE_SUBMISSIONS;
        let frames = match &self.pending {
            Some(PendingWork::Render(wave)) => &wave.frames[wave.next..],
            _ => &[],
        };
        // Lease a free physical slot independently of the logical ordinal.
        // A slow owner cannot stall other pools when another slot is free.
        Ok((0..STORAGE_SUBMISSIONS).map(|offset| (next + offset) % STORAGE_SUBMISSIONS)
            .find(|slot| frames.iter().all(|frame|
                !storage_conflicts(*slot, frame.ticket.item.storage_ordinal()))))
    }

    pub(crate) fn submit_next_render(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        replacement: Option<compute::Client>,
        compute_replacement: Option<compute::Client>,
        p: &super::g17p_render::Parameters,
    ) -> Result {
        self.submit_prepared_render(dev, image, replacement, compute_replacement, p, None)
    }
    pub(crate) fn submit_prepared_render(
        &mut self, dev: &kernel::device::Device, image: &Image,
        replacement: Option<compute::Client>, compute_replacement: Option<compute::Client>,
        p: &super::g17p_render::Parameters, prepared: Option<KBox<render::PreparedAppend>>,
    ) -> Result {
        self.submit_prepared_render_ticket(dev, image, replacement, compute_replacement, p, prepared).map(|_| ())
    }
    pub(crate) fn submit_prepared_render_ticket(
        &mut self, dev: &kernel::device::Device, image: &Image,
        replacement: Option<compute::Client>, compute_replacement: Option<compute::Client>,
        p: &super::g17p_render::Parameters, prepared: Option<KBox<render::PreparedAppend>>,
    ) -> Result<Arc<RenderReceipt>> {
        let profiling = *crate::module_parameters::submission_log.value() >= 3;
        let prep_started = profiling.then(kernel::time::Instant::<kernel::time::Monotonic>::now);
        let mut configured = source_fragment_parameters(p, false)?;
        configured.completion_control = u64::from(self.compute.is_some() && !self.independent_compute_enabled());
        let p = &configured;
        if self.phase != Phase::Running { return Err(EIO); }
        if !self.can_append_render_for(replacement.as_ref(), p)? { return Err(EBUSY); }
        if self.live_render_count() != 0 {
            // Changing caller PTE ownership while a ticket executes requires
            // a distinct root lease, not the retained-root rebind path.
            if (!self.independent_render_roots() && replacement.is_some()) || compute_replacement.is_some() { return Err(EBUSY); }
            // Existing owners route their records; unread primary reports
            // are not proof that an independent private pool is unavailable.
            self.service_owned_render_reports(dev, image)?;
        }
        self.restore_owned_render_fault()?;
        let private_pair = if self.independent_render_roots() {
            Some(self.next_render_pool_for_geometry(replacement.as_ref(), p)?.ok_or(EBUSY)?)
        } else { None };
        if *crate::module_parameters::native_render_vms.value() == 1 {
            self.prepare_native_render_context(dev, replacement, p)?;
        } else if self.native.is_some() {
            self.refresh_native_render(replacement, compute_replacement, p)?;
        } else if self.independent_render_roots() {
            let pair = private_pair.ok_or(EIO)?;
            self.prepare_pool_render_context(dev, pair, replacement, p).inspect_err(|e|
                dev_err!(dev,"G17P: render fail site prepare_pool pair {} error {:?}\n",pair,e))?;
        } else if let Some(client) = replacement {
            self.prepare_logical_render_context(dev, client, p)?;
            dev_info!(dev, "G17P: retained render caller mappings refreshed\n");
        }
        let root_us = prep_started.as_ref().map(|start| start.elapsed().as_nanos() / 1000).unwrap_or(0);
        let ordinal = self
            .render
            .as_ref()
            .ok_or(EINVAL)?
            .ordinal
            .checked_add(1)
            .ok_or(EOVERFLOW)?;
        if self.native.is_none()
            && *crate::module_parameters::native_render_vms.value() == 0
            && *crate::module_parameters::partial_independent_owner.value() == 1
        {
            let pair = match private_pair {
                Some(pair) => pair,
                None => self.next_render_pool()?.ok_or(EBUSY)?,
            };
            let storage = self.next_render_storage()?.ok_or(EBUSY)?;
            let work = self.render.as_mut().ok_or(EINVAL)?;
            work.select_pair(pair)?;
            // No later render may publish while this control owner is pending.
            work.next_storage = Some(storage);
        }
        let work = self.render.as_ref().ok_or(EINVAL)?;
        if work.layout.native
            && work.item_index + 1 >= super::g17p_render_lifecycle::NATIVE_SUBMISSIONS
        {
            return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
        }
        let receipt = RenderReceipt::new(ordinal)?;
        let result = (|| {
            if let Some(priority) = p.firmware_priority {
                self.render.as_ref().ok_or(EINVAL)?.set_priority(
                    self.memory.as_mut().ok_or(EINVAL)?,
                    self.vm.as_ref().ok_or(EINVAL)?,
                    priority,
                ).inspect_err(|e| dev_err!(dev,"G17P: render fail site set_priority {:?}\n",e))?;
            }
            // Allocate the control owner before its producer can be visible.
            let mut control = KBox::new(RenderControl {
                ordinal, announced: false, before: 0, target: 0, parameters: *p, prepared, receipt: receipt.clone(),
                started: kernel::time::Instant::now(),
            }, GFP_KERNEL)?;
            if self.native.is_none() && self.independent_render_roots() {
                if self.render_control_credit(image, ordinal)? {
                    return self.stage_announced_render(dev, image, ordinal, p,
                        control.prepared.take(), receipt.clone());
                }
                if matches!(self.pending, Some(PendingWork::Render(_))) {
                    self.render_control = Some(control);
                } else {
                    self.pending = Some(PendingWork::Control(control));
                }
                return Ok(());
            }
            match self.announce_render(dev, image, ordinal) {
                Ok(None) => self.stage_announced_render(dev, image, ordinal, p, control.prepared.take(), receipt.clone()),
                announcement => {
                    match announcement {
                        Ok(Some((before, target))) => {
                            control.before = before; control.target = target; control.announced = true;
                        },
                        Err(e) if e == EBUSY => (), // Accepted host owner; no producer yet.
                        Err(e) => return Err(e),
                        Ok(None) => unreachable!(),
                    }
                    if matches!(self.pending, Some(PendingWork::Render(_))) {
                        self.render_control = Some(control);
                    } else {
                        self.pending = Some(PendingWork::Control(control));
                    }
                    Ok(())
                },
            }
        })();
        if let Err(error) = &result {
            self.phase = Phase::Failed;
            receipt.fail(*error);
            self.fail_render_gate(*error);
        }
        if profiling && ordinal % 16 == 0 {
            dev_info!(dev, "G17P: render prep timing ordinal {} root_us {} total_us {}\n",
                ordinal, root_us, prep_started.ok_or(EIO)?.elapsed().as_nanos() / 1000);
        }
        result.map(|()| receipt)
    }

    fn stage_announced_render(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        ordinal: u32,
        p: &super::g17p_render::Parameters,
        prepared: Option<KBox<render::PreparedAppend>>,
        receipt: Arc<RenderReceipt>,
    ) -> Result {
        if let Some(native) = self.native.as_mut() {
            if !native.complete() {
                return Err(EBUSY);
            }
            native.service.bind_work(
                render::DESCRIPTORS[1] + ordinal as u64 * super::g17p_render::FRAGMENT_SIZE as u64,
            )?;
        }
        let profile_started = (*crate::module_parameters::submission_log.value() >= 3)
            .then(kernel::time::Instant::<kernel::time::Monotonic>::now);
        let work = self.render.as_mut().ok_or(EINVAL)?;
        render::stage_next_prepared(
            self.memory.as_mut().ok_or(EINVAL)?,
            self.vm.as_mut().ok_or(EINVAL)?,
            work,
            p,
            prepared,
        )
        .inspect_err(|e| dev_err!(dev,"G17P: render fail site stage_announced ordinal {} error {:?}\n",ordinal,e))
        .and_then(|()| {
            if ordinal % 16 == 0 {
                if let Some(start) = profile_started {
                    dev_info!(dev,"G17P: render stage timing ordinal {} stage_us {}\n",
                        ordinal,start.elapsed().as_nanos()/1000);
                }
            }
            // Finish retired transport and descriptor edits before waking
            // firmware. A tick can revisit the queue while handling control;
            // publishing it before a bank switch races its cached queue header.
            if self.native.is_none() && self.independent_render_roots() {
                self.announce_render(dev, image, ordinal)?;
            }
            self.run_render(dev, image, p, receipt).inspect_err(|e|
                dev_err!(dev,"G17P: render fail site publish_announced ordinal {} error {:?}\n",ordinal,e))
        })
    }

    fn render_control_credit(&self, image: &Image, ordinal: u32) -> Result<bool> {
        if ordinal < 2 { return Ok(true); }
        let memory = self.memory.as_ref().ok_or(EIO)?;
        let vm = self.vm.as_ref().ok_or(EIO)?;
        let mut values = [0; 3];
        for (value, at) in values.iter_mut().zip(image.graph.channels[0][12].states) {
            *value = memory.read_firmware32(vm.physical(memory, 2, at)?)?;
        }
        Ok(queue::Counters::new(values).map_err(|_| EIO)?.available() != 0)
    }

    fn announce_render(
        &mut self,
        _dev: &kernel::device::Device,
        image: &Image,
        ordinal: u32,
    ) -> Result<Option<(u8, u8)>> {
        use super::g17p_render_lifecycle as life;
        if ordinal < 2 {
            return Ok(None);
        }
        let body = life::control_tick(ordinal).map_err(|_| EINVAL)?;
        let channel = image.graph.channels[0][12];
        let pipelined = self.native.is_none() && self.independent_render_roots();
        // The support prestate is shared with compute. An idle render set
        // alone does not permit resetting these firmware-owned fields.
        let no_live_work = self.live_render_count() == 0
            && (!pipelined || (!self.independent_compute.pending() && self.compute_pending.is_none()));
        let memory = self.memory.as_mut().ok_or(EINVAL)?;
        let vm = self.vm.as_ref().ok_or(EINVAL)?;
        let mut before = [0; 3];
        for (value, at) in before.iter_mut().zip(channel.states) {
            *value = memory.read_firmware32(vm.physical(memory, 2, at)?)?;
        }
        let credits = queue::Counters::new(before).map_err(|_| EIO)?;
        if (pipelined && credits.available() == 0)
            || (!pipelined && (before[0] != before[2] || before[1] != before[2])) {
            return Err(EBUSY);
        }
        if ordinal >= 3 && no_live_work {
            vm.write(
                memory,
                2,
                self.render.as_ref().ok_or(EINVAL)?.layout.support + 0x20,
                &life::control_prestate(),
            )?;
        }
        vm.write(memory, 2, channel.ring + before[2] as u64 * 0x40, &body)?;
        g17p_memory::sync();
        let target = (before[2] + 1) & 0xff;
        if let Some(native) = self.native.as_mut() {
            native.set_control(super::g17p_dependency_release::Control {
                producer: channel.states[2],
                target: target.try_into()?,
                consumers_before: [before[0] as u8, before[1] as u8],
            })?;
        }
        vm.write(memory, 2, channel.states[2], &target.to_le_bytes())?;
        g17p_memory::sync();
        self.peers[0]
            .rtkit
            .as_mut()
            .ok_or(EINVAL)?
            .as_mut()
            .send_message(0x21, 0x0084000000000011)?;
        // Keep the control-before-work producer order, while allowing the
        // firmware to consume both rings without a host acknowledgement turn.
        // Both control consumers still bound finite slot reuse above.
        if pipelined { Ok(None) } else { Ok(Some((before[0] as u8, target as u8))) }
    }

    fn poll_control(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        state: &mut RenderControl,
    ) -> Result<bool> {
        if self
            .peers
            .iter()
            .any(|p| p.data.crashed.load(Ordering::Acquire))
        {
            return Err(EIO);
        }
        if !state.announced {
            if self.native.is_none() && self.independent_render_roots() {
                if !self.render_control_credit(image, state.ordinal)? { return Ok(false); }
                self.stage_announced_render(dev, image, state.ordinal, &state.parameters,
                    state.prepared.take(), state.receipt.clone())?;
                return Ok(false);
            }
            match self.announce_render(dev, image, state.ordinal) {
                Ok(Some((before,target))) => {
                    state.before = before; state.target = target; state.announced = true;
                    state.started = kernel::time::Instant::now();
                },
                Err(e) if e == EBUSY => return Ok(false),
                Err(e) => return Err(e),
                Ok(None) if self.independent_render_roots() => {
                    self.stage_announced_render(dev,image,state.ordinal,&state.parameters,
                        state.prepared.take(),state.receipt.clone())?;
                    return Ok(false);
                },
                Ok(None) => return Err(EIO),
            }
        }
        let channel = image.graph.channels[0][12];
        let memory = self.memory.as_ref().ok_or(EINVAL)?;
        let vm = self.vm.as_ref().ok_or(EINVAL)?;
        let consumer = memory.read_firmware32(vm.physical(memory, 2, channel.states[0])?)?;
        if consumer <= 255 && queue::reached(state.before, consumer as u8, state.target) {
            if *crate::module_parameters::submission_log.value() != 0 {
                dev_info!(
                    dev,
                    "G17P: render {} asynchronous control consumed\n",
                    state.ordinal
                );
            }
            self.stage_announced_render(dev, image, state.ordinal, &state.parameters, state.prepared.take(), state.receipt.clone())?;
            return Ok(false);
        }
        if state.started.elapsed().as_millis() >= 100 {
            return Err(ETIMEDOUT);
        }
        self.peers[0]
            .rtkit
            .as_mut()
            .ok_or(EINVAL)?
            .as_mut()
            .send_message(0x21, 0x0084000000000011)?;
        Ok(false)
    }

    pub(crate) fn submit_render(
        &mut self, dev: &kernel::device::Device, image: &Image,
        client: compute::Client, parameters: &super::g17p_render::Parameters,
    ) -> Result {
        self.submit_render_ticket(dev, image, client, parameters).map(|_| ())
    }
    pub(crate) fn submit_render_ticket(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        client: compute::Client,
        parameters: &super::g17p_render::Parameters,
    ) -> Result<Arc<RenderReceipt>> {
        let configured = source_fragment_parameters(parameters, true)?;
        let parameters = &configured;
        let work = if let Some(compute) = self.compute.as_ref() {
            if self.phase != Phase::Running {
                return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
            }
            compute::idle(
                self.memory.as_ref().ok_or(EINVAL)?,
                self.vm.as_ref().ok_or(EINVAL)?,
                compute,
            )?;
            // Keep ownership in Session throughout live adoption, including
            // errors after the retained root has acquired new caller mappings.
            let work = self.dormant_render.as_mut().ok_or(EIO)?;
            let result = work.adopt(
                self.memory.as_mut().ok_or(EINVAL)?,
                self.vm.as_ref().ok_or(EINVAL)?,
                client,
                parameters,
            );
            if let Err(error) = result {
                self.phase = Phase::Failed;
                return Err(error);
            }
            dev_info!(dev, "G17P: adopted dormant render owner after compute\n");
            self.dormant_render.take().ok_or(EIO)?
        } else {
            self.require_first_work()?;
            let ttbs = self.ttbs;
            self.stage(|memory, vm| render::build(memory, vm, image, ttbs, client, parameters))
                .inspect_err(|error| {
                    dev_err!(
                        dev,
                        "G17P: first render graph preparation failed: {:?}\n",
                        error
                    );
                })?
        };
        self.render = Some(work);
        let receipt = RenderReceipt::new(self.render.as_ref().ok_or(EIO)?.ordinal)?;
        let result = self.run_render(dev, image, parameters, receipt.clone());
        if let Err(error) = &result {
            self.phase = Phase::Failed;
            receipt.fail(*error);
            self.fail_render_gate(*error);
        }
        result.map(|()| receipt)
    }

    fn run_render(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        parameters: &super::g17p_render::Parameters,
        receipt: Arc<RenderReceipt>,
    ) -> Result {
        // Host/fence allocations precede every producer store. Failure keeps
        // an existing wave and all of its ticket ownership untouched.
        let gate = receipt.fence.clone();
        let mut first_frames = KVec::new();
        match self.pending.as_mut() {
            Some(PendingWork::Render(wave)) => wave.frames.reserve(1, GFP_KERNEL)?,
            None => first_frames.reserve(1, GFP_KERNEL)?,
            Some(_) => return Err(EBUSY),
        }
        let stamps = self.render.as_ref().ok_or(EINVAL)?.timestamps;
        if !self.render.as_ref().ok_or(EINVAL)?.caller_cache_prepared {
          for pair in stamps.chunks_exact(2) {
            self.timestamps
                .as_ref()
                .ok_or(EINVAL)?
                .cache([pair[0], pair[1]], false)?;
          }
        }
        if self.phase == Phase::Prepared {
            self.start(dev, image)?;
        } else if self.phase != Phase::Running {
            return Err(EIO);
        }
        let joining_live_wave = self.live_render_count() != 0 || self.compute_pending.is_some()
            || self.independent_compute.any();
        if joining_live_wave {
            // Another pool can retire while descriptors/transport are built.
            // Route its terminal/growth before capturing the new baseline.
            self.service_owned_render_reports(dev, image).inspect_err(|e|
                dev_err!(dev,"G17P: render fail site startup_route {:?}\n",e))?;
        }
        let mut startup = self.report_snapshot(image).inspect_err(|e|
            dev_err!(dev,"G17P: render fail site startup_snapshot {:?}\n",e))?;
        if !joining_live_wave && startup.iter().any(|report| !report.records.is_empty()) {
            dev_err!(dev,"G17P: render fail site serial_startup_unread: ordinal {} peer0 {}/{} records {} peer1 {}/{} records {}\n",
                self.render.as_ref().ok_or(EIO)?.ordinal, startup[0].host,startup[0].firmware,startup[0].records.len(),startup[1].host,startup[1].firmware,startup[1].records.len());
            return Err(EIO);
        }
        if joining_live_wave {
            // A primary record may race the bounded drain/snapshot. Leave its
            // credit at the service cursor; the next poll must classify it.
            // Never acknowledge a terminal/error merely because it was copied.
            startup[0].peer_credits[0] = self.render.as_ref().ok_or(EIO)?
                .growth.as_ref().ok_or(EIO)?.cursor();
            for body in &startup[1].records {
                let opcode=u32::from_le_bytes(body[..4].try_into().unwrap());
                if opcode != 1 {
                    dev_err!(dev,"G17P: render fail site concurrent_startup_peer1 opcode {} DATA {:02x?}\n",opcode,body);
                    return Err(EIO);
                }
            }
        }
        // submit_drm returns both instances' split report/telemetry credits
        // before exposing another group, including records arriving after
        // the previous completion snapshot.
        self.acknowledge_reports(image, &startup)?;
        let terminal_baseline;
        let control_done = false;
        let ticket;
        {
            let work = self.render.as_mut().ok_or(EINVAL)?;
            let vm = self.vm.as_ref().ok_or(EINVAL)?;
            let memory = self.memory.as_mut().ok_or(EINVAL)?;
            if work.ordinal == 0 {
                work.after_control(memory, vm, self.ttbs)?;
                if work.growth.is_none() {
                    work.growth = Some(super::g17p_growth_runtime::Service::new(
                        memory,
                        vm,
                        self.ttbs,
                        &work.client.root,
                        image.graph.channels[0][12],
                        image.graph.channels[0][13],
                    )?);
                    configure_growth_limits(work.growth.as_mut().ok_or(EINVAL)?)?;
                }
            }
        }
        if self.render.as_ref().ok_or(EINVAL)?.ordinal == 0 {
            self.prepare_first_native_render_context()?;
        }
        // A compute-first dormant render has no independent pools yet.
        // Its legacy compatibility predicate requires those pools, so it
        // cannot gate their creation in the independently bootstrapped profile.
        if self.native.is_none()
            && ((self.independent_render_roots() && self.bootstrapped)
                || self.render_compute_owner_compatible()?)
            && *crate::module_parameters::partial_independent_owner.value() == 1
            && *crate::module_parameters::alternate_queue_pairs.value() == 1
        {
            self.render.as_mut().ok_or(EINVAL)?.create_render_pools(
                self.memory.as_mut().ok_or(EINVAL)?, self.vm.as_mut().ok_or(EINVAL)?)?;
        }
        self.inject_owned_render_fault(parameters)?;
        if self.native.is_none() && *crate::module_parameters::native_limit_reply.value() == 1 {
            let work = self.render.as_mut().ok_or(EIO)?;
            if work.ordinal == 0 {
                let memory = self.memory.as_ref().ok_or(EIO)?;
                let vm = self.vm.as_ref().ok_or(EIO)?;
                let fragment = super::g17p_render_lifecycle::DESCRIPTORS[1];
                let stamp = memory.read_firmware32(vm.physical(memory, 2, fragment + 0x470)?)?;
                if stamp != memory.read_firmware32(vm.physical(memory, 2, fragment + 0x47c)?)? { return Err(EIO); }
                work.growth.as_mut().ok_or(EIO)?.bind_limit_reply(fragment, work.layout.queues[1], stamp)?;
            }
        }
        ticket = render::Ticket::capture(self.render.as_ref().ok_or(EINVAL)?, self.native.is_none())?;
        for bo in ticket.client.buffers() {
            self.retain_source_backing(bo)?;
        }
        {
            let work = self.render.as_mut().ok_or(EINVAL)?;
            let vm = self.vm.as_ref().ok_or(EINVAL)?;
            let memory = self.memory.as_mut().ok_or(EINVAL)?;
            // wait_pair_completed() requires this publication's new growth
            // terminal in addition to both queues and independent statuses.
            // Capture before either work producer is restored; a drained ring
            // alone cannot establish that firmware has issued this terminal.
            terminal_baseline = if let Some(native) = self.native.as_ref() {
                if !native.complete() {
                    return Err(EBUSY);
                }
                native.service.terminals()
            } else {
                work.growth.as_ref().ok_or(EINVAL)?.terminals()
            };
            work.restore(memory, vm, 1)?;
            work.restore(memory, vm, 0)?;
        }
        self.peers[0]
            .rtkit
            .as_mut()
            .ok_or(EINVAL)?
            .as_mut()
            .send_message(0x21, 0x0083000000000008)?;
        let client_root = self.render.as_ref().ok_or(EINVAL)?.client.root.root();
        if *crate::module_parameters::submission_log.value() != 0 {
            dev_info!(
                dev,
                "G17P: caller render published on TA2/3D2, client root {:#x}\n",
                client_root
            );
        }
        // Source Item values were validated before publication during staging.
        // Native receipts stay conservative and do not expose ordinary points.
        if !ticket.item.layout.native {
            for (stored, grid) in receipt.milestones.iter().zip(ticket.item.layout.grids) {
                stored.store(((ticket.item.layout.pair as u64) << 40)
                    | ((grid as u64) << 32) | u64::from(ticket.item.index + 1), Ordering::Relaxed);
            }
        }
        if *crate::module_parameters::submission_log.value() != 0 {
            let live = match &self.pending { Some(PendingWork::Render(wave)) =>
                wave.frames[wave.next..].iter().filter(|f| f.ticket.item.layout.pair == ticket.item.layout.pair).count(), _ => 0 };
            let mut unfinished = 0;
            if let Some(PendingWork::Render(wave)) = &self.pending {
                let memory=self.memory.as_ref().ok_or(EIO)?;
                let vm=self.vm.as_ref().ok_or(EIO)?;
                for frame in wave.frames[wave.next..].iter().filter(|f| f.ticket.item.layout.pair == ticket.item.layout.pair) {
                    let pa=vm.physical(memory,2,frame.ticket.firmware_completion)?;
                    memory.invalidate(pa,8)?;
                    unfinished += usize::from(memory.read64(pa)? == 0);
                }
            }
            dev_info!(dev, "G17P: render owner publication ordinal {} pool {} local {} prior_live {} unfinished {}\n",
                ticket.item.ordinal, ticket.item.layout.pair, ticket.item.index, live, unfinished);
        }
        let frame = RenderPending { receipt: receipt.clone(), ticket, gate, startup,
            terminal_baseline, control_done, started: kernel::time::Instant::now() };
        if let Some(PendingWork::Render(wave)) = self.pending.as_mut() {
            wave.frames.push(frame, GFP_KERNEL)?; // Capacity reserved prepublication.
        } else {
            first_frames.push(frame, GFP_KERNEL)?;
            self.pending = Some(PendingWork::Render(RenderWave { frames: first_frames, next: 0 }));
        }
        receipt.publication_ready.store(true, Ordering::Release);
        Ok(())
    }

    /// Consume primary records through their exact retained owners. This is
    /// also legal before another outer producer: it does not poll the newly
    /// staged ticket or quiesce the selected unpublished inner queue.
    fn service_owned_render_reports(&mut self, dev: &kernel::device::Device,
                                    _image: &Image) -> Result {
        // A bounded prefix handles coalesced growth notifications under the
        // runtime lock. Growth selects its retained pool's exact root.
        for _ in 0..32 {
            use super::g17p_growth_runtime::Action;
            let work = self.render.as_mut().or(self.dormant_render.as_mut()).ok_or(EINVAL)?;
            let action = if let Some(native) = self.native.as_mut() {
                native.service.step_dependency_render(
                    self.memory.as_mut().ok_or(EINVAL)?,
                    self.vm.as_ref().ok_or(EINVAL)?,
                    &mut self.compute.as_mut().ok_or(EINVAL)?.client.root,
                    self.ttbs,
                )?
            } else {
                let memory = self.memory.as_mut().ok_or(EINVAL)?;
                let vm = self.vm.as_ref().ok_or(EINVAL)?;
                let service = work.growth.as_mut().ok_or(EINVAL)?;
                let Some(requested) = service.requested_root(memory, vm)? else { break; };
                let root = if requested.is_none_or(|root| root == work.client.root.root()) {
                    &mut work.client.root
                } else {
                    &mut self.render_pool_clients.iter_mut().find(|(_, client)|
                        Some(client.root.root()) == requested).ok_or(EIO)?.1.root
                };
                service.step_ordinary(memory, vm, root, self.ttbs)?
            };
            match action {
                Action::Idle => break,
                Action::Consumed => (),
                Action::LimitReply { queue, stamp } => {
                    self.peers[0].rtkit.as_mut().ok_or(EINVAL)?.as_mut()
                        .send_message(0x21, 0x0084000000000011)?;
                    dev_info!(dev, "G17P: native render error reply queue {:#x} stamp {:#x}\n", queue, stamp);
                }
                Action::Limit => {
                    dev_info!(
                        dev,
                        "G17P: owned render memory limit consumed; awaiting own completion\n"
                    );
                }
                Action::Reply {
                    pool,
                    vm,
                    counter,
                    old,
                    new,
                    refused,
                } => {
                    self.peers[0]
                        .rtkit
                        .as_mut()
                        .ok_or(EINVAL)?
                        .as_mut()
                        .send_message(0x21, 0x0084000000000011)?;
                    dev_info!(
                        dev,
                        "G17P: TVB growth reply {} pool {} VM {}: {} -> {} blocks, refused={}\n",
                        counter,
                        pool,
                        vm,
                        old,
                        new,
                        refused
                    );
                }
            }
        }
        Ok(())
    }

    /// Failure-only Source DATA evidence. Never acknowledges reports, changes
    /// roots, or invalidates a GPU program allocation.
    fn diagnose_render_failure(&self, dev: &kernel::device::Device, image: &Image,
                               pending: &RenderPending, error: Error) {
        let ticket = &pending.ticket;
        dev_err!(dev, "G17P: render retirement failed {:?}: ordinal {} pair {} item {} control_done {} terminal_before {} completion {:#x} published {}\n",
            error, ticket.item.ordinal, ticket.item.layout.pair, ticket.item.index,
            pending.control_done, pending.terminal_baseline, ticket.firmware_completion,
            pending.receipt.is_published());
        let (Some(memory), Some(vm)) = (&self.memory, &self.vm) else { return; };
        let read32 = |va| -> Result<u32> { memory.read_firmware32(vm.physical(memory, 2, va)?) };
        for stage in 0..2 {
            let mut queues = [0u32; 6];
            let mut valid = true;
            for (i, offset) in [queue::POINTER_DONE, queue::POINTER_READ, queue::POINTER_WRITE].into_iter().enumerate() {
                match read32(ticket.item.layout.pointers[stage] + offset) { Ok(v) => queues[i]=v, Err(_) => valid=false }
            }
            for (i, va) in ticket.channels[stage].states.iter().enumerate() {
                match read32(*va) { Ok(v) => queues[i+3]=v, Err(_) => valid=false }
            }
            let status = (|| -> Result<[u64;8]> {
                let pa = vm.physical(memory, 2, ticket.statuses[stage])?;
                memory.invalidate(pa, 0x40)?;
                let mut words = [0;8];
                for (i,v) in words.iter_mut().enumerate() { *v=memory.read64(pa+i as u64*8)?; }
                Ok(words)
            })();
            dev_err!(dev, "G17P: render failure stage {} queues_valid {} queues {:?} status {:?}\n",stage,valid,queues,status);
        }
        let completion = (|| -> Result<u64> {
            let pa=vm.physical(memory,2,ticket.firmware_completion)?;
            memory.invalidate(pa,8)?; memory.read64(pa)
        })();
        dev_err!(dev, "G17P: render failure completion DATA {:?}\n",completion);
        if let Some(service)=self.render.as_ref().and_then(|work|work.growth.as_ref()) {
            if let Some(token)=ticket.growth {
                dev_err!(dev, "G17P: render failure cursor {} token terminals {:?} limited {:?} service_failed {:?}\n", service.cursor(),service.token_terminals(token),service.token_limited(token),service.failed);
            }
            for pool in &service.pools {
                dev_err!(dev,"G17P: render failure pool {} root {:#x} generation {} counter {} retired {} terminals {}\n",pool.identity.pool,pool.root,pool.generation,pool.counter,pool.retired,pool.terminals);
            }
        }
        match self.report_snapshot(image) {
            Ok(reports) => for (peer, report) in reports.iter().enumerate() {
                dev_err!(dev,"G17P: render failure peer {} report head {} tail {} credits {:?} records {}\n",peer,report.host,report.firmware,report.peer_credits,report.records.len());
                for (i,body) in report.records.iter().enumerate() {
                    dev_err!(dev,"G17P: render failure peer {} slot {} report DATA {:02x?}\n",peer,(report.host+i as u32)&255,body);
                }
            },
            Err(e)=>dev_err!(dev,"G17P: render failure report snapshot {:?}\n",e),
        }
    }
    fn poll_render(&mut self, dev: &kernel::device::Device, image: &Image,
                   pending: &mut RenderPending, last_in_wave: bool) -> Result<bool> {
        let result=self.poll_render_inner(dev,image,pending,last_in_wave);
        if let Err(error)=&result { self.diagnose_render_failure(dev,image,pending,*error); }
        result
    }
    fn poll_render_inner(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        pending: &mut RenderPending,
        last_in_wave: bool,
    ) -> Result<bool> {
        if pending.started.elapsed().as_millis() >= 5000 {
            return Err(ETIMEDOUT);
        }
        let mut last = [[0u32; 6]; 2];
        let mut status_changed = [false; 2];
        let command_status;
        if self
            .peers
            .iter()
            .any(|p| p.data.crashed.load(Ordering::Acquire))
        {
            return Err(EIO);
        }
        self.service_owned_render_reports(dev, image)?;
        let work = self.render.as_ref().ok_or(EINVAL)?;
        let owner = if pending.ticket.client.root.root() == work.client.root.root() {
            &work.client
        } else {
            &self.render_pool_clients.iter().find(|(_, client)|
                client.root.root() == pending.ticket.client.root.root()).ok_or(EIO)?.1
        };
        pending.ticket.client.refresh_root(owner).inspect_err(|e| dev_err!(dev,"G17P: render fail site refresh_root {:?}\n",e))?;
        let ticket = &pending.ticket;
        let memory = self.memory.as_ref().ok_or(EINVAL)?;
        let vm = self.vm.as_ref().ok_or(EINVAL)?;
        let service = if let Some(native) = self.native.as_ref() {
            &native.service
        } else {
            work.growth.as_ref().ok_or(EINVAL)?
        };
        let (limited, terminals) = if self.native.is_none() {
            let token = ticket.growth.ok_or(EIO)?;
            (service.token_limited(token).inspect_err(|e| dev_err!(dev,"G17P: render fail site token_limited {:?}\n",e))?, service.token_terminals(token).inspect_err(|e| dev_err!(dev,"G17P: render fail site token_terminals {:?}\n",e))?)
        } else {
            (service.limit_report().is_some(), service.terminals())
        };
        let mut done = limited || terminals > pending.terminal_baseline;
        for stage in 0..2 {
            for (i, offset) in [
                queue::POINTER_DONE,
                queue::POINTER_READ,
                queue::POINTER_WRITE,
            ]
            .into_iter()
            .enumerate()
            {
                last[stage][i] = memory.read_firmware32(vm.physical(
                    memory,
                    2,
                    ticket.item.layout.pointers[stage] + offset,
                )?)?;
            }
            for (i, address) in ticket.channels[stage].states.into_iter().enumerate() {
                last[stage][i + 3] = memory.read_firmware32(vm.physical(memory, 2, address)?)?;
            }
            let counters = queue::Counters::new([last[stage][3], last[stage][4], last[stage][5]])
                .map_err(|_| { dev_err!(dev,"G17P: render fail site queue_counters stage {} values {:?}\n",stage,last[stage]); EIO })?;
            done &= ticket.publications[stage].completed(last[stage][0], counters);
            // Match the shim's two independent 0x40-byte status gates.
            // build() initializes these retained private records to zero.
            let pa = vm.physical(
                memory,
                2,
                ticket.statuses[stage],
            )?;
            memory.invalidate(pa, 0x40)?;
            status_changed[stage] = false;
            for offset in (0..0x40).step_by(8) {
                status_changed[stage] |= memory.read64(pa + offset)? != 0;
            }
            done &= status_changed[stage];
        }
        let after = self.report_snapshot(image).inspect_err(|e| dev_err!(dev,"G17P: render fail site report_snapshot {:?}\n",e))?;
        for (peer, (report, before)) in after.iter().zip(pending.startup.iter()).enumerate() {
            // The growth service is the primary report reader. If a new
            // record arrived after its drain, service it next iteration;
            // never ACK it from this completion snapshot.
            if peer == 0 && report.firmware != service.cursor() {
                done = false;
                continue;
            }
            if peer == 0 && self.native.is_some() {
                continue;
            }
            for (index, body) in report.records.iter().enumerate() {
                let opcode = u32::from_le_bytes(body[..4].try_into().unwrap());
                let receipt = index == 0
                    && before.host == 0
                    && report.host == 0
                    && before.firmware == 1
                    && before.records.len() == 1
                    && before.records[0] == *body
                    && body[..12] == [13, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];
                if opcode != 1 && !receipt {
                    dev_err!(dev,"G17P: unhandled render report peer {} slot {} opcode {}; retaining graph\n",peer,(report.host+index as u32)&255,opcode);
                    return Err(EIO);
                }
            }
        }
        let pa = vm.physical(memory, 2, ticket.firmware_completion)?;
        memory.invalidate(pa, 8)?;
        command_status = memory.read64(pa)?;
        if done && command_status != 0 {
            if !pending.control_done {
                if self.native.is_some() {
                    if !self.complete_native_render_control(image)? {
                        return Ok(false);
                    }
                } else {
                    let count = if ticket.item.ordinal == 0 { 2 } else { 1 };
                    for _ in 0..count {
                        self.peers[0]
                            .rtkit
                            .as_mut()
                            .ok_or(EINVAL)?
                            .as_mut()
                            .send_message(0x21, 0x0084000000000011)?;
                    }
                }
                pending.control_done = true;
                // Fresh queues/status/report closure after pumping control.
                return Ok(false);
            }
            if limited {
                // The consumed report replaces only this owner's ordinary
                // terminal. Its own queue, status and fresh report gates
                // above still apply. Preserve BOs and scheduler state.
                self.acknowledge_reports(image, &after).inspect_err(|e| dev_err!(dev,"G17P: render fail site report_ack {:?}\n",e))?;
                if !pending.control_done {
                    self.peers[0]
                        .rtkit
                        .as_mut()
                        .ok_or(EINVAL)?
                        .as_mut()
                        .send_message(0x21, 0x0084000000000011)?;
                }
                if let Some(native) = self.native.as_mut() {
                    native.service.retire_work()?;
                } else {
                    self.render
                        .as_mut()
                        .ok_or(EINVAL)?
                        .growth
                        .as_mut()
                        .ok_or(EINVAL)?
                        .retire_token(ticket.growth.ok_or(EIO)?).inspect_err(|e| dev_err!(dev,"G17P: render fail site retire_token {:?}\n",e))?;
                }
                self.submission_error = Some(ENOMEM);
                pending.gate.set_error(ENOMEM);
                self.finish_render_gate(true)?;
                self.completed_owned_render_fault();
                return Ok(true);
            }
            if !self.defer_cache {
                ticket.client.cache(true).inspect_err(|e| dev_err!(dev,"G17P: render fail site client_cache {:?}\n",e))?;
                for pair in ticket.timestamps.chunks_exact(2) {
                    self.timestamps.as_ref().ok_or(EINVAL)?.cache([pair[0], pair[1]], true)?;
                }
            }
            if *crate::module_parameters::submission_log.value() != 0 {
                dev_info!(dev,"G17P: caller render complete: TA {:?}, 3D {:?}, status {:#x}; reports validated, terminals {} cursor {}\n",last[0],last[1],command_status,terminals,service.cursor());
            }
            self.acknowledge_reports(image, &after).inspect_err(|e| dev_err!(dev,"G17P: render fail site report_ack {:?}\n",e))?;
            // Same control-done boundary as the source synchronous shim.
            if !pending.control_done {
                self.peers[0]
                    .rtkit
                    .as_mut()
                    .ok_or(EINVAL)?
                    .as_mut()
                    .send_message(0x21, 0x0084000000000011)?;
            }
            render::complete_ticket_leaf_publication(
                self.memory.as_mut().ok_or(EINVAL)?,
                self.vm.as_ref().ok_or(EINVAL)?,
                ticket.item,
            )?;
            if last_in_wave && render::quiesce(
                self.memory.as_mut().ok_or(EINVAL)?,
                self.vm.as_ref().ok_or(EINVAL)?,
                self.render.as_ref().ok_or(EINVAL)?,
            )? {
                if *crate::module_parameters::submission_log.value() != 0 {
                    dev_info!(dev, "G17P: completed render scheduler list quiesced\n");
                }
            }
            if let Some(native) = self.native.as_mut() {
                native.service.retire_work()?;
            } else {
                self.render
                    .as_mut()
                    .ok_or(EINVAL)?
                    .growth
                    .as_mut()
                    .ok_or(EINVAL)?
                    .retire_token(ticket.growth.ok_or(EIO)?).inspect_err(|e| dev_err!(dev,"G17P: render fail site retire_token {:?}\n",e))?;
            }
            self.finish_render_gate(false).inspect_err(|e| dev_err!(dev,"G17P: render fail site finish_render_gate {:?}\n",e))?;
            self.completed_owned_render_fault();
            return Ok(true);
        }
        if pending.started.elapsed().as_millis() >= 5000 {
            dev_err!(
                dev,
                "G17P: asynchronous render timeout, queues {:?}, status {:#x}; retaining graph\n",
                last,
                command_status
            );
            return Err(ETIMEDOUT);
        }
        Ok(false)
    }

    fn report_snapshot(&self, image: &Image) -> Result<[Report; 2]> {
        let memory = self.memory.as_ref().ok_or(EINVAL)?;
        let vm = self.vm.as_ref().ok_or(EINVAL)?;
        let mut reports = [Report::new(), Report::new()];
        for (report, channels) in reports.iter_mut().zip(image.graph.channels.iter()) {
            let channel = channels[13];
            if channel.states[1].checked_add(256 * 0x48) != Some(channel.ring) {
                return Err(EINVAL);
            }
            for counter in [0, 2] {
                let pa = vm.physical(memory, 2, channel.states[counter])?;
                let host = memory.read_firmware32(pa)?;
                let firmware = memory.read_firmware32(pa + 0x20)?;
                if host >= 256 || firmware >= 256 {
                    return Err(EIO);
                }
                report.peer_credits[counter / 2] = firmware;
                if counter == 0 {
                    report.host = host;
                    report.firmware = firmware;
                }
            }
            for offset in 0..report.firmware.wrapping_sub(report.host) & 255 {
                let slot = (report.host + offset) & 255;
                let mut body = [0u8; 0x48];
                for index in 0..9 {
                    let pa = vm.physical(
                        memory,
                        2,
                        channel.states[1] + slot as u64 * 0x48 + index * 8,
                    )?;
                    memory.invalidate(pa, 8)?;
                    body[index as usize * 8..index as usize * 8 + 8]
                        .copy_from_slice(&memory.read64(pa)?.to_le_bytes());
                }
                report.records.push(body, GFP_KERNEL)?;
            }
        }
        Ok(reports)
    }

    fn acknowledge_reports(&mut self, image: &Image, reports: &[Report; 2]) -> Result {
        let vm = self.vm.as_ref().ok_or(EINVAL)?;
        let memory = self.memory.as_mut().ok_or(EINVAL)?;
        for (peer, channels) in image.graph.channels.iter().enumerate() {
            for index in [13, 14] {
                for state in [0, 2] {
                    let address = channels[index].states[state];
                    if address == 0 {
                        continue;
                    }
                    // Channel 13 returns only the copied, validated credits.
                    // Channel 14 is source telemetry credit bookkeeping.
                    let value = if index == 13 {
                        reports[peer].peer_credits[state / 2]
                    } else {
                        memory.read_firmware32(vm.physical(memory, 2, address)? + 0x20)?
                    };
                    vm.write(memory, 2, address, &value.to_le_bytes())?;
                }
            }
        }
        g17p_memory::sync();
        Ok(())
    }

    /// The source groups consecutive CL commands into bounded publication
    /// waves, not frontend-asynchronous ioctls. Retain every command's lease
    /// until its own queue target and distinct completion word are observed.
    pub(crate) fn submit_computes(
        &mut self, dev: &kernel::device::Device, image: &Image,
        client: &mut Option<compute::Client>, parameters: &[&compute::Parameters],
    ) -> Result<(usize, Option<kernel::dma_fence::Fence>)> {
        self.submit_computes_with_dependencies(dev, image, client, parameters, &[])
    }

    fn submit_computes_with_dependencies(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        client: &mut Option<compute::Client>,
        parameters: &[&compute::Parameters],
        dependencies: &[(u8, u32)],
    ) -> Result<(usize, Option<kernel::dma_fence::Fence>)> {
        if parameters.is_empty() {
            return Err(EINVAL);
        }
        if self.live_render_count() != 0 && !self.render_compute_owner_compatible()? { return Ok((0,None)); }
        self.restore_owned_render_fault()?;
        self.cleanup.require_idle()?;
        let mut wave = match self.compute_pending.take() {
            Some(wave) => wave,
            None => ComputePending {
                frames: KVec::with_capacity(36, GFP_KERNEL)?,
                gates: KVec::with_capacity(36, GFP_KERNEL)?,
                receipts: KVec::with_capacity(36, GFP_KERNEL)?,
                terminal_baselines: KVec::with_capacity(36, GFP_KERNEL)?,
                startup: [Report::new(), Report::new()],
                next: 0,
                started: kernel::time::Instant::now(),
            },
        };
        let joined = wave.next < wave.frames.len();
        let result = (|| {
            if self.phase == Phase::Prepared {
                let first = parameters[0];
                let owner = client.as_ref().ok_or(EINVAL)?.owner;
                self.bootstrap_compute(dev, image, owner, first.preempt)?;
            }
            let first = 0;
            let frames = &mut wave.frames;
            let gates = &mut wave.gates;
            {
                let next = self.compute.as_ref().map_or(0, |w| w.ordinal + 1);
                let after_render = self
                    .compute
                    .as_ref()
                    .map_or(self.render.is_some(), |w| w.after_render);
                let channel = self
                    .compute
                    .as_ref()
                    .map_or(image.graph.channels[0][8], |w| w.channel);
                let memory = self.memory.as_ref().ok_or(EINVAL)?;
                let vm = self.vm.as_ref().ok_or(EINVAL)?;
                let mut values = [0; 3];
                for (value, address) in values.iter_mut().zip(channel.states) {
                    *value = memory.read_firmware32(vm.physical(memory, 2, address)?)?;
                }
                let credits = queue::Counters::new(values).map_err(|_| EIO)?.available() as usize;
                // Startup executes its opening command before entering the
                // retained wave lifetime, as in the source direct bootstrap.
                let room = if self.phase == Phase::Prepared {
                    1
                } else {
                    // Both compute profiles rotate retired inner backing at
                    // this boundary. A live tail must retire before switching
                    // pointers; capacity pressure is not a publication error.
                    if joined
                        && next % super::g17p_compute_lifecycle::TRANSPORT_INTERVAL == 0
                    {
                        return Ok((0, None));
                    }
                    (super::g17p_compute_lifecycle::TRANSPORT_INTERVAL
                        - next % super::g17p_compute_lifecycle::TRANSPORT_INTERVAL) as usize
                };
                let count = (parameters.len() - first)
                    .min(36 - frames.len())
                    .min(credits)
                    .min(room);
                if count == 0 {
                    return Ok((0, None));
                }
                let mut published = 0;
                for index in 0..count {
                    if let Some(work) = self.compute.as_ref() {
                        let resources = work.resources(work.ordinal + 1)?;
                        if frames.iter().any(|prior| {
                            resources
                                .iter()
                                .zip(prior.resources)
                                .any(|(a, b)| *a != 0 && *a == b)
                        }) {
                            break;
                        }
                        let replacement = if client.as_ref().is_some_and(|next| Self::same_client(next, &work.client)) {
                            client.take(); None
                        } else { client.take() };
                        self.prepare_next_compute(
                            dev,
                            replacement,
                            parameters[first + index],
                            index != 0 || joined,
                            dependencies,
                        )?;
                    } else {
                        self.prepare_compute(
                            dev,
                            image,
                            client.take().ok_or(EINVAL)?,
                            parameters[first + index],
                        )?;
                    }
                    let frame = self.compute.as_ref().ok_or(EINVAL)?.pending()?;
                    let receipt = ComputeReceipt::new(frame.ordinal)?;
                    let gate = receipt.fence.clone();
                    if let Some(contexts) = self.compute_contexts.as_mut() {
                        contexts.retain(
                            self.compute.as_ref().ok_or(EINVAL)?.client.owner,
                            frame.ordinal,
                            gate.clone(),
                        )?;
                    }
                    gates.push(gate, GFP_KERNEL)?;
                    self.timestamps
                        .as_ref()
                        .ok_or(EINVAL)?
                        .cache(frame.timestamps, false)?;
                    if self.phase == Phase::Prepared {
                        self.start(dev, image)?;
                        if let Some(work) = self.dormant_render.as_ref() {
                            work.initialize_operands(
                                self.memory.as_mut().ok_or(EINVAL)?,
                                self.vm.as_ref().ok_or(EINVAL)?,
                            )?;
                        }
                    } else if self.phase != Phase::Running {
                        return Err(EIO);
                    }
                    if index == 0 && !joined {
                        wave.startup = self.report_snapshot(image)?;
                    }
                    if let Some(native) = self.native.as_mut() {
                        if !native.complete() {
                            return Err(EBUSY);
                        }
                        native.service.begin_compute(frame.ordinal)?;
                    } else if let Some(render) = self.render.as_mut() {
                        render
                            .growth
                            .as_mut()
                            .ok_or(EINVAL)?
                            .begin_compute(frame.ordinal)?;
                    }
                    if after_render && frame.ordinal == 2 {
                        self.stage_compute_tick(image)?;
                    }
                    // Metadata and report owner precede either producer.
                    let terminal_baseline = match wave.terminal_baselines.last() {
                        Some(previous) => previous.checked_add(1).ok_or(EOVERFLOW)?,
                        None => self.render.as_ref().and_then(|r| r.growth.as_ref())
                            .map_or(0, |service| service.compute_terminals()),
                    };
                    wave.terminal_baselines.push(terminal_baseline, GFP_KERNEL)?;
                    wave.receipts.push(receipt.clone(), GFP_KERNEL)?;
                    frames.push(frame, GFP_KERNEL)?;
                    let frame = frames.last().ok_or(EIO)?;
                    let memory = self.memory.as_mut().ok_or(EINVAL)?;
                    let vm = self.vm.as_ref().ok_or(EINVAL)?;
                    if let Some((address, value)) = frame.publication.deferred_inner {
                        vm.write(memory, 2, address, &value.to_le_bytes())?;
                        g17p_memory::sync();
                    }
                    let (address, value) = frame.publication.deferred_outer.ok_or(EINVAL)?;
                    vm.write(memory, 2, address, &value.to_le_bytes())?;
                    g17p_memory::sync();
                    if self.native.is_none() && self.compute.as_ref().is_some_and(|w| w.after_render) {
                        let point = frame.milestone;
                        receipt.point.store((u64::from(point.event_slot) << 40) |
                            (u64::from(point.grid) << 32) | u64::from(point.value), Ordering::Relaxed);
                    }
                    receipt.publication_ready.store(true, Ordering::Release);
                    published += 1;
                }
                if published == 0 {
                    return Ok((0, None));
                }
                self.peers[0]
                    .rtkit
                    .as_mut()
                    .ok_or(EINVAL)?
                    .as_mut()
                    .send_message(0x21, queue::COMPUTE_DOORBELL)?;
                let client_root = self.compute.as_ref().ok_or(EINVAL)?.client.root.root();
                if *crate::module_parameters::submission_log.value() != 0 {
                    dev_info!(dev,"G17P: asynchronous compute published {} commands, {} hardware leases pending, root {:#x}\n",
                        published,frames.len()-wave.next,client_root);
                }
                return Ok((published, gates.last().cloned()));
            }
        })();
        self.compute_pending = Some(wave);
        if let Err(error) = &result {
            self.phase = Phase::Failed;
            if let Some(timestamps) = self.timestamps.as_mut() {
                timestamps.fail_pending(*error);
            }
            if let Some(contexts) = self.compute_contexts.as_mut() {
                contexts.fail_pending(*error);
            }
        }
        result
    }

    /// Source stage_runtime_tick(0, context_word=2, update_sequence=True)
    /// precedes the first retained ordinary command; the CL kick carries it.
    fn stage_compute_tick(&mut self, image: &Image) -> Result {
        let channel = image.graph.channels[0][12];
        let vm = self.vm.as_ref().ok_or(EINVAL)?;
        let memory = self.memory.as_mut().ok_or(EINVAL)?;
        let producer = memory.read_firmware32(vm.physical(memory, 2, channel.states[2])?)?;
        if producer >= 256 {
            return Err(EBUSY);
        }
        let mut body = [0; 0x40];
        body[..4].copy_from_slice(&0x2eu32.to_le_bytes());
        body[12..16].copy_from_slice(&2u32.to_le_bytes());
        vm.write(memory, 2, channel.ring + producer as u64 * 0x40, &body)?;
        g17p_memory::sync();
        vm.write(memory, 2, channel.states[2], &(producer + 1).to_le_bytes())?;
        g17p_memory::sync();
        Ok(())
    }

    fn poll_compute(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        frame: &compute::Pending,
        startup: &[Report; 2],
        copyback: bool,
        service_reports: bool,
        terminal_baseline: Option<u32>,
    ) -> Result<bool> {
        let timestamps = frame.timestamps;
        let mut last = [0u32; 6];
        let command_status;
        if self
            .peers
            .iter()
            .any(|p| p.data.crashed.load(Ordering::Acquire))
        {
            return Err(EIO);
        }
        if let Some(native) = self.native.as_mut() {
            if !native.complete() {
                return Err(EBUSY);
            }
            let vm = self.vm.as_ref().ok_or(EINVAL)?;
            for _ in 0..32 {
                use super::g17p_growth_runtime::Action;
                let action = native.service.step_dependency_compute(
                    self.memory.as_mut().ok_or(EINVAL)?,
                    vm,
                    &mut self.compute.as_mut().ok_or(EINVAL)?.client.root,
                    self.ttbs,
                    frame.ordinal,
                )?;
                match action {
                    Action::Idle => break,
                    Action::Consumed => (),
                    _ => return Err(EIO),
                }
            }
        } else if self.render.is_some() {
            self.service_owned_render_reports(dev, image)?;
        }
        let vm = self.vm.as_ref().ok_or(EINVAL)?;
        let work = frame;
        let memory = self.memory.as_ref().ok_or(EINVAL)?;
        for (index, offset) in [
            queue::POINTER_DONE,
            queue::POINTER_READ,
            queue::POINTER_WRITE,
        ]
        .into_iter()
        .enumerate()
        {
            last[index] =
                memory.read_firmware32(vm.physical(memory, 2, work.pointers + offset)?)?;
        }
        for (index, address) in work.channel.states.into_iter().enumerate() {
            last[index + 3] = memory.read_firmware32(vm.physical(memory, 2, address)?)?;
        }
        let counters = queue::Counters::new([last[3], last[4], last[5]]).map_err(|_| EIO)?;
        if work.publication.completed(last[0], counters) {
            if let Some(baseline) = terminal_baseline {
                if self.render.as_ref().and_then(|r| r.growth.as_ref())
                    .ok_or(EIO)?.compute_terminals() <= baseline { return Ok(false); }
            }
            let after = self.report_snapshot(image)?;
            let cursor_pending = if let Some(native) = self.native.as_ref() {
                native.service.cursor() != after[0].firmware
            } else {
                self.render.as_ref().is_some_and(|render| {
                    render
                        .growth
                        .as_ref()
                        .is_none_or(|service| service.cursor() != after[0].firmware)
                })
            };
            if cursor_pending {
                return Ok(false);
            }
            for (peer, (report, before)) in after.iter().zip(startup.iter()).enumerate() {
                // The retained native reader has already validated every
                // primary record through this exact firmware cursor.
                if peer == 0 && self.native.is_some() {
                    return Ok(false);
                }
                for (index, body) in report.records.iter().enumerate() {
                    let opcode = u32::from_le_bytes(body[..4].try_into().unwrap());
                    let startup_receipt = index == 0
                        && before.host == 0
                        && report.host == 0
                        && before.firmware == 1
                        && before.records.len() == 1
                        && before.records[0] == *body
                        && body[..12] == [13, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0];
                    if opcode != 1 && !startup_receipt {
                        dev_err!(
                            dev,
                            "G17P: unhandled compute report peer {} slot {} opcode {}\n",
                            peer,
                            (report.host + index as u32) & 255,
                            opcode
                        );
                        return Err(EIO);
                    }
                }
            }
            let status_pa = vm.physical(memory, 2, work.status[1])?;
            memory.invalidate(status_pa, 8)?;
            command_status = memory.read64(status_pa)?;
            // Source completion has a second gate: transport retirement
            // may precede command execution or a fatal report. Every
            // command's status pair was cleared before publication.
            if command_status == 0 {
                return Ok(false);
            }
            if !self.defer_cache {
                if copyback { frame.client.cache(true)?; }
                self.timestamps.as_ref().ok_or(EINVAL)?.cache(timestamps, true)?;
            }
            if *crate::module_parameters::submission_log.value() != 0 {
                dev_info!(
                    dev,
                    "G17P: caller compute complete: queue {:?}, channel {:?}, status {:#x}; reports validated\n",
                    &last[..3],
                    &last[3..], command_status
                );
            }
            if service_reports {
                self.acknowledge_reports(image, &after)?;
            }
            let service = if let Some(native) = &mut self.native {
                Some(&mut native.service)
            } else if let Some(render) = &mut self.render {
                Some(render.growth.as_mut().ok_or(EINVAL)?)
            } else {
                None
            };
            if let Some(service) = service {
                service.finish_compute(frame.ordinal)?;
                if *crate::module_parameters::submission_log.value() != 0 {
                    dev_info!(
                        dev,
                        "G17P: mixed reports: render terminals {}, compute terminals {}, cursor {}\n",
                        service.terminals(),
                        service.compute_terminals(),
                        service.cursor()
                    );
                }
            }
            return Ok(true);
        }
        Ok(false)
    }

    // Bring-up's private opening command is completed before exposing the
    // device's ordinary retained queues. Caller work never uses this wait.
    fn finish_compute(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        frame: &compute::Pending,
        startup: &[Report; 2],
        copyback: bool,
        service_reports: bool,
    ) -> Result {
        for _ in 0..200 {
            if self.poll_compute(dev, image, frame, startup, copyback, service_reports, None)? {
                return Ok(());
            }
            kernel::time::delay::fsleep(kernel::time::Delta::from_millis(10));
        }
        Err(ETIMEDOUT)
    }

    /// Join only the exact installed caller snapshot, with an available private
    /// pool. Deferred admission clients need not materialize another root.
    fn same_client(next: &compute::Client, old: &compute::Client) -> bool {
        next.owner == old.owner && next.bindings == old.bindings
            && next.buffers.len() == old.buffers.len()
            && next.buffers.iter().zip(&old.buffers).all(|(a,b)| core::ptr::eq(&**a, &**b))
    }

    /// Read-only capacity and mapping ownership test. A blocked render does
    /// not reserve any compute resources or public-queue completion state.
    pub(crate) fn can_stage_render(&self, next: Option<&compute::Client>, p: &super::g17p_render::Parameters) -> Result<bool> {
        if let Err(e) = self.cleanup.require_idle() { if e == EBUSY { return Ok(false); } return Err(e); }
        if self.phase == Phase::Failed { return Err(self.submission_error.unwrap_or(EIO)); }
        if self.native.as_ref().is_some_and(|n| !n.complete()) { return Ok(false); }
        if self.phase == Phase::Prepared { return Ok(self.pending.is_none() && self.compute_pending.is_none()); }
        if self.render.is_none() {
            if self.compute_pending.is_some() { return Ok(false); }
            if let Some(work) = self.compute.as_ref() {
                return match compute::idle(self.memory.as_ref().ok_or(EIO)?, self.vm.as_ref().ok_or(EIO)?, work) {
                    Ok(_) => Ok(true), Err(e) if e == EBUSY => Ok(false), Err(e) => Err(e),
                };
            }
            return Ok(false);
        }
        if !self.can_append_render_for(next, p)? { return Ok(false); }
        if self.render.as_ref().ok_or(EIO)?.ordinal >= 1 {
            let memory = self.memory.as_ref().ok_or(EIO)?;
            let vm = self.vm.as_ref().ok_or(EIO)?;
            let channel = self.render.as_ref().ok_or(EIO)?.growth.as_ref().ok_or(EIO)?.command_channel();
            let mut values = [0;3];
            for (v,at) in values.iter_mut().zip(channel.states) { *v = memory.read_firmware32(vm.physical(memory,2,at)?)?; }
            if self.independent_render_roots() {
                if queue::Counters::new(values).map_err(|_| EIO)?.available() == 0 { return Ok(false); }
            } else if values[0] != values[2] || values[1] != values[2] { return Ok(false); }
        }
        if let (Some(next), Some(work)) = (next, self.render.as_ref()) {
            if !Self::same_client(next, &work.client) && self.live_render_count() != 0
                && !self.independent_render_roots() { return Ok(false); }
        }
        Ok(true)
    }

    /// The compute root and private resources are independent of a live
    /// render unless this command actually replaces a shared private leaf.
    pub(crate) fn can_stage_compute(&self, next: Option<&compute::Client>, p: &compute::Parameters) -> Result<bool> {
        if let Err(e) = self.cleanup.require_idle() { if e == EBUSY { return Ok(false); } return Err(e); }
        if self.phase == Phase::Failed { return Err(self.submission_error.unwrap_or(EIO)); }
        if self.native.as_ref().is_some_and(|n| !n.complete()) { return Ok(false); }
        let Some(work) = self.compute.as_ref() else {
            return Ok(self.live_render_count() == 0 && self.render_control.is_none()
                && !matches!(self.pending, Some(PendingWork::Control(_) | PendingWork::Native)));
        };
        if self.phase != Phase::Running { return Ok(false); }
        let ordinary = work.after_render && self.native.is_none() && work.native_context.is_none()
            && self.compute_contexts.is_none() && self.render_contexts.is_none();
        if self.live_render_count() != 0 && (!ordinary || !self.render_compute_owner_compatible()?) { return Ok(false); }
        let replacing = next.is_some_and(|next| !Self::same_client(next, &work.client));
        if replacing || p.preempt != work.preempt {
            if self.compute_pending.is_some() { return Ok(false); }
            match compute::idle(self.memory.as_ref().ok_or(EIO)?, self.vm.as_ref().ok_or(EIO)?, work) {
                Ok(_) => (), Err(e) if e == EBUSY => return Ok(false), Err(e) => return Err(e),
            }
            if p.preempt != work.preempt && (self.pending.is_some() || self.render_control.is_some()) { return Ok(false); }
        }
        let ordinal = work.ordinal.checked_add(1).ok_or(EOVERFLOW)?;
        if ordinal >= work.capacity() { return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32))); }
        if ordinary && ordinal == 2 && (self.render_control.is_some()
            || matches!(self.pending, Some(PendingWork::Control(_)))) { return Ok(false); }
        if let Some(wave) = &self.compute_pending {
            if wave.frames.len() - wave.next >= 36 || ordinal % super::g17p_compute_lifecycle::TRANSPORT_INTERVAL == 0 {
                return Ok(false);
            }
            let resources = work.resources(ordinal)?;
            if wave.frames[wave.next..].iter().any(|prior| resources.iter().zip(prior.resources)
                .any(|(a,b)| *a != 0 && *a == b)) { return Ok(false); }
        }
        let memory = self.memory.as_ref().ok_or(EIO)?;
        let vm = self.vm.as_ref().ok_or(EIO)?;
        let mut values = [0; 3];
        for (value, va) in values.iter_mut().zip(work.channel.states) { *value = memory.read_firmware32(vm.physical(memory,2,va)?)?; }
        if queue::Counters::new(values).map_err(|_| EIO)?.available() == 0 { return Ok(false); }
        // Registered root words remain authoritative for the retained client.
        for slot in [2u64,3] {
            if memory.word64(self.ttbs + slot*16)?.load() != ((slot << 48) | work.client.root.root() | 1) { return Err(EIO); }
        }
        Ok(true)
    }

    pub(crate) fn stage_compute_ticket(&mut self, dev: &kernel::device::Device, image: &Image,
        client: &mut Option<compute::Client>, p: &compute::Parameters, dependencies: &[(u8,u32)],
    ) -> Result<Option<Arc<ComputeReceipt>>> {
        if !self.can_stage_compute(client.as_ref(), p)? { return Ok(None); }
        if dependencies.len() > 3 || dependencies.iter().any(|&(g,v)| g >= 128 || v == 0 || v >= 1 << 30) { return Err(EINVAL); }
        // Opening construction keeps the source's proven serialized lifetime.
        if self.compute.is_none() && !dependencies.is_empty() { return Ok(None); }
        let (count, _) = self.submit_computes_with_dependencies(dev,image,client,&[p],dependencies)?;
        if count == 0 { return Ok(None); }
        Ok(Some(self.compute_pending.as_ref().ok_or(EIO)?.receipts.last().ok_or(EIO)?.clone()))
    }

    pub(crate) fn stage_render_ticket(&mut self, dev: &kernel::device::Device, image: &Image,
        client: &mut Option<compute::Client>, p: &super::g17p_render::Parameters,
        prepared_append: Option<KBox<render::PreparedAppend>>,
    ) -> Result<Option<Arc<RenderReceipt>>> {
        if !self.can_stage_render(client.as_ref(), p)? { return Ok(None); }
        if self.render.is_none() || self.phase == Phase::Prepared {
            return self.submit_render_ticket(dev,image,client.take().ok_or(EINVAL)?,p).map(Some);
        }
        let replacement = match (client.as_ref(),self.render.as_ref()) {
            (Some(next),Some(old)) if Self::same_client(next,&old.client) => { client.take(); None },
            _ => client.take(),
        };
        self.submit_prepared_render_ticket(dev,image,replacement,None,p,prepared_append).map(Some)
    }

    pub(crate) fn can_join_render(&self, next: Option<&compute::Client>,
                                  p: &super::g17p_render::Parameters) -> Result<bool> {
        if self.native.is_some() || !self.render_compute_owner_compatible()?
            || *crate::module_parameters::native_render_vms.value() != 0
            || *crate::module_parameters::partial_independent_owner.value() != 1
            || *crate::module_parameters::alternate_queue_pairs.value() != 1
            || !matches!(self.pending, Some(PendingWork::Render(_)))
            || !self.can_append_render_for(next, p)? { return Ok(false); }
        let (Some(next), Some(work)) = (next, self.render.as_ref()) else { return Ok(false); };
        let old = &work.client;
        if work.layout.native || (!self.independent_render_roots() && !work.same_geometry(p)) || next.owner != old.owner
            || next.bindings != old.bindings || next.buffers.len() != old.buffers.len()
            || !next.buffers.iter().zip(&old.buffers).all(|(a,b)| core::ptr::eq(&**a, &**b)) {
            return Ok(false);
        }
        let memory = self.memory.as_ref().ok_or(EIO)?;
        let low = memory.word64(self.ttbs + 16)?.load();
        let high = memory.word64(self.ttbs + 24)?.load();
        Ok(low == ((1u64 << 48) | old.root.root() | 1) && high & 1 != 0 && high >> 48 == 1)
    }

    pub(crate) fn can_join_compute(&self, next: Option<&compute::Client>) -> bool {
        if self.phase != Phase::Running
            || self.native.is_some()
            || self.compute_pending.is_none()
        {
            return false;
        }
        match (next, self.compute.as_ref()) {
            (Some(next), Some(old)) => {
                next.owner == old.client.owner
                    && next.bindings == old.client.bindings
                    && next.buffers.len() == old.client.buffers.len()
                    && next
                        .buffers
                        .iter()
                        .zip(&old.client.buffers)
                        .all(|(a, b)| core::ptr::eq(&**a, &**b))
            }
            _ => false,
        }
    }
    pub(crate) fn finish_owned_submission(&mut self, result: Result) -> Result<Option<Error>> {
        if self.phase == Phase::Failed { return self.finish_submission(result); }
        result.map(|()| None)
    }
    pub(crate) fn remember_owned_completed(&mut self, fence: kernel::dma_fence::Fence, failed: bool) {
        self.remember_owned_render_completed(fence, failed);
    }
    pub(crate) fn notify_failure(&self, error: Error) {
        if self.phase == Phase::Failed {
            self.events.fail(error);
        }
    }
    pub(crate) fn events(&self) -> Arc<super::g17p_drm::asynchronous::Events> {
        self.events.clone()
    }
    /// Return CPU visibility work even if another owner failed during this
    /// poll: every extracted command has independently verified retirement.
    /// The caller must release the device mutex before executing this list.
    pub(crate) fn poll_work_deferred(&mut self, dev: &kernel::device::Device,
        image: &Image) -> (Result<bool>, KVec<Completion>) {
        // Both successful and errored signaled fences release this scheduling
        // lease. These are already-retired GPU tickets or joined consumer
        // completions; keeping a negative fence exhausts all render pools.
        // Physical published backing remains Memory-owned until shutdown.
        self.render_cpu_leases.retain(|(_, fence)|
            unsafe { kernel::bindings::dma_fence_get_status(fence.raw()) } == 0);
        self.defer_cache = true;
        let result = self.poll_work(dev, image);
        self.defer_cache = false;
        (result, core::mem::take(&mut self.completions))
    }
    pub(crate) fn fail_cpu_completion(&mut self, error: Error) {
        self.phase = Phase::Failed;
        self.submission_error = Some(error);
        self.events.fail(error);
        self.fail_owned_receipts(error);
    }

    pub(crate) fn poll_work(
        &mut self, dev: &kernel::device::Device, image: &Image,
    ) -> Result<bool> {
        let Some(mut control) = self.render_control.take() else {
            return self.poll_pending_work(dev, image);
        };
        // Drain/classify existing owners' growth reports before the append
        // control reaches its consumer. Never replace a live wave with control.
        if let Err(error) = self.poll_pending_work(dev, image) {
            control.receipt.fail(error);
            self.render_control = Some(control);
            return Err(error);
        }
        if let Err(error) = self.poll_control(dev, image, &mut control) {
            control.receipt.fail(error);
            self.render_control = Some(control);
            self.phase = Phase::Failed;
            self.events.fail(error);
            return Err(error);
        }
        if self.render.as_ref().ok_or(EIO)?.ordinal < control.ordinal {
            self.render_control = Some(control);
            return Ok(false);
        }
        self.poll_pending_work(dev, image)
    }

    fn fail_owned_receipts(&self, error: Error) {
        self.independent_compute.fail(error);
        for pending in &self.independent_pending { pending.receipt.fail(error); }
        if let Some(control) = &self.render_control { control.receipt.fail(error); }
        if let Some(pending) = &self.pending {
            match pending {
                PendingWork::Render(wave) => for state in &wave.frames[wave.next..] { state.receipt.fail(error); },
                PendingWork::Control(state) => state.receipt.fail(error),
                PendingWork::Native => (),
            }
        }
        if let Some(wave) = &self.compute_pending {
            for receipt in &wave.receipts[wave.next..] { receipt.fail(error); }
        }
    }
    fn poll_pending_work(&mut self, dev: &kernel::device::Device, image: &Image) -> Result<bool> {
        let result = (|| -> Result<bool> {
            if self.phase != Phase::Failed && self.native.is_none()
                && self.render.as_ref().is_some_and(|r| r.growth.is_some()) {
                self.service_owned_render_reports(dev,image)?;
            }
            // A waiting render must never prevent compute progress/retirement.
            let render_done = self.poll_render_pending_work(dev, image)?;
            let compute_done = self.poll_compute_pending_work(dev, image)?;
            let independent_done=self.poll_independent_compute(dev,image)?;
            Ok(render_done && compute_done && independent_done)
        })();
        if let Err(error) = result {
            self.phase = Phase::Failed; self.events.fail(error);
            self.fail_owned_receipts(error);
        }
        result
    }

    fn poll_compute_pending_work(&mut self, dev: &kernel::device::Device, image: &Image) -> Result<bool> {
        if self.phase == Phase::Failed {
            if let Some(state) = &self.compute_pending {
                for receipt in &state.receipts[state.next..] { receipt.fail(self.submission_error.unwrap_or(EIO)); }
            }
            return Err(self.submission_error.unwrap_or(EIO));
        }
        let Some(mut state) = self.compute_pending.take() else { return Ok(true); };
        let result = (|| -> Result<bool> {

                    while state.next < state.frames.len() {
                        if !self.poll_compute(
                            dev,
                            image,
                            &state.frames[state.next],
                            &state.startup,
                            true,
                            true,
                            if self.compute.as_ref().is_some_and(|w| w.after_render) {
                                Some(state.terminal_baselines[state.next])
                            } else { None },
                        )? {
                            if state.started.elapsed().as_millis() >= 2000 {
                                return Err(ETIMEDOUT);
                            }
                            // Each prefix frame passed queue retirement, its
                            // own status, report validation and caller cache /
                            // timestamps before its gate was signalled below.
                            // Reclaim only these host records and fence refs.
                            // Published backing remains session-owned, and
                            // the surviving frames keep their exact identity.
                            if state.next != 0 {
                                drop(state.frames.drain(..state.next));
                                drop(state.gates.drain(..state.next));
                                drop(state.receipts.drain(..state.next));
                                drop(state.terminal_baselines.drain(..state.next));
                                state.next = 0;
                            }
                            return Ok(false);
                        }
                        if self.defer_cache {
                            self.completions.reserve(1, GFP_KERNEL)?;
                            let stamps = self.timestamps.as_ref().ok_or(EIO)?
                                .cache_seed(&state.frames[state.next].timestamps)?;
                            let frame = state.frames.remove(state.next).map_err(|_| EIO)?;
                            let fence = state.gates.remove(state.next).map_err(|_| EIO)?;
                            drop(state.receipts.remove(state.next).map_err(|_| EIO)?);
                            state.terminal_baselines.remove(state.next).map_err(|_| EIO)?;
                            self.completions.push(Completion { client: frame.client, stamps, fence, grid: frame.milestone.grid, error: None }, GFP_KERNEL)?;
                        } else {
                            state.gates[state.next].signal();
                            state.next += 1;
                        }
                        if let Some(contexts) = self.compute_contexts.as_mut() { contexts.reap(); }
                        state.started = kernel::time::Instant::now();
                    }
                    Ok(true)
        })();
        match result {
            Ok(true) => Ok(true),
            Ok(false) => { self.compute_pending = Some(state); Ok(false) },
            Err(error) => {
                for receipt in &state.receipts[state.next..] { receipt.fail(error); }
                self.compute_pending = Some(state);
                self.phase = Phase::Failed;
                self.events.fail(error);
                self.fail_render_gate(error);
                Err(error)
            },
        }
    }

    fn poll_render_pending_work(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
    ) -> Result<bool> {
        if self.phase == Phase::Failed {
            return Err(self.submission_error.unwrap_or(EIO));
        }
        let Some(mut pending) = self.pending.take() else {
            return Ok(true);
        };
        let result = (|| -> Result<bool> {
            match &mut pending {
                PendingWork::Native => self.poll_native(dev, image),
                PendingWork::Control(state) => self.poll_control(dev, image, state),
                PendingWork::Render(wave) => {
                    if wave.next != 0 {
                        drop(wave.frames.drain(..wave.next));
                        wave.next = 0;
                    }
                    // A blocked scene does not retain another pool's credit.
                    // Probe every exact ticket and retire each completed owner
                    // independently; physical pages remain session-owned.
                    let mut index = 0;
                    while index < wave.frames.len() {
                        let sole = wave.frames.len() == 1;
                        let state = &mut wave.frames[index];
                        let control_before = state.control_done;
                        let mut done = self.poll_render(dev, image, state, sole)?;
                        if !done && !control_before && state.control_done {
                            done = self.poll_render(dev, image, state, sole)?;
                        }
                        if done {
                            let stamps = if self.defer_cache {
                                self.completions.reserve(1, GFP_KERNEL)?;
                                self.render_cpu_leases.reserve(1, GFP_KERNEL)?;
                                Some(self.timestamps.as_ref().ok_or(EIO)?.cache_seed(&state.ticket.timestamps)?)
                            } else { None };
                            let state = wave.frames.remove(index).map_err(|_| EIO)?;
                            if let Some(stamps) = stamps {
                                // GPU retirement permits a private CPU cache
                                // task, but the published milestone stays live
                                // until its receipt fence is signaled. Do not
                                // reset this pool's registration in that gap.
                                self.render_cpu_leases.push((state.ticket.item.layout.pair,
                                    state.gate.clone()), GFP_KERNEL)?;
                                self.completions.push(Completion { client: state.ticket.client,
                                    stamps, fence: state.gate, grid: state.ticket.item.layout.grids[1] as u8, error: None }, GFP_KERNEL)?;
                            } else { state.gate.signal(); }
                        } else {
                            index += 1;
                        }
                    }
                    Ok(wave.frames.is_empty())
                },

            }
        })();
        match result {
            Ok(false) => {
                if self.pending.is_none() {
                    self.pending = Some(pending);
                }
                Ok(false)
            }
            Ok(true) => Ok(true),
            Err(error) => {
                self.phase = Phase::Failed;
                self.events.fail(error);
                self.fail_render_gate(error);
                if let PendingWork::Control(control) = &pending { control.receipt.fail(error); }
                if let PendingWork::Render(wave) = &pending {
                    for state in &wave.frames[wave.next..] {
                        state.receipt.fail(error);
                        if unsafe { kernel::bindings::dma_fence_get_status(state.gate.raw()) } == 0 {
                            state.gate.set_error(error);
                            state.gate.signal();
                        }
                    }
                }
                if let Some(timestamps) = self.timestamps.as_mut() {
                    timestamps.fail_pending(error);
                }
                if let Some(contexts) = self.compute_contexts.as_mut() {
                    contexts.fail_pending(error);
                }
                // A failed publication retains all backing and its metadata.
                self.pending = Some(pending);
                Err(error)
            }
        }
    }

    pub(crate) fn start(&mut self, dev: &kernel::device::Device, image: &Image) -> Result {
        if self.phase != Phase::Prepared {
            return Err(EBUSY);
        }
        self.phase = Phase::Starting;
        let result = self.start_inner(dev, image);
        self.phase = if result.is_ok() {
            Phase::Running
        } else {
            Phase::Failed
        };
        result
    }

    fn start_inner(&mut self, dev: &kernel::device::Device, image: &Image) -> Result {
        g17p_memory::sync();
        for (peer, root) in self.peers.iter_mut().zip(image.graph.roots()) {
            let rtkit = peer.rtkit.as_mut().ok_or(EINVAL)?;
            rtkit.as_mut().start_endpoint(0x20)?;
            rtkit.as_mut().start_endpoint(0x21)?;
            let message = (0x81u64 << 48) | (root & ((1 << 44) - 1));
            dev_info!(
                dev,
                "G17P: publishing {} initdata {:#018x}\n",
                peer.data.name,
                message
            );
            rtkit.as_mut().send_message(0x20, message)?;
        }
        for _ in 0..500 {
            if self
                .peers
                .iter()
                .any(|p| p.data.crashed.load(Ordering::Acquire))
            {
                return Err(EIO);
            }
            if self
                .peers
                .iter()
                .all(|p| p.data.acknowledged.load(Ordering::Acquire))
            {
                dev_info!(dev, "G17P: both firmware instances acknowledged Rust initdata; no workload submitted\n");
                self.start_control(dev, image)?;
                return Ok(());
            }
            kernel::time::delay::fsleep(kernel::time::Delta::from_millis(10));
        }
        for peer in &self.peers {
            dev_err!(
                dev,
                "G17P: {} initdata timeout: acknowledged={} last={:#018x}\n",
                peer.data.name,
                peer.data.acknowledged.load(Ordering::Acquire),
                peer.data.last_message.load(Ordering::Acquire)
            );
        }
        Err(ETIMEDOUT)
    }

    fn start_control(&mut self, dev: &kernel::device::Device, image: &Image) -> Result {
        for (index, peer) in self.peers.iter_mut().enumerate() {
            if peer.data.crashed.load(Ordering::Acquire) {
                return Err(EIO);
            }
            peer.rtkit
                .as_mut()
                .ok_or(EINVAL)?
                .as_mut()
                .send_message(0x21, 0x0089000000000000)?;
            if index == 0 {
                kernel::time::delay::fsleep(kernel::time::Delta::from_millis(12));
            }
        }
        let vm = self.vm.as_ref().ok_or(EINVAL)?;
        let memory = self.memory.as_mut().ok_or(EINVAL)?;
        let status = image.graph.base
            + layout::NATIVE_PRIMARY_WORK_STATE_OFFSET as u64
            + layout::NATIVE_STATUS_B_OFFSET as u64;
        for (offset, value) in opening::status_config(image.graph.addresses[1]) {
            let va = status + offset as u64;
            memory.invalidate(vm.physical(memory, 2, va)?, 8)?;
            vm.write(memory, 2, va, &value.to_le_bytes())?;
        }
        self.peers[0]
            .rtkit
            .as_mut()
            .ok_or(EINVAL)?
            .as_mut()
            .send_message(0x21, 0x0084000000000011)?;
        // These are presented-consumed counters, not a completion fence. A
        // bounded observation catches startup corruption/crashes before probe
        // succeeds without claiming that a GPU workload has run.
        kernel::time::delay::fsleep(kernel::time::Delta::from_millis(100));
        for (peer, channels) in self.peers.iter().zip(image.graph.channels.iter()) {
            if peer.data.crashed.load(Ordering::Acquire) {
                return Err(EIO);
            }
            let mut counters = [0u32; 3];
            for (index, va) in channels[12].states.iter().enumerate() {
                counters[index] = memory.read_firmware32(vm.physical(memory, 2, *va)?)?;
            }
            dev_info!(
                dev,
                "G17P: {} opening counters {:?} (source-presented consumed)\n",
                peer.data.name,
                counters
            );
            if counters != [1, 1, 1] {
                return Err(EIO);
            }
        }
        dev_info!(
            dev,
            "G17P: control start 0x89/0x84 complete; first-work outer producers withheld\n"
        );
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let mut stopped = true;
        for peer in self.peers.iter_mut().rev() {
            if !peer.started {
                continue;
            }
            if let Some(rtkit) = peer.rtkit.as_mut() {
                if let Err(error) = rtkit.as_mut().shutdown() {
                    stopped = false;
                    dev_err!(
                        peer.data.dev.as_ref(),
                        "G17P: {} shutdown failed {:?}; retaining GPU memory\n",
                        peer.data.name,
                        error
                    );
                }
            }
        }
        if !stopped {
            // Firmware may still reference these pages. The reservation is
            // permanent; keep the runtime owner too as it gains owned tables.
            core::mem::forget(self.memory.take());
            core::mem::forget(self.compute.take());
            core::mem::forget(core::mem::replace(&mut self.independent_compute, super::g17p_compute_queues::Queues::new()));
            core::mem::forget(core::mem::replace(&mut self.independent_pending, KVec::new()));
            core::mem::forget(self.compute_contexts.take());
            core::mem::forget(self.render_contexts.take());
            core::mem::forget(core::mem::replace(&mut self.render_clients, KVec::new()));
            core::mem::forget(core::mem::replace(&mut self.render_pool_clients, KVec::new()));
            core::mem::forget(self.render.take());
            core::mem::forget(self.dormant_render.take());
            core::mem::forget(self.timestamps.take());
            core::mem::forget(core::mem::replace(&mut self.faults, fault::State::new()));
            core::mem::forget(core::mem::replace(&mut self.retained_buffers, KVec::new()));
        }
    }
}

impl Session {
    pub(crate) fn independent_compute_enabled(&self) -> bool {
        *crate::module_parameters::compute_queues.value()==1 && self.native.is_none()
            && *crate::module_parameters::native_compute_vms.value()==0
            && *crate::module_parameters::native_render_vms.value()==0
            && *crate::module_parameters::native_barriers.value()==0
    }
    pub(crate) fn independent_compute_client(&self,key:super::g17p_compute_queues::Key,
        client:&compute::Client,p:&compute::Parameters,priority:u32)->Option<&compute::Client> {
        self.independent_compute.client(key,client,p,priority)
    }
    pub(crate) fn selected_compute_grid(&self,key:super::g17p_compute_queues::Key,
        client:&compute::Client,p:&compute::Parameters,priority:u32)->Option<u8> {
        self.independent_compute.selected_grid(key,client,p,priority)
    }
    pub(crate) fn can_stage_independent_compute(&mut self,image:&Image,key:super::g17p_compute_queues::Key,
        client:&compute::Client,p:&compute::Parameters,priority:u32)->Result<(bool,bool)> {
        if self.phase==Phase::Failed {return Err(EIO);}
        if !matches!(self.phase,Phase::Prepared|Phase::Running) || !self.independent_compute_enabled() {return Ok((false,false));}
        let memory=self.memory.as_ref().ok_or(EIO)?;let vm=self.vm.as_ref().ok_or(EIO)?;
        let mut values=[0;3];
        for (v,at) in values.iter_mut().zip(image.graph.channels[0][queue::COMPUTE_CHANNEL].states) {
            *v=memory.read_firmware32(vm.physical(memory,2,at)?)?;
        }
        if queue::Counters::new(values).map_err(|_|EIO)?.slot().is_err() {return Ok((false,false));}
        if !self.independent_compute.can_stage(key,client,p,priority)
            || self.cleanup.require_idle().is_err() { return Ok((false,false)); }
        let (ready, notify) = self.independent_compute.prepare_reconfiguration(
            self.memory.as_mut().ok_or(EIO)?, self.vm.as_ref().ok_or(EIO)?,
            image.graph.channels[0][12], key, client, p, priority)?;
        if notify {
            self.peers[0].rtkit.as_mut().ok_or(EIO)?.as_mut()
                .send_message(0x21, 0x0084000000000011)?;
        }
        Ok((ready,!ready))
    }
    pub(crate) fn compute_preparation_seed(&self, key: super::g17p_compute_queues::Key,
        client: &compute::Client, p: &compute::Parameters, priority: u32, waits: &[(u8,u32)])
        -> Result<Option<super::g17p_compute_queues::PreparationSeed>> {
        self.independent_compute.preparation_seed(key, client, p, priority, waits)
    }
    pub(crate) fn compute_rebind_size(&self,key:super::g17p_compute_queues::Key,client:&compute::Client,
        p:&compute::Parameters,priority:u32) -> Option<usize> {
        self.independent_compute.rebind_size(key,client,p,priority)
    }
    pub(crate) fn capture_compute_rebind(&self,key:super::g17p_compute_queues::Key,client:&compute::Client,
        p:&compute::Parameters,priority:u32,storage:Option<super::g17p_user_vm::TableStorage>)
        -> Result<Option<Option<super::g17p_compute_queues::ComputeRebindSeed>>> {
        self.independent_compute.capture_rebind(key,client,p,priority,storage)
    }
    pub(crate) fn compute_rebind_matches(&self,plan:Option<&super::g17p_compute_queues::PreparedComputeRebind>,
        key:super::g17p_compute_queues::Key,client:&compute::Client,p:&compute::Parameters,priority:u32) -> bool {
        self.independent_compute.rebind_matches(plan,key,client,p,priority)
    }
    pub(crate) fn timestamp_cache_seed(&self, addresses: &[u64]) -> Result<super::g17p_timestamp::Cache> {
        self.timestamps.as_ref().ok_or(EIO)?.cache_seed(addresses)
    }
    pub(crate) fn retain_dependency(&mut self, point: (u8,u32), fence: &kernel::dma_fence::Fence) -> Result {
        let (grid, value) = point;
        if *crate::module_parameters::submission_log.value() >= 3 {
            let mut address = None;
            for producer in &self.independent_pending {
                if producer.ticket.grid == grid && producer.ticket.value == value {
                    address = Some(producer.ticket.status[0]);
                }
            }
            if let Some(PendingWork::Render(wave)) = &self.pending {
                for producer in &wave.frames[wave.next..] {
                    if producer.ticket.item.index + 1 == value
                        && producer.ticket.item.layout.grids.contains(&u32::from(grid)) {
                        address = Some(producer.ticket.firmware_completion);
                    }
                }
            }
            let status = if let Some(address) = address {
                let memory = self.memory.as_ref().ok_or(EIO)?;
                let vm = self.vm.as_ref().ok_or(EIO)?;
                let mut word = [0; 8];
                memory.read_firmware_words(vm.physical(memory, 2, address)?, &mut word)?;
                Some(u64::from_le_bytes(word))
            } else { None };
            pr_info!("G17P: FIRMWARE_DEPENDENCY_LEASE point {:?} producer_status {:?}\n", point, status);
        }
        if self.independent_compute.retain_dependency(grid, fence)? { return Ok(()); }
        for pool in 0..super::g17p_render_lifecycle::POOL_SLOTS {
            if super::g17p_render_lifecycle::pool_grids(pool).map_err(|_| EINVAL)?.contains(&u32::from(grid)) {
                if !self.render_cpu_leases.iter().any(|(held_pool, held)| *held_pool == pool && held.raw() == fence.raw()) {
                    self.render_cpu_leases.push((pool, fence.clone()), GFP_KERNEL)?;
                }
                return Ok(());
            }
        }
        Err(EINVAL)
    }
    pub(crate) fn compute_prepared_matches(&self, plan: &super::g17p_compute_queues::PreparedWork,
        key: super::g17p_compute_queues::Key, client: &compute::Client, p: &compute::Parameters,
        priority: u32, waits: &[(u8,u32)]) -> Result<bool> {
        self.independent_compute.prepared_matches(plan, key, client, p, priority, waits)
    }
    pub(crate) fn shared_backing_plan(&self) -> Result<[(usize,usize);2]> {
        let memory = self.memory.as_ref().ok_or(EIO)?;
        if self.phase != Phase::Running || !self.independent_compute_enabled() { return Ok([(0,0);2]); }
        let growth = super::g17p_growth::INCREMENT * super::g17p_growth::BLOCK as usize;
        let active = self.render.as_ref().or(self.dormant_render.as_ref())
            .and_then(|work| work.growth.as_ref()).map_or(0, |service|
                service.pools.iter().filter(|pool| !pool.retired).count());
        // Firmware transport backing is page-sized; new compute queue setup
        // uses fewer than64 pages. Growth replies need one contiguous Source
        // increment per live pool, replenished without device-wide locking.
        Ok([(0x4000,64usize.saturating_sub(memory.prepared_count(0x4000))),
            (growth,active.saturating_sub(memory.prepared_count(growth)))])
    }
    pub(crate) fn shared_backing_refused(&mut self) -> Result {
        let memory = self.memory.as_mut().ok_or(EIO)?;
        memory.prepare_growth_mode(); memory.refuse_growth(true); Ok(())
    }
    pub(crate) fn commit_shared_backing(&mut self, prepared: Memory) -> Result<Option<Memory>> {
        let needed = self.shared_backing_plan()?.iter().any(|(_,count)| *count != 0);
        let memory = self.memory.as_mut().ok_or(EIO)?;
        memory.prepare_growth_mode(); memory.refuse_growth(false);
        if needed { memory.absorb(prepared)?; Ok(None) } else { Ok(Some(prepared)) }
    }
    pub(crate) fn backing_waiting(&self) -> bool {
        self.memory.as_ref().is_some_and(|memory| memory.growth_waiting())
    }
    pub(crate) fn compute_needs_private(&self, key: super::g17p_compute_queues::Key,
        client: &compute::Client, p: &compute::Parameters, priority: u32) -> bool {
        self.independent_compute.needs_private(key, client, p, priority)
    }
    pub(crate) fn stage_independent_compute(&mut self,dev:&kernel::device::Device,image:&Image,
        key:super::g17p_compute_queues::Key,reference:&compute::Client,client:Option<compute::Client>,
        storage: Option<Memory>, rebind: Option<&mut super::g17p_compute_queues::PreparedComputeRebind>,
        prepared: KBox<super::g17p_compute_queues::PreparedWork>,
        p:&compute::Parameters,priority:u32,dependencies:&[(u8,u32)]) -> Result<Arc<ComputeReceipt>> {
        let private_prepared = storage.is_some();
        if let Some(storage) = storage { self.memory.as_mut().ok_or(EIO)?.absorb(storage)?; }
        let result=self.stage_independent_compute_inner(dev,image,key,reference,client,private_prepared,rebind,prepared,p,priority,dependencies);
        if let Err(error)=result.as_ref() {
            dev_err!(dev,"G17P: compute stage key {:?} failed {:?}\n",key,error);
            self.phase=Phase::Failed;self.fail_owned_receipts(*error);self.events.fail(*error);
        }
        result
    }
    fn stage_independent_compute_inner(&mut self,dev:&kernel::device::Device,image:&Image,
        key:super::g17p_compute_queues::Key,reference:&compute::Client,client:Option<compute::Client>,
        private_prepared: bool, rebind: Option<&mut super::g17p_compute_queues::PreparedComputeRebind>,
        prepared: KBox<super::g17p_compute_queues::PreparedWork>, p:&compute::Parameters,priority:u32,dependencies:&[(u8,u32)]) -> Result<Arc<ComputeReceipt>> {
        self.independent_pending.reserve(1,GFP_KERNEL)?;
        let ordinal=self.independent_count.checked_add(1).ok_or(EOVERFLOW)?;
        let receipt=ComputeReceipt::new(ordinal)?;
        // The exact parameters/aliases were made visible by off-lock
        // preparation before this validated descriptor plan was consumed.
        if self.phase==Phase::Prepared {
            self.bootstrap_compute(dev,image,reference.owner,p.preempt)?;
            // The primer is internal permanent backing, never an application
            // VM snapshot. Caller cleanup must visit only its real owners.
            self.compute.as_mut().ok_or(EIO)?.client.owner=(0,0);
        }
        if self.render.is_none() {
            let work=self.dormant_render.as_mut().ok_or(EIO)?;
            if work.growth.is_none() {
                work.growth=Some(super::g17p_growth_runtime::Service::new(
                    self.memory.as_ref().ok_or(EIO)?,self.vm.as_ref().ok_or(EIO)?,self.ttbs,
                    &work.client.root,image.graph.channels[0][12],image.graph.channels[0][13])?);
                configure_growth_limits(work.growth.as_mut().ok_or(EIO)?)?;
            }
        }
        self.service_owned_render_reports(dev,image)?;
        let ticket=self.independent_compute.stage(self.memory.as_mut().ok_or(EIO)?,
            self.vm.as_mut().ok_or(EIO)?,self.ttbs,image.graph.channels[0][queue::COMPUTE_CHANNEL],
            key,client,private_prepared,rebind,prepared,reference,p,priority,dependencies,receipt.fence.clone())?;
        for bo in ticket.client.buffers() {self.retain_source_backing(bo)?;}
        self.render.as_mut().or(self.dormant_render.as_mut()).ok_or(EIO)?.growth.as_mut().ok_or(EIO)?
            .register_independent_compute(self.independent_compute.grid_mask());
        let publication=ticket.publication;let grid=ticket.grid;let value=ticket.value;let bank=ticket.queue;
        // Observe both consumers before every producer store. Once a ticket
        // is consumed, retain that fact across later turns of the outer ring.
        for pending in &mut self.independent_pending {
            for i in 0..2 {
                pending.outer_done[i] |= queue::reached(pending.ticket.publication.consumers_before[i],
                    publication.consumers_before[i],pending.ticket.publication.producer);
            }
        }
        self.independent_pending.push(IndependentPending {ticket,receipt:receipt.clone(),
            started:kernel::time::Instant::now(),outer_done:[false;2]},GFP_KERNEL)?;
        self.independent_count=ordinal;
        let memory=self.memory.as_mut().ok_or(EIO)?;let vm=self.vm.as_ref().ok_or(EIO)?;
        if let Some((address,value))=publication.deferred_inner {vm.write(memory,2,address,&value.to_le_bytes())?;}
        g17p_memory::sync();
        let (address,producer)=publication.deferred_outer.ok_or(EIO)?;
        vm.write(memory,2,address,&producer.to_le_bytes())?;g17p_memory::sync();
        receipt.point.store((2u64<<40)|(u64::from(grid)<<32)|u64::from(value),Ordering::Relaxed);
        receipt.publication_ready.store(true,Ordering::Release);
        self.peers[0].rtkit.as_mut().ok_or(EIO)?.as_mut().send_message(0x21,queue::COMPUTE_DOORBELL)?;
        if *crate::module_parameters::submission_log.value()!=0 {
            dev_info!(dev,"G17P: independent compute {} queue {:?} bank {} grid {} value {} live {}\n",
                ordinal,key,bank,grid,value,self.independent_pending.len());
        }
        Ok(receipt)
    }
    fn poll_independent_compute(&mut self,dev:&kernel::device::Device,image:&Image)->Result<bool> {
        if self.independent_pending.is_empty() {return Ok(true);}
        if self.peers.iter().any(|p|p.data.crashed.load(Ordering::Acquire)) {return Err(EIO);}
        self.service_owned_render_reports(dev,image)?;
        let report=self.report_snapshot(image)?;
        if self.render.as_ref().or(self.dormant_render.as_ref()).ok_or(EIO)?.growth.as_ref().ok_or(EIO)?.cursor()!=report[0].firmware {
            return Ok(false);
        }
        // Both peers' report credits are returned only after classification.
        for (peer,r) in report.iter().enumerate() { for (offset,body) in r.records.iter().enumerate() {
            if u32::from_le_bytes(body[..4].try_into().unwrap())!=1 {
                dev_err!(dev,"G17P: compute unhandled report peer {} slot {} DATA {:02x?}\n",peer,(r.host+offset as u32)&255,body);
                return Err(EIO);
            }
        }}
        let memory=self.memory.as_ref().ok_or(EIO)?;let vm=self.vm.as_ref().ok_or(EIO)?;
        let mut index=0;
        while index<self.independent_pending.len() {
            let pending=&mut self.independent_pending[index];let ticket=&pending.ticket;
            let done=memory.read_firmware32(vm.physical(memory,2,ticket.pointers)?)?;
            let mut values=[0;3];
            for (v,at) in values.iter_mut().zip(ticket.channel.states) {*v=memory.read_firmware32(vm.physical(memory,2,at)?)?;}
            let _counters=queue::Counters::new(values).map_err(|_|EIO)?;
            for i in 0..2 {
                pending.outer_done[i] |= queue::reached(ticket.publication.consumers_before[i],
                    values[i] as u8,ticket.publication.producer);
            }
            let complete=done>=ticket.publication.write_after && pending.outer_done.iter().all(|v|*v);
            let terminals=self.render.as_ref().or(self.dormant_render.as_ref()).ok_or(EIO)?
                .growth.as_ref().ok_or(EIO)?.independent_compute_terminals(ticket.grid);
            let status=vm.physical(memory,2,ticket.status[1])?;memory.invalidate(status,8)?;
            if !complete || memory.read64(status)?==0 || terminals<ticket.value {
                if pending.started.elapsed().as_millis()>10000 {
                    dev_err!(dev,"G17P: independent compute timeout grid {} value {} inner {}/{} outer {:?} status {:#x} terminals {}\n",
                        ticket.grid,ticket.value,done,ticket.publication.write_after,values,memory.read64(status)?,terminals);
                    return Err(ETIMEDOUT);
                }
                index+=1;continue;
            }
            if self.defer_cache {
                self.completions.reserve(1, GFP_KERNEL)?;
                let stamps = self.timestamps.as_ref().ok_or(EIO)?.cache_seed(&ticket.timestamps)?;
                let pending = self.independent_pending.remove(index).map_err(|_| EIO)?;
                self.completions.push(Completion { client: pending.ticket.client, stamps,
                    fence: pending.receipt.fence.clone(), grid: pending.ticket.grid, error: None }, GFP_KERNEL)?;
            } else {
                ticket.client.cache(true)?;
                self.timestamps.as_ref().ok_or(EIO)?.cache(ticket.timestamps,true)?;
                pending.receipt.fence.signal();
                self.independent_pending.remove(index).map_err(|_|EIO)?;
            }
        }
        self.acknowledge_reports(image,&report)?;
        Ok(self.independent_pending.is_empty())
    }
}
