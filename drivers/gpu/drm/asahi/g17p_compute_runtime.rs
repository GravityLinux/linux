// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! First direct-compute graph and its owned caller mappings. Addresses and
//! field values follow native_add3's direct queue zero, without its workload.

use super::{
    g17p_abi as abi, g17p_compute as c, g17p_compute_lifecycle as lifecycle,
    g17p_compute_memory as cm, g17p_drm::Object, g17p_image::Image, g17p_memory::Memory,
    g17p_queue as q, g17p_user_vm::UserVm, g17p_vm::Vm,
};
use kernel::{drm::gem::BaseObject, prelude::*, sync::aref::ARef};

const PAGE: usize = 0x4000;
pub(crate) const QUEUE: u64 = 0xfffffc20c0000300;
pub(crate) const POINTERS: u64 = 0xfffffc200165a870;
const RING: u64 = 0xfffffc20c08aa870;
const DESCRIPTOR: u64 = 0xfffffc20c0358000;
const DESCRIPTOR_LOW: u64 = 0x7000340000;
const OPTIONAL: u64 = 0xfffffc20c0605e80;
const EVENT: u64 = 0xfffffc20c05ea200;
const CTX_HIGH: u64 = 0xfffffc2000278000;
const CTX_LOW: u64 = 0x70004d8000;
const CONTROL: u64 = 0xfffffc20c07b8040;
const SCHEDULERS: u64 = 0xfffffc20c0868000;
const SCHEDULER: u64 = SCHEDULERS + 0x100;
const SHARED_STATE: u64 = 0xfffffc2001630000;
const SUPPORT: u64 = 0xfffffc20c0870000;
const SUPPORT_STATE: u64 = 0xfffffc2001638000;
const ZERO: u64 = 0xfffffc2001640000;
const JOB_LIST: u64 = 0xfffffc2000000018;
const OPERANDS: u64 = 0x7000238000;
const OPERAND_TABLE: u64 = 0x7000208000;
const STATE: u64 = 0x7000220000;
const ROBUSTNESS: u64 = 0x1000018000;
pub(crate) const STATUS: [u64; 2] = [0xfffffc2000024c68, 0xfffffc2000024c70];

pub(crate) struct Client {
    pub(crate) root: UserVm,
    pub(crate) buffers: KVec<ARef<Object>>,
    pub(crate) bindings: KVec<(u64, u64, u64, u32)>,
    pub(crate) owner: (u64, u32),
}
impl Client {
    /// Update only the caller-owned part of the quiescent retained root. Both
    /// generations of GEM references remain pinned until the final TLBI.
    pub(crate) fn rebind(&mut self, next: Self, render: bool) -> Result {
        if render && self.owner != next.owner {
            return Err(EINVAL);
        }
        let address = |base: u64| {
            if render && base < 0x1000000000 {
                base + 0x1000000000
            } else {
                base
            }
        };
        let mut changes: KVec<(u64, u64, u64)> = KVec::new();
        for &(base, size, _, _) in &self.bindings {
            for offset in (0..size).step_by(PAGE) {
                let va = address(base) + offset;
                let old = self.root.pte(va)?;
                if old == 0 {
                    return Err(EIO);
                }
                changes.push((va, old, 0), GFP_KERNEL)?;
            }
        }
        for &(base, size, _, _) in &next.bindings {
            for offset in (0..size).step_by(PAGE) {
                let va = address(base) + offset;
                let new = next.root.pte(va)?;
                if new == 0 {
                    return Err(EIO);
                }
                if let Some(row) = changes.iter_mut().find(|r| r.0 == va) {
                    if row.2 != 0 {
                        return Err(EINVAL);
                    }
                    row.2 = new;
                } else {
                    // Never replace a private/growth leaf on an addition.
                    if self.root.pte(va)? != 0 {
                        return Err(EBUSY);
                    }
                    changes.push((va, 0, new), GFP_KERNEL)?;
                }
            }
        }
        next.cache(false)?;
        self.root
            .rebind(&changes, if render { &[1] } else { &[2, 3] })?;
        // rebind cannot fail after its first live store. Its final ASID flush
        // precedes release of old BOs; the root and all private state survive.
        self.buffers = next.buffers;
        self.bindings = next.bindings;
        self.owner = next.owner;
        Ok(())
    }
    pub(crate) fn cache(&self, invalidate: bool) -> Result {
        for bo in &self.buffers {
            let map = bo.vmap::<u8>()?;
            if map.is_iomem() {
                return Err(EINVAL);
            }
            for offset in (0..bo.size()).step_by(64) {
                let pointer = map.ptr_from_index(offset)?;
                // SAFETY: Complete cache lines of pinned, page-sized GEM RAM.
                // Invalidation is used only after the caller's writes were
                // cleaned and hardware completion was observed.
                unsafe {
                    if invalidate {
                        core::arch::asm!("dc ivac, {p}", p=in(reg)pointer, options(nostack,preserves_flags));
                    } else {
                        core::arch::asm!("dc cvac, {p}", p=in(reg)pointer, options(nostack,preserves_flags));
                    }
                }
            }
        }
        super::g17p_memory::sync();
        Ok(())
    }
}
pub(crate) struct Parameters {
    pub(crate) preempt: u64,
    pub(crate) cdm: u64,
    pub(crate) end: u64,
    pub(crate) sampler: u64,
    pub(crate) sampler_count: u32,
    pub(crate) timestamps: [u64; 2],
}
pub(crate) struct Submission {
    pub(crate) client: Client,
    pub(crate) publication: q::Publication,
    pub(crate) channel: abi::Channel,
    pub(crate) ordinal: u32,
    pub(crate) preempt: u64,
    pub(crate) status: [u64; 2],
    pub(crate) timestamps: [u64; 2],
    pub(crate) after_render: bool,
    pub(crate) pointers: u64,
    ring: u64,
    transport: TransportPool,
    robustness: u64,
}
impl Submission {
    pub(crate) fn capacity(&self) -> u32 {
        if self.after_render {
            2
        } else {
            SUBMISSIONS
        }
    }
}
// The source direct bootstrap reserves 258 logical command placements while
// recycling its 240 descriptors, 256 context records and 36 scheduler records.
pub(crate) const SUBMISSIONS: u32 = 258;

struct TransportSlot {
    physical: u64,
    retired: Option<KVVec<u8>>,
}
struct TransportPool {
    slots: [Option<TransportSlot>; 2],
    switches: u32,
}

fn read_owned(memory: &Memory, vm: &Vm, address: u64, body: &mut [u8]) -> Result {
    if address & 7 != 0 || body.len() % 8 != 0 {
        return Err(EINVAL);
    }
    for (i, bytes) in body.chunks_exact_mut(8).enumerate() {
        let pa = vm.physical(memory, 2, address + i as u64 * 8)?;
        memory.invalidate(pa, 8)?;
        bytes.copy_from_slice(&memory.read64(pa)?.to_le_bytes());
    }
    Ok(())
}

impl TransportPool {
    fn switch(
        &mut self,
        memory: &mut Memory,
        vm: &mut Vm,
        pointers: u64,
        ring: u64,
        done: u32,
    ) -> Result<[u64; 2]> {
        let mut record = [0; 0xc0];
        read_owned(memory, vm, QUEUE, &mut record)?;
        let slot = self.switches as usize % 2;
        let next = lifecycle::transport_record(&mut record, [pointers, ring], done, slot as u32)
            .map_err(|_| EIO)?;
        // Preserve snapshots on the heap, never on the small kernel stack.
        let mut snapshot = KVVec::with_capacity(0x80 + 0x2870, GFP_KERNEL)?;
        snapshot.resize(0x80 + 0x2870, 0, GFP_KERNEL)?;
        for (index, owned) in self.slots.iter_mut().enumerate() {
            let Some(owned) = owned else {
                continue;
            };
            let base = lifecycle::TRANSPORT_BASE + index as u64 * 0x8000;
            for offset in [0, PAGE as u64] {
                if vm.physical(memory, 2, base + offset)? != owned.physical + offset {
                    return Err(EIO);
                }
            }
            if let Some(retired) = &owned.retired {
                if base != pointers {
                    read_owned(memory, vm, base, &mut snapshot[..0x80])?;
                    read_owned(memory, vm, base + PAGE as u64, &mut snapshot[0x80..])?;
                    if snapshot.as_slice() != retired.as_slice() {
                        return Err(EIO);
                    }
                }
            }
        }
        for (index, owned) in self.slots.iter_mut().enumerate() {
            if lifecycle::TRANSPORT_BASE + index as u64 * 0x8000 == pointers {
                let owned = owned.as_mut().ok_or(EIO)?;
                read_owned(memory, vm, pointers, &mut snapshot[..0x80])?;
                read_owned(memory, vm, ring, &mut snapshot[0x80..])?;
                owned.retired = Some(snapshot);
                break;
            }
        }
        if self.slots[slot].is_none() {
            let physical = vm.transport_backing(memory, next[0])?;
            self.slots[slot] = Some(TransportSlot {
                physical,
                retired: None,
            });
        }
        vm.write(memory, 2, next[0], &lifecycle::transport_pointers())?;
        // Memory owns both contiguous pages, so zero only the ring payload.
        let physical = self.slots[slot].as_ref().ok_or(EIO)?.physical;
        memory.zero(physical + PAGE as u64, 0x2870)?;
        memory.clean(physical + PAGE as u64, 0x2870)?;
        super::g17p_memory::sync();
        vm.write(memory, 2, QUEUE, &record)?;
        super::g17p_memory::sync();
        let mut after = [0; 0xc0];
        read_owned(memory, vm, QUEUE, &mut after)?;
        if after[..16] != record[..16] {
            return Err(EIO);
        }
        for (offset, expected) in [(0, 0), (0x30, 0), (0x40, 0), (0x50, u32::MAX)] {
            if memory.read_firmware32(vm.physical(memory, 2, next[0] + offset)?)? != expected {
                return Err(EIO);
            }
        }
        self.slots[slot].as_mut().ok_or(EIO)?.retired = None;
        self.switches = self.switches.checked_add(1).ok_or(EOVERFLOW)?;
        Ok(next)
    }
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
fn client_storage(
    memory: &mut Memory,
    root: &mut UserVm,
    address: u64,
    size: usize,
) -> Result<u64> {
    root.prepare(address, size as u64)?;
    let pa = memory.allocate(size)?;
    memory.clean(pa, size)?;
    for offset in (0..size).step_by(PAGE) {
        root.map_page(address + offset as u64, pa + offset as u64, true)?;
    }
    Ok(pa)
}

pub(crate) fn build(
    memory: &mut Memory,
    vm: &mut Vm,
    image: &Image,
    ttbs: u64,
    mut client: Client,
    parameters: &Parameters,
    after_render: bool,
) -> Result<Submission> {
    let status = if after_render {
        let base = vm.fresh_firmware_page(memory, lifecycle::AFTER_RENDER_STATUS)?;
        [base, base + 8]
    } else {
        STATUS
    };
    let mut page = KVVec::with_capacity(PAGE, GFP_KERNEL)?;
    page.resize(PAGE, 0, GFP_KERNEL)?;
    let lists = client_storage(memory, &mut client.root, 0x7000000000, 0x200000)?;
    let operands = client_storage(memory, &mut client.root, OPERAND_TABLE, 0x10000)?;
    client_storage(memory, &mut client.root, STATE, 0x14000)?;
    // Source maps a zero native-control page at 0x70013a0000 before
    // allocating operands, which replace that leaf. Map the final owner once:
    // the control address and blank low queue contexts lie in these tranches.
    for i in 0..42 {
        client_storage(memory, &mut client.root, OPERANDS + i * 0x108000, 0x100000)?;
    }
    // _ensure_compute_robustness retains private state in the VM's reserved
    // aperture before caller BOs replace the old native DVAs. The kernel's
    // preemption slots occupy its first 0x84000 bytes, so use the next MiB.
    // This is driver state, not a change to the fixed USC execution base.
    let robustness = parameters.preempt + 0x100000;
    for offset in [0, 0x8000] {
        let pa = client_storage(memory, &mut client.root, robustness + offset, PAGE)?;
        if client.root.pte(ROBUSTNESS + offset)? == 0 {
            client.root.prepare(ROBUSTNESS + offset, PAGE as u64)?;
            client.root.map_page(ROBUSTNESS + offset, pa, true)?;
        }
    }
    client_storage(memory, &mut client.root, parameters.preempt, 0xc000)?;
    client_storage(
        memory,
        &mut client.root,
        parameters.preempt + 0x78000,
        0xc000,
    )?;
    cm::PageLists {
        base: OPERANDS,
        entries: 21,
        buffer_size: 0x100000,
        buffer_stride: 0x108000,
        page_size: PAGE,
    }
    .build(&mut page)
    .map_err(|_| EINVAL)?;
    memory.write(lists, &page)?;
    memory.clean(lists, PAGE)?;
    cm::operand_table_contiguous(&mut page, OPERANDS, 21).map_err(|_| EINVAL)?;
    memory.write(operands, &page)?;
    memory.clean(operands, PAGE)?;

    for (va, size) in [
        (SCHEDULERS, PAGE),
        (SHARED_STATE, PAGE),
        (SUPPORT, PAGE),
        (SUPPORT_STATE, PAGE),
        (JOB_LIST - 0x18, 0x60),
        (CONTROL, 0x40),
        (OPTIONAL, 0xc0),
        (EVENT, 0x400),
        (ZERO, PAGE),
        (POINTERS, 0x80),
        (RING, 0x2870),
        (QUEUE, 0xc0),
        (CTX_HIGH, 8 * PAGE),
        (0xfffffc20001c8008, 8),
        (0xfffffc20c07c0008, 8),
        (status[0], 0x20),
    ] {
        vm.ensure_firmware(memory, va, size)?;
    }
    vm.alias_firmware(memory, DESCRIPTOR, DESCRIPTOR_LOW, PAGE)?;
    vm.alias_firmware(memory, CTX_HIGH, CTX_LOW, 8 * PAGE)?;
    // Reserve the retained-lifetime storage before initdata publication. This
    // preserves all page-table and alias placement while firmware is running.
    for ordinal in 1..if after_render { 2 } else { SUBMISSIONS } {
        let spec = lifecycle::Retained::new(ordinal).map_err(|_| EINVAL)?;
        vm.alias_firmware(memory, spec.descriptor, spec.descriptor_low, 0x1000)?;
        for (address, size) in [
            (spec.scheduler, 0x100),
            (spec.scheduler_slot, 4),
            (spec.optional, 0xc0),
            (spec.event, 0x40),
            (spec.dispatch[0], 8),
            (spec.dispatch[1], 8),
            (
                if after_render {
                    status[0] + 0x10
                } else {
                    spec.status[0]
                },
                8,
            ),
            (
                if after_render {
                    status[1] + 0x10
                } else {
                    spec.status[1]
                },
                8,
            ),
            (lifecycle::SUPPORT, PAGE),
            (lifecycle::SUPPORT_STATE, PAGE),
            (lifecycle::ZERO, PAGE),
        ] {
            vm.ensure_firmware(memory, address, size)?;
        }
    }
    let write = |memory: &mut Memory, va, bytes: &[u8]| vm.write(memory, 2, va, bytes);
    let mut dispatch = [0u8; 0x20];
    abi::compute_dispatch(&mut dispatch).map_err(|_| EINVAL)?;
    if after_render {
        // Graphics startup omits this record. Never overwrite live allocator
        // history or the adjacent render record on a force_fresh transition.
        for offset in (0..0x20).step_by(8) {
            let pa = vm.physical(memory, 2, 0xfffffc20015e8020 + offset)?;
            memory.invalidate(pa, 8)?;
            if memory.read64(pa)? != 0 {
                return Err(EIO);
            }
        }
    }
    write(memory, 0xfffffc20015e8020, &dispatch)?;
    for offset in [0, 0x18, 0x30, 0x48] {
        let address = JOB_LIST - 0x18 + offset;
        write(memory, address, &q::job_list(address))?;
    }
    let mut control = [0u8; 0x40];
    for (offset, value) in [
        (0, 0x000001000000ffffu64),
        (0x20, 0x0002000000000000),
        (0x30, 0xff000000),
    ] {
        control[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    write(memory, CONTROL, &control)?;
    page.fill(0);
    for i in 0..36 {
        c::u64_at(&mut page, i * 0x100, SHARED_STATE + i as u64 * 4);
    }
    for index in 0..if after_render { 2 } else { 3 } {
        page[(index + 1) * 0x100..(index + 2) * 0x100].copy_from_slice(
            &c::Scheduler {
                slot: SHARED_STATE + 4 + index as u64 * 4,
                work_id: 0,
                phase: 0,
                job_list: 0,
                node_id: 0,
                completion_kind: 0,
            }
            .build(),
        );
    }
    write(memory, SCHEDULERS, &page)?;
    page.fill(0);
    for index in 1..if after_render { 3 } else { 4 } {
        c::u32_at(&mut page, index * 4, 1);
    }
    write(memory, SHARED_STATE, &page)?;
    cm::Support {
        compact: None,
        header: 2,
        word_08: 1,
        word_10: 2,
        resource_class: 0x15,
        word_20: None,
        word_28: None,
        client_state: OPERAND_TABLE,
        firmware_state: SUPPORT_STATE,
        cursor: 0xa8,
        field_54: 0,
        field_5c: 1,
        final_kind: 2,
    }
    .build(&mut page)
    .map_err(|_| EINVAL)?;
    write(memory, SUPPORT, &page)?;
    cm::shared_state(&mut page, 1).map_err(|_| EINVAL)?;
    write(memory, SUPPORT_STATE, &page)?;
    page.fill(0);
    write(memory, ZERO, &page)?;
    for address in [0xfffffc20001c8008, 0xfffffc20c07c0008, status[0], status[1]] {
        write(memory, address, &[0; 8])?;
    }
    page.fill(0);
    page[..0x60].copy_from_slice(&q::pointers(u32::MAX));
    c::u32_at(&mut page, 0x60, 0x500);
    write(memory, POINTERS, &page[..0x80])?;
    page.fill(0);
    write(memory, RING, &page[..0x2870])?;
    write(
        memory,
        QUEUE,
        &q::Record {
            pointers: POINTERS,
            ring: RING,
            job_list: JOB_LIST,
            context: CONTROL,
            uuid: 0x170,
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
    page.fill(0);
    c::Context {
        descriptor: DESCRIPTOR,
        queue: QUEUE,
        grid: 4,
        flags: 0x1000000000000000,
        word_220: 0xffff080100000001,
        word_330: 0,
        word_338: 4,
        word_350: 0x000110038001a002,
        word_358: 0x000020038001a03b,
        word_378: 0x003fffffffffffff,
        item_index: 0,
        points: None,
        event_slot: None,
        completion: None,
    }
    .build(&mut page[0x200..0x400])
    .map_err(|_| EINVAL)?;
    write(memory, CTX_HIGH, &page)?;
    write(
        memory,
        OPTIONAL,
        &c::Optional {
            context_low: CTX_LOW,
            context_high: CTX_HIGH,
            grid: 4,
            ordinal: 0x3f,
            shared_control: SUPPORT,
            channel_control: CONTROL,
            uuid: 0x170,
            field_46: 1,
            field_1e: 2,
            field_32: 2,
            field_56: 1,
            field_5e: 2,
            first: true,
            item_index: 0,
        }
        .build(),
    )?;
    let mut cold_registers;
    let mut warm_registers;
    let registers: &[c::Register] = if after_render {
        warm_registers =
            lifecycle::after_render_opening_program(parameters.preempt, parameters.cdm)
                .map_err(|_| EINVAL)?;
        for (number, value) in &mut warm_registers {
            if *number == 0x14070 {
                *value = robustness | 1;
            }
        }
        &warm_registers
    } else {
        cold_registers =
            lifecycle::opening_program(parameters.preempt, parameters.cdm).map_err(|_| EINVAL)?;
        for (number, value) in &mut cold_registers {
            if *number == 0x14070 {
                *value = robustness | 1;
            }
        }
        &cold_registers
    };
    c::Descriptor {
        scheduler: SCHEDULER,
        low_alias: DESCRIPTOR_LOW,
        cdm_terminator: parameters.end - 4,
        sequence: 0,
        context: 2,
        grid: 4,
        dispatch: [0xfffffc20001c8008, 0xfffffc20c07c0008],
        status,
        timestamps: parameters.timestamps,
        shared_control: SUPPORT,
        zero_page: ZERO,
        support_control: 0xe0a00001,
        support_flags: 0,
        ordinal: 0,
        queue_submission: 1,
        queue_ordinal: 0,
        submission_index: 1,
        sampler_array: parameters.sampler,
        sampler_count: parameters.sampler_count,
    }
    .build(&mut page, registers)
    .map_err(|_| EINVAL)?;
    write(memory, DESCRIPTOR, &page)?;
    page.fill(0);
    write(memory, EVENT, &page[..0x400])?;
    prepare_cold_queues(memory, vm, &mut page, if after_render { 2 } else { 3 })?;
    let channel = image.graph.channels[0][q::COMPUTE_CHANNEL];
    if channel.ring != 0xfffffc20c07a1dc0 {
        return Err(EINVAL);
    }
    let mut counts = [0; 3];
    for (value, address) in counts.iter_mut().zip(channel.states) {
        *value = memory.read_firmware32(vm.physical(memory, 2, address)?)?;
    }
    if counts[0] != counts[1] || counts[1] != counts[2] {
        return Err(EBUSY);
    }
    let publication = q::Stage {
        queue: QUEUE,
        pointers: POINTERS,
        item_ring: RING,
        item_capacity: 0x2870 / 8,
        write_index: 0,
        channel_ring: channel.ring,
        channel_producer: channel.states[2],
        counters: q::Counters::new(counts).map_err(|_| EINVAL)?,
        slot: if after_render { None } else { Some(0) },
        items: &[DESCRIPTOR, OPTIONAL, EVENT],
        group: 1,
        grid: 4,
        kind: q::Kind::Compute,
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
    .map_err(|error| match error {
        q::StageError::Access(e) => e,
        q::StageError::Protocol(_) => EINVAL,
    })?;
    client.cache(false)?;
    vm.flush_tables(memory)?;
    // Exact first-work client context table: independent empty upper roots.
    // Install only after the graph and all caller references are owned.
    for context in if after_render { 2 } else { 0 }..3 {
        let high = memory.allocate(PAGE)?;
        memory.clean(high, PAGE)?;
        memory.write64(ttbs + context * 16 + 8, (context << 48) | high | 1)?;
    }
    memory.write64(ttbs + 2 * 16, (2 << 48) | client.root.root() | 1)?;
    memory.write64(ttbs + 3 * 16, (3 << 48) | client.root.root() | 1)?;
    // The source bootstrap aliases the same empty upper root in slots 2/3.
    let upper = memory.read64(ttbs + 2 * 16 + 8)? & 0x3ffffffc000;
    memory.write64(ttbs + 3 * 16 + 8, (3 << 48) | upper | 1)?;
    memory.clean(ttbs, 64 * 16)?;
    super::g17p_memory::sync();
    // SAFETY: Invalidate GPU ASID translations after publishing owned roots.
    unsafe {
        core::arch::asm!(
            ".inst 0xd508811f", // TLBI VMALLE1OS (FEAT_TLBIOS, present on T8140).
            "dsb sy",
            "isb",
            options(nostack, preserves_flags)
        );
    }
    Ok(Submission {
        client,
        publication,
        channel,
        ordinal: 0,
        preempt: parameters.preempt,
        status,
        timestamps: parameters.timestamps,
        after_render,
        pointers: POINTERS,
        robustness,
        ring: RING,
        transport: TransportPool {
            slots: [None, None],
            switches: 0,
        },
    })
}

/// Prepare one later item on the completed retained transport. No producer
/// becomes visible until the session validates and explicitly publishes it.
pub(crate) fn idle(memory: &Memory, vm: &Vm, work: &Submission) -> Result<(q::Counters, u32)> {
    let mut values = [0; 3];
    for (value, address) in values.iter_mut().zip(work.channel.states) {
        *value = memory.read_firmware32(vm.physical(memory, 2, address)?)?;
    }
    let counters = q::Counters::new(values).map_err(|_| EIO)?;
    let done = memory.read_firmware32(vm.physical(memory, 2, work.pointers)?)?;
    if done != work.publication.write_after
        || values[0] != values[1]
        || values[1] != values[2]
        || !work.publication.completed(done, counters)
    {
        return Err(EBUSY);
    }
    for offset in [0x30, 0x40] {
        if memory.read_firmware32(vm.physical(memory, 2, work.pointers + offset)?)? != done {
            return Err(EBUSY);
        }
    }
    Ok((counters, done))
}

pub(crate) fn stage_next(
    memory: &mut Memory,
    vm: &mut Vm,
    work: &mut Submission,
    parameters: &Parameters,
) -> Result {
    let ordinal = work.ordinal.checked_add(1).ok_or(EOVERFLOW)?;
    if ordinal >= work.capacity() || parameters.preempt != work.preempt {
        return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
    }
    let spec = lifecycle::Retained::new(ordinal).map_err(|_| EINVAL)?;
    let (counters, mut write_index) = idle(memory, vm, work)?;
    if work.after_render {
        return stage_after_render(memory, vm, work, parameters, counters);
    }
    if ordinal % lifecycle::TRANSPORT_INTERVAL == 0 {
        let next = work
            .transport
            .switch(memory, vm, work.pointers, work.ring, write_index)?;
        work.pointers = next[0];
        work.ring = next[1];
        write_index = 0;
    }
    let mut page = KVVec::with_capacity(PAGE, GFP_KERNEL)?;
    page.resize(PAGE, 0, GFP_KERNEL)?;
    let write = |memory: &mut Memory, address, bytes: &[u8]| vm.write(memory, 2, address, bytes);
    write(memory, spec.scheduler, &spec.scheduler_body())?;
    write(memory, spec.scheduler_slot, &1u32.to_le_bytes())?;
    if ordinal == 1 {
        cm::Support {
            compact: None,
            header: 3,
            word_08: 0,
            word_10: 2,
            resource_class: 0x15,
            word_20: None,
            word_28: None,
            client_state: OPERAND_TABLE,
            firmware_state: lifecycle::SUPPORT_STATE,
            cursor: 0xa8,
            field_54: 1,
            field_5c: 1,
            final_kind: 2,
        }
        .build(&mut page)
        .map_err(|_| EINVAL)?;
        write(memory, lifecycle::SUPPORT, &page)?;
        page.fill(0);
        write(memory, lifecycle::ZERO, &page)?;
        write(memory, CONTROL, &lifecycle::channel_control())?;
        write(memory, JOB_LIST, &q::job_list(JOB_LIST))?;
    }
    write(
        memory,
        lifecycle::SUPPORT_STATE,
        &(ordinal + 1).to_le_bytes(),
    )?;
    for address in spec.dispatch {
        write(memory, address, &[0; 4])?;
    }
    for address in spec.status {
        write(memory, address, &[0; 8])?;
    }
    let mut registers = spec
        .program(parameters.preempt, parameters.cdm, ordinal % 2)
        .map_err(|_| EINVAL)?;
    for (number, value) in &mut registers {
        if *number == 0x14070 {
            *value = (work.robustness + (ordinal % 2) as u64 * 0x8000) | 1;
        }
    }
    spec.descriptor_body(
        &mut page,
        &registers,
        parameters.end,
        parameters.sampler,
        parameters.sampler_count,
        parameters.timestamps,
    )
    .map_err(|_| EINVAL)?;
    write(memory, spec.descriptor, &page[..0x1000])?;
    write(memory, spec.optional, &spec.optional_body())?;
    spec.context_body(&mut page[..0x200]).map_err(|_| EINVAL)?;
    write(memory, spec.context_record, &page[..0x200])?;
    write(memory, spec.event, &[0; 0x40])?;
    work.client.cache(false)?;
    let publication = q::Stage {
        queue: QUEUE,
        pointers: work.pointers,
        item_ring: work.ring,
        item_capacity: 0x2870 / 8,
        write_index,
        channel_ring: work.channel.ring,
        channel_producer: work.channel.states[2],
        counters,
        slot: None,
        items: &[spec.descriptor, spec.optional, spec.event],
        group: ordinal + 1,
        grid: 4,
        kind: q::Kind::Compute,
        first: write_index == 0,
        in_place: false,
        announce: false,
        defer_inner: true,
        defer_outer: true,
        event_subtype: None,
        event_counter: None,
        event_counter_low: 2,
    }
    .publish(&mut Writer { memory, vm })
    .map_err(|error| match error {
        q::StageError::Access(e) => e,
        q::StageError::Protocol(_) => EINVAL,
    })?;
    work.ordinal = ordinal;
    work.publication = publication;
    work.status = spec.status;
    work.timestamps = parameters.timestamps;
    Ok(())
}

/// Ordinary post-render command one starts the second native queue. Preserve
/// the first queue's history; do not append it to the cold retained transport.
fn stage_after_render(
    memory: &mut Memory,
    vm: &Vm,
    work: &mut Submission,
    parameters: &Parameters,
    counters: q::Counters,
) -> Result {
    use lifecycle::{
        AFTER_RENDER_CONTEXT as context, AFTER_RENDER_EVENT as event,
        AFTER_RENDER_OPTIONAL as optional, AFTER_RENDER_POINTERS as pointers,
        AFTER_RENDER_QUEUE as queue, AFTER_RENDER_RING as ring,
    };
    for offset in [0, 0x30, 0x40] {
        if memory.read_firmware32(vm.physical(memory, 2, pointers + offset)?)? != 0 {
            return Err(EBUSY);
        }
    }
    let spec = lifecycle::Retained::new(1).map_err(|_| EINVAL)?;
    let status = [work.status[0] + 0x10, work.status[1] + 0x10];
    let mut page = KVVec::with_capacity(PAGE, GFP_KERNEL)?;
    page.resize(PAGE, 0, GFP_KERNEL)?;
    let write = |memory: &mut Memory, address, bytes: &[u8]| vm.write(memory, 2, address, bytes);
    write(memory, spec.scheduler, &spec.scheduler_body())?;
    write(memory, spec.scheduler_slot, &1u32.to_le_bytes())?;
    cm::Support {
        compact: None,
        header: 3,
        word_08: 0,
        word_10: 2,
        resource_class: 0x15,
        word_20: None,
        word_28: None,
        client_state: OPERAND_TABLE,
        firmware_state: lifecycle::SUPPORT_STATE,
        cursor: 0xa8,
        field_54: 1,
        field_5c: 1,
        final_kind: 2,
    }
    .build(&mut page)
    .map_err(|_| EINVAL)?;
    write(memory, lifecycle::SUPPORT, &page)?;
    page.fill(0);
    write(memory, lifecycle::ZERO, &page)?;
    write(memory, 0xfffffc20c07b80c0, &lifecycle::channel_control())?;
    write(memory, JOB_LIST + 0x30, &q::job_list(JOB_LIST + 0x30))?;
    write(memory, lifecycle::SUPPORT_STATE, &2u32.to_le_bytes())?;
    for address in [0xfffffc20001c8014, 0xfffffc20c07c0014] {
        write(memory, address, &[0; 4])?;
    }
    for address in status {
        write(memory, address, &[0; 8])?;
    }
    let mut registers = spec
        .program(parameters.preempt, parameters.cdm, 1)
        .map_err(|_| EINVAL)?;
    for (number, value) in &mut registers {
        if *number == 0x14070 {
            *value = (work.robustness + 0x8000) | 1;
        }
    }
    lifecycle::after_render_second_descriptor(
        &mut page,
        &registers,
        parameters.end,
        parameters.sampler,
        parameters.sampler_count,
        parameters.timestamps,
        status,
    )
    .map_err(|_| EINVAL)?;
    write(memory, spec.descriptor, &page[..0x1000])?;
    write(memory, optional, &lifecycle::after_render_second_optional())?;
    page.fill(0);
    lifecycle::after_render_second_context(&mut page[0x200..0x400]).map_err(|_| EINVAL)?;
    write(memory, context, &page)?;
    write(memory, event, &[0; 0x40])?;
    write(
        memory,
        queue,
        &q::Record {
            pointers,
            ring,
            job_list: JOB_LIST + 0x30,
            context: 0xfffffc20c07b80c0,
            uuid: 0x159,
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
    work.client.cache(false)?;
    let publication = q::Stage {
        queue,
        pointers,
        item_ring: ring,
        item_capacity: 0x2870 / 8,
        write_index: 0,
        channel_ring: work.channel.ring,
        channel_producer: work.channel.states[2],
        counters,
        slot: None,
        items: &[spec.descriptor, optional, event],
        group: 2,
        grid: 5,
        kind: q::Kind::Compute,
        first: true,
        in_place: false,
        announce: false,
        defer_inner: true,
        defer_outer: true,
        event_subtype: None,
        event_counter: None,
        event_counter_low: 2,
    }
    .publish(&mut Writer { memory, vm })
    .map_err(|error| match error {
        q::StageError::Access(e) => e,
        q::StageError::Protocol(_) => EINVAL,
    })?;
    work.ordinal = 1;
    work.publication = publication;
    work.pointers = pointers;
    work.ring = ring;
    work.status = status;
    work.timestamps = parameters.timestamps;
    Ok(())
}

/// The retained source bootstrap constructs all three queue owners before
/// initdata, even though only queue zero has its first descriptor published.
fn prepare_cold_queues(memory: &mut Memory, vm: &mut Vm, page: &mut [u8], count: u64) -> Result {
    for slot in 1u64..count {
        let (
            queue,
            pointers,
            ring,
            high,
            low,
            optional,
            event,
            scheduler,
            scheduler_slot,
            support,
            state,
            zero,
            grid,
            uuid,
            word220,
        ) = if slot == 1 {
            (
                0xfffffc20c00003c0,
                0xfffffc200166d0e0,
                0xfffffc20c08b50e0,
                0xfffffc20002a0000,
                0x7000500000,
                0xfffffc20c0603e40,
                0xfffffc20c05e96c0,
                0xfffffc20c0870200,
                0xfffffc2001638008,
                0xfffffc20c08d0000,
                0xfffffc2001688000,
                0xfffffc2001698000,
                5,
                0x159,
                0xffff080200000001,
            )
        } else {
            (
                0xfffffc20c0000540,
                0xfffffc20016ba870,
                0xfffffc20c0912870,
                0xfffffc20002f0000,
                0x7000550000,
                0xfffffc20c0604ec0,
                0xfffffc20c05e9c80,
                0xfffffc20c0870300,
                0xfffffc200163800c,
                0xfffffc20c0908000,
                0xfffffc20016a8000,
                0xfffffc20016c8000,
                7,
                0x183,
                0xffff080300000001,
            )
        };
        let control = 0xfffffc20c07b80c0;
        for (address, size) in [
            (queue, 0xc0),
            (pointers, 0x80),
            (ring, 0x2870),
            (optional, 0xc0),
            (event, 0x400),
            (scheduler, 0x100),
            (scheduler_slot, 4),
            (support, PAGE),
            (state, PAGE),
            (zero, PAGE),
            (control, 0x40),
        ] {
            vm.ensure_firmware(memory, address, size)?;
        }
        vm.alias_firmware(memory, high, low, 8 * PAGE)?;
        let write =
            |memory: &mut Memory, address, bytes: &[u8]| vm.write(memory, 2, address, bytes);
        if slot == 1 {
            write(memory, control, &lifecycle::channel_control())?;
        }
        write(
            memory,
            scheduler,
            &c::Scheduler {
                slot: scheduler_slot,
                work_id: slot as u32,
                phase: 0,
                job_list: 0,
                node_id: 0,
                completion_kind: 0,
            }
            .build(),
        )?;
        write(memory, scheduler_slot, &1u32.to_le_bytes())?;
        cm::Support {
            compact: None,
            header: 3,
            word_08: 0,
            word_10: 2,
            resource_class: 0x15,
            word_20: if slot == 2 {
                Some(0x0000159000000090)
            } else {
                None
            },
            word_28: if slot == 2 {
                Some(0x0000150000000000)
            } else {
                None
            },
            client_state: OPERAND_TABLE,
            firmware_state: state,
            cursor: 0xa8,
            field_54: slot as u32,
            field_5c: 1,
            final_kind: 2,
        }
        .build(page)
        .map_err(|_| EINVAL)?;
        write(memory, support, page)?;
        cm::shared_state(page, slot as u32 + 1).map_err(|_| EINVAL)?;
        write(memory, state, page)?;
        page.fill(0);
        write(memory, zero, page)?;
        page[..0x60].copy_from_slice(&q::pointers(u32::MAX));
        c::u32_at(page, 0x60, 0x500);
        write(memory, pointers, &page[..0x80])?;
        page.fill(0);
        write(memory, ring, &page[..0x2870])?;
        write(
            memory,
            queue,
            &q::Record {
                pointers,
                ring,
                job_list: JOB_LIST + 0x30,
                context: control,
                uuid,
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
        c::Context {
            descriptor: DESCRIPTOR + slot * 0x1040,
            queue,
            grid,
            flags: 0x1000000000000000,
            word_220: word220,
            word_330: 0,
            word_338: 8,
            word_350: 0x000110038001a002 + slot * (0x1040 / 0x20),
            word_358: 0x000020038001a03b + slot * (0x1040 / 0x20),
            word_378: 0x003fffffffffffff,
            item_index: 0,
            points: None,
            event_slot: None,
            completion: None,
        }
        .build(&mut page[0x200..0x400])
        .map_err(|_| EINVAL)?;
        write(memory, high, page)?;
    }
    Ok(())
}
