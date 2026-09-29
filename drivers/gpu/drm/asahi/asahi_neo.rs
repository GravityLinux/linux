// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! T8140 / G17P firmware startup and native DRM memory support.
//!
//! Validates boot resources, constructs the unpublished firmware graph and
//! boots both RTKit instances and registers the native Asahi memory UAPI.

use kernel::{
    c_str, device::Core, devres::Devres, io::mem::IoMem, io::Io, of, platform, prelude::*,
};

mod g17p_abi;
mod g17p_boot;
// Current source serializers, connected as the submission port is completed.
#[allow(dead_code)]
mod g17p_compute;
#[allow(dead_code)]
mod g17p_compute_memory;
mod g17p_compute_runtime;
#[allow(dead_code)]
mod g17p_compute_lifecycle;
mod g17p_drm;
mod g17p_image;
mod g17p_initgraph;
mod g17p_layout;
mod g17p_memory;
mod g17p_opening;
mod g17p_platform;
mod g17p_user_vm;
// Core queue port is exercised by source differential tests until submit is wired.
#[allow(dead_code)]
mod g17p_queue;
mod g17p_sync;
mod g17p_timestamp;
mod g17p_topology;
mod g17p_vm;

const SGX_SIZE: usize = 0x4000000;
const ID_VERSION: usize = 0xd04000;
const ID_COUNTS_1: usize = 0xd04010;
const ID_COUNTS_2: usize = 0xd04014;
const ID_CLUSTERS: usize = 0xd0401c;

struct NeoGpu {
    _sgx: Pin<KBox<Devres<IoMem<SGX_SIZE>>>>,
    _platform: g17p_platform::Platform,
    runtime: g17p_drm::RuntimeRef,
    _drm: kernel::sync::aref::ARef<kernel::drm::Device<g17p_drm::Driver>>,
}

impl Drop for NeoGpu {
    fn drop(&mut self) {
        // Serialize with ioctls and stop firmware while devres still owns the
        // mailbox/MMIO resources. Existing DRM files subsequently see ENODEV.
        let runtime = self.runtime.lock().take();
        drop(runtime);
    }
}

kernel::of_device_table!(
    OF_TABLE,
    MODULE_OF_TABLE,
    <NeoGpu as platform::Driver>::IdInfo,
    [(of::DeviceId::new(c_str!("apple,agx-t8140")), ())]
);

impl platform::Driver for NeoGpu {
    type IdInfo = ();
    const OF_ID_TABLE: Option<of::IdTable<Self::IdInfo>> = Some(&OF_TABLE);

    fn probe(
        pdev: &platform::Device<Core>,
        _info: Option<&Self::IdInfo>,
    ) -> impl PinInit<Self, Error> {
        let request = pdev.io_request_by_name(c_str!("sgx")).ok_or(EINVAL)?;
        let sgx = KBox::pin_init(request.iomap_sized::<SGX_SIZE>(), GFP_KERNEL)?;
        let (version, counts, mask) = {
            let regs = sgx.try_access().ok_or(ENODEV)?;
            let version = regs.read32(ID_VERSION);
            if version == 0 || version == u32::MAX {
                dev_err!(pdev.as_ref(), "Invalid GPU ID {:#010x}\n", version);
                return Err(ENODEV);
            }
            dev_info!(
                pdev.as_ref(),
                "Apple A18 Pro GPU recognized (T8140/G17P): ID={:#010x} counts={:#010x}/{:#010x} clusters={:#010x}\n",
                version,
                regs.read32(ID_COUNTS_1),
                regs.read32(ID_COUNTS_2),
                regs.read32(ID_CLUSTERS),
            );
            (version, regs.read32(ID_COUNTS_1), regs.read32(0xe01500))
        };
        let platform = g17p_platform::Platform::new(pdev.as_ref())?;
        let image = g17p_image::Image::new(pdev.as_ref(), &platform)?;
        let session = g17p_boot::Session::prepare(pdev, &platform, &sgx, &image)?;
        use kernel::dma::Device as _;
        // SAFETY: UAT consumes 42-bit DMA addresses, as validated by this driver.
        unsafe {
            pdev.dma_set_mask_and_coherent(kernel::dma::DmaMask::try_new(42)?)?;
        }
        let max_mhz = platform
            .performance
            .iter()
            .map(|state| state.frequency_a_mhz.max(state.frequency_b_mhz))
            .max()
            .ok_or(EINVAL)?;
        let runtime = kernel::sync::Arc::pin_init(
            kernel::new_mutex!(Some(g17p_drm::Runtime { session, image })),
            GFP_KERNEL,
        )?;
        let drm = g17p_drm::register(
            pdev.as_ref(),
            version,
            counts,
            mask,
            max_mhz,
            runtime.clone(),
        )?;
        Ok(Self {
            _sgx: sgx,
            _platform: platform,
            runtime,
            _drm: drm,
        })
    }
}

kernel::module_platform_driver! {
    type: NeoGpu,
    name: "asahi_neo",
    authors: ["Asahi Linux Contributors"],
    description: "Apple A18 Pro GPU driver",
    license: "Dual MIT/GPL",
}
