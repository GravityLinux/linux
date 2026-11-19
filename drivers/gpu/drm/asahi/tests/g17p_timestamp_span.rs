// SPDX-License-Identifier: GPL-2.0-only
#[path = "../g17p_timestamp_span.rs"]
mod span;
use span::{Mapping, PAGE};

#[test]
fn covers_both_sides_of_a_page_boundary() {
    assert_eq!(Mapping::new(PAGE - 8, 16, 3 * PAGE), Some(Mapping {
        bo_offset: 0, size: 2 * PAGE, bias: PAGE - 8,
    }));
    assert_eq!(Mapping::new(PAGE + 8, PAGE + 16, 3 * PAGE), Some(Mapping {
        bo_offset: PAGE, size: 2 * PAGE, bias: 8,
    }));
}

#[test]
fn exact_end_and_tiny_ranges_do_not_add_a_page() {
    assert_eq!(Mapping::new(8, 8, 3 * PAGE), Some(Mapping {bo_offset: 0, size: PAGE, bias: 8}));
    assert_eq!(Mapping::new(3 * PAGE - 8, 8, 3 * PAGE), Some(Mapping {
        bo_offset: 2 * PAGE, size: PAGE, bias: PAGE - 8,
    }));
    assert_eq!(Mapping::new(PAGE, PAGE, 3 * PAGE), Some(Mapping {bo_offset: PAGE, size: PAGE, bias: 0}));
}

#[test]
fn legacy_page_aligned_ranges_keep_their_original_mapping() {
    for offset in (0..16 * PAGE).step_by(PAGE as usize) {
        for size in (PAGE..=16 * PAGE - offset).step_by(PAGE as usize) {
            assert_eq!(Mapping::new(offset,size,16 * PAGE),Some(Mapping {
                bo_offset: offset, size, bias: 0,
            }));
        }
    }
}

#[test]
fn rejects_empty_misaligned_outside_and_overflowing_ranges() {
    for (offset, size, length) in [(0,0,PAGE),(1,8,PAGE),(8,1,PAGE),
        (PAGE,8,PAGE),(PAGE-8,16,PAGE),(u64::MAX-7,16,u64::MAX),
        (0,u64::MAX-7,u64::MAX),(0,8,8)] {
        assert_eq!(Mapping::new(offset,size,length),None);
    }
}
