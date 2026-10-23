// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Single-ASC G16C startup inside the common scheduler/session owner.
//! No caller producer is exposed here. Full command emission remains separate.

use super::super::{
    g17p_render::admission::{FListReceiptG16C, RegistrationG16C},
    scheduled_bootstrap as boot, scheduled_image::ImageG16C, scheduled_initdata::FirmwareRole,
    scheduled_resources::ResourcesG16C,
};
use super::{g17p_memory, Data, Memory, Operations, Peer, Phase, Session, Vm};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use kernel::{
    c_str,
    device::Core,
    devres::Devres,
    io::{mem::IoMem, Io},
    platform,
    prelude::*,
    soc::apple::rtkit,
    sync::Arc,
};

impl Session {
    pub(crate) fn destroy_g16c_vm(&mut self, owner: (u64, u32)) -> Result {
        self.release_g16c_vm(owner)
    }

    pub(crate) fn create_g16c_queues(
        &mut self,
        base: u64,
        priority: u8,
        uuid: u16,
        scratch_aliases: Option<[u64; 3]>,
    ) -> Result<super::super::scheduled_queues::QueueSetG16C> {
        if self.phase != Phase::Running { return Err(EBUSY); }
        let queues = super::super::scheduled_queues::QueueSetG16C::new(
            self.memory.as_mut().ok_or(EINVAL)?, self.vm.as_mut().ok_or(EINVAL)?,
            base, priority, uuid, scratch_aliases,
        )?;
        Vm::invalidate_gpu();
        Ok(queues)
    }

    pub(crate) fn submit_g16c(
        &mut self,
        requests: &mut [super::super::scheduled_transport::Request<'_>],
        requirements: super::super::scheduled_transport::Requirements,
        owners: KVec<super::compute::ClientLease>,
    ) -> Result<u64> {
        if self.phase != Phase::Running { return Err(EIO); }
        let Self { memory,vm,peers,g16c_transport,.. } = self;
        let state = g16c_transport.as_mut().ok_or(EINVAL)?;
        let result = state.submit(memory.as_mut().ok_or(EINVAL)?,vm.as_ref().ok_or(EINVAL)?,
            requests,requirements,owners,|endpoint,message| {
                peers[0].rtkit.as_mut().ok_or(EIO)?.as_mut().send_message(endpoint,message)
            });
        if state.poisoned() {
            self.phase=Phase::Failed;
            let error=result.as_ref().err().copied().unwrap_or(EIO);
            self.submission_error=Some(error);
            self.events.fail(error);
        }
        result
    }

    pub(super) fn prepare_g16c_submission(
        &mut self, requests: &mut [super::super::scheduled_transport::Request<'_>],
        requirements: super::super::scheduled_transport::Requirements,
        owners: KVec<super::compute::ClientLease>,
    ) -> Result<super::super::scheduled_transport::PreparedG16C> {
        if self.phase != Phase::Running { return Err(EIO); }
        self.g16c_transport.as_mut().ok_or(EINVAL)?.prepare(
            self.memory.as_mut().ok_or(EINVAL)?, self.vm.as_ref().ok_or(EINVAL)?,
            requests, requirements, owners)
    }

    pub(super) fn publish_g16c_submission(
        &mut self, requests: &mut [super::super::scheduled_transport::Request<'_>],
        prepared: super::super::scheduled_transport::PreparedG16C,
    ) -> Result<u64> {
        if self.phase != Phase::Running { return Err(EIO); }
        let Self { memory,vm,peers,g16c_transport,.. } = self;
        let state = g16c_transport.as_mut().ok_or(EINVAL)?;
        let result = state.publish(memory.as_mut().ok_or(EINVAL)?,vm.as_ref().ok_or(EINVAL)?,
            requests,prepared,|endpoint,message| {
                peers[0].rtkit.as_mut().ok_or(EIO)?.as_mut().send_message(endpoint,message)
            });
        if state.poisoned() {
            self.phase=Phase::Failed;
            let error=result.as_ref().err().copied().unwrap_or(EIO);
            self.submission_error=Some(error);
            self.events.fail(error);
        }
        result
    }

    pub(crate) fn poll_g16c(&mut self,id:u64)->Result<bool> {
        self.g16c_transport.as_mut().ok_or(EINVAL)?.poll(
            self.memory.as_ref().ok_or(EINVAL)?,self.vm.as_ref().ok_or(EINVAL)?,id)
    }

    pub(crate) fn release_g16c(&mut self,id:u64)->Result {
        self.g16c_transport.as_mut().ok_or(EINVAL)?.release(
            self.memory.as_ref().ok_or(EINVAL)?,self.vm.as_ref().ok_or(EINVAL)?,id)
    }

    pub(crate) fn prepare_g16c(
        pdev: &platform::Device<Core>,
        resources: &ResourcesG16C,
        sgx: Pin<KBox<Devres<IoMem<0x3758000>>>>,
        image: &ImageG16C,
    ) -> Result<KBox<Self>> {
        let dev = pdev.as_ref();
        // Validate cold state before allocating or changing any UAT backing.
        let asc = KBox::pin_init(
            pdev.io_request_by_name(c_str!("asc"))
                .ok_or(ENODEV)?
                .iomap_sized::<0x4000>(),
            GFP_KERNEL,
        )?;
        let control = {
            let regs = asc.try_access().ok_or(ENODEV)?;
            let control = regs.read32(0x44);
            let status = regs.read32(0x48);
            if control & 0x10 != 0 || status & 3 != 2 {
                return Err(EBUSY);
            }
            control
        };
        {
            let regs = sgx.try_access().ok_or(ENODEV)?;
            if regs.read32(0xd04000) != 0x0a041100 {
                return Err(ENODEV);
            }
        }
        let events = super::super::g17p_drm::asynchronous::Events::new()?;
        let mut session = Self::unstarted(
            Memory::new_g16c(dev, resources)?,
            resources.regions[0].base,
            events.clone(),
        )?;
        session.g16c_sgx = Some(sgx);
        let data = Arc::new(
            Data {
                dev: pdev.into(),
                role: FirmwareRole::Primary,
                name: c_str!("primary"),
                firmware_region: c_str!("firmware"),
                crashed: AtomicBool::new(false),
                last_message: AtomicU64::new(0),
                acknowledged: AtomicBool::new(false),
                wakeups: AtomicU64::new(0),
                events,
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
        {
            let Self { memory, vm, g16c_sgx, .. } = &mut *session;
            let memory = memory.as_mut().ok_or(EINVAL)?;
            *vm = Some(Vm::prepare_g16c(memory, resources)?);
            let regs = g16c_sgx.as_ref().ok_or(EINVAL)?.try_access().ok_or(ENODEV)?;
            for offset in [0x1000104, 0x1000108] {
                regs.write32(regs.read32(offset) | 1, offset);
            }
            regs.write32(6, 0xd06030);
            memory.prepare_cold_g16c(resources)?;
            vm.as_ref().ok_or(EINVAL)?.flush_tables(memory)?;
            Vm::invalidate_gpu();
        }
        let mut rtkit = KBox::pin(
            rtkit::RtKit::<Operations>::new(
                dev,
                Some(c_str!("primary")),
                0,
                session.peers[0].data.clone(),
            )?,
            GFP_KERNEL,
        )?;
        rtkit.as_mut().set_early_crashlog();
        session.peers[0].rtkit = Some(rtkit);
        session.phase = Phase::Starting;
        {
            let Self { peers, g16c_sgx, .. } = &mut *session;
            let peer = &mut peers[0];
            let asc = peer.asc.try_access().ok_or(ENODEV)?;
            let gpu = g16c_sgx.as_ref().ok_or(EINVAL)?.try_access().ok_or(ENODEV)?;
            let asc_address = asc.io_addr::<u32>(0x44)?;
            let gate_address = gpu.io_addr::<u32>(0xd06030)?;
            peer.started = true;
            // SAFETY: Both checked, live MMIO mappings remain borrowed. The
            // source-owned two-store sequence releases the gate immediately
            // after ASC RUN, before any RTKit negotiation or host scheduling.
            unsafe {
                core::arch::asm!(
                "str {control:w}, [{asc}]", "str wzr, [{gate}]", "dsb sy",
                control=in(reg)(control|0x10),asc=in(reg)asc_address,gate=in(reg)gate_address,
                options(nostack,preserves_flags));
            }
        }
        {
            let peer = &mut session.peers[0];
            let rtkit = peer.rtkit.as_mut().ok_or(EINVAL)?;
            rtkit.as_mut().boot().inspect_err(|error| {
                dev_err!(dev, "G16C: RTKit management boot failed: {:?}\n", error);
            })?;
            if peer.data.crashed.load(Ordering::Acquire) {
                return Err(EIO);
            }
            for endpoint in [0x20, 0x21] {
                if !rtkit.as_mut().has_endpoint(endpoint) {
                    return Err(ENODEV);
                }
                rtkit.as_mut().start_endpoint(endpoint)?;
            }
        }
        dev_info!(dev, "G16C: RTKit ready; constructing initdata mappings\n");
        {
            let Self { memory, vm, .. } = &mut *session;
            let memory = memory.as_mut().ok_or(EINVAL)?;
            let vm = vm.as_mut().ok_or(EINVAL)?;
            vm.refresh_g16c_boot_tables(memory, resources)?;
            vm.populate_g16c(memory, image).inspect_err(|error| {
                dev_err!(dev, "G16C: initdata mapping failed: {:?}\n", error);
            })?;
            boot::prepare_publication(memory, vm, resources.regions[0].base)
                .inspect_err(|error| {
                    dev_err!(dev, "G16C: root preparation failed: {:?}\n", error);
                })?;
            boot::drain(memory, vm)?;
        }
        dev_info!(dev, "G16C: publishing initdata root\n");
        session.g16c_send(0x20, boot::ROOT_MESSAGE)?;
        session.g16c_wait(boot::accepted)?;
        {
            let Self { memory, vm, .. } = &mut *session;
            boot::prepare_control(memory.as_mut().ok_or(EINVAL)?, vm.as_ref().ok_or(EINVAL)?)?;
        }
        session.g16c_send(0x21, boot::CONTROL_MESSAGE)?;
        session.g16c_wait(|memory, vm| boot::control_retired(memory, vm, 1))?;
        {
            let Self { memory, vm, .. } = &mut *session;
            boot::prepare_maintenance(memory.as_mut().ok_or(EINVAL)?, vm.as_ref().ok_or(EINVAL)?)?;
        }
        session.g16c_send(0x21, boot::CONTROL_MESSAGE)?;
        session.g16c_wait(|memory, vm| boot::control_retired(memory, vm, 7))?;
        session.g16c_transport = Some(KBox::new(
            super::super::scheduled_transport::StateG16C::new(&image.graph.channels[..12])?,
            GFP_KERNEL,
        )?);
        session.phase = Phase::Running;
        session.g16c_compute = Some(KBox::new(super::g16c_compute::State::new(), GFP_KERNEL)?);
        dev_info!(dev,"G16C: single-ASC initdata accepted and opening controls retired; native command emission pending\n");
        Ok(session)
    }

    pub(super) fn g16c_send(&mut self, endpoint: u8, message: u64) -> Result {
        g17p_memory::sync();
        self.peers[0]
            .rtkit
            .as_mut()
            .ok_or(EINVAL)?
            .as_mut()
            .send_message(endpoint, message)
    }

    pub(super) fn g16c_wait(&mut self, predicate: impl Fn(&Memory, &Vm) -> Result<bool>) -> Result {
        self.g16c_wait_receipt(predicate, None, false)
    }

    /// The caller retains every operand/root owner before this transaction.
    /// A receive match and all TX cursors are independently required.
    pub(super) fn g16c_register_flist(&mut self, registration: &RegistrationG16C) -> Result {
        if self.phase != Phase::Running { return Err(EIO); }
        let (request, mut receipt) = registration.prepare().map_err(|_| EINVAL)?;
        let result = (|| {
            let target = boot::publish_control(self.memory.as_mut().ok_or(EIO)?,
                self.vm.as_ref().ok_or(EIO)?, request.bytes())?;
            self.g16c_send(0x21, boot::CONTROL_MESSAGE)?;
            // Live compute operand extension follows the source shim's
            // retirement transaction: notify control, then wake the work loop.
            if registration.mode==0 {self.g16c_send(0x21,0x0083000000000010)?;}
            self.g16c_wait_receipt(|memory, vm| boot::control_retired(memory, vm, target),
                Some(&mut receipt), false)
        })();
        if let Err(error) = result { self.fail_cpu_completion(error); }
        result
    }

    pub(super) fn g16c_wait_work_control(&mut self, target:u32)->Result {
        self.g16c_wait_receipt(|memory,vm|boot::control_retired(memory,vm,target),None,true)
    }

    pub(super) fn g16c_wait_growth_condition(&mut self, predicate:impl Fn(&Memory,&Vm)->Result<bool>)->Result {
        self.g16c_wait_receipt(predicate,None,true)
    }

    fn g16c_wait_receipt(&mut self, predicate: impl Fn(&Memory, &Vm) -> Result<bool>,
                        mut receipt: Option<&mut FListReceiptG16C>, defer_work:bool) -> Result {
        let start = kernel::time::Instant::<kernel::time::Monotonic>::now();
        loop {
            if self.peers[0].data.crashed.load(Ordering::Acquire) {
                return Err(EIO);
            }
            let memory = self.memory.as_mut().ok_or(EINVAL)?;
            let vm = self.vm.as_ref().ok_or(EINVAL)?;
            if defer_work {
                // A following owned work event is handled by the outer runtime,
                // after the current reply is fully retired. Do not ACK here.
                boot::drain_with_work_event(memory,vm,&mut |_,_,_|Ok(false))?;
            } else {
                boot::drain_with_flist(memory, vm, receipt.as_deref_mut())?;
            }
            if predicate(memory, vm)? && receipt.as_ref().is_none_or(|r| r.received) {
                return Ok(());
            }
            if start.elapsed().as_millis() >= 10000 {
                return Err(ETIMEDOUT);
            }
            kernel::time::delay::fsleep(kernel::time::Delta::from_millis(1));
        }
    }
}
