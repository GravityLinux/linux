// SPDX-License-Identifier: GPL-2.0-only
// Copyright 2026 The Asahi Linux Contributors

//! Deferred unmap bookkeeping for GEM close.
//!
//! A live GEM close no longer waits for the VM's queued work. The object's
//! mappings stay in the page tables, marked closing, until every fence that
//! was pending at the close has signaled; then the reaper unmaps them.
//! Submissions already keep each mapped object (and so its pages) alive, so
//! deferral only keeps PTEs valid for work that may still use them. Closing
//! mappings no longer satisfy `covered`, and a bind over them waits only for
//! their own snapshot. Pure range logic, host-tested.

use core::ops::Range;

/// Most deferred closes one VM tracks. Past this a close drains as before.
pub(crate) const MAX_ENTRIES: usize = 4096;

/// A mapping as this logic sees it.
pub(crate) trait Span {
    fn range(&self) -> Range<u64>;
    fn permissions(&self) -> u32;
    fn closing(&self) -> bool;
}

/// Whether `items` (sorted by address, non-overlapping) cover
/// `start..start + size` with at least `permissions`, using only mappings
/// that are not closing.
pub(crate) fn covered<T: Span>(items: &[T], start: u64, size: u64, permissions: u32) -> bool {
    let Some(end) = start.checked_add(size) else { return false };
    if size == 0 {
        return false;
    }
    // The only candidate for `cursor` is the first mapping ending after it.
    let mut cursor = start;
    while cursor < end {
        let index = items.partition_point(|m| m.range().end <= cursor);
        match items.get(index) {
            Some(m)
                if m.range().contains(&cursor)
                    && !m.closing()
                    && m.permissions() & permissions == permissions =>
            {
                cursor = end.min(m.range().end);
            }
            _ => return false,
        }
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Overlap {
    /// Nothing mapped in the range.
    None,
    /// Only closing mappings: wait for their snapshots, then reap them.
    Deferred,
    /// At least one live mapping: the caller replaces it as before.
    Live,
}

/// Classify the mappings `start..end` overlaps.
pub(crate) fn overlap<T: Span>(items: &[T], start: u64, end: u64) -> Overlap {
    let mut kind = Overlap::None;
    for m in items {
        let r = m.range();
        if start < r.end && end > r.start {
            if !m.closing() {
                return Overlap::Live;
            }
            kind = Overlap::Deferred;
        }
    }
    kind
}

/// Whether a deferred close may be recorded on a VM already tracking
/// `entries`; otherwise the close drains as before.
pub(crate) fn admit(entries: usize) -> bool {
    entries < MAX_ENTRIES
}

/// Whether a snapshot is complete: every fence in it has signaled.
pub(crate) fn ready(signaled: impl IntoIterator<Item = bool>) -> bool {
    signaled.into_iter().all(|s| s)
}
