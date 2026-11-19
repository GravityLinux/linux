// SPDX-License-Identifier: GPL-2.0-only
//! Host checks for the kernel parser against synthetic Python encoder output.

#[path = "../../drivers/net/wireless/apple/centauri/centauri_firmware.rs"]
mod firmware;

use firmware::Image;
const PREPARED: &[u8] = include_bytes!(env!("CENTAURI_FW_TEST_IMAGE"));
const ALLOCATION: &[u8] = include_bytes!(env!("CENTAURI_FW_TEST_ALLOCATION"));

#[test]
fn python_packaging_and_allocation_agree() {
    let image = Image::parse(PREPARED).unwrap();
    let mut allocation = vec![0xa5; image.allocation_bytes];
    image.initialize_allocation(&mut allocation).unwrap();
    assert_eq!(allocation, ALLOCATION);
    assert!(image.initialize_allocation(&mut allocation[..64]).is_err());
}

#[test]
fn radio_resources_follow_function_and_allow_unaligned_source_offsets() {
    let image = Image::parse(PREPARED).unwrap();
    let wifi = image.radio_region(1).unwrap();
    let bluetooth = image.radio_region(2).unwrap();
    assert_eq!((wifi.tag, wifi.offset, wifi.size), (*b"msww", 0x4000, 0x8000));
    assert_eq!((bluetooth.tag, bluetooth.offset, bluetooth.size), (*b"mswb", 0xc000, 0x4000));
    for function in [0, 3, 7, 255] {
        assert!(image.radio_region(function).is_err());
    }
    let mut data = PREPARED.to_vec();
    let layout = u32::from_le_bytes(data[52..56].try_into().unwrap()) as usize;
    // The actual firmware's msww starts at640, not a page boundary. Only the
    // destination allocation/extent requires page geometry for normal DMA.
    data[layout + 52..layout + 56].copy_from_slice(&640u32.to_le_bytes());
    let parsed = Image::parse(&data).unwrap();
    assert_eq!(parsed.radio_region(1).unwrap().offset, 640);
}

#[test]
fn every_truncation_is_rejected_without_panicking() {
    for end in 0..PREPARED.len() {
        assert!(
            Image::parse(&PREPARED[..end]).is_err(),
            "accepted prefix {end}"
        );
    }
}

#[test]
fn ambiguous_or_out_of_range_entries_are_rejected() {
    for (offset, value) in [(40, 4097), (52, u32::MAX), (56, u32::MAX)] {
        let mut data = PREPARED.to_vec();
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(Image::parse(&data).is_err());
    }
    let mut duplicate = PREPARED.to_vec();
    duplicate[64..68].copy_from_slice(&PREPARED[48..52]);
    assert!(Image::parse(&duplicate).is_err());
}

#[test]
fn manifest_cannot_overlap_the_directory_or_payload() {
    for (offset, value) in [(16, 0u32), (20, 0), (20, u32::MAX)] {
        let mut data = PREPARED.to_vec();
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(Image::parse(&data).is_err());
    }
}

#[test]
fn missing_external_calibration_is_not_silently_defaulted() {
    let mut data = PREPARED.to_vec();
    // The fixture has vendor2ftb, nvrm, _cal, _apb in that order.
    data[80..84].copy_from_slice(b"none");
    assert!(Image::parse(&data).is_err());
}

#[test]
fn malformed_nested_radio_ranges_are_rejected() {
    let layout_offset = u32::from_le_bytes(PREPARED[52..56].try_into().unwrap()) as usize;
    for (relative, value) in [(8, 0u32), (52, 0xffffc000), (56, 0x4001)] {
        let mut data = PREPARED.to_vec();
        data[layout_offset + relative..layout_offset + relative + 4]
            .copy_from_slice(&value.to_le_bytes());
        assert!(Image::parse(&data).is_err());
    }
}
