// SPDX-License-Identifier: GPL-2.0-only

//! Scheduled-family compute descriptor DATA, macOS 26.6.2.
//! Only exclusive CPU staging is cast to a wire layout. Firmware mappings
//! remain behind Memory/Vm; the large Default image is copied from static RAM.

use kernel::macros::versions;

pub(crate) type Register = (u32, u64);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Error {
    Invalid,
    Overflow,
    MissingRegister,
    UnsupportedExecBase,
}

pub(crate) struct ProgramParameters {
    pub(crate) save_area: u64,
    pub(crate) cdm: u64,
    pub(crate) identity: u64,
    pub(crate) context: u32,
    pub(crate) ordinal: u32,
    pub(crate) robustness: u64,
    pub(crate) operand_state: u64,
    pub(crate) usc_exec_base: Option<u64>,
    pub(crate) helper_binary: u64,
    pub(crate) helper_data: u64,
    pub(crate) helper_config: u64,
    pub(crate) execution_gate: u64,
    pub(crate) register_a599: u64,
}

// Ordered writes, including repeated registers, are part of the ABI. This is
// CPU staging, not a repr(C) firmware record. Max's native Metal form omits the
// four queue-supplied USC/helper writes; the UAPI form includes all forty.
#[versions(AGX_SCHEDULED)]
pub(crate) struct ComputeProgram {
    registers: [Register; 40],
    count: usize,
}

#[versions(AGX_SCHEDULED)]
impl Default for ComputeProgram::ver {
    fn default() -> Self {
        Self {
            count: 40,
            registers: [
                (0x1a510, 0),
                (0x1a420, 0),
                (0x1a4d0, 0),
                (0x1a4d8, 0),
                (0x1a4e0, 0),
                (0x1a4e8, 0),
                (0x10071, 0),
                (0x11841, 0),
                (0x11849, 0),
                (0x11f81, 0),
                (0x1a440, 0x154024201),
                (0x1a458, 0x10c08860),
                (0x101d9, 0x1c),
                (0x1a089, 0),
                (0x1a091, 0),
                (0x1a059, 0),
                (0x1a061, 0),
                (0x1a0b9, 0),
                (0x1a0c1, 0),
                (0x101d1, 0),
                (0x0d479, 0),
                (0x1a0e9, 8),
                (0x107a1, 0xff0000),
                (0x0a599, 0x13200400020),
                (0x0d411, 0x200000001),
                (0x1a540, 0),
                (0x014a9, 0),
                (0x0a351, 0),
                (0x10201, 0),
                (0x10428, 0),
                (0x14028, 0),
                (0x14070, 0),
                (0x10229, 0),
                (0x140a8, 0),
                (0x10099, 0),
                (0x10091, 0),
                (0x0a5c1, 0),
                (0x0a5c9, 0),
                (0x1a440, 0x154024209),
                (0x0a599, 0x6000400020),
            ],
        }
    }
}

#[versions(AGX_SCHEDULED)]
impl ComputeProgram::ver {
    pub(crate) fn new(p: &ProgramParameters) -> core::result::Result<Self, Error> {
        // USC_EXEC_BASE is the operator-confirmed fixed UAPI exception.
        if p.usc_exec_base.is_some_and(|base| base != 1 << 40) {
            return Err(Error::UnsupportedExecBase);
        }
        #[ver(G == G17P)]
        if p.usc_exec_base.is_none() {
            return Err(Error::UnsupportedExecBase);
        }
        #[ver(G == G16C)]
        if !(1..64).contains(&p.context)
            || p.ordinal > 255
            || p.cdm == 0
            || p.cdm & 3 != 0
            || p.save_area == 0
            || p.save_area & 7 != 0
            || p.operand_state == 0
            || p.operand_state & 7 != 0
            || (p.usc_exec_base.is_none()
                && (p.helper_binary | p.helper_data | p.helper_config) != 0)
        {
            return Err(Error::Invalid);
        }
        #[ver(G == G16C)]
        let save_offsets = [0x2980, 0x2988, 0x2990, 0x2998];
        #[ver(G == G17P)]
        let save_offsets = [0x1480, 0x1488, 0x1490, 0x1498];
        #[ver(G == G16C)]
        let operand_offsets = [0x78800, 0x7a800, 0x3c405, 0x78400, 5, 0x3c000];
        #[ver(G == G17P)]
        let operand_offsets = [0x12800, 0x13000, 0x9405, 0x12400, 5, 0x9000];
        let mut raw = Self::default();
        raw.registers[0].1 = p.save_area;
        raw.registers[1].1 = p.cdm;
        for (i, offset) in save_offsets.into_iter().enumerate() {
            raw.registers[2 + i].1 = p.save_area.checked_add(offset).ok_or(Error::Overflow)?;
        }
        for (i, value) in [
            p.usc_exec_base.unwrap_or(0),
            p.helper_binary,
            p.helper_data,
            p.helper_config,
        ]
        .into_iter()
        .enumerate()
        {
            raw.registers[6 + i].1 = value;
        }
        raw.registers[23].1 = p.register_a599;
        for i in 25..28 {
            raw.registers[i].1 = p.identity;
        }
        let tag = ((p.context as u64) << 8) | (p.ordinal as u64 & 0xff);
        raw.registers[28].1 = tag;
        raw.registers[29].1 = tag;
        raw.registers[30].1 = p.execution_gate;
        raw.registers[31].1 = p.robustness | 1;
        for (i, offset) in operand_offsets.into_iter().enumerate() {
            raw.registers[32 + i].1 = p.operand_state.checked_add(offset).ok_or(Error::Overflow)?;
        }
        #[ver(G == G16C)]
        if p.usc_exec_base.is_none() {
            raw.registers.copy_within(10..40, 6);
            raw.registers[36..].fill((0, 0));
            raw.count = 36;
        }
        Ok(raw)
    }

    pub(crate) fn registers(&self) -> &[Register] {
        &self.registers[..self.count]
    }

    #[ver(G == G17P)]
    pub(crate) fn into_array(self) -> [Register; 40] {
        self.registers
    }
}

pub(crate) struct Parameters {
    pub(crate) save_area: u64,
    pub(crate) cdm: u64,
    pub(crate) identity: u64,
    pub(crate) sequence: u64,
    pub(crate) context: u32,
    pub(crate) scheduler: u64,
    pub(crate) alias: u64,
    pub(crate) terminator: u64,
    pub(crate) sampler_array: u64,
    pub(crate) sampler_count: u32,
    pub(crate) dispatch: [u64; 2],
    pub(crate) grid: u32,
    pub(crate) queue_submission: u32,
    pub(crate) queue_ordinal: u32,
    pub(crate) submission_index: u32,
    pub(crate) ordinal: u32,
    pub(crate) status: [u64; 2],
    pub(crate) timestamps: [u64; 2],
    pub(crate) support: u64,
    pub(crate) support_control: u32,
    pub(crate) support_flags: u32,
    pub(crate) support_kind: u16,
    pub(crate) support_phase: u8,
    pub(crate) zero_page: u64,
}

// Layout shared by both RE-backed source encoders. Register counts and support
// defaults differ; the two profiles intentionally retain distinct Rust types.
// The byte at 0xfcb belongs to zero_page, overwriting the high byte of the
// preceding ordinal word in both Python encoders. Model the final wire bytes
// as three ordinal bytes followed by the pointer, rather than overlapping Rust
// fields or relying on assignment order. The native packed object is 4 KiB;
// a standalone source allocation may use a zero-padded 16 KiB UAT page.
#[versions(AGX_SCHEDULED)]
#[repr(C)]
pub(crate) struct ComputeDescriptor {
    pub(crate) opcode: [u8; 4],
    pub(crate) sequence: [u8; 8],
    pub(crate) context: [u8; 4],
    pub(crate) scheduler: [u8; 8],
    pub(crate) counts: [[u8; 2]; 4],
    pub(crate) pad_0020: [u8; 0x20],
    pub(crate) primary: [[u8; 12]; 128],
    pub(crate) pad_0640: [u8; 0x100],
    pub(crate) primary_address: [u8; 8],
    pub(crate) primary_count: [u8; 4],
    pub(crate) pad_074c: [u8; 0x14],
    pub(crate) secondary: [[u8; 12]; 128],
    pub(crate) pad_0d60: [u8; 0x100],
    pub(crate) secondary_address: [u8; 8],
    pub(crate) secondary_count: [u8; 4],
    pub(crate) pad_0e6c: [u8; 0x6c],
    pub(crate) save_area: [u8; 8],
    pub(crate) terminator: [u8; 8],
    pub(crate) pad_0ee8: [u8; 0x20],
    pub(crate) control: [u8; 8],
    pub(crate) pad_0f10: [u8; 0x10],
    pub(crate) identity_high: [u8; 4],
    pub(crate) pad_0f24: [u8; 4],
    pub(crate) sentinel: [u8; 4],
    pub(crate) sampler_array: [u8; 8],
    pub(crate) sampler_count: [u8; 4],
    pub(crate) sampler_limit: [u8; 4],
    pub(crate) pad_0f3c: [u8; 4],
    pub(crate) dispatch: [[u8; 8]; 2],
    pub(crate) queue_submission: [u8; 4],
    pub(crate) grid: [u8; 4],
    pub(crate) queue_ordinal: [u8; 4],
    pub(crate) pad_0f5c: [u8; 4],
    pub(crate) submission_index: [u8; 4],
    pub(crate) pad_0f64: [u8; 4],
    pub(crate) identity_low: [u8; 8],
    pub(crate) ordinal: [u8; 4],
    pub(crate) pad_0f74: [u8; 8],
    pub(crate) status: [[u8; 8]; 2],
    pub(crate) timestamps: [[u8; 8]; 2],
    pub(crate) pad_0f9c: [u8; 0x14],
    pub(crate) support_opcode: [u8; 2],
    pub(crate) support: [u8; 8],
    pub(crate) support_control: [u8; 4],
    pub(crate) support_flags: [u8; 4],
    pub(crate) pad_0fc2: [u8; 3],
    pub(crate) support_kind: [u8; 2],
    pub(crate) pad_0fc7: [u8; 1],
    pub(crate) ordinal_overlap: [u8; 3],
    pub(crate) zero_page: [u8; 8],
    pub(crate) support_phase: [u8; 1],
    pub(crate) pad_0fd4: [u8; 0x2c],
}

#[versions(AGX_SCHEDULED)]
impl ComputeDescriptor::ver {
    pub(crate) const SIZE: usize = core::mem::size_of::<Self>();

    const fn template() -> Self {
        // SAFETY: This type consists exclusively of byte arrays.
        let mut raw: Self = unsafe { core::mem::zeroed() };
        raw.opcode = 3u32.to_le_bytes();
        raw.secondary_count = 0x300004u32.to_le_bytes();
        raw.sentinel = u32::MAX.to_le_bytes();
        raw.support_opcode = 0x1au16.to_le_bytes();
        #[ver(G == G17P)]
        {
            raw.counts = [
                0x22u16.to_le_bytes(),
                0x23u16.to_le_bytes(),
                0x23u16.to_le_bytes(),
                0x24u16.to_le_bytes(),
            ];
            raw.support_kind = 0x9fu16.to_le_bytes();
            raw.support_phase = [1];
        }
        raw
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        // SAFETY: Initialized byte-array fields, alignment one, no padding.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast(), Self::SIZE) }
    }

    pub(crate) fn write(
        out: &mut [u8],
        p: &Parameters,
        registers: &[Register],
    ) -> core::result::Result<(), Error> {
        if ![Self::SIZE, 0x4000].contains(&out.len())
            || registers.len() > 128
            || p.submission_index == 0
            || p.queue_submission > 0xffffff
            || p.sampler_count == u32::MAX
            || (p.sampler_array == 0) != (p.sampler_count == 0)
            || p.sampler_array & 7 != 0
        {
            return Err(Error::Invalid);
        }
        #[ver(G == G17P)]
        if p.queue_submission == 0 {
            return Err(Error::Invalid);
        }
        #[ver(G == G16C)]
        if registers.len() < 2
            || !(1..64).contains(&p.context)
            || p.scheduler == 0
            || p.alias == 0
            || p.alias & 7 != 0
            || p.grid >= 128
            || (p.queue_submission == 0 && p.submission_index & 0xffffff != 0)
        {
            return Err(Error::Invalid);
        }
        // Neo selects the FIRST occurrence for descriptor scalar fields.
        // Max takes scalars from explicit inputs and uses LAST matches only
        // for the secondary register array. Keep both policies explicit.
        #[ver(G == G17P)]
        let one = |number| {
            registers
                .iter()
                .find(|(n, _)| *n == number)
                .map(|(_, v)| *v)
                .ok_or(Error::MissingRegister)
        };
        #[ver(G == G16C)]
        let one = |number| {
            registers
                .iter()
                .rev()
                .find(|(n, _)| *n == number)
                .map(|(_, v)| *v)
                .ok_or(Error::MissingRegister)
        };
        let secondary: [Register; 4] = [
            (0x10099, one(0x0a5c1)?),
            (0x10091, one(0x0a5c9)?),
            (0x0a5c1, one(0x10099)?),
            (0x0a5c9, one(0x10091)?),
        ];
        #[ver(G == G17P)]
        let (save_area, cdm, identity) = (one(0x1a510)?, one(0x1a420)?, one(0x1a540)?);
        #[ver(G == G16C)]
        let (save_area, cdm, identity) = (p.save_area, p.cdm, p.identity);
        #[ver(G == G17P)]
        let control = one(0x1a440)?;
        #[ver(G == G16C)]
        let control = 0x154024201u64;
        if p.terminator < cdm {
            return Err(Error::Invalid);
        }
        #[ver(G == G16C)]
        if cdm == 0 || (cdm | p.terminator) & 3 != 0 {
            return Err(Error::Invalid);
        }
        let primary_address = p.alias.checked_add(0x40).ok_or(Error::Overflow)?;
        let secondary_address = p.alias.checked_add(0x760).ok_or(Error::Overflow)?;

        #[ver(G == G16C)]
        out[..Self::SIZE].copy_from_slice(TEMPLATE_G16C.as_bytes());
        #[ver(G == G17P)]
        out[..Self::SIZE].copy_from_slice(TEMPLATE_G17P.as_bytes());
        out[Self::SIZE..].fill(0);
        // SAFETY: Length is at least SIZE. repr(C) contains only byte arrays,
        // hence alignment one and all bit patterns valid. The exclusive CPU
        // staging borrow lasts only for this function; no firmware aliases.
        let raw = unsafe { &mut *out.as_mut_ptr().cast::<Self>() };
        raw.sequence = p.sequence.to_le_bytes();
        raw.context = p.context.to_le_bytes();
        raw.scheduler = p.scheduler.to_le_bytes();
        #[ver(G == G16C)]
        {
            let n = registers.len() as u16;
            raw.counts = [n - 2, n - 1, n - 1, n].map(u16::to_le_bytes);
            raw.support_kind = p.support_kind.to_le_bytes();
            raw.support_phase = [p.support_phase];
        }
        for (out, &(number, value)) in raw.primary.iter_mut().zip(registers) {
            out[..4].copy_from_slice(&number.to_le_bytes());
            out[4..].copy_from_slice(&value.to_le_bytes());
        }
        for (out, (number, value)) in raw.secondary.iter_mut().zip(secondary) {
            out[..4].copy_from_slice(&number.to_le_bytes());
            out[4..].copy_from_slice(&value.to_le_bytes());
        }
        raw.primary_address = primary_address.to_le_bytes();
        raw.primary_count =
            (((registers.len() as u32 * 12) << 16) | registers.len() as u32).to_le_bytes();
        raw.secondary_address = secondary_address.to_le_bytes();
        raw.save_area = save_area.to_le_bytes();
        raw.terminator = p.terminator.to_le_bytes();
        raw.control = control.to_le_bytes();
        raw.identity_high = ((identity >> 32) as u32).to_le_bytes();
        raw.sampler_array = p.sampler_array.to_le_bytes();
        raw.sampler_count = p.sampler_count.to_le_bytes();
        raw.sampler_limit = (if p.sampler_count == 0 {
            0
        } else {
            p.sampler_count + 1
        })
        .to_le_bytes();
        raw.dispatch = p.dispatch.map(u64::to_le_bytes);
        raw.queue_submission = (p.queue_submission << 8).to_le_bytes();
        raw.grid = p.grid.to_le_bytes();
        raw.queue_ordinal = p.queue_ordinal.to_le_bytes();
        raw.submission_index = p.submission_index.to_le_bytes();
        raw.identity_low = (identity & 0xffffffff).to_le_bytes();
        raw.ordinal = p.ordinal.to_le_bytes();
        raw.status = p.status.map(u64::to_le_bytes);
        raw.timestamps = p.timestamps.map(u64::to_le_bytes);
        raw.support = p.support.to_le_bytes();
        raw.support_control = p.support_control.to_le_bytes();
        raw.support_flags = p.support_flags.to_le_bytes();
        raw.ordinal_overlap
            .copy_from_slice(&((p.ordinal & 3) << 30).to_le_bytes()[..3]);
        raw.zero_page = p.zero_page.to_le_bytes();
        Ok(())
    }
}

#[versions(AGX_SCHEDULED)]
impl Default for ComputeDescriptor::ver {
    fn default() -> Self {
        Self::template()
    }
}
static TEMPLATE_G16C: ComputeDescriptorG16C = ComputeDescriptorG16C::template();
static TEMPLATE_G17P: ComputeDescriptorG17P = ComputeDescriptorG17P::template();

#[versions(AGX_SCHEDULED)]
const COMPUTE_LAYOUT: () = {
    assert!(core::mem::size_of::<ComputeDescriptor::ver>() == 0x1000);
    assert!(core::mem::align_of::<ComputeDescriptor::ver>() == 1);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, opcode) == 0x0);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, sequence) == 0x4);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, context) == 0xc);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, scheduler) == 0x10);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, counts) == 0x18);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0020) == 0x20);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, primary) == 0x40);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0640) == 0x640);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, primary_address) == 0x740);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, primary_count) == 0x748);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_074c) == 0x74c);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, secondary) == 0x760);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0d60) == 0xd60);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, secondary_address) == 0xe60);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, secondary_count) == 0xe68);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0e6c) == 0xe6c);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, save_area) == 0xed8);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, terminator) == 0xee0);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0ee8) == 0xee8);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, control) == 0xf08);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0f10) == 0xf10);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, identity_high) == 0xf20);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0f24) == 0xf24);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, sentinel) == 0xf28);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, sampler_array) == 0xf2c);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, sampler_count) == 0xf34);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, sampler_limit) == 0xf38);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0f3c) == 0xf3c);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, dispatch) == 0xf40);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, queue_submission) == 0xf50);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, grid) == 0xf54);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, queue_ordinal) == 0xf58);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0f5c) == 0xf5c);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, submission_index) == 0xf60);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0f64) == 0xf64);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, identity_low) == 0xf68);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, ordinal) == 0xf70);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0f74) == 0xf74);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, status) == 0xf7c);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, timestamps) == 0xf8c);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0f9c) == 0xf9c);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, support_opcode) == 0xfb0);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, support) == 0xfb2);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, support_control) == 0xfba);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, support_flags) == 0xfbe);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0fc2) == 0xfc2);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, support_kind) == 0xfc5);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0fc7) == 0xfc7);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, ordinal_overlap) == 0xfc8);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, zero_page) == 0xfcb);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, support_phase) == 0xfd3);
    assert!(core::mem::offset_of!(ComputeDescriptor::ver, pad_0fd4) == 0xfd4);
};
