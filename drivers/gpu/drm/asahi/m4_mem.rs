// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! GPU page-table publication, matching the Python shim's native UAT sync.

#[inline(always)]
pub(crate) fn sync() {
    // SAFETY: A system memory barrier changes no memory ownership.
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

#[inline(always)]
pub(crate) fn tlbi_all() {
    // SAFETY: AGX shares the outer-shareable TLB invalidation domain.
    unsafe {
        core::arch::asm!(
            ".arch armv8.4-a",
            "tlbi vmalle1os",
            options(nostack, preserves_flags)
        );
    }
}

#[inline(always)]
pub(crate) fn tlbi_asid(asid: u8) {
    // SAFETY: Invalidating a GPU ASID cannot expose stale translations.
    unsafe {
        core::arch::asm!(".arch armv8.4-a", "tlbi aside1os, {asid}",
            asid = in(reg) u64::from(asid) << 48, options(nostack, preserves_flags));
    }
}
