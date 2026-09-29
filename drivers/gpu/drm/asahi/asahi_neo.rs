// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Initial T8140 / G17P GPU identification support.
//!
//! Validates boot resources and constructs the unpublished firmware graph.
//! Firmware startup and DRM registration will follow in the synchronous port.

use kernel::{
    c_str, device::Core, devres::Devres, io::mem::IoMem, io::Io, of, platform, prelude::*,
};

mod g17p_abi;
mod g17p_image;
mod g17p_initgraph;
mod g17p_layout;
mod g17p_platform;
mod g17p_topology;

const SGX_SIZE: usize = 0x4000000;
const ID_VERSION: usize = 0xd04000;
const ID_COUNTS_1: usize = 0xd04010;
const ID_COUNTS_2: usize = 0xd04014;
const ID_CLUSTERS: usize = 0xd0401c;

struct NeoGpu {
    _sgx: Pin<KBox<Devres<IoMem<SGX_SIZE>>>>,
    _platform: g17p_platform::Platform,
    _image: g17p_image::Image,
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
        {
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
            dev_info!(
                pdev.as_ref(),
                "Identification only; firmware and DRM initialization are not implemented\n"
            );
        }
        let platform = g17p_platform::Platform::new(pdev.as_ref())?;
        let image = g17p_image::Image::new(pdev.as_ref(), &platform)?;
        Ok(Self {
            _sgx: sgx,
            _platform: platform,
            _image: image,
        })
    }
}

kernel::module_platform_driver! {
    type: NeoGpu,
    name: "asahi_neo",
    authors: ["Asahi Linux Contributors"],
    description: "Apple A18 Pro GPU identification driver",
    license: "Dual MIT/GPL",
}
