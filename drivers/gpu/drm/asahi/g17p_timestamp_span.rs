// SPDX-License-Identifier: GPL-2.0-only

//! Page coverage of a caller's logical timestamp object. This is host mapping
//! bookkeeping, not a firmware ABI structure.

pub(crate) const PAGE: u64 = 0x4000;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Mapping {
    pub(crate) bo_offset: u64,
    pub(crate) size: u64,
    pub(crate) bias: u64,
}

impl Mapping {
    pub(crate) fn new(offset: u64, size: u64, bo_size: u64) -> Option<Self> {
        let end = offset.checked_add(size)?;
        if size == 0 || (offset | size) & 7 != 0 || end > bo_size {
            return None;
        }
        let bo_offset = offset & !(PAGE - 1);
        let mapped_end = end.checked_add(PAGE - 1)? & !(PAGE - 1);
        if mapped_end > bo_size {
            return None;
        }
        Some(Self {
            bo_offset,
            size: mapped_end - bo_offset,
            bias: offset - bo_offset,
        })
    }
}
