// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Scheduled-family boot records, initialized from source Default templates.
//! Large wire images live in static storage; runtime encoders mutate only
//! exclusive CPU staging. No captured pages or live firmware references.

use kernel::macros::versions;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InvalidSize;

#[versions(AGX_SCHEDULED)]
#[repr(C)]
pub(crate) struct RegionC {
    pub(crate) pad_0000: [u8; 0x24],
    pub(crate) field_0024: [u8; 4],
    pub(crate) pad_0028: [u8; 0x8],
    pub(crate) field_0030: [u8; 4],
    pub(crate) field_0034: [u8; 4],
    pub(crate) pad_0038: [u8; 0x4],
    pub(crate) field_003c: [u8; 4],
    pub(crate) pad_0040: [u8; 0x14],
    pub(crate) field_0054: [u8; 4],
    pub(crate) field_0058: [u8; 4],
    pub(crate) field_005c: [u8; 4],
    pub(crate) field_0060: [u8; 4],
    pub(crate) field_0064: [u8; 4],
    pub(crate) field_0068: [u8; 4],
    pub(crate) pad_006c: [u8; 0xc],
    pub(crate) field_0078: [u8; 4],
    pub(crate) pad_007c: [u8; 0x8],
    pub(crate) power_limit: [u8; 4],
    pub(crate) field_0088: [u8; 4],
    pub(crate) field_008c: [u8; 4],
    pub(crate) pad_0090: [u8; 0x8],
    pub(crate) field_0098: [u8; 4],
    pub(crate) pad_009c: [u8; 0x728],
    pub(crate) field_07c4: [u8; 4],
    pub(crate) field_07c8: [u8; 4],
    pub(crate) field_07cc: [u8; 4],
    pub(crate) field_07d0: [u8; 4],
    pub(crate) field_07d4: [u8; 4],
    pub(crate) field_07d8: [u8; 4],
    pub(crate) field_07dc: [u8; 4],
    pub(crate) field_07e0: [u8; 4],
    pub(crate) field_07e4: [u8; 4],
    pub(crate) field_07e8: [u8; 4],
    pub(crate) field_07ec: [u8; 4],
    pub(crate) field_07f0: [u8; 4],
    pub(crate) field_07f4: [u8; 4],
    pub(crate) field_07f8: [u8; 4],
    pub(crate) pad_07fc: [u8; 0x19c],
    pub(crate) field_0998: [u8; 4],
    pub(crate) field_099c: [u8; 4],
    pub(crate) field_09a0: [u8; 4],
    pub(crate) field_09a4: [u8; 4],
    pub(crate) pad_09a8: [u8; 0x10],
    pub(crate) field_09b8: [u8; 4],
    pub(crate) field_09bc: [u8; 4],
    pub(crate) field_09c0: [u8; 4],
    pub(crate) pad_09c4: [u8; 0x4],
    pub(crate) field_09c8: [u8; 4],
    pub(crate) field_09cc: [u8; 4],
    pub(crate) field_09d0: [u8; 4],
    pub(crate) pad_09d4: [u8; 0xc],
    pub(crate) field_09e0: [u8; 4],
    pub(crate) field_09e4: [u8; 4],
    pub(crate) field_09e8: [u8; 4],
    pub(crate) field_09ec: [u8; 4],
    pub(crate) field_09f0: [u8; 4],
    pub(crate) field_09f4: [u8; 4],
    pub(crate) field_09f8: [u8; 4],
    pub(crate) field_09fc: [u8; 4],
    pub(crate) pad_0a00: [u8; 0x41c],
    pub(crate) field_0e1c: [u8; 4],
    pub(crate) pad_0e20: [u8; 0x18],
    pub(crate) field_0e38: [u8; 4],
    pub(crate) field_0e3c: [u8; 4],
    pub(crate) pad_0e40: [u8; 0x10],
    pub(crate) field_0e50: [u8; 4],
    #[ver(G == G16C)]
    pub(crate) pad_end: [u8; 0x31ac],
    #[ver(G == G17P)]
    pub(crate) pad_end: [u8; 0x1ac],
}

impl RegionCG16C {
    const fn template() -> Self {
        // SAFETY: exclusively initialized byte arrays, no implicit padding.
        let mut raw: Self = unsafe { core::mem::zeroed() };
        raw.field_0024 = 0xbb8u32.to_le_bytes();
        raw.field_0030 = 0x1u32.to_le_bytes();
        raw.field_0034 = 0x1u32.to_le_bytes();
        raw.field_003c = 0x78u32.to_le_bytes();
        raw.field_0054 = 0x280fa0u32.to_le_bytes();
        raw.field_0058 = 0xffffu32.to_le_bytes();
        raw.field_005c = 0x10000u32.to_le_bytes();
        raw.field_0060 = 0x10000u32.to_le_bytes();
        raw.field_0064 = 0x30000u32.to_le_bytes();
        raw.field_0068 = 0x10000u32.to_le_bytes();
        raw.field_0078 = 0x1u32.to_le_bytes();
        raw.field_0088 = 0x3e8u32.to_le_bytes();
        raw.field_008c = 0x3e8u32.to_le_bytes();
        raw.field_0098 = 0x3e8u32.to_le_bytes();
        raw.field_07c4 = 0x5dcu32.to_le_bytes();
        raw.field_07c8 = 0x3f800000u32.to_le_bytes();
        raw.field_07cc = 0x3f4ccccdu32.to_le_bytes();
        raw.field_07d0 = 0x3e4ccccdu32.to_le_bytes();
        raw.field_07d4 = 0x3f666666u32.to_le_bytes();
        raw.field_07d8 = 0x3dcccccdu32.to_le_bytes();
        raw.field_07dc = 0x3e800000u32.to_le_bytes();
        raw.field_07e0 = 0x3f19999au32.to_le_bytes();
        raw.field_07e4 = 0x3f666666u32.to_le_bytes();
        raw.field_07e8 = 0x6u32.to_le_bytes();
        raw.field_07ec = 0x1u32.to_le_bytes();
        raw.field_07f0 = 0x1u32.to_le_bytes();
        raw.field_07f4 = 0x64u32.to_le_bytes();
        raw.field_07f8 = 0x64u32.to_le_bytes();
        raw.field_0998 = 0x28u32.to_le_bytes();
        raw.field_099c = 0xau32.to_le_bytes();
        raw.field_09a0 = 0xfau32.to_le_bytes();
        raw.field_09a4 = 0x1u32.to_le_bytes();
        raw.field_09b8 = 0x2u32.to_le_bytes();
        raw.field_09bc = 0x28u32.to_le_bytes();
        raw.field_09c0 = 0x5u32.to_le_bytes();
        raw.field_09c8 = 0x28u32.to_le_bytes();
        raw.field_09cc = 0x32u32.to_le_bytes();
        raw.field_09d0 = 0x1u32.to_le_bytes();
        raw.field_09e0 = 0x1u32.to_le_bytes();
        raw.field_09e4 = 0x1020200u32.to_le_bytes();
        raw.field_09e8 = 0x10a2801u32.to_le_bytes();
        raw.field_09ec = 0x28020101u32.to_le_bytes();
        raw.field_09f0 = 0x101010au32.to_le_bytes();
        raw.field_09f4 = 0x10a2802u32.to_le_bytes();
        raw.field_09f8 = 0x28010101u32.to_le_bytes();
        raw.field_09fc = 0x1010au32.to_le_bytes();
        raw.field_0e1c = 0x100u32.to_le_bytes();
        raw.field_0e38 = 0x300u32.to_le_bytes();
        raw.field_0e3c = 0x100u32.to_le_bytes();
        raw
    }
}
impl Default for RegionCG16C {
    fn default() -> Self {
        Self::template()
    }
}
static REGIONC_G16C: RegionCG16C = RegionCG16C::template();

impl RegionCG17P {
    const fn template() -> Self {
        // SAFETY: exclusively initialized byte arrays, no implicit padding.
        let mut raw: Self = unsafe { core::mem::zeroed() };
        raw.field_0024 = 0xbb8u32.to_le_bytes();
        raw.field_0030 = 0x1u32.to_le_bytes();
        raw.field_0034 = 0x1u32.to_le_bytes();
        raw.field_003c = 0x78u32.to_le_bytes();
        raw.field_0054 = 0x28ffffu32.to_le_bytes();
        raw.field_0058 = 0xffffu32.to_le_bytes();
        raw.field_005c = 0x10000u32.to_le_bytes();
        raw.field_0060 = 0x10000u32.to_le_bytes();
        raw.field_0064 = 0x30000u32.to_le_bytes();
        raw.field_0068 = 0x10000u32.to_le_bytes();
        raw.field_0078 = 0x1u32.to_le_bytes();
        raw.power_limit = 0x254eu32.to_le_bytes();
        raw.field_0088 = 0x3e8u32.to_le_bytes();
        raw.field_008c = 0x3e8u32.to_le_bytes();
        raw.field_0098 = 0x3e8u32.to_le_bytes();
        raw.field_07c4 = 0x7d0u32.to_le_bytes();
        raw.field_07c8 = 0x3f800000u32.to_le_bytes();
        raw.field_07cc = 0x3f4ccccdu32.to_le_bytes();
        raw.field_07d0 = 0x3e4ccccdu32.to_le_bytes();
        raw.field_07d4 = 0x3f666666u32.to_le_bytes();
        raw.field_07d8 = 0x3dcccccdu32.to_le_bytes();
        raw.field_07dc = 0x3e800000u32.to_le_bytes();
        raw.field_07e0 = 0x3f19999au32.to_le_bytes();
        raw.field_07e4 = 0x3f666666u32.to_le_bytes();
        raw.field_07e8 = 0x6u32.to_le_bytes();
        raw.field_07ec = 0x1u32.to_le_bytes();
        raw.field_07f0 = 0x1u32.to_le_bytes();
        raw.field_07f4 = 0x64u32.to_le_bytes();
        raw.field_07f8 = 0x64u32.to_le_bytes();
        raw.field_0998 = 0x28u32.to_le_bytes();
        raw.field_099c = 0xau32.to_le_bytes();
        raw.field_09a0 = 0xfau32.to_le_bytes();
        raw.field_09a4 = 0x1u32.to_le_bytes();
        raw.field_09b8 = 0x927c0u32.to_le_bytes();
        raw.field_09bc = 0x927c0u32.to_le_bytes();
        raw.field_09c0 = 0x5u32.to_le_bytes();
        raw.field_09c8 = 0x28u32.to_le_bytes();
        raw.field_09cc = 0x32u32.to_le_bytes();
        raw.field_09d0 = 0x1u32.to_le_bytes();
        raw.field_0e1c = 0x100u32.to_le_bytes();
        raw.field_0e38 = 0x300u32.to_le_bytes();
        raw.field_0e3c = 0x100u32.to_le_bytes();
        raw.field_0e50 = 0x100u32.to_le_bytes();
        raw
    }
}
impl Default for RegionCG17P {
    fn default() -> Self {
        Self::template()
    }
}
static REGIONC_G17P: RegionCG17P = RegionCG17P::template();

#[versions(AGX_SCHEDULED)]
impl RegionC::ver {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();
    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: all initialized byte arrays, alignment one, no padding.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }
}
#[versions(AGX_SCHEDULED)]
const REGIONC_LAYOUT: () = {
    assert!(core::mem::align_of::<RegionC::ver>() == 1);
    assert!(core::mem::offset_of!(RegionC::ver, field_0024) == 0x24);
    assert!(core::mem::offset_of!(RegionC::ver, field_0030) == 0x30);
    assert!(core::mem::offset_of!(RegionC::ver, field_0034) == 0x34);
    assert!(core::mem::offset_of!(RegionC::ver, field_003c) == 0x3c);
    assert!(core::mem::offset_of!(RegionC::ver, field_0054) == 0x54);
    assert!(core::mem::offset_of!(RegionC::ver, field_0058) == 0x58);
    assert!(core::mem::offset_of!(RegionC::ver, field_005c) == 0x5c);
    assert!(core::mem::offset_of!(RegionC::ver, field_0060) == 0x60);
    assert!(core::mem::offset_of!(RegionC::ver, field_0064) == 0x64);
    assert!(core::mem::offset_of!(RegionC::ver, field_0068) == 0x68);
    assert!(core::mem::offset_of!(RegionC::ver, field_0078) == 0x78);
    assert!(core::mem::offset_of!(RegionC::ver, power_limit) == 0x84);
    assert!(core::mem::offset_of!(RegionC::ver, field_0088) == 0x88);
    assert!(core::mem::offset_of!(RegionC::ver, field_008c) == 0x8c);
    assert!(core::mem::offset_of!(RegionC::ver, field_0098) == 0x98);
    assert!(core::mem::offset_of!(RegionC::ver, field_07c4) == 0x7c4);
    assert!(core::mem::offset_of!(RegionC::ver, field_07c8) == 0x7c8);
    assert!(core::mem::offset_of!(RegionC::ver, field_07cc) == 0x7cc);
    assert!(core::mem::offset_of!(RegionC::ver, field_07d0) == 0x7d0);
    assert!(core::mem::offset_of!(RegionC::ver, field_07d4) == 0x7d4);
    assert!(core::mem::offset_of!(RegionC::ver, field_07d8) == 0x7d8);
    assert!(core::mem::offset_of!(RegionC::ver, field_07dc) == 0x7dc);
    assert!(core::mem::offset_of!(RegionC::ver, field_07e0) == 0x7e0);
    assert!(core::mem::offset_of!(RegionC::ver, field_07e4) == 0x7e4);
    assert!(core::mem::offset_of!(RegionC::ver, field_07e8) == 0x7e8);
    assert!(core::mem::offset_of!(RegionC::ver, field_07ec) == 0x7ec);
    assert!(core::mem::offset_of!(RegionC::ver, field_07f0) == 0x7f0);
    assert!(core::mem::offset_of!(RegionC::ver, field_07f4) == 0x7f4);
    assert!(core::mem::offset_of!(RegionC::ver, field_07f8) == 0x7f8);
    assert!(core::mem::offset_of!(RegionC::ver, field_0998) == 0x998);
    assert!(core::mem::offset_of!(RegionC::ver, field_099c) == 0x99c);
    assert!(core::mem::offset_of!(RegionC::ver, field_09a0) == 0x9a0);
    assert!(core::mem::offset_of!(RegionC::ver, field_09a4) == 0x9a4);
    assert!(core::mem::offset_of!(RegionC::ver, field_09b8) == 0x9b8);
    assert!(core::mem::offset_of!(RegionC::ver, field_09bc) == 0x9bc);
    assert!(core::mem::offset_of!(RegionC::ver, field_09c0) == 0x9c0);
    assert!(core::mem::offset_of!(RegionC::ver, field_09c8) == 0x9c8);
    assert!(core::mem::offset_of!(RegionC::ver, field_09cc) == 0x9cc);
    assert!(core::mem::offset_of!(RegionC::ver, field_09d0) == 0x9d0);
    assert!(core::mem::offset_of!(RegionC::ver, field_09e0) == 0x9e0);
    assert!(core::mem::offset_of!(RegionC::ver, field_09e4) == 0x9e4);
    assert!(core::mem::offset_of!(RegionC::ver, field_09e8) == 0x9e8);
    assert!(core::mem::offset_of!(RegionC::ver, field_09ec) == 0x9ec);
    assert!(core::mem::offset_of!(RegionC::ver, field_09f0) == 0x9f0);
    assert!(core::mem::offset_of!(RegionC::ver, field_09f4) == 0x9f4);
    assert!(core::mem::offset_of!(RegionC::ver, field_09f8) == 0x9f8);
    assert!(core::mem::offset_of!(RegionC::ver, field_09fc) == 0x9fc);
    assert!(core::mem::offset_of!(RegionC::ver, field_0e1c) == 0xe1c);
    assert!(core::mem::offset_of!(RegionC::ver, field_0e38) == 0xe38);
    assert!(core::mem::offset_of!(RegionC::ver, field_0e3c) == 0xe3c);
    assert!(core::mem::offset_of!(RegionC::ver, field_0e50) == 0xe50);
};

#[versions(AGX_SCHEDULED)]
#[repr(C)]
pub(crate) struct PrimaryStatus {
    pub(crate) pad_0000: [u8; 0x4],
    pub(crate) field_0004: [u8; 4],
    pub(crate) pad_0008: [u8; 0x48d8],
    pub(crate) fwctl_state: [u8; 8],
    pub(crate) fwctl_ring: [u8; 8],
    pub(crate) pad_48f0: [u8; 0x9b44],
    pub(crate) field_e434: [u8; 4],
    pub(crate) pad_e438: [u8; 0x18],
    pub(crate) field_e450: [u8; 4],
    pub(crate) field_e454: [u8; 4],
    pub(crate) field_e458: [u8; 4],
    pub(crate) field_e45c: [u8; 4],
    pub(crate) field_e460: [u8; 4],
    pub(crate) pad_e464: [u8; 0xa4],
    pub(crate) field_e508: [u8; 4],
    pub(crate) field_e50c: [u8; 4],
    pub(crate) pad_e510: [u8; 0x8],
    pub(crate) field_e518: [u8; 4],
    pub(crate) field_e51c: [u8; 4],
    pub(crate) pad_e520: [u8; 0x10],
    pub(crate) field_e530: [u8; 4],
    pub(crate) field_e534: [u8; 4],
    pub(crate) pad_e538: [u8; 0x10],
    pub(crate) field_e548: [u8; 4],
    pub(crate) field_e54c: [u8; 4],
    pub(crate) field_e550: [u8; 4],
    pub(crate) pad_e554: [u8; 0x14],
    pub(crate) field_e568: [u8; 4],
    pub(crate) field_e56c: [u8; 4],
    pub(crate) field_e570: [u8; 4],
    pub(crate) field_e574: [u8; 4],
    pub(crate) pad_e578: [u8; 0x14],
    pub(crate) field_e58c: [u8; 4],
    pub(crate) pad_e590: [u8; 0x70],
    pub(crate) field_e600: [u8; 4],
    pub(crate) field_e604: [u8; 4],
    pub(crate) field_e608: [u8; 4],
    pub(crate) field_e60c: [u8; 4],
    pub(crate) field_e610: [u8; 4],
    pub(crate) field_e614: [u8; 4],
    pub(crate) field_e618: [u8; 4],
    pub(crate) pad_e61c: [u8; 0x308],
    pub(crate) field_e924: [u8; 4],
    pub(crate) field_e928: [u8; 4],
    pub(crate) field_e92c: [u8; 4],
    pub(crate) field_e930: [u8; 4],
    pub(crate) field_e934: [u8; 4],
    pub(crate) field_e938: [u8; 4],
    pub(crate) field_e93c: [u8; 4],
    pub(crate) field_e940: [u8; 4],
    pub(crate) field_e944: [u8; 4],
    pub(crate) field_e948: [u8; 4],
    pub(crate) pad_e94c: [u8; 0x10],
    pub(crate) field_e95c: [u8; 4],
    pub(crate) field_e960: [u8; 4],
    pub(crate) field_e964: [u8; 4],
    pub(crate) field_e968: [u8; 4],
    pub(crate) field_e96c: [u8; 4],
    pub(crate) field_e970: [u8; 4],
    pub(crate) pad_e974: [u8; 0x58],
    pub(crate) field_e9cc: [u8; 4],
    pub(crate) field_e9d0: [u8; 4],
    pub(crate) field_e9d4: [u8; 4],
    pub(crate) field_e9d8: [u8; 4],
    pub(crate) field_e9dc: [u8; 4],
    pub(crate) pad_e9e0: [u8; 0x2c],
    pub(crate) field_ea0c: [u8; 4],
    pub(crate) field_ea10: [u8; 4],
    pub(crate) pad_ea14: [u8; 0x8],
    pub(crate) field_ea1c: [u8; 4],
    pub(crate) field_ea20: [u8; 4],
    pub(crate) field_ea24: [u8; 4],
    pub(crate) field_ea28: [u8; 4],
    pub(crate) field_ea2c: [u8; 4],
    pub(crate) field_ea30: [u8; 4],
    pub(crate) field_ea34: [u8; 4],
    pub(crate) field_ea38: [u8; 4],
    pub(crate) pad_ea3c: [u8; 0x8],
    pub(crate) field_ea44: [u8; 4],
    pub(crate) field_ea48: [u8; 4],
    pub(crate) field_ea4c: [u8; 4],
    pub(crate) field_ea50: [u8; 4],
    pub(crate) pad_ea54: [u8; 0xc],
    pub(crate) field_ea60: [u8; 4],
    pub(crate) pad_end: [u8; 0x9c],
}

impl PrimaryStatusG16C {
    const fn template() -> Self {
        // SAFETY: exclusively initialized byte arrays, no implicit padding.
        let mut raw: Self = unsafe { core::mem::zeroed() };
        raw.field_0004 = 0x1u32.to_le_bytes();
        raw.field_e434 = 0x1u32.to_le_bytes();
        raw.field_e450 = 0x1fu32.to_le_bytes();
        raw.field_e454 = 0x3cf5c290u32.to_le_bytes();
        raw.field_e458 = 0x3f19999au32.to_le_bytes();
        raw.field_e45c = 0x28u32.to_le_bytes();
        raw.field_e460 = 0x1u32.to_le_bytes();
        raw.field_e508 = 0x2af8u32.to_le_bytes();
        raw.field_e50c = 0x1f40u32.to_le_bytes();
        raw.field_e518 = 0x42080000u32.to_le_bytes();
        raw.field_e51c = 0x40baaaaau32.to_le_bytes();
        raw.field_e530 = 0x26acu32.to_le_bytes();
        raw.field_e534 = 0xc8u32.to_le_bytes();
        raw.field_e548 = 0x42c80000u32.to_le_bytes();
        raw.field_e54c = 0x43c80000u32.to_le_bytes();
        raw.field_e550 = 0xc8u32.to_le_bytes();
        raw.field_e568 = 0x1u32.to_le_bytes();
        raw.field_e56c = 0x20457u32.to_le_bytes();
        raw.field_e570 = 0x3e99999au32.to_le_bytes();
        raw.field_e574 = 0x3e0b4396u32.to_le_bytes();
        raw.field_ea44 = 0x41700000u32.to_le_bytes();
        raw.field_ea48 = 0x40a00000u32.to_le_bytes();
        raw.field_ea4c = 0x20u32.to_le_bytes();
        raw.field_ea50 = 0x3e8u32.to_le_bytes();
        raw.field_ea60 = 0x384u32.to_le_bytes();
        raw
    }
}
impl Default for PrimaryStatusG16C {
    fn default() -> Self {
        Self::template()
    }
}
static PRIMARYSTATUS_G16C: PrimaryStatusG16C = PrimaryStatusG16C::template();

impl PrimaryStatusG17P {
    const fn template() -> Self {
        // SAFETY: exclusively initialized byte arrays, no implicit padding.
        let mut raw: Self = unsafe { core::mem::zeroed() };
        raw.field_0004 = 0x1u32.to_le_bytes();
        raw.field_e434 = 0x1u32.to_le_bytes();
        raw.field_e450 = 0xfu32.to_le_bytes();
        raw.field_e454 = 0x3f000000u32.to_le_bytes();
        raw.field_e458 = 0x40880000u32.to_le_bytes();
        raw.field_e45c = 0x28u32.to_le_bytes();
        raw.field_e460 = 0x1u32.to_le_bytes();
        raw.field_e508 = 0x2af8u32.to_le_bytes();
        raw.field_e50c = 0x1f40u32.to_le_bytes();
        raw.field_e518 = 0x41a2b852u32.to_le_bytes();
        raw.field_e51c = 0x41160419u32.to_le_bytes();
        raw.field_e530 = 0x26acu32.to_le_bytes();
        raw.field_e534 = 0xc8u32.to_le_bytes();
        raw.field_e548 = 0x42c80000u32.to_le_bytes();
        raw.field_e54c = 0x43c80000u32.to_le_bytes();
        raw.field_e550 = 0xc8u32.to_le_bytes();
        raw.field_e568 = 0x1u32.to_le_bytes();
        raw.field_e56c = 0x254eu32.to_le_bytes();
        raw.field_e570 = 0x3f000000u32.to_le_bytes();
        raw.field_e574 = 0x40cccccdu32.to_le_bytes();
        raw.field_e58c = 0x1u32.to_le_bytes();
        raw.field_e600 = 0x1u32.to_le_bytes();
        raw.field_e604 = 0x1u32.to_le_bytes();
        raw.field_e608 = 0x4u32.to_le_bytes();
        raw.field_e60c = 0x1u32.to_le_bytes();
        raw.field_e610 = 0x1u32.to_le_bytes();
        raw.field_e614 = 0x1u32.to_le_bytes();
        raw.field_e618 = 0x1u32.to_le_bytes();
        raw.field_e924 = 0x1u32.to_le_bytes();
        raw.field_e928 = 0x1f4u32.to_le_bytes();
        raw.field_e92c = 0x6u32.to_le_bytes();
        raw.field_e930 = 0x30d4u32.to_le_bytes();
        raw.field_e934 = 0x30d4u32.to_le_bytes();
        raw.field_e938 = 0x30d4u32.to_le_bytes();
        raw.field_e93c = 0x30d4u32.to_le_bytes();
        raw.field_e940 = 0x30d4u32.to_le_bytes();
        raw.field_e944 = 0x30d4u32.to_le_bytes();
        raw.field_e948 = 0xfa0u32.to_le_bytes();
        raw.field_e95c = 0x30d4u32.to_le_bytes();
        raw.field_e960 = 0x30d4u32.to_le_bytes();
        raw.field_e964 = 0x30d4u32.to_le_bytes();
        raw.field_e968 = 0x30d4u32.to_le_bytes();
        raw.field_e96c = 0x30d4u32.to_le_bytes();
        raw.field_e970 = 0x1u32.to_le_bytes();
        raw.field_e9cc = 0x6u32.to_le_bytes();
        raw.field_e9d0 = 0xdacu32.to_le_bytes();
        raw.field_e9d4 = 0x3e8u32.to_le_bytes();
        raw.field_e9d8 = 0xbb8u32.to_le_bytes();
        raw.field_e9dc = 0x64u32.to_le_bytes();
        raw.field_ea0c = 0x1u32.to_le_bytes();
        raw.field_ea10 = 0x4u32.to_le_bytes();
        raw.field_ea1c = 0x1f40u32.to_le_bytes();
        raw.field_ea20 = 0xc8u32.to_le_bytes();
        raw.field_ea24 = 0xfa0u32.to_le_bytes();
        raw.field_ea28 = 0xc8u32.to_le_bytes();
        raw.field_ea2c = 0x7d0u32.to_le_bytes();
        raw.field_ea30 = 0xc8u32.to_le_bytes();
        raw.field_ea34 = 0x3e8u32.to_le_bytes();
        raw.field_ea38 = 0xc8u32.to_le_bytes();
        raw.field_ea44 = 0x41700000u32.to_le_bytes();
        raw.field_ea48 = 0x40a00000u32.to_le_bytes();
        raw.field_ea4c = 0x20u32.to_le_bytes();
        raw.field_ea50 = 0x3e8u32.to_le_bytes();
        raw.field_ea60 = 0x384u32.to_le_bytes();
        raw
    }
}
impl Default for PrimaryStatusG17P {
    fn default() -> Self {
        Self::template()
    }
}
static PRIMARYSTATUS_G17P: PrimaryStatusG17P = PrimaryStatusG17P::template();

#[versions(AGX_SCHEDULED)]
impl PrimaryStatus::ver {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();
    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: all initialized byte arrays, alignment one, no padding.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }
}
#[versions(AGX_SCHEDULED)]
const PRIMARYSTATUS_LAYOUT: () = {
    assert!(core::mem::align_of::<PrimaryStatus::ver>() == 1);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_0004) == 0x4);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, fwctl_state) == 0x48e0);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, fwctl_ring) == 0x48e8);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e434) == 0xe434);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e450) == 0xe450);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e454) == 0xe454);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e458) == 0xe458);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e45c) == 0xe45c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e460) == 0xe460);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e508) == 0xe508);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e50c) == 0xe50c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e518) == 0xe518);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e51c) == 0xe51c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e530) == 0xe530);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e534) == 0xe534);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e548) == 0xe548);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e54c) == 0xe54c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e550) == 0xe550);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e568) == 0xe568);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e56c) == 0xe56c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e570) == 0xe570);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e574) == 0xe574);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e58c) == 0xe58c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e600) == 0xe600);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e604) == 0xe604);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e608) == 0xe608);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e60c) == 0xe60c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e610) == 0xe610);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e614) == 0xe614);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e618) == 0xe618);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e924) == 0xe924);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e928) == 0xe928);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e92c) == 0xe92c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e930) == 0xe930);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e934) == 0xe934);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e938) == 0xe938);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e93c) == 0xe93c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e940) == 0xe940);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e944) == 0xe944);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e948) == 0xe948);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e95c) == 0xe95c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e960) == 0xe960);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e964) == 0xe964);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e968) == 0xe968);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e96c) == 0xe96c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e970) == 0xe970);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e9cc) == 0xe9cc);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e9d0) == 0xe9d0);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e9d4) == 0xe9d4);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e9d8) == 0xe9d8);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_e9dc) == 0xe9dc);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea0c) == 0xea0c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea10) == 0xea10);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea1c) == 0xea1c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea20) == 0xea20);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea24) == 0xea24);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea28) == 0xea28);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea2c) == 0xea2c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea30) == 0xea30);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea34) == 0xea34);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea38) == 0xea38);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea44) == 0xea44);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea48) == 0xea48);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea4c) == 0xea4c);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea50) == 0xea50);
    assert!(core::mem::offset_of!(PrimaryStatus::ver, field_ea60) == 0xea60);
};

impl RegionCG16C {
    pub(crate) fn encode(out: &mut [u8], maximum_power: u32) -> Result<(), InvalidSize> {
        if out.len() < Self::SIZE {
            return Err(InvalidSize);
        }
        out[..Self::SIZE].copy_from_slice(REGIONC_G16C.as_bytes());
        // SAFETY: validated exclusive CPU staging, alignment one, byte fields.
        let raw = unsafe { &mut *out.as_mut_ptr().cast::<Self>() };
        raw.power_limit = maximum_power.to_le_bytes();
        Ok(())
    }
}
impl RegionCG17P {
    pub(crate) fn template_bytes() -> &'static [u8] {
        REGIONC_G17P.as_bytes()
    }
}

#[versions(AGX_SCHEDULED)]
impl PrimaryStatus::ver {
    pub(crate) fn encode(
        out: &mut [u8],
        fwctl_state: u64,
        fwctl_ring: u64,
    ) -> Result<(), InvalidSize> {
        if out.len() < Self::SIZE {
            return Err(InvalidSize);
        }
        #[ver(G == G16C)]
        out[..Self::SIZE].copy_from_slice(PRIMARYSTATUS_G16C.as_bytes());
        #[ver(G == G17P)]
        out[..Self::SIZE].copy_from_slice(PRIMARYSTATUS_G17P.as_bytes());
        // SAFETY: validated exclusive CPU staging, alignment one, byte fields.
        let raw = unsafe { &mut *out.as_mut_ptr().cast::<Self>() };
        raw.fwctl_state = fwctl_state.to_le_bytes();
        raw.fwctl_ring = fwctl_ring.to_le_bytes();
        Ok(())
    }
}

const _: () = {
    assert!(RegionCG16C::SIZE == 0x4000);
    assert!(RegionCG17P::SIZE == 0x1000);
    assert!(PrimaryStatusG16C::SIZE == 0xeb00);
    assert!(PrimaryStatusG17P::SIZE == 0xeb00);
};
