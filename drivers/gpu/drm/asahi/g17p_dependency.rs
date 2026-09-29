// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Unpublished native C/R/C objects from G17PShimBackend. This profile is
//! distinct from retained standalone work: four fresh queues share context
//! one, while dependency points, rather than scheduler IDs, order engines.
//! Storage, live-root joins and the tight publication window belong to the
//! runtime. Nothing here imports firmware state or publishes a producer.

use super::{
    g17p_compute::{self as c, Error, Register, Result},
    g17p_compute_memory as cm, g17p_queue as q,
    g17p_render::{self as r, Kind, Parameters},
    g17p_render_graph as graph,
};

pub(crate) const JOB_LIST: u64 = 0xfffffc2000000000;
pub(crate) const CHANNEL_CONTROL: u64 = 0xfffffc20c07b8000;
pub(crate) const COMPUTE_SUPPORT: u64 = 0xfffffc20c0838000;
pub(crate) const COMPUTE_STATE: u64 = 0xfffffc2001610000;
pub(crate) const OPERAND_TABLE: u64 = 0x7000208000;
pub(crate) const RENDER_SUPPORT: u64 = 0xfffffc20c0840000;
pub(crate) const RENDER_INNER: u64 = 0xfffffc2001620000;
pub(crate) const RENDER_OPERANDS: u64 = 0x70019e8000;
pub(crate) const POOLS: [u64; 2] = [0xfffffc20c0828100, 0xfffffc20c0848080];
pub(crate) const SHARED: [u64; 2] = [0xfffffc20c0878000, 0xfffffc20c084a800];
pub(crate) const RENDER_DESCRIPTORS: [u64; 2] = [0xfffffc20c0018000, 0xfffffc20c00b0000];
pub(crate) const RENDER_OPTIONALS: [u64; 2] = [0xfffffc20c0600180, 0xfffffc20c06000c0];
pub(crate) const RENDER_EVENTS: [u64; 2] = [0xfffffc20c05e8080, 0xfffffc20c05e8040];
pub(crate) const RENDER_STATUS: [u64; 2] = [0xfffffc2001628000, 0xfffffc2001648000];
// Same order as graph::Leaf below. Growth uses the new secondary index and
// shared slot pages; COMPUTE_SUPPORT/STATE are no longer render allocations.
pub(crate) const LEAVES: [u64; 6] = [
    0xfffffc20c0860000,
    0xfffffc20c0850000,
    0xfffffc2001600000,
    0xfffffc2001638000,
    0xfffffc2001630000,
    0xfffffc2001640000,
];
pub(crate) const FRESH_SCHEDULER_PAGES: [u64; 4] = [
    0xfffffc20c0820000,
    0xfffffc20015f8000,
    0xfffffc20c0830000,
    0xfffffc2001608000,
];

#[derive(Clone, Copy)]
pub(crate) struct Layout {
    pub(crate) queue: u64,
    pub(crate) pointers: u64,
    pub(crate) ring: u64,
    pub(crate) grid: u32,
    pub(crate) job_list: u64,
    pub(crate) control: u64,
    pub(crate) context_low: u64,
    pub(crate) context_high: u64,
}
pub(crate) const LAYOUTS: [Layout; 4] = [
    Layout {
        queue: 0xfffffc20c0000000,
        pointers: 0xfffffc2000010000,
        ring: 0xfffffc20c0008000,
        grid: 0,
        job_list: JOB_LIST,
        control: CHANNEL_CONTROL,
        context_low: 0x7000438000,
        context_high: 0xfffffc20001d8000,
    },
    Layout {
        queue: 0xfffffc20c00000c0,
        pointers: 0xfffffc2000012870,
        ring: 0xfffffc20c000a870,
        grid: 1,
        job_list: JOB_LIST + 0x18,
        control: CHANNEL_CONTROL + 0x40,
        context_low: 0x7000460000,
        context_high: 0xfffffc2000200000,
    },
    Layout {
        queue: 0xfffffc20c0000180,
        pointers: 0xfffffc20000150e0,
        ring: 0xfffffc20c000d0e0,
        grid: 2,
        job_list: JOB_LIST + 0x18,
        control: CHANNEL_CONTROL + 0x40,
        context_low: 0x7000488000,
        context_high: 0xfffffc2000228000,
    },
    Layout {
        queue: 0xfffffc20c0000240,
        pointers: 0xfffffc2001658000,
        ring: 0xfffffc20c0880000,
        grid: 3,
        job_list: JOB_LIST + 0x30,
        control: CHANNEL_CONTROL + 0x80,
        context_low: 0x70004b0000,
        context_high: 0xfffffc2000250000,
    },
];
impl Layout {
    pub(crate) fn record(self) -> [u8; 0xc0] {
        q::Record {
            pointers: self.pointers,
            ring: self.ring,
            job_list: self.job_list,
            context: self.control,
            uuid: 0x16,
            priority: 2,
            prio5: 2,
            unk_2c: 2,
            unk_38: 0,
            unk_30: None,
            unk_94: 0,
            sentinel_size: 2,
        }
        .build()
        .unwrap()
    }
}
pub(crate) fn pointers() -> [u8; 0x80] {
    let mut out = [0; 0x80];
    out[..0x60].copy_from_slice(&q::pointers(u32::MAX));
    c::u32_at(&mut out, 0x60, 0x500);
    out
}
pub(crate) fn channel_control() -> [u8; 0x40] {
    let mut out = [0; 0x40];
    for (at, value) in [
        (0, 0x1000000ffff),
        (0x20, 0x2000000000000),
        (0x30, 0xff000000),
    ] {
        c::u64_at(&mut out, at, value);
    }
    out
}

pub(crate) const SCHEDULERS: [(u64, u64, u32); 3] = [
    (0xfffffc20c0820100, 0xfffffc20015f8004, 0),
    (0xfffffc20c0828100, 0xfffffc2001600004, 1),
    (0xfffffc20c0830100, 0xfffffc2001608004, 2),
];
pub(crate) fn scheduler(index: usize) -> Result<[u8; 0x100]> {
    let &(_, slot, work_id) = SCHEDULERS.get(index).ok_or(Error::Invalid)?;
    Ok(c::Scheduler {
        slot,
        work_id,
        phase: 0,
        job_list: 0,
        node_id: 0,
        completion_kind: 0,
    }
    .build())
}
pub(crate) fn compute_support(out: &mut [u8]) -> Result {
    cm::Support {
        compact: None,
        header: 1,
        word_08: 0,
        word_10: 2,
        resource_class: 0x15,
        word_20: Some(0x150000000000),
        word_28: Some(0x150000000000),
        client_state: OPERAND_TABLE,
        firmware_state: COMPUTE_STATE,
        cursor: 0xa8,
        field_54: 0,
        field_5c: 1,
        final_kind: 2,
    }
    .build(out)
}
pub(crate) fn render_support(out: &mut [u8]) -> Result {
    // Runtime writes only the 0x70-byte body after zeroing its full page.
    cm::Support {
        compact: Some((2, 0x70017e0000)),
        header: 2,
        word_08: 1,
        word_10: 0,
        resource_class: 0x16,
        word_20: None,
        word_28: None,
        client_state: RENDER_OPERANDS,
        firmware_state: RENDER_INNER,
        cursor: 0xb0,
        field_54: 0,
        field_5c: 0,
        final_kind: 3,
    }
    .build(out)
}
pub(crate) fn operand_table(out: &mut [u8]) -> Result {
    cm::operand_table_contiguous(out, 0x7000238000, 21)
}
pub(crate) fn leaf(out: &mut [u8], kind: graph::Leaf) -> Result {
    graph::leaf(out, kind, 0, &[(0x12, 6), (0x3c, 2)], 8, 0)
}
pub(crate) fn pool_a(out: &mut [u8]) -> Result {
    graph::record_array_a(out, LEAVES[2] + 4, 1)
}
pub(crate) fn pool_b(out: &mut [u8]) -> Result {
    graph::record_array_b(out, LEAVES[3] + 4, LEAVES[4] + 0x40, 0, 1)
}
pub(crate) fn shared(out: &mut [u8]) -> Result {
    graph::Shared {
        pointers: [LEAVES[0], LEAVES[1], LEAVES[4], LEAVES[5]],
        pair: 0,
        groups: 8,
        work: 1,
    }
    .build(out)
}

/// Both compute commands restart descriptor sequence/ordinal at zero on
/// different physical queues, although retained runtime ordinals are 1/2.
#[derive(Clone, Copy)]
pub(crate) enum Compute {
    Opening,
    Closing,
}
impl Compute {
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Opening => 0,
            Self::Closing => 1,
        }
    }
    pub(crate) fn layout(self) -> Layout {
        LAYOUTS[self.index() * 3]
    }
    pub(crate) fn descriptor(self) -> u64 {
        0xfffffc20c0358000 + self.index() as u64 * 0x1040
    }
    pub(crate) fn low(self) -> u64 {
        0x7000340000 + self.index() as u64 * 0x1040
    }
    pub(crate) fn optional(self) -> u64 {
        [0xfffffc20c0600000, 0xfffffc20c0600240][self.index()]
    }
    pub(crate) fn event(self) -> u64 {
        0xfffffc20c05e8000 + self.index() as u64 * 0xc0
    }
    pub(crate) fn completion(self) -> u64 {
        [0xfffffc2001618000, 0xfffffc2001650000][self.index()]
    }
    pub(crate) fn status(self) -> [u64; 2] {
        let step = self.index() as u64 * 0x20;
        [0xfffffc2000024c68 + step, 0xfffffc2000024c70 + step]
    }
    pub(crate) fn dispatch(self) -> [u64; 2] {
        let step = self.index() as u64 * 0xc;
        [0xfffffc20001c8000 + step, 0xfffffc20c07c0000 + step]
    }
    pub(crate) fn state_alias(self) -> u64 {
        [0x7000220000, 0x70035d8000][self.index()]
    }
    pub(crate) fn robustness_alias(self) -> u64 {
        [0x1000078000, 0x1000238000][self.index()]
    }
    pub(crate) fn program(self, caller_cdm: u64, usc_exec_base: u64) -> Result<[Register; 36]> {
        if usc_exec_base != c::USC_EXEC_BASE {
            return Err(Error::UnsupportedExecBase);
        }
        // The compact dependency form drops helper/USC registers only after
        // validating the same fixed execution-base constraint as normal work.
        let cdm = if self.index() == 0 {
            0x100000b0000
        } else {
            caller_cdm
        };
        let all = c::Program {
            preempt: c::add(caller_cdm, 0x30000)?,
            cdm,
            identity: [0xc6010000e2, 0xc801000132][self.index()],
            context: 1,
            ordinal: self.index() as u32 * 2,
            robustness: self.robustness_alias(),
            operand_state: self.state_alias(),
            usc_exec_base,
            helper_binary: 0,
            helper_data: 0,
            helper_cfg: 0,
            execution_gate: 0,
        }
        .build()?;
        let mut out = [(0, 0); 36];
        let mut index = 0;
        for reg in all {
            if !matches!(reg.0, 0x10071 | 0x11841 | 0x11849 | 0x11f81) {
                out[index] = reg;
                index += 1;
            }
        }
        Ok(out)
    }
    pub(crate) fn descriptor_body(
        self,
        out: &mut [u8],
        registers: &[Register],
        end: u64,
        sampler: u64,
        sampler_count: u32,
        timestamps: [u64; 2],
    ) -> Result {
        c::Descriptor {
            scheduler: SCHEDULERS[self.index() * 2].0,
            low_alias: self.low(),
            cdm_terminator: end,
            sequence: 0,
            context: 1,
            grid: self.layout().grid,
            dispatch: self.dispatch(),
            status: self.status(),
            timestamps,
            shared_control: COMPUTE_SUPPORT,
            zero_page: self.completion(),
            support_control: 0xe0a00001,
            support_flags: 0,
            ordinal: 0,
            queue_submission: 1,
            queue_ordinal: 0,
            submission_index: 1,
            sampler_array: sampler,
            sampler_count,
        }
        .build(out, registers)
    }
    pub(crate) fn optional_body(self) -> [u8; 0xc0] {
        let t = self.layout();
        c::Optional {
            context_low: t.context_low,
            context_high: t.context_high,
            grid: t.grid,
            ordinal: self.index() as u32 * 2,
            shared_control: COMPUTE_SUPPORT,
            channel_control: t.control,
            uuid: 0x16,
            field_46: 0,
            field_1e: 2,
            field_32: 1,
            field_56: self.index() as u32 * 2,
            field_5e: 2,
            first: true,
            item_index: 0,
        }
        .build()
    }
    pub(crate) fn context_body(self, out: &mut [u8], opening_completion: u32) -> Result {
        // Validate even on the closing object: source admission validates the
        // command-buffer completion value before building any owned object.
        c::completion_header(0, opening_completion)?;
        let i = self.index();
        let opening = [(0, 0)];
        let closing = [(2, 1), (3, 0)];
        let step = (self.descriptor() - 0xfffffc20c0358000) / 0x20;
        c::Context {
            descriptor: self.descriptor(),
            queue: self.layout().queue,
            grid: self.layout().grid,
            flags: [0x1000000000000004, 0x10000c0000000004][i],
            word_220: [0xffff080000000001, 0xffff080200000001][i],
            word_330: 0,
            word_338: 2,
            word_350: 0x110038001a002 + step,
            word_358: 0x20038001a03b + step,
            word_378: 0x3fffffffffffff,
            item_index: 0,
            points: Some(if i == 0 { &opening } else { &closing }),
            event_slot: Some(i as u8 * 2),
            completion: Some(if i == 0 { opening_completion } else { 1 }),
        }
        .build(out)
    }
    pub(crate) fn event_body(self) -> [u8; 0x40] {
        q::event(
            1,
            self.layout().grid,
            q::Kind::Compute,
            None,
            Some(0x102),
            2,
        )
        .unwrap()
    }
}

pub(crate) fn render_optional(out: &mut [u8], kind: Kind) -> Result {
    let t = LAYOUTS[kind.index() as usize + 1];
    graph::Optional {
        kind,
        context_scratch: t.context_low,
        firmware_scratch: t.context_high,
        shared_control: RENDER_SUPPORT,
        channel_control: t.control,
        tiling_shared: if kind == Kind::Tiling {
            Some(SHARED[0])
        } else {
            None
        },
        grid: t.grid as u16,
        item: 0,
        ordinal: 1,
        context: Some(1),
        uuid: Some(0x16),
        scheduler_class: Some(2),
        context_index: None,
        context_phase: None,
        first: None,
        lifecycle: if kind == Kind::Tiling { Some(0) } else { None },
        namespace: None,
        overrides: &[(0x46, 1), (0x56, 1), (0x5e, 2)],
    }
    .build(out)
}
pub(crate) fn render_context(out: &mut [u8], kind: Kind, opening_completion: u32) -> Result {
    c::completion_header(0, opening_completion)?;
    let i = kind.index() as usize;
    let tiling = [(0, opening_completion), (1, 0)];
    let fragment = [(1, 1), (2, 0)];
    graph::Context {
        kind,
        descriptor: RENDER_DESCRIPTORS[i],
        queue: LAYOUTS[i + 1].queue,
        pair: 0,
        item: 0,
        context: Some(1),
        grid: Some(LAYOUTS[i + 1].grid),
        locator_context: None,
        partial_opening: false,
        dependency_grid: Some(LAYOUTS[i + 1].grid),
        points: Some(if i == 0 { &tiling } else { &fragment }),
        event_slot: None,
        completion: None,
    }
    .build(out)
}
pub(crate) fn render_registers(kind: Kind, registers: &mut [Register]) {
    for (number, value) in registers {
        let replacement = match (kind, *number) {
            (Kind::Tiling, 0x0a5a1) => Some(0xa900400020),
            (Kind::Tiling, 0x1ca30 | 0x16c39) | (Kind::Fragment, 0x1ca28) => Some(0x180020),
            (Kind::Tiling, 0x1c910) => Some(0x88005),
            (Kind::Tiling, 0x1ca10 | 0x014a1 | 0x0a349) => Some(0xc701000114),
            (Kind::Tiling, 0x10209 | 0x1c9f0 | 0x14320) | (Kind::Fragment, 0x10211 | 0x10420) => {
                Some(0x101)
            }
            (Kind::Tiling, 0x14308) | (Kind::Fragment, 0x14048) => Some(1),
            (Kind::Tiling, 0x14318) => Some(0x1000080001),
            (Kind::Fragment, 0x0a5a9) => Some(0xd900400020),
            (Kind::Fragment, 0x160e0 | 0x01499 | 0x0a341) => Some(0xc701000113),
            (Kind::Fragment, 0x14080) => Some(0x10001a8001),
            _ => None,
        };
        if let Some(v) = replacement {
            *value = v;
        }
    }
}

pub(crate) fn render_parameters(p: &Parameters) -> Parameters {
    Parameters {
        lifecycle_ordinal: 1,
        queue_pair: 0,
        queue_item_index: 0,
        timestamp_a: 0xfffffc2000024c78,
        timestamp_b: 0xfffffc2000024c80,
        fragment_timestamp_start: 0xfffffc2000024c78,
        fragment_timestamp_end: 0xfffffc2000024c80,
        ..*p
    }
}
pub(crate) fn render_descriptor(out: &mut [u8], kind: Kind, p: &Parameters) -> Result {
    let p = render_parameters(p);
    let mut ta;
    let mut frag;
    let registers = if kind == Kind::Tiling {
        ta = r::tiling_registers(&p)?;
        render_registers(kind, &mut ta);
        &ta[..]
    } else {
        frag = r::fragment_registers(&p)?;
        render_registers(kind, &mut frag);
        &frag[..]
    };
    let dispatch = if kind == Kind::Tiling { 4 } else { 8 };
    let pointers = [
        (
            if kind == Kind::Tiling { 0x8a6 } else { 0x2140 },
            0xfffffc20001c8000 + dispatch,
        ),
        (
            if kind == Kind::Tiling { 0x8ae } else { 0x2148 },
            0xfffffc20c07c0000 + dispatch,
        ),
        (
            if kind == Kind::Tiling { 0x934 } else { 0x21ce },
            RENDER_SUPPORT,
        ),
    ];
    r::Descriptor {
        kind,
        index: 0,
        sequence: u64::from(kind == Kind::Tiling),
        ordinal: 1,
        context: 1,
        queue_pair: 0,
        pool_bases: POOLS,
        record_indices: [0, 0],
        shared: SHARED,
        low_alias: Some(if kind == Kind::Tiling {
            0x7000000000
        } else {
            0x7000098000
        }),
        status_base: Some(RENDER_STATUS[kind.index() as usize]),
        grid: Some(kind.index() + 1),
        write_tail: true,
        write_lifecycle: true,
        write_item: true,
        write_structural: true,
        pointer_overrides: &pointers,
        item_overrides: if kind == Kind::Fragment {
            &[(0x215c, 0)]
        } else {
            &[]
        },
    }
    .build(out, registers, Some(&p))
}

pub(crate) fn render_registration(sequence: u32) -> [u8; 0x40] {
    let mut out = [0; 0x40];
    for (at, value) in [
        (0, 0x20),
        (4, 1),
        (8, 0x3f),
        (0xc, sequence),
        (0x2c, 0x28),
        (0x30, 1),
        (0x34, 1),
    ] {
        c::u32_at(&mut out, at, value);
    }
    for (at, value) in [
        (0x14, RENDER_SUPPORT),
        (0x1c, RENDER_OPERANDS),
        (0x24, RENDER_OPERANDS + 0x580),
    ] {
        c::u64_at(&mut out, at, value);
    }
    out
}
pub(crate) fn render_receipt(sequence: u32) -> [u8; 0x28] {
    let mut out = [0; 0x28];
    for (at, value) in [(0, 13), (4, 1), (8, sequence), (0x24, 0x28)] {
        c::u32_at(&mut out, at, value);
    }
    for (at, value) in [
        (0xc, RENDER_SUPPORT),
        (0x14, RENDER_OPERANDS),
        (0x1c, RENDER_OPERANDS + 0x580),
    ] {
        c::u64_at(&mut out, at, value);
    }
    out
}

/// One owned control transaction. A match identifies registration only, not
/// GPU completion. The source accepts the 0x28-byte prefix once on primary;
/// trailing diagnostic fields remain opaque and a duplicate stays unknown.
pub(crate) struct Receipt {
    prefix: [u8; 0x28],
    pending: bool,
}
impl Receipt {
    pub(crate) fn new(sequence: u32) -> Self {
        Self {
            prefix: render_receipt(sequence),
            pending: true,
        }
    }
    pub(crate) fn pending(&self) -> bool {
        self.pending
    }
    pub(crate) fn consume(&mut self, peer: usize, body: &[u8]) -> bool {
        if !self.pending || peer != 0 || body.len() != 0x48 || body[..0x28] != self.prefix {
            return false;
        }
        self.pending = false;
        true
    }
}

pub(crate) fn tick(counter: u32) -> Result<[u8; 0x40]> {
    if counter > 2 {
        return Err(Error::Invalid);
    }
    let mut out = [0; 0x40];
    c::u32_at(&mut out, 0, 0x2e);
    c::u32_at(&mut out, 4, counter);
    c::u32_at(&mut out, 0xc, u32::from(counter == 1));
    Ok(out)
}
pub(crate) fn engine_owner(engine: u32) -> Result<[u8; 0x40]> {
    if engine > 2 {
        return Err(Error::Invalid);
    }
    let mut out = [0; 0x40];
    c::u32_at(&mut out, 0, 0x14);
    c::u32_at(&mut out, 8, 0x04000002 | engine << 16);
    c::u64_at(&mut out, 0xc, CHANNEL_CONTROL + engine as u64 * 0x40);
    Ok(out)
}

/// Host-owned state advanced after the class announcement and before render
/// producers. These are the writes in release_dependency_window, including
/// its partial fourth index group. Reads use the live owned leaves, not seeds.
pub(crate) trait ReleaseWriter {
    type Error;
    fn read32(&mut self, address: u64) -> core::result::Result<u32, Self::Error>;
    fn write32(&mut self, address: u64, value: u32) -> core::result::Result<(), Self::Error>;
}
pub(crate) fn render_transition<W: ReleaseWriter>(
    writer: &mut W,
) -> core::result::Result<(), W::Error> {
    writer.write32(RENDER_SUPPORT + 0x48, 0xd8)?;
    writer.write32(RENDER_INNER, 2)?;
    for offset in (0..0x20).step_by(4) {
        let at = LEAVES[0] + 0x60 + offset;
        let value = writer.read32(at)?;
        writer.write32(at, value.wrapping_add(1))?;
    }
    for offset in [0, 8] {
        let at = LEAVES[1] + 0x30 + offset;
        let value = writer.read32(at)?;
        writer.write32(at, value.wrapping_add(1))?;
    }
    Ok(())
}
