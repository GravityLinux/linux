// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Initial device-wide operand cursor DATA for the scheduled firmware family.
//! These are owned staging bytes. GPU-written live cursors remain behind Memory.

use kernel::macros::versions;

#[versions(AGX_SCHEDULED)]
#[repr(C)]
pub(crate) struct DispatchState {
    directory: [u8; 8],
    read_position: [u8; 4],
    write_position: [u8; 4],
    pages: [u8; 4],
    reserved: [u8; 12],
}

#[versions(AGX_SCHEDULED)]
impl DispatchState::ver {
    #[ver(G == G16C)]
    const DIRECTORY_TAG: u64 = 0x4000000000000000;
    #[ver(G == G17P)]
    const DIRECTORY_TAG: u64 = 0x0800000000000000;
    #[ver(G == G16C)]
    const INITIAL_PAGES: u32 = 20 * 2048;
    #[ver(G == G17P)]
    const INITIAL_PAGES: u32 = 0x1c00;

    /// Current host reservation policy: one pool entry per application ASID.
    /// Native clients can change pools/banks without changing their ASID;
    /// this arithmetic is not an inherent firmware ownership relationship.
    #[ver(G == G16C)]
    pub(crate) fn slot_offset(context: u16) -> Option<usize> {
        if !(1..64).contains(&context) {
            return None;
        }
        Some(usize::from(context - 1) * 0x20)
    }

    /// Encode geometry; pool allocation validates its full directory extents.
    pub(crate) fn initial(directory: u64, pages: u32) -> Option<Self> {
        if directory < 0x4000 || directory >= 1 << 42 || directory & 0x3fff != 0 || pages == 0 {
            return None;
        }
        let tail = pages.checked_mul(2)?;
        Some(Self {
            directory: (Self::DIRECTORY_TAG | (directory >> 7)).to_le_bytes(),
            write_position: tail.to_le_bytes(),
            pages: pages.to_le_bytes(),
            ..Self::default()
        })
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 0x20] {
        // SAFETY: Initialized byte arrays, no implicit padding; asserted below.
        unsafe { &*core::ptr::from_ref(self).cast::<[u8; 0x20]>() }
    }
}

#[versions(AGX_SCHEDULED)]
impl Default for DispatchState::ver {
    fn default() -> Self {
        Self {
            directory: (Self::DIRECTORY_TAG | (0x7000000000 >> 7)).to_le_bytes(),
            read_position: [0; 4],
            write_position: (Self::INITIAL_PAGES * 2).to_le_bytes(),
            pages: Self::INITIAL_PAGES.to_le_bytes(),
            reserved: [0; 12],
        }
    }
}

#[versions(AGX_SCHEDULED)]
const DISPATCH_LAYOUT: () = {
    assert!(core::mem::size_of::<DispatchState::ver>() == 0x20);
    assert!(core::mem::align_of::<DispatchState::ver>() == 1);
    assert!(core::mem::offset_of!(DispatchState::ver, directory) == 0);
    assert!(core::mem::offset_of!(DispatchState::ver, read_position) == 8);
    assert!(core::mem::offset_of!(DispatchState::ver, write_position) == 12);
    assert!(core::mem::offset_of!(DispatchState::ver, pages) == 16);
    assert!(core::mem::offset_of!(DispatchState::ver, reserved) == 20);
};
