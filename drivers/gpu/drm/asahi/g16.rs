// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors
// Adapted from Niklas Sheth's linux-m4-integration prototype.

//! G16 firmware bootstrap. Its 43-bit address space and initialization graph
//! must not be passed to a G13/G14 firmware manager.

use kernel::io::{Io, IoBase};
use kernel::{c_str, device::Core, platform, prelude::*};

const PAGE_SIZE: u64 = 0x4000;

/// Validate the memory contract supplied by m1n1 before starting firmware.
pub(crate) fn validate_boot_resources(pdev: &platform::Device<Core<'_>>) -> Result {
    let dev = pdev.as_ref();
    let node = dev.of_node().ok_or(EINVAL)?;
    let fwnode = dev.fwnode().ok_or(EINVAL)?;
    // This is the firmware build exercised by the Python M4 shim. The
    // loader's generic macOS-version lookup currently does not recognize it.
    let firmware: KVec<u8> = node.get_property(c_str!("apple,firmware-build"))?;
    if firmware.as_slice() != b"mBoot-18000.161.10\0" {
        dev_err!(dev, "G16: unqualified boot firmware identifier\n");
        return Err(ENOTSUPP);
    }
    let private_vm: KVec<u64> = fwnode
        .property_read_array_vec(c_str!("apple,rtkit-private-vm-region"), 2)?
        .required_by(dev)?;

    if private_vm.len() != 2 {
        return Err(EINVAL);
    }
    if private_vm[0] != 0xffff_fc00_0000_0000 || private_vm[1] != 0x20_0000_0000 {
        dev_err!(
            dev,
            "G16: unsupported firmware private VM {:?}\n",
            private_vm
        );
        return Err(EINVAL);
    }

    dev_info!(dev, "G16: boot firmware mBoot-18000.161.10\n");
    let mut regions = KVec::<(u64, u64)>::new();
    for (name, min_size) in [
        (c_str!("ttbs"), PAGE_SIZE),
        (c_str!("pagetables"), PAGE_SIZE),
        (c_str!("l2"), PAGE_SIZE),
        (c_str!("handoff"), PAGE_SIZE),
        (c_str!("firmware"), PAGE_SIZE),
    ] {
        let res = node.reserved_mem_region_to_resource_byname(name)?;
        let base = res.start() as u64;
        let size = res.size() as u64;
        let end = base.checked_add(size).ok_or(EINVAL)?;
        if base == 0 || size < min_size || (base | size) & (PAGE_SIZE - 1) != 0 {
            dev_err!(
                dev,
                "G16: invalid {} region {:#x}+{:#x}\n",
                name,
                base,
                size
            );
            return Err(EINVAL);
        }
        for &(other_base, other_end) in &regions {
            if base < other_end && other_base < end {
                dev_err!(dev, "G16: overlapping boot memory regions\n");
                return Err(EINVAL);
            }
        }
        regions.push((base, end), GFP_KERNEL)?;
        dev_info!(dev, "G16: {} {:#x}+{:#x}\n", name, base, size);
    }

    dev_info!(
        dev,
        "G16: boot memory contract validated; firmware initialization pending\n"
    );
    Ok(())
}

use kernel::{
    io::{self, mem::DevresIoMem, IoSysMap, SysMem},
    soc::apple::rtkit,
    sync::Arc,
    sync::aref::ARef,
    types::ForeignOwnable,
};

struct BootData {
    dev: ARef<platform::Device>,
    notifications: Arc<Notifications>,
}

struct CrashBuffer {
    map: io::mem::Mem,
    offset: usize,
    size: usize,
    iova: usize,
}

impl rtkit::Buffer for CrashBuffer {
    fn iova(&self) -> Result<usize> {
        Ok(self.iova)
    }

    fn buf(&mut self) -> Result<IoSysMap<'_, [u8]>> {
        if self.offset.checked_add(self.size).ok_or(EINVAL)? > self.map.size() {
            return Err(EINVAL);
        }
        // SAFETY: `offset..offset + size` lies within the mapped reserved
        // firmware region, which stays mapped while `self` (and the borrow) lives.
        let region = unsafe {
            core::ptr::slice_from_raw_parts_mut(self.map.ptr().add(self.offset), self.size)
        };
        // SAFETY: As above; the region is valid kernel memory for the borrow.
        Ok(IoSysMap::Sys(unsafe { SysMem::new(region) }))
    }
}

struct BootOps;

#[vtable]
impl rtkit::Operations for BootOps {
    type Data = Arc<BootData>;
    type Buffer = CrashBuffer;

    fn shmem_map(
        data: <Self::Data as ForeignOwnable>::Borrowed<'_>,
        iova: usize,
        size: usize,
    ) -> Result<Self::Buffer> {
        let node = data.dev.as_ref().of_node().ok_or(EINVAL)?;
        let res = node.reserved_mem_region_to_resource_byname(c_str!("firmware"))?;
        let offset = iova.checked_sub(res.start() as usize).ok_or(EINVAL)?;
        if offset.checked_add(size).ok_or(EINVAL)? > res.size() as usize {
            return Err(EINVAL);
        }
        // SAFETY: The loader reserved this RAM for the already loaded GPU
        // firmware. Only its advertised crash-buffer subrange is exposed.
        let map = unsafe { io::mem::Mem::try_new(res, io::mem::MemFlag::WB.into())? };
        dev_info!(
            data.dev.as_ref(),
            "G16: crash buffer {:#x}+{:#x}\n",
            iova,
            size
        );
        Ok(CrashBuffer {
            map,
            offset,
            size,
            iova,
        })
    }

    fn recv_message_early(
        data: <Self::Data as ForeignOwnable>::Borrowed<'_>,
        ep: u8,
        msg: u64,
    ) -> bool {
        // timeline=2 only stamps the IRQ; RTKit's thread still delivers it.
        if crate::module_parameters::timeline.value() >= 2 && ep == 0x20 && msg == 0x0042_0000_0000_0000 {
            data.notifications
                .last_irq
                .store(crate::g16_drm::ticks(), core::sync::atomic::Ordering::Relaxed);
        }
        false
    }

    fn recv_message(data: <Self::Data as ForeignOwnable>::Borrowed<'_>, ep: u8, msg: u64) {
        if ep == 0x20 && msg == 0x0042_0000_0000_0000 {
            if crate::module_parameters::timeline.value() >= 2 {
                data.notifications
                    .last_rx
                    .store(crate::g16_drm::ticks(), core::sync::atomic::Ordering::Relaxed);
            }
            // RTKit delivers this callback in process context after the
            // mailbox receive barrier. The worker drains the firmware rings.
            data.notifications.notify(false);
            return;
        }
        dev_info!(
            data.dev.as_ref(),
            "G16: firmware message {:#x}:{:#018x}\n",
            ep,
            msg
        );
    }

    fn crashed(data: <Self::Data as ForeignOwnable>::Borrowed<'_>, _log: Option<&[u8]>) {
        data.notifications.notify(true);
        dev_err!(data.dev.as_ref(), "G16: firmware crashed\n");
    }
}

/// Resources retained while the autonomous firmware session is running.
/// No render node is published until the submission backend is available.
#[path = "g16_runtime.rs"]
mod render_runtime;
pub(crate) use render_runtime::{FirmwareQueues, Stamp, Support, MAX_FLIGHTS};

#[path = "g16_notify.rs"]
mod notify;
pub(crate) use notify::Notifications;

// Keep the opening barrier transport separate from render queues. Firmware
// retains queue and item references after consuming this bootstrap prefix.
const BOOT_QUEUE: u64 = 0xffff_fc20_c000_4980;
const BOOT_POINTERS: u64 = 0xffff_fc20_0001_50c0;
const BOOT_RING: u64 = 0xffff_fc20_c001_50c0;
const BOOT_JOBS: u64 = 0xffff_fc20_0000_0030;
const BOOT_CONTEXT: u64 = 0xffff_fc20_c050_8080;
const BOOT_BARRIER: u64 = 0xffff_fc20_c052_0080;

/// Hang evidence is always available, independently of runtime tracing knobs.
/// Keep its state on the heap rather than growing the firmware probe frame.
struct Diagnostics {
    /// A4 wedge detection state (check_wedge).
    wedge_since: Option<kernel::time::Instant<kernel::time::Monotonic>>,
    /// When check_wedge last sampled (at most every 50 ms; it reads every render
    /// flight's stamps and every channel's registers).
    wedge_sampled: Option<kernel::time::Instant<kernel::time::Monotonic>>,
    wedge_signature: u64,
    /// The current wedge has been logged.
    wedge_reported: bool,
    retire_log: KVec<render_runtime::Retired>,
    retire_next: usize,
    /// Heap-allocated: Bootstrap is built on the probe stack.
    handovers: KVec<Option<render_runtime::Handover>>,
    lane_events: KVec<(u64, u32)>,
    stale_event_tags: u32,
    stale_live_tags: u32,
    stale_live_own_tags: u32,
    dependency_waits: u32,
    /// The last 64 stale tags: (time ns, stamp, want, tag, current, live).
    stale_recent: KVec<(i64, u64, u32, u32, u32, bool)>,
    stale_next: usize,
    pressure_reported: core::sync::atomic::AtomicU64,
    /// Backpressure bits seen since the last take_pressure() (timeline=1).
    pressure_seen: core::sync::atomic::AtomicU64,
    hold_key: core::sync::atomic::AtomicU64,
    hold_since_ns: core::sync::atomic::AtomicI64,
    hold_near_misses: core::sync::atomic::AtomicU32,
}

pub(crate) struct Bootstrap {
    dev: ARef<platform::Device>,
    pub(crate) notifications: Arc<Notifications>,
    _rtkit: Pin<KBox<rtkit::RtKit<BootOps>>>,
    _asc: DevresIoMem<0x4000>,
    _sgx: DevresIoMem<0x4000000>,
    _handoff: io::mem::Mem,
    _address_space: Option<crate::g16_vm::AddressSpace>,
    _firmware_space: Option<crate::g16_vm::FirmwareSpace>,
    _opening_client: Option<crate::g16_vm::AddressSpace>,
    timestamp_ranges: KVec<core::ops::Range<u64>>,
    opening_notification_pending: bool,
    pub(crate) gpu_revision: u32,
    pub(crate) core_mask: u32,
    pub(crate) maximum_frequency_khz: u32,
    tvb_pools: KVec<crate::g16_tvb::Tvb>,
    /// Work encoding scratch, allocated once so publication never allocates.
    pub(crate) encodings: render_runtime::Encodings,
    render_publication: u64,
    diagnostics: KBox<Diagnostics>,
    next_work_va: u64,
    /// Queue identities and TVB pools; see render_runtime::OBJECT_BASE.
    next_object_va: u64,
    work_arenas: [core::ops::Range<u64>; 2],
    /// Retired Work slots: VA and the client root their alias was last
    /// mapped into (the device alias is mapped on first use).
    free_work: [KVec<(u64, u64)>; 2],
    compute_publication: u64,
    queue_events: Arc<core::sync::atomic::AtomicU64>,
    /// Queue storage returned by dropped queues, awaiting context destruction.
    /// It holds a reserved slot for every region, so dropping never allocates.
    retired_queues: Arc<kernel::sync::Mutex<KVec<u64>>>,
    /// The last retire scan's firmware words, on the heap: this
    /// struct is built by value on the probe stack.
    pub(crate) last_scan: KBox<render_runtime::WordSnapshot>,
    queue_regions: usize,
    /// Storage whose context destruction is published, with its control target.
    destroying_queues: KVec<(u64, u32)>,
    free_queues: KVec<u64>,
    render_failed: bool,
    flights: KVec<render_runtime::Flight>,
    /// Flights retired by the latest scan, returned one per retire() call.
    retired: KVec<(u64, bool)>,
    publications: KVec<render_runtime::Publication>,
    /// Admission's compact view of `flights` and cached channel cursors.
    ledger: KBox<render_runtime::Ledger>,
    /// The latest retirement progress; the watchdog measures each flight
    /// from the later of this and its publication.
    progress: Option<kernel::time::Instant<kernel::time::Monotonic>>,
    batch_works: usize,
    context_roots: [Option<crate::g16_vm::Roots>; 64],
    context_users: [u32; 64],
    render_support: kernel::sync::Arc<core::sync::atomic::AtomicU64>,
}

// SAFETY: The session owns its mapped memory and page tables; none are
// thread-local. Runtime access is exclusive (under the DRM frontend mutex).
// RTKit callbacks access only their independently synchronized callback data.
unsafe impl Send for Bootstrap {}

/// Firmware-private heaps allocated at bootstrap, never released.
const FIRMWARE_HEAPS: [(u64, usize); 18] = [
    (0xffff_fc20_c002_0000, 0x8c000),
    (0xffff_fc20_c00b_0000, 0xc0000),
    (0xffff_fc20_c017_8000, 0x94000),
    (0xffff_fc20_c021_0000, 0x84000),
    (0xffff_fc20_c029_8000, 0xac000),
    (0xffff_fc20_c034_8000, 0x4000),
    (0xffff_fc20_c035_0000, 0x4000),
    (0xffff_fc20_c035_8000, 0x8000),
    (0xffff_fc20_c036_8000, 0x8000),
    (0xffff_fc20_c037_8000, 0xc000),
    (0xffff_fc20_c038_8000, 0x34000),
    (0xffff_fc20_c03c_0000, 0xc4000),
    (0xffff_fc20_c048_8000, 0x34000),
    (0xffff_fc20_c04c_0000, 0x10000),
    (0xffff_fc20_c051_0000, 0x4000),
    (0xffff_fc20_c053_0000, 0x4000),
    (0xffff_fc20_c05e_8000, 0x4000),
    (0xffff_fc20_c05e_c000, 0x4000),
];

/// Shared single pages allocated at bootstrap, never released.
const SHARED_PAGES: [u64; 13] = [
    0xffff_fc20_0011_0000,
    0xffff_fc20_0011_8000,
    0xffff_fc20_0012_0000,
    0xffff_fc20_0012_8000,
    0xffff_fc20_0013_0000,
    0xffff_fc20_0015_0000,
    0xffff_fc20_0014_0000,
    0xffff_fc20_001b_0000,
    0xffff_fc20_001c_0000,
    0xffff_fc20_001c_4000,
    0xffff_fc20_001c_8000,
    0xffff_fc20_001c_c000,
    0xffff_fc20_c052_4000,
];

impl Drop for Bootstrap {
    fn drop(&mut self) {
        if let Err(e) = self._rtkit.as_mut().shutdown() {
            dev_err!(
                self.dev.as_ref(),
                "G16: shutdown failed {:?}; retaining GPU mappings\n",
                e
            );
            // A failed shutdown is not proof that firmware stopped accessing
            // these pages. Keep their tables and backing memory alive.
            core::mem::forget(self._address_space.take());
            core::mem::forget(self._firmware_space.take());
            core::mem::forget(self._opening_client.take());
        }
    }
}

impl Bootstrap {
    /// Debug (debugfs `fw_mem`): pages of the firmware regions allocated at
    /// bootstrap and never released, sorted by VA.
    pub(crate) fn firmware_pages(&self) -> Result<KVec<(u64, kernel::addr::PhysicalAddr)>> {
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        let pointers = crate::g16_fw::RootPointers::bootstrap();
        let mut pages = KVec::new();
        let fixed = [
            crate::g16_fw::ROOT_ADDRESS,
            crate::g16_fw::BUNDLE_ADDRESS,
            0xffff_fc20_0003_0000,
            pointers.region_a,
            pointers.region_c,
            0xffff_fc20_c050_0000,
            0xffff_fc20_001b_8000,
            crate::g16_compute::QUEUE,
            crate::g16_compute::SHARED,
        ];
        let heaps = FIRMWARE_HEAPS.iter().map(|(va, _)| *va);
        for va in Iterator::chain(Iterator::chain(fixed.into_iter(), heaps), SHARED_PAGES) {
            for page in fw.region_pages(va)? {
                pages.push(page, GFP_KERNEL)?;
            }
        }
        pages.sort_unstable_by_key(|p| p.0);
        Ok(pages)
    }

    pub(crate) fn new(pdev: &platform::Device<Core<'_>>) -> Result<Self> {
        if crate::module_parameters::native_mesh.value() > 1 {
            return Err(EINVAL);
        }
        validate_boot_resources(pdev)?;
        let platform = crate::g16_platform::Platform::new(pdev.as_ref())?;
        // All board inputs are checked before any firmware is started.
        let mut bundle = platform.bundle(unsafe { kernel::bindings::get_random_u32() })?;
        // The firmware Timestamp handler accepts user destinations only in
        // this 64 MiB aperture, based at header+0x28.
        bundle[0x28..0x30].copy_from_slice(&crate::g16_fw::TIMESTAMP_BASE.to_le_bytes());
        let region_c = platform.region_c()?;
        let dev = pdev.as_ref();
        let mut address_space = crate::g16_vm::AddressSpace::new()?;
        address_space.check_geometry(dev)?;
        address_space.check_borrowed_root(dev)?;
        let asc = pdev
            .io_request_by_name(c_str!("asc"))
            .ok_or(EINVAL)?
            .iomap_sized::<0x4000>()?
            .into_devres()?;
        let sgx = pdev
            .io_request_by_name(c_str!("sgx"))
            .ok_or(EINVAL)?
            .iomap_sized::<0x4000000>()?
            .into_devres()?;
        let res = dev
            .of_node()
            .ok_or(EINVAL)?
            .reserved_mem_region_to_resource_byname(c_str!("handoff"))?;
        // SAFETY: This is the reserved firmware handoff page from the loader,
        // not an allocator-owned page. No new DMA address is introduced here.
        let handoff = unsafe { io::mem::Mem::try_new(res, io::mem::MemFlag::WB.into())? };
        let asc_guard = asc.try_access().ok_or(ENODEV)?;
        let asc_io = &*asc_guard;
        let control = asc_io.as_view().relaxed().read32(0x44);
        let status = asc_io.as_view().relaxed().read32(0x48);
        drop(asc_guard);
        dev_info!(
            dev,
            "G16: cold ASC control={:#x} status={:#x}\n",
            control,
            status
        );
        if control & 0x10 != 0 || status & 3 != 2 {
            return Err(EBUSY);
        }
        let sgx_guard = sgx.try_access().ok_or(ENODEV)?;
        let sgx_io = &*sgx_guard;
        let gpu_id = sgx_io.as_view().relaxed().read32(crate::ID_VERSION);
        if gpu_id == 0 || gpu_id == u32::MAX {
            dev_err!(dev, "Invalid GPU ID {:#010x}\n", gpu_id);
            return Err(ENODEV);
        }
        // ID_VERSION retains the revision byte used by the Asahi UAPI:
        // 0x00 = A0, 0x10 = B0, 0x11 = B1. This is not the firmware enum.
        let gpu_revision = (gpu_id >> 8) & 0xff;
        dev_info!(dev, "G16: GPU ID={:#010x} revision={:#04x}\n", gpu_id, gpu_revision);
        for offset in [0x1000104, 0x1000108] {
            sgx_io.as_view().relaxed().write32(sgx_io.as_view().relaxed().read32(offset) | 1, offset);
        }
        sgx_io.as_view().relaxed().write32(6, 0xd06030);
        drop(sgx_guard);
        // SAFETY: The region validation guarantees a full aligned page. The
        // coprocessor is stopped, so these host-owned bootstrap fields are
        // not concurrently accessed by firmware.
        unsafe {
            handoff
                .ptr()
                .cast::<u64>()
                .write_volatile(0x4b1d000000000002);
            handoff
                .ptr()
                .add(0x18)
                .cast::<u32>()
                .write_volatile(u32::MAX);
            handoff.ptr().add(0x640).cast::<u64>().write_volatile(0);
            for off in (0..PAGE_SIZE as usize).step_by(64) {
                core::arch::asm!("dc cvac, {addr}", addr = in(reg) handoff.ptr().add(off));
            }
        }
        crate::mem::sync();

        let notifications = Arc::pin_init(Notifications::new(), GFP_KERNEL)?;
        let data = Arc::new(
            BootData {
                dev: pdev.into(),
                notifications: notifications.clone(),
            },
            GFP_KERNEL,
        )?;
        let mut rtkit = KBox::pin(
            rtkit::RtKit::<BootOps>::new(dev, None, 0, data)?,
            GFP_KERNEL,
        )?;
        rtkit.as_mut().set_early_crashlog();
        let asc_guard = asc.try_access().ok_or(ENODEV)?;
        asc_guard.as_view().relaxed().write32(control | 0x10, 0x44);
        drop(asc_guard);
        crate::mem::sync();
        rtkit.as_mut().boot()?;
        for ep in [0x20, 0x21] {
            // Fails with EINVAL if the firmware did not advertise the endpoint.
            rtkit.as_mut().start_endpoint(ep)?;
        }
        dev_info!(
            dev,
            "G16: autonomous RTKit boot complete; firmware endpoints ready\n"
        );
        let mut firmware_space = crate::g16_vm::FirmwareSpace::attach(dev)?;
        firmware_space.alloc(
            crate::g16_fw::ROOT_ADDRESS,
            PAGE_SIZE as usize,
            crate::pgtable::prot::PROT_FW_SHARED_RO,
        )?;
        firmware_space.write(
            crate::g16_fw::ROOT_ADDRESS,
            &crate::g16_fw::RootPointers::bootstrap().encode(),
        )?;
        firmware_space.alloc(
            crate::g16_fw::BUNDLE_ADDRESS,
            crate::g16_fw::BUNDLE_SIZE + PAGE_SIZE as usize,
            crate::pgtable::prot::PROT_FW_PRIV_RW,
        )?;
        firmware_space.write(crate::g16_fw::BUNDLE_ADDRESS, &bundle)?;
        // All root support objects and receive rings occupy this shared extent.
        firmware_space.alloc(
            0xffff_fc20_0003_0000,
            0xc4000,
            crate::pgtable::prot::PROT_FW_SHARED_RW,
        )?;
        let pointers = crate::g16_fw::RootPointers::bootstrap();
        for va in [pointers.region_a, pointers.region_c] {
            firmware_space.alloc(
                va,
                PAGE_SIZE as usize,
                crate::pgtable::prot::PROT_FW_SHARED_RO,
            )?;
        }
        for va in [pointers.status_a, pointers.status_b] {
            firmware_space.write(va + 4, &1u32.to_le_bytes())?;
        }
        firmware_space.write(pointers.region_c, &region_c)?;
        // Extra policy objects, computed descriptors, PB/UMA tables and
        // scheduler counters begin zero, exactly as the current shim.
        firmware_space.alloc(
            0xffff_fc20_c050_0000,
            PAGE_SIZE as usize,
            crate::pgtable::prot::PROT_FW_PRIV_RW,
        )?;
        for r in &platform.registers {
            let prot = if r.flags & 2 != 0 {
                crate::pgtable::prot::PROT_FW_MMIO_RW
            } else {
                crate::pgtable::prot::PROT_FW_MMIO_RO
            };
            if r.slot == 3 {
                for (i, physical) in platform.mcc.iter().enumerate() {
                    firmware_space.map_external(
                        r.address + i as u64 * u64::from(r.stride),
                        *physical,
                        r.stride as usize,
                        prot,
                    )?;
                }
            } else {
                let offset = r.physical & (PAGE_SIZE - 1);
                let size = (offset + u64::from(r.size) + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
                firmware_space.map_external(
                    r.address - offset,
                    r.physical - offset,
                    size as usize,
                    prot,
                )?;
            }
        }
        // Work-channel cursor pages use shared attributes, unlike other MMIO.
        firmware_space.map_external(
            0xffff_fc20_0002_0000,
            0x300d60000,
            PAGE_SIZE as usize,
            crate::pgtable::prot::PROT_FW_SHARED_RO,
        )?;
        firmware_space.map_external(
            0xffff_fc20_0002_8000,
            0x300d64000,
            PAGE_SIZE as usize,
            crate::pgtable::prot::PROT_FW_SHARED_RW,
        )?;
        for (va, size) in FIRMWARE_HEAPS {
            firmware_space.alloc(va, size, crate::pgtable::prot::PROT_FW_PRIV_RW)?;
        }
        for va in SHARED_PAGES {
            firmware_space.alloc(
                va,
                PAGE_SIZE as usize,
                crate::pgtable::prot::PROT_FW_SHARED_RW,
            )?;
        }
        firmware_space.alloc(
            0xffff_fc20_001b_8000,
            PAGE_SIZE as usize,
            crate::pgtable::prot::PROT_FW_SHARED_RO,
        )?;
        // Shared support version and read-only resume records also serve the
        // render support object; construct them before the firmware starts.
        firmware_space.write(0xffff_fc20_001b_0000, &1u32.to_le_bytes())?;
        for record in 0..96u64 {
            firmware_space.write(
                0xffff_fc20_001b_8000 + record * 0x40,
                &0x0000_0070_0000_1822u64.to_le_bytes(),
            )?;
        }
        for (va, size) in [
            (0x70_0000_0000, 0x8c000),
            (0x70_0009_0000, 0xc0000),
            (0x70_0015_8000, 0x94000),
            (0x70_001f_0000, 0x84000),
        ] {
            address_space.alloc_low(va, size, crate::pgtable::prot::PROT_GPU_SHARED_RO)?;
        }
        for (low, high) in [
            (0x70_0027_8000, 0xffff_fc20_0012_8000),
            (0x70_0028_0000, 0xffff_fc20_0013_0000),
        ] {
            let pa = firmware_space.physical(high)?;
            address_space.low.map_pages(
                low..low + PAGE_SIZE,
                pa,
                crate::pgtable::prot::PROT_GPU_SHARED_RW,
                false,
            )?;
            if address_space.low.translate(low)? != Some(pa) {
                return Err(EIO);
            }
        }
        firmware_space.alloc_render_support()?;
        use crate::g16_compute as compute;
        firmware_space.alloc(
            compute::QUEUE,
            0xc000,
            crate::pgtable::prot::PROT_FW_PRIV_RW,
        )?;
        firmware_space.alloc(
            compute::SHARED,
            0x4000,
            crate::pgtable::prot::PROT_FW_SHARED_RW,
        )?;
        let render = crate::g16_render::Addresses::bootstrap();
        // The Work register windows are firmware-owned pages exposed through
        // context zero's low root. Replace the initial zero backing explicitly.
        for (alias, work) in [
            (render.tiling_register_alias, render.tiling_work),
            (render.fragment_register_alias, render.fragment_work),
        ] {
            let base = alias & !(PAGE_SIZE - 1);
            let pa = firmware_space.physical(work & !(PAGE_SIZE - 1))?;
            address_space.low.unmap_pages(base..base + PAGE_SIZE)?;
            address_space.low.map_pages(
                base..base + PAGE_SIZE,
                pa,
                crate::pgtable::prot::PROT_GPU_SHARED_RW,
                false,
            )?;
        }
        address_space.sync();
        // Reserve the transport before firmware can cache a missing mapping.
        for (va, size, access) in [
            (
                0xffff_fc20_c000_0000,
                0x8000,
                crate::pgtable::prot::PROT_FW_PRIV_RW,
            ),
            (
                0xffff_fc20_0001_0000,
                0x8000,
                crate::pgtable::prot::PROT_FW_SHARED_RW,
            ),
            (
                0xffff_fc20_c001_0000,
                0x8000,
                crate::pgtable::prot::PROT_FW_PRIV_RW,
            ),
            (
                0xffff_fc20_0000_0000,
                0x4000,
                crate::pgtable::prot::PROT_FW_SHARED_RW,
            ),
            (
                0xffff_fc20_c050_8000,
                0x4000,
                crate::pgtable::prot::PROT_FW_SHARED_RW,
            ),
            (
                0xffff_fc20_c052_0000,
                0x4000,
                crate::pgtable::prot::PROT_FW_PRIV_RW,
            ),
            (
                0xffff_fc20_0013_8000,
                0x4000,
                crate::pgtable::prot::PROT_FW_SHARED_RW,
            ),
        ] {
            firmware_space.alloc(va, size, access)?;
        }
        use crate::g16_fw::queue as q;
        firmware_space.write(BOOT_POINTERS, &q::pointers(0x500, 2))?;
        firmware_space.write(BOOT_JOBS, &q::jobs(BOOT_JOBS))?;
        firmware_space.write(BOOT_CONTEXT, &q::context())?;
        firmware_space.write(
            BOOT_QUEUE,
            &q::Queue {
                pointers: BOOT_POINTERS,
                ring: BOOT_RING,
                jobs: BOOT_JOBS,
                private: BOOT_QUEUE + 0xb0,
                context: BOOT_CONTEXT,
                uuid: 1,
            }
            .encode(),
        )?;
        firmware_space.write(BOOT_BARRIER, &q::barrier(q::STAMP, 0, 0, 1, 1))?;
        for off in [0, 8] {
            firmware_space.write(BOOT_RING + off, &BOOT_BARRIER.to_le_bytes())?;
        }
        firmware_space.sync();
        let mut opening_client = crate::g16_vm::AddressSpace::new()?;
        opening_client.alloc_low(
            0x70_0000_0000,
            0x400000,
            crate::pgtable::prot::PROT_GPU_SHARED_RW,
        )?;
        opening_client.alloc_low(
            crate::g16_fw::OPERAND_TABLE,
            0x10000,
            crate::pgtable::prot::PROT_GPU_SHARED_RW,
        )?;
        for index in 0..20 {
            opening_client.alloc_low(
                crate::g16_fw::operand_page(index * 512),
                0x200000,
                crate::pgtable::prot::PROT_GPU_SHARED_RW,
            )?;
        }
        for first in (0..20 * 512).step_by(32) {
            let mut row = [0u8; 256];
            for i in 0..32 {
                row[i * 8..i * 8 + 8]
                    .copy_from_slice(&crate::g16_fw::operand_page(first + i).to_le_bytes());
            }
            opening_client.write_low(0x70_0000_0000 + (first * 8) as u64, &row)?;
        }
        for (base, size) in compute::private_ranges() {
            opening_client.alloc_low(base, size, crate::pgtable::prot::PROT_GPU_SHARED_RW)?;
        }
        for buffer in 0..compute::BUFFER_COUNT {
            for first in (0..compute::BUFFER_SIZE / 0x1000).step_by(32) {
                let mut row = [0; 256];
                for i in 0..32 {
                    let va = compute::BUFFER_BASE
                        + buffer as u64 * compute::BUFFER_STRIDE
                        + (first + i) as u64 * 0x1000;
                    row[i * 8..i * 8 + 8].copy_from_slice(&va.to_le_bytes());
                }
                opening_client.write_low(
                    compute::DIRECTORY
                        + (buffer * (compute::BUFFER_SIZE / 0x1000) + first) as u64 * 8,
                    &row,
                )?;
            }
        }
        firmware_space.write_live(compute::SUPPORT, &compute::support())?;
        opening_client.sync();
        dev_info!(
            dev,
            "G16: Linux-built root/main descriptors mapped at {:#x}; not yet published\n",
            crate::g16_fw::ROOT_ADDRESS
        );
        let diagnostics = KBox::init(try_init!(Diagnostics {
            wedge_since: None,
            wedge_sampled: None,
            wedge_signature: 0,
            wedge_reported: false,
            // Full capacity now: retirement logs without allocating.
            retire_log: KVec::with_capacity(render_runtime::RETIRE_LOG, GFP_KERNEL)?,
            retire_next: 0,
            handovers: {
                let mut v = KVec::new();
                v.extend_with(64, None, GFP_KERNEL)?;
                v
            },
            lane_events: KVec::new(),
            stale_event_tags: 0,
            stale_live_tags: 0,
            stale_live_own_tags: 0,
            dependency_waits: 0,
            stale_recent: {
                let mut v = KVec::new();
                v.extend_with(64, (0, 0, 0, 0, 0, false), GFP_KERNEL)?;
                v
            },
            stale_next: 0,
            pressure_reported: core::sync::atomic::AtomicU64::new(0),
            pressure_seen: core::sync::atomic::AtomicU64::new(0),
            hold_key: core::sync::atomic::AtomicU64::new(0),
            hold_since_ns: core::sync::atomic::AtomicI64::new(0),
            hold_near_misses: core::sync::atomic::AtomicU32::new(0),
        }), GFP_KERNEL)?;
        let mut session = Self {
            dev: pdev.into(),
            notifications,
            _rtkit: rtkit,
            _asc: asc,
            _sgx: sgx,
            _handoff: handoff,
            _address_space: Some(address_space),
            _firmware_space: Some(firmware_space),
            _opening_client: Some(opening_client),
            timestamp_ranges: KVec::new(),
            opening_notification_pending: true,
            gpu_revision,
            core_mask: platform.core_mask,
            maximum_frequency_khz: platform.max_frequency() * 1000,
            tvb_pools: KVec::new(),
            encodings: render_runtime::Encodings::new()?,
            render_publication: crate::module_parameters::fw_counter_start.value(),
            diagnostics,
            next_work_va: render_runtime::WORK_BASE,
            next_object_va: render_runtime::OBJECT_BASE,
            work_arenas: [0..0, 0..0],
            free_work: [KVec::new(), KVec::new()],
            compute_publication: crate::module_parameters::fw_counter_start.value(),
            queue_events: Arc::new(core::sync::atomic::AtomicU64::new(0), GFP_KERNEL)?,
            retired_queues: Arc::pin_init(kernel::new_mutex!(KVec::new()), GFP_KERNEL)?,
            last_scan: KBox::new(Default::default(), GFP_KERNEL)?,
            queue_regions: 0,
            destroying_queues: KVec::new(),
            free_queues: KVec::new(),
            render_failed: false,
            flights: KVec::with_capacity(render_runtime::MAX_FLIGHTS, GFP_KERNEL)?,
            retired: KVec::with_capacity(render_runtime::MAX_FLIGHTS, GFP_KERNEL)?,
            publications: KVec::with_capacity(2 * render_runtime::MAX_FLIGHTS, GFP_KERNEL)?,
            ledger: KBox::new(render_runtime::Ledger::with_capacity(render_runtime::MAX_FLIGHTS)?, GFP_KERNEL)?,
            progress: None,
            batch_works: 0,
            context_roots: [None; 64],
            context_users: [0; 64],
            render_support: kernel::sync::Arc::new(
                core::sync::atomic::AtomicU64::new(0),
                GFP_KERNEL,
            )?,
        };
        if crate::module_parameters::fw_trace.value() & 1 != 0 {
            // Region C starts with the firmware KTrace mask, as in the shim.
            session
                ._firmware_space
                .as_mut()
                .ok_or(EIO)?
                .write_live(0xffff_fc20_000f_8000, &u32::MAX.to_le_bytes())?;
        }
        session.publish(pdev)?;
        session.open_control(pdev)?;
        session.submit_barrier(pdev)?;
        Ok(session)
    }

    fn submit_barrier(&mut self, pdev: &platform::Device<Core<'_>>) -> Result {
        use crate::g16_fw::queue as q;
        let dev = pdev.as_ref();
        let firmware = self._firmware_space.as_mut().ok_or(EINVAL)?;
        let sgx_guard = self._sgx.try_access().ok_or(ENODEV)?;
        let sgx = &*sgx_guard;
        for addr in [0xffff_fc20_0003_f190, 0xffff_fc20_0003_f198] {
            firmware.write_live(addr, &0u32.to_le_bytes())?;
        }
        let before = (
            sgx.as_view().relaxed().read32(0xd64040),
            sgx.as_view().relaxed().read32(0xd64048),
            sgx.as_view().relaxed().read32(0xd60020),
        );
        if before != (0, 0, 0) {
            return Err(EBUSY);
        }
        // Pair-zero fragment channel, with two satisfied barrier items.
        firmware.write_live(
            crate::g16_fw::BUNDLE_ADDRESS + 0x16240,
            &q::channel(BOOT_QUEUE, 2, 1, 1, true, 1),
        )?;
        sgx.as_view().relaxed().write32(1, 0xd60020);
        crate::mem::sync();
        drop(sgx_guard);
        let mut committed = false;
        for _ in 0..3000 {
            let cursor = firmware.read_u32(crate::g16_fw::CONTROL_DATA + 0x48)?;
            let aux = firmware.read_u32(crate::g16_fw::CONTROL_AUX)?;
            if cursor == 20 * 8 && (aux == 0 || aux == 1) {
                committed = true;
                break;
            }
            kernel::time::delay::fsleep(kernel::time::Delta::from_millis(1));
        }
        if !committed {
            dev_err!(
                dev,
                "G16: first-work registration stalled at {:#x}/{:#x}\n",
                firmware.read_u32(crate::g16_fw::CONTROL_DATA + 0x48)?,
                firmware.read_u32(crate::g16_fw::CONTROL_AUX)?
            );
            return Err(ETIMEDOUT);
        }
        self._rtkit
            .as_mut()
            .send_message(0x21, 0x0083_0000_0000_0001)?;
        for _ in 0..3000 {
            let sgx_guard = self._sgx.try_access().ok_or(ENODEV)?;
            let sgx = &*sgx_guard;
            if sgx.as_view().relaxed().read32(0xd64040) == 1
                && sgx.as_view().relaxed().read32(0xd64048) == 1
                && firmware.read_u32(BOOT_POINTERS + 0x30)? == 2
            {
                // Barriers consume the inner ring without advancing its
                // separate completion cursor, as in the working source path.
                dev_info!(dev, "G16: barrier channel retired; two items consumed\n");
                return Ok(());
            }
            drop(sgx_guard);
            kernel::time::delay::fsleep(kernel::time::Delta::from_millis(1));
        }
        let sgx_guard = self._sgx.try_access().ok_or(ENODEV)?;
        let sgx = &*sgx_guard;
        dev_err!(
            dev,
            "G16: barrier timeout; channel {}/{}/{} item done/read {}/{}\n",
            sgx.as_view().relaxed().read32(0xd64040),
            sgx.as_view().relaxed().read32(0xd64048),
            sgx.as_view().relaxed().read32(0xd60020),
            firmware.read_u32(BOOT_POINTERS)?,
            firmware.read_u32(BOOT_POINTERS + 0x30)?
        );
        Err(ETIMEDOUT)
    }

    fn open_control(&mut self, pdev: &platform::Device<Core<'_>>) -> Result {
        let dev = pdev.as_ref();
        let firmware = self._firmware_space.as_mut().ok_or(EINVAL)?;
        let client = self._opening_client.as_ref().ok_or(EINVAL)?;
        let res = dev
            .of_node()
            .ok_or(EINVAL)?
            .reserved_mem_region_to_resource_byname(c_str!("ttbs"))?;
        // SAFETY: This is the loader's validated context table reservation.
        let ttbs = unsafe { io::mem::Mem::try_new(res, io::mem::MemFlag::WB.into())? };
        unsafe {
            // Context one has ASID one, with independently allocated roots.
            ttbs.ptr()
                .add(16)
                .cast::<u64>()
                .write_volatile(client.low.ttb() | (1 << 48) | 1);
            ttbs.ptr()
                .add(24)
                .cast::<u64>()
                .write_volatile(client.high.ttb() | (1 << 48) | 1);
            core::arch::asm!("dc cvac, {addr}", addr = in(reg) ttbs.ptr());
        }
        crate::mem::sync();
        firmware.alloc(
            crate::g16_fw::CONTROL_DATA,
            PAGE_SIZE as usize,
            crate::pgtable::prot::PROT_FW_PRIV_RW,
        )?;
        firmware.alloc(
            crate::g16_fw::CONTROL_AUX,
            PAGE_SIZE as usize,
            crate::pgtable::prot::PROT_FW_SHARED_RW,
        )?;
        firmware.write(crate::g16_fw::CONTROL_DATA, &crate::g16_fw::opening_flist())?;
        // Own distinct shared pages for the mapping-notification state/ring.
        firmware.alloc(
            0xffff_fc21_8100_0000,
            PAGE_SIZE as usize,
            crate::pgtable::prot::PROT_FW_SHARED_RW,
        )?;
        firmware.alloc(
            0xffff_fc21_8100_8000,
            PAGE_SIZE as usize,
            crate::pgtable::prot::PROT_FW_SHARED_RW,
        )?;
        firmware.sync();
        firmware.write_live(
            crate::g16_fw::BUNDLE_ADDRESS + 0x1df40,
            &crate::g16_fw::opening_control(),
        )?;
        let sgx_guard = self._sgx.try_access().ok_or(ENODEV)?;
        let sgx = &*sgx_guard;
        let before = (
            sgx.as_view().relaxed().read32(0xd640c0),
            sgx.as_view().relaxed().read32(0xd640c8),
            sgx.as_view().relaxed().read32(0xd60060),
        );
        if before != (1, 1, 1) {
            dev_err!(dev, "G16: unexpected opening cursors {:?}\n", before);
            return Err(EIO);
        }
        sgx.as_view().relaxed().write32(3, 0xd60060);
        drop(sgx_guard);
        for (address, value) in [
            (0xffff_fc20_0003_4f88, 0xffff_fc21_8100_0000u64),
            (0xffff_fc20_0003_4f90, 0xffff_fc21_8100_8000u64),
        ] {
            firmware.write_live(address, &value.to_le_bytes())?;
        }
        firmware.write_live(0xffff_fc20_0003_4fd4, &1u32.to_le_bytes())?;
        crate::mem::tlbi_asid(1);
        crate::mem::sync();
        kernel::time::delay::fsleep(kernel::time::Delta::from_millis(100));
        self._rtkit
            .as_mut()
            .send_message(0x21, 0x0084_0000_0000_0011)?;
        for _ in 0..3000 {
            let sgx_guard = self._sgx.try_access().ok_or(ENODEV)?;
            let sgx = &*sgx_guard;
            let cursors = (
                sgx.as_view().relaxed().read32(0xd640c0),
                sgx.as_view().relaxed().read32(0xd640c8),
                sgx.as_view().relaxed().read32(0xd60060),
            );
            if cursors == (3, 3, 3) && firmware.read_u32(0xffff_fc20_0003_f19c)? == 1 {
                dev_info!(
                    dev,
                    "G16: opening control registration retired; cursors {:?}\n",
                    cursors
                );
                return Ok(());
            }
            drop(sgx_guard);
            kernel::time::delay::fsleep(kernel::time::Delta::from_millis(1));
        }
        let sgx_guard = self._sgx.try_access().ok_or(ENODEV)?;
        let sgx = &*sgx_guard;
        dev_err!(
            dev,
            "G16: control timeout; cursors {}/{}/{} completion {}\n",
            sgx.as_view().relaxed().read32(0xd640c0),
            sgx.as_view().relaxed().read32(0xd640c8),
            sgx.as_view().relaxed().read32(0xd60060),
            firmware.read_u32(0xffff_fc20_0003_f19c)?
        );
        Err(ETIMEDOUT)
    }

    fn publish(&mut self, pdev: &platform::Device<Core<'_>>) -> Result {
        let dev = pdev.as_ref();
        let address_space = self._address_space.as_ref().ok_or(EINVAL)?;
        let firmware_space = self._firmware_space.as_ref().ok_or(EINVAL)?;
        let res = dev
            .of_node()
            .ok_or(EINVAL)?
            .reserved_mem_region_to_resource_byname(c_str!("ttbs"))?;
        // SAFETY: This is the validated reserved hardware context table.
        let ttbs = unsafe { io::mem::Mem::try_new(res, io::mem::MemFlag::WB.into())? };
        let sgx_guard = self._sgx.try_access().ok_or(ENODEV)?;
        let sgx = &*sgx_guard;
        let main = crate::g16_fw::MainConfig::bootstrap(crate::g16_fw::BUNDLE_ADDRESS);
        for channel in &main.channels[..12] {
            for va in channel.state {
                let offset = if va >= 0xffff_fc20_0002_8000 {
                    0xd64000 + (va - 0xffff_fc20_0002_8000) as usize
                } else {
                    0xd60000 + (va - 0xffff_fc20_0002_0000) as usize
                };
                sgx.try_write32(0, offset)?;
                if sgx.try_read32(offset)? != 0 {
                    return Err(EIO);
                }
            }
        }
        // Slot zero (opcode 0x16) is already embedded in the main object.
        for (offset, value) in [(0xd640c0, 0), (0xd640c8, 0), (0xd60060, 1)] {
            sgx.try_write32(value, offset)?;
            if sgx.try_read32(offset)? != value {
                return Err(EIO);
            }
        }
        drop(sgx_guard);
        // Slot zero's high root is empty. Firmware keeps using its separate
        // bootstrap root from the loader reservation, not this context entry.
        unsafe {
            ttbs.ptr()
                .cast::<u64>()
                .write_volatile(address_space.low.ttb() | 1);
            ttbs.ptr()
                .add(8)
                .cast::<u64>()
                .write_volatile(address_space.high.ttb() | 1);
            core::arch::asm!("dc cvac, {addr}", addr = in(reg) ttbs.ptr());
        }
        crate::mem::sync();
        crate::mem::tlbi_asid(0);
        crate::mem::sync();
        self._rtkit
            .as_mut()
            .send_message(0x20, 0x0081_0c20_0010_0000)?;
        dev_info!(dev, "G16: published Linux-built initialization graph\n");
        for _ in 0..3000 {
            if firmware_space.read_u32(crate::g16_fw::RootPointers::bootstrap().status_a + 0x14)?
                == 1
            {
                dev_info!(dev, "G16: firmware accepted initialization graph\n");
                return Ok(());
            }
            kernel::time::delay::fsleep(kernel::time::Delta::from_millis(1));
        }
        dev_err!(
            dev,
            "G16: initialization timeout; status {:#x}/{:#x}/{:#x}\n",
            firmware_space.read_u32(0xffff_fc20_0003_f190)?,
            firmware_space.read_u32(0xffff_fc20_0003_f194)?,
            firmware_space.read_u32(0xffff_fc20_0003_f198)?
        );
        Err(ETIMEDOUT)
    }
}
