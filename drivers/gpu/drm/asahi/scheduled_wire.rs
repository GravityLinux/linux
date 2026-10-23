// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! G16C/G17P scheduled-work wire layouts for macOS 26.6.2.
//!
//! These values are owned host staging images, never references to live
//! firmware-mutated storage. Byte-array fields preserve little-endian encoding
//! and unaligned addresses without packed references or implicit padding.

use kernel::macros::versions;

#[versions(AGX_SCHEDULED)]
#[repr(C)]
pub(crate) struct QueueContext {
    sentinel: [u8; 2],
    pad_02: [u8; 3],
    field_05: [u8; 1],
    pad_06: [u8; 0x20],
    field_26: [u8; 1],
    pad_27: [u8; 0xc],
    field_33: [u8; 1],
    pad_34: [u8; 0xc],
}

#[versions(AGX_SCHEDULED)]
impl Default for QueueContext::ver {
    fn default() -> Self {
        Self { sentinel: [0xff; 2], pad_02: [0; 3], field_05: [1],
            pad_06: [0; 0x20], field_26: [2], pad_27: [0; 0xc],
            field_33: [0xff], pad_34: [0; 0xc] }
    }
}

#[versions(AGX_SCHEDULED)]
impl QueueContext::ver {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: Initialized byte-array fields, alignment one, no padding.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), 0x40) }
    }
}

#[versions(AGX_SCHEDULED)]
const QUEUE_CONTEXT_LAYOUT: () = {
    assert!(core::mem::size_of::<QueueContext::ver>() == 0x40);
    assert!(core::mem::align_of::<QueueContext::ver>() == 1);
    assert!(core::mem::offset_of!(QueueContext::ver, sentinel) == 0);
    assert!(core::mem::offset_of!(QueueContext::ver, pad_02) == 2);
    assert!(core::mem::offset_of!(QueueContext::ver, field_05) == 5);
    assert!(core::mem::offset_of!(QueueContext::ver, pad_06) == 6);
    assert!(core::mem::offset_of!(QueueContext::ver, field_26) == 0x26);
    assert!(core::mem::offset_of!(QueueContext::ver, pad_27) == 0x27);
    assert!(core::mem::offset_of!(QueueContext::ver, field_33) == 0x33);
    assert!(core::mem::offset_of!(QueueContext::ver, pad_34) == 0x34);
};

#[derive(Clone, Copy)]
pub(crate) struct QueueParameters {
    pub(crate) pointers: u64,
    pub(crate) ring: u64,
    pub(crate) jobs: u64,
    pub(crate) context: u64,
    pub(crate) uuid: u32,
    pub(crate) priority: u32,
    pub(crate) unk_2c: u32,
    pub(crate) unk_30: u64,
    pub(crate) unk_38: u32,
    pub(crate) prio5: u32,
    pub(crate) unk_94: u32,
}

// G16C: g16c/queue.py Queue.encode, source-built and output-qualified.
// G17P: g17p.py QUEUE_RECORD_SIZE. The 0xc0 allocation stride belongs to the
// owner and must not be confused with the 0xac wire record.
#[versions(AGX_SCHEDULED)]
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct QueueRecord {
    pointers: [u8; 8],
    ring: [u8; 8],
    jobs: [u8; 8],
    gpu_rptr1: [u8; 4],
    gpu_rptr2: [u8; 4],
    gpu_rptr3: [u8; 4],
    unk_24: [u8; 4],
    priority: [u8; 4],
    unk_2c: [u8; 4],
    unk_30: [u8; 8],
    unk_38: [u8; 4],
    unk_3c: [u8; 4],
    prio5: [u8; 4],
    unk_44: [u8; 4],
    uuid: [u8; 4],
    unk_4c: [u8; 4],
    unk_50: [u8; 8],
    busy: [u8; 4],
    pad_5c: [u8; 0x1c],
    unk_78: [u8; 4],
    has_commands: [u8; 4],
    pad_80: [u8; 0x10],
    inflight: [u8; 4],
    unk_94: [u8; 4],
    unk_98: [u8; 4],
    context: [u8; 8],
    pad_a4: [u8; 4],
    #[ver(G == G17P)]
    pad_a8: [u8; 4],
}

#[versions(AGX_SCHEDULED)]
impl Default for QueueRecord::ver {
    fn default() -> Self {
        Self {
            pointers: [0; 8],
            ring: [0; 8],
            jobs: [0; 8],
            gpu_rptr1: [0; 4],
            gpu_rptr2: [0; 4],
            gpu_rptr3: [0; 4],
            unk_24: u32::MAX.to_le_bytes(),
            priority: [0; 4],
            unk_2c: [0; 4],
            unk_30: [0; 8],
            unk_38: [0; 4],
            unk_3c: [0; 4],
            prio5: [0; 4],
            unk_44: u32::MAX.to_le_bytes(),
            uuid: [0; 4],
            unk_4c: [0; 4],
            unk_50: [0; 8],
            busy: [0; 4],
            pad_5c: [0; 0x1c],
            unk_78: [0; 4],
            has_commands: [0; 4],
            pad_80: [0; 0x10],
            inflight: [0; 4],
            unk_94: [0; 4],
            unk_98: [0; 4],
            context: [0; 8],
            pad_a4: [0; 4],
            #[ver(G == G17P)]
            pad_a8: [0; 4],
        }
    }
}

#[versions(AGX_SCHEDULED)]
impl QueueRecord::ver {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();

    pub(crate) fn new(p: QueueParameters) -> Self {
        Self {
            pointers: p.pointers.to_le_bytes(),
            ring: p.ring.to_le_bytes(),
            jobs: p.jobs.to_le_bytes(),
            priority: p.priority.to_le_bytes(),
            unk_2c: p.unk_2c.to_le_bytes(),
            unk_30: p.unk_30.to_le_bytes(),
            unk_38: p.unk_38.to_le_bytes(),
            prio5: p.prio5.to_le_bytes(),
            uuid: p.uuid.to_le_bytes(),
            unk_94: p.unk_94.to_le_bytes(),
            context: p.context.to_le_bytes(),
            ..Self::default()
        }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: every field is an initialized byte array; their alignment is
        // one, so repr(C) introduces no implicit padding. The slice borrows
        // exactly this owned staging value and cannot outlive it.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }
}

// Every source-defined field boundary is checked, including reserved spans.
// Whole-image reference comparisons additionally verify the template values.
#[versions(AGX_SCHEDULED)]
const QUEUE_LAYOUT: () = {
    assert!(core::mem::offset_of!(QueueRecord::ver, pointers) == 0x00);
    assert!(core::mem::offset_of!(QueueRecord::ver, ring) == 0x08);
    assert!(core::mem::offset_of!(QueueRecord::ver, jobs) == 0x10);
    assert!(core::mem::offset_of!(QueueRecord::ver, gpu_rptr1) == 0x18);
    assert!(core::mem::offset_of!(QueueRecord::ver, gpu_rptr2) == 0x1c);
    assert!(core::mem::offset_of!(QueueRecord::ver, gpu_rptr3) == 0x20);
    assert!(core::mem::offset_of!(QueueRecord::ver, unk_24) == 0x24);
    assert!(core::mem::offset_of!(QueueRecord::ver, priority) == 0x28);
    assert!(core::mem::offset_of!(QueueRecord::ver, unk_2c) == 0x2c);
    assert!(core::mem::offset_of!(QueueRecord::ver, unk_30) == 0x30);
    assert!(core::mem::offset_of!(QueueRecord::ver, unk_38) == 0x38);
    assert!(core::mem::offset_of!(QueueRecord::ver, unk_3c) == 0x3c);
    assert!(core::mem::offset_of!(QueueRecord::ver, prio5) == 0x40);
    assert!(core::mem::offset_of!(QueueRecord::ver, unk_44) == 0x44);
    assert!(core::mem::offset_of!(QueueRecord::ver, uuid) == 0x48);
    assert!(core::mem::offset_of!(QueueRecord::ver, unk_4c) == 0x4c);
    assert!(core::mem::offset_of!(QueueRecord::ver, unk_50) == 0x50);
    assert!(core::mem::offset_of!(QueueRecord::ver, busy) == 0x58);
    assert!(core::mem::offset_of!(QueueRecord::ver, pad_5c) == 0x5c);
    assert!(core::mem::offset_of!(QueueRecord::ver, unk_78) == 0x78);
    assert!(core::mem::offset_of!(QueueRecord::ver, has_commands) == 0x7c);
    assert!(core::mem::offset_of!(QueueRecord::ver, pad_80) == 0x80);
    assert!(core::mem::offset_of!(QueueRecord::ver, inflight) == 0x90);
    assert!(core::mem::offset_of!(QueueRecord::ver, unk_94) == 0x94);
    assert!(core::mem::offset_of!(QueueRecord::ver, unk_98) == 0x98);
    assert!(core::mem::offset_of!(QueueRecord::ver, context) == 0x9c);
    assert!(core::mem::offset_of!(QueueRecord::ver, pad_a4) == 0xa4);
};

const _: () = {
    assert!(QueueRecordG16C::SIZE == 0xa8);
    assert!(QueueRecordG17P::SIZE == 0xac);
    assert!(core::mem::align_of::<QueueRecordG16C>() == 1);
    assert!(core::mem::align_of::<QueueRecordG17P>() == 1);
    assert!(core::mem::offset_of!(QueueRecordG16C, context) == 0x9c);
    assert!(core::mem::offset_of!(QueueRecordG17P, context) == 0x9c);
    assert!(core::mem::offset_of!(QueueRecordG17P, pad_a8) == 0xa8);
};

// Queue consumer/producer state is staged before publication. G16C has an
// extended record: its legacy capacity word is a sentinel, not the ring bound.
#[versions(AGX_SCHEDULED)]
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct QueuePointers {
    done: [u8; 4],
    pad_04: [u8; 0xc],
    unk_10: [u8; 4],
    pad_14: [u8; 0xc],
    unk_20: [u8; 4],
    pad_24: [u8; 0xc],
    read: [u8; 4],
    pad_34: [u8; 0xc],
    write: [u8; 4],
    pad_44: [u8; 0xc],
    capacity: [u8; 4],
    pad_54: [u8; 0xc],
    #[ver(G == G16C)]
    extended_60: [u8; 4],
    #[ver(G == G16C)]
    pad_64: [u8; 0x1c],
}

#[versions(AGX_SCHEDULED)]
impl Default for QueuePointers::ver {
    fn default() -> Self {
        Self {
            done: [0; 4],
            pad_04: [0; 0xc],
            unk_10: [0; 4],
            pad_14: [0; 0xc],
            unk_20: [0; 4],
            pad_24: [0; 0xc],
            read: [0; 4],
            pad_34: [0; 0xc],
            write: [0; 4],
            pad_44: [0; 0xc],
            #[ver(G == G17P)]
            capacity: [0; 4],
            #[ver(G == G16C)]
            capacity: u32::MAX.to_le_bytes(),
            pad_54: [0; 0xc],
            #[ver(G == G16C)]
            extended_60: 0x500u32.to_le_bytes(),
            #[ver(G == G16C)]
            pad_64: [0; 0x1c],
        }
    }
}

#[versions(AGX_SCHEDULED)]
impl QueuePointers::ver {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();

    #[ver(G == G17P)]
    pub(crate) fn with_capacity(capacity: u32) -> Self {
        Self {
            capacity: capacity.to_le_bytes(),
            ..Self::default()
        }
    }

    #[ver(G == G16C)]
    pub(crate) fn with_write(write: u32) -> Self {
        Self {
            write: write.to_le_bytes(),
            ..Self::default()
        }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: initialized byte-array fields have alignment one and no
        // implicit padding. The result borrows only the owned staging image.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }
}

#[versions(AGX_SCHEDULED)]
const POINTER_LAYOUT: () = {
    assert!(core::mem::align_of::<QueuePointers::ver>() == 1);
    assert!(core::mem::offset_of!(QueuePointers::ver, done) == 0x00);
    assert!(core::mem::offset_of!(QueuePointers::ver, pad_04) == 0x04);
    assert!(core::mem::offset_of!(QueuePointers::ver, unk_10) == 0x10);
    assert!(core::mem::offset_of!(QueuePointers::ver, pad_14) == 0x14);
    assert!(core::mem::offset_of!(QueuePointers::ver, unk_20) == 0x20);
    assert!(core::mem::offset_of!(QueuePointers::ver, pad_24) == 0x24);
    assert!(core::mem::offset_of!(QueuePointers::ver, read) == 0x30);
    assert!(core::mem::offset_of!(QueuePointers::ver, pad_34) == 0x34);
    assert!(core::mem::offset_of!(QueuePointers::ver, write) == 0x40);
    assert!(core::mem::offset_of!(QueuePointers::ver, pad_44) == 0x44);
    assert!(core::mem::offset_of!(QueuePointers::ver, capacity) == 0x50);
    assert!(core::mem::offset_of!(QueuePointers::ver, pad_54) == 0x54);
};

const _: () = {
    assert!(QueuePointersG16C::SIZE == 0x80);
    assert!(QueuePointersG17P::SIZE == 0x60);
    assert!(core::mem::offset_of!(QueuePointersG16C, extended_60) == 0x60);
    assert!(core::mem::offset_of!(QueuePointersG16C, pad_64) == 0x64);
};
