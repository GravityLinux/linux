// SPDX-License-Identifier: GPL-2.0-only
// Copyright 2026 The Asahi Linux Contributors

//! Native object/mesh launch for the paired experiment.
//!
//! The render UAPI does not distinguish a user object shader from a stub that
//! writes outputs. `native_mesh=1/2/3` selects the user-object convention. A paired
//! stream starts with a complete mesh state; there is no search for magic words
//! inside shader data, PPP records, or ordinary vertex streams.

/// Runtime copy of `asahi_m4.native_mesh`, writable from debugfs
/// `asahi_m4/native_mesh` without a reboot. Each submission reads it once.
/// A VM's support object latches the value at its first render (see
/// `Support::native`); later changes never rewrite a published header.
pub(crate) static NATIVE_MESH: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// The highest accepted `native_mesh` value.
pub(crate) const MAX_MODE: u32 = 3;

pub(crate) fn mode() -> u32 {
    NATIVE_MESH.load(core::sync::atomic::Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Disabled,
    SingleParent,
    MultipleParents,
    /// Research admission for the capability study: the wider state model in
    /// `research_state`, with mode 2's object dispatch rules. Not qualified.
    Research,
}

impl Mode {
    pub(crate) fn from_parameter(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Disabled),
            1 => Some(Self::SingleParent),
            2 => Some(Self::MultipleParents),
            3 => Some(Self::Research),
            _ => None,
        }
    }

    pub(crate) fn enabled(self) -> bool {
        self != Self::Disabled
    }
}

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
/// Mode 1 retains the qualified one-parent class. Only mode 2 admits multiple
/// complete physical object workgroups, pending device qualification.
/// This does not bound the child grid computed by the object shader, nor prove
/// that a particular aggregate workload fits the available GPU resources.
/// Mode 3 replaces the state check with `research_state` and also admits point
/// and line draws; the stream structure and object dispatch rules are mode 2's.
/// The packet budget bounds malformed streams, including a missing terminator.
pub(crate) fn decode(
    start: u64,
    mode: Mode,
    mut read: impl FnMut(u64) -> Option<u32>,
) -> Result<Option<Profile>, DecodeError> {
    if !mode.enabled() {
        return Ok(None);
    }
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
    if mode == Mode::Research {
        if !research_state(&state) {
            return Err(DecodeError::Unsupported);
        }
    } else if !(4..=128).contains(&components)
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
            3 if !drew
                && (word == 0x7000_0600
                    || (mode == Mode::Research && matches!(word, 0x7000_0000 | 0x7000_0100))) =>
            {
                // Total threads describe a nonempty grid of complete object
                // workgroups. Keep the scalar carrier and physical T16/T32,
                // independently of the mesh width in the state packet.
                let total = [next()?, next()?, next()?];
                let local = [next()?, next()?, next()?];
                if !matches!(local, [1, 1, 1] | [16, 1, 1] | [32, 1, 1])
                    || (mode == Mode::SingleParent && total != local)
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

/// Research state model (`native_mesh=3`), from the 2026-10-22 capability
/// study's own-MSL captures: n vertex planes, P primitives, ceil(V / 4) vertex
/// quads and C 32-bit primitive components.
///
/// - word 1: n | (n + 1) << 8, 4 <= n <= 128
/// - word 2: 0xd0 with primitive data (C > 0), otherwise 0x10
/// - word 10: (P - 1) | (ceil(P / 16) - 1) << 16, 1 <= P <= 512, no culling
/// - word 11: T16 or T32 mesh state, as in modes 1 and 2
/// - word 12: P * C, 0 <= C <= 128
/// - word 13: 1 + ceil(V / 4) * n, 1 <= V <= 256
///
/// Primitive data adds one link plane to n. The application output, vertex
/// planes without that link plus 4 * P * C bytes of primitive data, must fit
/// the compiler's 32 KiB mesh-output limit. Words 3, 4 and 9 keep their mode
/// 1/2 values; words 5..8 are launch handles, as in the other modes. The object
/// payload is not a packet field. Its exclusive storage is fixed at 16 KiB,
/// the largest payload in the study. Everything is computed from the 13 words
/// already read, so this adds no command-memory reads.
fn research_state(state: &[u32; 13]) -> bool {
    let planes = state[0] & 0xff;
    let primitives = (state[9] & 0xffff) + 1;
    let data = state[1] == 0xd0;
    if !(4..=128).contains(&planes)
        || state[0] != planes | ((planes + 1) << 8)
        || !matches!(state[1], 0x10 | 0xd0)
        || state[2] != 0
        || state[3] != 0
        || state[8] != 1
        || primitives > 512
        || state[9] != (primitives - 1) | ((primitives.div_ceil(16) - 1) << 16)
        || !matches!(state[10], 0x400810 | 0x400820)
    {
        return false;
    }
    let components = state[11] / primitives;
    if state[11] % primitives != 0
        || data != (components != 0)
        || components > 128
        || (data && planes < 5)
    {
        return false;
    }
    // The row count includes the header row. At most 64 vertex quads (V <= 256).
    let rows = state[12].wrapping_sub(1);
    let quads = rows / planes;
    if rows % planes != 0 || !(1..=64).contains(&quads) {
        return false;
    }
    // Bounded above: at most 64 * 16 * 128 + 4 * 512 * 128 bytes.
    quads * 16 * (planes - u32::from(data)) + 4 * primitives * components <= 0x8000
}
