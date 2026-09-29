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
    io::{self, mem::IoMem, Io},
    iosys_map::IoSysMapRef,
    platform,
    prelude::*,
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
    compute: Option<compute::Submission>,
    render: Option<render::Submission>,
    dormant_render: Option<render::Submission>,
    timestamps: Option<super::g17p_timestamp::Registry>,
    peers: KVec<Peer>,
    memory: Option<Memory>,
    vm: Option<Vm>,
}

struct Report {
    host: u32,
    firmware: u32,
    records: KVec<[u8; 0x48]>,
    peer_credits: [u32; 2],
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
    pub(crate) fn prepare(
        pdev: &platform::Device<Core>,
        platform: &Platform,
        sgx: &Devres<IoMem<0x4000000>>,
        image: &Image,
    ) -> Result<Self> {
        let dev = pdev.as_ref();
        let mut session = Self {
            phase: Phase::Prepared,
            ttbs: platform.regions[0].base,
            compute: None,
            render: None,
            dormant_render: None,
            timestamps: Some(super::g17p_timestamp::Registry::new()),
            peers: KVec::new(),
            memory: Some(Memory::new(dev, platform)?),
            vm: None,
        };
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
        let memory = session.memory.as_mut().ok_or(EINVAL)?;
        memory.write_handoff(platform)?;
        for (index, peer) in session.peers.iter().enumerate() {
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
        session.vm = Some(Vm::build(dev, memory, platform, image)?);
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

    pub(crate) fn compute_client(&self) -> Result<Option<&compute::Client>> {
        if self.phase == Phase::Failed {
            return Err(EIO);
        }
        if self.phase != Phase::Prepared && self.phase != Phase::Running {
            return Err(EBUSY);
        }
        Ok(self.compute.as_ref().map(|work| &work.client))
    }

    pub(crate) fn require_compute_owner(&self, owner: (u64, u32)) -> Result {
        if self
            .render
            .as_ref()
            .is_some_and(|work| work.client.owner != owner)
        {
            return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
        }
        Ok(())
    }

    pub(crate) fn compute_remaining(&self) -> Result<usize> {
        self.compute_client()?;
        Ok(self.compute.as_ref().map_or(
            if self.render.is_some() {
                2
            } else {
                compute::SUBMISSIONS as usize
            },
            |work| (work.capacity() - work.ordinal - 1) as usize,
        ))
    }

    pub(crate) fn submit_next_compute(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        replacement: Option<compute::Client>,
        parameters: &compute::Parameters,
    ) -> Result {
        if self.phase != Phase::Running {
            return Err(EIO);
        }
        let work = self.compute.as_mut().ok_or(EINVAL)?;
        if work.ordinal + 1 >= work.capacity() || work.preempt != parameters.preempt {
            return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
        }
        if let Some(client) = replacement {
            compute::idle(
                self.memory.as_ref().ok_or(EINVAL)?,
                self.vm.as_ref().ok_or(EINVAL)?,
                work,
            )?;
            let previous_owner = work.client.owner;
            // The source's compute mirror uses this same low root in native
            // slots 2/3. Flush both ASIDs before releasing the old GEMs.
            work.client.rebind(client, false)?;
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
        let old_pointers = work.pointers;
        let staged = compute::stage_next(
            self.memory.as_mut().ok_or(EINVAL)?,
            self.vm.as_mut().ok_or(EINVAL)?,
            work,
            parameters,
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
        let result = staged.and_then(|()| self.run_compute(dev, image));
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result
    }

    pub(crate) fn submit_compute(
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
        let work = built.inspect_err(|error| {
            dev_err!(
                dev,
                "G17P: first compute graph preparation failed: {:?}\n",
                error
            );
        })?;
        // Ownership precedes the first mailbox publication. Even an initdata
        // timeout or firmware error keeps every reachable client page pinned.
        self.compute = Some(work);
        let result = self.run_compute(dev, image);
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result
    }

    pub(crate) fn render_client(&self) -> Result<Option<&compute::Client>> {
        if self.phase == Phase::Failed {
            return Err(EIO);
        }
        Ok(self.render.as_ref().map(|work| &work.client))
    }
    pub(crate) fn render_remaining(&self) -> Result<u32> {
        self.render_client()?;
        Ok(self
            .render
            .as_ref()
            .map_or(super::g17p_render_lifecycle::OWNER_SUBMISSIONS, |work| {
                super::g17p_render_lifecycle::OWNER_SUBMISSIONS - work.ordinal - 1
            }))
    }
    pub(crate) fn submit_next_render(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        replacement: Option<compute::Client>,
        p: &super::g17p_render::Parameters,
    ) -> Result {
        if self.phase != Phase::Running {
            return Err(EIO);
        }
        if let Some(client) = replacement {
            render::validate_client(&client, p)?;
            self.render
                .as_mut()
                .ok_or(EINVAL)?
                .client
                .rebind(client, true)?;
            dev_info!(dev, "G17P: retained render caller mappings refreshed\n");
        }
        let ordinal = self.render.as_ref().ok_or(EINVAL)?.ordinal + 1;
        if ordinal >= super::g17p_render_lifecycle::OWNER_SUBMISSIONS {
            return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
        }
        let result = (|| {
            self.announce_render(dev, image, ordinal)?;
            let work = self.render.as_mut().ok_or(EINVAL)?;
            render::stage_next(
                self.memory.as_mut().ok_or(EINVAL)?,
                self.vm.as_ref().ok_or(EINVAL)?,
                work,
                p,
            )
            .and_then(|()| self.run_render(dev, image))
        })();
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result
    }

    fn announce_render(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        ordinal: u32,
    ) -> Result {
        use super::g17p_render_lifecycle as life;
        if ordinal < 2 {
            return Ok(());
        }
        let body = life::control_tick(ordinal).map_err(|_| EINVAL)?;
        let channel = image.graph.channels[0][12];
        let memory = self.memory.as_mut().ok_or(EINVAL)?;
        let vm = self.vm.as_ref().ok_or(EINVAL)?;
        let mut before = [0; 3];
        for (value, at) in before.iter_mut().zip(channel.states) {
            *value = memory.read_firmware32(vm.physical(memory, 2, at)?)?;
        }
        if before[2] >= 255 || before[0] != before[2] || before[1] != before[2] {
            return Err(EBUSY);
        }
        if ordinal >= 3 {
            vm.write(
                memory,
                2,
                super::g17p_opening::SUPPORT + 0x20,
                &life::control_prestate(),
            )?;
        }
        vm.write(memory, 2, channel.ring + before[2] as u64 * 0x40, &body)?;
        g17p_memory::sync();
        let target = before[2] + 1;
        vm.write(memory, 2, channel.states[2], &target.to_le_bytes())?;
        g17p_memory::sync();
        for _ in 0..100 {
            if self
                .peers
                .iter()
                .any(|p| p.data.crashed.load(Ordering::Acquire))
            {
                return Err(EIO);
            }
            self.peers[0]
                .rtkit
                .as_mut()
                .ok_or(EINVAL)?
                .as_mut()
                .send_message(0x21, 0x0084000000000011)?;
            let consumer = memory.read_firmware32(vm.physical(memory, 2, channel.states[0])?)?;
            if consumer >= target {
                dev_info!(
                    dev,
                    "G17P: render {} control tick {} consumed at slot {}\n",
                    ordinal,
                    ordinal - 1,
                    before[2]
                );
                return Ok(());
            }
            kernel::time::delay::fsleep(kernel::time::Delta::from_millis(1));
        }
        dev_err!(
            dev,
            "G17P: render {} control tick not consumed; retaining graph\n",
            ordinal
        );
        Err(ETIMEDOUT)
    }

    pub(crate) fn submit_render(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        client: compute::Client,
        parameters: &super::g17p_render::Parameters,
    ) -> Result {
        let work = if let Some(compute) = self.compute.as_ref() {
            if self.phase != Phase::Running || compute.client.owner != client.owner {
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
        let result = self.run_render(dev, image);
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result
    }

    fn run_render(&mut self, dev: &kernel::device::Device, image: &Image) -> Result {
        let stamps = self.render.as_ref().ok_or(EINVAL)?.timestamps;
        for pair in stamps.chunks_exact(2) {
            self.timestamps
                .as_ref()
                .ok_or(EINVAL)?
                .cache([pair[0], pair[1]], false)?;
        }
        if self.phase == Phase::Prepared {
            self.start(dev, image)?;
        } else if self.phase != Phase::Running {
            return Err(EIO);
        }
        let startup = self.report_snapshot(image)?;
        let terminal_baseline;
        {
            let work = self.render.as_mut().ok_or(EINVAL)?;
            let vm = self.vm.as_ref().ok_or(EINVAL)?;
            let memory = self.memory.as_mut().ok_or(EINVAL)?;
            if work.ordinal == 0 {
                work.after_control(memory, vm, self.ttbs)?;
                work.growth = Some(super::g17p_growth_runtime::Service::new(
                    memory,
                    vm,
                    self.ttbs,
                    work.client.root.root(),
                    image.graph.channels[0][12],
                    image.graph.channels[0][13],
                )?);
            }
            // wait_pair_completed() requires this publication's new growth
            // terminal in addition to both queues and independent statuses.
            // Capture before either work producer is restored; a drained ring
            // alone cannot establish that firmware has issued this terminal.
            terminal_baseline = work.growth.as_ref().ok_or(EINVAL)?.terminals();
            work.restore(memory, vm, 1)?;
            work.restore(memory, vm, 0)?;
        }
        self.peers[0]
            .rtkit
            .as_mut()
            .ok_or(EINVAL)?
            .as_mut()
            .send_message(0x21, 0x0083000000000008)?;
        dev_info!(
            dev,
            "G17P: caller render published on TA2/3D2, client root {:#x}\n",
            self.render.as_ref().ok_or(EINVAL)?.client.root.root()
        );
        let mut last = [[0u32; 6]; 2];
        let mut status_changed = [false; 2];
        let mut command_status = 0;
        for _ in 0..500 {
            if self
                .peers
                .iter()
                .any(|p| p.data.crashed.load(Ordering::Acquire))
            {
                return Err(EIO);
            }
            // A bounded prefix handles coalesced growth notifications while
            // retaining the runtime lock and the sole active root owner.
            for _ in 0..32 {
                use super::g17p_growth_runtime::Action;
                let work = self.render.as_mut().ok_or(EINVAL)?;
                let action = work.growth.as_mut().ok_or(EINVAL)?.step(
                    self.memory.as_mut().ok_or(EINVAL)?,
                    self.vm.as_ref().ok_or(EINVAL)?,
                    &mut work.client.root,
                    self.ttbs,
                    None,
                )?;
                match action {
                    Action::Idle => break,
                    Action::Consumed => (),
                    Action::Limit => {
                        dev_err!(
                            dev,
                            "G17P: owned render memory limit consumed; retaining graph\n"
                        );
                        return Err(ENOMEM);
                    }
                    Action::Reply {
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
                            "G17P: TVB growth reply {} pool 0 VM 1: {} -> {} blocks, refused={}\n",
                            counter,
                            old,
                            new,
                            refused
                        );
                    }
                }
            }
            let work = self.render.as_ref().ok_or(EINVAL)?;
            let memory = self.memory.as_ref().ok_or(EINVAL)?;
            let vm = self.vm.as_ref().ok_or(EINVAL)?;
            let mut done = work.growth.as_ref().ok_or(EINVAL)?.terminals() > terminal_baseline;
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
                        render::POINTERS[stage] + offset,
                    )?)?;
                }
                for (i, address) in work.channels[stage].states.into_iter().enumerate() {
                    last[stage][i + 3] =
                        memory.read_firmware32(vm.physical(memory, 2, address)?)?;
                }
                let counters =
                    queue::Counters::new([last[stage][3], last[stage][4], last[stage][5]])
                        .map_err(|_| EIO)?;
                done &= work.publications[stage].completed(last[stage][0], counters);
                // Match the shim's two independent 0x40-byte status gates.
                // build() initializes these retained private records to zero.
                let pa = vm.physical(
                    memory,
                    2,
                    render::STATUS[stage] + work.ordinal as u64 * 0x40,
                )?;
                memory.invalidate(pa, 0x40)?;
                status_changed[stage] = false;
                for offset in (0..0x40).step_by(8) {
                    status_changed[stage] |= memory.read64(pa + offset)? != 0;
                }
                done &= status_changed[stage];
            }
            let after = self.report_snapshot(image)?;
            for (peer, (report, before)) in after.iter().zip(startup.iter()).enumerate() {
                // The growth service is the primary report reader. If a new
                // record arrived after its drain, service it next iteration;
                // never ACK it from this completion snapshot.
                if peer == 0 && report.firmware != work.growth.as_ref().ok_or(EINVAL)?.cursor() {
                    done = false;
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
            let pa = vm.physical(memory, 2, 0xfffffc2000024c70)?;
            memory.invalidate(pa, 8)?;
            command_status = memory.read64(pa)?;
            if done && command_status != 0 {
                work.client.cache(true)?;
                for pair in stamps.chunks_exact(2) {
                    self.timestamps
                        .as_ref()
                        .ok_or(EINVAL)?
                        .cache([pair[0], pair[1]], true)?;
                }
                dev_info!(dev,"G17P: caller render complete: TA {:?}, 3D {:?}, status {:#x}; reports validated, terminals {} cursor {}\n",last[0],last[1],command_status,work.growth.as_ref().ok_or(EINVAL)?.terminals(),work.growth.as_ref().ok_or(EINVAL)?.cursor());
                self.acknowledge_reports(image, &after)?;
                // Same control-done boundary as the source synchronous shim.
                self.peers[0]
                    .rtkit
                    .as_mut()
                    .ok_or(EINVAL)?
                    .as_mut()
                    .send_message(0x21, 0x0084000000000011)?;
                if render::quiesce(
                    self.memory.as_mut().ok_or(EINVAL)?,
                    self.vm.as_ref().ok_or(EINVAL)?,
                    self.render.as_ref().ok_or(EINVAL)?,
                )? {
                    dev_info!(dev, "G17P: completed render scheduler list quiesced\n");
                }
                return Ok(());
            }
            kernel::time::delay::fsleep(kernel::time::Delta::from_millis(10));
        }
        dev_err!(
            dev,
            "G17P: caller render timeout: TA {:?}, 3D {:?}, status {:#x}, changed {:?}; retaining graph\n",
            last[0],
            last[1],
            command_status,
            status_changed
        );
        Err(ETIMEDOUT)
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

    fn run_compute(&mut self, dev: &kernel::device::Device, image: &Image) -> Result {
        let timestamps = self.compute.as_ref().ok_or(EINVAL)?.timestamps;
        self.timestamps
            .as_ref()
            .ok_or(EINVAL)?
            .cache(timestamps, false)?;
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
        let startup = self.report_snapshot(image)?;
        let work = self.compute.as_ref().ok_or(EINVAL)?;
        if let Some(render) = self.render.as_mut() {
            render
                .growth
                .as_mut()
                .ok_or(EINVAL)?
                .begin_compute(work.ordinal)?;
        }
        let vm = self.vm.as_ref().ok_or(EINVAL)?;
        let memory = self.memory.as_mut().ok_or(EINVAL)?;
        if let Some((address, value)) = work.publication.deferred_inner {
            vm.write(memory, 2, address, &value.to_le_bytes())?;
            g17p_memory::sync();
        }
        let (address, value) = work.publication.deferred_outer.ok_or(EINVAL)?;
        vm.write(memory, 2, address, &value.to_le_bytes())?;
        g17p_memory::sync();
        self.peers[0]
            .rtkit
            .as_mut()
            .ok_or(EINVAL)?
            .as_mut()
            .send_message(0x21, queue::COMPUTE_DOORBELL)?;
        dev_info!(
            dev,
            "G17P: caller compute {} published on CL2, client root {:#x}\n",
            work.ordinal,
            work.client.root.root()
        );
        let mut last = [0u32; 6];
        let mut command_status = 0;
        for _ in 0..200 {
            if self
                .peers
                .iter()
                .any(|p| p.data.crashed.load(Ordering::Acquire))
            {
                return Err(EIO);
            }
            if let Some(render) = self.render.as_mut() {
                for _ in 0..32 {
                    use super::g17p_growth_runtime::Action;
                    let action = render.growth.as_mut().ok_or(EINVAL)?.step(
                        self.memory.as_mut().ok_or(EINVAL)?,
                        vm,
                        &mut render.client.root,
                        self.ttbs,
                        Some(self.compute.as_ref().ok_or(EINVAL)?.ordinal),
                    )?;
                    match action {
                        Action::Idle => break,
                        Action::Consumed => (),
                        // No render is live during this serialized compute
                        // publication; a new growth request cannot be attributed.
                        _ => return Err(EIO),
                    }
                }
            }
            let work = self.compute.as_ref().ok_or(EINVAL)?;
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
                let after = self.report_snapshot(image)?;
                if self.render.as_ref().is_some_and(|render| {
                    render
                        .growth
                        .as_ref()
                        .is_none_or(|service| service.cursor() != after[0].firmware)
                }) {
                    continue;
                }
                for (peer, (report, before)) in after.iter().zip(startup.iter()).enumerate() {
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
                    kernel::time::delay::fsleep(kernel::time::Delta::from_millis(10));
                    continue;
                }
                work.client.cache(true)?;
                self.timestamps
                    .as_ref()
                    .ok_or(EINVAL)?
                    .cache(timestamps, true)?;
                dev_info!(
                    dev,
                    "G17P: caller compute complete: queue {:?}, channel {:?}, status {:#x}; reports validated\n",
                    &last[..3],
                    &last[3..], command_status
                );
                self.acknowledge_reports(image, &after)?;
                if let Some(render) = &mut self.render {
                    let service = render.growth.as_mut().ok_or(EINVAL)?;
                    service.finish_compute();
                    dev_info!(dev, "G17P: mixed reports: render terminals {}, compute terminals {}, cursor {}\n",
                        service.terminals(), service.compute_terminals(), service.cursor());
                }
                return Ok(());
            }
            kernel::time::delay::fsleep(kernel::time::Delta::from_millis(10));
        }
        dev_err!(
            dev,
            "G17P: caller compute timeout: queue {:?}, channel {:?}, status {:#x}; retaining all GPU memory\n",
            &last[..3],
            &last[3..], command_status
        );
        Err(ETIMEDOUT)
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
            core::mem::forget(self.render.take());
            core::mem::forget(self.dormant_render.take());
            core::mem::forget(self.timestamps.take());
        }
    }
}
