// SPDX-License-Identifier: GPL-2.0-only

//! The two observed multi-cluster G16 profiles share one submission ABI.
//! G16S values describe the reported 20-core T6040; other bins are unqualified.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Profile {
    G16S,
    G16C,
}

impl Profile {
    pub(crate) const fn for_chip(chip: u32) -> Option<Self> {
        match chip {
            0x6040 => Some(Self::G16S),
            0x6041 => Some(Self::G16C),
            _ => None,
        }
    }
    pub(crate) const fn chip(self) -> u32 {
        match self {
            Self::G16S => 0x6040,
            Self::G16C => 0x6041,
        }
    }
    pub(crate) const fn gpu_id(self) -> u32 {
        match self {
            Self::G16S => 0x0a031100,
            Self::G16C => 0x0a041100,
        }
    }
    pub(crate) const fn variant(self) -> u32 {
        match self {
            Self::G16S => b'S' as u32,
            Self::G16C => b'C' as u32,
        }
    }
    pub(crate) const fn clusters(self) -> usize {
        match self {
            Self::G16S => 2,
            Self::G16C => 4,
        }
    }
    pub(crate) const fn cores(self) -> usize {
        self.clusters() * 10
    }
    pub(crate) const fn operand_bytes(self) -> u64 {
        match self {
            Self::G16S => 0x44000,
            Self::G16C => 0x7c000,
        }
    }
    pub(crate) const fn deflake(self) -> [u64; 2] {
        match self {
            Self::G16S => [0xa0, 0x820],
            Self::G16C => [0x120, 0xda0],
        }
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self::G16C
    }
}
