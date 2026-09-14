// SPDX-License-Identifier: GPL-2.0-only

//! Wrapping firmware completion stamps, independent of software sequence counts.

const STEP: u32 = 0x100;

/// The first command has ordinal zero. Only the firmware stamp wraps; callers
/// retain the full software ordinal for accounting and command identities.
pub(crate) fn value(ordinal: u64, first: u32) -> u32 {
    first.wrapping_add((ordinal as u32).wrapping_mul(STEP))
}

/// Initialize a private completion destination to an uncompleted value. Zero
/// cannot serve as a sentinel: it is a valid stamp and sorts after high stamps.
pub(crate) fn previous(target: u32) -> u32 {
    target.wrapping_sub(STEP)
}

/// Queue admission bounds outstanding work to much less than half the stamp
/// range, making this signed modular comparison unambiguous across rollover.
pub(crate) fn reached(current: u32, target: u32) -> bool {
    (current.wrapping_sub(target) as i32) >= 0
}
