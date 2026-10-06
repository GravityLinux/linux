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
mod g17p_compute_lifecycle;
#[allow(dead_code)]
mod g17p_compute_memory;
mod g17p_compute_runtime;
mod g17p_compute_queues;
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
mod g17p_user_vm;
// Core queue port is exercised by source differential tests until submit is wired.
#[allow(dead_code)]
mod g17p_queue;
// Render serializers are checked against the shim before runtime integration.
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
            kernel::new_mutex!(Some(g17p_drm::Runtime {
                session,
                image,
                active: 0,
                exclusive: false,
                engine_handoff: [None,None],
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
            default: 0,
            description: "Qualification diagnostic: publish cross-owner command dependencies in firmware; zero retains qualified error handling",
        },
        native_limit_reply: u32 {
            default: 0,
            description: "Qualification diagnostic: reply to an owned render memory-limit report with the observed native type9 command",
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
