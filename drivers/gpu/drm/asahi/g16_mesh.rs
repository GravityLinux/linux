// SPDX-License-Identifier: GPL-2.0-only
// Copyright 2026 The Asahi Linux Contributors

//! Native object/mesh launch for the paired experiment.
//!
//! The render UAPI does not distinguish a user object shader from a stub that
//! writes outputs. `native_mesh=1/2/3` selects the user-object convention. A paired
//! stream starts with a complete mesh state; there is no search for magic words
//! inside shader data, PPP records, or ordinary vertex streams.

/// Runtime copy of `asahi_m4.native_mesh`, writable from debugfs
/// `asahi_m4/native_mesh` without a reboot, and the generation of its last
/// change: `generation << 8 | mode` in one word, so a writer, a VM's latch and
/// the advertisement always see a consistent pair. A VM latches the mode when
/// it is created (`Vm::native_mode`); its support header and every decode use
/// that latch, so a later change affects only VMs created after it.
static NATIVE_MESH: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The highest accepted `native_mesh` value.
pub(crate) const MAX_MODE: u32 = 4;

/// The current (generation, mode).
pub(crate) fn snapshot() -> (u64, u32) {
    let value = NATIVE_MESH.load(core::sync::atomic::Ordering::Acquire);
    (value >> 8, (value & 0xff) as u32)
}

pub(crate) fn mode() -> u32 {
    snapshot().1
}

/// The boot value: generation 0.
pub(crate) fn init_mode(mode: u32) {
    NATIVE_MESH.store(u64::from(mode.min(MAX_MODE)), core::sync::atomic::Ordering::Release);
}

/// A runtime change. Every call, even to the current mode, starts a new
/// generation, so a reader that sees one generation twice knows the mode held
/// in between. Returns the new generation.
pub(crate) fn set_mode(mode: u32) -> u64 {
    use core::sync::atomic::Ordering::{AcqRel, Acquire};
    let next = |value: u64| (((value >> 8) + 1) << 8) | u64::from(mode.min(MAX_MODE));
    let previous = NATIVE_MESH.fetch_update(AcqRel, Acquire, |value| Some(next(value)));
    next(previous.unwrap_or_else(|value| value)) >> 8
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Disabled,
    SingleParent,
    MultipleParents,
    /// Research admission for the capability study: the wider state model in
    /// `research_state`, with mode 2's object dispatch rules. Not qualified.
    Research,
    /// Exactly the 31 device-qualified native-index classes (`QUALIFIED`), with
    /// the single-parent dispatches they were qualified with: Honeykrisp's
    /// (16, 1, 1) and the scalar (1, 1, 1) fixture carrier.
    Qualified,
}

impl Mode {
    pub(crate) fn from_parameter(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Disabled),
            1 => Some(Self::SingleParent),
            2 => Some(Self::MultipleParents),
            3 => Some(Self::Research),
            4 => Some(Self::Qualified),
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
/// The first word of a native object/mesh stream.
const NATIVE_HEADER: u32 = 0x4000_0d2c;

/// Whether a stream starting with `header` may be native: the native header,
/// or an unfamiliar header with the native state extension (bit 11), which
/// `decode` rejects. Any other first word makes `decode` return `Ok(None)`
/// without reading further, so an ordinary render needs nothing else.
pub(crate) fn may_be_native(header: u32) -> bool {
    header == NATIVE_HEADER || (header >> 29 == 2 && header & (1 << 11) != 0)
}

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
    if header != NATIVE_HEADER {
        // Bit 11 marks the native state extension. Reject unfamiliar native
        // layouts rather than submitting them with ordinary TA state.
        return if may_be_native(header) {
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
    } else if mode == Mode::Qualified {
        if !qualified_state(&state) {
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
                    || (mode == Mode::Qualified
                        && (!matches!(local, [1, 1, 1] | [16, 1, 1]) || total != local))
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

/// The qualified layouts (`native_mesh=4`): (mesh state word 11, primitive
/// word 10, vertex row word 13) of the 31 classes in Mesa's
/// `hk_apple9_native_index_shapes.def`, from their qualification fixtures. The
/// kernel sees vertices only as ceil(V / 4) quads, so V3 and V4 share a layout
/// at P1 and P2; the exact class is Mesa's. T16V16P16 is deliberately absent.
const QUALIFIED: [(u32, u32, u32); 29] = [
    // T16, one vertex quad (V3, V4): P1..P8, P11, P17, P20, P21.
    (0x400810, 0, 5), (0x400810, 1, 5), (0x400810, 2, 5), (0x400810, 3, 5),
    (0x400810, 4, 5), (0x400810, 5, 5), (0x400810, 6, 5), (0x400810, 7, 5),
    (0x400810, 10, 5), (0x400810, 0x10010, 5), (0x400810, 0x10013, 5), (0x400810, 0x10014, 5),
    // T16, two quads (V6, V8): P2, P3, P5, P8.
    (0x400810, 1, 9), (0x400810, 2, 9), (0x400810, 4, 9), (0x400810, 7, 9),
    // T16, four quads (V16): P4, P8, P17, P21.
    (0x400810, 3, 17), (0x400810, 7, 17), (0x400810, 0x10010, 17), (0x400810, 0x10014, 17),
    // T32: V3P1; V24 (six quads) P3, P17, P31, P32; V32 (eight quads) P16, P32, P41, P42.
    (0x400820, 0, 5),
    (0x400820, 2, 25), (0x400820, 0x10010, 25), (0x400820, 0x1001e, 25), (0x400820, 0x1001f, 25),
    (0x400820, 15, 33), (0x400820, 0x1001f, 33), (0x400820, 0x20028, 33), (0x400820, 0x20029, 33),
];

/// Mode 4's state check: position-only (four planes, no primitive data or
/// components), as qualified, at one of the `QUALIFIED` layouts. The object
/// dispatch must be a single parent of (16, 1, 1), as Honeykrisp emits, or the
/// scalar (1, 1, 1) of the qualification fixtures.
fn qualified_state(state: &[u32; 13]) -> bool {
    state[0] == 0x504
        && state[1] == 0x10
        && state[2] == 0
        && state[3] == 0
        && state[8] == 1
        && state[11] == 0
        && QUALIFIED.contains(&(state[10], state[9], state[12]))
}

/// sha256 of the canonical class list `QUALIFIED` was taken from: one line
/// `T<t>V<v>P<p>\n` per `HK_NATIVE_INDEX_SHAPE(t, v, p)` of Mesa's
/// `hk_apple9_native_index_shapes.def`, in file order (31 classes).
pub(crate) const QUALIFIED_CLASSES_SHA256: &str =
    "5550ecf263192c596c3f9d8c3b84c0324ac3d0c8ee76da37c378205247f1a3bd";

/// The longest advertisement line, newline included.
pub(crate) const ADVERTISEMENT_MAX: usize = 160;

/// The native-index advertisement (sysfs `asahi_native_mesh` on the GPU
/// device), one line: `asahi-native-mesh v1 generation=<G> profile=<P>
/// classes=<H>`. `qualified` (mode 4) carries the class-list hash, `research`
/// (mode 3) and `none` (modes 0-2) carry `-`. Userspace treats only
/// `qualified` with the expected hash as positive, and samples it before and
/// after VM_CREATE (see `snapshot`).
pub(crate) fn advertise(generation: u64, mode: u32, out: &mut impl core::fmt::Write) -> core::fmt::Result {
    let (profile, classes) = match Mode::from_parameter(mode) {
        Some(Mode::Qualified) => ("qualified", QUALIFIED_CLASSES_SHA256),
        Some(Mode::Research) => ("research", "-"),
        _ => ("none", "-"),
    };
    writeln!(out, "asahi-native-mesh v1 generation={generation} profile={profile} classes={classes}")
}

/// Research state model (`native_mesh=3`), from the 2026-10-22 capability
/// study's own-MSL captures: n vertex planes, P primitives, ceil(V / 4) vertex
/// quads and C 32-bit primitive components.
///
/// - word 1: n | (n + 1) << 8, 4 <= n <= 128
/// - word 2: 0xd0 with primitive data (C > 0), otherwise 0x10
/// - word 10: (P - 1) | (ceil(P / 16) - 1) << 16, 1 <= P <= 512, no culling
/// - word 11: T16 or T32 mesh state, as in modes 1 and 2. Physical W64
///   (0x400840) only for one probe shape: 4 vertex planes, one primitive, no
///   primitive data, one vertex quad. That admits the W64 study's streams
///   and qualifies neither W64 completion nor the fixed TA support resources.
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
/// The one research shape admitted at physical W64 (see `research_state`).
fn w64_probe(state: &[u32; 13]) -> bool {
    state[10] == 0x400840
        && state[0] == 0x504
        && state[1] == 0x10
        && state[9] == 0
        && state[11] == 0
        && state[12] == 5
}

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
        || !(matches!(state[10], 0x400810 | 0x400820) || w64_probe(state))
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
