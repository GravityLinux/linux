// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Scheduled-family hardware data, separate from older Asahi initdata.
//!
//! Byte-array fields describe owned staging storage, never live DMA borrows.
//! Large constant templates live in static storage so construction does not
//! put a whole 0x3db4-byte firmware object on the kernel stack.

use super::scheduled_initdata::{HardwareRegion, MmioRegion};
use kernel::macros::versions;

#[path = "g17p_constants.rs"]
mod g17p_constants;

pub(crate) const SIZE: usize = 0x3db4;

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct ClockVoltageTable {
    pub(crate) frequency: [[u8; 4]; 11],
    pub(crate) pad_2c: [u8; 0x14],
    pub(crate) core: [[[u8; 4]; 16]; 11],
    pub(crate) pad_300: [u8; 0x140],
    pub(crate) memory: [[[u8; 4]; 16]; 11],
}

impl Default for ClockVoltageTable {
    fn default() -> Self {
        Self {
            frequency: [[0; 4]; 11],
            pad_2c: [0; 0x14],
            core: [[[0; 4]; 16]; 11],
            pad_300: [0; 0x140],
            memory: [[[0; 4]; 16]; 11],
        }
    }
}

impl ClockVoltageTable {
    fn as_bytes(&self) -> &[u8] {
        // SAFETY: fully initialized byte fields, no implicit padding.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), 0x700) }
    }
}

#[versions(AGX_SCHEDULED)]
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct HardwareData {
    pub(crate) prefix: [u8; 0x640],
    pub(crate) registers: [MmioRegion; 53],
    pub(crate) hardware_status: [u8; 8],
    pub(crate) chip: [u8; 4],
    pub(crate) revision_major: [u8; 4],
    pub(crate) revision_minor: [u8; 4],
    pub(crate) pad_gpc: [u8; 0x4],
    pub(crate) gpc: [u8; 4],
    pub(crate) policy_ea4: [u8; 4],
    pub(crate) pad_policy_eb8: [u8; 0x10],
    pub(crate) policy_eb8: [u8; 4],
    pub(crate) pad_policy_ec0: [u8; 0x4],
    pub(crate) policy_ec0: [u8; 4],
    pub(crate) pad_policy_ec8: [u8; 0x4],
    pub(crate) policy_ec8: [u8; 4],
    pub(crate) pad_clock_mhz: [u8; 0x4],
    pub(crate) clock_mhz: [u8; 4],
    pub(crate) policy_ed4: [u8; 4],
    pub(crate) period_ms: [u8; 4],
    pub(crate) pad_policy_ee0: [u8; 0x4],
    pub(crate) policy_ee0: [u8; 4],
    pub(crate) policy_ee4: [u8; 4],
    pub(crate) policy_ee8: [u8; 4],
    pub(crate) pad_policy_f04: [u8; 0x18],
    pub(crate) policy_f04: [u8; 4],
    pub(crate) pad_policy_f24: [u8; 0x1c],
    pub(crate) policy_f24: [u8; 4],
    pub(crate) policy_f28: [u8; 4],
    pub(crate) pad_policy_f34: [u8; 0x8],
    pub(crate) policy_f34: [u8; 4],
    pub(crate) policy_f38: [u8; 4],
    pub(crate) pad_policy_f4c: [u8; 0x10],
    pub(crate) policy_f4c: [u8; 4],
    pub(crate) pad_policy_f6c: [u8; 0x1c],
    pub(crate) policy_f6c: [u8; 4],
    pub(crate) pad_core_count: [u8; 0x18],
    pub(crate) core_count: [u8; 4],
    pub(crate) policy_f8c: [u8; 4],
    pub(crate) pad_policy_fac: [u8; 0x1c],
    pub(crate) policy_fac: [u8; 4],
    pub(crate) pad_policy_fb8: [u8; 0x8],
    pub(crate) policy_fb8: [u8; 4],
    pub(crate) cluster_count: [u8; 4],
    pub(crate) core_count_copy: [u8; 4],
    pub(crate) max_state_a: [u8; 4],
    pub(crate) table_a: ClockVoltageTable,
    pub(crate) pad_freq_b: [u8; 0x140],
    pub(crate) freq_b: [[u8; 4]; 11],
    pub(crate) pad_scale_b: [u8; 0x14],
    pub(crate) scale_b: [[u8; 4]; 11],
    pub(crate) pad_relative_a: [u8; 0x54],
    pub(crate) relative_a: [[u8; 4]; 11],
    pub(crate) pad_relative_b: [u8; 0x14],
    pub(crate) relative_b: [[u8; 4]; 11],
    pub(crate) pad_index_a: [u8; 0x94],
    pub(crate) index_a: [[u8; 4]; 11],
    pub(crate) pad_index_b: [u8; 0x14],
    pub(crate) index_b: [[u8; 4]; 11],
    pub(crate) pad_cs_max_state: [u8; 0x15c],
    pub(crate) cs_max_state: [u8; 4],
    pub(crate) cs_max_state_copy: [u8; 4],
    pub(crate) cs_frequency: [[u8; 4]; 16],
    pub(crate) cs_core_voltage: [[u8; 8]; 16],
    pub(crate) cs_memory_voltage: [[u8; 8]; 16],
    pub(crate) max_state_b: [u8; 4],
    pub(crate) table_b: ClockVoltageTable,
    pub(crate) pad_policy_2540: [u8; 0x164],
    #[ver(G == G16C)]
    pub(crate) policy_2540: [u8; 0x56],
    #[ver(G == G17P)]
    pub(crate) policy_2540: [u8; 0x4e],
    // Neo's field is its twelve work channels. Max carries 46 here; its
    // meaning is unresolved, so do not give it Neo's semantic name.
    #[ver(G == G16C)]
    pub(crate) unk_2596: [u8; 2],
    #[ver(G == G17P)]
    pub(crate) channel_count: [u8; 2],
    pub(crate) flag_groups: [[[u8; 4]; 4]; 2],
    pub(crate) pad_unassigned: [u8; 0x4],
    pub(crate) unassigned: [[u8; 4]; 12],
    #[ver(G == G16C)]
    pub(crate) transport_policy: [u8; 0x30],
    #[ver(G == G17P)]
    pub(crate) transport_policy: [u8; 0x2c],
    pub(crate) regions: [HardwareRegion; 2],
    pub(crate) pad_computed: [u8; 0x1c],
    pub(crate) computed: [u8; 8],
    #[ver(G == G16C)]
    pub(crate) policy_tail: [u8; 0x40],
    #[ver(G == G17P)]
    pub(crate) policy_tail: [u8; 0x4c],
    pub(crate) pad_sentinel_3d98: [u8; 0x1698],
    pub(crate) sentinel_3d98: [u8; 4],
    pub(crate) pad_sentinel_3db0: [u8; 0x14],
    pub(crate) sentinel_3db0: [u8; 4],
}

#[versions(AGX_SCHEDULED)]
impl HardwareData::ver {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: every field is initialized, alignment one, no padding.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), SIZE) }
    }
}

const fn write32(out: &mut [u8], at: usize, value: u32) {
    let bytes = value.to_le_bytes();
    let mut i = 0;
    while i < 4 {
        out[at + i] = bytes[i];
        i += 1;
    }
}

const fn runs(out: &mut [u8], runs: &[(usize, &[u8])]) {
    let mut i = 0;
    while i < runs.len() {
        let (at, bytes) = runs[i];
        let mut j = 0;
        while j < bytes.len() {
            out[at + j] = bytes[j];
            j += 1;
        }
        i += 1;
    }
}

const fn template_g17p_transport() -> HardwareDataG17P {
    // SAFETY: this wire type consists exclusively of byte arrays.
    let mut raw: HardwareDataG17P = unsafe { core::mem::zeroed() };
    raw.channel_count = 12u16.to_le_bytes();
    raw.flag_groups = [[
        [0; 4],
        1u32.to_le_bytes(),
        1u32.to_le_bytes(),
        1u32.to_le_bytes(),
    ]; 2];
    raw.unassigned = [u32::MAX.to_le_bytes(); 12];
    // SAFETY: owned staging, byte fields only, alignment one, no padding.
    let bytes =
        unsafe { core::slice::from_raw_parts_mut(core::ptr::from_mut(&mut raw).cast(), SIZE) };
    write32(bytes, 0x25f4, 1);
    write32(bytes, 0x2600, 1);
    write32(bytes, 0x2608, 1);
    raw
}

const fn template_g17p() -> HardwareDataG17P {
    let mut raw = template_g17p_transport();
    // SAFETY: owned staging, byte fields only, alignment one, no padding.
    let bytes =
        unsafe { core::slice::from_raw_parts_mut(core::ptr::from_mut(&mut raw).cast(), SIZE) };
    runs(bytes, g17p_constants::HWDATA_CONSTANTS);
    raw
}

const MAX_POLICY: &[(usize, u32)] = &[
    (0xea4, 0x1),
    (0xec0, 0x1),
    (0xed4, 0x1),
    (0xf04, 0x1f),
    (0xf24, 0x7002004),
    (0xf28, 0x6),
    (0xf34, 0x1),
    (0xf38, 0x1),
    (0xf4c, 0x31),
    (0xf6c, 0x1),
    (0xf8c, 0xff),
    (0xfac, 0x1),
    (0xfb8, 0x1d),
    (0x2544, 0x5),
    (0x2548, 0x6),
    (0x254c, 0x3),
    (0x2550, 0x7),
    (0x2554, 0x7),
    (0x2570, 0x28),
    (0x2578, 0x1),
    (0x25f4, 0x2),
    (0x25fc, 0x1),
    (0x2610, 0x1),
    (0x2614, 0x1),
    (0x26c0, 0x1),
    (0x26c4, 0x1),
    (0x26f4, 0x1),
    (0x3d98, 0xffffffff),
    (0x3db0, 0xffffffff),
];
const COMMON_PREFIX: &[(usize, &[u8])] = &[
    (0x4, &[0x6f, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc0, 0xff]),
    (
        0x14,
        &[
            0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x80,
            0xff, 0xff, 0xff, 0x02, 0x00, 0x00,
        ],
    ),
    (0xe0, &[0x08, 0x20]),
    (0xea, &[0x08, 0x20]),
    (
        0xf4,
        &[
            0x08, 0x20, 0x00, 0x00, 0xcb, 0x24, 0x00, 0x00, 0xfa, 0x2c, 0x8a, 0xc9, 0xcb, 0x24,
            0xf6, 0xf4, 0x17, 0xe9, 0x77, 0x18, 0xcb, 0x24, 0xd9, 0x38, 0x00, 0x00, 0xab, 0xbd,
        ],
    ),
    (
        0x2d9,
        &[
            0x20, 0xf8, 0xff, 0xdb, 0x2c, 0x2d, 0xd3, 0x00, 0x20, 0x00, 0xf5, 0x26, 0xe9, 0xda,
            0x21, 0x00, 0x20, 0xb6, 0x38, 0x08, 0x00, 0x42, 0xc7,
        ],
    ),
    (0x3e0, &[0xe0, 0x7f]),
    (0x3ea, &[0xe0, 0x7f]),
    (0x3f4, &[0xe0, 0x7f, 0x00, 0x00, 0x00, 0x80]),
    (0x403, &[0x80]),
    (0x40d, &[0x80]),
    (
        0x5d8,
        &[
            0x45, 0x26, 0x23, 0x4b, 0x98, 0x0e, 0x00, 0x00, 0x5f, 0xea, 0xa2, 0xd5, 0xff, 0x3f,
            0x00, 0x40, 0x00, 0x40, 0x5e, 0xca, 0xa2, 0xf5, 0x00, 0x40,
        ],
    ),
];
pub(crate) const UNMAPPED_SLOTS: &[usize] = &[2, 5, 6, 7, 8, 27, 30, 38, 42, 46, 47, 48, 49, 52];

const fn template_g16c() -> HardwareDataG16C {
    // SAFETY: this wire type consists exclusively of byte arrays.
    let mut raw: HardwareDataG16C = unsafe { core::mem::zeroed() };
    raw.chip = 0x6041u32.to_le_bytes();
    raw.policy_eb8 = 1u32.to_le_bytes();
    raw.policy_ec8 = 1u32.to_le_bytes();
    raw.clock_mhz = 24000u32.to_le_bytes();
    raw.policy_ee0 = 1u32.to_le_bytes();
    raw.policy_ee4 = 1u32.to_le_bytes();
    raw.policy_ee8 = 1u32.to_le_bytes();
    raw.core_count = 40u32.to_le_bytes();
    raw.cluster_count = 4u32.to_le_bytes();
    raw.core_count_copy = 40u32.to_le_bytes();
    raw.max_state_a = 10u32.to_le_bytes();
    raw.max_state_b = 10u32.to_le_bytes();
    raw.scale_b = [0x3f828f5cu32.to_le_bytes(); 11]; // f32(1.02)
    raw.unk_2596 = 46u16.to_le_bytes();
    raw.flag_groups = [[
        [0; 4],
        1u32.to_le_bytes(),
        1u32.to_le_bytes(),
        1u32.to_le_bytes(),
    ]; 2];
    raw.unassigned = [u32::MAX.to_le_bytes(); 12];
    // SAFETY: owned staging, byte fields only, alignment one, no padding.
    let bytes =
        unsafe { core::slice::from_raw_parts_mut(core::ptr::from_mut(&mut raw).cast(), SIZE) };
    runs(bytes, COMMON_PREFIX);
    let mut i = 0;
    while i < MAX_POLICY.len() {
        write32(bytes, MAX_POLICY[i].0, MAX_POLICY[i].1);
        i += 1;
    }
    i = 0;
    while i < UNMAPPED_SLOTS.len() {
        write32(bytes, 0x640 + UNMAPPED_SLOTS[i] * 0x28 + 0x20, 2);
        i += 1;
    }
    raw
}

static G16C_TEMPLATE: HardwareDataG16C = template_g16c();
static G17P_TRANSPORT_TEMPLATE: HardwareDataG17P = template_g17p_transport();
static G17P_TEMPLATE: HardwareDataG17P = template_g17p();

impl Default for HardwareDataG16C {
    fn default() -> Self {
        template_g16c()
    }
}
impl Default for HardwareDataG17P {
    fn default() -> Self {
        template_g17p()
    }
}

pub(crate) fn template_g16c_bytes() -> &'static [u8] {
    G16C_TEMPLATE.as_bytes()
}
/// Required transport defaults before the caller's final policy overlay.
/// Separate from Default to preserve the existing explicit-opaque encoder API.
pub(crate) fn template_g17p_transport_bytes() -> &'static [u8] {
    G17P_TRANSPORT_TEMPLATE.as_bytes()
}

pub(crate) fn template_g17p_bytes() -> &'static [u8] {
    G17P_TEMPLATE.as_bytes()
}

#[derive(Clone, Copy)]
pub(crate) struct CsState {
    pub(crate) frequency: u32,
    pub(crate) core_voltage: u32,
    pub(crate) memory_voltage: u32,
}

/// Board-derived values are constructor inputs, not another wire-layout axis.
pub(crate) struct ParametersG16C<'a> {
    pub(crate) registers: &'a [(usize, MmioRegion)],
    pub(crate) freq_a: &'a [u32; 11],
    pub(crate) freq_b: &'a [u32; 11],
    pub(crate) core_voltage: &'a [u32; 11],
    pub(crate) memory_voltage: &'a [u32; 11],
    pub(crate) cluster_core: &'a [[u32; 4]; 11],
    pub(crate) cluster_memory: &'a [[u32; 4]; 11],
    pub(crate) relative_a: &'a [u32; 11],
    pub(crate) relative_b: &'a [u32; 11],
    pub(crate) index_a: &'a [u32; 11],
    pub(crate) index_b: &'a [u32; 11],
    pub(crate) cs: &'a [CsState],
    pub(crate) revision: u32,
    pub(crate) gpc: bool,
    pub(crate) period_ms: u32,
    pub(crate) hardware_status: u64,
    pub(crate) computed: u64,
    pub(crate) timestamp: u64,
    pub(crate) regions: Option<&'a [HardwareRegion; 2]>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InvalidGeometry;

// Populate individual raw table fields directly; never stage the complete
// hardware object on the stack. The initial template comes from static storage.
fn populate_table(
    raw: &mut ClockVoltageTable,
    frequency: &[u32; 11],
    columns: usize,
    voltage: impl Fn(usize, usize) -> (u32, u32),
) {
    *raw = ClockVoltageTable::default();
    for state in 0..11 {
        raw.frequency[state] = frequency[state].to_le_bytes();
        for word in 0..columns {
            let (core, memory) = voltage(state, word);
            raw.core[state][word] = core.to_le_bytes();
            raw.memory[state][word] = memory.to_le_bytes();
        }
    }
}

/// G17P uses scalar voltages repeated across sixteen / one columns. G16C
/// supplies independent cluster columns to the same raw table constructor.
#[inline(never)]
pub(crate) fn tables_g17p(
    out: &mut [u8],
    freq_a: &[u32; 11],
    freq_b: &[u32; 11],
    core: &[u32; 11],
    memory: &[u32; 11],
) {
    let mut raw = ClockVoltageTable::default();
    for (at, frequency, columns) in [(0xfc8, freq_a, 16), (0x1cdc, freq_b, 1)] {
        populate_table(&mut raw, frequency, columns, |state, _| {
            (core[state], memory[state])
        });
        out[at..at + 0x700].copy_from_slice(raw.as_bytes());
    }
}

impl ParametersG16C<'_> {
    pub(crate) fn validate(&self) -> Result<(), InvalidGeometry> {
        if self.cs.is_empty()
            || self.cs.len() > 16
            || self.period_ms == 0
            || self.hardware_status & 0x3fff != 0
            || self.computed & 0x3fff != 0
        {
            return Err(InvalidGeometry);
        }
        let mut seen = [false; 53];
        for (slot, _) in self.registers {
            if *slot >= 53 || seen[*slot] || UNMAPPED_SLOTS.contains(slot) {
                return Err(InvalidGeometry);
            }
            seen[*slot] = true;
        }
        Ok(())
    }

    pub(crate) fn build(&self, out: &mut [u8]) -> Result<(), InvalidGeometry> {
        if out.len() < SIZE {
            return Err(InvalidGeometry);
        }
        self.validate()?;
        // All caller-controlled geometry is validated before any mutation.
        out[..SIZE].copy_from_slice(template_g16c_bytes());
        // SAFETY: `out` is exclusive, initialized host staging storage;
        // validated SIZE bytes are available. HardwareDataG16C has alignment
        // one, no padding, and exclusively byte-array fields (all bit patterns
        // valid). This reference never escapes to firmware-mutated memory.
        let raw = unsafe { &mut *out.as_mut_ptr().cast::<HardwareDataG16C>() };
        for (slot, register) in self.registers {
            raw.registers[*slot] = *register;
        }
        raw.revision_major = (self.revision >> 4).to_le_bytes();
        raw.revision_minor = (self.revision & 7).to_le_bytes();
        raw.gpc = u32::from(self.gpc).to_le_bytes();
        raw.period_ms = self.period_ms.to_le_bytes();
        raw.prefix[0x28..0x30].copy_from_slice(&self.timestamp.to_le_bytes());
        raw.hardware_status = self.hardware_status.to_le_bytes();
        raw.computed = self.computed.to_le_bytes();
        let voltage = |state: usize, word: usize| {
            if word < 4 {
                (
                    self.cluster_core[state][word],
                    self.cluster_memory[state][word],
                )
            } else {
                (self.core_voltage[state], self.memory_voltage[state])
            }
        };
        populate_table(&mut raw.table_a, self.freq_a, 16, voltage);
        populate_table(&mut raw.table_b, self.freq_b, 4, voltage);
        for (target, ladder) in [
            (&mut raw.freq_b, self.freq_b),
            (&mut raw.relative_a, self.relative_a),
            (&mut raw.relative_b, self.relative_b),
            (&mut raw.index_a, self.index_a),
            (&mut raw.index_b, self.index_b),
        ] {
            for (target, value) in target.iter_mut().zip(ladder) {
                *target = value.to_le_bytes();
            }
        }
        raw.cs_max_state = (self.cs.len() as u32 - 1).to_le_bytes();
        raw.cs_max_state_copy = raw.cs_max_state;
        for (i, state) in self.cs.iter().enumerate() {
            raw.cs_frequency[i] = state.frequency.to_le_bytes();
            raw.cs_core_voltage[i] = u64::from(state.core_voltage).to_le_bytes();
            raw.cs_memory_voltage[i] = u64::from(state.memory_voltage).to_le_bytes();
        }
        if let Some(regions) = self.regions {
            raw.regions = *regions;
        }
        Ok(())
    }
}

const _: () = {
    assert!(core::mem::size_of::<ClockVoltageTable>() == 0x700);
    assert!(core::mem::align_of::<ClockVoltageTable>() == 1);
    assert!(core::mem::offset_of!(ClockVoltageTable, frequency) == 0);
    assert!(core::mem::offset_of!(ClockVoltageTable, core) == 0x40);
    assert!(core::mem::offset_of!(ClockVoltageTable, memory) == 0x440);
    assert!(core::mem::size_of::<HardwareDataG16C>() == SIZE);
    assert!(core::mem::size_of::<HardwareDataG17P>() == SIZE);
    assert!(core::mem::align_of::<HardwareDataG16C>() == 1);
    assert!(core::mem::align_of::<HardwareDataG17P>() == 1);
    assert!(core::mem::offset_of!(HardwareDataG16C, prefix) == 0x0);
    assert!(core::mem::offset_of!(HardwareDataG17P, prefix) == 0x0);
    assert!(core::mem::offset_of!(HardwareDataG16C, registers) == 0x640);
    assert!(core::mem::offset_of!(HardwareDataG17P, registers) == 0x640);
    assert!(core::mem::offset_of!(HardwareDataG16C, hardware_status) == 0xe88);
    assert!(core::mem::offset_of!(HardwareDataG17P, hardware_status) == 0xe88);
    assert!(core::mem::offset_of!(HardwareDataG16C, chip) == 0xe90);
    assert!(core::mem::offset_of!(HardwareDataG17P, chip) == 0xe90);
    assert!(core::mem::offset_of!(HardwareDataG16C, revision_major) == 0xe94);
    assert!(core::mem::offset_of!(HardwareDataG17P, revision_major) == 0xe94);
    assert!(core::mem::offset_of!(HardwareDataG16C, revision_minor) == 0xe98);
    assert!(core::mem::offset_of!(HardwareDataG17P, revision_minor) == 0xe98);
    assert!(core::mem::offset_of!(HardwareDataG16C, gpc) == 0xea0);
    assert!(core::mem::offset_of!(HardwareDataG17P, gpc) == 0xea0);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_ea4) == 0xea4);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_ea4) == 0xea4);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_eb8) == 0xeb8);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_eb8) == 0xeb8);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_ec0) == 0xec0);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_ec0) == 0xec0);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_ec8) == 0xec8);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_ec8) == 0xec8);
    assert!(core::mem::offset_of!(HardwareDataG16C, clock_mhz) == 0xed0);
    assert!(core::mem::offset_of!(HardwareDataG17P, clock_mhz) == 0xed0);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_ed4) == 0xed4);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_ed4) == 0xed4);
    assert!(core::mem::offset_of!(HardwareDataG16C, period_ms) == 0xed8);
    assert!(core::mem::offset_of!(HardwareDataG17P, period_ms) == 0xed8);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_ee0) == 0xee0);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_ee0) == 0xee0);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_ee4) == 0xee4);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_ee4) == 0xee4);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_ee8) == 0xee8);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_ee8) == 0xee8);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_f04) == 0xf04);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_f04) == 0xf04);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_f24) == 0xf24);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_f24) == 0xf24);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_f28) == 0xf28);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_f28) == 0xf28);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_f34) == 0xf34);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_f34) == 0xf34);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_f38) == 0xf38);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_f38) == 0xf38);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_f4c) == 0xf4c);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_f4c) == 0xf4c);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_f6c) == 0xf6c);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_f6c) == 0xf6c);
    assert!(core::mem::offset_of!(HardwareDataG16C, core_count) == 0xf88);
    assert!(core::mem::offset_of!(HardwareDataG17P, core_count) == 0xf88);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_f8c) == 0xf8c);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_f8c) == 0xf8c);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_fac) == 0xfac);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_fac) == 0xfac);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_fb8) == 0xfb8);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_fb8) == 0xfb8);
    assert!(core::mem::offset_of!(HardwareDataG16C, cluster_count) == 0xfbc);
    assert!(core::mem::offset_of!(HardwareDataG17P, cluster_count) == 0xfbc);
    assert!(core::mem::offset_of!(HardwareDataG16C, core_count_copy) == 0xfc0);
    assert!(core::mem::offset_of!(HardwareDataG17P, core_count_copy) == 0xfc0);
    assert!(core::mem::offset_of!(HardwareDataG16C, max_state_a) == 0xfc4);
    assert!(core::mem::offset_of!(HardwareDataG17P, max_state_a) == 0xfc4);
    assert!(core::mem::offset_of!(HardwareDataG16C, table_a) == 0xfc8);
    assert!(core::mem::offset_of!(HardwareDataG17P, table_a) == 0xfc8);
    assert!(core::mem::offset_of!(HardwareDataG16C, freq_b) == 0x1808);
    assert!(core::mem::offset_of!(HardwareDataG17P, freq_b) == 0x1808);
    assert!(core::mem::offset_of!(HardwareDataG16C, scale_b) == 0x1848);
    assert!(core::mem::offset_of!(HardwareDataG17P, scale_b) == 0x1848);
    assert!(core::mem::offset_of!(HardwareDataG16C, relative_a) == 0x18c8);
    assert!(core::mem::offset_of!(HardwareDataG17P, relative_a) == 0x18c8);
    assert!(core::mem::offset_of!(HardwareDataG16C, relative_b) == 0x1908);
    assert!(core::mem::offset_of!(HardwareDataG17P, relative_b) == 0x1908);
    assert!(core::mem::offset_of!(HardwareDataG16C, index_a) == 0x19c8);
    assert!(core::mem::offset_of!(HardwareDataG17P, index_a) == 0x19c8);
    assert!(core::mem::offset_of!(HardwareDataG16C, index_b) == 0x1a08);
    assert!(core::mem::offset_of!(HardwareDataG17P, index_b) == 0x1a08);
    assert!(core::mem::offset_of!(HardwareDataG16C, cs_max_state) == 0x1b90);
    assert!(core::mem::offset_of!(HardwareDataG17P, cs_max_state) == 0x1b90);
    assert!(core::mem::offset_of!(HardwareDataG16C, cs_max_state_copy) == 0x1b94);
    assert!(core::mem::offset_of!(HardwareDataG17P, cs_max_state_copy) == 0x1b94);
    assert!(core::mem::offset_of!(HardwareDataG16C, cs_frequency) == 0x1b98);
    assert!(core::mem::offset_of!(HardwareDataG17P, cs_frequency) == 0x1b98);
    assert!(core::mem::offset_of!(HardwareDataG16C, cs_core_voltage) == 0x1bd8);
    assert!(core::mem::offset_of!(HardwareDataG17P, cs_core_voltage) == 0x1bd8);
    assert!(core::mem::offset_of!(HardwareDataG16C, cs_memory_voltage) == 0x1c58);
    assert!(core::mem::offset_of!(HardwareDataG17P, cs_memory_voltage) == 0x1c58);
    assert!(core::mem::offset_of!(HardwareDataG16C, max_state_b) == 0x1cd8);
    assert!(core::mem::offset_of!(HardwareDataG17P, max_state_b) == 0x1cd8);
    assert!(core::mem::offset_of!(HardwareDataG16C, table_b) == 0x1cdc);
    assert!(core::mem::offset_of!(HardwareDataG17P, table_b) == 0x1cdc);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_2540) == 0x2540);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_2540) == 0x2540);
    assert!(core::mem::offset_of!(HardwareDataG16C, unk_2596) == 0x2596);
    assert!(core::mem::offset_of!(HardwareDataG17P, channel_count) == 0x258e);
    assert!(core::mem::offset_of!(HardwareDataG16C, flag_groups) == 0x2598);
    assert!(core::mem::offset_of!(HardwareDataG17P, flag_groups) == 0x2590);
    assert!(core::mem::offset_of!(HardwareDataG16C, unassigned) == 0x25bc);
    assert!(core::mem::offset_of!(HardwareDataG17P, unassigned) == 0x25b4);
    assert!(core::mem::offset_of!(HardwareDataG16C, transport_policy) == 0x25ec);
    assert!(core::mem::offset_of!(HardwareDataG17P, transport_policy) == 0x25e4);
    assert!(core::mem::offset_of!(HardwareDataG16C, regions) == 0x261c);
    assert!(core::mem::offset_of!(HardwareDataG17P, regions) == 0x2610);
    assert!(core::mem::offset_of!(HardwareDataG16C, computed) == 0x26b8);
    assert!(core::mem::offset_of!(HardwareDataG17P, computed) == 0x26ac);
    assert!(core::mem::offset_of!(HardwareDataG16C, policy_tail) == 0x26c0);
    assert!(core::mem::offset_of!(HardwareDataG17P, policy_tail) == 0x26b4);
    assert!(core::mem::offset_of!(HardwareDataG16C, sentinel_3d98) == 0x3d98);
    assert!(core::mem::offset_of!(HardwareDataG17P, sentinel_3d98) == 0x3d98);
    assert!(core::mem::offset_of!(HardwareDataG16C, sentinel_3db0) == 0x3db0);
    assert!(core::mem::offset_of!(HardwareDataG17P, sentinel_3db0) == 0x3db0);
};
