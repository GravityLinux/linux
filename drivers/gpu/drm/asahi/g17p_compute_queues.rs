// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Independently retained ordinary compute queue generations. A generation
//! owns its ASID, private operand/preemption state, firmware queue and storage.
//! Rollover chooses another owner while the old generation is still live.

use super::{
    g17p_abi::Channel,
    g17p_compute as c, g17p_compute_memory as cm,
    g17p_compute_runtime::{self as runtime, Client, ClientLease, Parameters, TransportPool},
    g17p_memory::{self, Memory},
    g17p_queue as q,
    g17p_user_vm::UserVm,
    g17p_vm::Vm,
};
use kernel::{
    dma_fence::{Fence, RawDmaFence},
    prelude::*,
};

const PAGE: usize = 0x4000;
// Contexts zero/one and the legacy bootstrap's two/three remain reserved.
const FIRST_ASID: u16 = 4;
const CONTEXTS: usize = 64 - FIRST_ASID as usize;
const FIRST_GRID: u8 = 32;
const WINDOW: u32 = super::g17p_compute_lifecycle::TRANSPORT_INTERVAL;
const RECORDS: u32 = 256;
const FW_BASE: u64 = 0xfffffc20e0000000;
const LOW_BASE: u64 = 0x7200000000;
const STRIDE: u64 = 0x200000;

pub(crate) type Key = (u64, u32);
#[derive(Clone, Copy)]
struct Layout {
    high: u64,
    low: u64,
    grid: u8,
    asid: u16,
}
impl Layout {
    fn new(index: usize) -> Result<Self> {
        if index >= CONTEXTS {
            return Err(EBUSY);
        }
        Ok(Self {
            high: FW_BASE + index as u64 * STRIDE,
            low: LOW_BASE + index as u64 * STRIDE,
            grid: FIRST_GRID + index as u8,
            asid: FIRST_ASID + index as u16,
        })
    }
    fn queue(self) -> u64 {
        self.high + 0x100000
    }
    fn pointers(self) -> u64 {
        self.high + 0x104000
    }
    fn ring(self) -> u64 {
        self.high + 0x108000
    }
    fn context(self) -> u64 {
        self.high + 0x110000
    }
    fn context_low(self) -> u64 {
        self.low + 0x110000
    }
    fn support(self) -> u64 {
        self.high + 0x140000
    }
    fn support_state(self) -> u64 {
        self.high + 0x144000
    }
    fn zero(self) -> u64 {
        self.high + 0x148000
    }
    fn completion_scratch(self, slot: u32) -> u64 {
        self.zero() + slot as u64 * 0x40
    }
    fn control(self) -> u64 {
        self.high + 0x14c000
    }
    fn job_list(self) -> u64 {
        self.high + 0x150000
    }
    fn optional(self) -> u64 {
        self.high + 0x154000
    }
    fn scheduler(self, slot: u32) -> u64 {
        self.high + 0x160000 + slot as u64 * 0x100
    }
    fn scheduler_slot(self, slot: u32) -> u64 {
        self.high + 0x170000 + slot as u64 * 64
    }
    fn event(self, slot: u32) -> u64 {
        self.high + 0x1a0000 + slot as u64 * 0x400
    }
    fn status(self, slot: u32) -> u64 {
        self.high + 0x178000 + slot as u64 * 64
    }
    fn dispatch(self, slot: u32) -> [u64; 2] {
        [
            self.high + 0x17c000 + slot as u64 * 64,
            self.high + 0x180000 + slot as u64 * 64,
        ]
    }
    fn descriptor(self, slot: u32) -> u64 {
        self.high + slot as u64 * 0x1040
    }
    fn descriptor_low(self, slot: u32) -> u64 {
        self.low + slot as u64 * 0x1040
    }
}

pub(crate) struct Ticket {
    pub(crate) client: ClientLease,
    pub(crate) publication: q::Publication,
    pub(crate) channel: Channel,
    pub(crate) pointers: u64,
    pub(crate) status: [u64; 2],
    pub(crate) timestamps: [u64; 2],
    pub(crate) grid: u8,
    pub(crate) value: u32,
    pub(crate) queue: usize,
}
pub(crate) struct Queue {
    layout: Layout,
    key: Key,
    pub(crate) client: Client,
    preempt: u64,
    priority: u32,
    count: u32,
    window: u32,
    publication: Option<q::Publication>,
    pointers: u64,
    ring: u64,
    transport: TransportPool,
    fences: KVec<Fence>,
    upper: u64,
}
pub(crate) struct Queues {
    owners: KVec<Queue>,
}
impl Queues {
    pub(crate) fn new() -> Self {
        Self {
            owners: KVec::new(),
        }
    }
    pub(crate) fn any(&self) -> bool {
        !self.owners.is_empty()
    }
    pub(crate) fn grid_mask(&self) -> u128 {
        self.owners
            .iter()
            .fold(0, |mask, q| mask | (1u128 << q.layout.grid))
    }
    pub(crate) fn pending(&self) -> bool {
        self.owners.iter().any(|q| !q.idle())
    }
    pub(crate) fn require_owner_idle(&self, owner: (u64, u32)) -> Result {
        if self
            .owners
            .iter()
            .any(|q| q.client.owner == owner && !q.idle())
        {
            return Err(EBUSY);
        }
        Ok(())
    }
    pub(crate) fn unbind(&mut self, owner: (u64, u32), start: u64, size: u64) -> Result {
        self.require_owner_idle(owner)?;
        let end = start.checked_add(size).ok_or(EINVAL)?;
        for queue in self.owners.iter_mut().filter(|q| q.client.owner == owner) {
            // Different idle generations can retain older binding snapshots.
            // Remove this range from every such owner using its own leaves.
            let mut expected = KVec::new();
            for binding in &queue.client.bindings {
                for address in (start.max(binding.0)..end.min(binding.0 + binding.1)).step_by(PAGE)
                {
                    expected.push((address, queue.client.root.pte(address)?), GFP_KERNEL)?;
                }
            }
            queue
                .client
                .unbind(start, size, &expected, false, &[queue.layout.asid])?;
        }
        Ok(())
    }
    pub(crate) fn fail(&self, error: Error) {
        for queue in &self.owners {
            for fence in &queue.fences {
                if unsafe { kernel::bindings::dma_fence_get_status(fence.raw()) } == 0 {
                    fence.set_error(error);
                    fence.signal();
                }
            }
        }
    }
    fn choose(&self, key: Key, client: &Client, p: &Parameters, priority: u32) -> Option<usize> {
        self.owners
            .iter()
            .position(|q| q.matches(key, client, p, priority) && q.window < WINDOW)
            .or_else(|| self.owners.iter().position(|q| q.key == key && q.idle()))
            // Rebind a fully retired owner before growing the permanent
            // backing pool. Live owners never block another queue: allocate
            // another owner when none can be reused safely.
            .or_else(|| self.owners.iter().position(Queue::idle))
            .or_else(|| (self.owners.len() < CONTEXTS).then_some(self.owners.len()))
    }
    pub(crate) fn can_stage(
        &self,
        key: Key,
        client: &Client,
        p: &Parameters,
        priority: u32,
    ) -> bool {
        self.choose(key, client, p, priority).is_some()
    }
    pub(crate) fn client(
        &self,
        key: Key,
        client: &Client,
        p: &Parameters,
        priority: u32,
    ) -> Option<&Client> {
        // Use the same owner stage() will select, including an idle owner
        // whose full window can rotate without rebuilding its retained VM.
        let index = self.choose(key, client, p, priority)?;
        self.owners
            .get(index)
            .filter(|q| q.matches(key, client, p, priority))
            .map(|q| &q.client)
    }
    pub(crate) fn stage(
        &mut self,
        memory: &mut Memory,
        vm: &mut Vm,
        ttbs: u64,
        channel: Channel,
        key: Key,
        mut client: Option<Client>,
        reference: &Client,
        p: &Parameters,
        priority: u32,
        dependencies: &[(u8, u32)],
        fence: Fence,
    ) -> Result<Ticket> {
        let index = self.choose(key, reference, p, priority).ok_or(EBUSY)?;
        if index == self.owners.len() {
            self.owners.reserve(1, GFP_KERNEL)?;
            let queue = Queue::new(
                memory,
                vm,
                Layout::new(index)?,
                key,
                client.take().ok_or(EINVAL)?,
                p,
                priority,
            )?;
            self.owners.push(queue, GFP_KERNEL)?;
            self.owners[index].install(memory, ttbs)?;
        } else {
            let queue = &mut self.owners[index];
            if !queue.matches(key, reference, p, priority) {
                if !queue.idle() {
                    return Err(EBUSY);
                }
                queue.replace_client(
                    memory,
                    vm,
                    ttbs,
                    key,
                    client.take().ok_or(EINVAL)?,
                    p,
                    priority,
                )?;
            }
            if queue.window == WINDOW {
                queue.rotate(memory, vm)?;
            }
        }
        self.owners[index].stage(memory, vm, channel, index, p, dependencies, fence)
    }
}
impl Queue {
    fn idle(&self) -> bool {
        self.fences
            .iter()
            .all(|f| unsafe { kernel::bindings::dma_fence_get_status(f.raw()) } > 0)
    }
    fn matches(&self, key: Key, client: &Client, p: &Parameters, priority: u32) -> bool {
        self.key == key
            && self.preempt == p.preempt
            && self.priority == priority
            && self.client.owner == client.owner
            && self.client.bindings == client.bindings
            && self.client.buffers.len() == client.buffers.len()
            && self
                .client
                .buffers
                .iter()
                .zip(&client.buffers)
                .all(|(a, b)| core::ptr::eq(&**a, &**b))
    }
    fn new(
        memory: &mut Memory,
        vm: &mut Vm,
        layout: Layout,
        key: Key,
        mut client: Client,
        p: &Parameters,
        priority: u32,
    ) -> Result<Self> {
        runtime::build_independent_client(memory, &mut client, p)?;
        for (address, size) in [
            (layout.queue(), PAGE),
            (layout.pointers(), PAGE),
            (layout.ring(), PAGE),
            (layout.context(), 8 * PAGE),
            (layout.support(), PAGE),
            (layout.support_state(), PAGE),
            (layout.zero(), PAGE),
            (layout.control(), PAGE),
            (layout.job_list(), PAGE),
            (layout.optional(), PAGE),
            (layout.high + 0x160000, 0x24000),
            (layout.high + 0x1a0000, 0x40000),
        ] {
            vm.ensure_firmware(memory, address, size)?;
        }
        vm.alias_firmware(memory, layout.context(), layout.context_low(), 8 * PAGE)?;
        let mut page = KVVec::with_capacity(PAGE, GFP_KERNEL)?;
        page.resize(PAGE, 0, GFP_KERNEL)?;
        cm::Support {
            compact: None,
            header: u64::from(layout.asid),
            word_08: 1,
            word_10: 2,
            resource_class: 0x15,
            word_20: Some(0x150000000000),
            word_28: Some(0x150000000000),
            client_state: 0x7000208000,
            firmware_state: layout.support_state(),
            cursor: 0xa8,
            field_54: 0,
            field_5c: 1,
            final_kind: 2,
        }
        .build(&mut page)
        .map_err(|_| EINVAL)?;
        vm.write(memory, 2, layout.support(), &page)?;
        cm::shared_state(&mut page, 1).map_err(|_| EINVAL)?;
        vm.write(memory, 2, layout.support_state(), &page)?;
        vm.write(
            memory,
            2,
            layout.control(),
            &super::g17p_dependency::channel_control(),
        )?;
        vm.write(
            memory,
            2,
            layout.job_list(),
            &q::job_list(layout.job_list()),
        )?;
        vm.write(
            memory,
            2,
            layout.pointers(),
            &super::g17p_compute_lifecycle::transport_pointers(),
        )?;
        let mut record = q::Record {
            pointers: layout.pointers(),
            ring: layout.ring(),
            job_list: layout.job_list(),
            context: layout.control(),
            uuid: 0x200 + u32::from(layout.grid),
            priority,
            prio5: priority,
            unk_2c: priority,
            unk_38: 0,
            unk_30: None,
            unk_94: 0,
            sentinel_size: 2,
        }
        .build()
        .map_err(|_| EINVAL)?;
        record[0x28..0x48].copy_from_slice(&q::priority_profile(priority).map_err(|_| EINVAL)?);
        vm.write(memory, 2, layout.queue(), &record)?;
        let upper = memory.allocate(PAGE)?;
        memory.clean(upper, PAGE)?;
        vm.flush_tables(memory)?;
        Ok(Self {
            layout,
            key,
            client,
            preempt: p.preempt,
            priority,
            count: 0,
            window: 0,
            publication: None,
            pointers: layout.pointers(),
            ring: layout.ring(),
            transport: TransportPool::new(layout.high + 0x190000),
            fences: KVec::new(),
            upper,
        })
    }
    // Session owns the complete root before either TTB becomes visible.
    fn install(&self, memory: &mut Memory, ttbs: u64) -> Result {
        let at = ttbs + u64::from(self.layout.asid) * 16;
        if memory.read64(at)? & 1 != 0 {
            return Err(EBUSY);
        }
        memory.write64(at + 8, (u64::from(self.layout.asid) << 48) | self.upper | 1)?;
        memory.write64(
            at,
            (u64::from(self.layout.asid) << 48) | self.client.root.root() | 1,
        )?;
        memory.clean(at, 16)?;
        UserVm::invalidate(self.layout.asid);
        Ok(())
    }
    fn replace_client(
        &mut self,
        memory: &mut Memory,
        vm: &Vm,
        ttbs: u64,
        key: Key,
        client: Client,
        p: &Parameters,
        priority: u32,
    ) -> Result {
        if !self.idle() {
            return Err(EBUSY);
        }
        let profile = q::priority_profile(priority).map_err(|_| EINVAL)?;
        let at = ttbs + u64::from(self.layout.asid) * 16;
        if memory.read64(at)? != ((u64::from(self.layout.asid) << 48) | self.client.root.root() | 1)
            || memory.read64(at + 8)? != ((u64::from(self.layout.asid) << 48) | self.upper | 1)
        {
            return Err(EIO);
        }
        // Reuse this owner's allocator and private backing. A changed reserved
        // aperture moves the same save/robustness leaves after this owner retires.
        let mut private: KVec<(u64, u64, u64)> = KVec::new();
        if self.preempt != p.preempt {
            let mut offsets = KVec::with_capacity(256 * 3 + 2, GFP_KERNEL)?;
            for slot in 0..256u64 {
                for page in [0, 0x4000, 0x8000] {
                    offsets.push(slot * 0x78000 + page, GFP_KERNEL)?;
                }
            }
            for offset in [0x100000, 0x108000] {offsets.push(offset, GFP_KERNEL)?;}
            let mut leaves = KVec::with_capacity(offsets.len(), GFP_KERNEL)?;
            for &offset in &offsets {
                let old = self.preempt + offset;
                let leaf = self.client.root.pte(old)?;
                if leaf == 0 {
                    return Err(EIO);
                }
                leaves.push(leaf, GFP_KERNEL)?;
                private.push((old, leaf, 0), GFP_KERNEL)?;
            }
            for (&offset, &leaf) in offsets.iter().zip(leaves.iter()) {
                let address = p.preempt + offset;
                if let Some(row) = private.iter_mut().find(|r| r.0 == address) {
                    row.2 = leaf;
                } else {
                    let old = self.client.root.pte(address)?;
                    if old != 0
                        && !self
                            .client
                            .bindings
                            .iter()
                            .any(|b| address >= b.0 && address < b.0 + b.1)
                    {
                        return Err(EBUSY);
                    }
                    private.push((address, old, leaf), GFP_KERNEL)?;
                }
            }
        }
        self.client
            .prepare_rebind(&client, false, &[self.layout.asid], &private)?
            .commit();
        self.client.adopt_rebound(client);
        vm.write(memory, 2, self.layout.queue() + 0x28, &profile)?;
        self.key = key;
        self.preempt = p.preempt;
        self.priority = priority;
        Ok(())
    }
    fn rotate(&mut self, memory: &mut Memory, vm: &mut Vm) -> Result {
        if !self.idle() {
            return Err(EBUSY);
        }
        let previous = self.publication.ok_or(EIO)?;
        let next = self.transport.switch(
            memory,
            vm,
            self.layout.queue(),
            self.pointers,
            self.ring,
            previous.write_after,
        )?;
        self.pointers = next[0];
        self.ring = next[1];
        self.window = 0;
        Ok(())
    }
    fn stage(
        &mut self,
        memory: &mut Memory,
        vm: &mut Vm,
        channel: Channel,
        index: usize,
        p: &Parameters,
        dependencies: &[(u8, u32)],
        fence: Fence,
    ) -> Result<Ticket> {
        self.fences
            .retain(|f| unsafe { kernel::bindings::dma_fence_get_status(f.raw()) } == 0);
        self.fences.reserve(1, GFP_KERNEL)?;
        let ordinal = self.count;
        if ordinal >= 0xfffffe {
            return Err(EOVERFLOW);
        }
        let slot = ordinal % RECORDS;
        let l = self.layout;
        let descriptor = l.descriptor(ordinal % 240);
        let low = l.descriptor_low(ordinal % 240);
        // The firmware accesses the entire 0x1040-byte record, including
        // its trailing state. Mapping only the 0x1000 host-authored bytes
        // faults when those bytes end exactly at a 16 KiB page boundary.
        vm.alias_firmware(memory, descriptor, low, 0x1040)?;
        let mut counters = [0; 3];
        for (v, at) in counters.iter_mut().zip(channel.states) {
            *v = memory.read_firmware32(vm.physical(memory, 2, at)?)?;
        }
        let counters = q::Counters::new(counters).map_err(|_| EIO)?;
        counters.slot().map_err(|_| EBUSY)?;
        let write_index = if self.window == 0 {
            0
        } else {
            self.publication.ok_or(EIO)?.write_after
        };
        // The host event prefix is only 0x40 bytes, but firmware may
        // append records into the complete 0x400-byte event allocation.
        vm.write(memory, 2, l.event(slot), &[0; 0x400])?;
        // Native descriptors use a separate zero-initialized 64-byte
        // completion record for every simultaneously live command.
        vm.write(memory, 2, l.completion_scratch(slot), &[0; 0x40])?;
        let dispatch = l.dispatch(slot);
        let status = [l.status(slot), l.status(slot) + 8];
        let scheduler = c::Scheduler {
            slot: l.scheduler_slot(slot),
            work_id: ordinal,
            phase: 0,
            job_list: 0,
            node_id: 0,
            completion_kind: 0,
        }
        .build();
        vm.write(memory, 2, l.scheduler(slot), &scheduler)?;
        vm.write(memory, 2, l.scheduler_slot(slot), &1u32.to_le_bytes())?;
        vm.write(memory, 2, l.support_state(), &(ordinal + 1).to_le_bytes())?;
        for address in dispatch {
            vm.write(memory, 2, address, &[0; 8])?;
        }
        vm.write(memory, 2, status[0], &[0; 16])?;
        let regs = c::Program {
            preempt: p.preempt + u64::from(slot) * 0x78000,
            cdm: p.cdm,
            identity: 0x0200020803000247u64 + u64::from(ordinal) * 0x200000001,
            context: u32::from(l.asid),
            ordinal,
            robustness: p.preempt + 0x100000 + u64::from(slot) * 0x40,
            // The operand allocator state belongs to this queue/root,
            // not to the alternating per-command preemption save areas.
            operand_state: 0x7000220000,
            usc_exec_base: c::USC_EXEC_BASE,
            helper_binary: 0,
            helper_data: 0,
            helper_cfg: 0,
            execution_gate: 1,
        }
        .build()
        .map_err(|_| EINVAL)?;
        let mut page = KVVec::with_capacity(PAGE, GFP_KERNEL)?;
        page.resize(PAGE, 0, GFP_KERNEL)?;
        c::Descriptor {
            scheduler: l.scheduler(slot),
            low_alias: low,
            cdm_terminator: p.end.checked_sub(4).ok_or(EINVAL)?,
            sequence: u64::from(ordinal),
            context: u32::from(l.asid),
            grid: u32::from(l.grid),
            dispatch,
            status,
            timestamps: p.timestamps,
            shared_control: l.support(),
            zero_page: l.completion_scratch(slot),
            support_control: 0xe0a00001,
            support_flags: 0,
            ordinal,
            queue_submission: ordinal + 1,
            queue_ordinal: 0,
            submission_index: ordinal + 1,
            sampler_array: p.sampler,
            sampler_count: p.sampler_count,
        }
        .build(&mut page, &regs)
        .map_err(|_| EINVAL)?;
        vm.write(memory, 2, descriptor, &page[..0x1000])?;
        c::Context {
            descriptor,
            queue: l.queue(),
            grid: u32::from(l.grid),
            flags: 0x1000000000000000,
            word_220: 0xffff080200000001,
            word_330: 0,
            word_338: 8,
            word_350: 0x0001100000000000 | ((low + 0x40) >> 5),
            word_358: 0x0000200000000000 | ((low + 0x760) >> 5),
            word_378: 0x003fffffffffffff,
            item_index: ordinal,
            points: None,
            event_slot: None,
            completion: None,
        }
        .build(&mut page[..0x200])
        .map_err(|_| EINVAL)?;
        c::context_dependencies(&mut page[..0x200], dependencies).map_err(|_| EINVAL)?;
        let context = l.context() + c::context_offset(ordinal, RECORDS).map_err(|_| EINVAL)? as u64;
        runtime::write_context_item(memory, vm, context, &page[..0x200], ordinal >= RECORDS)?;
        if ordinal == 0 {
            let optional = c::Optional {
                context_low: l.context_low(),
                context_high: l.context(),
                grid: u32::from(l.grid),
                ordinal: 0x29,
                shared_control: l.support(),
                channel_control: l.control(),
                uuid: 0x200 + u32::from(l.grid),
                field_46: 0,
                field_1e: 2,
                field_32: u32::from(l.asid),
                field_56: 2,
                field_5e: 2,
                first: true,
                item_index: 0,
            }
            .build();
            vm.write(memory, 2, l.optional(), &optional)?;
        }
        vm.flush_tables(memory)?;
        // New firmware aliases are global mappings in context zero.
        // SAFETY: The runtime lock serializes host page-table publication.
        unsafe {
            core::arch::asm!(
                ".inst 0xd508811f",
                "dsb sy",
                "isb",
                options(nostack, preserves_flags)
            );
        }
        self.client.cache(false)?;
        let client = self.client.lease()?;
        let initial = [descriptor, l.optional(), l.event(slot)];
        let retained = [descriptor, l.event(slot)];
        let publication = q::Stage {
            queue: l.queue(),
            pointers: self.pointers,
            item_ring: self.ring,
            item_capacity: 0x500,
            write_index,
            channel_ring: channel.ring,
            channel_producer: channel.states[2],
            counters,
            slot: None,
            items: if ordinal == 0 { &initial } else { &retained },
            group: ordinal + 1,
            grid: u32::from(l.grid),
            kind: q::Kind::Compute,
            first: ordinal == 0,
            in_place: false,
            announce: false,
            defer_inner: true,
            defer_outer: true,
            event_subtype: None,
            event_counter: None,
            event_counter_low: 2,
        }
        .publish(&mut Writer { memory, vm })
        .map_err(|e| match e {
            q::StageError::Access(e) => e,
            _ => EINVAL,
        })?;
        self.fences.push(fence, GFP_KERNEL)?;
        self.publication = Some(publication);
        self.count += 1;
        self.window += 1;
        Ok(Ticket {
            client,
            publication,
            channel,
            pointers: self.pointers,
            status,
            timestamps: p.timestamps,
            grid: l.grid,
            value: self.count,
            queue: index,
        })
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
