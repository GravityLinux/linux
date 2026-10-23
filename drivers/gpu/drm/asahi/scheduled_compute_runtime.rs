// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Max command lowering within the common Session. CPU jobs, dependencies,
//! immutable snapshots, GEMs, fences and completions use the shared owners.

use super::super::{
    g17p_compute::{wire, work},
    g17p_compute_queues::Key,
    g17p_render::parameter_error,
    g17p_user_vm::UserVm,
    scheduled_bootstrap as boot,
    scheduled_lifetime::{ContextCloseG16C, LifetimeTickG16C, MaintenanceG16C},
    scheduled_operands as operands,
    scheduled_queues::{self, EngineScratchG16C, QueueSetG16C},
    scheduled_transport::{Request, Requirements, Stamp},
};
use super::{compute, Completion, ComputeReceipt, RenderReceipt, Memory, Phase, Session, Vm};
use core::sync::atomic::{AtomicU64, Ordering};
use kernel::{bindings, dma_fence::RawDmaFence, io::Io, prelude::*, sync::Arc};

const PAGE: u64 = 0x4000;
const ADDRESS: u64 = 0x000003ffffffc000;
const SHARED: u64 = 0x00c000000000044b;
const PRIVATE: u64 = 0x00c0000000000443;
const GPU_READ: u64 = 0x0080000000000c8b;
const DISPATCH: u64 = 0xfffffc2001550000;
const WORK_BASE: u64 = 0xfffffc20d1000000;
const WORK_FLAGS: [u64; 5] = [PRIVATE, PRIVATE, PRIVATE, SHARED, 0x008000000000044b];

#[path = "scheduled_render_runtime.rs"]
#[allow(dead_code)]
mod render;
#[path = "scheduled_render_submit.rs"]
mod render_submit;
#[path = "scheduled_execution_g16c.rs"]
mod execution;
#[path="scheduled_parameter_runtime.rs"]
mod parameter_growth;
#[path="scheduled_parameter_oom.rs"]
mod parameter_oom;
#[path="scheduled_operand_runtime.rs"]
mod operand_growth;
use execution::BindingG16C;

/// Errno alone cannot prove that an error preceded publication. Every plain
/// kernel error is fatal; only explicitly transactional rejection sites may
/// preserve this session and its independently owned GPU work.
#[derive(Clone, Copy)]
enum StageError {
    Rejected(Error),
    Fatal(Error),
}
type StageResult<T> = core::result::Result<T, StageError>;
impl From<Error> for StageError {
    fn from(error: Error) -> Self { Self::Fatal(error) }
}
impl From<kernel::alloc::AllocError> for StageError {
    fn from(error: kernel::alloc::AllocError) -> Self { Self::Fatal(error.into()) }
}
impl From<core::num::TryFromIntError> for StageError {
    fn from(error: core::num::TryFromIntError) -> Self { Self::Fatal(error.into()) }
}
impl StageError {
    fn allocation<E: Into<Error>>(error: E) -> Self {
        let error = error.into();
        if error == ENOMEM { Self::Rejected(error) } else { Self::Fatal(error) }
    }
}

struct Owner {
    client: Option<compute::Client>,
    context: u16,
    high_root: u64,
    directory: u64,
    private_limit: u64,
    operand_state: [u64;2],
    support: u64,
    support_pages: [u64; 2],
    private_next: u64,
    dispatch: [u8; 32],
    compute_ordinal: u32,
    published: bool,
    close_requested: bool,
    invalid_roots: [u64; 2],
    private: KVec<PrivateBlock>,
    // Offsets relative to directory; survive detached-root recycling.
    operand_buffers: KVec<u64>,
    operand_probe_seeded: bool,
    operand_probe_pending: bool,
    retired_queues: bool,
    render: Option<render::ResourcesG16C>,
}
#[derive(Clone, Copy)]
struct PrivateBlock {
    offset: u64,
    physical: u64,
    size: u64,
}
struct Queue {
    live: bool,
    key: Key,
    owner: usize,
    storage: QueueSetG16C,
    save: u64,
    robustness: u64,
    ordinal: u32,
    save_pages: [u64; 2],
    render_count: u32,
    compute_binding: Option<usize>,
}
enum ReceiptG16C { Compute(Arc<ComputeReceipt>), Render(Arc<RenderReceipt>) }
impl ReceiptG16C {
    fn fence(&self)->&kernel::dma_fence::Fence{match self{Self::Compute(r)=>&r.fence,Self::Render(r)=>&r.fence}}
    fn fail(&self,error:Error){match self{Self::Compute(r)=>r.fail(error),Self::Render(r)=>r.fail(error)}}
}
struct Pending {
    id: u64,
    receipt: ReceiptG16C,
    completion: Completion,
    started: kernel::time::Instant<kernel::time::Monotonic>,
    render_owner: Option<usize>,
    growth_tokens: KVec<u32>,
    render_work: Option<parameter_error::RenderOwnerG16C>,
    parameter_error: Option<parameter_oom::RepliedError>,
    compute_slot: Option<usize>,
}
enum WorkEvent {Growth(parameter_growth::AcceptedGrowth),ParameterError(parameter_oom::AcceptedError)}
struct QueueClose {
    key: Key,
    active_jobs: Arc<AtomicU64>,
}
pub(super) struct State {
    owners: KVec<Owner>,
    queues: KVec<Queue>,
    pending: KVec<Pending>,
    cpu_leases: KVec<kernel::dma_fence::Fence>,
    selected: Option<usize>,
    queue_next: u64,
    metadata_next: u64,
    work_pages: [Option<[u64; 5]>;2],
    work_leases: [Option<(usize,kernel::dma_fence::Fence)>;2],
    compute_mode: bool,
    compute_sequences: [u32;2],
    compute_bound: [bool;2],
    engine_scratch: Option<EngineScratchG16C>,
    queue_closes: KVec<QueueClose>,
    ordinal: u32,
    operand_allocation_faulted: bool,
    initial_allocation_faulted: bool,
    render_ordinal: u32,
    render_work_pages: Option<[u64;7]>,
    render_status: bool,
    render_close: u32,
    compute_first: Option<bool>,
}
impl State {
    fn idle(&mut self) -> bool {
        self.cpu_leases
            .retain(|f| unsafe { bindings::dma_fence_get_status(f.raw()) } == 0);
        self.pending.is_empty() && self.cpu_leases.is_empty()
    }
    fn release_requested(&mut self, session: &mut Session) -> Result {
        if !self.idle() {
            return Err(EBUSY);
        }
        self.release_queues(session)?;
        for index in 0..self.owners.len() {
            let o = &self.owners[index];
            if !o.close_requested || o.client.is_none() {
                continue;
            }
            if !o.published {
                if let Some(r) = &mut self.owners[index].render { r.detach()?; }
                // A rejected admission may have mapped queue save pages in
                // this CPU-only root. Root drop invalidates those aliases just
                // as the published close below does; a later owner must map
                // the retained pages again, never validate stale addresses.
                for q in &mut self.queues {
                    if q.owner == index {
                        q.live = false;
                        q.save = 0;
                        q.robustness = 0;
                    }
                }
                self.owners[index].client = None;
                continue;
            }
            let memory = session.memory.as_ref().ok_or(EIO)?;
            let vm = session.vm.as_ref().ok_or(EIO)?;
            let at = session.ttbs + o.context as u64 * 16;
            memory.invalidate(at, 16)?;
            let before = [memory.read64(at)?, memory.read64(at + 8)?];
            let expected = [o.client.as_ref().ok_or(EIO)?.root.root(), o.high_root]
                .map(|root| root | ((o.context as u64) << 48) | 1);
            if before != expected {
                return Err(EIO);
            }
            let cursors = boot::control_cursors(memory, vm)?;
            if cursors != [cursors[0]; 3] {
                return Err(EBUSY);
            }
            let mut closes = KVec::new();
            // Validate every live prefix before publishing the first control.
            for q in self
                .queues
                .iter()
                .rev()
                .filter(|q| q.live && q.owner == index && (q.ordinal != 0 || q.render_count != 0))
            {
                let prefix = memory
                    .read_firmware32(vm.physical(memory, 2, q.storage.context)?)?
                    .to_le_bytes();
                closes.push(
                    ContextCloseG16C::new(q.storage.context, prefix).ok_or(EIO)?,
                    GFP_KERNEL,
                )?;
            }
            let detached = closes.is_empty();
            if detached && !o.retired_queues {
                return Err(EIO);
            }
            let mut count = 0;
            if !detached {
                if o.render.as_ref().is_some_and(|r| r.attached()) {
                    let tick=LifetimeTickG16C::for_render(self.render_close);
                    session.g16c_control(tick.bytes())?;
                    self.render_close=self.render_close.wrapping_add(1);
                    count=closes.len()+1;
                } else {
                    session.g16c_close_prelude()?;
                    count = closes.len() + 7;
                }
                for close in &closes {
                    session.g16c_control(close.bytes())?;
                }
            }
            let memory = session.memory.as_mut().ok_or(EIO)?;
            // Recheck exact roots after firmware retirement, before detaching.
            memory.invalidate(at, 16)?;
            if [memory.read64(at)?, memory.read64(at + 8)?] != before {
                return Err(EIO);
            }
            for i in 0..2 {
                memory.write64(at + i as u64 * 8, before[i] & !1)?;
            }
            memory.clean(at, 16)?;
            UserVm::invalidate(o.context);
            memory.invalidate(at, 16)?;
            if [memory.read64(at)?, memory.read64(at + 8)?] != before.map(|v| v & !1) {
                return Err(EIO);
            }
            // Qualified detached release: after the last live roots break,
            // retire all six maintenance phases before reusing their backing.
            if (detached || o.render.as_ref().is_some_and(|r| r.attached())) && !self.owners.iter().enumerate()
                .any(|(i, owner)| i != index && owner.published) {
                for engine in 0..3 {
                    for phase in 1..=2 {
                        session.g16c_control(MaintenanceG16C::new(engine, phase).ok_or(EIO)?.bytes())?;
                    }
                }
                count += 6;
            }
            let context = o.context;
            if self.selected == Some(index) {
                self.selected = None;
            }
            self.owners[index].published = false;
            self.owners[index].invalid_roots = before.map(|v| v & !1);
            for q in &mut self.queues {
                if q.owner == index {
                    q.live = false;
                    q.save = 0;
                    q.robustness = 0;
                }
            }
            // No client root or BO reference may drop before this receipt.
            if let Some(r) = &mut self.owners[index].render { r.detach()?; }
            drop(self.owners[index].client.take());
            // Context retirement invalidates any engine's retained binding.
            self.compute_bound = [false; 2];
            kernel::dev_info!(
                session.peers[0].data.dev.as_ref(),
                "G16C: context {} released after {} retired controls and owned TTBR invalidation\n",
                context,
                count
            );
            let (allocations, bytes) = session.memory.as_ref().ok_or(EIO)?.allocation_stats();
            kernel::dev_info!(session.peers[0].data.dev.as_ref(),
                "G16C: retained backing allocations={} bytes={} pool_slots={} queue_slots={}\n",
                allocations, bytes, self.owners.len(), self.queues.len());
        }
        Ok(())
    }
    fn release_queues(&mut self, session: &mut Session) -> Result {
        let mut request = 0;
        while request < self.queue_closes.len() {
            if self.queue_closes[request].active_jobs.load(Ordering::Acquire) != 0 {
                request += 1;
                continue;
            }
            let key = self.queue_closes[request].key;
            // Snapshot and validate every installed owner, including a VM
            // whose last logical queue has already retired. No root changes.
            let mut roots = KVec::new();
            let memory = session.memory.as_ref().ok_or(EIO)?;
            for o in self.owners.iter().filter(|o| o.published) {
                let at = session.ttbs + o.context as u64 * 16;
                memory.invalidate(at, 16)?;
                let words = [memory.read64(at)?, memory.read64(at + 8)?];
                let expected = [o.client.as_ref().ok_or(EIO)?.root.root(), o.high_root]
                    .map(|root| root | ((o.context as u64) << 48) | 1);
                if words != expected { return Err(EIO); }
                roots.push((at, words), GFP_KERNEL)?;
            }
            let vm = session.vm.as_ref().ok_or(EIO)?;
            let cursors = boot::control_cursors(memory, vm)?;
            if cursors != [cursors[0]; 3] { return Err(EBUSY); }
            // A single accepted logical queue can own more than one frozen
            // binding generation. Preflight every prefix before first send.
            let mut closes = KVec::new();
            for (i, q) in self.queues.iter().enumerate().filter(|(_, q)| q.live && q.key == key) {
                let close = if q.ordinal == 0 && q.render_count == 0 { None } else {
                    let prefix = memory.read_firmware32(vm.physical(memory, 2, q.storage.context)?)?.to_le_bytes();
                    Some(ContextCloseG16C::new(q.storage.context, prefix).ok_or(EIO)?)
                };
                closes.push((i, close), GFP_KERNEL)?;
            }
            for (i, close) in closes {
                let controls = if let Some(close) = close {
                    let count=if self.owners[self.queues[i].owner].render.as_ref().is_some_and(|r| r.attached()) {
                        session.g16c_control(LifetimeTickG16C::for_render(self.render_close).bytes())?;
                        self.render_close=self.render_close.wrapping_add(1);2
                    } else {session.g16c_close_prelude()?;8};
                    session.g16c_control(close.bytes())?;
                    count
                } else { 0 };
                let memory = session.memory.as_ref().ok_or(EIO)?;
                for &(at, words) in &roots {
                    memory.invalidate(at, 16)?;
                    if [memory.read64(at)?, memory.read64(at + 8)?] != words { return Err(EIO); }
                }
                let q = &mut self.queues[i];
                q.live = false;
                if controls != 0 {
                    self.owners[q.owner].retired_queues = true;
                    self.compute_bound = [false; 2];
                }
                let (allocations, bytes) = memory.allocation_stats();
                kernel::dev_info!(session.peers[0].data.dev.as_ref(),
                    "G16C: queue {} retired controls={} roots_preserved={} allocations={} bytes={} pool_slots={} queue_slots={}\n",
                    key.1, controls, roots.len(), allocations, bytes, self.owners.len(), self.queues.len());
            }
            // This may be an entirely unpublished queue. Its zero accepted
            // count still proves no future worker can construct this key.
            self.queue_closes.remove(request).map_err(|_| EIO)?;
        }
        Ok(())
    }
    pub(super) fn new() -> Self {
        Self {
            owners: KVec::new(),
            queues: KVec::new(),
            pending: KVec::new(),
            cpu_leases: KVec::new(),
            selected: None,
            queue_next: scheduled_queues::START,
            metadata_next: 0xfffffc2207000000,
            work_pages: [None;2],
            work_leases: core::array::from_fn(|_|None),
            compute_mode: false,
            compute_sequences: [0;2],
            compute_bound: [false;2],
            engine_scratch: None,
            queue_closes: KVec::new(),
            ordinal: 0,
            operand_allocation_faulted: false,
            initial_allocation_faulted: false,
            render_ordinal: 0, render_work_pages: None, render_status: false, render_close: 0, compute_first: None,
        }
    }
    fn client(&self, key: Key) -> Option<&compute::Client> {
        self.queues
            .iter()
            .rev()
            .find(|q| q.live && q.key == key)
            .and_then(|q| self.owners[q.owner].client.as_ref())
    }
    pub(super) fn fail_receipts(&self, error: Error) {
        for pending in &self.pending {
            pending.receipt.fail(error);
        }
    }
}

fn bytes(size: usize) -> Result<KVVec<u8>> {
    let mut b = KVVec::new();
    b.extend_with(size, 0, GFP_KERNEL)?;
    Ok(b)
}
fn private_span(memory: &mut Memory, blocks: &mut KVec<PrivateBlock>, offset: u64, size: u64) -> Result {
    // Avoid a high-order 8/16-MiB allocation. The existing owner can retain
    // separate chunks while the GPU sees the required contiguous VA extent.
    let mut cursor = 0;
    while cursor < size {
        let span = (size - cursor).min(0x200000);
        blocks.reserve(1, GFP_KERNEL)?;
        let pa = memory.allocate(span as usize)?;
        blocks.push(PrivateBlock { offset: offset + cursor, physical: pa, size: span }, GFP_KERNEL)?;
        cursor += span;
    }
    Ok(())
}
fn map_private(memory: &mut Memory, root: &mut UserVm, base: u64, blocks: &[PrivateBlock]) -> Result {
    // Preflight all fresh root paths before zeroing any retired backing.
    for block in blocks {
        root.prepare(base.checked_add(block.offset).ok_or(EINVAL)?, block.size)?;
    }
    for block in blocks {
        memory.zero(block.physical, block.size as usize)?;
        memory.clean(block.physical, block.size as usize)?;
        for page in (0..block.size).step_by(PAGE as usize) {
            root.map_page(base + block.offset + page, block.physical + page, true)?;
        }
    }
    Ok(())
}
fn map_prepared_private(root: &mut UserVm, base: u64, blocks: &[PrivateBlock]) -> Result {
    // Paths and cleaned DATA already exist; preserve the prepared contents.
    for block in blocks {
        for offset in (0..block.size).step_by(PAGE as usize) {
            root.map_page(base + block.offset + offset, block.physical + offset, true)?;
        }
    }
    Ok(())
}
fn write_low(memory: &mut Memory, root: &UserVm, va: u64, data: &[u8]) -> Result {
    for (i, chunk) in data.chunks(PAGE as usize).enumerate() {
        let pa = root.pte(va + i as u64 * PAGE)? & ADDRESS;
        if pa == 0 {
            return Err(EIO);
        }
        memory.write(pa, chunk)?;
        memory.clean(pa, chunk.len())?;
    }
    Ok(())
}
fn fw_span(memory: &mut Memory, vm: &mut Vm, va: u64, size: u64, flags: u64) -> Result<u64> {
    let pa = memory.allocate(size as usize)?;
    vm.span(memory, 2, va, pa, size as usize, flags)?;
    memory.clean(pa, size as usize)?;
    Ok(pa)
}
/// Fault injection is limited to detached backing, before any adoption or
/// mapping. Production defaults disable it. A single Session rejects once.
fn initial_allocation_checkpoint(stage: u32, fired: &mut bool, memory: &Memory) -> StageResult<()> {
    if *crate::module_parameters::g16c_initial_alloc_fail.value() == stage && !*fired {
        *fired = true;
        let (allocations, bytes) = memory.allocation_stats();
        kernel::pr_info!("G16C: injected initial allocation ENOMEM stage={} detached_allocations={} detached_bytes={}\n",
            stage, allocations, bytes);
        return Err(StageError::Rejected(ENOMEM));
    }
    Ok(())
}

/// Populate owned, still-unmapped backing using its source DVA offsets.
fn write_blocks(memory: &mut Memory, blocks: &[PrivateBlock], offset: u64, data: &[u8]) -> Result {
    let mut cursor = 0;
    while cursor < data.len() {
        let at = offset.checked_add(cursor as u64).ok_or(EOVERFLOW)?;
        let block = blocks.iter().find(|b| at >= b.offset && at - b.offset < b.size).ok_or(EIO)?;
        let within = at - block.offset;
        let size = (data.len() - cursor).min((block.size - within) as usize);
        memory.write(block.physical + within, &data[cursor..cursor + size])?;
        memory.clean(block.physical + within, size)?;
        cursor += size;
    }
    Ok(())
}

/// Shared host preparation for fixed Max work-storage DATA. This changes no
/// graph topology or wire layout: all leaves remain absent until adoption.
fn stage_work_pages<const N: usize>(memory: &mut Memory, vm: &mut Vm, base: u64,
    flags: [u64; N], aliases: &[usize]) -> Result<(Memory, [u64; N])> {
    let alias = 0x7000000000 + base - work::DESCRIPTOR_BASE_G16C;
    for (i, _) in flags.into_iter().enumerate() { vm.prepare_g16c_empty(memory, 2, base + i as u64 * PAGE, PAGE as usize)?; }
    for &i in aliases {
        if i >= N { return Err(EINVAL); }
        vm.prepare_g16c_empty(memory, 0, alias + i as u64 * PAGE, PAGE as usize)?;
    }
    let mut staged = Memory::detached();
    let mut pages = [0; N];
    for page in &mut pages { *page = staged.allocate(PAGE as usize)?; staged.clean(*page, PAGE as usize)?; }
    // Keep attributes at the installation call site alongside the wire graph.
    Ok((staged, pages))
}
fn install_work_pages<const N: usize>(memory: &mut Memory, vm: &mut Vm, base: u64,
    flags: [u64; N], aliases: &[usize], pages: [u64; N]) -> Result {
    let alias = 0x7000000000 + base - work::DESCRIPTOR_BASE_G16C;
    for (i, flag) in flags.into_iter().enumerate() { vm.span(memory, 2, base + i as u64 * PAGE, pages[i], PAGE as usize, flag)?; }
    for &i in aliases { vm.span(memory, 0, alias + i as u64 * PAGE, pages[i], PAGE as usize, GPU_READ)?; }
    Ok(())
}
fn flight_allocation_checkpoint(fired: &mut bool) -> StageResult<()> {
    if *crate::module_parameters::g16c_initial_alloc_fail.value() == 8 && !*fired {
        *fired = true;
        kernel::pr_info!("G16C: injected unpublished flight metadata ENOMEM before graph/control publication\n");
        return Err(StageError::Rejected(ENOMEM));
    }
    Ok(())
}

fn same(a: &compute::Client, b: &compute::Client) -> bool {
    a.owner == b.owner
        && a.bindings == b.bindings
        && a.buffers.len() == b.buffers.len()
        && a.buffers
            .iter()
            .zip(&b.buffers)
            .all(|(x, y)| core::ptr::eq(&**x, &**y))
}

impl State {
    fn build_owner(
        &mut self,
        memory: &mut Memory,
        vm: &mut Vm,
        ttbs: u64,
        mut client: compute::Client,
        base: u64,
        limit: u64,
    ) -> StageResult<usize> {
        let capacity = limit.checked_sub(base).ok_or(EINVAL)?;
        if capacity == 0 || (base | limit) & (PAGE - 1) != 0 || limit > 1 << 42 {
            return Err(EINVAL.into());
        }
        // A grown dormant pool may not fit a later caller's smaller window.
        // Check all retained extents before moving ownership or clearing RAM.
        let reusable = self.owners.iter().position(|o| o.client.is_none()
            && o.private.iter().all(|b| b.offset.checked_add(b.size).is_some_and(|end| end <= capacity))
            && o.operand_buffers.iter().all(|offset| offset.checked_add(operands::BUFFER_STRIDE)
                .is_some_and(|end| end <= capacity)));
        if reusable.is_none() && self.owners.len() >= 63 {
            return Err(StageError::Rejected(ENOSPC));
        }
        let context = reusable.map_or(self.owners.len() as u16 + 1, |i| self.owners[i].context);
        let invalid_roots = reusable.map_or([0; 2], |i| self.owners[i].invalid_roots);
        let table = base
            .checked_add(operands::DIRECTORY_SIZE + 2 * PAGE)
            .ok_or(EINVAL)?;
        let state = table
            .checked_add(operands::TABLE_SIZE + 2 * PAGE)
            .ok_or(EINVAL)?;
        let buffers = state + 0x80000;
        let end = buffers
            .checked_add(operands::INITIAL_BUFFERS as u64 * operands::BUFFER_STRIDE)
            .ok_or(EINVAL)?;
        let mut private_end=end.checked_add(0x80000).ok_or(EINVAL)?;
        if private_end > limit {
            return Err(EINVAL.into());
        }
        // Only a context with the qualified release receipt has client=None.
        // Revalidate its exact invalid TTBR words before touching cached RAM.
        let at = ttbs + context as u64 * 16;
        memory.invalidate(at, 16)?;
        if [memory.read64(at)?, memory.read64(at + 8)?] != invalid_roots {
            return Err(EBUSY.into());
        }
        let mut detached = Memory::detached();
        if reusable.is_none() { self.owners.reserve(1, GFP_KERNEL).map_err(StageError::allocation)?; }
        let (support, support_pages, high_root, private) = if let Some(i) = reusable {
            let o = &self.owners[i];
            for (j, flags) in [PRIVATE, SHARED].into_iter().enumerate() {
                if vm.pte(memory, 2, o.support + j as u64 * PAGE)? != o.support_pages[j] | flags {
                    return Err(EIO.into());
                }
            }
            let mut blocks = KVec::new();
            blocks.reserve(o.private.len(), GFP_KERNEL).map_err(StageError::allocation)?;
            for block in &o.private { blocks.push(*block, GFP_KERNEL).expect("reserved retained private blocks"); }
            (o.support, o.support_pages, o.high_root, blocks)
        } else {
            let support = self.metadata_next;
            if support + 2 * PAGE > 0xfffffc2208000000 { return Err(StageError::Rejected(ENOSPC)); }
            let pages = [detached.allocate(PAGE as usize).map_err(StageError::allocation)?,
                detached.allocate(PAGE as usize).map_err(StageError::allocation)?];
            let high_root = detached.allocate(PAGE as usize).map_err(StageError::allocation)?;
            let mut private = KVec::new();
            private_span(&mut detached, &mut private, 0, operands::DIRECTORY_SIZE).map_err(StageError::allocation)?;
            private_span(&mut detached, &mut private, table - base, operands::TABLE_SIZE).map_err(StageError::allocation)?;
            private_span(&mut detached, &mut private, state - base, 0x7c000).map_err(StageError::allocation)?;
            // Independently leased CDM engines require different GPU-written
            // state and work tags even when they share one VM operand pool.
            private_span(&mut detached, &mut private, end - base, 0x7c000).map_err(StageError::allocation)?;
            for i in 0..operands::INITIAL_BUFFERS as u64 {
                private_span(&mut detached, &mut private,
                    buffers - base + i * operands::BUFFER_STRIDE, operands::BUFFER_SIZE).map_err(StageError::allocation)?;
            }
            (support, pages, high_root, private)
        };
        let mut operand_buffers = KVec::new();
        if let Some(i) = reusable {
            operand_buffers.reserve(self.owners[i].operand_buffers.len(), GFP_KERNEL).map_err(StageError::allocation)?;
            for &offset in &self.owners[i].operand_buffers {
                operand_buffers.push(offset, GFP_KERNEL).expect("reserved retained operand offsets");
            }
        } else {
            operand_buffers.reserve(operands::INITIAL_BUFFERS as usize, GFP_KERNEL).map_err(StageError::allocation)?;
            for i in 0..operands::INITIAL_BUFFERS as u64 {
                operand_buffers.push(buffers-base+i*operands::BUFFER_STRIDE,GFP_KERNEL).expect("reserved initial operand offsets");
            }
        }
        // A recycled owner retains every successfully grown buffer. Save
        // slots/render backing are placed after this complete retained span.
        for offset in &operand_buffers {
            private_end=private_end.max(base.checked_add(*offset)
                .and_then(|p|p.checked_add(operands::BUFFER_STRIDE)).ok_or(EOVERFLOW)?);
        }
        if private_end>limit {return Err(StageError::Rejected(ENOSPC));}
        let mut directory = bytes(PAGE as usize).map_err(StageError::allocation)?;
        let mut entries = bytes(operands::TABLE_SIZE as usize).map_err(StageError::allocation)?;
        for block in &private {
            client.root.prepare(base + block.offset, block.size).map_err(StageError::allocation)?;
        }
        if reusable.is_none() {
            vm.prepare_g16c_empty(memory, 2, support, 2 * PAGE as usize).map_err(StageError::allocation)?;
        }
        initial_allocation_checkpoint(1, &mut self.initial_allocation_faulted, &detached)?;
        memory.absorb(detached).map_err(StageError::allocation)?;
        // All fallible allocations precede adoption. Errors from here are
        // fatal, including unexpected mapping/ownership validation failures.
        if reusable.is_none() {
            for (j, flags) in [PRIVATE, SHARED].into_iter().enumerate() {
                vm.span(memory, 2, support + j as u64 * PAGE, support_pages[j], PAGE as usize, flags)?;
            }
            self.metadata_next += 2 * PAGE;
        }
        map_private(memory, &mut client.root, base, &private)?;
        for physical in [support_pages[0], support_pages[1], high_root] {
            memory.zero(physical, PAGE as usize)?;
            memory.clean(physical, PAGE as usize)?;
        }
        for (i,offset) in operand_buffers.iter().enumerate() {
            let buffer = base + offset;
            for j in 0..2048 {
                directory[j * 8..j * 8 + 8]
                    .copy_from_slice(&(buffer + j as u64 * 0x1000).to_le_bytes());
            }
            write_low(memory, &client.root, base + i as u64 * PAGE, &directory)?;
            entries[i * 64..i * 64 + 8].copy_from_slice(&(buffer | 1 << 63).to_le_bytes());
        }
        write_low(memory, &client.root, table, &entries)?;
        let record = operands::ComputeSupportG16C::new(
            base,
            table,
            support + PAGE,
            operand_buffers.len().try_into()?,
        )
        .ok_or(EINVAL)?;
        vm.write(memory, 2, support, record.as_bytes())?;
        vm.write(memory, 2, support + PAGE, &1u32.to_le_bytes())?;
        let mut dispatch = [0u8; 32];
        dispatch[..8].copy_from_slice(&(0x4000000000000000u64 | (base >> 7)).to_le_bytes());
        let pages = u32::try_from(operand_buffers.len())? * 2048;
        dispatch[12..16].copy_from_slice(&(pages << 1).to_le_bytes());
        dispatch[16..20].copy_from_slice(&pages.to_le_bytes());
        let index = reusable.unwrap_or(self.owners.len());
        let render = reusable.and_then(|i| self.owners[i].render.take());
        let value = Owner {
            client: Some(client),
            context,
            high_root,
            directory: base,
            private_limit: limit,
            operand_state: [state,end],
            support,
            support_pages,
            private_next: private_end,
            dispatch,
            compute_ordinal: 0,
            published: false,
            close_requested: false,
            invalid_roots,
            private,
            operand_buffers,
            operand_probe_seeded: false,
            operand_probe_pending: false,
            retired_queues: false,
            render,
        };
        if let Some(i) = reusable {
            self.owners[i] = value;
        } else {
            self.owners.push(value, GFP_KERNEL)?;
        }
        Ok(index)
    }
    fn prepare_queue(
        &mut self,
        session: &mut Session,
        key: Key,
        client: &mut Option<compute::Client>,
        base: u64,
        limit: u64,
        priority: u32,
        compute_binding: Option<usize>,
    ) -> StageResult<usize> {
        // A caller-only immutable snapshot can be accepted while another
        // worker changes bindings. At this serialized admission boundary all
        // previously published GPU work and CPU cache leases have retired.
        // Unstaged jobs still own their exact snapshots/BOs and can reconstruct
        // that generation later; they do not own these firmware roots/queues.
        if let Some(next) = client.as_ref() {
            let mut obsolete = false;
            for o in &mut self.owners {
                if o.client.as_ref().is_some_and(|old| old.owner == next.owner && !same(old, next)) {
                    o.close_requested = true;
                    obsolete = true;
                }
            }
            if obsolete {
                self.release_requested(session)?;
            }
        }
        if let Some(i) = self
            .queues
            .iter().enumerate()
            .rfind(|(index,q)| q.live && q.key == key && self.owners[q.owner].client.is_some()
                && compute_binding.is_none_or(|binding|q.compute_binding.is_none_or(|old|old==binding))
                && !self.work_leases.iter().flatten().any(|(qi,_)|qi==index))
            .map(|(index,_)|index)
        {
            if client.as_ref().is_none_or(|c| {
                self.owners[self.queues[i].owner]
                    .client
                    .as_ref()
                    .is_some_and(|old| same(c, old))
            }) {
                let o = &self.owners[self.queues[i].owner];
                if o.directory != base || o.private_limit != limit { return Err(EIO.into()); }
                if compute_binding.is_some() { self.queues[i].compute_binding=compute_binding; }
                return Ok(i);
            }
        }
        let next = client.take();
        let memory = session.memory.as_mut().ok_or(EIO)?;
        let vm = session.vm.as_mut().ok_or(EIO)?;
        let owner = if let Some(next) = next {
            if let Some(i) = self.owners.iter()
                .position(|o| o.client.as_ref().is_some_and(|c| same(c, &next))) { i }
            else { self.build_owner(memory, vm, session.ttbs, next, base, limit)? }
        } else {
            // Another native binding may already hold this logical queue's
            // immutable client. Keep it pinned while adding a disjoint binding.
            self.queues.iter().rfind(|q|q.live && q.key==key && self.owners[q.owner].client.is_some())
                .map(|q|q.owner).ok_or(EIO)?
        };
        if self.owners[owner].directory != base || self.owners[owner].private_limit != limit {
            return Err(EIO.into());
        }
        if self.engine_scratch.is_none() {
            let (staged, prepared) = EngineScratchG16C::prepare(memory, vm).map_err(StageError::allocation)?;
            initial_allocation_checkpoint(2, &mut self.initial_allocation_faulted, &staged)?;
            memory.absorb(staged).map_err(StageError::allocation)?;
            self.engine_scratch = Some(prepared.install(memory, vm)?);
        }
        // Preserve aliases in another still-live root. Prefer this owner's
        // already-mapped save slot; otherwise use backing detached by VM close.
        let reusable = self.queues.iter().position(|q| !q.live && q.owner == owner && q.save != 0)
            .or_else(|| self.queues.iter().position(|q| !q.live && q.save == 0));
        let i = if let Some(i) = reusable {
            let q = &self.queues[i];
            if q.save != 0 {
                let root = &self.owners[owner].client.as_ref().ok_or(EIO)?.root;
                for (j, physical) in q.save_pages.into_iter().enumerate() {
                    if root.pte(q.save + j as u64 * PAGE)? != physical | GPU_READ | (1 << 54) {
                        return Err(EIO.into());
                    }
                }
            }
            self.queues[i].storage.reset(memory, vm, priority.try_into()?)?;
            i
        } else {
            self.queues.reserve(1, GFP_KERNEL).map_err(StageError::allocation)?;
            let uuid: u16 = (self.queues.len() + 1).try_into()?;
            let (mut staged, prepared) = QueueSetG16C::prepare(memory, vm, self.queue_next,
                priority.try_into()?, uuid, None).map_err(StageError::allocation)?;
            let save_pages = [staged.allocate(PAGE as usize).map_err(StageError::allocation)?,
                staged.allocate(PAGE as usize).map_err(StageError::allocation)?];
            let o = &mut self.owners[owner];
            if o.private_next.checked_add(2 * PAGE).ok_or(EOVERFLOW)? > o.private_limit {
                return Err(StageError::Rejected(ENOSPC));
            }
            o.client.as_mut().ok_or(EIO)?.root.prepare(o.private_next, 2 * PAGE)
                .map_err(StageError::allocation)?;
            initial_allocation_checkpoint(3, &mut self.initial_allocation_faulted, &staged)?;
            memory.absorb(staged).map_err(StageError::allocation)?;
            let storage = prepared.install(memory, vm, priority.try_into()?)?;
            let i = self.queues.len();
            self.queues.push(Queue { live: false, key, owner, storage,
                save: 0, robustness: 0, ordinal: 0, save_pages, render_count: 0, compute_binding }, GFP_KERNEL)?;
            self.queue_next += scheduled_queues::SIZE;
            i
        };
        let q = &mut self.queues[i];
        let scratch = self.engine_scratch.as_ref().ok_or(EIO)?;
        for i in 0..3 {
            q.storage.lanes[i].scratch_high = scratch.high[i];
            q.storage.lanes[i].scratch_low = scratch.low[i];
        }
        let o = &mut self.owners[owner];
        let mapped = q.owner == owner && q.save != 0;
        let save = if mapped { q.save } else { o.private_next };
        if save.checked_add(2 * PAGE).ok_or(EINVAL)? > o.private_limit {
            return Err(ENOSPC.into());
        }
        let root = &mut o.client.as_mut().ok_or(EIO)?.root;
        if !mapped { root.prepare(save, 2 * PAGE).map_err(StageError::allocation)?; }
        for (j, physical) in q.save_pages.into_iter().enumerate() {
            memory.zero(physical, PAGE as usize)?;
            memory.clean(physical, PAGE as usize)?;
            if !mapped { root.map_page(save + j as u64 * PAGE, physical, true)?; }
        }
        if !mapped { o.private_next += 2 * PAGE; }
        q.live = true;
        q.key = key;
        q.owner = owner;
        q.save = save;
        q.robustness = save + PAGE;
        q.ordinal = 0;
        q.render_count = 0;
        q.compute_binding = compute_binding;
        Ok(i)
    }
    fn activate(&mut self, session: &mut Session, index: usize) -> Result {
        let memory = session.memory.as_mut().ok_or(EIO)?;
        let vm = session.vm.as_ref().ok_or(EIO)?;
        if self.selected != Some(index) {
            if let Some(previous) = self.selected {
                let pa = vm.physical(memory, 2, DISPATCH)?;
                memory.read_firmware_words(pa, &mut self.owners[previous].dispatch)?;
            }
            vm.write(memory, 2, DISPATCH, &self.owners[index].dispatch)?;
            self.selected = Some(index);
        }
        let o = &mut self.owners[index];
        if !o.published {
            let at = session.ttbs + o.context as u64 * 16;
            memory.invalidate(at, 16)?;
            if [memory.read64(at)?, memory.read64(at + 8)?] != o.invalid_roots {
                return Err(EBUSY);
            }
            memory.write64(at, 0)?;
            memory.write64(at + 8, 0)?;
            memory.clean(at, 16)?;
            UserVm::invalidate(o.context);
            // Retain this client in State before exposing either TTBR.
            o.published = true;
            memory.write64(
                at,
                o.client.as_ref().ok_or(EIO)?.root.root() | ((o.context as u64) << 48) | 1,
            )?;
            memory.write64(at + 8, o.high_root | ((o.context as u64) << 48) | 1)?;
            memory.clean(at, 16)?;
        }
        vm.flush_tables(memory)?;
        Vm::invalidate_gpu();
        // Seed diagnostics before this fresh pool can be used by either
        // rendering or compute, including a recycled render-first owner.
        self.seed_operand_probe(session,index)?;
        Ok(())
    }
    fn compute_slots()->Result<usize> {
        let slots=*crate::module_parameters::g16c_compute_slots.value() as usize;
        if !(1..=2).contains(&slots) {return Err(EINVAL);}
        Ok(slots)
    }
    fn work_base(slot:usize)->Result<u64> {
        // Slot zero preserves its qualified address. The render graph owns
        // +0x40000..+0x5c000; other compute graphs must not overlap it.
        Ok(WORK_BASE+match slot {0=>0,1=>0x80000,_=>return Err(EINVAL)})
    }
    fn work_event(&self,slot:usize)->u8 {
        (slot+if self.compute_first==Some(false){2}else{0})as u8
    }
    fn prepare_work(&mut self,memory:&mut Memory,vm:&mut Vm,slot:usize)->StageResult<()> {
        let base=Self::work_base(slot)?;
        let alias=0x7000000000+base-work::DESCRIPTOR_BASE_G16C;
        if self.work_leases[slot].is_some() {return Err(EBUSY.into());}
        if self.work_pages[slot].is_none() {
            let (staged, pages) = stage_work_pages(memory, vm, base, WORK_FLAGS, &[0])
                .map_err(StageError::allocation)?;
            initial_allocation_checkpoint(5, &mut self.initial_allocation_faulted, &staged)?;
            memory.absorb(staged).map_err(StageError::allocation)?;
            install_work_pages(memory, vm, base, WORK_FLAGS, &[0], pages)?;
            self.work_pages[slot]=Some(pages);
        }
        let pages=self.work_pages[slot].ok_or(EIO)?;
        for(i,flags)in WORK_FLAGS.into_iter().enumerate() {
            if vm.pte(memory,2,base+i as u64*PAGE)?!=pages[i]|flags {return Err(EIO.into());}
        }
        if vm.pte(memory,0,alias)?!=pages[0]|GPU_READ {return Err(EIO.into());}
        for pa in pages {memory.zero(pa,PAGE as usize)?;memory.clean(pa,PAGE as usize)?;}
        Ok(())
    }
    fn reap_work_slots(&mut self) {
        for lease in &mut self.work_leases {
            if lease.as_ref().is_some_and(|(_,f)|unsafe {bindings::dma_fence_get_status(f.raw())}!=0) {
                *lease=None;
            }
        }
    }
    fn configure_compute(&mut self,session:&mut Session,slot:usize)->Result {
        if self.work_leases[slot].is_some() {return Err(EBUSY);}
        let scratch=self.engine_scratch.as_ref().ok_or(EIO)?;
        // Initial binding requires the established global start. Resuming a
        // bound engine preserves its scratch and in-flight sibling state.
        session.configure_g16c_engines(
            &[BindingG16C::compute(self.work_event(slot),scratch.low[slot+2]).ok_or(EIO)?],
            self.compute_bound[slot], !self.compute_bound[slot])?;
        self.compute_bound[slot] = true;
        self.compute_mode=true;
        Ok(())
    }
    fn stage(
        &mut self,
        session: &mut Session,
        key: Key,
        client: &mut Option<compute::Client>,
        p: &compute::Parameters,
        priority: u32,
        dependencies: [Option<(u8,u32)>;3],
    ) -> StageResult<Option<Arc<ComputeReceipt>>> {
        self.cpu_leases.retain(|f|unsafe {bindings::dma_fence_get_status(f.raw())}==0);
        self.reap_work_slots();
        let busy=!self.pending.is_empty() || !self.cpu_leases.is_empty();
        if busy {
            // The global operand view must stay on the SAME immutable VM
            // generation. Different VM dispatch switching needs its own proof.
            let Some(owner)=self.selected.and_then(|i|self.owners[i].client.as_ref()) else {return Ok(None);};
            let requested=client.as_ref().or_else(||self.client(key));
            if !self.compute_mode || requested.is_none_or(|next|!same(owner,next))
                || self.pending.iter().any(|p|p.render_owner.is_some()) {return Ok(None);}
        } else {self.release_requested(session)?;}
        let Some(slot)=self.work_leases[..Self::compute_slots()?].iter().position(Option::is_none) else {return Ok(None);};
        let qi=self.prepare_queue(session,key,client,p.preempt,p.kernel_end,priority,Some(slot))?;
        let oi=self.queues[qi].owner;
        if !busy && self.selected != Some(oi) {
            // A resume is valid only in the retained immutable VM generation.
            self.compute_bound = [false; 2];
        }
        if busy && self.selected!=Some(oi) {return Err(EIO.into());}
        let target=*crate::module_parameters::g16c_operand_buffers.value();
        if !(operands::INITIAL_BUFFERS..=1024).contains(&target) {return Err(EINVAL.into());}
        if !busy && self.queues[qi].ordinal!=0 && target as usize>self.owners[oi].operand_buffers.len() {
            self.activate(session,oi)?;
            self.grow_operands(session,oi,target)?;
        }
        if !self.compute_mode {
            for slot in 0..Self::compute_slots()? {self.prepare_work(session.memory.as_mut().ok_or(EIO)?,session.vm.as_mut().ok_or(EIO)?,slot)?;}
        }
        let receipt_ordinal=self.ordinal;
        // The record waits on this ENGINE's previous item. A global ordinal
        // would make its first job wait for nonexistent work on a new engine.
        let sequence=self.compute_sequences[slot];
        let event=self.work_event(slot);
        let stamp=sequence.wrapping_add(1).wrapping_shl(8);
        let base=Self::work_base(slot)?;
        self.pending.reserve(1,GFP_KERNEL).map_err(StageError::allocation)?;
        self.prepare_work(session.memory.as_mut().ok_or(EIO)?,session.vm.as_mut().ok_or(EIO)?,slot)?;
        let scratch=self.engine_scratch.as_ref().ok_or(EIO)?;
        self.queues[qi].storage.lanes[2].scratch_high=scratch.high[slot+2];
        self.queues[qi].storage.lanes[2].scratch_low=scratch.low[slot+2];
        let receipt = ComputeReceipt::new(receipt_ordinal).map_err(StageError::allocation)?;
        let o = &self.owners[oi];
        let q = &self.queues[qi];
        // All these bindings use the owner's one compute support domain.
        // Its submission identity is independent of queue and engine counters.
        let support_ordinal=o.compute_ordinal;
        let completion = Completion {
            client: o.client.as_ref().ok_or(EIO)?.lease().map_err(StageError::allocation)?,
            stamps: session
                .timestamps
                .as_ref()
                .ok_or(EIO)?
                .cache_seed(&p.timestamps).map_err(StageError::allocation)?,
            fence: receipt.fence.clone(),
            grid: event,
            error: None,
        };
        let mut leases = KVec::new();
        leases.push(o.client.as_ref().ok_or(EIO)?.lease().map_err(StageError::allocation)?, GFP_KERNEL).map_err(StageError::allocation)?;
        o.client.as_ref().ok_or(EIO)?.cache(false).map_err(StageError::allocation)?;
        completion.stamps.run(false).map_err(StageError::allocation)?;
        let mut drivers = KVec::new();
        drivers.push(
            Stamp {
                address: base + 0xc100,
                value: stamp,
            },
            GFP_KERNEL,
        ).map_err(StageError::allocation)?;
        let mut counters = KVec::new();
        counters.push(
            Stamp {
                address: base + 0x800c,
                value: 1,
            },
            GFP_KERNEL,
        ).map_err(StageError::allocation)?;
        let next_queue_ordinal = self.queues[qi].ordinal.checked_add(1).ok_or(EOVERFLOW)?;
        let next_sequence = sequence.checked_add(1).ok_or(EOVERFLOW)?;
        let next_ordinal = self.ordinal.checked_add(1).ok_or(EOVERFLOW)?;
        flight_allocation_checkpoint(&mut self.initial_allocation_faulted)?;
        let memory = session.memory.as_mut().ok_or(EIO)?;
        let vm = session.vm.as_mut().ok_or(EIO)?;
        let alias = 0x7000000000 + base - work::DESCRIPTOR_BASE_G16C;
        let identity = ((q.storage.uuid as u64) << 32) | 0xc0 | o.context as u64;
        let program = wire::ComputeProgramG16C::new(&wire::ProgramParameters {
            save_area: q.save,
            cdm: p.cdm,
            identity,
            context: o.context as u32,
            ordinal: slot as u32,
            robustness: q.robustness,
            operand_state: o.operand_state[slot],
            usc_exec_base: Some(1 << 40),
            helper_binary: 0,
            helper_data: 0,
            helper_config: 0,
            execution_gate: 0,
            register_a599: 0x13200400020,
        })
        .map_err(|_| EINVAL)?;
        let mut descriptor = bytes(PAGE as usize).map_err(StageError::allocation)?;
        wire::ComputeDescriptorG16C::write(
            &mut descriptor,
            &wire::Parameters {
                save_area: q.save,
                cdm: p.cdm,
                identity,
                sequence: u64::from(support_ordinal),
                context: o.context as u32,
                scheduler: base + 0x8000,
                alias,
                terminator: p.end.checked_sub(4).ok_or(EINVAL)?,
                sampler_array: p.sampler,
                sampler_count: p.sampler_count,
                // Shared Neo names: dispatch = completion stamps; status = internal timestamps.
                dispatch: [base + 0xc100, q.storage.lanes[2].queue + 0x1040],
                grid: u32::from(event),
                queue_submission: stamp >> 8,
                queue_ordinal: 0,
                submission_index: sequence.checked_add(1).ok_or(EOVERFLOW)?,
                ordinal: support_ordinal,
                status: [base + 0xc120, base + 0xc128],
                timestamps: p.timestamps,
                support: o.support,
                support_control: 0x6c000001,
                support_flags: 6,
                support_kind: 0x45f,
                support_phase: 1,
                zero_page: base + 0x10000,
            },
            program.registers(),
        )
        .map_err(|_| EINVAL)?;
        vm.write(memory, 2, base, &descriptor)?;
        let wrapper = work::ComputeWrapperG16C::new(&work::WrapperParameters {
            scratch_low: q.storage.lanes[2].scratch_low,
            scratch_high: q.storage.lanes[2].scratch_high,
            support: o.support,
            queue_control: q.storage.context,
            grid: u16::from(event),
            item: q.ordinal as u16 & 255,
            ordinal: q.ordinal as u16,
            context: o.context,
            // One qualified compute support domain is installed per VM.
            support_id: 0,
            uuid: q.storage.uuid,
            scheduler: priority as u16,
            scheduler_hint: 0,
            first: sequence == 0,
            field_52: u16::from(q.ordinal == 0),
            field_5e: 2,
            field_62: u16::from(q.ordinal == 0),
        })
        .map_err(|_| EINVAL)?;
        vm.write(memory, 2, base + 0x4000, wrapper.as_bytes())?;
        let mut end = [0u8; 0x40];
        end[..4].copy_from_slice(&16u32.to_le_bytes());
        end[4..8].copy_from_slice(&(((sequence.wrapping_add(1) & 0xffff) << 16) | u32::from(event)).to_le_bytes());
        end[8..16].copy_from_slice(
            &(0x00fe000000000000u64 | ((sequence as u64 + 1) >> 16)).to_le_bytes(),
        );
        vm.write(memory, 2, base + 0x40c0, &end)?;
        // The common scheduler supplies exact backend target counters. Keep
        // cold binding conservative, but never add unrelated hot-engine waits.
        let mut required=[None;4];required[event as usize]=Some(sequence);
        for (grid,value) in dependencies.into_iter().flatten() {
            if grid>=4 || value>=1<<30 || (grid==event && value>sequence) {return Err(EIO.into());}
            let cell=&mut required[grid as usize];
            *cell=Some(cell.unwrap_or(0).max(value));
        }
        if !self.compute_bound.iter().all(|bound|*bound) {
            for index in 0..Self::compute_slots()? {
                if index==slot || self.compute_sequences[index]!=0 {
                    let cell=&mut required[self.work_event(index)as usize];
                    *cell=Some(cell.unwrap_or(0).max(self.compute_sequences[index]));
                }
            }
        }
        let mut points=[(0,0);4];let mut point_count=0;
        for (grid,value)in required.into_iter().enumerate() {
            if let Some(value)=value {points[point_count]=(grid as u8,value);point_count+=1;}
        }
        let record = work::ComputeRecordG16C::for_compute_control_dependencies(&work::RecordParametersG16C {
            descriptor: base,
            queue: q.storage.lanes[2].queue,
            grid: event,
            item: sequence,
            primary_count: (program.registers().len() - 2) as u16,
            control: 0x0800,
            retire_control: 2,
            ..Default::default()
        }, 0x0800, &points[..point_count])
        .map_err(|_| EINVAL)?;
        vm.write(
            memory,
            2,
            q.storage.lanes[2].scratch_high + ((sequence as u64 + 1) & 255) * 0x200,
            record.as_bytes(),
        )?;
        vm.write(
            memory,
            2,
            base + 0x8000,
            work::ComputeSchedulerG16C::for_compute(base + 0xc140).as_bytes(),
        )?;
        vm.write(memory, 2, base + 0xc140, &1u32.to_le_bytes())?;
        vm.write(
            memory,
            2,
            base + 0xc100,
            &stamp.wrapping_sub(0x100).to_le_bytes(),
        )?;
        let works = [base + 0x4000, base + 0x40c0];
        let lane = &mut self.queues[qi].storage.lanes[2];
        let prepared = session.prepare_g16c_admission(
            &mut [Request {
                lane,
                works: &works,
                event,
                tag: 0,
            }],
            Requirements {
                firmware: KVec::new(),
                driver: drivers,
                notifiers: KVec::new(),
                drains: KVec::new(),
                counters,
            },
            leases,
        )?;
        self.activate(session, oi)?;
        // All host allocations and retirement ownership precede engine arm.
        // Activation publishes scratch/graph tables before alias fetches.
        self.configure_compute(session,slot)?;
        let id = session.publish_g16c_submission(&mut [Request {
            lane: &mut self.queues[qi].storage.lanes[2], works: &works, event, tag: 0,
        }], prepared)?;
        self.compute_first.get_or_insert(true);
        receipt.point.store((u64::from(event)<<40)|(u64::from(event)<<32)|u64::from(next_sequence), Ordering::Relaxed);
        receipt.publication_ready.store(true, Ordering::Release);
        self.work_leases[slot]=Some((qi,receipt.fence.clone()));
        if *crate::module_parameters::g16c_operand_probe.value()!=0 {
            self.owners[oi].operand_probe_pending=true;
        }
        if *crate::module_parameters::submission_log.value()>=3 {
            kernel::dev_info!(session.peers[0].data.dev.as_ref(),"G16C: compute published queue={} context={} slot={} stamp={} in_flight={}\n",key.1,self.owners[oi].context,slot,stamp,self.pending.len()+1);
        }
        self.pending.push(
            Pending {
                id,
                receipt: ReceiptG16C::Compute(receipt.clone()),
                render_owner: None,
                growth_tokens: KVec::new(),
                render_work: None,
                parameter_error: None,
                compute_slot: Some(slot),
                completion,
                started: kernel::time::Instant::now(),
            },
            GFP_KERNEL,
        )?;
        self.queues[qi].ordinal = next_queue_ordinal;
        self.owners[oi].compute_ordinal = support_ordinal.wrapping_add(1);
        self.compute_sequences[slot]=next_sequence;
        self.ordinal = next_ordinal;
        Ok(Some(receipt))
    }
}

impl Session {
    fn prepare_g16c_admission(&mut self, requests: &mut [Request<'_>], requirements: Requirements,
        owners: KVec<compute::ClientLease>) -> StageResult<super::super::scheduled_transport::PreparedG16C> {
        match self.prepare_g16c_submission(requests, requirements, owners) {
            Ok(id) => Ok(id),
            // Preparation runs BEFORE engine arm, owns all retirement metadata
            // and performs no producer writes or sends. Only allocation failure
            // at this boundary is a recoverable unpublished rejection.
            Err(error) if error == ENOMEM && self.phase == Phase::Running
                && self.g16c_transport.as_ref().is_some_and(|t| !t.poisoned()) => Err(StageError::Rejected(error)),
            Err(error) => Err(StageError::Fatal(error)),
        }
    }
    fn configure_g16c_engines(&self, bindings: &[BindingG16C], resume: bool, start: bool) -> Result {
        let regs = self.g16c_sgx.as_ref().ok_or(EIO)?.try_access().ok_or(EIO)?;
        if regs.read32(0xd06030) != 0 { return Err(EBUSY); }
        for binding in bindings {
            let initial = binding.transactions();
            let resumed = binding.resume_transactions();
            let transactions: &[(usize, u8, u64)] = if resume { &resumed } else { &initial };
            for &(offset, width, value) in transactions {
                if width == 32 { regs.try_write32(value as u32, offset)?; }
                else { regs.try_write64(value, offset)?; }
            }
        }
        if regs.read32(0xd06030) != 0 { return Err(EIO); }
        if start { regs.write32(0x50005, 0xd14000); }
        Ok(())
    }
    fn g16c_close_prelude(&mut self) -> Result {
        self.g16c_control(MaintenanceG16C::new(0, 1).ok_or(EIO)?.bytes())?;
        self.g16c_control(LifetimeTickG16C::default().bytes())?;
        for (engine, phase) in [(0, 2), (1, 1), (1, 2), (2, 1), (2, 2)] {
            self.g16c_control(MaintenanceG16C::new(engine, phase).ok_or(EIO)?.bytes())?;
        }
        Ok(())
    }
    pub(crate) fn close_g16c_queue(&mut self, key: Key, active_jobs: Arc<AtomicU64>) -> Result {
        if self.phase != Phase::Running { return Err(EIO); }
        self.g16c_compute.as_mut().ok_or(EIO)?.queue_closes
            .push(QueueClose { key, active_jobs }, GFP_KERNEL)?;
        self.reap_g16c_contexts()
    }
    pub(super) fn g16c_control(&mut self, record: &[u8; 64]) -> Result {
        let target = boot::publish_control(
            self.memory.as_mut().ok_or(EIO)?,
            self.vm.as_ref().ok_or(EIO)?,
            record,
        )?;
        self.g16c_send(0x21, boot::CONTROL_MESSAGE)?;
        self.g16c_send(0x21, 0x0083000000000010)?;
        self.g16c_wait(|memory, vm| boot::control_retired(memory, vm, target))
    }
    pub(crate) fn release_g16c_vm(&mut self, owner: (u64, u32)) -> Result {
        if self.phase != Phase::Running {
            return Err(EIO);
        }
        let mut state = self.g16c_compute.take().ok_or(EIO)?;
        for o in &mut state.owners {
            if o.client.as_ref().is_some_and(|c| c.owner == owner) {
                o.close_requested = true;
            }
        }
        let result = state.release_requested(self);
        self.g16c_compute = Some(state);
        if let Err(error) = result {
            if error != EBUSY {
                self.fail_cpu_completion(error);
            }
        }
        result
    }
    pub(crate) fn reap_g16c_contexts(&mut self) -> Result {
        if self.phase != Phase::Running { return Err(EIO); }
        let mut state = self.g16c_compute.take().ok_or(EIO)?;
        let result = state.release_requested(self);
        self.g16c_compute = Some(state);
        match result {
            Err(error) if error == EBUSY => Ok(()),
            Err(error) => {
                self.fail_cpu_completion(error);
                Err(error)
            }
            Ok(()) => Ok(()),
        }
    }
    pub(crate) fn g16c_client(&self, key: Key) -> Option<&compute::Client> {
        self.g16c_compute.as_ref().and_then(|s| s.client(key))
    }
    pub(crate) fn stage_g16c_compute(
        &mut self,
        key: Key,
        client: &mut Option<compute::Client>,
        p: &compute::Parameters,
        priority: u32,
        dependencies: [Option<(u8,u32)>;3],
    ) -> Result<Option<Arc<ComputeReceipt>>> {
        if self.phase != Phase::Running
            || self
                .peers
                .iter()
                .any(|p| p.data.crashed.load(Ordering::Acquire))
        {
            return Err(EIO);
        }
        let mut state = self.g16c_compute.take().ok_or(EIO)?;
        let result = state.stage(self, key, client, p, priority, dependencies);
        self.g16c_compute = Some(state);
        self.finish_g16c_admission(result)
    }
    fn finish_g16c_admission<T>(&mut self, result: StageResult<T>) -> Result<T> {
        result.map_err(|failure| match failure {
            StageError::Rejected(error) => {
                kernel::dev_info!(self.peers[0].data.dev.as_ref(),
                    "G16C: unpublished admission rejected {:?}; installed ownership preserved\n", error);
                error
            }
            StageError::Fatal(error) => {
                self.fail_cpu_completion(error);
                error
            }
        })
    }
    pub(crate) fn poll_g16c_work(&mut self) -> (Result<bool>, KVec<Completion>) {
        let mut completions = KVec::new();
        let result = (|| -> Result<bool> {
            if self.phase != Phase::Running
                || self
                    .peers
                    .iter()
                    .any(|p| p.data.crashed.load(Ordering::Acquire))
            {
                return Err(EIO);
            }
            let mut state = self.g16c_compute.take().ok_or(EIO)?;
            let result = (|| -> Result<bool> {
                let mut accepted=None;
                let ttbs=self.ttbs;
                boot::drain_with_work_event(self.memory.as_mut().ok_or(EIO)?,
                    self.vm.as_ref().ok_or(EIO)?, &mut |memory,vm,raw| {
                        let result=match u32::from_le_bytes(raw[..4].try_into().unwrap()) {
                            6=>state.accept_growth(memory,vm,ttbs,raw).map(WorkEvent::Growth),
                            7=>state.accept_parameter_error(memory,vm,ttbs,raw).map(WorkEvent::ParameterError),
                            _=>Err(EIO),
                        };
                        accepted=Some(result.map_err(|error|{
                            kernel::pr_err!("G16C: work event ownership failed {:?} RX={:02x?}\n",error,raw);error
                        })?);
                        Ok(true)
                    }).map_err(|error|{kernel::pr_err!("G16C: work RX drain failed {:?}\n",error);error})?;
                if let Some(request)=accepted {
                    match request {
                        WorkEvent::Growth(request)=>state.service_growth(self,request),
                        WorkEvent::ParameterError(request)=>state.service_parameter_error(self,request),
                    }.map_err(|error|{kernel::pr_err!("G16C: work event service failed {:?}\n",error);error})?;
                }
                let mut index=0;
                while index<state.pending.len() {
                    let p=&state.pending[index];
                    if !self.poll_g16c(p.id).map_err(|error|{kernel::pr_err!("G16C: flight poll {} failed {:?}\n",p.id,error);error})? {
                        if p.started.elapsed().as_nanos() > 10_000_000_000 {
                            return Err(ETIMEDOUT);
                        }
                        index+=1;continue;
                    }
                    let id = p.id;
                    completions.reserve(1, GFP_KERNEL)?;
                    state.cpu_leases.reserve(1, GFP_KERNEL)?;
                    self.release_g16c(id).map_err(|error|{kernel::pr_err!("G16C: flight release {} failed {:?}\n",id,error);error})?;
                    let mut p = state.pending.remove(index).map_err(|_| EIO)?;
                    if let Some(error)=p.parameter_error.take() {
                        // Release above independently proved every required
                        // stamp, event counter and queue/channel cursor.
                        p.completion.error=Some(ENOMEM);
                        kernel::dev_info!(self.peers[0].data.dev.as_ref(),
                            "G16C: retired parameter ENOMEM context={} slot={} stamp={} driver_before={:?} work_quiescent=1 error9_retired=1\n",
                            error.owner.context,error.owner.slot,error.owner.stamp,error.before_driver);
                    }
                    if let Some(slot)=p.compute_slot.filter(|_|*crate::module_parameters::submission_log.value()>=3) {
                        kernel::dev_info!(self.peers[0].data.dev.as_ref(),"G16C: compute retired slot={} flight={} remaining={}\n",slot,id,state.pending.len());
                    }
                    state.cpu_leases.push(p.receipt.fence().clone(), GFP_KERNEL)?;
                    completions.push(p.completion, GFP_KERNEL)?;
                }
                if state.pending.is_empty() {state.retire_operand_probe(self)?;}
                Ok(state.pending.is_empty())
            })();
            if let Err(error) = result {
                for p in &state.pending {
                    p.receipt.fail(error);
                }
            }
            self.g16c_compute = Some(state);
            result
        })();
        if let Err(error) = result {
            self.fail_cpu_completion(error);
        }
        (result, completions)
    }
}
