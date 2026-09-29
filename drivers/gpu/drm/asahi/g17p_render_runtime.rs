// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Caller-first partial render and a retained pair-zero append.
//! Firmware objects and caller GEMs remain owned through both RTKit shutdowns.

use super::{
    g17p_abi as abi, g17p_compute as c,
    g17p_compute_runtime::Client,
    g17p_image::Image,
    g17p_memory::{self, Memory},
    g17p_opening as opening, g17p_queue as q,
    g17p_render::{self as r, Kind, Parameters},
    g17p_render_graph as graph, g17p_topology as topology,
    g17p_vm::Vm,
};
use kernel::prelude::*;
const PAGE: usize = 0x4000;
use super::g17p_render_lifecycle::{
    self as life, EVENTS, FW_TIMESTAMPS, JOB_LIST, LEAVES, OPTIONAL, POOLS, QUEUES, RINGS, SHARED,
};
pub(crate) use super::g17p_render_lifecycle::{DESCRIPTORS, POINTERS, STATUS};

pub(crate) fn first_parameters() -> Parameters {
    Parameters {
        context_base: 0x1000000000,
        tilemap: 0x10001b0000,
        heapmeta: 0x10001b1000,
        tpc: 0x10001d8000,
        ta_status: 0x1000078000,
        fragment_status: 0x10001a8000,
        deflake_1: 0x10000682a0,
        deflake_2: 0x1000068020,
        deflake_3: 0x1000068000,
        aux_fb: 0x10000300000,
        reactive_tvb_growth: true,
        emit_uapi_fields: true,
        ..Default::default()
    }
}

struct Deferred {
    address: u64,
    body: KVVec<u8>,
}
pub(crate) struct Submission {
    pub(crate) client: Client,
    pub(crate) publications: [q::Publication; 2],
    pub(crate) channels: [abi::Channel; 2],
    pub(crate) timestamps: [u64; 4],
    pub(crate) growth: Option<super::g17p_growth_runtime::Service>,
    pub(crate) ordinal: u32,
    deferred: [KVec<Deferred>; 2],
    empty_high: [u64; 2],
    adopted: bool,
}

/// quiesce_submission(semantic_complete=True), for the sole synchronous owner.
/// Call only after its queues, statuses and new terminal have all completed.
pub(crate) fn quiesce(memory: &mut Memory, vm: &Vm, work: &Submission) -> Result<bool> {
    for stage in 0..2 {
        let target = work.publications[stage].write_after;
        for offset in [q::POINTER_DONE, q::POINTER_READ, q::POINTER_WRITE] {
            if memory.read_firmware32(vm.physical(memory, 2, POINTERS[stage] + offset)?)? != target
            {
                return Err(EBUSY);
            }
        }
    }
    let tail = vm.physical(memory, 2, JOB_LIST + 8)?;
    memory.invalidate(tail, 8)?;
    if memory.read64(tail)? == JOB_LIST {
        return Ok(false);
    }
    // Both halves name this one retained head. Never clear pool records or
    // neighboring list nodes; Python resets only the 0x18-byte list header.
    vm.write(memory, 2, JOB_LIST, &q::job_list(JOB_LIST))?;
    g17p_memory::sync();
    memory.invalidate(tail, 8)?;
    if memory.read64(tail)? != JOB_LIST {
        return Err(EIO);
    }
    Ok(true)
}
struct Writer<'a> {
    memory: &'a mut Memory,
    vm: &'a Vm,
}
impl q::Writer for Writer<'_> {
    type Error = Error;
    fn write(&mut self, address: u64, bytes: &[u8]) -> Result {
        self.vm.write(self.memory, 2, address, bytes)
    }
}
fn pause(
    memory: &mut Memory,
    vm: &Vm,
    deferred: &mut KVec<Deferred>,
    address: u64,
    early: &[u8],
) -> Result {
    let mut body = KVVec::with_capacity(early.len(), GFP_KERNEL)?;
    body.resize(early.len(), 0, GFP_KERNEL)?;
    for offset in 0..early.len() {
        let at = address + offset as u64;
        let pa = vm.physical(memory, 2, at & !7)?;
        memory.invalidate(pa, 8)?;
        let bytes = memory.read64(pa)?.to_le_bytes();
        body[offset] = bytes[(at & 7) as usize];
    }
    deferred.push(Deferred { address, body }, GFP_KERNEL)?;
    vm.write(memory, 2, address, early)
}
fn overlap(client: &Client, address: u64, size: u64) -> bool {
    client.bindings.iter().any(|&(base, len, _, _)| {
        let base = if base < 0x1000000000 {
            base + 0x1000000000
        } else {
            base
        };
        base < address + size && address < base + len
    })
}

pub(crate) fn validate_client(client: &Client, p: &Parameters) -> Result {
    p.validate().map_err(|_| EINVAL)?;
    // These writable/private pages cannot be handed over to caller bindings.
    for (va, len) in [
        (p.deflake_3, PAGE as u64),
        (p.ta_status, PAGE as u64),
        (p.fragment_status, PAGE as u64),
        (p.tilemap, 0x24000),
        (p.tpc, PAGE as u64),
        (0x1000080000, PAGE as u64),
        (0x1000190000, 4 * PAGE as u64),
        (p.aux_fb, PAGE as u64),
        (0x7000000000, 0x10000),
        (
            opening::CONTEXTS[0].1,
            opening::CONTEXT_PAGES as u64 * PAGE as u64,
        ),
        (
            opening::CONTEXTS[1].1,
            opening::CONTEXT_PAGES as u64 * PAGE as u64,
        ),
        (0x7000208000, PAGE as u64),
        (
            super::g17p_growth::GROWTH_BASE,
            super::g17p_growth::GROWTH_END - super::g17p_growth::GROWTH_BASE,
        ),
    ] {
        if overlap(client, va, len) {
            return Err(EINVAL);
        }
    }
    for va in opening::EXTRA_RENDER {
        if overlap(client, va, PAGE as u64) {
            return Err(EINVAL);
        }
    }
    Ok(())
}

// Shared first-generation constructor: a dormant owner has no workload;
// adoption replaces this body with the actual caller before its first producer.
fn first_descriptor(index: usize, p: &Parameters, page: &mut [u8]) -> Result {
    let kind = if index == 0 {
        Kind::Tiling
    } else {
        Kind::Fragment
    };
    let mut registers = KVec::new();
    if index == 0 {
        registers.extend_from_slice(&r::tiling_registers(p).map_err(|_| EINVAL)?, GFP_KERNEL)?;
    } else {
        registers.extend_from_slice(&r::fragment_registers(p).map_err(|_| EINVAL)?, GFP_KERNEL)?;
    }
    for (n, v) in &mut registers {
        *v = match (index, *n) {
            (0, 0xa5a1) => 0xe900400020,
            (1, 0xa5a9) => 0xed00400020,
            (0, 0x1ca10 | 0x14a1 | 0xa349) => 0xc4010000e8,
            (1, 0x160e0 | 0x1499 | 0xa341) => 0xc4010000e7,
            _ => *v,
        };
    }
    r::Descriptor {
        kind,
        index: 0,
        sequence: if index == 0 { 1 } else { 0 },
        ordinal: 0,
        context: 1,
        queue_pair: 0,
        pool_bases: POOLS,
        record_indices: [0, 0],
        shared: SHARED,
        low_alias: None,
        status_base: Some(STATUS[index]),
        grid: None,
        write_tail: true,
        write_lifecycle: true,
        write_item: true,
        write_structural: true,
        pointer_overrides: &[(if index == 0 { 0x934 } else { 0x21ce }, opening::SUPPORT)],
        item_overrides: &[],
    }
    .build(page, &registers, Some(p))
    .map_err(|_| EINVAL)?;
    let extra: &[(usize, u8)] = if index == 0 {
        &[(0x789, 8), (0x93e, 0xd0), (0x93f, 0x91)]
    } else {
        &[(0x215c, 0), (0x21d8, 0x10), (0x21d9, 0xa2), (0x222d, 0)]
    };
    for &(at, v) in extra {
        page[at] = v;
    }
    Ok(())
}

pub(crate) fn build(
    memory: &mut Memory,
    vm: &mut Vm,
    image: &Image,
    ttbs: u64,
    mut client: Client,
    p: &Parameters,
) -> Result<Submission> {
    validate_client(&client, p)?;
    // Borrow only the already-owned, zero/source-built private render shape.
    // Caller pages keep their own GEM backing, permissions and physical owner.
    for &(first, count, flags) in topology::RENDER_RUNS {
        for index in 0..count {
            let va = first + index as u64 * PAGE as u64;
            if overlap(&client, va, PAGE as u64) {
                continue;
            }
            let pa = vm.physical(memory, 1, va)?;
            client.root.prepare(va, PAGE as u64)?;
            client.root.map_owned_page(va, pa, flags)?;
        }
    }
    for va in opening::EXTRA_RENDER {
        if overlap(&client, va, PAGE as u64) {
            return Err(EINVAL);
        }
        let pa = vm.physical(memory, 1, va)?;
        client.root.prepare(va, PAGE as u64)?;
        client.root.map_page(va, pa, true)?;
    }
    let mut page = KVVec::with_capacity(PAGE, GFP_KERNEL)?;
    page.resize(PAGE, 0, GFP_KERNEL)?;
    r::aux_fb(&mut page).map_err(|_| EINVAL)?;
    vm.write(memory, 1, p.aux_fb, &page)?;

    for (address, size) in [
        (POOLS[0] - 0x100, PAGE),
        (POOLS[1] - 0x80, PAGE),
        (SHARED[0], PAGE),
        (SHARED[1], 0x100),
        (OPTIONAL[1], 0x180),
        (EVENTS[1], 0x80),
        (QUEUES[0], 0x180),
        (POINTERS[0], 0x28f0),
        (RINGS[0], 0x2890),
        (LEAVES[1] + PAGE as u64, PAGE),
    ] {
        vm.ensure_firmware(memory, address, size)?;
    }
    // The submission builder allocates these leaves explicitly; the static
    // firmware extent does not contain every page (notably PrimaryIndex).
    for address in LEAVES {
        vm.submission_leaf(memory, address)?;
    }
    vm.alias_firmware(
        memory,
        DESCRIPTORS[0],
        0x7000000000,
        life::SUBMISSIONS as usize * r::TA_SIZE,
    )?;
    vm.alias_firmware(
        memory,
        DESCRIPTORS[1],
        0x7000098000,
        life::SUBMISSIONS as usize * r::FRAGMENT_SIZE,
    )?;
    let kinds = [
        graph::Leaf::PrimaryIndex,
        graph::Leaf::SecondaryIndex,
        graph::Leaf::PoolASlots,
        graph::Leaf::PoolBSlots,
        graph::Leaf::SharedSlots,
        graph::Leaf::Flag,
    ];
    for (kind, address) in kinds.into_iter().zip(LEAVES) {
        graph::leaf(&mut page, kind, 0, &graph::CONTEXT2_INDEX_GROUPS, 8, 0).map_err(|_| EINVAL)?;
        vm.write(memory, 2, address, &page)?;
    }
    graph::leaf(
        &mut page,
        graph::Leaf::PrimaryIndex,
        0,
        &graph::CONTEXT2_INDEX_GROUPS,
        8,
        0,
    )
    .map_err(|_| EINVAL)?;
    vm.write(memory, 1, 0x1000190000, &page)?;
    // apply_render_firmware_aliases(prefer_low=True): four index pages plus
    // the two status records and pool-B handoff. Growth uses the full index.
    for (low, high) in [
        (p.ta_status, STATUS[0]),
        (0x1000080000, LEAVES[3]),
        (0x1000190000, LEAVES[0]),
        (0x1000194000, LEAVES[0] + 0x4000),
        (0x1000198000, LEAVES[0] + 0x8000),
        (0x100019c000, LEAVES[0] + 0xc000),
        (p.fragment_status, STATUS[1]),
    ] {
        vm.render_firmware_alias(memory, low, high)?;
    }
    let write = |memory: &mut Memory, address, body: &[u8]| vm.write(memory, 2, address, body);
    graph::record_array_a(&mut page[..graph::POOL_A_SIZE], LEAVES[2] + 4, 0).map_err(|_| EINVAL)?;
    write(memory, POOLS[0], &page[..graph::POOL_A_SIZE])?;
    page.fill(0);
    c::u64_at(&mut page, 0, LEAVES[2]);
    write(memory, POOLS[0] - 0x100, &page[..0x100])?;
    graph::record_array_b(
        &mut page[..graph::POOL_B_SIZE],
        LEAVES[3] + 4,
        LEAVES[4] + 0x40,
        0,
        0,
    )
    .map_err(|_| EINVAL)?;
    write(memory, POOLS[1], &page[..graph::POOL_B_SIZE])?;
    page.fill(0);
    c::u32_at(&mut page, 0, 0x80000);
    c::u32_at(&mut page, 4, 0x10);
    c::u64_at(&mut page, 8, LEAVES[3]);
    c::u32_at(&mut page, 0x28, 0x178000);
    c::u64_at(&mut page, 0x40, LEAVES[4] + 0x40);
    write(memory, POOLS[1] - 0x80, &page[..0x80])?;
    graph::Shared {
        pointers: [LEAVES[0], LEAVES[1], LEAVES[4], LEAVES[5]],
        pair: 0,
        groups: 8,
        work: 0,
    }
    .build(&mut page[..0x88])
    .map_err(|_| EINVAL)?;
    write(memory, SHARED[0], &page[..0x88])?;
    page.fill(0);
    write(memory, SHARED[1], &page[..0x100])?;
    write(memory, LEAVES[1] + PAGE as u64, &page)?;
    page[..8].copy_from_slice(&opening::resource_record());
    write(memory, 0xfffffc20015e0000, &page)?;
    write(memory, JOB_LIST, &q::job_list(JOB_LIST))?;
    for status in STATUS {
        write(memory, status, &[0; 0x40])?;
    }
    for status in FW_TIMESTAMPS {
        write(memory, status, &[0; 8])?;
    }

    let channels = [image.graph.channels[0][6], image.graph.channels[0][7]];
    if channels[0].ring != 0xfffffc20c0795dc0 || channels[1].ring != 0xfffffc20c079bdc0 {
        return Err(EINVAL);
    }
    let mut publications = KVec::with_capacity(2, GFP_KERNEL)?;
    for index in 0..2 {
        let kind = if index == 0 {
            Kind::Tiling
        } else {
            Kind::Fragment
        };
        first_descriptor(index, p, &mut page[..kind.size()])?;
        write(memory, DESCRIPTORS[index], &page[..kind.size()])?;
        graph::Optional {
            kind,
            context_scratch: opening::CONTEXTS[index].1,
            firmware_scratch: opening::CONTEXTS[index].0,
            shared_control: opening::SUPPORT,
            channel_control: opening::CHANNEL_CONTROL,
            tiling_shared: if index == 0 { Some(SHARED[0]) } else { None },
            grid: index as u16,
            item: 0,
            ordinal: 0,
            context: None,
            uuid: Some(0x15),
            scheduler_class: None,
            context_index: None,
            context_phase: None,
            first: None,
            lifecycle: None,
            namespace: None,
            overrides: &[(0x1e, 2), (0x32, 1), (0x5e, 2)],
        }
        .build(&mut page[..0xc0])
        .map_err(|_| EINVAL)?;
        write(memory, OPTIONAL[index], &page[..0xc0])?;
        page.fill(0);
        page[..0x60].copy_from_slice(&q::pointers(u32::MAX));
        c::u64_at(&mut page, 0x60, 0x500);
        write(memory, POINTERS[index], &page[..0x80])?;
        write(
            memory,
            QUEUES[index],
            &q::Record {
                pointers: POINTERS[index],
                ring: RINGS[index],
                job_list: JOB_LIST,
                context: opening::CHANNEL_CONTROL,
                uuid: 0x15,
                priority: 2,
                prio5: 2,
                unk_2c: 2,
                unk_38: 0,
                unk_30: None,
                unk_94: 0,
                sentinel_size: 2,
            }
            .build()
            .map_err(|_| EINVAL)?,
        )?;
        let context = graph::Context {
            kind,
            descriptor: DESCRIPTORS[index],
            queue: QUEUES[index],
            pair: 0,
            item: 0,
            context: Some(1),
            grid: None,
            locator_context: None,
            partial_opening: true,
            dependency_grid: None,
            points: None,
            event_slot: None,
            completion: None,
        };
        context
            .build(&mut page[..graph::CONTEXT_SIZE])
            .map_err(|_| EINVAL)?;
        write(
            memory,
            opening::CONTEXTS[index].0 + 0x200,
            &page[..graph::CONTEXT_SIZE],
        )?;
        // Context 0 already aliases the firmware context pages. The render
        // root at this same low DVA is independent operand backing and must
        // remain blank until the GPU uses it (partial bootstrap's clear pass).
        let channel = channels[index];
        publications.push(
            q::Stage {
                queue: QUEUES[index],
                pointers: POINTERS[index],
                item_ring: RINGS[index],
                item_capacity: 0x2870 / 8,
                write_index: 0,
                channel_ring: channel.ring,
                channel_producer: channel.states[2],
                counters: q::Counters::new([0; 3]).map_err(|_| EINVAL)?,
                slot: Some(0),
                items: &[DESCRIPTORS[index], OPTIONAL[index], EVENTS[index]],
                group: 1,
                grid: index as u32,
                kind: if index == 0 {
                    q::Kind::Tiling
                } else {
                    q::Kind::Fragment
                },
                first: true,
                in_place: false,
                announce: false,
                defer_inner: false,
                defer_outer: true,
                event_subtype: None,
                event_counter: None,
                event_counter_low: 2,
            }
            .publish(&mut Writer { memory, vm })
            .map_err(|e| match e {
                q::StageError::Access(e) => e,
                _ => EINVAL,
            })?,
            GFP_KERNEL,
        )?;
    }
    let mut deferred = [KVec::new(), KVec::new()];
    // Preserve the shim's default withhold/restore order. The outer producer
    // stays hidden until each stage's closure has been fully restored.
    for index in 0..2 {
        pause(
            memory,
            vm,
            &mut deferred[index],
            channels[index].ring,
            &[0; 24],
        )?;
        pause(
            memory,
            vm,
            &mut deferred[index],
            POINTERS[index] + 0x40,
            &[0; 4],
        )?;
        if index == 0 {
            pause(
                memory,
                vm,
                &mut deferred[index],
                LEAVES[2] + 4,
                &1u32.to_le_bytes(),
            )?;
            pause(
                memory,
                vm,
                &mut deferred[index],
                opening::STATE,
                &1u32.to_le_bytes(),
            )?;
        }
        pause(memory, vm, &mut deferred[index], RINGS[index], &[0; 24])?;
        pause(memory, vm, &mut deferred[index], EVENTS[index], &[0; 0x40])?;
        pause(
            memory,
            vm,
            &mut deferred[index],
            OPTIONAL[index],
            &[0; 0xc0],
        )?;
    }
    let mut empty_high = [0; 2];
    for pa in &mut empty_high {
        *pa = memory.allocate(PAGE)?;
        memory.clean(*pa, PAGE)?;
    }
    client.cache(false)?;
    vm.flush_tables(memory)?;
    memory.write64(ttbs + 16, (1 << 48) | client.root.root() | 1)?;
    memory.clean(ttbs, 64 * 16)?;
    tlbi();
    let fragment = publications.pop().ok_or(EINVAL)?;
    let tiling = publications.pop().ok_or(EINVAL)?;
    Ok(Submission {
        client,
        publications: [tiling, fragment],
        channels,
        deferred,
        empty_high,
        timestamps: [
            p.ta_user_timestamp_start,
            p.ta_user_timestamp_end,
            p.fragment_user_timestamp_start,
            p.fragment_user_timestamp_end,
        ],
        growth: None,
        ordinal: 0,
        adopted: false,
    })
}

fn tlbi() {
    g17p_memory::sync();
    // SAFETY: Owned GPU page tables are cleaned before invalidating translations.
    unsafe {
        core::arch::asm!(
            ".inst 0xd508811f",
            "dsb sy",
            "isb",
            options(nostack, preserves_flags)
        );
    }
}
impl Submission {
    pub(crate) fn after_control(&self, memory: &mut Memory, vm: &Vm, ttbs: u64) -> Result {
        if self.adopted {
            // The dormant startup already installed these roots and operands.
            // In particular, slots 2/3 now belong to the live compute owner.
            return Ok(());
        }
        for (slot, pa) in self.empty_high.into_iter().enumerate() {
            memory.write64(ttbs + slot as u64 * 16 + 8, ((slot as u64) << 48) | pa | 1)?;
        }
        memory.write64(ttbs + 2 * 16, 0)?;
        memory.write64(ttbs + 2 * 16 + 8, 0)?;
        memory.clean(ttbs, 64 * 16)?;
        tlbi();
        self.initialize_operands(memory, vm)
    }
    pub(crate) fn initialize_operands(&self, memory: &mut Memory, vm: &Vm) -> Result {
        // Render-root directory is distinct from the same DVAs in context 0.
        let size = graph::operand_directory_size(28).map_err(|_| EINVAL)?;
        let mut body = KVVec::with_capacity(size, GFP_KERNEL)?;
        body.resize(size, 0, GFP_KERNEL)?;
        graph::operand_directory(&mut body, 0x7000220000, 28).map_err(|_| EINVAL)?;
        vm.write(memory, 1, 0x7000000000, &body)?;
        graph::operand_table(&mut body[..PAGE], 0x7000220000, 28).map_err(|_| EINVAL)?;
        vm.write(memory, 1, 0x7000208000, &body[..PAGE])?;
        Ok(())
    }
    /// _prepare_render_after_compute: adopt the retained graph. Neither its
    /// pools nor compute history is reset, and every producer remains hidden
    /// until the complete caller body and root have been installed.
    pub(crate) fn adopt(
        &mut self,
        memory: &mut Memory,
        vm: &Vm,
        client: Client,
        p: &Parameters,
    ) -> Result {
        validate_client(&client, p)?;
        if self.adopted || self.ordinal != 0 || self.growth.is_some() {
            return Err(EINVAL);
        }
        for index in 0..2 {
            for at in [q::POINTER_DONE, q::POINTER_READ, q::POINTER_WRITE] {
                if memory.read_firmware32(vm.physical(memory, 2, POINTERS[index] + at)?)? != 0 {
                    return Err(EBUSY);
                }
            }
            for at in self.channels[index].states {
                if memory.read_firmware32(vm.physical(memory, 2, at)?)? != 0 {
                    return Err(EBUSY);
                }
            }
        }
        let mut body = KVVec::with_capacity(r::FRAGMENT_SIZE, GFP_KERNEL)?;
        body.resize(r::FRAGMENT_SIZE, 0, GFP_KERNEL)?;
        // The source startup's blank render extent is replaceable by caller
        // mappings, except the private ranges rejected by validate_client.
        // Preserve the retained root and perform break-before-make before
        // adopting any caller references. Blank backing stays Memory-owned.
        let mut changes = KVec::new();
        for &(base, size, _, _) in &client.bindings {
            let va = if base < 0x1000000000 {
                base + 0x1000000000
            } else {
                base
            };
            for offset in (0..size).step_by(PAGE) {
                let address = va + offset;
                let old = self.client.root.pte(address)?;
                let new = client.root.pte(address)?;
                if new == 0 {
                    return Err(EIO);
                }
                changes.push((address, old, new), GFP_KERNEL)?;
            }
        }
        client.cache(false)?;
        self.client.root.rebind(&changes, &[1])?;
        self.client.buffers = client.buffers;
        self.client.bindings = client.bindings;
        self.client.owner = client.owner;
        for (index, kind) in [Kind::Tiling, Kind::Fragment].into_iter().enumerate() {
            first_descriptor(index, p, &mut body[..kind.size()])?;
            vm.write(memory, 2, DESCRIPTORS[index], &body[..kind.size()])?;
            // UUID belongs to the retained queue, as in the source adoption.
            vm.write(memory, 2, QUEUES[index] + 0x48, &0x15u32.to_le_bytes())?;
        }
        for address in FW_TIMESTAMPS {
            vm.write(memory, 2, address, &[0; 8])?;
        }
        self.timestamps = [
            p.ta_user_timestamp_start,
            p.ta_user_timestamp_end,
            p.fragment_user_timestamp_start,
            p.fragment_user_timestamp_end,
        ];
        self.adopted = true;
        g17p_memory::sync();
        Ok(())
    }
    pub(crate) fn restore(&self, memory: &mut Memory, vm: &Vm, index: usize) -> Result {
        for store in self.deferred.get(index).ok_or(EINVAL)? {
            vm.write(memory, 2, store.address, &store.body)?;
        }
        g17p_memory::sync();
        let (address, value) = self.publications[index].deferred_outer.ok_or(EINVAL)?;
        vm.write(memory, 2, address, &value.to_le_bytes())?;
        g17p_memory::sync();
        Ok(())
    }
}

/// Append only after the previous submission passed queues, status, timestamps
/// and report validation. Preserve all firmware-owned pool and directory state.
pub(crate) fn stage_next(
    memory: &mut Memory,
    vm: &Vm,
    work: &mut Submission,
    p: &Parameters,
) -> Result {
    let item = life::Item::new(work.ordinal + 1).map_err(|_| EINVAL)?;
    let mut counters = [q::Counters::new([0; 3]).map_err(|_| EIO)?; 2];
    let word = |address| memory.read_firmware32(vm.physical(memory, 2, address)?);
    for index in 0..2 {
        let previous = work.publications[index].write_after;
        for at in [0, 0x30, 0x40] {
            if word(POINTERS[index] + at)? != previous {
                return Err(EBUSY);
            }
        }
        let channel = work.channels[index];
        counters[index] = q::Counters::new([
            word(channel.states[0])?,
            word(channel.states[1])?,
            word(channel.states[2])?,
        ])
        .map_err(|_| EIO)?;
        if !work.publications[index].completed(previous, counters[index]) {
            return Err(EBUSY);
        }
        counters[index].slot().map_err(|_| EBUSY)?;
    }
    if counters[0].0 != counters[1].0 {
        return Err(EIO);
    }
    // Allocate/build the entire append before editing any live resource state.
    // Every destination was mapped and retained before the first publication.
    let mut objects: [KVec<Deferred>; 2] = [KVec::new(), KVec::new()];
    for index in 0..2 {
        let kind = if index == 0 {
            Kind::Tiling
        } else {
            Kind::Fragment
        };
        for (address, size, ty) in [
            (item.descriptor_address(kind), kind.size(), 0),
            (item.optional_address(kind), 0xc0, 1),
            (
                opening::CONTEXTS[index].0 + 0x200 * (item.ordinal as u64 + 1),
                graph::CONTEXT_SIZE,
                2,
            ),
        ] {
            let mut body = KVVec::with_capacity(size, GFP_KERNEL)?;
            body.resize(size, 0, GFP_KERNEL)?;
            match ty {
                0 => item.descriptor(kind, p, &mut body),
                1 => item.optional(kind, &mut body),
                _ => item.context(kind, &mut body),
            }
            .map_err(|_| EINVAL)?;
            for off in (0..size).step_by(PAGE) {
                vm.physical(memory, 2, address + off as u64)?;
            }
            vm.physical(memory, 2, address + size as u64 - 1)?;
            objects[index].push(Deferred { address, body }, GFP_KERNEL)?;
        }
    }
    // _advance_tilemap_block resets a completed allocation only when its
    // eight-block ring wraps. Keep the persistent directory and TPC intact.
    if item.ordinal >= 8 {
        let zeros = [0u8; 0x1200];
        vm.write(
            memory,
            1,
            p.tilemap + (item.ordinal % 8) as u64 * 0x1200,
            &zeros,
        )?;
    }
    work.client.cache(false)?;
    work.growth
        .as_mut()
        .ok_or(EINVAL)?
        .bind_work(item.descriptor_address(Kind::Fragment))?;
    let write = |memory: &mut Memory, address, body: &[u8]| vm.write(memory, 2, address, body);
    let records = item.records();
    let node = item.ordinal + item.ordinal / 2;
    write(
        memory,
        POOLS[0] + records[0] as u64 * 0x100 + 8,
        &node.to_le_bytes(),
    )?;
    write(
        memory,
        POOLS[0] + records[0] as u64 * 0x100 + 0x10,
        &0x50u32.to_le_bytes(),
    )?;
    write(
        memory,
        POOLS[1] + records[1] as u64 * 0x80 + 0x4c,
        &1u32.to_le_bytes(),
    )?;
    // Source lifecycle before -> fragment -> tiling. No producer yet visible.
    write(memory, LEAVES[4] + 0x60, &1u32.to_le_bytes())?;
    write(memory, item.slot(), &0u32.to_le_bytes())?;
    for index in [1, 0] {
        for object in &objects[index] {
            write(memory, object.address, &object.body)?;
        }
        // Do not copy this firmware record into the render-root operand
        // backing at the same low DVA. Context 0 already aliases the high view.
        let phase = if index == 1 { 1u32 } else { 2u32 };
        write(memory, item.slot(), &phase.to_le_bytes())?;
        write(
            memory,
            opening::STATE,
            &(2 * item.ordinal + phase).to_le_bytes(),
        )?;
        write(
            memory,
            item.status(if index == 0 {
                Kind::Tiling
            } else {
                Kind::Fragment
            }),
            &[0; 0x40],
        )?;
        if index == 1 {
            write(memory, LEAVES[5], &(item.ordinal + 1).to_le_bytes())?;
            for address in FW_TIMESTAMPS {
                write(memory, address, &[0; 8])?;
            }
        }
    }
    // Retired prior PB release, then the same retained owner is selected again.
    write(memory, SHARED[0] + 0x0c, &u32::MAX.to_le_bytes())?;
    write(memory, SHARED[0] + 0x0c, &0u32.to_le_bytes())?;
    write(memory, SHARED[0], &item.ordinal.to_le_bytes())?;
    let count = memory.read_firmware32(vm.physical(memory, 2, SHARED[0] + 0x34)?)?;
    let pa = vm.physical(memory, 2, SHARED[0] + 0x28)?;
    memory.invalidate(pa, 8)?;
    if let Some(body) = life::index_registration(memory.read64(pa)?, count).map_err(|_| EIO)? {
        write(memory, 0xfffffc20015e0000, &body)?;
    }
    g17p_memory::sync();
    for index in 0..2 {
        let kind = if index == 0 {
            Kind::Tiling
        } else {
            Kind::Fragment
        };
        let channel = work.channels[index];
        work.publications[index] = q::Stage {
            queue: QUEUES[index],
            pointers: POINTERS[index],
            item_ring: RINGS[index],
            item_capacity: 0x2870 / 8,
            write_index: work.publications[index].write_after,
            channel_ring: channel.ring,
            channel_producer: channel.states[2],
            counters: counters[index],
            slot: None,
            items: &[
                item.descriptor_address(kind),
                item.optional_address(kind),
                item.event_address(kind),
            ],
            group: item.ordinal + 1,
            grid: index as u32,
            kind: if index == 0 {
                q::Kind::Tiling
            } else {
                q::Kind::Fragment
            },
            first: false,
            in_place: false,
            announce: false,
            defer_inner: false,
            defer_outer: true,
            event_subtype: None,
            event_counter: None,
            event_counter_low: 0,
        }
        .publish(&mut Writer { memory, vm })
        .map_err(|e| match e {
            q::StageError::Access(e) => e,
            _ => EINVAL,
        })?;
    }
    for index in 0..2 {
        work.deferred[index].clear();
    }
    work.ordinal = item.ordinal;
    work.timestamps = [
        p.ta_user_timestamp_start,
        p.ta_user_timestamp_end,
        p.fragment_user_timestamp_start,
        p.fragment_user_timestamp_end,
    ];
    Ok(())
}
