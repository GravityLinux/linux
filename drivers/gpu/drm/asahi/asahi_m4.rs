// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! T8132 GPU driver, ported from the working Python DRM shim.

#![recursion_limit = "2048"]

mod float;
mod g16;
mod g16_alloc;
mod g16_compute;
mod g16_deferred;
mod g16_drain;
mod g16_drm;
mod g16_finished;
mod g16_fw;
mod g16_mesh;
mod g16_platform;
mod g16_power;
mod g16_queue_fail;
mod g16_render;
mod g16_stamp;
mod g16_timeline;
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
        if !(11..=g16_tvb::MAX_BLOCKS).contains(&module_parameters::tvb_max_blocks.value())
            || module_parameters::dependency_kind.value() > 1
        {
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
            description: "Trace mask: 1=firmware KTrace/publications, 2=admission saturation, 4=publication batches, 8=queued dependencies, 16=retirement delays, 32=dump one render's Work structs, 64=firmware RX ring activity, 128=firmware mappings and configuration dump, 256=queue event index changes",
        },
        wedge_ms: u32 {
            default: 2000,
            description: "A4 wedge detection window: a render with TA done and all waits met whose 3D never starts, with all channels drained and no fragment progress on any lane for this long",
        },
        max_queue_entries: u32 {
            default: 0,
            description: "Maximum unretired Works plus their firmware dependency items per queue (all lanes together) before submissions back off; 0 (default): unlimited, with max_lane_entries bounding each lane (production #149 used 96). Runtime: debugfs asahi_m4/queue_cap",
        },
        max_lane_entries: u32 {
            default: 120,
            description: "Maximum unretired Works plus their firmware dependency items on one firmware lane (render TA or 3D, or compute) before submissions back off; 0: unlimited. Keeps dependency items below the firmware's 128-Work per-lane window. Keep it at 127 or less when firmware dependency items are enabled",
        },
        max_lane_works: u32 {
            default: 64,
            description: "Maximum unretired Works per firmware lane (render TA/3D and compute) before submissions back off; 0: unlimited. The firmware stops completing Works past a 128-Work window per lane, so keep this at 127 or less; Metal keeps at most 64 command buffers in flight per queue",
        },
        perf_base: u32 {
            default: 0,
            description: "Debug: GPU performance controller base pstate (0: ADT value; 10 is the top state)",
        },
        tvb_max_blocks: u32 {
            default: 2193,
            description: "Maximum retained 128 KiB TVB blocks per context (11 through 6579)",
        },
        extra0: u32 {
            default: 1,
            description: "1 (default) fills the extra_0 policy object (power and thermal controller parameters) from the bundle; 0 leaves it zero, as before",
        },
        probe_only: u32 {
            default: 0,
            description: "Identify the GPU without starting firmware or registering DRM",
        },
        debug_ctx: u32 {
            default: 0,
            description: "Debug: 1 logs each VM's owning process and every firmware context slot rebind",
        },
        native_mesh: u32 {
            default: 0,
            description: "Experimental paired native object/mesh streams: 0=disabled, 1=T16/T32 one-parent profile, 2=unqualified multiple-parent profile, 3=research admission of wider mesh state (not qualified); no capability advertisement. Runtime: debugfs asahi_m4/native_mesh (a VM keeps the support mode of its first render)",
        },
        dependency_kind: u32 {
            default: 0,
            description: "Firmware kind of render-to-render dependencies: 0 (default) as the internal TA-to-fragment wait, which every production boot has passed since early October 2026; 1 the dynamic dependency graph",
        },
        fault_control: u32 {
            default: 3,
            description: "Firmware fault control word (region C +0xe28): 3 as macOS; 0 hard faults",
        },
        soft_faults: u32 {
            default: 1,
            description: "1 advertises soft faults to userspace (needs fault_control=3; Mesa with hk 243c5ac0c43 and 31b9d1d5bfb); 0 hides them",
        },
        shared_notifier: u32 {
            default: 1,
            description: "1 (default): the renders of one submission share a completion notifier, as Metal shares one per command buffer; 0: each render has its own (fallback). Runtime: debugfs asahi_m4/shared_notifier",
        },
        timeline: u32 {
            default: 0,
            description: "Debug: 1 records each submission when its fence signals into a binary ring at debugfs asahi_m4/timeline (format in g16_timeline.rs): the client pid and file, input fences pending at the ioctl, CNTVCT ticks (1 GHz) at the ioctl, scheduler run, first and last publication and completion, plus admission deferrals and their reasons. 2 also records the doorbell, and for the notification that retired the submission its IRQ, RTKit delivery, pump wake and retire-scan times",
        },
        profile: u32 {
            default: 0,
            description: "Debug: 1 times publish-path sections into debugfs asahi_m4/publish_profile (two serialized counter reads per section, per firmware read). Runtime: debugfs asahi_m4/profile",
        },
        timeline_records: u32 {
            default: 131072,
            description: "Debug: timeline ring capacity in records (160 bytes each), allocated at probe only when timeline is nonzero",
        },
    },
}
