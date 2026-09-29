// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Source-presented partial opening, shared by the Python shim's render and
//! compute paths. These are protocol objects, with no workload or shader data.
//! The opening counters are deliberately presented consumed, not evidence of
//! live command execution. Descriptor/queue pointers are installed with work.

pub(crate) const CONTEXT_PAGES: usize = 8;
pub(crate) const CONTEXTS: [(u64, u64); 2] = [
    (0xfffffc20001d8000, 0x7000438000),
    (0xfffffc2000200000, 0x7000460000),
];
pub(crate) const CHANNEL_CONTROL: u64 = 0xfffffc20c07b8000;
pub(crate) const SUPPORT: u64 = 0xfffffc20c0828000;
pub(crate) const STATE: u64 = 0xfffffc2001600000;
pub(crate) const OPERAND_TABLE: u64 = 0x7000208000;
pub(crate) const EXTRA_RENDER: [u64; 2] = [0x70017c0000, 0x7001088000];

const CONTEXT_WORDS: [&[(usize, u64)]; 2] = [
    &[
        (0x200, 0x1000000000000004),
        (0x220, 0xffff0c0000000001),
        (0x350, 0x0002380380000003),
        (0x378, 0x003fffffffffffff),
    ],
    &[
        (0x200, 0x1000040000000004),
        (0x220, 0xffff180000000003),
        (0x228, 1),
        (0x230, 0x10000000000),
        (0x350, 0x0002b00380004c05),
        (0x358, 0x0000800380004c3e),
        (0x360, 0x0000b80380004c77),
        (0x368, 0x0000500380004cb0),
        (0x378, 0x003fffffffffffff),
    ],
];

pub(crate) fn context(kind: usize) -> Option<[u8; 0x380]> {
    let mut out = [0; 0x380];
    for &(offset, value) in *CONTEXT_WORDS.get(kind)? {
        out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    Some(out)
}

pub(crate) fn channel_control() -> [u8; 0x40] {
    let mut out = [0; 0x40];
    for (offset, value) in [
        (0, 0x000001000000ffffu64),
        (0x20, 0x0002000000000000),
        (0x30, 0xff000000),
    ] {
        out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    out
}

pub(crate) fn message(secondary: bool) -> [u8; 0x40] {
    let mut out = [0; 0x40];
    out[0] = if secondary { 0x2a } else { 0x16 };
    out
}

/// build_compute_compact_control_support(class=2), cursor after registration.
pub(crate) fn support() -> [u8; 0x70] {
    let mut out = [0; 0x70];
    for (offset, value) in [(0, 1u32), (0x10, 2), (0x40, 4), (0x48, 0xe0), (0x60, 3)] {
        out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [
        (0x18, 0x0004000000000070u64),
        (0x20, 0x190000000000),
        (0x28, 0x190000000000),
        (0x30, OPERAND_TABLE),
        (0x4c, STATE),
    ] {
        out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    out
}

/// prepare_render_status_config(), after ACK and before control-done.
pub(crate) fn status_config(fwctl: u64) -> [(usize, u64); 5] {
    [
        (0x4018, 0x0005060100000000),
        (0x4020, 61),
        (0x40b0, 5),
        (0x48e0, fwctl),
        (0x48e8, fwctl + 0x40),
    ]
}
