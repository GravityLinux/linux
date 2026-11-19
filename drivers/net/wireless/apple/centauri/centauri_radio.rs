// SPDX-License-Identifier: GPL-2.0-only

//! Cold radio resource publication and generated Wi-Fi IPC.
//! All DMA addresses and buffers come from the owning PCI function's DMA API.

use core::{ffi::c_void, ptr, slice};
use kernel::prelude::*;

pub(crate) const WIFI_ARENA_BYTES: usize = 0x200000;
pub(crate) const WIFI_POOL_BYTES: usize = 0x800000;
const PAGE_BYTES: usize = 0x4000;
const HEADER_BYTES: usize = 0x68;
const METADATA_BYTES: usize = 0x960;
const COMPLETION_QUEUES: usize = 234;
const SUBMISSION_QUEUES: usize = 192;
pub(super) const DEVICE_COMPLETIONS: usize = 0x78;
pub(super) const DEVICE_SUBMISSIONS: usize = DEVICE_COMPLETIONS + COMPLETION_QUEUES * 2;
pub(super) const HOST_COMPLETIONS: usize = DEVICE_SUBMISSIONS + SUBMISSION_QUEUES * 2;
pub(super) const HOST_SUBMISSIONS: usize = HOST_COMPLETIONS + COMPLETION_QUEUES * 2;
pub(super) const ADMIN_SUBMISSION: usize = HOST_SUBMISSIONS + SUBMISSION_QUEUES * 2;
pub(super) const ADMIN_COMPLETION: usize = ADMIN_SUBMISSION + 16 * 16;
const AUXILIARY: usize = ADMIN_COMPLETION + 16 * 16;
const WIFI_FIRMWARE_STATE: usize = 0x08;
const WIFI_IPC_STATE: usize = 0x0c;
const WIFI_CONTEXT_LOW: usize = 0x10;
const WIFI_CONTEXT_HIGH: usize = 0x14;
const WIFI_WINDOW_LOW: usize = 0x18;
const WIFI_WINDOW_HIGH: usize = 0x1c;
const WIFI_WINDOW_LENGTH: usize = 0x20;
const WIFI_HANDSHAKE: usize = 0x1028;
const PCI_RESOURCE_LOW: u16 = 0xf88;
const PCI_RESOURCE_HIGH: u16 = 0xf8c;
const PCI_RESOURCE_BYTES: u16 = 0xf90;
const PCI_TIME_LOW: u16 = 0xff8;
const PCI_TIME_HIGH: u16 = 0xffc;
const WAIT_ATTEMPTS: usize = 600;
const WIFI_MSI_VECTORS: u32 = 16;
const WIFI_RESOURCE_ADDRESS_BITS: u32 = 32;

/// This config resource has a stricter limit than the context/DMA window.
/// Direct240/242 reject resource high word1;241 qualifies context high word1.
#[no_mangle]
pub(crate) extern "C" fn centauri_wifi_resource_dma_mask() -> u64 {
    (1 << WIFI_RESOURCE_ADDRESS_BITS) - 1
}

unsafe extern "C" {
    fn centauri_boot_read(bar: *mut c_void, offset: usize) -> u32;
    fn centauri_boot_write(bar: *mut c_void, offset: usize, value: u32);
    fn centauri_boot_sleep();
    fn centauri_boot_time_us() -> u64;
    fn centauri_radio_enable_dma(owner: *mut c_void);
    fn centauri_radio_enable_irqs(owner: *mut c_void, count: u32) -> i32;
    fn centauri_radio_config_read(owner: *mut c_void, offset: u16, value: *mut u32) -> i32;
    fn centauri_radio_config_write(owner: *mut c_void, offset: u16, value: u32) -> i32;
}

fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn dma_end(address: u64, bytes: usize) -> Result<u64> {
    if address & (PAGE_BYTES as u64 - 1) != 0 || bytes == 0 || bytes & (PAGE_BYTES - 1) != 0 {
        return Err(EINVAL);
    }
    address.checked_add(bytes as u64).filter(|end| *end <= 1 << 36).ok_or(EINVAL)
}

pub(crate) fn make_wifi_context(bytes: &mut [u8], dma: u64) -> Result {
    if bytes.len() != METADATA_BYTES {
        return Err(EINVAL);
    }
    dma_end(dma, PAGE_BYTES)?;
    bytes.fill(0);
    put16(bytes, 0, 0x0300);
    put16(bytes, 2, HEADER_BYTES as u16);
    for (field, target) in [
        (8, HEADER_BYTES),
        (0x10, DEVICE_COMPLETIONS),
        (0x18, DEVICE_SUBMISSIONS),
        (0x20, HOST_COMPLETIONS),
        (0x28, HOST_SUBMISSIONS),
        (0x34, ADMIN_COMPLETION),
        (0x3c, ADMIN_SUBMISSION),
        (0x58, AUXILIARY),
    ] {
        put64(bytes, field, dma + target as u64);
    }
    put16(bytes, 0x30, COMPLETION_QUEUES as u16);
    put16(bytes, 0x32, SUBMISSION_QUEUES as u16);
    put16(bytes, 0x44, 16);
    put16(bytes, 0x46, 16);
    // Qualified Wi-Fi header profile. Its individual bit roles remain open.
    put32(bytes, 0x60, 0x00500000);
    put32(bytes, HEADER_BYTES, 2);
    put32(bytes, HEADER_BYTES + 4, 2);
    Ok(())
}

fn wait(bar: *mut c_void, offset: usize, value: u32) -> Result {
    for _ in 0..WAIT_ATTEMPTS {
        // SAFETY: The private callers retain the validated PCI BAR throughout.
        if unsafe { centauri_boot_read(bar, offset) } == value {
            return Ok(());
        }
        // SAFETY: Probe runs in sleepable context with all DMA owners live.
        unsafe { centauri_boot_sleep() };
    }
    Err(ETIMEDOUT)
}

/// Publish a fresh Wi-Fi firmware resource and generated IPC context.
///
/// # Safety
/// C retains a live PCI owner/BAR and its disjoint coherent radio resource and
/// arena/pool allocations for this call and all subsequent device use. The
/// arena is exclusive until its context address is published. On any failure
/// C must stop/drain DMA before returning these allocations to the allocator.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_start(
    owner: *mut c_void,
    bar: *mut c_void,
    resource_dma: u64,
    resource_bytes: usize,
    arena: *mut u8,
    arena_dma: u64,
    arena_bytes: usize,
) -> i32 {
    let mut stage = "preflight";
    let mut start = || -> Result {
        if owner.is_null() || bar.is_null() || arena.is_null()
            || arena as usize & 3 != 0
            || arena_bytes < WIFI_ARENA_BYTES + WIFI_POOL_BYTES
        {
            return Err(EINVAL);
        }
        let resource_end = dma_end(resource_dma, resource_bytes)?;
        let arena_end = dma_end(arena_dma, arena_bytes)?;
        if resource_end > centauri_wifi_resource_dma_mask() + 1
            || !(resource_end <= arena_dma || arena_end <= resource_dma)
        {
            return Err(EINVAL);
        }
        let window = resource_dma.min(arena_dma);
        let window_bytes = u32::try_from(resource_end.max(arena_end) - window).map_err(|_| EINVAL)?;
        let mut prior_resource = 0;
        // SAFETY: Owner resolves this function only; output and BAR are live.
        let error = unsafe { centauri_radio_config_read(owner, PCI_RESOURCE_BYTES, &mut prior_resource) };
        if error < 0 {
            return Err(Error::from_errno(error));
        }
        if prior_resource != 0 || unsafe { centauri_boot_read(bar, WIFI_IPC_STATE) } != 0 {
            return Err(EBUSY);
        }
        // Direct237 isolates MSI Enable: disabling it at the endpoint changes
        // successful publication into firmware3/IPC1, without a DART fault.
        // Retain the independently qualified16-vector profile for now.
        stage = "MSI allocation";
        // SAFETY: C retains the PCI function and drains even partially
        // allocated IRQs before freeing any DMA on failure or remove.
        let error = unsafe { centauri_radio_enable_irqs(owner, WIFI_MSI_VECTORS) };
        if error < 0 {
            return Err(Error::from_errno(error));
        }
        // SAFETY: The new coherent allocation is not visible to firmware yet.
        unsafe { ptr::write_bytes(arena, 0, arena_bytes) };
        make_wifi_context(unsafe { slice::from_raw_parts_mut(arena, METADATA_BYTES) }, arena_dma)?;
        stage = "radio resource publication";
        // SAFETY: The adapter orders our initialized buffers before enabling
        // mastering; no resource pointer has been published before this point.
        unsafe { centauri_radio_enable_dma(owner) };
        let timestamp = unsafe { centauri_boot_time_us() };
        for (offset, value) in [
            (PCI_TIME_LOW, timestamp as u32),
            (PCI_TIME_HIGH, (timestamp >> 32) as u32),
            (PCI_RESOURCE_LOW, resource_dma as u32),
            (PCI_RESOURCE_HIGH, (resource_dma >> 32) as u32),
            (PCI_RESOURCE_BYTES, resource_bytes as u32),
        ] {
            // SAFETY: This live PCI function owns all five config fields.
            let error = unsafe { centauri_radio_config_write(owner, offset, value) };
            if error < 0 {
                return Err(Error::from_errno(error));
            }
        }
        stage = "radio firmware readiness";
        wait(bar, WIFI_FIRMWARE_STATE, 2)?;
        stage = "IPC handshake";
        // SAFETY: The write and all subsequent offsets fit the mapped BAR.
        unsafe { centauri_boot_write(bar, WIFI_HANDSHAKE, 1) };
        wait(bar, WIFI_IPC_STATE, 1)?;
        for (offset, value) in [
            (WIFI_WINDOW_LOW, window as u32),
            (WIFI_WINDOW_HIGH, (window >> 32) as u32),
            (WIFI_WINDOW_LENGTH, window_bytes),
            (WIFI_CONTEXT_LOW, arena_dma as u32),
            (WIFI_CONTEXT_HIGH, (arena_dma >> 32) as u32),
            (WIFI_HANDSHAKE, 2),
        ] {
            // SAFETY: Context and all receive-pool storage remain DMA-owned;
            // this window includes only this function's mapped allocations.
            unsafe { centauri_boot_write(bar, offset, value) };
        }
        stage = "IPC readiness";
        wait(bar, WIFI_IPC_STATE, 2)
    };
    match start() {
        Ok(()) => 0,
        Err(error) => {
            kernel::pr_err!("Wi-Fi {} failed: {}\n", stage, error.to_errno());
            error.to_errno()
        }
    }
}
