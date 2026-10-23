// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Owned Max active parameter-buffer handshake. Unknown request bytes stay
//! opaque; RX context_word is echoed, without assigning it wider semantics.

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct GrowthRequestG16C {
    kind: [u8; 4],
    context: [u8; 4],
    slot: [u8; 4],
    token: [u8; 4],
    context_word: [u8; 4],
    opaque: [u8; 0x34],
}
#[repr(C)]
pub(crate) struct GrowthReplyG16C {
    kind: [u8; 4],
    context: [u8; 4],
    slot: [u8; 4],
    generation: [u8; 4],
    token: [u8; 4],
    context_word: [u8; 4],
    pad: [u8; 40],
}
const _: () = {
    assert!(core::mem::size_of::<GrowthRequestG16C>() == 0x48);
    assert!(core::mem::align_of::<GrowthRequestG16C>() == 1);
    assert!(core::mem::offset_of!(GrowthRequestG16C, kind) == 0);
    assert!(core::mem::offset_of!(GrowthRequestG16C, context) == 4);
    assert!(core::mem::offset_of!(GrowthRequestG16C, slot) == 8);
    assert!(core::mem::offset_of!(GrowthRequestG16C, token) == 12);
    assert!(core::mem::offset_of!(GrowthRequestG16C, context_word) == 16);
    assert!(core::mem::offset_of!(GrowthRequestG16C, opaque) == 20);
    assert!(core::mem::size_of::<GrowthReplyG16C>() == 64);
    assert!(core::mem::align_of::<GrowthReplyG16C>() == 1);
    assert!(core::mem::offset_of!(GrowthReplyG16C, kind) == 0);
    assert!(core::mem::offset_of!(GrowthReplyG16C, context) == 4);
    assert!(core::mem::offset_of!(GrowthReplyG16C, slot) == 8);
    assert!(core::mem::offset_of!(GrowthReplyG16C, generation) == 12);
    assert!(core::mem::offset_of!(GrowthReplyG16C, token) == 16);
    assert!(core::mem::offset_of!(GrowthReplyG16C, context_word) == 20);
    assert!(core::mem::offset_of!(GrowthReplyG16C, pad) == 24);
};
impl Default for GrowthReplyG16C {
    fn default() -> Self {
        Self { kind: 8u32.to_le_bytes(), context: [0;4], slot: [0;4],
            generation: [0;4], token: [0;4], context_word: [0;4], pad: [0;40] }
    }
}
impl GrowthRequestG16C {
    pub(crate) fn parse(raw: &[u8; 0x48]) -> Option<Self> {
        // SAFETY: Every field is an initialized byte array, alignment one.
        let request = unsafe { raw.as_ptr().cast::<Self>().read() };
        (u32::from_le_bytes(request.kind) == 6
            && (1..64).contains(&request.context()) && request.slot() < 15).then_some(request)
    }
    pub(crate) fn context(&self) -> u32 { u32::from_le_bytes(self.context) }
    pub(crate) fn slot(&self) -> u32 { u32::from_le_bytes(self.slot) }
    pub(crate) fn token(&self) -> u32 { u32::from_le_bytes(self.token) }
    pub(crate) fn reply(&self, generation: u32) -> Option<GrowthReplyG16C> {
        if generation == 0 { return None; }
        Some(GrowthReplyG16C { context: self.context, slot: self.slot,
            generation: generation.to_le_bytes(), token: self.token,
            context_word: self.context_word, ..Default::default() })
    }
}
impl GrowthReplyG16C {
    pub(crate) fn bytes(&self) -> &[u8;64] {
        // SAFETY: All bytes initialized, no padding, exact size asserted above.
        unsafe { &*(core::ptr::from_ref(self).cast::<[u8;64]>()) }
    }
}

/// Check firmware-updated DATA against the retained backing owner. Active
/// free-list ordering/holes are firmware-owned; foreign nonzero IDs are not.
pub(crate) fn metadata_owned(p: &super::resources::ParameterBufferG16C, ids: &[u32],
    manager: &[u8;0x88], control: &[u8;0x80], counter: u32, region: &[u8;16],
    pages: &[u8], blocks: &[u8]) -> bool {
    use super::resources::{ParameterDescriptorG16C, GPU_PAGE_LIST, MAX_BLOCKS, PAGE_LIST_SIZE};
    let word = |raw: &[u8], at: usize| u32::from_le_bytes(raw[at..at+4].try_into().unwrap());
    if p.validate().is_err() || ids.len() != p.blocks as usize || counter == 0
        || pages.len() != PAGE_LIST_SIZE || blocks.len() != ids.len()*8
        || ids.first().is_none_or(|id| *id == 0)
        || ids.windows(2).any(|v| v[0].checked_add(4).is_none_or(|end| end > v[1]))
        || ids.last().is_some_and(|id| id.checked_add(3).is_none())
        || word(manager,0xc) != p.slot || word(manager,0x34) != p.blocks*4
        || word(manager,0x38) != MAX_BLOCKS || word(manager,0x3c) != p.blocks
        || word(manager,0x54) >= p.blocks*4
        || word(control,0) != p.blocks || word(control,4) != p.blocks
        || word(control,0x60) > 1 { return false; }
    for (at, pointer) in [(0x20,p.page_list),(0x44,p.block_list),
        (0x4c,p.block_control),(0x64,p.counter)] {
        if u64::from_le_bytes(manager[at..at+8].try_into().unwrap()) != pointer { return false; }
    }
    let Ok(descriptor) = ParameterDescriptorG16C::new(GPU_PAGE_LIST,p.blocks*4) else { return false; };
    if word(region,0)&!1 != word(descriptor.bytes(),0) || region[4..8] != descriptor.bytes()[4..8] {
        return false;
    }
    for (raw, id) in blocks.chunks_exact(8).zip(ids) {
        if word(raw,0) != *id || word(raw,4) != 0 { return false; }
    }
    if pages[p.blocks as usize*16..].iter().any(|b| *b != 0) { return false; }
    pages[..p.blocks as usize*16].chunks_exact(4).all(|raw| {
        let value = word(raw,0);
        value == 0 || ids.partition_point(|id| *id <= value).checked_sub(1)
            .is_some_and(|index| value - ids[index] < 4)
    })
}
