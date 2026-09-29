// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Dual-ASC management boot for the synchronous G17P shim port.
//! Both transports exist before either CPU starts. Descriptor publication is
//! separate: successful RTKit boot is not proof that GPU work can execute.

use super::{
    g17p_memory::{self, Memory},
    g17p_platform::Platform,
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

pub(crate) struct Session {
    peers: KVec<Peer>,
    memory: Option<Memory>,
}

impl Session {
    pub(crate) fn new(
        pdev: &platform::Device<Core>,
        platform: &Platform,
        sgx: &Devres<IoMem<0x4000000>>,
    ) -> Result<Self> {
        let dev = pdev.as_ref();
        let mut session = Self {
            peers: KVec::new(),
            memory: Some(Memory::new(dev, platform)?),
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
        dev_info!(
            dev,
            "G17P: both firmware instances booted; initdata roots not published yet\n"
        );
        Ok(session)
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
        }
    }
}
