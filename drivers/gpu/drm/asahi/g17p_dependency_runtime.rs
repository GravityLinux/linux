// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Preparation of the first retained native C/R/C owner. No work doorbell,
//! control notification or execution-root switch is issued by this module.
//! Session must retain both clients and mark preparation failure terminal.

use super::{
    g17p_abi as abi, g17p_compute as c, g17p_compute_memory as cm,
    g17p_compute_runtime::{self as compute, Client},
    g17p_dependency as d, g17p_dependency_vm as join,
    g17p_image::Image,
    g17p_memory::{self, Memory},
    g17p_opening as opening, g17p_queue as q,
    g17p_render::{self as r, Kind},
    g17p_render_graph as graph, g17p_render_runtime as render, g17p_topology as topology,
    g17p_vm::Vm,
};
use kernel::prelude::*;
const PAGE: usize = 0x4000;
const ADDRESS: u64 = 0x000003ffffffc000;

struct Space<'a> {
    memory: &'a mut Memory,
    compute: &'a mut Client,
    render: &'a Client,
}
impl join::Space for Space<'_> {
    type Error = Error;
    fn pte(&self, root: join::Root, va: u64) -> Result<u64> {
        match root {
            join::Root::Compute => self.compute.root.pte(va),
            join::Root::Render => self.render.root.pte(va),
        }
    }
    fn caller_overlap(&self, va: u64) -> bool {
        self.compute
            .bindings
            .iter()
            .any(|&(base, size, _, _)| base < va + PAGE as u64 && va < base + size)
    }
    fn replace(&mut self, va: u64, pte: u64) -> Result {
        let old = self.compute.root.pte(va)?;
        // This root is the retained compute owner in contexts 2/3, and is
        // about to become the command-buffer owner in context 1. Its old
        // caller pages remain pinned by the Client throughout all three TLBI.
        self.compute.root.rebind(&[(va, old, pte)], &[1, 2, 3])
    }
    fn zero_page(&mut self) -> Result<u64> {
        let pa = self.memory.allocate(PAGE)?;
        self.memory.clean(pa, PAGE)?;
        Ok(pa)
    }
}
fn join_error(error: join::Error<Error>) -> Error {
    match error {
        join::Error::Access(error) => error,
        join::Error::Invalid | join::Error::Unmapped(_) | join::Error::Overlap(_) => EINVAL,
    }
}
struct Writer<'a> {
    memory: &'a mut Memory,
    vm: &'a Vm,
}
impl q::Writer for Writer<'_> {
    type Error = Error;
    fn write(&mut self, va: u64, body: &[u8]) -> Result {
        self.vm.write(self.memory, 2, va, body)
    }
}

pub(crate) struct Prepared {
    /// Opening CL, TA, 3D, closing CL; none of these outer heads is visible.
    pub(crate) publications: [q::Publication; 4],
    pub(crate) channels: [abi::Channel; 4],
    pub(crate) root: u64,
    pub(crate) closing_context: KVVec<u8>,
    pub(crate) robustness: [u64; 2],
    pub(crate) joined: join::Join,
    pub(crate) late_aliases: usize,
    pub(crate) compute_timestamps: [[u64; 2]; 2],
    pub(crate) render_timestamps: [u64; 4],
}
impl Prepared {
    /// Replace only context 1's low root after a quiescent source primer.
    /// The independently owned hardware upper roots stay empty; the populated
    /// firmware high tree is used only for software translation of fields.
    pub(crate) fn activate(&self, memory: &Memory, ttbs: u64, render_root: u64) -> Result {
        let tagged = (1 << 48) | self.root | 1;
        if (self.root | render_root | ttbs) & 0x3fff != 0
            || self.root >= 1 << 42
            || render_root >= 1 << 42
            || memory.read64(ttbs + 16)? != ((1 << 48) | render_root | 1)
        {
            return Err(EINVAL);
        }
        for context in 0..2u64 {
            let entry = memory.read64(ttbs + context * 16 + 8)?;
            let upper = entry & ADDRESS;
            if entry != ((context << 48) | upper | 1) || upper == 0 {
                return Err(EIO);
            }
            memory.invalidate(upper, PAGE)?;
            for offset in (0..PAGE).step_by(8) {
                if memory.read64(upper + offset as u64)? != 0 {
                    return Err(EBUSY);
                }
            }
        }
        let word = memory.word64(ttbs + 16)?;
        // All ownership checks precede the first break. Retained compute and
        // render Clients pin both generations until the final invalidation.
        word.store(0);
        Vm::invalidate_gpu();
        word.store(tagged);
        Vm::invalidate_gpu();
        Ok(())
    }
    /// Capture before release; these are the source's own command destinations,
    /// not a shared global word or a terminal count borrowed from another owner.
    pub(crate) fn retirement(
        &self,
        memory: &Memory,
        vm: &Vm,
    ) -> Result<super::g17p_dependency_retire::Retirement> {
        let observations = self.observe(memory, vm)?;
        Ok(super::g17p_dependency_retire::Retirement::new(
            self.publications,
            observations.map(|o| o.status),
        ))
    }
    pub(crate) fn observe(
        &self,
        memory: &Memory,
        vm: &Vm,
    ) -> Result<[super::g17p_dependency_retire::Observation; 4]> {
        use super::g17p_dependency_retire::Observation;
        let mut observations = [Observation {
            done: 0,
            counters: [0; 3],
            status: [0; 0x40],
        }; 4];
        let addresses = [
            d::Compute::Opening.status()[1],
            d::RENDER_STATUS[0],
            d::RENDER_STATUS[1],
            d::Compute::Closing.status()[1],
        ];
        for (i, observation) in observations.iter_mut().enumerate() {
            observation.done = memory.read_firmware32(vm.physical(
                memory,
                2,
                d::LAYOUTS[i].pointers + q::POINTER_DONE,
            )?)?;
            for (value, va) in observation.counters.iter_mut().zip(self.channels[i].states) {
                *value = memory.read_firmware32(vm.physical(memory, 2, va)?)?;
            }
            let size = if i == 0 || i == 3 { 8 } else { 0x40 };
            for offset in (0..size).step_by(8) {
                let pa = vm.physical(memory, 2, addresses[i] + offset as u64)?;
                memory.invalidate(pa, 8)?;
                observation.status[offset..offset + 8]
                    .copy_from_slice(&memory.read64(pa)?.to_le_bytes());
            }
        }
        Ok(observations)
    }
    pub(crate) fn boundary(&self) -> Result<super::g17p_dependency_release::Boundary<'_>> {
        for publication in &self.publications[..3] {
            if publication.deferred_inner.is_some() {
                return Err(EIO);
            }
        }
        let boundary = super::g17p_dependency_release::Boundary {
            opening_outer: self.publications[0].deferred_outer.ok_or(EIO)?,
            render_outer: [
                self.publications[1].deferred_outer.ok_or(EIO)?,
                self.publications[2].deferred_outer.ok_or(EIO)?,
            ],
            closing_outer: self.publications[3].deferred_outer.ok_or(EIO)?,
            closing_inner: self.publications[3].deferred_inner.ok_or(EIO)?,
            closing_context: (d::LAYOUTS[3].context_high + 0x200, &self.closing_context),
        };
        if !boundary.valid() {
            return Err(EIO);
        }
        Ok(boundary)
    }
}

fn counters(memory: &Memory, vm: &Vm, channel: abi::Channel) -> Result<q::Counters> {
    let mut values = [0; 3];
    for (value, va) in values.iter_mut().zip(channel.states) {
        *value = memory.read_firmware32(vm.physical(memory, 2, va)?)?;
    }
    q::Counters::new(values).map_err(|_| EIO)
}
fn render_overlap(client: &Client, address: u64) -> bool {
    client.bindings.iter().any(|&(base, size, _, _)| {
        let base = if base < 0x1000000000 {
            base + 0x1000000000
        } else {
            base
        };
        base < address + PAGE as u64 && address < base + size
    })
}
fn inventory() -> Result<KVec<u64>> {
    let mut addresses = KVec::new();
    for &(base, count, _) in topology::RENDER_RUNS {
        for i in 0..count {
            addresses.push(base + i as u64 * PAGE as u64, GFP_KERNEL)?;
        }
    }
    addresses.extend_from_slice(&opening::EXTRA_RENDER, GFP_KERNEL)?;
    addresses.sort_unstable();
    // RENDER_RUNS is an ownership inventory. Repeated inventory entries are
    // harmless, but join_render requires its canonical sorted set of DVAs.
    let mut unique = KVec::new();
    for va in addresses {
        if unique.last() != Some(&va) {
            unique.push(va, GFP_KERNEL)?;
        }
    }
    Ok(unique)
}

/// Inspect only the bounded source-built graph, across all packed pointer
/// alignments. Both kernel clients already share Vm's firmware-high root.
/// Low mismatches retain the source's explicit three-leaf allow-list. Unknown
/// mismatches reject; an arbitrary report/payload pointer never selects RAM.
pub(crate) fn join_late_references(
    memory: &Memory,
    vm: &Vm,
    compute: &mut Client,
    render: &Client,
) -> Result<usize> {
    let ranges = [
        (0xfffffc20c00000c0, 2 * 0xc0),
        (0xfffffc2000012870, 0x80),
        (0xfffffc20000150e0, 0x80),
        (0xfffffc20c000a870, 0x80),
        (0xfffffc20c000d0e0, 0x80),
        (d::RENDER_DESCRIPTORS[0], r::TA_SIZE),
        (d::RENDER_DESCRIPTORS[1], r::FRAGMENT_SIZE),
        (0xfffffc20c0358000, PAGE),
        (d::COMPUTE_SUPPORT, 0x100),
        (0xfffffc20c06000c0, 2 * 0xc0),
        (0xfffffc20c05e8040, 2 * 0x40),
        (d::POOLS[0], 0x100),
        (d::RENDER_SUPPORT, 0x100),
        (d::POOLS[1], 0x2800),
        (d::SHARED[0], 0x100),
        (d::POOLS[0], 0x2800),
        (d::LAYOUTS[1].context_high, 8 * PAGE),
        (d::LAYOUTS[2].context_high, 8 * PAGE),
    ];
    let mut body = KVVec::with_capacity(8 * PAGE, GFP_KERNEL)?;
    body.resize(8 * PAGE, 0, GFP_KERNEL)?;
    let mut changes: KVec<(u64, u64, u64)> = KVec::new();
    for (owner, size) in ranges {
        for (i, word) in body[..size].chunks_exact_mut(8).enumerate() {
            let pa = vm.physical(memory, 2, owner + i as u64 * 8)?;
            memory.invalidate(pa, 8)?;
            word.copy_from_slice(&memory.read64(pa)?.to_le_bytes());
        }
        for bytes in body[..size].windows(8) {
            let va = u64::from_le_bytes(bytes.try_into().unwrap()) & !(PAGE as u64 - 1);
            if !(0x1000000000..0x20000000000).contains(&va) {
                // A high pointer has one canonical live mapping in Vm; no
                // per-client upper-root cloning or ownership repair is needed.
                continue;
            }
            if owner == 0xfffffc20c0358000 || owner == d::COMPUTE_SUPPORT {
                continue;
            }
            if compute
                .bindings
                .iter()
                .any(|&(base, size, _, _)| base <= va && va < base + size)
            {
                continue;
            }
            let new = render.root.pte(va)?;
            if new == 0 {
                continue;
            }
            let old = compute.root.pte(va)?;
            if old == new {
                continue;
            }
            if ![0x1000080000, 0x1000198000, 0x10001a8000].contains(&va) {
                pr_err!(
                    "G17P: dependency root has unadmitted ownership mismatch at {:#x} from {:#x}\n",
                    va,
                    owner
                );
                return Err(EINVAL);
            }
            if !changes.iter().any(|r| r.0 == va) {
                changes.push((va, old, new), GFP_KERNEL)?;
            }
        }
    }
    let count = changes.len();
    compute.root.rebind(&changes, &[1, 2, 3])?;
    Ok(count)
}

/// The retained bootstrap must have retired before this first wave. This
/// function never runs a caller command to manufacture a bootstrap. A cold
/// backend-owned prologue must be supplied separately by Session.
pub(crate) fn prepare(
    memory: &mut Memory,
    vm: &mut Vm,
    image: &Image,
    compute: &mut compute::Submission,
    render: &mut render::Submission,
    inputs: [&compute::Parameters; 2],
    parameters: &r::Parameters,
    opening_completion: u32,
) -> Result<Prepared> {
    c::completion_header(0, opening_completion).map_err(|_| EINVAL)?;
    if compute.after_render
        || compute.ordinal != 0
        || render.ordinal != 0
        || render.growth.is_some()
        || compute.client.owner != render.client.owner
        || render.client.buffers.is_empty()
    {
        return Err(EINVAL);
    }
    compute::idle(memory, vm, compute)?;
    render::validate_client(&render.client, parameters)?;
    for input in inputs {
        if input.cdm >= 1 << 42
            || input.cdm & (PAGE as u64 - 1) != 0
            || input.end <= input.cdm + 4
            || input.end.checked_sub(input.cdm).ok_or(EINVAL)? > PAGE as u64
            || input.end & 3 != 0
        {
            return Err(EINVAL);
        }
        d::Compute::Opening
            .program(input.cdm, c::USC_EXEC_BASE)
            .map_err(|_| EINVAL)?;
    }
    let channels = [
        compute.channel,
        render.channels[0],
        render.channels[1],
        compute.channel,
    ];
    // Source compute-first startup retains TA2/3D2, although its graph is pair
    // zero. Do not confuse logical queue grids with channel-table indices.
    for (index, table_index) in [(0, q::COMPUTE_CHANNEL), (1, 6), (2, 7)] {
        let expected = image.graph.channels[0][table_index];
        if channels[index].states != expected.states || channels[index].ring != expected.ring {
            return Err(EIO);
        }
    }
    for index in 0..2 {
        for offset in [q::POINTER_DONE, q::POINTER_READ, q::POINTER_WRITE] {
            if memory.read_firmware32(vm.physical(
                memory,
                2,
                render::POINTERS[index] + offset,
            )?)? != 0
            {
                return Err(EBUSY);
            }
        }
        if counters(memory, vm, render.channels[index])?.0 != [0; 3] {
            return Err(EBUSY);
        }
    }
    // The inserted 3D and closing CL contexts are new active private owners.
    for layout in &d::LAYOUTS {
        for offset in (0..8 * PAGE).step_by(PAGE) {
            let va = layout.context_low + offset as u64;
            if render_overlap(&render.client, va)
                || join::Space::caller_overlap(
                    &Space {
                        memory,
                        compute: &mut compute.client,
                        render: &render.client,
                    },
                    va,
                )
            {
                return Err(EINVAL);
            }
        }
    }
    let retained = inventory()?;
    let mut body = KVVec::with_capacity(r::FRAGMENT_SIZE.max(PAGE), GFP_KERNEL)?;
    body.resize(r::FRAGMENT_SIZE.max(PAGE), 0, GFP_KERNEL)?;
    let mut closing_context = KVVec::with_capacity(0x200, GFP_KERNEL)?;
    closing_context.resize(0x200, 0, GFP_KERNEL)?;

    for layout in d::LAYOUTS {
        for (va, size) in [
            (layout.queue, 0xc0),
            (layout.pointers, 0x80),
            (layout.ring, 0x80),
            (layout.context_high, 8 * PAGE),
        ] {
            vm.ensure_firmware(memory, va, size)?;
        }
    }
    for (va, size) in [
        (d::COMPUTE_SUPPORT, PAGE),
        (d::COMPUTE_STATE, PAGE),
        (d::RENDER_SUPPORT, PAGE),
        (d::RENDER_INNER, PAGE),
        (d::POOLS[0], graph::POOL_A_SIZE),
        (d::POOLS[1], graph::POOL_B_SIZE),
        (d::SHARED[0], PAGE),
        (d::SHARED[1], 0x80),
    ] {
        vm.ensure_firmware(memory, va, size)?;
    }
    // The packed render owner advertises a 64 KiB primary-index view
    // (Shared +0x30). Its first page contains the authored group indices;
    // firmware also clears the remaining pages during partial allocation.
    // Preserve existing owners, including the retired primer scheduler page,
    // and supply zero backing for the two absent pages before publication.
    vm.ensure_firmware(memory, d::LEAVES[0], 0x10000)?;
    let leaves = Iterator::chain(d::LEAVES.into_iter(), d::RENDER_STATUS);
    for va in Iterator::chain(
        leaves,
        [
            d::Compute::Opening.completion(),
            d::Compute::Closing.completion(),
        ],
    ) {
        vm.ensure_firmware(memory, va, PAGE)?;
    }
    let mut permissions = KVec::new();
    for va in [
        d::LEAVES[2],
        d::COMPUTE_STATE,
        d::Compute::Opening.completion(),
        d::RENDER_INNER,
        d::RENDER_STATUS[0],
        d::LEAVES[4],
        d::LEAVES[3],
        d::LEAVES[5],
        d::RENDER_STATUS[1],
        d::Compute::Closing.completion(),
    ] {
        let old = vm.pte(memory, 2, va)?;
        permissions.push(
            (2, va, old, (old & ADDRESS) | 0x00c000000000044b),
            GFP_KERNEL,
        )?;
    }
    vm.rebind_pages(memory, &permissions)?;
    // Low status aliases preserve the attributes of their live render leaves.
    let mut statuses = KVec::new();
    for (low, high) in [0x1000080000, 0x10001a8000]
        .into_iter()
        .zip(d::RENDER_STATUS)
    {
        if render_overlap(&render.client, low) {
            return Err(EINVAL);
        }
        let old = render.client.root.pte(low)?;
        if old == 0 {
            return Err(EIO);
        }
        let new = vm.physical(memory, 2, high)? | (old & !ADDRESS);
        statuses.push((low, old, new), GFP_KERNEL)?;
    }
    render.client.root.rebind(&statuses, &[1])?;
    let primary_low = 0x1000198000;
    if render_overlap(&render.client, primary_low) {
        return Err(EINVAL);
    }
    render.client.root.rebind(
        &[(
            primary_low,
            render.client.root.pte(primary_low)?,
            vm.physical(memory, 2, d::LEAVES[0])? | 0x00c0000000000c8b,
        )],
        &[1],
    )?;
    let last = d::LAYOUTS[3];
    let mut closing_aliases = KVec::new();
    for offset in (0..8 * PAGE).step_by(PAGE) {
        let high = last.context_high + offset as u64;
        vm.write(memory, 2, high, &body[..PAGE])?;
        let low = last.context_low + offset as u64;
        closing_aliases.push(
            (
                low,
                render.client.root.pte(low)?,
                vm.physical(memory, 2, high)? | 0x00c000000000044b,
            ),
            GFP_KERNEL,
        )?;
    }
    render.client.root.rebind(&closing_aliases, &[1])?;
    for layout in d::LAYOUTS {
        vm.alias_firmware(memory, layout.context_high, layout.context_low, 8 * PAGE)?;
    }

    // Reset only the already-retired CL outer channel, as the shim does.
    for va in channels[0].states {
        vm.write(memory, 2, va, &[0; 4])?;
    }
    for i in 0..3 {
        let node = d::JOB_LIST + i * 0x18;
        vm.write(memory, 2, node, &q::job_list(node))?;
    }
    for layout in d::LAYOUTS {
        vm.write(memory, 2, layout.pointers, &d::pointers())?;
        vm.write(memory, 2, layout.ring, &[0; 0x80])?;
        vm.write(memory, 2, layout.queue, &layout.record())?;
    }
    for (i, kind) in [
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
        d::leaf(&mut body[..PAGE], kind).map_err(|_| EINVAL)?;
        vm.write(memory, 2, d::LEAVES[i], &body[..PAGE])?;
    }
    body[..PAGE].fill(0);
    for va in [
        d::RENDER_SUPPORT,
        d::SHARED[0],
        d::RENDER_INNER,
        d::RENDER_STATUS[0],
        d::RENDER_STATUS[1],
    ] {
        vm.write(memory, 2, va, &body[..PAGE])?;
    }
    d::render_support(&mut body[..PAGE]).map_err(|_| EINVAL)?;
    vm.write(memory, 2, d::RENDER_SUPPORT, &body[..0x70])?;
    vm.write(memory, 2, d::RENDER_INNER, &1u32.to_le_bytes())?;
    d::pool_a(&mut body[..graph::POOL_A_SIZE]).map_err(|_| EINVAL)?;
    vm.write(memory, 2, d::POOLS[0], &body[..graph::POOL_A_SIZE])?;
    d::pool_b(&mut body[..graph::POOL_B_SIZE]).map_err(|_| EINVAL)?;
    vm.write(memory, 2, d::POOLS[1], &body[..graph::POOL_B_SIZE])?;
    d::shared(&mut body[..0x88]).map_err(|_| EINVAL)?;
    vm.write(memory, 2, d::SHARED[0], &body[..0x88])?;
    vm.write(memory, 2, d::SHARED[1], &[0; 0x80])?;

    let (joined, robustness) = {
        let mut space = Space {
            memory,
            compute: &mut compute.client,
            render: &render.client,
        };
        let joined = join::join_render(&mut space, &retained).map_err(join_error)?;
        let robustness =
            join::join_compute(&mut space, inputs.map(|p| p.cdm)).map_err(join_error)?;
        (joined, robustness)
    };
    d::operand_table(&mut body[..PAGE]).map_err(|_| EINVAL)?;
    // The source updates the render operand table, not the distinct retained
    // compute table. Import leaves preserve this intentional root distinction.
    let operand = render.client.root.pte(d::OPERAND_TABLE)? & ADDRESS;
    if operand == 0 {
        return Err(EIO);
    }
    memory.write(operand, &body[..PAGE])?;
    memory.clean(operand, PAGE)?;
    d::compute_support(&mut body[..PAGE]).map_err(|_| EINVAL)?;
    vm.write(memory, 2, d::COMPUTE_SUPPORT, &body[..PAGE])?;
    cm::shared_state(&mut body[..PAGE], 1).map_err(|_| EINVAL)?;
    vm.write(memory, 2, d::COMPUTE_STATE, &body[..PAGE])?;
    vm.freshen_firmware(memory, &d::FRESH_SCHEDULER_PAGES)?;

    for va in [0xfffffc20c07d0000, 0xfffffc20c07f8000, d::CHANNEL_CONTROL] {
        vm.write(memory, 2, va, &[0; 0x100])?;
    }
    for va in [
        d::Compute::Opening.status()[0],
        d::Compute::Opening.status()[1],
        d::Compute::Closing.status()[0],
        d::Compute::Closing.status()[1],
        0xfffffc2000024c78,
        0xfffffc2000024c80,
    ] {
        vm.write(memory, 2, va, &[0; 8])?;
    }

    let mut publications = KVec::new();
    for index in 0..4 {
        let layout = d::LAYOUTS[index];
        let (optional, descriptor, event, kind) = if index == 0 || index == 3 {
            let command = if index == 0 {
                d::Compute::Opening
            } else {
                d::Compute::Closing
            };
            let input = inputs[command.index()];
            let program = command
                .program(input.cdm, c::USC_EXEC_BASE)
                .map_err(|_| EINVAL)?;
            let end = if index == 0 {
                0x100000b0000 + input.end - input.cdm - 4
            } else {
                input.end - 4
            };
            command
                .descriptor_body(
                    &mut body[..PAGE],
                    &program,
                    end,
                    input.sampler,
                    input.sampler_count,
                    input.timestamps,
                )
                .map_err(|_| EINVAL)?;
            vm.ensure_firmware(memory, command.descriptor(), PAGE)?;
            vm.write(memory, 2, command.descriptor(), &body[..0x1000])?;
            vm.alias_firmware(memory, command.descriptor(), command.low(), 0x1000)?;
            vm.write(memory, 2, command.optional(), &command.optional_body())?;
            command
                .context_body(&mut closing_context, opening_completion)
                .map_err(|_| EINVAL)?;
            if index == 0 {
                vm.write(memory, 2, layout.context_high + 0x200, &closing_context)?;
            }
            vm.write(memory, 2, command.completion(), &[0; 0x40])?;
            (
                command.optional(),
                command.descriptor(),
                command.event(),
                q::Kind::Compute,
            )
        } else {
            let kind = if index == 1 {
                Kind::Tiling
            } else {
                Kind::Fragment
            };
            let i = kind.index() as usize;
            let p = d::render_parameters(parameters);
            d::render_descriptor(&mut body[..kind.size()], kind, &p).map_err(|_| EINVAL)?;
            vm.ensure_firmware(memory, d::RENDER_DESCRIPTORS[i], kind.size())?;
            vm.write(memory, 2, d::RENDER_DESCRIPTORS[i], &body[..kind.size()])?;
            d::render_optional(&mut body[..0xc0], kind).map_err(|_| EINVAL)?;
            vm.write(memory, 2, d::RENDER_OPTIONALS[i], &body[..0xc0])?;
            d::render_context(&mut body[..graph::CONTEXT_SIZE], kind, opening_completion)
                .map_err(|_| EINVAL)?;
            vm.write(
                memory,
                2,
                layout.context_high + 0x200,
                &body[..graph::CONTEXT_SIZE],
            )?;
            (
                d::RENDER_OPTIONALS[i],
                d::RENDER_DESCRIPTORS[i],
                d::RENDER_EVENTS[i],
                if i == 0 {
                    q::Kind::Tiling
                } else {
                    q::Kind::Fragment
                },
            )
        };
        let publication = q::Stage {
            queue: layout.queue,
            pointers: layout.pointers,
            item_ring: layout.ring,
            item_capacity: 0x500,
            write_index: 0,
            channel_ring: channels[index].ring,
            channel_producer: channels[index].states[2],
            counters: q::Counters([0; 3]),
            slot: Some(if index == 3 { 1 } else { 0 }),
            items: &[descriptor, optional, event],
            group: 1,
            grid: layout.grid,
            kind,
            first: true,
            in_place: false,
            announce: false,
            defer_inner: index == 3,
            defer_outer: true,
            event_subtype: None,
            event_counter: Some(0x102),
            event_counter_low: 2,
        }
        .publish(&mut Writer { memory, vm })
        .map_err(|error| match error {
            q::StageError::Access(error) => error,
            q::StageError::Protocol(_) => EINVAL,
        })?;
        publications.push(publication, GFP_KERNEL)?;
    }
    let late_aliases = join_late_references(memory, vm, &mut compute.client, &render.client)?;
    // Descriptor construction is over. Restore the exact native scheduler
    // before-images and cold controls immediately before root selection.
    for (i, (record, slot, _)) in d::SCHEDULERS.into_iter().enumerate() {
        vm.write(memory, 2, record, &d::scheduler(i).map_err(|_| EINVAL)?)?;
        vm.write(
            memory,
            2,
            slot,
            &(if i == 1 { 2u32 } else { 1 }).to_le_bytes(),
        )?;
        vm.write(
            memory,
            2,
            d::CHANNEL_CONTROL + i as u64 * 0x40,
            &d::channel_control(),
        )?;
    }
    vm.flush_tables(memory)?;
    compute.client.cache(false)?;
    render.client.cache(false)?;
    g17p_memory::sync();
    Ok(Prepared {
        publications: [
            publications[0],
            publications[1],
            publications[2],
            publications[3],
        ],
        channels,
        root: compute.client.root.root(),
        closing_context,
        robustness,
        joined,
        late_aliases,
        compute_timestamps: inputs.map(|p| p.timestamps),
        render_timestamps: [
            parameters.ta_user_timestamp_start,
            parameters.ta_user_timestamp_end,
            parameters.fragment_user_timestamp_start,
            parameters.fragment_user_timestamp_end,
        ],
    })
}
