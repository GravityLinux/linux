// SPDX-License-Identifier: GPL-2.0-only

//! Source-defined initdata staging templates for the scheduled-work family.
//!
//! Only wire-compatible fields share definitions. Max's root extras retain
//! neutral names: matching Neo's extended shape does not establish that they
//! describe a second firmware instance. These values never borrow live DMA.

use kernel::macros::versions;

/// Host-side ownership/boot identity, never a discriminator inserted into a
/// wire record. Concrete constructors choose each ABI's actual kind words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FirmwareRole {
    Primary,
    Secondary,
}

pub(crate) const G16C_ROLES: &[FirmwareRole] = &[FirmwareRole::Primary];
pub(crate) const G17P_ROLES: &[FirmwareRole] = &[FirmwareRole::Primary, FirmwareRole::Secondary];

pub(crate) const fn version_words(words: [u16; 4]) -> [u8; 8] {
    let mut bytes = [0; 8];
    let mut index = 0;
    while index < 4 {
        let word = words[index].to_le_bytes();
        bytes[index * 2] = word[0];
        bytes[index * 2 + 1] = word[1];
        index += 1;
    }
    bytes
}

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct UatLevel {
    pub(crate) entry_size: u8,
    pub(crate) granule_bits: u8,
    pub(crate) granule_bits_2: u8,
    pub(crate) index_shift: u8,
    pub(crate) entries: [u8; 2],
    pub(crate) table_size: [u8; 2],
    pub(crate) unk_8: [u8; 8],
    pub(crate) physical_mask: [u8; 8],
    pub(crate) index_mask: [u8; 8],
}

impl UatLevel {
    const fn template(shift: u8, entries: u16) -> Self {
        Self {
            entry_size: 8,
            granule_bits: 14,
            granule_bits_2: 14,
            index_shift: shift,
            entries: entries.to_le_bytes(),
            table_size: 0x4000u16.to_le_bytes(),
            unk_8: 1u64.to_le_bytes(),
            physical_mask: 0x0000_03ff_ffff_c000u64.to_le_bytes(),
            index_mask: ((entries as u64 - 1) << shift).to_le_bytes(),
        }
    }
}

#[versions(AGX_SCHEDULED)]
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct InitRoot {
    pub(crate) version: [u8; 8],
    pub(crate) region_a: [u8; 8],
    pub(crate) unk_10: [u8; 8],
    pub(crate) main: [u8; 8],
    pub(crate) region_c: [u8; 8],
    pub(crate) kind: [u8; 4],
    pub(crate) unk_2c: [u8; 4],
    pub(crate) granule: [u8; 2],
    pub(crate) granule_bits: u8,
    pub(crate) level_count: u8,
    pub(crate) levels: [UatLevel; 3],
    pub(crate) pad_94: [u8; 0x14],
    pub(crate) status_a: [u8; 8],
    pub(crate) status_b: [u8; 8],
    #[ver(G == G16C)]
    pub(crate) extra_0: [u8; 8],
    #[ver(G == G16C)]
    pub(crate) extra_1: [u8; 8],
}

#[versions(AGX_SCHEDULED)]
impl Default for InitRoot::ver {
    fn default() -> Self {
        Self {
            version: version_words([0x04c0, 0x0396, 0xa322, 0x0c8a]),
            region_a: [0; 8],
            unk_10: [0; 8],
            main: [0; 8],
            region_c: [0; 8],
            kind: [0; 4],
            unk_2c: 1u32.to_le_bytes(),
            granule: 0x4000u16.to_le_bytes(),
            granule_bits: 14,
            level_count: 3,
            levels: [
                UatLevel::template(36, 64),
                UatLevel::template(25, 2048),
                UatLevel::template(14, 2048),
            ],
            pad_94: [0; 0x14],
            status_a: [0; 8],
            status_b: [0; 8],
            #[ver(G == G16C)]
            extra_0: [0; 8],
            #[ver(G == G16C)]
            extra_1: [0; 8],
        }
    }
}

#[versions(AGX_SCHEDULED)]
impl InitRoot::ver {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();

    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: all fields are initialized byte arrays or byte-aligned
        // records with no implicit padding; this borrows only owned staging.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }
}

/// Neo's secondary instance extends its own root. This is not an independent
/// macro axis, nor does it assign the same role to the Max root's extra words.
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct SecondaryInitRootG17P {
    pub(crate) root: InitRootG17P,
    pub(crate) extra_0: [u8; 8],
    pub(crate) extra_1: [u8; 8],
}

impl Default for SecondaryInitRootG17P {
    fn default() -> Self {
        Self {
            root: InitRootG17P {
                kind: 1u32.to_le_bytes(),
                ..Default::default()
            },
            extra_0: [0; 8],
            extra_1: [0; 8],
        }
    }
}

impl SecondaryInitRootG17P {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();

    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: the initialized root and byte-array tail are byte-aligned
        // and contain no implicit padding; the lifetime is bounded by self.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct Channel {
    pub(crate) states: [[u8; 8]; 3],
    pub(crate) ring: [u8; 8],
}

impl Channel {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();

    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: initialized byte arrays have no implicit padding, and the
        // returned slice borrows only this host-owned staging record.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct RegionTriple {
    pub(crate) address: [u8; 8],
    pub(crate) value: [u8; 4],
    pub(crate) kind: [u8; 4],
}

#[versions(AGX_SCHEDULED)]
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct MainConfig {
    pub(crate) hardware: [u8; 8],
    pub(crate) repeated: [u8; 8],
    pub(crate) repeated_2: [u8; 8],
    pub(crate) pad_18: [u8; 8],
    pub(crate) channels: [Channel; 17],
    #[ver(G == G16C)]
    pub(crate) pad_240: [u8; 0xc],
    #[ver(G == G17P)]
    pub(crate) pad_240: [u8; 0x14],
    #[ver(G == G16C)]
    pub(crate) auxiliary: [[u8; 8]; 6],
    #[ver(G == G17P)]
    pub(crate) auxiliary: [[u8; 8]; 5],
    pub(crate) pad_27c: [u8; 0x54],
    #[ver(G == G16C)]
    pub(crate) regions: [[u8; 8]; 6],
    #[ver(G == G17P)]
    pub(crate) regions: [RegionTriple; 3],
    pub(crate) kind: [u8; 4],
    pub(crate) pad_304: [u8; 0xdc],
    pub(crate) byte_mask: [u8; 4],
    pub(crate) pad_3e4: [u8; 0x8d],
    pub(crate) unaligned: [u8; 8],
    pub(crate) pad_479: [u8; 0x47],
    pub(crate) interval: [u8; 4],
    #[ver(G == G16C)]
    pub(crate) pad_4c4: [u8; 0x3c],
    #[ver(G == G17P)]
    pub(crate) pad_4c4: [u8; 0x13c],
}

#[versions(AGX_SCHEDULED)]
impl Default for MainConfig::ver {
    fn default() -> Self {
        Self {
            hardware: [0; 8],
            repeated: [0; 8],
            repeated_2: [0; 8],
            pad_18: [0; 8],
            channels: [Channel::default(); 17],
            #[ver(G == G16C)]
            pad_240: [0; 0xc],
            #[ver(G == G17P)]
            pad_240: [0; 0x14],
            #[ver(G == G16C)]
            auxiliary: [[0; 8]; 6],
            #[ver(G == G17P)]
            auxiliary: [[0; 8]; 5],
            pad_27c: [0; 0x54],
            #[ver(G == G16C)]
            regions: [[0; 8]; 6],
            #[ver(G == G17P)]
            regions: [RegionTriple::default(); 3],
            #[ver(G == G16C)]
            kind: 4u32.to_le_bytes(),
            #[ver(G == G17P)]
            kind: [0; 4],
            pad_304: [0; 0xdc],
            byte_mask: 0xffu32.to_le_bytes(),
            pad_3e4: [0; 0x8d],
            unaligned: [0; 8],
            pad_479: [0; 0x47],
            interval: 0x16u32.to_le_bytes(),
            #[ver(G == G16C)]
            pad_4c4: [0; 0x3c],
            #[ver(G == G17P)]
            pad_4c4: [0; 0x13c],
        }
    }
}

#[versions(AGX_SCHEDULED)]
impl MainConfig::ver {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();

    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: initialized byte arrays and byte-aligned subrecords occupy
        // the entire repr(C) image, whose lifetime is bounded by self.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }
}

impl MainConfigG17P {
    pub(crate) fn for_role(role: FirmwareRole) -> Self {
        match role {
            FirmwareRole::Primary => Self::default(),
            FirmwareRole::Secondary => Self {
                kind: 4u32.to_le_bytes(),
                byte_mask: [0; 4],
                interval: 0x2au32.to_le_bytes(),
                ..Self::default()
            },
        }
    }
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(crate) struct MmioRegion {
    pub(crate) physical: [u8; 8],
    pub(crate) address: [u8; 8],
    pub(crate) size: [u8; 4],
    pub(crate) stride: [u8; 4],
    pub(crate) relative: [u8; 8],
    pub(crate) flags: [u8; 4],
    pub(crate) pad_24: [u8; 4],
}

impl MmioRegion {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: the initialized byte arrays have alignment one, no implicit
        // padding and a lifetime bounded by this owned staging value.
        unsafe {
            core::slice::from_raw_parts(
                core::ptr::from_ref(self).cast(),
                core::mem::size_of::<Self>(),
            )
        }
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct HardwareRegion {
    pub(crate) lead: [u8; 4],
    pub(crate) low: [u8; 4],
    pub(crate) low_tag: [u8; 4],
    pub(crate) high: [u8; 8],
    pub(crate) count: [u8; 4],
    pub(crate) stride: [u8; 4],
    pub(crate) trail: [u8; 4],
    pub(crate) pad_20: [u8; 0x20],
}

impl Default for HardwareRegion {
    fn default() -> Self {
        Self {
            lead: [0; 4],
            low: [0; 4],
            low_tag: 0x70u32.to_le_bytes(),
            high: [0; 8],
            count: [0; 4],
            stride: [0; 4],
            trail: [0; 4],
            pad_20: [0; 0x20],
        }
    }
}

impl HardwareRegion {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: initialized byte-array fields have alignment one and no
        // implicit padding; the slice borrows only this staging value.
        unsafe {
            core::slice::from_raw_parts(
                core::ptr::from_ref(self).cast(),
                core::mem::size_of::<Self>(),
            )
        }
    }
}

// Layout gates are separate from initialization values and execution proof.
#[versions(AGX_SCHEDULED)]
const INIT_LAYOUT: () = {
    assert!(core::mem::align_of::<InitRoot::ver>() == 1);
    assert!(core::mem::offset_of!(InitRoot::ver, version) == 0x00);
    assert!(core::mem::offset_of!(InitRoot::ver, region_a) == 0x08);
    assert!(core::mem::offset_of!(InitRoot::ver, unk_10) == 0x10);
    assert!(core::mem::offset_of!(InitRoot::ver, main) == 0x18);
    assert!(core::mem::offset_of!(InitRoot::ver, region_c) == 0x20);
    assert!(core::mem::offset_of!(InitRoot::ver, kind) == 0x28);
    assert!(core::mem::offset_of!(InitRoot::ver, unk_2c) == 0x2c);
    assert!(core::mem::offset_of!(InitRoot::ver, granule) == 0x30);
    assert!(core::mem::offset_of!(InitRoot::ver, granule_bits) == 0x32);
    assert!(core::mem::offset_of!(InitRoot::ver, level_count) == 0x33);
    assert!(core::mem::offset_of!(InitRoot::ver, levels) == 0x34);
    assert!(core::mem::offset_of!(InitRoot::ver, pad_94) == 0x94);
    assert!(core::mem::offset_of!(InitRoot::ver, status_a) == 0xa8);
    assert!(core::mem::offset_of!(InitRoot::ver, status_b) == 0xb0);
    assert!(core::mem::align_of::<MainConfig::ver>() == 1);
    assert!(core::mem::offset_of!(MainConfig::ver, hardware) == 0);
    assert!(core::mem::offset_of!(MainConfig::ver, repeated) == 8);
    assert!(core::mem::offset_of!(MainConfig::ver, repeated_2) == 0x10);
    assert!(core::mem::offset_of!(MainConfig::ver, pad_18) == 0x18);
    assert!(core::mem::offset_of!(MainConfig::ver, channels) == 0x20);
    assert!(core::mem::offset_of!(MainConfig::ver, pad_240) == 0x240);
    assert!(core::mem::offset_of!(MainConfig::ver, pad_27c) == 0x27c);
    assert!(core::mem::offset_of!(MainConfig::ver, regions) == 0x2d0);
    assert!(core::mem::offset_of!(MainConfig::ver, kind) == 0x300);
    assert!(core::mem::offset_of!(MainConfig::ver, pad_304) == 0x304);
    assert!(core::mem::offset_of!(MainConfig::ver, byte_mask) == 0x3e0);
    assert!(core::mem::offset_of!(MainConfig::ver, pad_3e4) == 0x3e4);
    assert!(core::mem::offset_of!(MainConfig::ver, unaligned) == 0x471);
    assert!(core::mem::offset_of!(MainConfig::ver, pad_479) == 0x479);
    assert!(core::mem::offset_of!(MainConfig::ver, interval) == 0x4c0);
    assert!(core::mem::offset_of!(MainConfig::ver, pad_4c4) == 0x4c4);
};

const _: () = {
    assert!(core::mem::size_of::<UatLevel>() == 0x20);
    assert!(core::mem::align_of::<UatLevel>() == 1);
    assert!(core::mem::offset_of!(UatLevel, entry_size) == 0);
    assert!(core::mem::offset_of!(UatLevel, granule_bits) == 1);
    assert!(core::mem::offset_of!(UatLevel, granule_bits_2) == 2);
    assert!(core::mem::offset_of!(UatLevel, index_shift) == 3);
    assert!(core::mem::offset_of!(UatLevel, entries) == 4);
    assert!(core::mem::offset_of!(UatLevel, table_size) == 6);
    assert!(core::mem::offset_of!(UatLevel, unk_8) == 8);
    assert!(core::mem::offset_of!(UatLevel, physical_mask) == 0x10);
    assert!(core::mem::offset_of!(UatLevel, index_mask) == 0x18);
    assert!(InitRootG16C::SIZE == 0xc8);
    assert!(InitRootG17P::SIZE == 0xb8);
    assert!(SecondaryInitRootG17P::SIZE == 0xc8);
    assert!(core::mem::offset_of!(InitRootG16C, extra_0) == 0xb8);
    assert!(core::mem::offset_of!(InitRootG16C, extra_1) == 0xc0);
    assert!(core::mem::offset_of!(SecondaryInitRootG17P, root) == 0);
    assert!(core::mem::offset_of!(SecondaryInitRootG17P, extra_0) == 0xb8);
    assert!(core::mem::offset_of!(SecondaryInitRootG17P, extra_1) == 0xc0);
    assert!(Channel::SIZE == 0x20);
    assert!(core::mem::offset_of!(Channel, states) == 0);
    assert!(core::mem::offset_of!(Channel, ring) == 0x18);
    assert!(core::mem::size_of::<RegionTriple>() == 0x10);
    assert!(core::mem::offset_of!(RegionTriple, address) == 0);
    assert!(core::mem::offset_of!(RegionTriple, value) == 8);
    assert!(core::mem::offset_of!(RegionTriple, kind) == 0xc);
    assert!(MainConfigG16C::SIZE == 0x500);
    assert!(MainConfigG17P::SIZE == 0x600);
    assert!(core::mem::offset_of!(MainConfigG16C, auxiliary) == 0x24c);
    assert!(core::mem::offset_of!(MainConfigG17P, auxiliary) == 0x254);
    assert!(core::mem::size_of::<MmioRegion>() == 0x28);
    assert!(core::mem::align_of::<MmioRegion>() == 1);
    assert!(core::mem::offset_of!(MmioRegion, physical) == 0);
    assert!(core::mem::offset_of!(MmioRegion, address) == 8);
    assert!(core::mem::offset_of!(MmioRegion, size) == 0x10);
    assert!(core::mem::offset_of!(MmioRegion, stride) == 0x14);
    assert!(core::mem::offset_of!(MmioRegion, relative) == 0x18);
    assert!(core::mem::offset_of!(MmioRegion, flags) == 0x20);
    assert!(core::mem::offset_of!(MmioRegion, pad_24) == 0x24);
    assert!(core::mem::size_of::<HardwareRegion>() == 0x40);
    assert!(core::mem::align_of::<HardwareRegion>() == 1);
    assert!(core::mem::offset_of!(HardwareRegion, lead) == 0);
    assert!(core::mem::offset_of!(HardwareRegion, low) == 4);
    assert!(core::mem::offset_of!(HardwareRegion, low_tag) == 8);
    assert!(core::mem::offset_of!(HardwareRegion, high) == 0xc);
    assert!(core::mem::offset_of!(HardwareRegion, count) == 0x14);
    assert!(core::mem::offset_of!(HardwareRegion, stride) == 0x18);
    assert!(core::mem::offset_of!(HardwareRegion, trail) == 0x1c);
    assert!(core::mem::offset_of!(HardwareRegion, pad_20) == 0x20);
};
