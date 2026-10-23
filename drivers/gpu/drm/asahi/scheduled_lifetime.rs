// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Source G16C control DATA. Constructors never publish a channel producer.

#[repr(C)]
pub(crate) struct MaintenanceG16C {
    opcode: [u8; 4],
    engine: [u8; 4],
    phase: [u8; 4],
    reserved: [u8; 0x34],
}
impl Default for MaintenanceG16C {
    fn default() -> Self {
        Self {
            opcode: 0x1bu32.to_le_bytes(),
            engine: [0; 4],
            phase: [0; 4],
            reserved: [0; 0x34],
        }
    }
}
impl MaintenanceG16C {
    pub(crate) fn new(engine: u32, phase: u32) -> Option<Self> {
        if engine > 2 || !(1..=2).contains(&phase) {
            return None;
        }
        Some(Self {
            engine: engine.to_le_bytes(),
            phase: phase.to_le_bytes(),
            ..Self::default()
        })
    }
    pub(crate) fn bytes(&self) -> &[u8; 64] {
        // SAFETY: Initialized byte arrays, no padding, checked layout below.
        unsafe { &*core::ptr::from_ref(self).cast() }
    }
}
#[repr(C)]
pub(crate) struct LifetimeTickG16C {
    opcode: [u8; 4],
    reserved: [u8; 0x3c],
}
impl Default for LifetimeTickG16C {
    fn default() -> Self {
        Self {
            opcode: 0x2eu32.to_le_bytes(),
            reserved: [0; 0x3c],
        }
    }
}
impl LifetimeTickG16C {
    pub(crate) fn for_render(sequence: u32) -> Self {
        let mut raw = Self::default();
        raw.reserved[..4].copy_from_slice(&sequence.to_le_bytes());
        raw
    }

    pub(crate) fn bytes(&self) -> &[u8; 64] {
        // SAFETY: Same byte-array layout invariant.
        unsafe { &*core::ptr::from_ref(self).cast() }
    }
}
#[repr(C)]
pub(crate) struct ContextCloseG16C {
    opcode: [u8; 4],
    reserved_04: [u8; 4],
    indices_flags: [u8; 4],
    context: [u8; 8],
    reserved_14: [u8; 0x2c],
}
impl Default for ContextCloseG16C {
    fn default() -> Self {
        Self {
            opcode: 0x14u32.to_le_bytes(),
            reserved_04: [0; 4],
            indices_flags: 0x04000002u32.to_le_bytes(),
            context: [0; 8],
            reserved_14: [0; 0x2c],
        }
    }
}
impl ContextCloseG16C {
    pub(crate) fn new(context: u64, prefix: [u8; 4]) -> Option<Self> {
        if context < 0xfffffc0000000000 || context & 15 != 0 || prefix[..2].contains(&255) {
            return None;
        }
        // These are firmware-assigned prefix indices, not a scheduler slot.
        Some(Self {
            context: context.to_le_bytes(),
            indices_flags: (0x04000002u32 | ((prefix[0] as u32) << 8) | ((prefix[1] as u32) << 16))
                .to_le_bytes(),
            ..Self::default()
        })
    }
    pub(crate) fn bytes(&self) -> &[u8; 64] {
        // SAFETY: Same byte-array layout invariant.
        unsafe { &*core::ptr::from_ref(self).cast() }
    }
}
const _: () = {
    assert!(core::mem::size_of::<MaintenanceG16C>() == 64);
    assert!(core::mem::align_of::<MaintenanceG16C>() == 1);
    assert!(core::mem::offset_of!(MaintenanceG16C, opcode) == 0);
    assert!(core::mem::offset_of!(MaintenanceG16C, engine) == 4);
    assert!(core::mem::offset_of!(MaintenanceG16C, phase) == 8);
    assert!(core::mem::offset_of!(MaintenanceG16C, reserved) == 12);
    assert!(core::mem::size_of::<LifetimeTickG16C>() == 64);
    assert!(core::mem::align_of::<LifetimeTickG16C>() == 1);
    assert!(core::mem::offset_of!(LifetimeTickG16C, opcode) == 0);
    assert!(core::mem::offset_of!(LifetimeTickG16C, reserved) == 4);
    assert!(core::mem::size_of::<ContextCloseG16C>() == 64);
    assert!(core::mem::align_of::<ContextCloseG16C>() == 1);
    assert!(core::mem::offset_of!(ContextCloseG16C, opcode) == 0);
    assert!(core::mem::offset_of!(ContextCloseG16C, reserved_04) == 4);
    assert!(core::mem::offset_of!(ContextCloseG16C, indices_flags) == 8);
    assert!(core::mem::offset_of!(ContextCloseG16C, context) == 12);
    assert!(core::mem::offset_of!(ContextCloseG16C, reserved_14) == 20);
};
