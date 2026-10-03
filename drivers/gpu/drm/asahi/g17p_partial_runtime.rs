// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Direct g17p_partial_runtime.py second-owner resource graph. Transport
//! admission remains separate from constructing this independent inventory.

use super::{
    g17p_compute as c,
    g17p_memory::{self, Memory},
    g17p_render::Parameters,
    g17p_render_graph as graph,
    g17p_render_lifecycle::{Layout, SECOND},
    g17p_resource_record as resource,
    g17p_user_vm::UserVm,
    g17p_vm::Vm,
};
use kernel::prelude::*;
const PAGE: usize = 0x4000;
const BASE: u64 = 0x1000000000;
pub(crate) const STRIDE: u64 = 0x1b0000;
pub(crate) const GROUPS: [u32; 8] = [71, 76, 81, 86, 91, 96, 114, 119];
pub(crate) const GRAPH: [(&str, u64, usize); 10] = [
    ("submission_primary_index", 0xfffffc20c0888000, 0x10000),
    ("submission_secondary_index", 0xfffffc20c0878000, 0x8000),
    ("submission_pool_a_slots", 0xfffffc2001700000, PAGE),
    ("submission_pool_b_slots", 0xfffffc2001708000, PAGE),
    ("submission_shared_slots", SECOND.leaves[4], PAGE),
    ("submission_flag", 0xfffffc2001648000, PAGE),
    ("record_pool_a", 0xfffffc20c0900100, 0x2300),
    ("record_pool_b", SECOND.pools[1], 0x2780),
    ("descriptor_shared_object", 0xfffffc20c08a0000, 0x88),
    ("descriptor_zero_object", SECOND.shared[1], 0x100),
];
pub(crate) fn private_ranges() -> [(u64, usize); 14] {
    let mut ranges = [(0, 0); 14];
    for (i, group) in GROUPS.into_iter().enumerate() {
        ranges[i] = (BASE + group as u64 * 0x8000, 0x20000);
    }
    ranges[8..].copy_from_slice(&[
        (BASE + 0x328000, PAGE),
        (BASE + 0x330000, 0x8000),
        (BASE + 0x360000, 0x24000),
        (BASE + 0x388000, PAGE),
        (BASE + 0x1000000, PAGE),
        (BASE + 0x1004000, PAGE),
    ]);
    ranges
}
pub(crate) fn alias_pages() -> [u64; 6] {
    [0x230000, 0x340000, 0x344000, 0x348000, 0x34c000, 0x358000].map(|offset| BASE + offset)
}
pub(crate) fn parameters(p: &Parameters) -> Result<Parameters> {
    if p.queue_pair > 1 {
        return Err(EINVAL);
    }
    let mut p = Parameters {
        tvb_pool_id: Some(p.queue_pair),
        pair_resource_stride: STRIDE,
        native_cycle_registers: true,
        native_record_index_register: true,
        native_status_registers: false,
        native_item_fields: false,
        ta_status: BASE + [0x78000, 0x230000][p.queue_pair as usize],
        fragment_status: BASE + 0x1a8000 + p.queue_pair * STRIDE,
        ..*p
    };
    if p.queue_pair == 1 {
        p.deflake_1 = BASE + 0x10002a0;
        p.deflake_2 = BASE + 0x1000020;
        p.deflake_3 = BASE + 0x1000000;
        p.aux_fb = BASE + 0x1004000;
    }
    Ok(p)
}
fn read64(memory: &Memory, vm: &Vm, address: u64) -> Result<u64> {
    let aligned = address & !7;
    let pa = vm.physical(memory, 2, aligned)?;
    memory.invalidate(pa, 8)?;
    let low = memory.read64(pa)?;
    if address == aligned {
        return Ok(low);
    }
    let pa = vm.physical(memory, 2, aligned + 8)?;
    memory.invalidate(pa, 8)?;
    let shift = (address & 7) * 8;
    Ok(low >> shift | memory.read64(pa)? << (64 - shift))
}

pub(crate) fn build_graph(
    memory: &mut Memory,
    vm: &mut Vm,
    root: &mut UserVm,
    primary: Layout,
    control: u64,
) -> Result {
    build_pool_graph(memory, vm, root, primary, SECOND, 0, control)
}

pub(crate) fn build_pool_graph(
    memory: &mut Memory, vm: &mut Vm, root: &mut UserVm,
    primary: Layout, layout: Layout, delta: u64, control: u64,
) -> Result {
    let graph = [
        (layout.leaves[0], 0x10000), (layout.leaves[1], 0x8000),
        (layout.leaves[2], PAGE), (layout.leaves[3], PAGE),
        (layout.leaves[4], PAGE), (layout.leaves[5], PAGE),
        (layout.pools[0], 0x2300), (layout.pools[1], 0x2780),
        (layout.shared[0], 0x88), (layout.shared[1], 0x100),
    ];
    let group_delta: u32 = (delta / 0x8000).try_into().map_err(|_| EINVAL)?;
    if delta % 0x8000 != 0 { return Err(EINVAL); }
    let inner = read64(memory, vm, control + 0x4c)?;
    if inner == 0 {
        return Err(EIO);
    }
    let mut protected = KVec::new();
    for address in primary.leaves {
        protected.push((address, PAGE as u64), GFP_KERNEL)?;
    }
    for address in [
        primary.pools[0] - 0x100,
        primary.pools[1] - 0x80,
        primary.shared[0] & !0x3fff,
        primary.shared[1] & !0x3fff,
        control & !0x3fff,
        inner & !0x3fff,
    ] {
        protected.push((address, PAGE as u64), GFP_KERNEL)?;
    }
    for &(address, size) in &graph {
        if protected
            .iter()
            .any(|&(base, length)| base < address + size as u64 && address < base + length)
        {
            return Err(EBUSY);
        }
    }
    for &(address, size) in &graph {
        vm.ensure_firmware(memory, address, size)?;
    }
    let mut page = KVVec::with_capacity(PAGE, GFP_KERNEL)?;
    page.resize(PAGE, 0, GFP_KERNEL)?;
    for (index, kind) in [
        graph::Leaf::PrimaryIndex,
        graph::Leaf::SecondaryIndex,
        graph::Leaf::PoolASlots,
        graph::Leaf::PoolBSlots,
        graph::Leaf::SharedSlots,
        graph::Leaf::Flag,
    ]
    .into_iter()
    .enumerate()
    {
        graph::leaf(&mut page, kind, 0, &[(71 + group_delta, 6), (114 + group_delta, 2)], 8, 0).map_err(|_| EINVAL)?;
        if index == 3 {
            vm.write(memory, 2, graph[index].0 + 0x140, &[0; 0x140])?;
        } else {
            vm.write(memory, 2, graph[index].0, &page)?;
        }
    }
    graph::record_array_a(&mut page[..graph::POOL_A_SIZE], graph[2].0 + 4, 0)
        .map_err(|_| EINVAL)?;
    vm.write(memory, 2, graph[6].0, &page[..graph::POOL_A_SIZE])?;
    page[..0x100].fill(0);
    c::u64_at(&mut page, 0, graph[2].0);
    vm.write(memory, 2, graph[6].0 - 0x100, &page[..0x100])?;
    resource::build_partial_pool_b(
        &mut page,
        graph[3].0 + 0x140,
        graph[4].0 + 0x40,
        (0x80140 + delta).try_into().map_err(|_| EINVAL)?,
        (0x328000 + delta).try_into().map_err(|_| EINVAL)?,
        1,
    )
    .map_err(|_| EINVAL)?;
    vm.write(memory, 2, graph[7].0 - 0x80, &page[..0x2800])?;
    resource::PartialIndexSharedObject {
        index_high: graph[0].0,
        index_low: BASE + 0x340000 + delta,
        pool_b_slots: graph[1].0,
        shared_slots: graph[4].0,
        flag: graph[5].0,
        owner: layout.pair,
        head: 8,
        tail: 0,
        group_count: 8,
        opaque_84: (0x330000 + delta).try_into().map_err(|_| EINVAL)?,
    }
    .build(&mut page)
    .map_err(|_| EINVAL)?;
    vm.write(memory, 2, graph[8].0, &page[..0x88])?;
    vm.write(memory, 2, graph[9].0, &[0; 0x100])?;
    let mut changes = KVec::with_capacity(4, GFP_KERNEL)?;
    for offset in (0..0x10000).step_by(PAGE) {
        let low = BASE + 0x340000 + delta + offset as u64;
        let pa = vm.physical(memory, 2, graph[0].0 + offset as u64)?;
        let old = root.pte(low)?;
        if old != 0 && old & 0x000003ffffffc000 != pa {
            return Err(EBUSY);
        }
        let new = pa | 0x00c0000000000c8b;
        if old != new {
            changes.push((low, old, new), GFP_KERNEL)?;
        }
    }
    root.rebind(&changes, &[1])?;
    vm.flush_tables(memory)?;
    g17p_memory::sync();
    Ok(())
}
