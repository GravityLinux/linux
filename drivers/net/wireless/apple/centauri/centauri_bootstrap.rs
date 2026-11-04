// SPDX-License-Identifier: GPL-2.0-only

//! Image transfers into Linux-owned bootstrap/control PCI functions.
//!
//! Port restart and radio initialization are still required before this is a
//! network driver. The two image stages have different doorbells and states.

use super::centauri_firmware::{Image, MAX_IMAGE_BYTES};
use core::{ffi::c_void, ptr, slice};
use kernel::prelude::*;

const BANK: usize = 0x8000;
const STATUS: usize = BANK;
const SUBMISSION_STATUS: usize = BANK + 0x40;
const SETUP_3C: usize = BANK + 0x3c;
const RECORD_LOW: usize = BANK + 0x44;
const RECORD_HIGH: usize = BANK + 0x48;
const RECORD_BYTES: usize = BANK + 0x4c;
const SETUP_78: usize = BANK + 0x78;
const HOST_PLATFORM_ID: usize = BANK + 0x7c;
const SUBMIT: usize = BANK + 0x1000;
const READY: u32 = 4;
const RUNTIME_SUBMIT: usize = BANK + 0x1070;
const RUNTIME_READY: u32 = 2;
const RUNTIME_AWAITING_IMAGE: u32 = 1;
const IMAGE_RECORD_BYTES: usize = 32;
const BOOTSTRAP_MSI_VECTORS: u32 = 1;
const CONTROL_MSI_VECTORS: u32 = 4;
const PCI_RESOURCE_LOW: u16 = 0xf88;
const PCI_RESOURCE_HIGH: u16 = 0xf8c;
const PCI_RESOURCE_BYTES: u16 = 0xf90;
const PCI_UNIX_US_LOW: u16 = 0xff8;
const PCI_UNIX_US_HIGH: u16 = 0xffc;
const BLUETOOTH_PCI_ID: u32 = 0x1903106b;

unsafe extern "C" {
    fn centauri_boot_read(bar: *mut c_void, offset: usize) -> u32;
    fn centauri_boot_write(bar: *mut c_void, offset: usize, value: u32);
    fn centauri_boot_publish_dma(owner: *mut c_void);
    fn centauri_boot_enable_irqs(owner: *mut c_void, count: u32) -> i32;
    fn centauri_boot_time_us() -> u64;
    fn centauri_boot_config_read(
        owner: *mut c_void,
        function: u8,
        offset: u16,
        value: *mut u32,
    ) -> i32;
    fn centauri_boot_config_write(owner: *mut c_void, function: u8, offset: u16, value: u32)
        -> i32;
    fn centauri_boot_sleep();
}

/// # Safety
/// C supplies an immutable image buffer and a writable size output. Neither
/// pointer may overlap, and both must remain valid for this entire call.
#[no_mangle]
unsafe extern "C" fn centauri_bootstrap_layout(
    image: *const u8,
    bytes: usize,
    allocation: *mut usize,
) -> i32 {
    if image.is_null() || allocation.is_null() || bytes == 0 || bytes > MAX_IMAGE_BYTES {
        return EINVAL.to_errno();
    }
    // SAFETY: The immutable buffer lifetime and extent are the caller contract.
    match Image::parse(unsafe { slice::from_raw_parts(image, bytes) }) {
        Ok(parsed) => {
            // SAFETY: C supplies a live exclusive size output.
            unsafe { *allocation = parsed.allocation_bytes };
            0
        }
        Err(_) => EINVAL.to_errno(),
    }
}

/// Return only directory metadata for an opaque current-boot radio resource.
///
/// # Safety
/// The immutable image and disjoint writable outputs remain live throughout
/// this call. This function never reads the firmware-produced allocation.
#[no_mangle]
unsafe extern "C" fn centauri_bootstrap_radio_layout(
    image: *const u8,
    bytes: usize,
    function: u8,
    offset: *mut usize,
    length: *mut usize,
) -> i32 {
    if image.is_null() || offset.is_null() || length.is_null()
        || bytes == 0 || bytes > MAX_IMAGE_BYTES
    {
        return EINVAL.to_errno();
    }
    // SAFETY: Caller owns the immutable, bounded prepared image.
    let result = Image::parse(unsafe { slice::from_raw_parts(image, bytes) })
        .and_then(|parsed| parsed.radio_region(function));
    match result {
        Ok(entry) => {
            // SAFETY: These exclusive, disjoint outputs are caller-owned.
            unsafe { *offset = entry.offset; *length = entry.size };
            0
        }
        Err(_) => EINVAL.to_errno(),
    }
}

/// # Safety
/// The image is immutable, output is an exclusive coherent allocation of
/// output_bytes, neither aliases, and neither is visible to device DMA yet.
#[no_mangle]
unsafe extern "C" fn centauri_bootstrap_allocation(
    image: *const u8,
    bytes: usize,
    output: *mut u8,
    output_bytes: usize,
) -> i32 {
    if image.is_null()
        || output.is_null()
        || bytes == 0
        || bytes > MAX_IMAGE_BYTES
        || output_bytes == 0
        || output_bytes > MAX_IMAGE_BYTES
    {
        return EINVAL.to_errno();
    }
    // SAFETY: Both slices are disjoint and live for this call by the contract.
    let image = unsafe { slice::from_raw_parts(image, bytes) };
    // SAFETY: C has not published the coherent allocation to the device yet.
    let output = unsafe { slice::from_raw_parts_mut(output, output_bytes) };
    match Image::parse(image).and_then(|parsed| parsed.initialize_allocation(output)) {
        Ok(()) => 0,
        Err(_) => EINVAL.to_errno(),
    }
}

/// # Safety
/// C passes a live PCI owner, a BAR mapping covering SUBMIT, an immutable firmware buffer,
/// and an exclusive coherent record allocation of at least 32 bytes. Bus
/// mastering is initially disabled. C holds these resources through completion
/// and clears bus mastering before freeing either allocation on every path.
#[no_mangle]
unsafe extern "C" fn centauri_bootstrap_image(
    owner: *mut c_void,
    bar: *mut c_void,
    image: *const u8,
    bytes: usize,
    image_dma: u64,
    record: *mut u8,
    record_dma: u64,
    platform: u32,
    runtime: bool,
    companion_dma: u64,
    companion_bytes: usize,
) -> i32 {
    let transfer = || -> Result {
        if owner.is_null()
            || bar.is_null()
            || image.is_null()
            || record.is_null()
            || bytes == 0
            || bytes > MAX_IMAGE_BYTES
            || (!runtime && platform == 0)
            || image_dma
                .checked_add(bytes as u64)
                .is_none_or(|end| end > 1 << 36)
            || record_dma
                .checked_add(IMAGE_RECORD_BYTES as u64)
                .is_none_or(|end| end > 1 << 36)
        {
            return Err(EINVAL);
        }
        // SAFETY: C owns the immutable buffer for this entire call, as required
        // above. Parse directory metadata only; payloads remain opaque.
        let firmware = unsafe { slice::from_raw_parts(image, bytes) };
        let parsed = Image::parse(firmware).map_err(|_| EINVAL)?;
        if runtime
            && (companion_bytes != parsed.allocation_bytes
                || companion_dma
                    .checked_add(companion_bytes as u64)
                    .is_none_or(|end| end > 1 << 36))
        {
            return Err(EINVAL);
        }
        // SAFETY: The checked BAR mapping is live throughout this call.
        if unsafe { centauri_boot_read(bar, STATUS) }
            != if runtime { RUNTIME_AWAITING_IMAGE } else { 0 }
        {
            return Err(EBUSY);
        }
        // A timed-out image may still retain a descriptor pointer. Never
        // re-enable mastering on reprobe merely because main status is zero.
        // Only the fresh zero submission state is qualified as unused here.
        // SAFETY: The submission status is inside the validated live BAR.
        if unsafe { centauri_boot_read(bar, SUBMISSION_STATUS) } != 0 {
            return Err(EBUSY);
        }
        if runtime {
            let mut identity = 0;
            // SAFETY: The live PCI owner resolves the peer and writes only
            // this exclusive output. Validate identity before enabling DMA.
            let error = unsafe { centauri_boot_config_read(owner, 2, 0, &mut identity) };
            if error < 0 {
                return Err(Error::from_errno(error));
            }
            if identity != BLUETOOTH_PCI_ID {
                return Err(ENODEV);
            }
        }
        // MSI must be enabled at the endpoint before image submission even
        // when completion is polled. Independent201 isolates its enable bit;
        // independent202 qualifies one vector for the initial image. Runtime
        // control retains its qualified four-vector profile.
        // SAFETY: The live PCI owner allocates/owns the exact vector range and
        // drains its handlers before releasing DMA resources on every path.
        let error = unsafe {
            centauri_boot_enable_irqs(
                owner,
                if runtime {
                    CONTROL_MSI_VECTORS
                } else {
                    BOOTSTRAP_MSI_VECTORS
                },
            )
        };
        if error < 0 {
            return Err(Error::from_errno(error));
        }
        let mut descriptor = [0; IMAGE_RECORD_BYTES];
        descriptor[..8].copy_from_slice(&image_dma.to_le_bytes());
        descriptor[8..16].copy_from_slice(&(bytes as u64).to_le_bytes());
        // SAFETY: The coherent record is exclusive and has not been published
        // to the device. Its backing storage outlives the hardware transaction.
        unsafe { ptr::copy_nonoverlapping(descriptor.as_ptr(), record, descriptor.len()) };
        // SAFETY: Publish initialized coherent memory before the doorbell;
        // each write is within the C-validated BAR mapping. Setup3c/78 zero
        // are qualified values; their individual roles remain unresolved.
        unsafe {
            centauri_boot_publish_dma(owner);
            let unix_us = centauri_boot_time_us();
            for function in if runtime { &[0, 2][..] } else { &[0][..] } {
                for (offset, value) in [
                    (PCI_UNIX_US_LOW, unix_us as u32),
                    (PCI_UNIX_US_HIGH, (unix_us >> 32) as u32),
                ] {
                    let error = centauri_boot_config_write(owner, *function, offset, value);
                    if error < 0 {
                        return Err(Error::from_errno(error));
                    }
                }
            }
            if runtime {
                for (offset, value) in [
                    (PCI_RESOURCE_LOW, companion_dma as u32),
                    (PCI_RESOURCE_HIGH, (companion_dma >> 32) as u32),
                    (PCI_RESOURCE_BYTES, companion_bytes as u32),
                ] {
                    let error = centauri_boot_config_write(owner, 0, offset, value);
                    if error < 0 {
                        return Err(Error::from_errno(error));
                    }
                }
            }
            if !runtime {
                centauri_boot_write(bar, SETUP_3C, 0);
                centauri_boot_write(bar, HOST_PLATFORM_ID, platform);
                centauri_boot_write(bar, SETUP_78, 0);
            }
            centauri_boot_write(bar, RECORD_LOW, record_dma as u32);
            centauri_boot_write(bar, RECORD_HIGH, (record_dma >> 32) as u32);
            centauri_boot_write(bar, RECORD_BYTES, IMAGE_RECORD_BYTES as u32);
            centauri_boot_write(bar, if runtime { RUNTIME_SUBMIT } else { SUBMIT }, 1);
        }
        // Sleepable probe context; bounded to approximately ten seconds. MSI
        // and restart requirements still need kernel hardware qualification.
        for _ in 0..2000 {
            // SAFETY: C retains the BAR and this runs in sleepable probe context.
            unsafe {
                if centauri_boot_read(bar, STATUS) == if runtime { RUNTIME_READY } else { READY } {
                    return Ok(());
                }
                centauri_boot_sleep();
            }
        }
        // SAFETY: Both status registers are within the still-live validated
        // BAR. Preserve the two distinct state values for fault diagnosis.
        let (status, submission) = unsafe {
            (
                centauri_boot_read(bar, STATUS),
                centauri_boot_read(bar, SUBMISSION_STATUS),
            )
        };
        kernel::pr_err!(
            "image transfer timeout: runtime={} status={:#x} submission={:#x}\n",
            runtime,
            status,
            submission
        );
        Err(ETIMEDOUT)
    };
    match transfer() {
        Ok(()) => 0,
        Err(error) => error.to_errno(),
    }
}
