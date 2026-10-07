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
#[derive(Clone, Copy, PartialEq, Eq)]
struct Layout {
    high: u64,
    low: u64,
    grid: u8,
    asid: u16,
}
impl Layout {
    fn new(index: usize, asid: u16) -> Result<Self> {
        if index >= CONTEXTS {
            return Err(EBUSY);
        }
        Ok(Self {
            high: FW_BASE + index as u64 * STRIDE,
            low: LOW_BASE + index as u64 * STRIDE,
            grid: FIRST_GRID + index as u8,
            asid,
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

/// Immutable physical-owner/ordinal seed. Descriptor serialization takes
/// no firmware or caller memory borrow, and runs outside the runtime mutex.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct PreparationSeed {
    layout: Layout,
    key: Key,
    ordinal: u32,
    parameters: Parameters,
    priority: u32,
    dependencies: [(u8, u32); 3],
    dependency_count: usize,
    optional: bool,
    first: bool,
}
pub(crate) struct PreparedWork {
    seed: PreparationSeed,
    descriptor: KVVec<u8>,
    context: KVVec<u8>,
    optional: Option<[u8; 0xc0]>,
}
impl PreparationSeed {
    pub(crate) fn prepare(self) -> Result<KBox<PreparedWork>> {
        let l = self.layout;
        let ordinal = self.ordinal;
        let slot = ordinal % RECORDS;
        let p = &self.parameters;
        let dependencies = &self.dependencies[..self.dependency_count];
        let descriptor = l.descriptor(ordinal % 240);
        let low = l.descriptor_low(ordinal % 240);
        let dispatch = l.dispatch(slot);
        let status = [l.status(slot), l.status(slot) + 8];
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
        let descriptor_body = page;
        let mut page = KVVec::with_capacity(0x200, GFP_KERNEL)?;
        page.resize(0x200, 0, GFP_KERNEL)?;
        c::Context {
            descriptor,
            queue: l.queue(),
            grid: u32::from(l.grid),
            flags: 0x1000000000000000,
            // Keep receiver slots disjoint from all render pools. Compute grids begin
            // at 32; use the corresponding free event slot instead of the
            // legacy fixed slot 2 shared by every independent compute owner.
            // This is the receiver event slot, separate from the root ASID.
            word_220: 0xffff080000000001 | (u64::from(l.grid) << 32),
            word_330: 0,
            // Native +0x138 tracks the descriptor UAT ASID (1 << ASID).
            // Independent owners reserve distinct roots; the legacy ASID3
            // constant cannot describe each installed compute owner.
            word_338: 1u64 << l.asid,
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
        let context_body = page;
        let optional_body = if self.optional {
            Some(c::Optional {
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
                first: self.first,
                item_index: ordinal,
            }
            .build())
        } else { None };
        Ok(KBox::new(PreparedWork { seed: self, descriptor: descriptor_body,
            context: context_body, optional: optional_body }, GFP_KERNEL)?)
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
    needs_optional: bool,
    needs_first: bool,
    reconfiguration: Option<Maintenance>,
}
struct Maintenance {
    target: Key,
    next: u32,
    published: Option<([u8; 2], u8)>,
    acknowledged: [bool; 2],
    started: kernel::time::Instant<kernel::time::Monotonic>,
}
pub(crate) struct Queues {
    owners: KVec<Queue>,
    reserved_asids: u64,
}
impl Queues {
    pub(crate) fn new() -> Self {
        Self {
            owners: KVec::new(),
            reserved_asids: 0xf,
        }
    }
    pub(crate) fn asid_mask(&self) -> u64 {
        self.owners.iter().fold(self.reserved_asids, |mask, q| mask | (1u64 << q.layout.asid))
    }
    pub(crate) fn reserve_render_asid(&mut self, asid: u16) -> Result {
        if asid >= 64 || self.asid_mask() & (1u64 << asid) != 0 { return Err(EBUSY); }
        self.reserved_asids |= 1u64 << asid;
        Ok(())
    }
    fn free_asid(&self) -> Option<u16> {
        (FIRST_ASID..64).find(|asid| self.asid_mask() & (1u64 << asid) == 0)
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
    pub(crate) fn retain_dependency(&mut self, grid: u8, fence: &Fence) -> Result<bool> {
        let Some(queue) = self.owners.iter_mut().find(|queue| queue.layout.grid == grid) else { return Ok(false); };
        if !queue.fences.iter().any(|held| held.raw() == fence.raw()) {
            queue.fences.push(fence.clone(), GFP_KERNEL)?;
        }
        Ok(true)
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
        let eligible = |q: &Queue| q.reconfiguration.as_ref().is_none_or(|m| m.target == key);
        self.owners.iter().position(|q| q.reconfiguration.as_ref().is_some_and(|m| m.target == key))
            .or_else(|| self.owners
            .iter()
            .position(|q| eligible(q) && q.matches(key, client, p, priority) && q.window < WINDOW))
            .or_else(|| self.owners.iter().position(|q| eligible(q) && q.key == key && q.idle()))
            // Rebind a fully retired owner before growing the permanent
            // backing pool. Live owners never block another queue: allocate
            // another owner when none can be reused safely.
            .or_else(|| self.owners.iter().position(|q| eligible(q) && q.idle()))
            .or_else(|| (self.owners.len() < CONTEXTS && self.free_asid().is_some()).then_some(self.owners.len()))
    }
    pub(crate) fn selected_grid(&self, key: Key, client: &Client, p: &Parameters, priority: u32) -> Option<u8> {
        self.choose(key, client, p, priority).map(|index| FIRST_GRID + index as u8)
    }
    pub(crate) fn preparation_seed(&self, key: Key, client: &Client,
        p: &Parameters, priority: u32, dependencies: &[(u8, u32)]) -> Result<Option<PreparationSeed>> {
        if dependencies.len() > 3 { return Err(EINVAL); }
        let Some(index) = self.choose(key, client, p, priority) else { return Ok(None); };
        let (layout, ordinal, optional, first) = if let Some(q) = self.owners.get(index) {
            (q.layout, q.count, q.needs_optional || !q.matches(key, client, p, priority),
                q.count == 0 || q.needs_first || q.reconfiguration.is_some())
        } else {
            (Layout::new(index, self.free_asid().ok_or(EBUSY)?)?, 0, true, true)
        };
        let mut points = [(0, 0); 3];
        points[..dependencies.len()].copy_from_slice(dependencies);
        Ok(Some(PreparationSeed { layout, key, ordinal, parameters: *p, priority,
            dependencies: points, dependency_count: dependencies.len(), optional, first }))
    }
    pub(crate) fn prepared_matches(&self, plan: &PreparedWork, key: Key, client: &Client,
        p: &Parameters, priority: u32, dependencies: &[(u8,u32)]) -> Result<bool> {
        Ok(self.preparation_seed(key, client, p, priority, dependencies)? == Some(plan.seed))
    }
    pub(crate) fn needs_private(&self, key: Key, client: &Client, p: &Parameters,
        priority: u32) -> bool {
        self.choose(key, client, p, priority).is_some_and(|i| i == self.owners.len())
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
    /// Only the retired owner being reassigned waits for its exact, live
    /// ownership-checked cleanup receipt. Backing remains pinned and unrelated
    /// owners are selectable throughout this nonblocking handoff.
    pub(crate) fn prepare_reconfiguration(
        &mut self, memory: &mut Memory, vm: &Vm, channel: Channel,
        key: Key, client: &Client, p: &Parameters, priority: u32,
    ) -> Result<(bool, bool)> {
        let Some(index) = self.choose(key, client, p, priority) else { return Ok((false, false)); };
        let Some(queue) = self.owners.get_mut(index) else { return Ok((true, false)); };
        if queue.matches(key, client, p, priority) && queue.reconfiguration.is_none() {
            return Ok((true, false));
        }
        if !queue.idle() { pr_err!("G17P: compute admission nonidle grid {} key {:?} target {:?}\n",queue.layout.grid,queue.key,key); return Err(EIO); }
        let maintenance = queue.reconfiguration.get_or_insert_with(|| Maintenance {
            target: key, next: 0, published: None, acknowledged: [false; 2],
            started: kernel::time::Instant::now(),
        });
        if maintenance.started.elapsed().as_millis() > 10000 { return Err(ETIMEDOUT); }
        let mut values = [0; 3];
        for (value, address) in values.iter_mut().zip(channel.states) {
            *value = memory.read_firmware32(vm.physical(memory, 2, address)?)?;
        }
        let counters = q::Counters::new(values).inspect_err(|e| pr_err!("G17P: compute control counters {:?} error {:?}\n",values,e)).map_err(|_| EIO)?;
        if let Some((before, target)) = maintenance.published {
            for i in 0..2 {
                maintenance.acknowledged[i] |= q::reached(before[i], values[i] as u8, target);
            }
            if !maintenance.acknowledged.iter().all(|v| *v) { return Ok((false, false)); }
            maintenance.next += 1;
            maintenance.published = None;
        }
        if maintenance.next == 1 { return Ok((true, false)); }
        if counters.available() == 0 { return Ok((false, false)); }
        let mut live = [0; 0x40];
        runtime::read_owned(memory,vm,queue.layout.control(),&mut live)?;
        let body = super::g17p_lifecycle::build_live_context_cleanup(queue.layout.control(),&live)?;
        let target = ((values[2] + 1) & 255) as u8;
        vm.write(memory, 2, channel.ring + u64::from(values[2]) * 0x40, &body)?;
        g17p_memory::sync();
        vm.write(memory, 2, channel.states[2], &u32::from(target).to_le_bytes())?;
        g17p_memory::sync();
        maintenance.published = Some(([values[0] as u8, values[1] as u8], target));
        maintenance.acknowledged = [false; 2];
        Ok((false, true))
    }
    pub(crate) fn stage(
        &mut self,
        memory: &mut Memory,
        vm: &mut Vm,
        ttbs: u64,
        channel: Channel,
        key: Key,
        mut client: Option<Client>,
        private_prepared: bool,
        rebind: Option<&mut PreparedComputeRebind>,
        prepared: KBox<PreparedWork>,
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
                Layout::new(index, self.free_asid().ok_or(EBUSY)?)?,
                key,
                client.take().ok_or(EINVAL)?,
                p,
                priority,
                private_prepared,
            )?;
            self.owners.push(queue, GFP_KERNEL)?;
            self.owners[index].install(memory, ttbs)?;
        } else {
            let queue = &mut self.owners[index];
            if let Some(receipt) = &queue.reconfiguration {
                if receipt.target != key || receipt.next != 1 || !queue.idle() { return Err(EBUSY); }
                // A first registration starts with an empty inner transport.
                // Switch while the old source header still mirrors its retired
                // read index; resetting that header first loses rollover proof.
                // Both finite backing slots and logical counters stay retained.
                queue.rotate(memory,vm).inspect_err(|e| pr_err!("G17P: compute cleanup rotate grid {} error {:?}\n",queue.layout.grid,e))?;
                queue.reset_registration(memory,vm).inspect_err(|e| pr_err!("G17P: compute reset registration grid {} error {:?}\n",queue.layout.grid,e))?;
                queue.reconfiguration = None;
            }
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
                    rebind,
                    p,
                    priority,
                ).inspect_err(|e| pr_err!("G17P: compute replace client grid {} error {:?}\n",queue.layout.grid,e))?;
            }
            if queue.window == WINDOW {
                queue.rotate(memory, vm).inspect_err(|e| pr_err!("G17P: compute rotate grid {} error {:?}\n",queue.layout.grid,e))?;
            }
        }
        self.owners[index].stage(memory, vm, channel, index, p, dependencies, prepared, fence)
            .inspect_err(|e| pr_err!("G17P: compute build grid {} error {:?}\n",self.owners[index].layout.grid,e))
    }
}
impl Queue {
    fn idle(&self) -> bool {
        // Own receipt fences signal after transport/status retirement. A
        // consumer Point signals after its own receipt retires (including
        // joined producer errors), or after rejection before publication.
        // Session-wide failure prevents further publication separately.
        // Match stage() reaping: a terminal negative lease is not pending.
        self.fences
            .iter()
            .all(|f| unsafe { kernel::bindings::dma_fence_get_status(f.raw()) } != 0)
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
        client: Client,
        p: &Parameters,
        priority: u32,
        private_prepared: bool,
    ) -> Result<Self> {
        // The ordinary worker constructs the large operand/save allocations
        // outside publication locking, then transfers their retained ownership.
        if !private_prepared { return Err(EIO); }
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
            needs_optional: true,
            needs_first: false,
            reconfiguration: None,
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
    fn reset_registration(&mut self, memory: &mut Memory, vm: &Vm) -> Result {
        // Cleanup consumed the exact firmware owner identity, after all its
        // fences completed. Rebuild source-authored registration metadata;
        // never return, repurpose or zero any caller/GPU executable pages.
        for offset in (0..8 * PAGE).step_by(PAGE) {
            let pa = vm.physical(memory,2,self.layout.context() + offset as u64)?;
            memory.zero(pa,PAGE)?;
            memory.clean(pa,PAGE)?;
        }
        vm.write(memory,2,self.layout.control(),&super::g17p_dependency::channel_control())?;
        vm.write(memory,2,self.layout.job_list(),&q::job_list(self.layout.job_list()))?;
        let mut record = q::Record {
            pointers:self.pointers,ring:self.ring,job_list:self.layout.job_list(),
            context:self.layout.control(),uuid:0x200+u32::from(self.layout.grid),
            priority:self.priority,prio5:self.priority,unk_2c:self.priority,
            unk_38:0,unk_30:None,unk_94:0,sentinel_size:2,
        }.build().map_err(|_|EINVAL)?;
        record[0x28..0x48].copy_from_slice(&q::priority_profile(self.priority).map_err(|_|EINVAL)?);
        vm.write(memory,2,self.layout.queue(),&record)?;
        self.needs_optional = true;
        self.needs_first = true;
        Ok(())
    }
    fn replace_client(
        &mut self,
        memory: &mut Memory,
        vm: &Vm,
        ttbs: u64,
        key: Key,
        client: Client,
        rebind: Option<&mut PreparedComputeRebind>,
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
            pr_err!("G17P: compute rebind TTB grid {} actual [{:#x},{:#x}] expected [{:#x},{:#x}]\n",
                self.layout.grid,memory.read64(at)?,memory.read64(at+8)?,
                (u64::from(self.layout.asid)<<48)|self.client.root.root()|1,
                (u64::from(self.layout.asid)<<48)|self.upper|1);
            return Err(EIO);
        }
        // Reuse this owner's allocator and private backing. A changed reserved
        // aperture moves the same save/robustness leaves after this owner retires.
        let plan = rebind.ok_or(EIO)?;
        if !plan.matches(self, key, &client, p, priority) { return Err(EBUSY); }
        plan.root.commit(&mut self.client.root)?;
        self.client.adopt_rebound(client);
        self.needs_optional = true;
        // Retained firmware queues keep their established scheduling state
        // when another caller requests the same priority family.
        if self.priority != priority {
            vm.write(memory, 2, self.layout.queue() + 0x28, &profile)?;
        }
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
        prepared: KBox<PreparedWork>,
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
        let seed = &prepared.seed;
        if seed.layout != l || seed.key != self.key || seed.ordinal != ordinal
            || seed.parameters != *p || seed.priority != self.priority
            || seed.optional != self.needs_optional || seed.first != (ordinal == 0 || self.needs_first)
            || &seed.dependencies[..seed.dependency_count] != dependencies { return Err(EIO); }
        vm.write(memory, 2, descriptor, &prepared.descriptor[..0x1000])?;
        let context = l.context() + c::context_offset(ordinal, RECORDS).map_err(|_| EINVAL)? as u64;
        runtime::write_context_item(memory, vm, context, &prepared.context, ordinal >= RECORDS - 1)?;
        if let Some(optional) = &prepared.optional {
            vm.write(memory, 2, l.optional(), optional)?;
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
        // Caller vmap validation and timestamp visibility were prepared
        // outside the runtime lock before seed revalidation above.
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
            items: if self.needs_optional { &initial } else { &retained },
            group: ordinal + 1,
            grid: u32::from(l.grid),
            kind: q::Kind::Compute,
            first: ordinal == 0 || self.needs_first,
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
        self.needs_optional = false;
        self.needs_first = false;
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

/// Retired compute-owner preparation pins DATA/BO ownership, never save bodies.
pub(crate) struct ComputeRebindSeed {
    layout: Layout,
    old_key: Key,
    old_preempt: u64,
    new_preempt: u64,
    old_priority: u32,
    snapshot: super::g17p_user_vm::RebindSnapshot,
    buffers: KVec<kernel::sync::aref::ARef<super::g17p_drm::Object>>,
    bindings: KVec<(u64,u64,u64,u32)>,
    owner: (u64,u32),
    cpu_maps: runtime::CpuMaps,
    primer_aliases: Option<[(u64,u64);2]>,
    new_key: Key,
    new_priority: u32,
}
pub(crate) struct PreparedComputeRebind {
    layout: Layout,
    old_key: Key,
    old_preempt: u64,
    new_preempt: u64,
    old_priority: u32,
    root: super::g17p_user_vm::PreparedRebind,
    new_key: Key,
    new_priority: u32,
    owner: (u64,u32),
    bindings: KVec<(u64,u64,u64,u32)>,
    buffers: KVec<kernel::sync::aref::ARef<super::g17p_drm::Object>>,
}
impl ComputeRebindSeed {
    pub(crate) fn prepare(self, client:&Client) -> Result<PreparedComputeRebind> {
        let started=kernel::time::Instant::<kernel::time::Monotonic>::now();
        let (root,relocation)=self.snapshot.clone_tree()?;
        let mut shadow=Client { root,buffers:self.buffers,bindings:self.bindings,owner:self.owner,
            cpu_maps:self.cpu_maps,primer_aliases:self.primer_aliases };
        let mut private: KVec<(u64, u64, u64)> = KVec::new();
        if self.old_preempt != self.new_preempt {
            let mut offsets = KVec::with_capacity(256 * 3 + 2, GFP_KERNEL)?;
            for slot in 0..256u64 {
                for page in [0, 0x4000, 0x8000] {
                    offsets.push(slot * 0x78000 + page, GFP_KERNEL)?;
                }
            }
            for offset in [0x100000, 0x108000] {offsets.push(offset, GFP_KERNEL)?;}
            let mut leaves = KVec::with_capacity(offsets.len(), GFP_KERNEL)?;
            for &offset in &offsets {
                let old = self.old_preempt + offset;
                let leaf = shadow.root.pte(old)?;
                if leaf == 0 {
                    pr_err!("G17P: compute rebind missing private leaf grid {} preempt {:#x} new {:#x} offset {:#x}\n",self.layout.grid,self.old_preempt,self.new_preempt,offset);
                    return Err(EIO);
                }
                leaves.push(leaf, GFP_KERNEL)?;
                private.push((old, leaf, 0), GFP_KERNEL)?;
            }
            for (&offset, &leaf) in offsets.iter().zip(leaves.iter()) {
                let address = self.new_preempt + offset;
                if let Some(row) = private.iter_mut().find(|r| r.0 == address) {
                    row.2 = leaf;
                } else {
                    let old = shadow.root.pte(address)?;
                    if old != 0
                        && !shadow
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

        shadow.prepare_rebind(client,false,&[self.layout.asid],&private)?.commit_unpublished();
        let mut bindings=KVec::new();bindings.extend_from_slice(&client.bindings,GFP_KERNEL)?;
        let mut buffers=KVec::with_capacity(client.buffers.len(),GFP_KERNEL)?;
        for bo in &client.buffers { buffers.push(bo.clone(),GFP_KERNEL)?; }
        let root=relocation.prepare(&shadow.root,&[self.layout.asid])?;
        if *crate::module_parameters::submission_log.value() >= 3 {
            pr_info!("G17P: COMPUTE_ROOT_PREPARED grid {} bindings {} elapsed_us {} outside_runtime_lock\n",
                self.layout.grid,bindings.len(),started.elapsed().as_nanos()/1000);
        }
        Ok(PreparedComputeRebind { layout:self.layout,old_key:self.old_key,old_preempt:self.old_preempt,
            new_preempt:self.new_preempt,old_priority:self.old_priority,root,new_key:self.new_key,
            new_priority:self.new_priority,owner:client.owner,bindings,buffers })
    }
}
impl PreparedComputeRebind {
    fn matches(&self,queue:&Queue,key:Key,client:&Client,p:&Parameters,priority:u32) -> bool {
        self.layout==queue.layout && self.old_key==queue.key && self.old_preempt==queue.preempt
            && self.old_priority==queue.priority && self.root.matches(&queue.client.root) && queue.idle()
            && self.new_preempt==p.preempt && self.new_key==key && self.new_priority==priority
            && self.owner==client.owner && self.bindings==client.bindings
            && self.buffers.len()==client.buffers.len()
            && self.buffers.iter().zip(&client.buffers).all(|(a,b)|core::ptr::eq(&**a,&**b))
    }
}
impl Queues {
    pub(crate) fn rebind_size(&self,key:Key,client:&Client,p:&Parameters,priority:u32) -> Option<usize> {
        let queue=self.owners.get(self.choose(key,client,p,priority)?)?;
        (!queue.matches(key,client,p,priority) && queue.idle()).then(||queue.client.root.table_count())
    }
    pub(crate) fn capture_rebind(&self,key:Key,client:&Client,p:&Parameters,priority:u32,
        storage:Option<super::g17p_user_vm::TableStorage>) -> Result<Option<Option<ComputeRebindSeed>>> {
        if self.rebind_size(key,client,p,priority).is_none() { return Ok(Some(None)); }
        let Some(storage)=storage else {return Ok(None);};
        let queue=self.owners.get(self.choose(key,client,p,priority).ok_or(EBUSY)?).ok_or(EIO)?;
        let Some(snapshot)=queue.client.root.capture_rebind(storage)? else {return Ok(None);};
        let mut buffers=KVec::with_capacity(queue.client.buffers.len(),GFP_KERNEL)?;
        for bo in &queue.client.buffers {buffers.push(bo.clone(),GFP_KERNEL)?;}
        let mut bindings=KVec::new();bindings.extend_from_slice(&queue.client.bindings,GFP_KERNEL)?;
        Ok(Some(Some(ComputeRebindSeed {layout:queue.layout,old_key:queue.key,old_preempt:queue.preempt,
            new_preempt:p.preempt,old_priority:queue.priority,snapshot,buffers,bindings,owner:queue.client.owner,
            cpu_maps:queue.client.cpu_maps.clone(),primer_aliases:queue.client.primer_aliases,
            new_key:key,new_priority:priority})))
    }
    pub(crate) fn rebind_matches(&self,plan:Option<&PreparedComputeRebind>,key:Key,client:&Client,
        p:&Parameters,priority:u32) -> bool {
        let Some(index)=self.choose(key,client,p,priority) else {return false;};
        let Some(queue)=self.owners.get(index) else {return plan.is_none();};
        if queue.matches(key,client,p,priority) {return true;}
        plan.is_some_and(|plan|plan.matches(queue,key,client,p,priority))
    }
}
