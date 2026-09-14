// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! T8132 GPU driver, ported from the working Python DRM shim.

#![recursion_limit = "2048"]

#[path = "m4_float.rs"]
mod float;
mod g16;
mod g16_cdm;
mod g16_compute;
mod g16_drm;
mod g16_fw;
mod g16_platform;
mod g16_power;
mod g16_render;
mod g16_tvb;
mod g16_vm;
#[path = "m4_mem.rs"]
mod mem;
#[path = "m4_pgtable.rs"]
mod pgtable;
#[path = "m4_util.rs"]
mod util;

use kernel::{c_str, device::Core, devres::Devres, io::mem::IoMem, of, platform, prelude::*};

const SGX_SIZE: usize = 0x4000000;
const ID_VERSION: usize = 0xd04000;
const ID_COUNTS_1: usize = 0xd04010;
const ID_COUNTS_2: usize = 0xd04014;
const ID_CLUSTERS: usize = 0xd0401c;

struct M4Gpu {
    _sgx: Option<Pin<KBox<Devres<IoMem<SGX_SIZE>>>>>,
    _drm: Option<g16_drm::DeviceRef>,
}

kernel::of_device_table!(
    OF_TABLE,
    MODULE_OF_TABLE,
    <M4Gpu as platform::Driver>::IdInfo,
    [(of::DeviceId::new(c_str!("apple,agx-t8132")), ())]
);

impl platform::Driver for M4Gpu {
    type IdInfo = ();
    const OF_ID_TABLE: Option<of::IdTable<Self::IdInfo>> = Some(&OF_TABLE);

    fn probe(
        pdev: &platform::Device<Core>,
        _info: Option<&Self::IdInfo>,
    ) -> impl PinInit<Self, Error> {
        let dev = pdev.as_ref();
        if !(11..=331).contains(module_parameters::tvb_max_blocks.value()) {
            return Err(EINVAL);
        }

        if *module_parameters::probe_only.value() == 0 {
            return Ok(Self {
                _sgx: None,
                _drm: Some(g16_drm::register(pdev)?),
            });
        }
        let request = pdev.io_request_by_name(c_str!("sgx")).ok_or(EINVAL)?;
        let sgx = KBox::pin_init(request.iomap_sized::<SGX_SIZE>(), GFP_KERNEL)?;
        {
            let regs = sgx.try_access().ok_or(ENODEV)?;
            let version = regs.read32_relaxed(ID_VERSION);
            if version == 0 || version == u32::MAX {
                dev_err!(dev, "Invalid GPU ID {:#010x}\n", version);
                return Err(ENODEV);
            }
            dev_info!(
                dev,
                "Apple M4 GPU recognized: ID={:#010x} counts={:#010x}/{:#010x} clusters={:#010x}\n",
                version,
                regs.read32_relaxed(ID_COUNTS_1),
                regs.read32_relaxed(ID_COUNTS_2),
                regs.read32_relaxed(ID_CLUSTERS),
            );
            dev_info!(
                dev,
                "Probe-only driver; GPU firmware execution is disabled\n"
            );
        }
        g16::validate_boot_resources(pdev)?;
        let platform = g16_platform::Platform::new(dev)?;
        let hwdata = platform.hwdata()?;
        let region_c = platform.region_c()?;
        let bundle = platform.bundle(0)?;
        dev_info!(dev,
            "M4 source configuration: {} register windows, mask={:#x}, max-frequency={} MHz, max-power={} mW, period={}/{} ms/clocks, objects={}/{}/{} bytes\n",
            platform.registers.len(), platform.core_mask, platform.freq_a[10],
            platform.maximum_power, platform.period_ms, platform.period_clocks,
            hwdata.len(), region_c.len(), bundle.len());
        Ok(Self {
            _sgx: Some(sgx),
            _drm: None,
        })
    }
}

kernel::module_platform_driver! {
    type: M4Gpu,
    name: "asahi_m4",
    authors: ["Gravity Linux Contributors"],
    description: "Apple M4 GPU driver",
    license: "GPL v2",
    params: {
        fw_trace: u32 {
            default: 0,
            description: "Trace mask: 1=firmware KTrace/publications, 2=admission saturation",
        },
        tvb_max_blocks: u32 {
            default: 331,
            description: "Maximum retained TVB blocks per context (11 through 331)",
        },
        probe_only: u32 {
            default: 0,
            description: "Identify the GPU without starting firmware or registering DRM",
        },
    },
}
