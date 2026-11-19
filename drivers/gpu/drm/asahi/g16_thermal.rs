// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! Bounds and units for the firmware fast-die thermal policy.

/// Convert degrees C to firmware centidegrees, permitting only a lower limit.
/// Zero keeps the existing 110 C limit. Release is still supplied by the ADT.
pub(crate) fn fast_die_limits(override_c: u32, release_c: u32) -> Option<(u32, u32)> {
    let limit_c = if override_c == 0 { 110 } else { override_c };
    if !(50..=110).contains(&limit_c) || !(30..limit_c).contains(&release_c) {
        return None;
    }
    Some((limit_c * 100, release_c * 100))
}

#[cfg(test)]
mod tests {
    use super::fast_die_limits;

    #[test]
    fn existing_and_air_limits() {
        assert_eq!(fast_die_limits(0, 80), Some((11000, 8000)));
        assert_eq!(fast_die_limits(95, 80), Some((9500, 8000)));
        assert_eq!(fast_die_limits(110, 80), Some((11000, 8000)));
        assert_eq!(fast_die_limits(50, 30), Some((5000, 3000)));
    }

    #[test]
    fn reject_bad_units_and_release() {
        for limit in [1, 49, 111, 125, 9500, 95000, u32::MAX] {
            assert_eq!(fast_die_limits(limit, 80), None);
        }
        for release in [0, 29, 95, 110, u32::MAX] {
            assert_eq!(fast_die_limits(95, release), None);
        }
        assert_eq!(fast_die_limits(0, 110), None);
    }
}
