// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! G16C compute FList metadata. These are staging bytes, never live references.

pub(crate) const PAGE: usize = 0x4000;
pub(crate) const DIRECTORY_SIZE: u64 = 0x1000000;
pub(crate) const TABLE_SIZE: u64 = 0x10000;
pub(crate) const BUFFER_SIZE: u64 = 0x800000;
pub(crate) const BUFFER_STRIDE: u64 = 0x808000;
pub(crate) const INITIAL_BUFFERS: u32 = 20;

#[repr(C)]
pub(crate) struct ComputeSupportG16C {
    header: [u8; 4],
    pad_04: [u8; 4],
    active: [u8; 4],
    pad_0c: [u8; 4],
    resource_class: [u8; 4],
    directory: [u8; 8],
    capacity: [u8; 4],
    pad_20: [u8; 4],
    registered_pages: [u8; 4],
    notify: [u8; 4],
    available_pages: [u8; 4],
    table: [u8; 8],
    pad_38: [u8; 8],
    field_40: [u8; 4],
    pad_44: [u8; 4],
    entries: [u8; 4],
    auxiliary: [u8; 8],
    pad_54: [u8; 8],
    field_5c: [u8; 4],
    kind: [u8; 4],
}
impl Default for ComputeSupportG16C {
    fn default() -> Self {
        // SAFETY: All fields are initialized byte arrays, with no padding.
        let mut value: Self = unsafe { core::mem::zeroed() };
        value.header = 1u32.to_le_bytes();
        value.resource_class = 2u32.to_le_bytes();
        value.capacity = (DIRECTORY_SIZE as u32 / 8).to_le_bytes();
        value.field_40 = 4u32.to_le_bytes();
        value.field_5c = 1u32.to_le_bytes();
        value.kind = 2u32.to_le_bytes();
        value
    }
}
impl ComputeSupportG16C {
    pub(crate) fn for_render(directory:u64,table:u64,auxiliary:u64,
                            allocated:u32,registered:u32,entries:u32)->Option<Self>{
        if registered>entries || entries>allocated {return None;}
        let mut raw=Self::new(directory,table,auxiliary,allocated)?;
        raw.registered_pages=(registered*2048).to_le_bytes();
        raw.available_pages=raw.registered_pages;
        raw.entries=(entries*8).to_le_bytes();
        raw.field_5c=[0;4];
        raw.kind=4u32.to_le_bytes();
        Some(raw)
    }
    pub(crate) fn new(directory: u64, table: u64, auxiliary: u64, count: u32) -> Option<Self> {
        if count == 0
            || count > 1024
            || [directory, table, auxiliary].contains(&0)
            || (directory | table | auxiliary) & (PAGE as u64 - 1) != 0
            || directory.checked_add(DIRECTORY_SIZE)? > 1 << 42
            || table.checked_add(TABLE_SIZE)? > 1 << 42
        {
            return None;
        }
        Some(Self {
            directory: directory.to_le_bytes(),
            table: table.to_le_bytes(),
            auxiliary: auxiliary.to_le_bytes(),
            registered_pages: (count * 2048).to_le_bytes(),
            available_pages: (count * 2048).to_le_bytes(),
            entries: (count * 8).to_le_bytes(),
            ..Self::default()
        })
    }
    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: Initialized byte arrays; size/alignment/offsets below.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), 0x64) }
    }
}
pub(crate) type OperandSupportG16C=ComputeSupportG16C;
const _: () = {
    assert!(core::mem::size_of::<ComputeSupportG16C>() == 0x64);
    assert!(core::mem::align_of::<ComputeSupportG16C>() == 1);
    macro_rules! at {
        ($f:ident,$o:expr) => {
            assert!(core::mem::offset_of!(ComputeSupportG16C, $f) == $o);
        };
    }
    at!(header, 0);
    at!(pad_04, 4);
    at!(active, 8);
    at!(pad_0c, 0xc);
    at!(resource_class, 0x10);
    at!(directory, 0x14);
    at!(capacity, 0x1c);
    at!(pad_20, 0x20);
    at!(registered_pages, 0x24);
    at!(notify, 0x28);
    at!(available_pages, 0x2c);
    at!(table, 0x30);
    at!(pad_38, 0x38);
    at!(field_40, 0x40);
    at!(pad_44, 0x44);
    at!(entries, 0x48);
    at!(auxiliary, 0x4c);
    at!(pad_54, 0x54);
    at!(field_5c, 0x5c);
    at!(kind, 0x60);
};
