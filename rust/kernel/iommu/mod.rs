// SPDX-License-Identifier: GPL-2.0

//! Rust support related to IOMMU.

pub mod pgtable;

/// Map a read/write MMIO range into a bound device's current IOMMU domain.
///
/// # Safety
///
/// The caller must ensure that the physical range is valid for device MMIO,
/// the IOVA range is unused, and the device retains this domain while the
/// mapping is in use. The caller must arrange unmapping before reusing it.
pub unsafe fn map_device_mmio(
    dev: &crate::device::Device<crate::device::Bound>,
    iova: usize,
    paddr: u64,
    size: usize,
    flags: crate::alloc::Flags,
) -> crate::error::Result {
    // SAFETY: dev is a live, bound device. The domain pointer is used only
    // during this call; its mapping lifetime is the caller's responsibility.
    let domain = unsafe { crate::bindings::iommu_get_domain_for_dev(dev.as_raw()) };
    if domain.is_null() {
        return Err(crate::error::code::ENODEV);
    }
    // SAFETY: The caller guarantees valid physical/IOVA ranges and domain
    // lifetime. The protection bits intentionally describe device MMIO.
    crate::error::to_result(unsafe {
        crate::bindings::iommu_map(
            domain,
            iova,
            paddr,
            size,
            (crate::bindings::IOMMU_READ | crate::bindings::IOMMU_WRITE |
             crate::bindings::IOMMU_MMIO) as i32,
            flags.as_raw(),
        )
    })
}
