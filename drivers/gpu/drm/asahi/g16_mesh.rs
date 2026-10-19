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

/// Recognize only the currently qualified resource class: 16 mesh threads,
/// position-only output, at most four vertices and one primitive, one direct
/// object invocation. Broader output/resource classes need measured sizing.
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
    if state[0] != 0x504
        || state[1] != 0x10
        || state[2] != 0
        || state[3] != 0
        || state[8] != 1
        || state[9] != 0
        || state[10] != 0x400810
        || state[11] != 0
        || state[12] != 5
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
                // Direct object grid and local dimensions, three words each.
                for _ in 0..6 {
                    if next()? != 1 {
                        return Err(DecodeError::Unsupported);
                    }
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
