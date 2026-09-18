// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! Allocation of retained backing within a VM's private GPU aperture.
//!
//! Scratch grows from the low end and tiler backing from the high end. Both
//! consume the same remaining range, so neither needs a fixed reservation.
//! Clone the arena to stage a reservation before a fallible backing allocation.

use core::ops::Range;

#[derive(Clone, Default, Debug, PartialEq)]
pub(crate) struct Arena {
    remaining: Range<u64>,
}

impl Arena {
    pub(crate) fn new(remaining: Range<u64>) -> Self {
        Self { remaining }
    }

    pub(crate) fn alloc(&mut self, size: u64, align: u64) -> Option<Range<u64>> {
        if size == 0 || !align.is_power_of_two() {
            return None;
        }
        let start = self.remaining.start.checked_add(align - 1)? & !(align - 1);
        let end = start.checked_add(size)?;
        if end > self.remaining.end {
            return None;
        }
        self.remaining.start = end;
        Some(start..end)
    }

    pub(crate) fn alloc_back(&mut self, size: u64, align: u64) -> Option<Range<u64>> {
        if size == 0 || !align.is_power_of_two() {
            return None;
        }
        let start = self.remaining.end.checked_sub(size)? & !(align - 1);
        if start < self.remaining.start {
            return None;
        }
        self.remaining.end = start;
        Some(start..start + size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaved_scratch_and_tiler_exceed_old_growth_reservation() {
        let mut arena = Arena::new(0x1000_4000..0x3000_0000);
        let mut live = Vec::new();
        for _ in 0..400 {
            live.push(arena.alloc(0x14000, 0x4000).unwrap());
            let block = arena.alloc_back(0x28000, 0x8000).unwrap();
            assert_eq!(block.start & 0x7fff, 0);
            live.push(block);
        }
        live.sort_unstable_by_key(|r| r.start);
        assert!(live.windows(2).all(|pair| pair[0].end <= pair[1].start));
    }

    #[test]
    fn exhaustion_and_bad_requests_preserve_reservations() {
        let mut arena = Arena::new(0x4000..0x24000);
        assert_eq!(arena.alloc(0x10000, 0x4000), Some(0x4000..0x14000));
        let saved = arena.clone();
        for (size, align) in [(0x20000, 0x8000), (0, 1), (1, 0), (1, 3), (u64::MAX, 1)] {
            assert_eq!(arena.alloc(size, align), None);
            assert_eq!(arena.alloc_back(size, align), None);
            assert_eq!(arena, saved);
        }
        assert_eq!(arena.alloc_back(0x10000, 0x4000), Some(0x14000..0x24000));
        assert_eq!(arena.alloc(1, 1), None);
    }

    #[test]
    fn backing_failure_can_discard_staged_allocation() {
        let arena = Arena::new(0x4000..0x24000);
        let mut staged = arena.clone();
        assert_eq!(staged.alloc_back(0x10000, 0x8000), Some(0x10000..0x20000));
        let mut retry = arena;
        assert_eq!(retry.alloc_back(0x10000, 0x8000), Some(0x10000..0x20000));
        assert_eq!(retry, staged);
    }

    #[test]
    fn address_overflow_is_rejected() {
        let mut arena = Arena::new(u64::MAX - 1..u64::MAX);
        assert_eq!(arena.alloc(1, 0x4000), None);
        assert_eq!(arena.alloc(4, 1), None);
        assert_eq!(arena.alloc_back(4, 1), None);
    }
}
