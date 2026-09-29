// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Retained pair-zero append lifecycle from G17PShimBackend's source path.
//! Each later generation owns a sequenced control tick, new records and
//! an independently retired tilemap block. Ring recycling is still bounded.

use super::{
    g17p_compute::{self as c, Error, Result},
    g17p_opening as opening,
    g17p_render::{self as r, Kind, Parameters},
    g17p_render_graph as graph,
};
pub(crate) const SUBMISSIONS: u32 = 32;
pub(crate) const DESCRIPTORS: [u64; 2] = [0xfffffc20c0018000, 0xfffffc20c00b0000];
pub(crate) const QUEUES: [u64; 2] = [0xfffffc20c0000000, 0xfffffc20c00000c0];
pub(crate) const POINTERS: [u64; 2] = [0xfffffc2000010000, 0xfffffc2000012870];
pub(crate) const RINGS: [u64; 2] = [0xfffffc20c0008000, 0xfffffc20c000a870];
pub(crate) const OPTIONAL: [u64; 2] = [0xfffffc20c06000c0, 0xfffffc20c0600000];
pub(crate) const EVENTS: [u64; 2] = [0xfffffc20c05e8040, 0xfffffc20c05e8000];
pub(crate) const POOLS: [u64; 2] = [0xfffffc20c0820100, 0xfffffc20c0830080];
pub(crate) const SHARED: [u64; 2] = [0xfffffc20c0860000, 0xfffffc20c0832800];
pub(crate) const STATUS: [u64; 2] = [0xfffffc2001608000, 0xfffffc2001628000];
pub(crate) const LEAVES: [u64; 6] = [
    0xfffffc20c0848000,
    0xfffffc20c0838000,
    0xfffffc20015f8000,
    0xfffffc2001618000,
    0xfffffc2001610000,
    0xfffffc2001620000,
];
pub(crate) const JOB_LIST: u64 = 0xfffffc2000000000;
pub(crate) const FW_TIMESTAMPS: [u64; 2] = [0xfffffc2000024c68, 0xfffffc2000024c70];

#[derive(Clone, Copy)]
pub(crate) struct Item {
    pub(crate) ordinal: u32,
}
impl Item {
    pub(crate) fn new(ordinal: u32) -> Result<Self> {
        if ordinal == 0 || ordinal >= SUBMISSIONS {
            return Err(Error::Invalid);
        }
        Ok(Self { ordinal })
    }
    pub(crate) fn descriptor_address(self, kind: Kind) -> u64 {
        DESCRIPTORS[kind.index() as usize] + self.ordinal as u64 * kind.size() as u64
    }
    pub(crate) fn optional_address(self, kind: Kind) -> u64 {
        OPTIONAL[kind.index() as usize] + self.ordinal as u64 * 0x180
    }
    pub(crate) fn event_address(self, kind: Kind) -> u64 {
        EVENTS[kind.index() as usize] + self.ordinal as u64 * 0x80
    }
    pub(crate) fn status(self, kind: Kind) -> u64 {
        STATUS[kind.index() as usize] + self.ordinal as u64 * 0x40
    }
    pub(crate) fn records(self) -> [u32; 2] {
        [(2 * self.ordinal) % 35, self.ordinal % 79]
    }
    pub(crate) fn slot(self) -> u64 {
        LEAVES[2] + 4 + self.records()[0] as u64 * 4
    }
    pub(crate) fn parameters(self, p: &Parameters) -> Parameters {
        Parameters {
            lifecycle_ordinal: self.ordinal as u64,
            queue_pair: 0,
            queue_item_index: self.ordinal as u64,
            local_item_registers: true,
            native_status_registers: true,
            tvb_pool_id: Some(0),
            ..*p
        }
    }
    pub(crate) fn descriptor(self, kind: Kind, p: &Parameters, out: &mut [u8]) -> Result {
        let p = self.parameters(p);
        let ta;
        let frag;
        let registers = if kind == Kind::Tiling {
            ta = r::tiling_registers(&p)?;
            &ta[..]
        } else {
            frag = r::fragment_registers(&p)?;
            &frag[..]
        };
        r::Descriptor {
            kind,
            index: self.ordinal,
            sequence: 2 * self.ordinal as u64 + u64::from(kind == Kind::Tiling),
            ordinal: self.ordinal,
            context: 1,
            queue_pair: 0,
            pool_bases: POOLS,
            record_indices: self.records(),
            shared: SHARED,
            low_alias: None,
            status_base: Some(STATUS[kind.index() as usize]),
            grid: None,
            write_tail: true,
            write_lifecycle: true,
            write_item: true,
            write_structural: true,
            pointer_overrides: &[(
                if kind == Kind::Tiling { 0x934 } else { 0x21ce },
                opening::SUPPORT,
            )],
            item_overrides: &[],
        }
        .build(out, registers, Some(&p))?;
        // _advance_tilemap_block: descriptor offsets, including its full DVA.
        // The retained pool and TPC are not reinitialized on an append.
        let step = (self.ordinal % 8) as u64 * 0x1200;
        let offsets: &[usize] = if kind == Kind::Tiling {
            &[0x7c, 0x88, 0xa0, 0xac, 0x148, 0x154]
        } else {
            &[0x17c, 0x314, 0x320]
        };
        for &at in offsets {
            let v = u32::from_le_bytes(out[at..at + 4].try_into().unwrap());
            c::u32_at(out, at, v.wrapping_add(step as u32));
        }
        if kind == Kind::Fragment {
            let v = u64::from_le_bytes(out[0x40..0x48].try_into().unwrap());
            c::u64_at(out, 0x40, v + step);
        }
        Ok(())
    }
    pub(crate) fn optional(self, kind: Kind, out: &mut [u8]) -> Result {
        let i = kind.index() as usize;
        graph::Optional {
            kind,
            context_scratch: opening::CONTEXTS[i].1,
            firmware_scratch: opening::CONTEXTS[i].0,
            shared_control: opening::SUPPORT,
            channel_control: opening::CHANNEL_CONTROL,
            tiling_shared: if i == 0 { Some(SHARED[0]) } else { None },
            grid: i as u16,
            item: self.ordinal as u16,
            ordinal: self.ordinal as u16,
            context: Some(1),
            uuid: Some(0x15),
            scheduler_class: None,
            context_index: None,
            context_phase: None,
            first: None,
            lifecycle: None,
            namespace: None,
            overrides: &[],
        }
        .build(out)
    }
    pub(crate) fn context(self, kind: Kind, out: &mut [u8]) -> Result {
        let (points, count) = graph::paired_dependencies(kind, 0, self.ordinal)?;
        graph::Context {
            kind,
            descriptor: self.descriptor_address(kind),
            queue: QUEUES[kind.index() as usize],
            pair: 0,
            item: self.ordinal,
            context: Some(1),
            grid: Some(kind.index()),
            locator_context: Some(1),
            partial_opening: false,
            dependency_grid: None,
            points: Some(&points[..count]),
            event_slot: Some(0),
            completion: None,
        }
        .build(out)
    }
}

/// announce_runtime_tick(): context zero, no fields inferred from prior work.
pub(crate) fn control_tick(ordinal: u32) -> Result<[u8; 0x40]> {
    if ordinal < 2 || ordinal >= SUBMISSIONS {
        return Err(Error::Invalid);
    }
    let mut body = [0; 0x40];
    c::u32_at(&mut body, 0, 0x2e);
    c::u32_at(&mut body, 4, ordinal - 1);
    Ok(body)
}
/// G17P_RUNTIME_NATIVE_SHARED_PRESTATE, before ticks for ordinal >= 3.
pub(crate) fn control_prestate() -> [u8; 16] {
    let mut body = [0; 16];
    for (i, value) in [0x1a0, 0x1ea0, 0, 0x1d00].into_iter().enumerate() {
        c::u32_at(&mut body, i * 4, value);
    }
    body
}

/// _publish_partial_index_owner leaves firmware's grown registration intact.
/// Only the original eight-group partial layout has a host refresh rule.
pub(crate) fn index_registration(low: u64, count: u32) -> Result<Option<[u8; 8]>> {
    if count != 32 {
        return Ok(None);
    }
    if !(0x1000000000..0x2000000000).contains(&low) || low & 15 != 0 {
        return Err(Error::Invalid);
    }
    let mut body = [0; 8];
    c::u32_at(&mut body, 0, ((low - 0x1000000000) >> 4) as u32);
    c::u32_at(&mut body, 4, count);
    Ok(Some(body))
}
