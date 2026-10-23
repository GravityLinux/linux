// SPDX-License-Identifier: GPL-2.0-only
// Included by test-deferred-unmap.py with the production g16_deferred.rs.
use core::ops::Range;
use g16_deferred::{admit, covered, overlap, ready, Overlap, Span, MAX_ENTRIES};

#[derive(Clone)]
struct M(Range<u64>, u32, bool);

impl Span for M {
    fn range(&self) -> Range<u64> {
        self.0.clone()
    }
    fn permissions(&self) -> u32 {
        self.1
    }
    fn closing(&self) -> bool {
        self.2
    }
}

const RW: u32 = 3;
const R: u32 = 1;

fn maps() -> Vec<M> {
    vec![M(0x4000..0x8000, RW, false), M(0x8000..0x10000, R, false), M(0x20000..0x24000, RW, true)]
}

#[test]
fn live_mappings_cover_as_before() {
    let m = maps();
    assert!(covered(&m, 0x4000, 0x4000, RW));
    assert!(covered(&m, 0x4000, 0xc000, R)); // spans two adjacent mappings
    assert!(!covered(&m, 0x4000, 0xc000, RW)); // second is read-only
    assert!(!covered(&m, 0x10000, 0x4000, R)); // hole
    assert!(!covered(&m, 0x4000, 0, R)); // empty
    assert!(!covered(&m, u64::MAX - 0x10, 0x4000, R)); // overflow
}

#[test]
fn closing_mappings_never_cover() {
    let m = maps();
    assert!(!covered(&m, 0x20000, 0x4000, R));
    assert!(!covered(&m, 0x21000, 0x10, R));
    let mut live = m.clone();
    live[2].2 = false;
    assert!(covered(&live, 0x20000, 0x4000, RW));
}

#[test]
fn overlap_distinguishes_closed_from_live_mappings() {
    let m = maps();
    assert_eq!(overlap(&m, 0x10000, 0x20000), Overlap::None);
    assert_eq!(overlap(&m, 0x20000, 0x24000), Overlap::Deferred);
    assert_eq!(overlap(&m, 0x1c000, 0x30000), Overlap::Deferred);
    assert_eq!(overlap(&m, 0x7000, 0x9000), Overlap::Live);
    // Any live overlap wins: the caller replaces it with the whole-VM rule.
    assert_eq!(overlap(&m, 0x4000, 0x30000), Overlap::Live);
    // Touching ends are not overlaps.
    assert_eq!(overlap(&m, 0x24000, 0x28000), Overlap::None);
    assert_eq!(overlap(&m, 0x1c000, 0x20000), Overlap::None);
}

#[test]
fn readiness_needs_every_fence() {
    assert!(ready([]));
    assert!(ready([true, true]));
    assert!(!ready([true, false, true]));
}

#[test]
fn the_entry_cap_falls_back_to_waiting() {
    assert!(admit(0));
    assert!(admit(MAX_ENTRIES - 1));
    assert!(!admit(MAX_ENTRIES));
}
