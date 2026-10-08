// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! T8132 GPU driver, ported from the working Python DRM shim.

#![recursion_limit = "2048"]

mod float;
mod g16;
mod g16_alloc;
mod g16_compute;
mod g16_drm;
mod g16_fw;
mod g16_platform;
mod g16_power;
mod g16_render;
mod g16_stamp;
mod g16_tvb;
mod g16_vm;
#[path = "m4_mem.rs"]
mod mem;
#[path = "m4_pgtable.rs"]
mod pgtable;
#[path = "m4_util.rs"]
mod util;

use kernel::io::{Io, IoBase};
use kernel::{c_str, device::Core, drm, io::mem::DevresIoMem, of, platform, prelude::*};

const SGX_SIZE: usize = 0x4000000;
const ID_VERSION: usize = 0xd04000;
const ID_COUNTS_1: usize = 0xd04010;
const ID_COUNTS_2: usize = 0xd04014;
const ID_CLUSTERS: usize = 0xd0401c;

struct M4Gpu;

struct M4GpuData<'bound> {
    _sgx: Option<DevresIoMem<SGX_SIZE>>,
    _drm: Option<drm::Registration<'bound, g16_drm::Driver>>,
}

kernel::of_device_table!(
    OF_TABLE,
    <M4Gpu as platform::Driver>::IdInfo,
    [(of::DeviceId::new(c_str!("apple,agx-t8132")), ())]
);

impl platform::Driver for M4Gpu {
    type IdInfo = ();
    type Data<'bound> = M4GpuData<'bound>;
    const OF_ID_TABLE: Option<of::IdTable<Self::IdInfo>> = Some(&OF_TABLE);

    fn probe<'bound>(
        pdev: &'bound platform::Device<Core<'_>>,
        _info: Option<&'bound Self::IdInfo>,
    ) -> impl PinInit<Self::Data<'bound>, Error> + 'bound {
        let dev = pdev.as_ref();
        if !(11..=g16_tvb::MAX_BLOCKS).contains(&module_parameters::tvb_max_blocks.value()) {
            return Err(EINVAL);
        }

        if module_parameters::probe_only.value() == 0 {
            return Ok(M4GpuData {
                _sgx: None,
                _drm: Some(g16_drm::register(pdev)?),
            });
        }
        let request = pdev.io_request_by_name(c_str!("sgx")).ok_or(EINVAL)?;
        let sgx = request.iomap_sized::<SGX_SIZE>()?.into_devres()?;
        {
            let regs = sgx.try_access().ok_or(ENODEV)?;
            let version = regs.as_view().relaxed().read32(ID_VERSION);
            if version == 0 || version == u32::MAX {
                dev_err!(dev, "Invalid GPU ID {:#010x}\n", version);
                return Err(ENODEV);
            }
            dev_info!(
                dev,
                "Apple M4 GPU recognized: ID={:#010x} counts={:#010x}/{:#010x} clusters={:#010x}\n",
                version,
                regs.as_view().relaxed().read32(ID_COUNTS_1),
                regs.as_view().relaxed().read32(ID_COUNTS_2),
                regs.as_view().relaxed().read32(ID_CLUSTERS),
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
            platform.registers.len(), platform.core_mask, platform.max_frequency(),
            platform.maximum_power, platform.period_ms, platform.period_clocks,
            hwdata.len(), region_c.len(), bundle.len());
        Ok(M4GpuData {
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
        fw_counter_start: u64 {
            default: 0,
            description: "Initial submission counters for firmware rollover qualification (normally 0)",
        },
        fw_trace: u32 {
            default: 0,
            description: "Trace mask: 1=firmware KTrace/publications, 2=admission saturation, 4=publication batches, 8=queued dependencies, 16=retirement delays, 32=dump one render's Work structs, 64=firmware RX ring activity, 128=firmware mappings and configuration dump",
        },
        render_deps: u32 {
            default: 0,
            description: "Experiment: 1 orders renders in the kernel (waits for other queues and for the previous fragment stamp a tiling stage needs, then submits with no firmware dependency items); 0 encodes firmware dependency items",
        },
        compute_deps: u32 {
            default: 1,
            description: "Compute Work ordering against other queues: 1 waits in the kernel and submits Works without firmware dependency items (as Metal); 0 encodes firmware dependency items (the earlier behaviour, which can stall the firmware under heavy compute bursts)",
        },
        max_lane_works: u32 {
            default: 64,
            description: "Maximum unretired Works per firmware lane (render TA/3D and compute) before submissions back off; 0: unlimited. The firmware stops completing Works past a 128-Work window per lane, so keep this at 127 or less; Metal keeps at most 64 command buffers in flight per queue",
        },
        perf_base: u32 {
            default: 0,
            description: "Debug: GPU performance controller base pstate (0: ADT value; 10 is the top state)",
        },
        tvb_growth: u32 {
            default: 3,
            description: "TVB growth factor per firmware allocation request (debug; 3 as Apple8)",
        },
        tvb_max_blocks: u32 {
            default: 2193,
            description: "Maximum retained 128 KiB TVB blocks per context (11 through 6579)",
        },
        probe_only: u32 {
            default: 0,
            description: "Identify the GPU without starting firmware or registering DRM",
        },
        work_timestamps: u32 {
            default: 0,
            description: "Per-Work firmware profiling timestamp slots (0: shared end stamp only, as macOS)",
        },
        region_c_layout: u32 {
            default: 0,
            description: "Debug: region C host tunables as macOS 25G83 holds them (0: original)",
        },
        work_layout: u32 {
            default: 0,
            description: "Work fields as macOS (debug): 1=no TVB growth request or fragment flags 0xba8/0xbdc, 2=render index rings, 4=completion control 3, 8=fragment gate 0xb5 after each 40-render cycle start",
        },
        micro_layout: u32 {
            default: 0,
            description: "Microsequence scene and Work counter fields: 0=original, 1=as macOS (scene slot, per-Work counters)",
        },
        render_barrier: u32 {
            default: 0,
            description: "Debug render-to-render barriers: 1=apply every render's vdm_barrier at fragment, 2=omit fragment waits already ordered by the queue's fragment lane",
        },
        shared_notifier: u32 {
            default: 1,
            description: "Renders share one notifier and driver stamp per queue lane, as on macOS (0: one per Work)",
        },
        notifier_flush: u32 {
            default: 0,
            description: "Tiling Work notifier-stamp flush flag (macOS writes 0; this driver previously wrote 1)",
        },
        dependency_kind: u32 {
            default: 1,
            description: "Experiment: firmware kind of render-to-render dependencies (1 normal, 0 as the internal TA-to-fragment wait)",
        },
    },
}
