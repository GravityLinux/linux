// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! First direct-compute graph and its owned caller mappings. Addresses and
//! field values follow native_add3's direct queue zero, without its workload.

use super::{
    g17p_abi as abi, g17p_compute as c, g17p_compute_memory as cm, g17p_drm::Object,
    g17p_image::Image, g17p_memory::Memory, g17p_queue as q, g17p_user_vm::UserVm, g17p_vm::Vm,
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
}
impl Client {
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
}
pub(crate) struct Submission {
    pub(crate) client: Client,
    pub(crate) publication: q::Publication,
    pub(crate) channel: abi::Channel,
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
) -> Result<Submission> {
    let mut page = KVVec::with_capacity(PAGE, GFP_KERNEL)?;
    page.resize(PAGE, 0, GFP_KERNEL)?;
    let lists = client_storage(memory, &mut client.root, 0x7000000000, 0x200000)?;
    let operands = client_storage(memory, &mut client.root, OPERAND_TABLE, 0x10000)?;
    client_storage(memory, &mut client.root, STATE, 0x14000)?;
    // Source maps a zero native-control page at 0x70013a0000 before
    // allocating operands, which replace that leaf. Map the final owner once:
    // the control address and blank low queue contexts lie in these tranches.
    for i in 0..21 {
        client_storage(memory, &mut client.root, OPERANDS + i * 0x108000, 0x100000)?;
    }
    client_storage(memory, &mut client.root, ROBUSTNESS, PAGE)?;
    client_storage(memory, &mut client.root, parameters.preempt, 0xc000)?;
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
        (STATUS[0], 8),
        (STATUS[1], 8),
    ] {
        vm.ensure_firmware(memory, va, size)?;
    }
    vm.alias_firmware(memory, DESCRIPTOR, DESCRIPTOR_LOW, PAGE)?;
    vm.alias_firmware(memory, CTX_HIGH, CTX_LOW, 8 * PAGE)?;
    let write = |memory: &mut Memory, va, bytes: &[u8]| vm.write(memory, 2, va, bytes);
    let mut dispatch = [0u8; 0x20];
    abi::compute_dispatch(&mut dispatch).map_err(|_| EINVAL)?;
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
    page[0x100..0x200].copy_from_slice(
        &c::Scheduler {
            slot: SHARED_STATE + 4,
            work_id: 0,
            phase: 0,
            job_list: 0,
            node_id: 0,
            completion_kind: 0,
        }
        .build(),
    );
    write(memory, SCHEDULERS, &page)?;
    page.fill(0);
    c::u32_at(&mut page, 4, 1);
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
    for address in [0xfffffc20001c8008, 0xfffffc20c07c0008, STATUS[0], STATUS[1]] {
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
    let registers = c::Program {
        preempt: parameters.preempt,
        cdm: parameters.cdm,
        identity: 0x010001d7020001dc,
        context: 2,
        ordinal: 0,
        robustness: ROBUSTNESS,
        operand_state: STATE,
        usc_exec_base: c::USC_EXEC_BASE,
        helper_binary: 0,
        helper_data: 0,
        helper_cfg: 0,
        execution_gate: 1,
    }
    .build()
    .map_err(|_| EINVAL)?;
    c::Descriptor {
        scheduler: SCHEDULER,
        low_alias: DESCRIPTOR_LOW,
        cdm_terminator: parameters.end - 4,
        sequence: 0,
        context: 2,
        grid: 4,
        dispatch: [0xfffffc20001c8008, 0xfffffc20c07c0008],
        status: STATUS,
        timestamps: [0; 2],
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
    .build(&mut page, &registers)
    .map_err(|_| EINVAL)?;
    write(memory, DESCRIPTOR, &page)?;
    page.fill(0);
    write(memory, EVENT, &page[..0x400])?;
    let channel = image.graph.channels[0][q::COMPUTE_CHANNEL];
    if channel.ring != 0xfffffc20c07a1dc0 {
        return Err(EINVAL);
    }
    let publication = q::Stage {
        queue: QUEUE,
        pointers: POINTERS,
        item_ring: RING,
        item_capacity: 0x2870 / 8,
        write_index: 0,
        channel_ring: channel.ring,
        channel_producer: channel.states[2],
        counters: q::Counters::new([0; 3]).map_err(|_| EINVAL)?,
        slot: Some(0),
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
    for context in 0..3 {
        let high = memory.allocate(PAGE)?;
        memory.clean(high, PAGE)?;
        memory.write64(ttbs + context * 16 + 8, (context << 48) | high | 1)?;
    }
    memory.write64(ttbs + 2 * 16, (2 << 48) | client.root.root() | 1)?;
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
    })
}
