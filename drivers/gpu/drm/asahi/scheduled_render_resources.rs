// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Source-built Max parameter-buffer DATA. These are exclusive CPU staging
//! images; no mutable firmware mapping is borrowed through a Rust reference.

use super::Error;

pub(crate) const PAGE: usize = 0x4000;
pub(crate) const MAX_BLOCKS: u32 = 19737;
pub(crate) const PAGE_LIST_SIZE: usize = 0x50000;
pub(crate) const BLOCK_LIST_SIZE: usize = ((MAX_BLOCKS as usize * 8 + PAGE - 1) / PAGE) * PAGE;
pub(crate) const CONTEXT_BASE: u64 = 0x1000000000;
pub(crate) const GPU_PAGE_LIST: u64 = CONTEXT_BASE + 0x438000;
pub(crate) const GPU_SCENE_BASE: u64 = CONTEXT_BASE + 0x420000;
pub(crate) const GPU_AUXILIARY: u64 = CONTEXT_BASE + 0x428000;

#[derive(Clone, Copy)]
pub(crate) struct ParameterBufferG16C {
    pub(crate) slot: u32,
    pub(crate) blocks: u32,
    pub(crate) page_list: u64,
    pub(crate) block_list: u64,
    pub(crate) block_control: u64,
    pub(crate) counter: u64,
    pub(crate) scene_list: u64,
}

#[repr(C)]
pub(crate) struct ParameterManagerG16C {
    pad_0000: [u8; 0xc],
    slot: [u8; 0x4],
    pad_0010: [u8; 0x10],
    page_list: [u8; 0x8],
    gpu_page_list: [u8; 0x4],
    field_2c: [u8; 0x4],
    page_list_size: [u8; 0x4],
    pages: [u8; 0x4],
    max_blocks: [u8; 0x4],
    blocks: [u8; 0x4],
    pad_0040: [u8; 0x4],
    block_list: [u8; 0x8],
    block_control: [u8; 0x8],
    last_page: [u8; 0x4],
    block_size: [u8; 0x4],
    pad_005c: [u8; 0x8],
    counter: [u8; 0x8],
    pad_006c: [u8; 0x10],
    max_pages: [u8; 0x4],
    field_80: [u8; 0x4],
    gpu_auxiliary: [u8; 0x4],
    pad_0088: [u8; 0x3f78],
}
const _: () = {
    assert!(core::mem::size_of::<ParameterManagerG16C>() == PAGE);
    assert!(core::mem::align_of::<ParameterManagerG16C>() == 1);
    assert!(core::mem::offset_of!(ParameterManagerG16C, pad_0000) == 0x0);
    assert!(core::mem::offset_of!(ParameterManagerG16C, slot) == 0xc);
    assert!(core::mem::offset_of!(ParameterManagerG16C, pad_0010) == 0x10);
    assert!(core::mem::offset_of!(ParameterManagerG16C, page_list) == 0x20);
    assert!(core::mem::offset_of!(ParameterManagerG16C, gpu_page_list) == 0x28);
    assert!(core::mem::offset_of!(ParameterManagerG16C, field_2c) == 0x2c);
    assert!(core::mem::offset_of!(ParameterManagerG16C, page_list_size) == 0x30);
    assert!(core::mem::offset_of!(ParameterManagerG16C, pages) == 0x34);
    assert!(core::mem::offset_of!(ParameterManagerG16C, max_blocks) == 0x38);
    assert!(core::mem::offset_of!(ParameterManagerG16C, blocks) == 0x3c);
    assert!(core::mem::offset_of!(ParameterManagerG16C, pad_0040) == 0x40);
    assert!(core::mem::offset_of!(ParameterManagerG16C, block_list) == 0x44);
    assert!(core::mem::offset_of!(ParameterManagerG16C, block_control) == 0x4c);
    assert!(core::mem::offset_of!(ParameterManagerG16C, last_page) == 0x54);
    assert!(core::mem::offset_of!(ParameterManagerG16C, block_size) == 0x58);
    assert!(core::mem::offset_of!(ParameterManagerG16C, pad_005c) == 0x5c);
    assert!(core::mem::offset_of!(ParameterManagerG16C, counter) == 0x64);
    assert!(core::mem::offset_of!(ParameterManagerG16C, pad_006c) == 0x6c);
    assert!(core::mem::offset_of!(ParameterManagerG16C, max_pages) == 0x7c);
    assert!(core::mem::offset_of!(ParameterManagerG16C, field_80) == 0x80);
    assert!(core::mem::offset_of!(ParameterManagerG16C, gpu_auxiliary) == 0x84);
    assert!(core::mem::offset_of!(ParameterManagerG16C, pad_0088) == 0x88);
};
impl ParameterManagerG16C {
    const fn template() -> Self {
        // SAFETY: All fields are byte arrays, initialized, alignment one.
        let mut raw: Self = unsafe { core::mem::zeroed() };
        raw.gpu_page_list = ((GPU_PAGE_LIST - CONTEXT_BASE) as u32).to_le_bytes();
        raw.field_2c = 0x10u32.to_le_bytes();
        raw.page_list_size = (PAGE_LIST_SIZE as u32).to_le_bytes();
        raw.max_blocks = MAX_BLOCKS.to_le_bytes();
        raw.block_size = 0x20000u32.to_le_bytes();
        raw.max_pages = (MAX_BLOCKS * 4).to_le_bytes();
        raw.field_80 = (6579u32 * 4).to_le_bytes();
        raw.gpu_auxiliary = ((GPU_AUXILIARY - CONTEXT_BASE) as u32).to_le_bytes();
        raw
    }
    pub(crate) fn write(out: &mut [u8], p: &ParameterBufferG16C) -> Result<(), Error> {
        p.validate()?;
        if out.len() != PAGE {
            return Err(Error::Invalid);
        }
        out.copy_from_slice(PARAMETER_TEMPLATE_G16C.as_bytes());
        // SAFETY: Checked size and exclusive CPU staging, byte-array fields.
        let raw = unsafe { &mut *out.as_mut_ptr().cast::<Self>() };
        raw.slot = p.slot.to_le_bytes();
        raw.pages = (p.blocks * 4).to_le_bytes();
        raw.blocks = p.blocks.to_le_bytes();
        raw.last_page = (p.blocks * 4 - 1).to_le_bytes();
        raw.page_list = p.page_list.to_le_bytes();
        raw.block_list = p.block_list.to_le_bytes();
        raw.block_control = p.block_control.to_le_bytes();
        raw.counter = p.counter.to_le_bytes();
        Ok(())
    }
    fn as_bytes(&self) -> &[u8] {
        // SAFETY: Initialized byte-array fields with no padding.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), PAGE) }
    }
}
impl Default for ParameterManagerG16C {
    fn default() -> Self {
        Self::template()
    }
}
static PARAMETER_TEMPLATE_G16C: ParameterManagerG16C = ParameterManagerG16C::template();

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SceneG16C {
    low_slot: [u8; 8],
    firmware_slot: [u8; 8],
    pad_10: [u8; 0x18],
    compact_scene: [u8; 8],
    pad_30: [u8; 0x10],
    control: [u8; 8],
    pad_48: [u8; 0x38],
}
impl Default for SceneG16C {
    fn default()->Self {
        // SAFETY: All fields are byte arrays, so zero initializes each field.
        unsafe { core::mem::zeroed() }
    }
}
const _: () = {
    assert!(core::mem::size_of::<SceneG16C>() == 0x80);
    assert!(core::mem::align_of::<SceneG16C>() == 1);
    macro_rules! at {
        ($f:ident,$o:expr) => {
            assert!(core::mem::offset_of!(SceneG16C, $f) == $o);
        };
    }
    at!(low_slot, 0);
    at!(firmware_slot, 8);
    at!(pad_10, 0x10);
    at!(compact_scene, 0x28);
    at!(pad_30, 0x30);
    at!(control, 0x40);
    at!(pad_48, 0x48);
};

#[repr(C)]
pub(crate) struct ParameterDescriptorG16C {
    page_list: [u8; 4],
    pages: [u8; 4],
    pad_08: [u8; 8],
}
impl ParameterDescriptorG16C {
    pub(crate) fn new(page_list: u64, pages: u32) -> Result<Self, Error> {
        if !(0x1000000000..0x2000000000).contains(&page_list)
            || page_list & 0x7f != 0
            || pages == 0
            || pages >= 1 << 22
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            page_list: ((page_list >> 4) as u32 & 0xfffffff8).to_le_bytes(),
            pages: pages.to_le_bytes(),
            pad_08: [0; 8],
        })
    }
    pub(crate) fn bytes(&self) -> &[u8; 16] {
        // SAFETY: All initialized byte arrays, checked layout below.
        unsafe { &*core::ptr::from_ref(self).cast() }
    }
}
const _: () = {
    assert!(core::mem::size_of::<ParameterDescriptorG16C>() == 16);
    assert!(core::mem::align_of::<ParameterDescriptorG16C>() == 1);
    assert!(core::mem::offset_of!(ParameterDescriptorG16C, page_list) == 0);
    assert!(core::mem::offset_of!(ParameterDescriptorG16C, pages) == 4);
    assert!(core::mem::offset_of!(ParameterDescriptorG16C, pad_08) == 8);
};
impl ParameterBufferG16C {
    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.slot >= 15 || self.blocks == 0 || self.blocks > MAX_BLOCKS {
            return Err(Error::Invalid);
        }
        for address in [
            self.page_list,
            self.block_list,
            self.block_control,
            self.counter,
            self.scene_list,
        ] {
            if address == 0
                || address & (PAGE as u64 - 1) != 0
                || address.checked_add(PAGE as u64).is_none()
            {
                return Err(Error::Invalid);
            }
        }
        Ok(())
    }
    pub(crate) fn scenes(&self, out: &mut [u8], low_slots: u64) -> Result<(), Error> {
        self.validate()?;
        if out.len() != PAGE || low_slots & 3 != 0 {
            return Err(Error::Invalid);
        }
        low_slots.checked_add(80 * 4).ok_or(Error::Overflow)?;
        let firmware_end = self.scene_list.checked_add(80 * 4).ok_or(Error::Overflow)?;
        let control = self
            .block_control
            .checked_add(0x40)
            .ok_or(Error::Overflow)?;
        out.fill(0);
        for i in 0..80usize {
            let raw = SceneG16C {
                low_slot: (low_slots + i as u64 * 4).to_le_bytes(),
                firmware_slot: (firmware_end - 80 * 4 + i as u64 * 4).to_le_bytes(),
                compact_scene: (GPU_SCENE_BASE - CONTEXT_BASE + (i % 36) as u64 * 0x70)
                    .to_le_bytes(),
                control: control.to_le_bytes(),
                ..Default::default()
            };
            // SAFETY: Initialized byte arrays, checked size/alignment above.
            let bytes =
                unsafe { core::slice::from_raw_parts(core::ptr::from_ref(&raw).cast(), 0x80) };
            out[i * 0x80..(i + 1) * 0x80].copy_from_slice(bytes);
        }
        Ok(())
    }
    pub(crate) fn block_ids(&self, out: &mut [u8]) -> Result<(), Error> {
        self.validate()?;
        if out.len() != BLOCK_LIST_SIZE {
            return Err(Error::Invalid);
        }
        out.fill(0);
        for i in 0..self.blocks as usize {
            out[i * 8..i * 8 + 4].copy_from_slice(&(0x11 + i as u32 * 5).to_le_bytes());
        }
        Ok(())
    }
    pub(crate) fn pages(&self, out: &mut [u8]) -> Result<(), Error> {
        self.validate()?;
        if out.len() != PAGE_LIST_SIZE {
            return Err(Error::Invalid);
        }
        out.fill(0);
        for i in 0..self.blocks as usize {
            for page in 0..4usize {
                let at = (i * 4 + page) * 4;
                out[at..at + 4].copy_from_slice(&(0x11 + i as u32 * 5 + page as u32).to_le_bytes());
            }
        }
        Ok(())
    }
    pub(crate) fn control(&self, out: &mut [u8]) -> Result<(), Error> {
        self.validate()?;
        if out.len() != PAGE {
            return Err(Error::Invalid);
        }
        out.fill(0);
        out[..4].copy_from_slice(&self.blocks.to_le_bytes());
        out[4..8].copy_from_slice(&self.blocks.to_le_bytes());
        out[0x60..0x64].copy_from_slice(&1u32.to_le_bytes());
        Ok(())
    }
    pub(crate) fn counter(&self, out: &mut [u8]) -> Result<(), Error> {
        self.validate()?;
        if out.len() != PAGE {
            return Err(Error::Invalid);
        }
        out.fill(0);
        out[..4].copy_from_slice(&1u32.to_le_bytes());
        Ok(())
    }
    pub(crate) fn backing_ranges(&self) -> Result<impl Iterator<Item = (u64, u64)>, Error> {
        self.validate()?;
        Ok((0..self.blocks).map(|i| (CONTEXT_BASE + u64::from(0x11 + i * 5) * 0x8000, 0x20000)))
    }
}
