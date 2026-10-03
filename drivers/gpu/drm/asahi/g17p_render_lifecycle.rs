// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Retained pair-zero append lifecycle from G17PShimBackend's source path.
//! Each later generation owns a sequenced control tick, new records and
//! an independently retired tilemap block. Physical storage is finite and reused after retirement.

use super::{
    g17p_compute::{self as c, Error, Result},
    g17p_opening as opening,
    g17p_render::{self as r, Kind, Parameters},
    g17p_render_graph as graph,
};
// Retired physical storage wraps independently of logical queue identities.
// The optional item's ordinal is u16; native dependency waves retain their
// previously qualified bounded storage profile.
pub(crate) const STORAGE_SUBMISSIONS: u32 = 256;
pub(crate) const NATIVE_SUBMISSIONS: u32 = 255;
// The terminal report is a u32 grid bitmap. Compute reserves grids 4/5;
// each render owner consumes an adjacent TA/FR pair from the remaining bits.
pub(crate) const POOL_SLOTS: u32 = (u32::BITS - 2) / 2;
pub(crate) fn pool_grids(pool: u32) -> Result<[u32; 2]> {
    if pool >= POOL_SLOTS { return Err(Error::Invalid); }
    let ta = 2 * pool + if pool >= 2 { 2 } else { 0 };
    Ok([ta, ta + 1])
}
pub(crate) fn private_delta(pool: u32) -> u64 {
    if pool < 2 { 0 } else { 0x80000000 + (pool as u64 - 2) * 0x2000000 }
}
pub(crate) fn extra_pool(pool: u32) -> Result<Layout> {
    if pool < 2 { return Err(Error::Invalid); }
    let grids = pool_grids(pool)?;
    let base = 0xfffffc20d0000000 + (pool as u64 - 2) * 0x200000;
    let low = 0x7100000000 + (pool as u64 - 2) * 0x100000;
    Ok(Layout {
        queues: [base, base + 0xc0],
        pointers: [base + 0x8000, base + 0x10000],
        rings: [base + 0x18000, base + 0x20000],
        status: [base + 0x28000, base + 0x30000],
        job_list: base + 0x38000,
        pools: [base + 0x40100, base + 0x48080],
        shared: [base + 0x88000, base + 0x4a800],
        leaves: [base + 0x50000, base + 0x60000, base + 0x68000,
                 base + 0x70000, base + 0x78000, base + 0x80000],
        contexts: [(base + 0x90000, low), (base + 0xb0000, low + 0x20000)],
        grids, pair: pool, ..SECOND
    })
}

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

/// Descriptor arrays retain their Source addresses and partially overlap:
/// TA slots 249..255 intersect FR slots 0..1. Ownership follows physical
/// intervals, not matching slot numbers. Equal slots also own the optional,
/// event and timestamp records. The low aliases have the same relative layout.
pub(crate) fn storage_conflicts(left: u32, right: u32) -> bool {
    if left == right { return true; }
    for a in [Kind::Tiling, Kind::Fragment] {
        let start = DESCRIPTORS[a.index() as usize] + left as u64 * a.size() as u64;
        for b in [Kind::Tiling, Kind::Fragment] {
            let other = DESCRIPTORS[b.index() as usize] + right as u64 * b.size() as u64;
            if start < other + b.size() as u64 && other < start + a.size() as u64 {
                return true;
            }
        }
    }
    false
}

/// Source scheduler_publication_values for the synchronous ordinary pair.
pub(crate) fn scheduler_phases(current: u32, reused: bool) -> Result<[u32; 3]> {
    let base = if reused { current } else { 0 };
    Ok([base, base.checked_add(1).ok_or(Error::Overflow)?,
        base.checked_add(2).ok_or(Error::Overflow)?])
}

/// Source scheduler_descriptor_stamp: retain the context marker while the
/// hardware tag wraps independently of the full scheduler ordinal.
pub(crate) fn scheduler_stamp(stamp: u32, previous_node: u32, node: u32) -> Result<u32> {
    stamp.checked_sub(previous_node & 0xff).and_then(|base| base.checked_add(node & 0xff))
        .ok_or(Error::Overflow)
}

/// Retained muxed_queue_pair, pointer_sets and paired_builder state from
/// G17PShimBackend. Global allocation/generation and queue-local items differ
/// after the native C/R/C owner, exactly as they do in the Python dictionaries.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) queues: [u64; 2],
    pub(crate) pointers: [u64; 2],
    pub(crate) rings: [u64; 2],
    pub(crate) pools: [u64; 2],
    pub(crate) shared: [u64; 2],
    pub(crate) status: [u64; 2],
    pub(crate) leaves: [u64; 6],
    pub(crate) contexts: [(u64, u64); 2],
    pub(crate) grids: [u32; 2],
    pub(crate) job_list: u64,
    pub(crate) support: u64,
    pub(crate) inner: u64,
    pub(crate) control: u64,
    pub(crate) uuid: u16,
    pub(crate) native: bool,
    pub(crate) pair: u32,
    pub(crate) independent: bool,
    pub(crate) context: u32,
}
pub(crate) const ORDINARY: Layout = Layout {
    queues: QUEUES,
    pointers: POINTERS,
    rings: RINGS,
    pools: POOLS,
    shared: SHARED,
    status: STATUS,
    leaves: LEAVES,
    contexts: opening::CONTEXTS,
    grids: [0, 1],
    job_list: JOB_LIST,
    support: opening::SUPPORT,
    inner: opening::STATE,
    control: opening::CHANNEL_CONTROL,
    uuid: 0x15,
    native: false,
    pair: 0,
    independent: false,
    context: 1,
};
pub(crate) const NATIVE: Layout = {
    use super::g17p_dependency as d;
    Layout {
        queues: [d::LAYOUTS[1].queue, d::LAYOUTS[2].queue],
        pointers: [d::LAYOUTS[1].pointers, d::LAYOUTS[2].pointers],
        rings: [d::LAYOUTS[1].ring, d::LAYOUTS[2].ring],
        pools: d::POOLS,
        shared: d::SHARED,
        status: d::RENDER_STATUS,
        leaves: d::LEAVES,
        contexts: [
            (d::LAYOUTS[1].context_high, d::LAYOUTS[1].context_low),
            (d::LAYOUTS[2].context_high, d::LAYOUTS[2].context_low),
        ],
        grids: [1, 2],
        job_list: d::LAYOUTS[1].job_list,
        support: d::RENDER_SUPPORT,
        inner: d::RENDER_INNER,
        control: d::LAYOUTS[1].control,
        uuid: 0x16,
        native: true,
        pair: 0,
        independent: false,
        context: 1,
    }
};

/// Explicit owned SECOND firmware-DATA namespaces. The legacy template
/// overlaps cold/post-render compute support, scheduler slots and job lists.
/// Allocate each absent 32KiB span before any SECOND producer; no prior page
/// is overwritten, adopted or returned. Low caller GPU resource VAs stay fixed.
pub(crate) const SECOND_OWNED_STORAGE: [u64;5] = [
    0xfffffc20cf060000, // Pool-B array + its shared object.
    0xfffffc20cf068000, // TA status slots.
    0xfffffc20cf070000, // Shared submission leaf.
    0xfffffc20cf078000, // Queue job list.
    0xfffffc20cf080000, // Initial FR item ring, distinct from compute c08aa870.
];
/// Match each old Source object's AttrIndex. PoolB/ring are firmware-only
/// Normal; TA status/shared counts/joblist were outside c0000000 and Shared.
pub(crate) const SECOND_OWNED_ATTRIBUTES: [u64;5] = [
    0x00c0000000000443,
    0x00c000000000044b,
    0x00c000000000044b,
    0x00c000000000044b,
    0x00c0000000000443,
];
pub(crate) const SECOND: Layout = Layout {
    queues: [0xfffffc20c0000180, 0xfffffc20c0000240],
    pointers: [0xfffffc20000150e0, 0xfffffc2001658000],
    rings: [0xfffffc20c000d0e0, SECOND_OWNED_STORAGE[4]],
    pools: [0xfffffc20c0900100, SECOND_OWNED_STORAGE[0] + 0x80],
    shared: [0xfffffc20c08a0000, SECOND_OWNED_STORAGE[0] + 0x2800],
    status: [SECOND_OWNED_STORAGE[1], 0xfffffc2001650000],
    leaves: [
        0xfffffc20c0888000,
        0xfffffc20c0878000,
        0xfffffc2001700000,
        0xfffffc2001708000,
        SECOND_OWNED_STORAGE[2],
        0xfffffc2001648000,
    ],
    contexts: [
        (0xfffffc2000228000, 0x7000488000),
        (0xfffffc2000250000, 0x70004b0000),
    ],
    grids: [2, 3],
    job_list: SECOND_OWNED_STORAGE[3],
    support: opening::SUPPORT,
    inner: opening::STATE,
    control: opening::CHANNEL_CONTROL,
    uuid: 0x15,
    native: false,
    pair: 1,
    independent: true,
    context: 1,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) ordinal: u32,
    pub(crate) storage: u32,
    pub(crate) index: u32,
    pub(crate) layout: Layout,
}
impl Item {
    pub(crate) fn new(ordinal: u32) -> Result<Self> {
        if ordinal == 0 {
            return Err(Error::Invalid);
        }
        Ok(Self {
            ordinal,
            storage: ordinal % STORAGE_SUBMISSIONS,
            index: ordinal,
            layout: ORDINARY,
        })
    }
    pub(crate) fn retained(ordinal: u32, index: u32, layout: Layout) -> Result<Self> {
        if ordinal == 0
            || (layout.native && (ordinal >= NATIVE_SUBMISSIONS || index >= NATIVE_SUBMISSIONS)) {
            return Err(Error::Invalid);
        }
        Ok(Self {
            ordinal,
            storage: ordinal % STORAGE_SUBMISSIONS,
            index,
            layout,
        })
    }
    pub(crate) fn descriptor_address(self, kind: Kind) -> u64 {
        DESCRIPTORS[kind.index() as usize] + self.storage_ordinal() as u64 * kind.size() as u64
    }
    pub(crate) fn storage_ordinal(self) -> u32 { self.storage }
    pub(crate) fn descriptor_alias(self, kind: Kind) -> u64 {
        [0x7000000000, 0x7000098000][kind.index() as usize]
            + self.storage_ordinal() as u64 * kind.size() as u64
    }
    pub(crate) fn optional_address(self, kind: Kind) -> u64 {
        OPTIONAL[kind.index() as usize] + self.storage_ordinal() as u64 * 0x180
    }
    pub(crate) fn event_address(self, kind: Kind) -> u64 {
        EVENTS[kind.index() as usize] + self.storage_ordinal() as u64 * 0x80
    }
    pub(crate) fn status(self, kind: Kind) -> u64 {
        self.layout.status[kind.index() as usize] + (self.index % STORAGE_SUBMISSIONS) as u64 * 0x40
    }
    pub(crate) fn context_address(self, kind: Kind) -> u64 {
        self.layout.contexts[kind.index() as usize].0
            + ((self.index + 1) % STORAGE_SUBMISSIONS) as u64 * 0x200
    }
    pub(crate) fn records(self) -> [u32; 2] {
        [(2 * self.index) % 35, self.index % 79]
    }
    // G17P_DEFAULTS enables KEEP_BASE_DESCRIPTOR_MIRRORS. Pair zero keeps
    // its constructor values; a created pair does so only for its first item.
    pub(crate) fn refresh_pool_b_mirrors(self) -> bool {
        self.layout.pair != 0 && self.index != 0
    }
    pub(crate) fn pool_b_mirrors(self, kind: Kind, out: &mut [u8], b00: u32, b28: u32) {
        if !self.refresh_pool_b_mirrors() {
            return;
        }
        if kind == Kind::Tiling {
            for at in [0x310, 0x31c] {
                c::u32_at(out, at, b28);
            }
            c::u32_at(out, 0x328, b00 | 1);
        } else {
            c::u32_at(out, 0x464, b28);
        }
    }
    pub(crate) fn retirement_leaf(self) -> Option<u64> {
        (self.layout.pair != 0 && self.index == 0).then_some(self.layout.leaves[4] + 0x40)
    }
    pub(crate) fn tilemap_reset(self, p: &Parameters) -> Option<u64> {
        (self.index >= 8).then_some(
            p.tilemap + (self.index % 8) as u64 * p.scratch_layout().unwrap().1
                + if self.layout.independent { self.layout.pair as u64 * p.scratch_pair_stride } else { 0 },
        )
    }
    pub(crate) fn scheduler_node(self, kind: Kind, out: &mut [u8]) -> Result {
        let node = self.ordinal + self.ordinal / 2;
        let offsets: &[usize] = if kind == Kind::Tiling {
            c::u32_at(out, 0x48, node);
            &[0x370, 0x37c, 0x388]
        } else {
            &[0x470, 0x47c]
        };
        for &at in offsets {
            let value = u32::from_le_bytes(out[at..at + 4].try_into().unwrap());
            c::u32_at(out, at, scheduler_stamp(value, node, node)?);
        }
        Ok(())
    }
    pub(crate) fn slot(self) -> u64 {
        self.layout.leaves[2] + 4 + self.records()[0] as u64 * 4
    }
    pub(crate) fn parameters(self, p: &Parameters) -> Parameters {
        let p = Parameters {
            lifecycle_ordinal: self.ordinal as u64,
            // Ordinary independent compute can start during any retained
            // render, so its caller preselects the completion1/CS-gate0 pair.
            // Opening and native profiles retain their qualified values;
            // ownership and per-pool lifetime rules are unchanged.
            completion_control: u64::from(self.ordinal > 0
                && (!self.layout.independent || !self.layout.native)
                && p.completion_control != 0),
            queue_pair: self.layout.pair as u64,
            queue_item_index: self.index as u64,
            local_item_registers: true,
            native_status_registers: true,
            tvb_pool_id: Some(self.layout.pair as u64),
            native_context_slot: Some(self.layout.context as u64),
            ..*p
        };
        if self.layout.independent {
            let p = Parameters {
                pair_resource_stride: 0x1b0000,
                native_cycle_registers: true,
                native_record_index_register: true,
                native_status_registers: false,
                native_item_fields: false,
                ..p
            };
            if self.layout.pair == 0 {
                return p;
            }
            let delta = private_delta(self.layout.pair);
            Parameters {
                cycle_base: Some(0x328000 + delta),
                record_index_base: Some(0x80000 + delta),
                record_index_offset: Some(0x140),
                ta_status: 0x1000230000 + delta,
                fragment_status: 0x1000358000 + delta,
                deflake_1: 0x10010002a0 + delta,
                deflake_2: 0x1001000020 + delta,
                deflake_3: 0x1001000000 + delta,
                aux_fb: 0x1001004000 + delta,
                ..p
            }
        } else {
            p
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
        let pointers = [
            (
                if kind == Kind::Tiling { 0x934 } else { 0x21ce },
                self.layout.support,
            ),
            (if kind == Kind::Tiling { 0x945 } else { 0x21df }, self.status(kind)),
            (
                if kind == Kind::Tiling { 0x8a6 } else { 0x2140 },
                0xfffffc20001c8000 + if kind == Kind::Tiling { 4 } else { 8 },
            ),
            (
                if kind == Kind::Tiling { 0x8ae } else { 0x2148 },
                0xfffffc20c07c0000 + if kind == Kind::Tiling { 4 } else { 8 },
            ),
        ];
        r::Descriptor {
            kind,
            index: self.index,
            sequence: 2 * self.index as u64 + u64::from(kind == Kind::Tiling),
            ordinal: self.ordinal,
            context: self.layout.context,
            queue_pair: self.layout.pair,
            pool_bases: self.layout.pools,
            record_indices: self.records(),
            shared: self.layout.shared,
            low_alias: Some(self.descriptor_alias(kind)),
            status_base: Some(self.layout.status[kind.index() as usize]),
            grid: Some(self.layout.grids[kind.index() as usize]),
            write_tail: true,
            write_lifecycle: true,
            write_item: true,
            write_structural: true,
            pointer_overrides: &pointers[..if self.layout.native { 4 } else { 2 }],
            item_overrides: if self.layout.native && kind == Kind::Fragment {
                &[(0x215c, 0)]
            } else {
                &[]
            },
        }
        .build(out, registers, Some(&p))?;
        // _advance_tilemap_block: descriptor offsets, including its full DVA.
        // The retained pool and TPC are not reinitialized on an append.
        let pair_step = if self.layout.independent {
            self.layout.pair as u64 * p.scratch_pair_stride
        } else {
            0
        };
        let step = (self.index % 8) as u64 * p.scratch_layout()?.1 + pair_step;
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
        } else if pair_step != 0 {
            for at in [0x94, 0x780] {
                let v = u32::from_le_bytes(out[at..at + 4].try_into().unwrap());
                c::u32_at(out, at, v.wrapping_add(pair_step as u32));
            }
        }
        Ok(())
    }
    pub(crate) fn optional(self, kind: Kind, out: &mut [u8]) -> Result {
        let i = kind.index() as usize;
        graph::Optional {
            kind,
            context_scratch: self.layout.contexts[i].1,
            firmware_scratch: self.layout.contexts[i].0,
            shared_control: self.layout.support,
            channel_control: self.layout.control,
            tiling_shared: if i == 0 {
                Some(self.layout.shared[0])
            } else {
                None
            },
            grid: self.layout.grids[i] as u16,
            item: self.index as u16,
            ordinal: self.ordinal as u16,
            context: Some(self.layout.context as u16),
            uuid: Some(self.layout.uuid),
            scheduler_class: if self.layout.native { Some(2) } else { None },
            context_index: None,
            context_phase: None,
            first: Some(self.index == 0),
            lifecycle: if self.layout.native && i == 0 {
                Some(0)
            } else {
                None
            },
            namespace: None,
            overrides: if self.layout.native {
                &[(0x46, 1), (0x56, 1), (0x5e, 2)]
            } else {
                &[]
            },
        }
        .build(out)
    }
    pub(crate) fn context(self, kind: Kind, p: &r::Parameters, out: &mut [u8]) -> Result {
        let (internal, count) = graph::paired_grid_dependencies(kind, self.layout.grids, self.index)?;
        let mut points = [(0u8, 0u32); 4];
        points[..count].copy_from_slice(&internal[..count]);
        let mut count = count;
        // Keep receiver event-slot and paired internal dependencies unchanged.
        // A referenced Source milestone names its target grid and counter.
        let external = [
            if kind == Kind::Tiling { p.prior_queue_ta } else { None },
            if kind == Kind::Tiling { p.cdm_dependency } else { None },
            if (kind == Kind::Fragment) == p.vdm_barrier_fragment {
                p.vdm_dependency
            } else { None },
        ];
        for point in external.into_iter().flatten() {
            if point.0 >= 128 || point.1 >= (1 << 30) { return Err(c::Error::Invalid); }
            if let Some(existing) = points[..count].iter_mut().find(|existing| existing.0 == point.0) {
                existing.1 = existing.1.max(point.1);
            } else {
                points[count] = point;
                count += 1;
            }
        }
        graph::Context {
            kind,
            descriptor: self.descriptor_address(kind),
            queue: self.layout.queues[kind.index() as usize],
            pair: self.layout.pair,
            item: self.index,
            context: Some(self.layout.context),
            grid: Some(self.layout.grids[kind.index() as usize]),
            locator_context: Some(self.layout.context),
            partial_opening: false,
            dependency_grid: None,
            points: if self.layout.native {
                None
            } else {
                Some(&points[..count])
            },
            event_slot: Some(self.layout.pair as u8),
            completion: None,
        }
        .build(out)
    }
}

/// announce_runtime_tick(): context zero, no fields inferred from prior work.
pub(crate) fn control_tick(ordinal: u32) -> Result<[u8; 0x40]> {
    if ordinal < 2 {
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
