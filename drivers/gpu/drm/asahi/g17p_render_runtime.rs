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
// Full-page clears must not consume the ARM64 kernel's 32 KiB task stack.
static ZERO_PAGE: [u8; PAGE] = [0; PAGE];
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
        // Mesa owns its fixed render-state BO through +0x6c000. Keep
        // tiler scratch in the already-owned source page above that BO.
        deflake_1: 0x10000702a0,
        deflake_2: 0x1000070020,
        deflake_3: 0x1000070000,
        // Auxiliary metadata must also stay outside Mesa's fixed USC arena.
        aux_fb: 0x1000074000,
        reactive_tvb_growth: true,
        emit_uapi_fields: true,
        ..Default::default()
    }
}

// Exact logical geometry identity, never just allocation counts.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ScratchKey([u64; 10]);
impl ScratchKey {
    fn of(p: &Parameters) -> Self {
        Self([p.context_base, p.width, p.height, p.layers, p.utile_width, p.utile_height,
              p.samples, p.utile_config, p.tib_blocks, p.tile_config])
    }
}
struct ScratchLease {
    key: ScratchKey,
    tilemap: u64,
    tpc: u64,
    pair_stride: u64,
    span: u64,
    pages: KVec<(u64, u64)>,
}
struct Deferred {
    address: u64,
    body: KVVec<u8>,
}
fn pool_count() -> u64 {
    if *crate::module_parameters::partial_independent_owner.value() != 1 { 1 }
    else if *crate::module_parameters::alternate_queue_pairs.value() != 1
        || *crate::module_parameters::native_render_vms.value() == 1 { 2 }
    else { life::POOL_SLOTS as u64 }
}
fn transport_pools(pair: u32) -> [super::g17p_compute_runtime::TransportPool; 2] {
    let base = if pair < 2 { 0xfffffc20cf010000 + pair as u64 * 0x20000 }
        else { 0xfffffc20d00d0000 + (pair as u64 - 2) * 0x200000 };
    [super::g17p_compute_runtime::TransportPool::new(base),
     super::g17p_compute_runtime::TransportPool::new(base + 0x10000)]
}
struct Transport {
    layout: life::Layout,
    item_index: u32,
    publications: [q::Publication; 2],
    deferred: [KVec<Deferred>; 2],
    transports: [super::g17p_compute_runtime::TransportPool; 2],
    fresh: bool,
}
/// Short-lock seed for CPU-only preparation. It owns the exact immutable
/// caller identities and executable root pages; construction does not read
/// firmware, change PTEs, clear scratch or publish any queue/control producer.
pub(crate) struct PreparationSeed {
    item: life::Item,
    parameters: Parameters,
    client: super::g17p_compute_runtime::ClientLease,
    bindings: KVec<(u64, u64, u64, u32)>,
}
pub(crate) struct PreparedAppend {
    seed: KBox<PreparationSeed>,
    objects: [KVec<Deferred>; 2],
}
impl PreparationSeed {
    /// Execute after dropping the runtime mutex; only Source serialization
    /// and host allocations run here. No hardware or cache operation occurs.
    pub(crate) fn prepare(seed: KBox<Self>) -> Result<KBox<PreparedAppend>> {
        let objects = build_host_objects(seed.item, &seed.parameters)?;
        Ok(KBox::new(PreparedAppend { seed, objects }, GFP_KERNEL)?)
    }
}
fn build_host_objects(item: life::Item, p: &Parameters) -> Result<[KVec<Deferred>; 2]> {
    let mut objects = [KVec::new(), KVec::new()];
    for (index, kind) in [Kind::Tiling, Kind::Fragment].into_iter().enumerate() {
        for (address, size, ty) in [
            (item.descriptor_address(kind), kind.size(), 0),
            (item.optional_address(kind), 0xc0, 1),
            (item.context_address(kind), graph::CONTEXT_SIZE, 2),
        ] {
            let mut body = KVVec::with_capacity(size, GFP_KERNEL)?;
            body.resize(size, 0, GFP_KERNEL)?;
            match ty {
                0 => item.descriptor(kind, p, &mut body),
                1 => item.optional(kind, &mut body),
                _ => item.context(kind, p, &mut body),
            }.map_err(|_| EINVAL)?;
            objects[index].push(Deferred { address, body }, GFP_KERNEL)?;
        }
    }
    Ok(objects)
}
impl PreparedAppend {
    fn matches(&self, work: &Submission, item: life::Item, p: &Parameters) -> bool {
        self.seed.item == item && self.seed.parameters == *p
            && self.seed.client.owner == work.client.owner
            && self.seed.client.root.root() == work.client.root.root()
            && self.seed.bindings.as_slice() == work.client.bindings.as_slice()
            && self.seed.client.matches_buffers(&work.client)
    }
}

/// Source queue-context dependency point. Host/public fences own completion;
/// these values identify firmware publication barriers without a CPU wait.
#[derive(Clone, Copy)]
pub(crate) struct Milestone {
    pub(crate) event_slot: u8,
    pub(crate) grid: u8,
    pub(crate) value: u32,
}
pub(crate) fn milestones(item: life::Item) -> Result<[Milestone; 2]> {
    if item.layout.native {
        return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
    }
    let event_slot: u8 = item.layout.pair.try_into().map_err(|_| EINVAL)?;
    if u32::from(event_slot) >= life::POOL_SLOTS { return Err(EINVAL); }
    let value = item.index.checked_add(1).ok_or(EOVERFLOW)?;
    if value >= (1 << 30) { return Err(EOVERFLOW); }
    let mut result = [Milestone { event_slot, grid: 0, value }; 2];
    for (milestone, grid) in result.iter_mut().zip(item.layout.grids) {
        milestone.grid = grid.try_into().map_err(|_| EINVAL)?;
    }
    Ok(result)
}

/// Completion and resource identity captured before publishing a render.
/// Later producer preparation must not change what this ticket retires.
pub(crate) struct Ticket {
    pub(crate) item: life::Item,
    pub(crate) publications: [q::Publication; 2],
    pub(crate) channels: [abi::Channel; 2],
    pub(crate) statuses: [u64; 2],
    pub(crate) timestamps: [u64; 4],
    pub(crate) client: super::g17p_compute_runtime::ClientLease,
    pub(crate) growth: Option<super::g17p_growth_runtime::WorkToken>,
    pub(crate) firmware_completion: u64,
}
impl Ticket {
    pub(crate) fn capture(work: &Submission, ordinary_growth: bool) -> Result<Self> {
        let item = life::Item { ordinal: work.ordinal, storage: work.storage, index: work.item_index, layout: work.layout };
        let mut statuses = [0; 2];
        for (status, base) in statuses.iter_mut().zip(work.layout.status) {
            *status = base.checked_add((work.item_index % life::STORAGE_SUBMISSIONS) as u64 * 0x40)
                .ok_or(EOVERFLOW)?;
        }
        Ok(Self { item, publications: work.publications, channels: work.channels,
            statuses, timestamps: work.timestamps, firmware_completion: work.firmware_completion, client: work.client.lease()?,
            growth: if ordinary_growth {
                Some(work.growth.as_ref().ok_or(EIO)?.work_token()?)
            } else { None } })
    }
}

pub(crate) struct Submission {
    pub(crate) client: Client,
    pub(crate) publications: [q::Publication; 2],
    pub(crate) channels: [abi::Channel; 2],
    pub(crate) timestamps: [u64; 4],
    pub(crate) growth: Option<super::g17p_growth_runtime::Service>,
    pub(crate) ordinal: u32,
    pub(crate) storage: u32,
    pub(crate) next_storage: Option<u32>,
    pub(crate) item_index: u32,
    pub(crate) layout: life::Layout,
    deferred: [KVec<Deferred>; 2],
    transports: [super::g17p_compute_runtime::TransportPool; 2],
    empty_high: [u64; 2],
    adopted: bool,
    others: KVec<Transport>,
    fresh_pair: bool,
    scratch_layout: (u64, u64, u64),
    scratch_key: ScratchKey,
    scratch_leases: KVec<ScratchLease>,
    scratch_cursor: u64,
    firmware_timestamps: Option<u64>,
    firmware_completion: u64,
}
impl Submission {
    pub(crate) fn same_geometry(&self, p: &Parameters) -> bool {
        self.scratch_key == ScratchKey::of(p)
    }
    fn ticket_parameters(&self, storage: u32, p: Parameters) -> Parameters {
        let Some(base) = self.firmware_timestamps else { return p; };
        // Finite descriptor ownership has the same 256-item storage cycle.
        // The physical slot lease must retire before its timestamp is reused.
        // CPU Vm::write performs whole64-byte line maintenance. Never
        // clear/clean a line containing another live firmware timestamp.
        let a = base + storage as u64 * 64;
        Parameters { timestamp_a: a, timestamp_b: a + 8,
            ta_timestamp_end: a + 8, fragment_timestamp_start: a,
            fragment_timestamp_end: a + 8, ..p }
    }
    pub(crate) fn preparation_seed(&self, p: &Parameters, ahead: u32, pair: u32, storage: u32) -> Result<Option<KBox<PreparationSeed>>> {
        if storage >= life::STORAGE_SUBMISSIONS { return Err(EINVAL); }
        if ahead == 0 { return Err(EINVAL); }
        if ahead != 1 { return Ok(None); }
        if self.layout.native { return Ok(None); }
        let mut p = p.with_geometry_scratch().map_err(|_| EINVAL)?;
        let (_, stride, tpc) = p.scratch_layout().map_err(|_| EINVAL)?;
        if pool_count() > 2 || stride.checked_mul(8).ok_or(EOVERFLOW)? > 0x24000 || tpc > PAGE as u64 {
            let key = ScratchKey::of(&p);
            let Some(lease) = self.scratch_leases.iter().find(|lease| lease.key == key) else {
                return Ok(None); // New private geometry requires locked allocation.
            };
            p = Parameters { tilemap: lease.tilemap, tpc: lease.tpc,
                scratch_pair_stride: lease.pair_stride, ..p }
                .with_geometry_scratch().map_err(|_| EINVAL)?;
        }
        let ordinal = self.ordinal.checked_add(ahead).ok_or(EOVERFLOW)?;
        let (layout, index) = if self.layout.pair != pair {
            let Some(other) = self.others.iter().find(|other| other.layout.pair == pair) else { return Ok(None); };
            (other.layout, if other.fresh { 0 } else { other.item_index.checked_add(1).ok_or(EOVERFLOW)? })
        } else {
            (self.layout, if self.fresh_pair { 0 } else { self.item_index.checked_add(1).ok_or(EOVERFLOW)? })
        };
        let mut item = life::Item::retained(ordinal, index, layout).map_err(|_| EINVAL)?;
        item.storage = storage;
        p = self.ticket_parameters(item.storage, p);
        let mut bindings = KVec::new();
        bindings.extend_from_slice(&self.client.bindings, GFP_KERNEL)?;
        Ok(Some(KBox::new(PreparationSeed { item, parameters: p, client: self.client.lease()?, bindings }, GFP_KERNEL)?))
    }
    pub(crate) fn set_priority(&self, memory: &mut Memory, vm: &Vm, priority: u32) -> Result {
        let profile = q::priority_profile(priority).map_err(|_| EINVAL)?;
        for pointers in self.layout.pointers {
            let mut values = [0; 3];
            for (value, offset) in
                values
                    .iter_mut()
                    .zip([q::POINTER_DONE, q::POINTER_READ, q::POINTER_WRITE])
            {
                *value = memory.read_firmware32(vm.physical(memory, 2, pointers + offset)?)?;
            }
            if values[0] != values[1] || values[1] != values[2] {
                return Err(EBUSY);
            }
        }
        // Resolve both host-owned families before changing either queue.
        let mut addresses = [0; 2];
        for (address, queue) in addresses.iter_mut().zip(self.layout.queues) {
            *address = vm.physical(memory, 2, queue + 0x28)?;
        }
        for address in addresses {
            // The priority family shares cache lines with firmware-owned
            // queue fields. Match Vm::write: discard the CPU clean copy
            // before the partial store so a later clean preserves those
            // adjacent fields after firmware has advanced them.
            memory.invalidate(address, profile.len())?;
            memory.write(address, &profile)?;
            memory.clean(address, profile.len())?;
        }
        g17p_memory::sync();
        Ok(())
    }
}

/// quiesce_submission(semantic_complete=True), for the sole synchronous owner.
/// Call only after its queues, statuses and new terminal have all completed.
pub(crate) fn complete_leaf_publication(memory: &mut Memory, vm: &Vm, work: &Submission) -> Result {
    // _complete_native_leaf_publication, enabled by G17P_DEFAULTS. Retain
    // the created pair's first retired shared-slot value before list cleanup.
    complete_ticket_leaf_publication(memory, vm, life::Item {
        ordinal: work.ordinal, storage: work.storage, index: work.item_index, layout: work.layout,
    })
}
pub(crate) fn complete_ticket_leaf_publication(memory: &mut Memory, vm: &Vm, item: life::Item) -> Result {
    if let Some(address) = item.retirement_leaf() {
        vm.write(memory, 2, address, &0x13u32.to_le_bytes())?;
        g17p_memory::sync();
    }
    Ok(())
}
pub(crate) fn quiesce(memory: &mut Memory, vm: &Vm, work: &Submission) -> Result<bool> {
    for stage in 0..2 {
        let target = work.publications[stage].write_after;
        for offset in [q::POINTER_DONE, q::POINTER_READ, q::POINTER_WRITE] {
            if memory.read_firmware32(vm.physical(
                memory,
                2,
                work.layout.pointers[stage] + offset,
            )?)? != target
            {
                return Err(EBUSY);
            }
        }
    }
    let tail = vm.physical(memory, 2, work.layout.job_list + 8)?;
    memory.invalidate(tail, 8)?;
    if memory.read64(tail)? == work.layout.job_list {
        return Ok(false);
    }
    // Both halves name this one retained head. Never clear pool records or
    // neighboring list nodes; Python resets only the 0x18-byte list header.
    vm.write(
        memory,
        2,
        work.layout.job_list,
        &q::job_list(work.layout.job_list),
    )?;
    g17p_memory::sync();
    memory.invalidate(tail, 8)?;
    if memory.read64(tail)? != work.layout.job_list {
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
    if *crate::module_parameters::partial_independent_owner.value() == 1 {
        for (address, size) in super::g17p_partial_runtime::private_ranges() {
            if overlap(client, address, size as u64) {
                return Err(EINVAL);
            }
        }
        for address in super::g17p_partial_runtime::alias_pages() {
            if overlap(client, address, PAGE as u64) {
                return Err(EINVAL);
            }
        }
    }
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

fn align_page(n: u64) -> Result<u64> {
    Ok(n.checked_add(PAGE as u64 - 1).ok_or(EOVERFLOW)? & !(PAGE as u64 - 1))
}
// Selection occurs under the serialized render owner before any publication.
// New pages and old generations stay in Memory through session shutdown.
fn select_scratch(memory: &mut Memory, client: &mut Client,
                  leases: &mut KVec<ScratchLease>, cursor: &mut u64,
                  p: &Parameters) -> Result<Parameters> {
    p.validate().map_err(|_| EINVAL)?;
    let (_, stride, tpc) = p.scratch_layout().map_err(|_| EINVAL)?;
    let ring = stride.checked_mul(8).ok_or(EOVERFLOW)?;
    if pool_count() <= 2 && ring <= 0x24000 && tpc <= PAGE as u64 {
        return p.with_geometry_scratch().map_err(|_| EINVAL);
    }
    let key = ScratchKey::of(p);
    let i = if let Some(i) = leases.iter().position(|lease| lease.key == key) { i } else {
        let tilemap_size = align_page(ring)?;
        let tpc_size = align_page(tpc)?;
        let pair_stride = tilemap_size.checked_add(tpc_size).ok_or(EOVERFLOW)?;
        // Both independent queue-pair namespaces have disjoint region/TPC pages.
        let span = pair_stride.checked_mul(pool_count()).ok_or(EOVERFLOW)?;
        let base = align_page(*cursor)?;
        let end = base.checked_add(span).ok_or(EOVERFLOW)?;
        // TA compact addresses cover exactly the source context's 4GB aperture.
        if base < p.context_base || end > p.context_base.checked_add(1 << 32).ok_or(EOVERFLOW)? {
            return Err(ENOMEM);
        }
        if overlap(client, base, span) { return Err(EINVAL); }
        let pages_count = usize::try_from(span / PAGE as u64).map_err(|_| EOVERFLOW)?;
        let mut pages = KVec::with_capacity(pages_count, GFP_KERNEL)?;
        leases.reserve(1, GFP_KERNEL)?;
        // Allocate all backing before any PTE is visible. On failure Memory
        // still retains acquired pages; no queue or command was published.
        for offset in (0..span).step_by(PAGE) {
            let pa = memory.allocate(PAGE)?;
            memory.clean(pa, PAGE)?;
            pages.push((base + offset, pa), GFP_KERNEL)?;
        }
        leases.push(ScratchLease { key, tilemap: base,
            tpc: base + tilemap_size, pair_stride, span, pages }, GFP_KERNEL)?;
        *cursor = end;
        leases.len() - 1
    };
    let lease = &leases[i];
    if overlap(client, lease.tilemap, lease.span) { return Err(EINVAL); }
    // An older logical owner may predate this lease. Mirror only absent leaves;
    // refuse every mismatched physical identity or attribute before publication.
    let mut absent = KVec::new();
    for &(va, pa) in &lease.pages {
        let pte = client.root.pte(va)?;
        if pte == 0 { absent.push((va, pa), GFP_KERNEL)?; }
        else if pte != pa | 0x00c0000000000c8b { return Err(EBUSY); }
    }
    if !absent.is_empty() {
        client.root.grow(&absent)?;
        Vm::invalidate_gpu();
    }
    Parameters { tilemap: lease.tilemap, tpc: lease.tpc,
        scratch_pair_stride: lease.pair_stride, ..*p }
        .with_geometry_scratch().map_err(|_| EINVAL)
}
fn validate_scratch_backing(client: &Client, p: &Parameters) -> Result {
    let (_, stride, tpc) = p.scratch_layout().map_err(|_| EINVAL)?;
    let ring = stride.checked_mul(8).ok_or(EOVERFLOW)?;
    let pairs = pool_count();
    for pair in 0..pairs {
        let delta = pair.checked_mul(p.scratch_pair_stride).ok_or(EOVERFLOW)?;
        for (base, len) in [(p.tilemap, ring), (p.tpc, tpc)] {
            let base = base.checked_add(delta).ok_or(EOVERFLOW)?;
            let end = base.checked_add(len).ok_or(EOVERFLOW)?;
            let mut at = base & !(PAGE as u64 - 1);
            while at < end {
                if client.root.pte(at)? & 3 != 3 { return Err(ENOMEM); }
                at = at.checked_add(PAGE as u64).ok_or(EOVERFLOW)?;
            }
        }
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
    let configured = p.with_geometry_scratch().map_err(|_| EINVAL)?;
    let p = &configured;
    validate_client(&client, p)?;
    if *crate::module_parameters::partial_independent_owner.value() == 1 {
        for (base, size) in super::g17p_partial_runtime::private_ranges() {
            let pa = memory.allocate(size)?;
            if base == 0x1001004000 {
                let mut body = KVVec::with_capacity(PAGE, GFP_KERNEL)?;
                body.resize(PAGE, 0, GFP_KERNEL)?;
                r::aux_fb(&mut body).map_err(|_| EINVAL)?;
                memory.write(pa, &body)?;
            }
            memory.clean(pa, size)?;
            client.root.prepare(base, size as u64)?;
            for offset in (0..size).step_by(PAGE) {
                client
                    .root
                    .map_page(base + offset as u64, pa + offset as u64, true)?;
            }
        }
    }
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
    let mut scratch_leases = KVec::new();
    let mut scratch_cursor = super::g17p_growth::GROWTH_END;
    let configured = select_scratch(memory, &mut client, &mut scratch_leases,
        &mut scratch_cursor, p)?;
    let p = &configured;
    validate_scratch_backing(&client, p)?;
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
        128 * r::TA_SIZE,
    )?;
    vm.alias_firmware(
        memory,
        DESCRIPTORS[1],
        0x7000098000,
        128 * r::FRAGMENT_SIZE,
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
        storage: 0,
        next_storage: None,
        item_index: 0,
        layout: life::ORDINARY,
        adopted: false,
        others: KVec::new(),
        transports: transport_pools(0),
        fresh_pair: false,
        scratch_layout: p.scratch_layout().map_err(|_| EINVAL)?,
        scratch_key: ScratchKey::of(p),
        scratch_leases,
        scratch_cursor,
        firmware_timestamps: None,
        firmware_completion: FW_TIMESTAMPS[1],
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
    pub(crate) fn create_second_pair(&mut self, memory: &mut Memory, vm: &mut Vm) -> Result {
        if self.others.len() >= 1 {
            return Ok(());
        }
        if self.layout.native || self.layout.pair != 0 || self.growth.is_none() {
            return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
        }
        // G17PFirstRender reserved the inventory before the first publication.
        for (base, size) in super::g17p_partial_runtime::private_ranges() {
            for offset in (0..size).step_by(PAGE) {
                let pte = self.client.root.pte(base + offset as u64)?;
                if pte & 3 != 3 {
                    return Err(EIO);
                }
                memory.word64(pte & 0x000003ffffffc000)?;
            }
        }
        // Fresh owned firmware backing, collision checked by the same Source
        // allocator as transport banks. No existing captured data is adopted.
        if self.firmware_timestamps.is_none() {
            let base = 0xfffffc20cf050000;
            vm.transport_backing(memory, base)?;
            self.firmware_timestamps = Some(base);
        }
        // Compute startup owns the legacy SECOND Pool-B/status/shared-slot
        // templates. Preflight/allocate fresh exact firmware mappings, retaining
        // all old physical backing. Reject any existing private owner; never
        // remap its leaves or infer ownership from an allocation count.
        for (address, attributes) in life::SECOND_OWNED_STORAGE.into_iter()
            .zip(life::SECOND_OWNED_ATTRIBUTES) {
            vm.transport_backing_with_attributes(memory, address, attributes)?;
        }
        let layout = life::SECOND;
        for index in 0..2 {
            for (address, size) in [
                (layout.queues[index], 0xc0),
                (layout.pointers[index], 0x80),
                (layout.rings[index], PAGE),
                (layout.status[index], PAGE),
            ] {
                vm.ensure_firmware(memory, address, size)?;
            }
            let (high, low) = layout.contexts[index];
            vm.alias_firmware(memory, high, low, 8 * PAGE)?;
            let mut page = KVVec::with_capacity(PAGE, GFP_KERNEL)?;
            page.resize(PAGE, 0, GFP_KERNEL)?;
            graph::Context {
                kind: if index == 0 {
                    Kind::Tiling
                } else {
                    Kind::Fragment
                },
                descriptor: 0,
                queue: 0,
                pair: 1,
                item: 0,
                context: None,
                grid: None,
                locator_context: None,
                partial_opening: false,
                dependency_grid: None,
                points: None,
                event_slot: None,
                completion: None,
            }
            .build(&mut page[0x200..0x200 + graph::CONTEXT_SIZE])
            .map_err(|_| EINVAL)?;
            vm.write(memory, 2, high, &page)?;
            for offset in (PAGE..8 * PAGE).step_by(PAGE) {
                vm.write(memory, 2, high + offset as u64, &ZERO_PAGE)?;
            }
            // Root one owns the existing GPU operand pages at these DVAs.
            // Firmware queue contexts have separate backing; keep the root-zero
            // companion aliases without replacing the render operand owner.
            let status_low = [0x1000230000, 0x1000358000][index];
            let pa = vm.physical(memory, 2, layout.status[index])?;
            self.client.root.prepare(status_low, PAGE as u64)?;
            self.client.root.map_page(status_low, pa, true)?;
            vm.write(memory, 2, layout.status[index], &ZERO_PAGE)?;
            let mut pointers = [0; 0x80];
            pointers[..0x60].copy_from_slice(&q::pointers(u32::MAX));
            c::u32_at(&mut pointers, 0x60, 0x500);
            vm.write(memory, 2, layout.pointers[index], &pointers)?;
            vm.write(memory, 2, layout.rings[index], &ZERO_PAGE)?;
            let mut record = q::Record {
                pointers: layout.pointers[index],
                ring: layout.rings[index],
                job_list: layout.job_list,
                context: layout.control,
                uuid: layout.uuid as u32,
                priority: 0,
                prio5: 1,
                unk_2c: 0,
                unk_38: 1,
                unk_30: None,
                unk_94: 0,
                sentinel_size: 6,
            }
            .build()
            .map_err(|_| EINVAL)?;
            for (at, value) in [
                (0x20, 0xffffffff00000000),
                (0x30, 0xffffffffffff0000),
                (0x38, 1),
                (0x40, 0xffffffff00000001),
            ] {
                c::u64_at(&mut record, at, value);
            }
            vm.write(memory, 2, layout.queues[index], &record)?;
        }
        vm.write(memory, 2, layout.job_list, &q::job_list(layout.job_list))?;
        super::g17p_partial_runtime::build_graph(
            memory,
            vm,
            &mut self.client.root,
            self.layout,
            layout.support,
        )?;
        // Both entries share a CPU cacheline with firmware counters. Seed
        // their immutable lowindex/count inputs before either render producer
        // is exposed, not by a dirty partial-line store during another pool's
        // execution. The selected-pool stage refresh then only changes data
        // if it differs; ordinary eight-group ownership remains unchanged.
        for owner in [self.layout, layout] {
            let count = memory.read_firmware32(vm.physical(memory, 2, owner.shared[0] + 0x34)?)?;
            let pa = vm.physical(memory, 2, owner.shared[0] + 0x28)?;
            memory.invalidate(pa, 8)?;
            if let Some(body) = life::index_registration(memory.read64(pa)?, count).map_err(|_| EIO)? {
                vm.write(memory, 2, 0xfffffc20015e0000 + owner.pair as u64 * 0x10, &body)?;
            }
        }
        self.growth.as_mut().ok_or(EINVAL)?.register_pool(
            memory,
            vm,
            &self.client.root,
            layout.leaves[4],
            layout.leaves[1],
            layout.shared[0],
            super::g17p_growth::REQUEST_LIMIT,
            layout,
        )?;
        // Later geometry allocations cannot occupy the selected pool's
        // unpublished growth tranche. Its pages are allocated on demand.
        self.scratch_cursor = self.scratch_cursor.max(
            self.growth.as_ref().ok_or(EINVAL)?.reserved_growth_end()?);
        vm.flush_tables(memory)?;
        super::g17p_user_vm::UserVm::invalidate(1);
        let empty = q::Publication {
            slot: 0,
            producer: 0,
            consumers_before: [0; 2],
            write_before: 0,
            write_after: 0,
            deferred_inner: None,
            deferred_outer: None,
        };
        self.others.push(Transport {
            layout,
            item_index: 0,
            publications: [empty; 2],
            deferred: [KVec::new(), KVec::new()],
            transports: transport_pools(layout.pair),
            fresh: true,
        }, GFP_KERNEL)?;
        self.layout.independent = true;
        Ok(())
    }
    pub(crate) fn pool_count(&self) -> u32 { self.others.len() as u32 + 1 }
    /// Construct all representable ordinary owners before the first producer.
    /// This also seeds immutable registration fields while their neighboring
    /// firmware counters are still quiescent. Physical backing is retained.
    pub(crate) fn create_render_pools(&mut self, memory: &mut Memory, vm: &mut Vm) -> Result {
        self.create_second_pair(memory, vm)?;
        for pool in self.pool_count()..life::POOL_SLOTS {
            self.create_extra_pool(memory, vm, pool)?;
        }
        Ok(())
    }
    fn create_extra_pool(&mut self, memory: &mut Memory, vm: &mut Vm, pair: u32) -> Result {
        if self.ordinal != 0 || self.layout.pair != 0 || self.pool_count() != pair { return Err(EBUSY); }
        let layout = life::extra_pool(pair).map_err(|_| EINVAL)?;
        let delta = life::private_delta(pair);
        self.others.reserve(1, GFP_KERNEL)?;
        // Fresh low private inventory. These are ordinary source-built GPU
        // operands, never aliases of firmware queue-context memory.
        for (base, size) in Iterator::chain(super::g17p_partial_runtime::private_ranges().into_iter(),
            [(0x1000080000, PAGE)]) {
            let base = base + delta;
            if overlap(&self.client, base, size as u64) { return Err(EBUSY); }
            for offset in (0..size).step_by(PAGE) {
                if self.client.root.pte(base + offset as u64)? != 0 { return Err(EBUSY); }
            }
            let pa = memory.allocate(size)?;
            if base == 0x1001004000 + delta {
                let mut body = KVVec::with_capacity(PAGE, GFP_KERNEL)?;
                body.resize(PAGE, 0, GFP_KERNEL)?;
                r::aux_fb(&mut body).map_err(|_| EINVAL)?;
                memory.write(pa, &body)?;
            }
            memory.clean(pa, size)?;
            self.client.root.prepare(base, size as u64)?;
            for offset in (0..size).step_by(PAGE) {
                self.client.root.map_page(base + offset as u64, pa + offset as u64, true)?;
            }
        }
        for base in super::g17p_partial_runtime::alias_pages() {
            if overlap(&self.client, base + delta, PAGE as u64)
                || self.client.root.pte(base + delta)? != 0 { return Err(EBUSY); }
        }
        let arena = layout.queues[0];
        // Same AttrIndex as each original object. Reserve exact disjoint
        // spans; the allocator refuses existing mappings instead of adopting.
        for offset in (0..0xd0000).step_by(0x8000) {
            let normal = matches!(offset, 0 | 0x18000 | 0x20000 | 0x40000 | 0x48000 |
                0x50000 | 0x58000 | 0x60000 | 0x88000);
            vm.transport_backing_with_attributes(memory, arena + offset,
                if normal { 0x00c0000000000443 } else { 0x00c000000000044b })?;
        }
        for index in 0..2 {
            let (high, low) = layout.contexts[index];
            vm.alias_firmware(memory, high, low, 8 * PAGE)?;
            let status_low = [0x1000230000, 0x1000358000][index] + delta;
            let pa = vm.physical(memory, 2, layout.status[index])?;
            self.client.root.prepare(status_low, PAGE as u64)?;
            self.client.root.map_page(status_low, pa, true)?;
            let mut pointers = [0; 0x80];
            pointers[..0x60].copy_from_slice(&q::pointers(u32::MAX));
            c::u32_at(&mut pointers, 0x60, 0x500);
            vm.write(memory, 2, layout.pointers[index], &pointers)?;
            let mut record = q::Record {
                pointers: layout.pointers[index], ring: layout.rings[index],
                job_list: layout.job_list, context: layout.control,
                uuid: layout.uuid as u32, priority: 0, prio5: 1, unk_2c: 0,
                unk_38: 1, unk_30: None, unk_94: 0, sentinel_size: 6,
            }.build().map_err(|_| EINVAL)?;
            for (at, value) in [(0x20, 0xffffffff00000000), (0x30, 0xffffffffffff0000),
                (0x38, 1), (0x40, 0xffffffff00000001)] { c::u64_at(&mut record, at, value); }
            vm.write(memory, 2, layout.queues[index], &record)?;
        }
        vm.write(memory, 2, layout.job_list, &q::job_list(layout.job_list))?;
        super::g17p_partial_runtime::build_pool_graph(memory, vm, &mut self.client.root,
            self.layout, layout, delta, layout.support)?;
        let body = life::index_registration(0x1000340000 + delta, 32)
            .map_err(|_| EINVAL)?.ok_or(EINVAL)?;
        vm.write(memory, 2, 0xfffffc20015e0000 + pair as u64 * 0x10, &body)?;
        self.growth.as_mut().ok_or(EINVAL)?.register_pool(memory, vm, &self.client.root,
            layout.leaves[4], layout.leaves[1], layout.shared[0],
            super::g17p_growth::REQUEST_LIMIT, layout)?;
        self.scratch_cursor = self.scratch_cursor.max(self.growth.as_ref().ok_or(EINVAL)?.reserved_growth_end()?);
        vm.flush_tables(memory)?;
        super::g17p_user_vm::UserVm::invalidate(1);
        let empty = q::Publication { slot: 0, producer: 0, consumers_before: [0; 2],
            write_before: 0, write_after: 0, deferred_inner: None, deferred_outer: None };
        self.others.push(Transport { layout, item_index: 0, publications: [empty; 2],
            deferred: [KVec::new(), KVec::new()], transports: transport_pools(pair), fresh: true }, GFP_KERNEL)?;
        pr_info!("G17P: independent render pool {} grids {:?} firmware {:#x} private {:#x}\n",
            pair, layout.grids, arena, delta);
        Ok(())
    }
    pub(crate) fn select_pair(&mut self, pair: u32) -> Result {
        if self.layout.pair == pair {
            return Ok(());
        }
        let other = self.others.iter_mut().find(|other| other.layout.pair == pair).ok_or(EINVAL)?;
        core::mem::swap(&mut self.layout, &mut other.layout);
        core::mem::swap(&mut self.item_index, &mut other.item_index);
        core::mem::swap(&mut self.publications, &mut other.publications);
        core::mem::swap(&mut self.deferred, &mut other.deferred);
        core::mem::swap(&mut self.fresh_pair, &mut other.fresh);
        core::mem::swap(&mut self.transports, &mut other.transports);
        Ok(())
    }
    pub(crate) fn after_control(&self, memory: &mut Memory, vm: &Vm, ttbs: u64) -> Result {
        if self.adopted {
            // The dormant startup already installed these roots and operands.
            // In particular, slots 2/3 now belong to the live compute owner.
            return Ok(());
        }
        for (slot, pa) in self.empty_high.into_iter().enumerate() {
            let high = if slot == 1 && *crate::module_parameters::native_render_vms.value() == 1 {
                vm.firmware_root()
            } else {
                pa
            };
            memory.write64(
                ttbs + slot as u64 * 16 + 8,
                ((slot as u64) << 48) | high | 1,
            )?;
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
        client.cache_with(false, &self.client.cpu_maps)?;
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
    pub(crate) fn retain_native(
        &mut self,
        publications: [q::Publication; 2],
        channels: [abi::Channel; 2],
        timestamps: [u64; 4],
    ) -> Result {
        if !self.adopted || self.ordinal != 0 || self.growth.is_some() {
            return Err(EINVAL);
        }
        self.publications = publications;
        self.channels = channels;
        self.timestamps = timestamps;
        self.layout = life::NATIVE;
        self.ordinal = 1;
        self.storage = 1;
        self.next_storage = None;
        self.item_index = 0;
        for list in &mut self.deferred {
            list.clear();
        }
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
    vm: &mut Vm,
    work: &mut Submission,
    p: &Parameters,
) -> Result {
    stage_next_prepared(memory, vm, work, p, None)
}
pub(crate) fn stage_next_prepared(
    memory: &mut Memory, vm: &mut Vm, work: &mut Submission,
    p: &Parameters, prepared: Option<KBox<PreparedAppend>>,
) -> Result {
    let configured = p.with_geometry_scratch().map_err(|_| EINVAL)?;
    let p = &configured;
    let mut item = life::Item::retained(
        work.ordinal.checked_add(1).ok_or(EOVERFLOW)?,
        if work.fresh_pair {
            0
        } else {
            work.item_index.checked_add(1).ok_or(EOVERFLOW)?
        },
        work.layout,
    )
    .map_err(|_| EINVAL)?;
    if let Some(storage) = work.next_storage {
        if storage >= life::STORAGE_SUBMISSIONS { return Err(EINVAL); }
        item.storage = storage;
    }
    let mut counters = [q::Counters::new([0; 3]).map_err(|_| EIO)?; 2];
    let word = |address| memory.read_firmware32(vm.physical(memory, 2, address)?);
    // Source scheduler_publication_values: firmware retains +0x0c across
    // finite record reuse. A fresh seeded slot must still start at 0/1/2.
    let record_a = work.layout.pools[0] + item.records()[0] as u64 * 0x100;
    // Native dependency waves explicitly select scheduler publication base
    // zero in the source; retain that diagnostic override for this profile.
    let reused = !work.layout.native && word(record_a + 0x0c)? != 0;
    let phases = life::scheduler_phases(word(item.slot())?, reused)
        .map_err(|_| EIO)?;
    for index in 0..2 {
        let previous = work.publications[index].write_after;
        for at in [0, 0x30, 0x40] {
            if word(work.layout.pointers[index] + at)? != previous {
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
        if !work.fresh_pair && !work.publications[index].completed(previous, counters[index]) {
            return Err(EBUSY);
        }
        counters[index].slot().map_err(|_| EBUSY)?;
    }
    if counters[0].0 != counters[1].0 {
        pr_err!("G17P: append fail site stage_counters ordinal {} pair {} TA {:?} FR {:?}\n",item.ordinal,item.layout.pair,counters[0].0,counters[1].0);
        return Err(EIO);
    }
    let configured = select_scratch(memory, &mut work.client,
        &mut work.scratch_leases, &mut work.scratch_cursor, p)?;
    let configured = work.ticket_parameters(item.storage, configured);
    let p = &configured;
    validate_scratch_backing(&work.client, p)?;
    let next_scratch = p.scratch_layout().map_err(|_| EINVAL)?;
    let next_key = ScratchKey::of(p);
    // All preceding GPU readers retired before geometry input is cleared.
    if work.scratch_key != next_key {
        let (_, stride, tpc) = next_scratch;
        let pairs = pool_count();
        for pair in 0..pairs {
        let delta = pair * p.scratch_pair_stride;
        for (base, len) in [(p.tilemap + delta, stride * 8), (p.tpc + delta, tpc)] {
            let mut offset = 0;
            while offset < len {
                let size = (len - offset).min(PAGE as u64) as usize;
                let at = base + offset;
                let pte = work.client.root.pte(at & !(PAGE as u64 - 1))?;
                let pa = (pte & 0x000003ffffffc000) | (at & (PAGE as u64 - 1));
                memory.invalidate(pa, size)?;
                memory.write(pa, &ZERO_PAGE[..size])?;
                memory.clean(pa, size)?;
                offset += size as u64;
            }
        }
        }
        work.scratch_layout = next_scratch;
        work.scratch_key = next_key;
    }
    // Both stages and both channel consumers have retired. Replace only
    // their finite pointer/item backing; retain queue identities and logical
    // completion counters. Inactive banks are checked before every reuse.
    if !work.layout.native {
        for index in 0..2 {
            let backing_capacity = if work.layout.independent && work.layout.pair == 1
                && work.layout.pointers[index] < 0xfffffc20cf000000 {
                PAGE as u32 / 8
            } else { 0x2870 / 8 };
            let capacity = memory.read_firmware32(vm.physical(memory, 2, work.layout.pointers[index] + 0x60)?)?.min(backing_capacity);
            if capacity < 3 { return Err(EIO); }
            let previous = work.publications[index].write_after;
            if previous.checked_add(3).ok_or(EOVERFLOW)? > capacity {
                let next = work.transports[index].switch(memory, vm,
                    work.layout.queues[index], work.layout.pointers[index],
                    work.layout.rings[index], previous).inspect_err(|e|
                        pr_err!("G17P: append fail site transport_switch ordinal {} pair {} stage {} previous {} error {:?}\n",item.ordinal,item.layout.pair,index,previous,e))?;
                work.layout.pointers[index] = next[0];
                work.layout.rings[index] = next[1];
                work.publications[index].write_after = 0;
            }
        }
        item.layout = work.layout;
    }
    // Source _map_descriptor_alias grows context-zero descriptor aliases on
    // demand. Do not expose the late fragment range during cold startup: its
    // low DVAs overlap earlier context-zero operand backing.
    if item.ordinal >= 128 {
        for kind in [Kind::Tiling, Kind::Fragment] {
            vm.alias_firmware(
                memory,
                item.descriptor_address(kind),
                item.descriptor_alias(kind),
                kind.size(),
            )?;
        }
        vm.flush_tables(memory)?;
        tlbi();
    }
    let word = |address| memory.read_firmware32(vm.physical(memory, 2, address)?);
    // Allocate/build the entire append before editing any live resource state.
    // Every destination was mapped and retained before the first publication.
    let mut objects = if let Some(plan) = prepared.filter(|plan| plan.matches(work, item, p)) {
        KBox::into_inner(plan).objects
    } else {
        // Stale ordinal/layout/BO/flag/VA/offset/geometry identities never
        // publish an old plan. Rebuild against the exact current owner.
        build_host_objects(item, p)?
    };
    for (index, kind) in [Kind::Tiling, Kind::Fragment].into_iter().enumerate() {
        for (ty, object) in objects[index].iter_mut().enumerate() {
            let size = object.body.len();
            if ty == 2 && item.index >= life::STORAGE_SUBMISSIONS - 1 {
                let mut previous = KVVec::with_capacity(size, GFP_KERNEL)?;
                previous.resize(size, 0, GFP_KERNEL)?;
                let mut offset = 0;
                while offset < size {
                    let va = object.address.checked_add(offset as u64).ok_or(EOVERFLOW)?;
                    let count = (size - offset).min(PAGE - (va as usize & (PAGE - 1)));
                    let pa = vm.physical(memory, 2, va)?;
                    let last = va.checked_add(count as u64 - 1).ok_or(EOVERFLOW)?;
                    if vm.physical(memory, 2, last)? != pa.checked_add(count as u64 - 1).ok_or(EOVERFLOW)? {
                        pr_err!("G17P: append fail site context_span ordinal {} stage {} address {:#x} count {}\n",item.ordinal,index,va,count);
                        return Err(EIO);
                    }
                    memory.read_firmware_words(pa, &mut previous[offset..offset + count])?;
                    offset += count;
                }
                graph::update_context(kind, &mut previous, &object.body).map_err(|_| EINVAL)?;
                object.body = previous;
            }
            for off in (0..size).step_by(PAGE) {
                vm.physical(memory, 2, object.address + off as u64)?;
            }
            vm.physical(memory, 2, object.address + size as u64 - 1)?;
        }
    }
    if !work.layout.native {
        for (index, kind) in [Kind::Tiling, Kind::Fragment].into_iter().enumerate() {
            item.scheduler_node(kind, &mut objects[index][0].body).map_err(|_| EINVAL)?;
        }
    }
    // _apply_scheduler_node retains the base mirrors for every pair-zero
    // item and the created pair's first item. Only its later items use the
    // live selected Pool-B record, whose finite cycle differs from registers.
    if item.refresh_pool_b_mirrors() {
        let pool_b = work.layout.pools[1] + item.records()[1] as u64 * 0x80;
        let b00 = word(pool_b)?;
        let b28 = word(pool_b + 0x28)?;
        for (index, kind) in [Kind::Tiling, Kind::Fragment].into_iter().enumerate() {
            item.pool_b_mirrors(kind, &mut objects[index][0].body, b00, b28);
        }
    }
    let inner = word(work.layout.inner)?;
    if inner > 2 * (item.ordinal + 1) {
        pr_err!("G17P: append fail site inner_progress ordinal {} pair {} inner {}\n",item.ordinal,item.layout.pair,inner);
        return Err(EIO);
    }
    // _advance_tilemap_block resets a completed allocation only when its
    // eight-block ring wraps. Keep the persistent directory and TPC intact.
    if let Some(address) = item.tilemap_reset(p) {
        let mut zeros = KVVec::with_capacity(next_scratch.1 as usize, GFP_KERNEL)?;
        zeros.resize(next_scratch.1 as usize, 0, GFP_KERNEL)?;
        // Source _write_dva resolves the currently selected caller root.
        // Second-owner private pages belong to that root and need not exist
        // in the original VM template. Validate every owned span first.
        let mut spans = KVec::new();
        let mut offset = 0;
        while offset < zeros.len() {
            let at = address + offset as u64;
            let pte = work.client.root.pte(at & !(PAGE as u64 - 1))?;
            if pte & 3 != 3 {
                pr_err!("G17P: append fail site tilemap_pte ordinal {} address {:#x} pte {:#x}\n",item.ordinal,at,pte);
                return Err(EIO);
            }
            let pa = (pte & 0x000003ffffffc000) | (at & (PAGE as u64 - 1));
            let size = (zeros.len() - offset).min(PAGE - (at as usize & (PAGE - 1)));
            memory.word64(pa & !7)?;
            memory.word64((pa + size as u64 - 1) & !7)?;
            spans.push((pa, offset, size), GFP_KERNEL)?;
            offset += size;
        }
        for (pa, offset, size) in spans {
            memory.invalidate(pa, size)?;
            memory.write(pa, &zeros[offset..offset + size])?;
            memory.clean(pa, size)?;
        }
    }
    work.client.cache(false)?;
    if let Some(service) = work.growth.as_mut() {
        service.bind_pool_work(
            work.layout.pair,
            [0xfffffc2000000100, 0xfffffc2000000200],
            item.descriptor_address(Kind::Fragment),
            work.layout.grids[1],
            item.ordinal,
        )?;
    } else if !work.layout.native {
        return Err(EINVAL);
    }
    let write = |memory: &mut Memory, address, body: &[u8]| vm.write(memory, 2, address, body);
    let records = item.records();
    let node = item.ordinal + item.ordinal / 2;
    write(
        memory,
        work.layout.pools[0] + records[0] as u64 * 0x100 + 8,
        &node.to_le_bytes(),
    )?;
    write(
        memory,
        work.layout.pools[0] + records[0] as u64 * 0x100 + 0x10,
        &0x50u32.to_le_bytes(),
    )?;
    write(
        memory,
        work.layout.pools[1] + records[1] as u64 * 0x80 + 0x4c,
        &1u32.to_le_bytes(),
    )?;
    // Source lifecycle before -> fragment -> tiling. No producer yet visible.
    write(memory, work.layout.leaves[4] + 0x60, &1u32.to_le_bytes())?;
    write(memory, item.slot(), &phases[0].to_le_bytes())?;
    for index in [1, 0] {
        for object in &objects[index] {
            write(memory, object.address, &object.body)?;
        }
        // Do not copy this firmware record into the render-root operand
        // backing at the same low DVA. Context 0 already aliases the high view.
        let phase = if index == 1 { 1u32 } else { 2u32 };
        write(memory, item.slot(), &phases[phase as usize].to_le_bytes())?;
        write(
            memory,
            work.layout.inner,
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
            write(
                memory,
                work.layout.leaves[5],
                &(item.index + 1).to_le_bytes(),
            )?;
            for address in [p.timestamp_a, p.timestamp_b] {
                write(memory, address, &[0; 8])?;
            }
        }
    }
    // Retired prior PB release, then the same retained owner is selected again.
    write(
        memory,
        work.layout.shared[0] + 0x0c,
        &u32::MAX.to_le_bytes(),
    )?;
    write(
        memory,
        work.layout.shared[0] + 0x0c,
        &work.layout.pair.to_le_bytes(),
    )?;
    write(memory, work.layout.shared[0], &item.index.to_le_bytes())?;
    let count = memory.read_firmware32(vm.physical(memory, 2, work.layout.shared[0] + 0x34)?)?;
    let pa = vm.physical(memory, 2, work.layout.shared[0] + 0x28)?;
    memory.invalidate(pa, 8)?;
    if let Some(body) = life::index_registration(memory.read64(pa)?, count).map_err(|_| EIO)? {
        let at = 0xfffffc20015e0000 + work.layout.pair as u64 * 0x10;
        let pa = vm.physical(memory, 2, at)?;
        memory.invalidate(pa, 8)?;
        if memory.read64(pa)? != u64::from_le_bytes(body) {
            write(memory, at, &body)?;
        }
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
            queue: work.layout.queues[index],
            pointers: work.layout.pointers[index],
            item_ring: work.layout.rings[index],
            item_capacity: if work.layout.independent && work.layout.pair == 1
                && work.layout.pointers[index] < 0xfffffc20cf000000 {
                PAGE as u32 / 8
            } else {
                0x2870 / 8
            },
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
            group: item.index + 1,
            grid: work.layout.grids[index],
            kind: if index == 0 {
                q::Kind::Tiling
            } else {
                q::Kind::Fragment
            },
            first: work.fresh_pair,
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
    work.firmware_completion = p.fragment_timestamp_end;
    work.ordinal = item.ordinal;
    work.storage = item.storage;
    work.next_storage = None;
    work.item_index = item.index;
    work.fresh_pair = false;
    work.timestamps = [
        p.ta_user_timestamp_start,
        p.ta_user_timestamp_end,
        p.fragment_user_timestamp_start,
        p.fragment_user_timestamp_end,
    ];
    Ok(())
}
