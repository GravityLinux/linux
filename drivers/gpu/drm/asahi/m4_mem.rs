// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! GPU memory publication and translation invalidation.
//! Cache-clean loops queue maintenance; publication boundaries complete it.

#[inline(always)]
pub(crate) fn sync() {
    // SAFETY: A system memory barrier changes no memory ownership.
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

#[inline(always)]
pub(crate) fn tlbi_asid(asid: u8) {
    // SAFETY: Invalidating a GPU ASID cannot expose stale translations.
    unsafe {
        core::arch::asm!(".arch armv8.4-a", "tlbi aside1os, {asid}",
            asid = in(reg) u64::from(asid) << 48, options(nostack, preserves_flags));
    }
}

/// Invalidate only these GPU virtual pages, including global firmware mappings.
/// VA operands use 4 KiB units even though the GPU page size is 16 KiB.
/// The caller publishes PTE writes before this and completes the batch after it.
pub(crate) fn tlbi_range(asid: Option<u8>, start: u64, end: u64) {
    for va in (start..end).step_by(0x4000) {
        let operand = (va >> 12) & 0x0000_0fff_ffff_fffc;
        // SAFETY: These instructions only discard cached translations.
        unsafe {
            match asid {
                Some(asid) => core::arch::asm!(
                    ".arch armv8.4-a", "tlbi vae1os, {operand}",
                    operand = in(reg) operand | (u64::from(asid) << 48),
                    options(nostack, preserves_flags)),
                None => core::arch::asm!(
                    ".arch armv8.4-a", "tlbi vaae1os, {operand}",
                    operand = in(reg) operand, options(nostack, preserves_flags)),
            }
        }
    }
}
