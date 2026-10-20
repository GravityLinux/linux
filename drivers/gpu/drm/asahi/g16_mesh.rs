// SPDX-License-Identifier: GPL-2.0-only
// Copyright 2026 The Asahi Linux Contributors

//! Native object/mesh launch for the paired experiment.
//!
//! The render UAPI does not distinguish a user object shader from a stub that
//! writes outputs. `native_mesh=1` selects the user-object convention. A paired
//! stream starts with a complete mesh state; there is no search for magic words
//! inside shader data, PPP records, or ordinary vertex streams.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Profile {
    /// Exclusive per-render storage, filled by prepare_render, never userspace.
    pub(crate) payload: u64,
}

impl Profile {
    pub(crate) const PAYLOAD_BYTES: usize = 0x4000;
    pub(crate) const TA_SIZE: u64 = 0x13300;
    pub(crate) const GATE: u64 = 0xec_0040_0020;

    pub(crate) fn payload_register(self) -> Option<u64> {
        // Absolute-address interpretation, still requiring device qualification.
        // Do not retain the research kernel's captured context-relative address.
        (self.payload != 0 && self.payload < (1 << 40) && self.payload & 0x3fff == 0)
            .then_some(0x0200_0000_0000_0000 | self.payload)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DecodeError {
    Unmapped,
    Unsupported,
}

/// Admit the paired T16/T32 class: four vertices' storage, one primitive and one
/// direct object dispatch, with position and up to 124 per-vertex scalars.
/// Primitive outputs, task payload and wider resource classes remain excluded.
/// The dispatch grid may contain multiple complete physical object workgroups.
/// This does not bound the child grid computed by the object shader, nor prove
/// that a particular aggregate workload fits the available GPU resources.
/// The packet budget bounds malformed streams, including a missing terminator.
pub(crate) fn decode(
    start: u64,
    mut read: impl FnMut(u64) -> Option<u32>,
) -> Result<Option<Profile>, DecodeError> {
    let mut cursor = start;
    let mut remaining = 1024;
    let mut next = || {
        if remaining == 0 || cursor & 3 != 0 {
            return Err(DecodeError::Unsupported);
        }
        remaining -= 1;
        let value = read(cursor).ok_or(DecodeError::Unmapped)?;
        cursor = cursor.checked_add(4).ok_or(DecodeError::Unsupported)?;
        Ok(value)
    };
    let header = next()?;
    if header != 0x4000_0d2c {
        // Bit 11 marks the native state extension. Reject unfamiliar native
        // layouts rather than submitting them with ordinary TA state.
        return if header >> 29 == 2 && header & (1 << 11) != 0 {
            Err(DecodeError::Unsupported)
        } else {
            Ok(None)
        };
    }
    let mut state = [0; 13];
    for word in &mut state {
        *word = next()?;
    }
    // The low byte counts scalar vertex planes, including XYZW. The next byte
    // includes the cull/header row as well. For the four-vertex storage class,
    // each plane occupies 16 bytes, and word 13 repeats that row count. Require
    // their relationship, rather than enumerating individual shader layouts.
    // The paired Mesa limit is 128 components (2080 local bytes including the
    // 32-byte count/index header). The uploaded launch rounds this allocation
    // up to a power of two. This packet parser does not validate shader code or
    // its local-allocation instructions.
    let components = state[0] & 0xff;
    if !(4..=128).contains(&components)
        || state[0] != components | ((components + 1) << 8)
        || state[1] != 0x10
        || state[2] != 0
        || state[3] != 0
        || state[8] != 1
        || state[9] != 0
        || !matches!(state[10], 0x400810 | 0x400820)
        || state[11] != 0
        || state[12] != components + 1
    {
        return Err(DecodeError::Unsupported);
    }
    // Words 5..8 describe the object and mesh USC launches, including resource
    // counts and relocatable handles. They are consumed by the GPU as submitted.
    let mut drew = false;
    loop {
        let word = next()?;
        match word >> 29 {
            0 if word & 0x1fff_0000 == 0 && word & 0xff00 != 0 => {
                // PPP state update: one header and one context-relative pointer.
                next()?;
            }
            1 if word & !0x2000_0013 == 0 => {} // G16 coherency barrier
            3 if word == 0x7000_0600 && !drew => {
                // Total threads describe a nonempty grid of complete object
                // workgroups. Keep the scalar carrier and physical T16/T32,
                // independently of the mesh width in the state packet.
                let total = [next()?, next()?, next()?];
                let local = [next()?, next()?, next()?];
                if !matches!(local, [1, 1, 1] | [16, 1, 1] | [32, 1, 1])
                    || total
                        .iter()
                        .zip(local)
                        .any(|(&n, size)| n == 0 || n % size != 0)
                {
                    return Err(DecodeError::Unsupported);
                }
                drew = true;
            }
            6 if word == 0xc000_0000 && drew => return Ok(Some(Profile { payload: 0 })),
            // Includes links, indirect draws, mixed vertex/mesh streams, a
            // second draw, and state changes requiring a different size class.
            _ => return Err(DecodeError::Unsupported),
        }
    }
}
