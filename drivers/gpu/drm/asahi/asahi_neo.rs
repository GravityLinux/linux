// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Scheduled-family G16C/G17P GPU driver.
//!
//! Both profiles use the common Session, DRM files, memory and scheduler
//! owners. G16C command admission remains under construction.

use kernel::{
    c_str, device::Core, devres::Devres, io::mem::IoMem, io::Io, of, platform, prelude::*,
};

#[allow(dead_code)]
mod float;
#[allow(dead_code)]
mod scheduled_board;
#[allow(dead_code)]
mod scheduled_boot_records;
#[allow(dead_code)]
mod scheduled_bootstrap;
#[allow(dead_code)]
mod scheduled_hwdata;
#[allow(dead_code)]
mod scheduled_image;
#[allow(dead_code)]
mod scheduled_initdata;
#[allow(dead_code)]
mod scheduled_initgraph;
#[allow(dead_code)]
mod scheduled_mmio;
#[allow(dead_code)]
mod scheduled_power;
#[allow(dead_code)]
mod scheduled_resources;
#[allow(dead_code)]
mod scheduled_queues;
mod scheduled_operands;
#[allow(dead_code)]
mod scheduled_dispatch;
mod scheduled_lifetime;
#[allow(dead_code)]
mod scheduled_transport;
#[allow(dead_code)]
mod scheduled_vm_g16c;

mod g17p_abi;
mod g17p_boot;
// Source serializers and retained queue/resource lifecycle helpers.
#[allow(dead_code)]
mod g17p_compute;
#[allow(dead_code)]
mod g17p_compute_lifecycle;
#[allow(dead_code)]
mod g17p_compute_memory;
mod g17p_compute_queues;
mod g17p_compute_runtime;
#[allow(dead_code)]
mod g17p_context;
#[allow(dead_code)]
mod g17p_dependency;
#[allow(dead_code)]
mod g17p_dependency_control;
#[allow(dead_code)]
mod g17p_dependency_release;
#[allow(dead_code)]
mod g17p_dependency_retire;
#[allow(dead_code)]
mod g17p_dependency_runtime;
#[allow(dead_code)]
mod g17p_dependency_vm;
mod g17p_drm;
mod g17p_growth;
mod g17p_growth_runtime;
mod g17p_image;
mod g17p_initgraph;
mod g17p_layout;
#[allow(dead_code)]
mod g17p_lifecycle;
mod g17p_memory;
mod g17p_opening;
#[allow(dead_code)]
mod g17p_partial_runtime;
mod g17p_platform;
#[allow(dead_code)]
mod g17p_queue;
#[allow(dead_code)]
mod g17p_render;
#[allow(dead_code)]
mod g17p_render_graph;
mod g17p_render_lifecycle;
mod g17p_render_runtime;
#[allow(dead_code)]
mod g17p_resource_record;
mod g17p_sync;
mod g17p_timestamp;
mod g17p_timestamp_span;
mod g17p_topology;
mod g17p_user_vm;
mod g17p_vm;

const SGX_SIZE: usize = 0x4000000;
const ID_VERSION: usize = 0xd04000;
const ID_COUNTS_1: usize = 0xd04010;
const ID_COUNTS_2: usize = 0xd04014;
const ID_CLUSTERS: usize = 0xd0401c;

#[derive(Clone, Copy)]
enum GpuAbi {
    G16C,
    G17P,
}

// Profile-specific MMIO/resources surround the same runtime and DRM owners.
enum ScheduledGpu {
    G17P {
        _sgx: Pin<KBox<Devres<IoMem<SGX_SIZE>>>>,
        _platform: g17p_platform::Platform,
        runtime: g17p_drm::RuntimeRef,
        _drm: kernel::sync::aref::ARef<kernel::drm::Device<g17p_drm::Driver>>,
    },
    G16C {
        _resources: scheduled_resources::ResourcesG16C,
        runtime: g17p_drm::RuntimeRef,
        _drm: kernel::sync::aref::ARef<kernel::drm::Device<g17p_drm::Driver>>,
    },
}

impl Drop for ScheduledGpu {
    fn drop(&mut self) {
        // Stop firmware while devres still owns all mailbox/MMIO mappings.
        match self {
            Self::G17P { runtime, .. } | Self::G16C { runtime, .. } => {
                drop(runtime.lock().take());
            }
        }
    }
}

kernel::of_device_table!(
    OF_TABLE,
    MODULE_OF_TABLE,
    <ScheduledGpu as platform::Driver>::IdInfo,
    [
        (of::DeviceId::new(c_str!("apple,agx-t8140")), GpuAbi::G17P),
        (of::DeviceId::new(c_str!("apple,agx-g16c")), GpuAbi::G16C),
        (of::DeviceId::new(c_str!("apple,agx-t6041")), GpuAbi::G16C),
    ]
);

impl platform::Driver for ScheduledGpu {
    type IdInfo = GpuAbi;
    const OF_ID_TABLE: Option<of::IdTable<Self::IdInfo>> = Some(&OF_TABLE);

    fn probe(
        pdev: &platform::Device<Core>,
        _info: Option<&Self::IdInfo>,
    ) -> impl PinInit<Self, Error> {
        match _info {
            Some(GpuAbi::G16C) => Self::probe_g16c(pdev),
            _ => Self::probe_g17p(pdev),
        }
    }
}

impl ScheduledGpu {
    // Isolate per-profile construction frames from each other and from the
    // platform adapter. The common entry must not reserve both probe stacks.
    #[inline(never)]
    fn probe_g16c(pdev: &platform::Device<Core>) -> Result<Self> {
        let node = pdev.as_ref().of_node().ok_or(ENODEV)?;
        let resources = scheduled_resources::ResourcesG16C::from_fdt(&node)?;
        let request = pdev.io_request_by_name(c_str!("sgx")).ok_or(EINVAL)?;
        let sgx = KBox::pin_init(request.iomap_sized::<0x3758000>(), GFP_KERNEL)?;
        let version = sgx.try_access().ok_or(ENODEV)?.read32(ID_VERSION);
        let board = scheduled_board::BoardG16C::from_fdt(&node)?;
        let max_mhz = *board.freq_a.iter().max().ok_or(EINVAL)?;
        let image = KBox::new(scheduled_image::ImageG16C::new(&node, 0)?, GFP_KERNEL)?;
        let session = g17p_boot::Session::prepare_g16c(pdev, &resources, sgx, &image)?;
        use kernel::dma::Device as _;
        // SAFETY: Max application mappings use the same checked 42-bit DMA
        // aperture as the common GEM/UAT owners.
        unsafe {
            pdev.dma_set_mask_and_coherent(kernel::dma::DmaMask::try_new(42)?)?;
        }
        let runtime = kernel::sync::Arc::pin_init(
            kernel::new_mutex!(Some(g17p_drm::Runtime {
                session,
                image: g17p_drm::RuntimeImage::G16C(image),
                active: 0,
                exclusive: false,
                engine_handoff: [None, None],
                admission_handoff: None
            })),
            GFP_KERNEL,
        )?;
        let drm = g17p_drm::register_g16c(pdev.as_ref(), version, max_mhz, runtime.clone())?;
        Ok(Self::G16C {
            _resources: resources,
            runtime,
            _drm: drm,
        })
    }

    #[inline(never)]
    fn probe_g17p(pdev: &platform::Device<Core>) -> Result<Self> {
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
            kernel::new_mutex!(Some(g17p_drm::Runtime {
                session,
                image: g17p_drm::RuntimeImage::G17P(image),
                active: 0,
                exclusive: false,
                engine_handoff: [None, None],
                admission_handoff: None
            })),
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
        Ok(Self::G17P {
            _sgx: sgx,
            _platform: platform,
            runtime,
            _drm: drm,
        })
    }
}

kernel::module_platform_driver! {
    type: ScheduledGpu,
    name: "asahi_neo",
    authors: ["Asahi Linux Contributors"],
    description: "Apple A18 Pro GPU driver",
    license: "Dual MIT/GPL",
    params: {
        submission_log: u32 {
            default: 1,
            description: "Successful render/compute submission logs: 0 off for benchmarking, nonzero retains diagnostic evidence",
        },
        cpu_prepare_pause_queue: u32 {
            default: 0,
            description: "Integration diagnostic: pause one private preparation on this public queue for 2 seconds; zero disables",
        },
        cpu_cache_pause_grid: u32 {
            default: 128,
            description: "Integration diagnostic: pause one retired CPU visibility task on this firmware grid for 2 seconds; 128 disables",
        },
        firmware_dependencies: u32 {
            default: 1,
            description: "Publish render/compute command dependencies in firmware; zero selects the CPU-wait diagnostic",
        },
        native_limit_reply: u32 {
            default: 1,
            description: "Acknowledge owned render memory-limit reports with native type9; zero selects consume-only diagnostics",
        },
        first_render_fragment_sync_grow: u32 {
            default: 2,
            description: "Source first-render fragment sync-grow diagnostic: 0 or 1, 2 retains the caller default",
        },
        repeat_fragment_sync_grow: u32 {
            default: 2,
            description: "Source repeat fragment sync-grow diagnostic: 0 or 1, 2 retains the caller default",
        },
        tvb_max_blocks: u32 {
            default: 0,
            description: "Source G17P_TVB_MAX_BLOCKS: zero for the full index capacity, otherwise 8..2048",
        },
        tvb_max_blocks_pool0: u32 {
            default: 0xffffffff,
            description: "Source pool-zero TVB bound: inherit by default, zero unbounded, otherwise 8..2048",
        },
        tvb_max_blocks_pool1: u32 {
            default: 0xffffffff,
            description: "Source pool-one TVB bound: inherit by default, zero unbounded, otherwise 8..2048",
        },
        g16c_compute_slots: u32 {
            default: 2,
            description: "G16C dedicated compute engine slots, 1..2; preserves two render engines",
        },
        g16c_operand_buffers: u32 {
            default: 20,
            description: "G16C quiescent operand-pool target after initial compute use; 20 retains the initial pool",
        },
        g16c_operand_probe: u32 {
            default: 0,
            description: "G16C diagnostic: sample appended operand pages before dispatch and after GPU retirement",
        },
        g16c_initial_alloc_fail: u32 {
            default: 0,
            description: "G16C test only: reject once (1 owner, 2 engines, 3 queue, 4 render, 5 compute work, 6 render work, 7 geometry, 8 flight, 9 transport); zero disables",
        },
        g16c_operand_alloc_fail_after: u32 {
            default: 0xffffffff,
            description: "G16C test only: reject one unpublished operand extension after this many staged buffers; disabled by default",
        },
        g16c_parameter_oom_test: u32 {
            default: 0xffffffff,
            description: "G16C test only: force TA growth off/fragment sync-grow on at this render ordinal; 0xffffffff disables",
        },
        owned_render_fault_kind: u32 {
            default: 0,
            description: "Source owned render fault diagnostic: 0 off, 1 shader store, 2 command fetch",
        },
        owned_render_fault_address: u64 {
            default: 0,
            description: "Page-aligned caller render DVA for the one owned fault diagnostic",
        },
        owned_render_fault_ordinal: u32 {
            default: 0,
            description: "Render ordinal on which to arm the one owned fault diagnostic",
        },
        cleanup_diagnostics: u32 {
            default: 0,
            description: "Source cleanup relocation diagnostics at VM_DESTROY: bit0 descriptor, bit1 context, bit2 transport, bit3 root",
        },
        compute_queues: u32 {
            default: 1,
            description: "Use independently owned ordinary compute queues (0: legacy diagnostic path)",
        },
        native_render_vms: u32 {
            default: 0,
            description: "Source G17P_NATIVE_RENDER_VMS: retain two standalone render VM roots",
        },
        partial_independent_owner: u32 {
            default: 0,
            description: "Source G17P_PARTIAL_INDEPENDENT_OWNER: retain two render pools",
        },
        alternate_queue_pairs: u32 {
            default: 0,
            description: "Source G17P_ALTERNATE_QUEUE_PAIRS: alternate the two render transports",
        },
        keep_partial_operand_mappings: u32 {
            default: 0,
            description: "Source G17P_PARTIAL_KEEP_OPERAND_MAPPINGS: preserve low render operands",
        },
        native_compute_vms: u32 {
            default: 0,
            description: "Source G17P_NATIVE_COMPUTE_VMS: retain two standalone compute VM roots",
        },
        native_barriers: u32 {
            default: 0,
            description: "Source G17P_MODERN_NATIVE_BARRIERS: use native first C/R/C publication",
        },
    },
}
