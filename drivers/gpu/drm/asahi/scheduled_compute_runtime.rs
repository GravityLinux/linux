// SPDX-License-Identifier: GPL-2.0-only

//! G16C render protocol state during migration to common retained render pools.
//! Compute uses the common Queues, root rebinding and receipt implementation.

use super::super::{
    g17p_compute::work,
    g17p_compute_queues::Key,
    g17p_render::parameter_error,
    g17p_user_vm::UserVm,
    scheduled_bootstrap as boot,
    scheduled_dispatch::DispatchStateG16C,
    scheduled_generation::Generation,
    scheduled_lifetime::{ContextCloseG16C, LifetimeTickG16C, MaintenanceG16C},
    scheduled_operands as operands,
    scheduled_queues::{self, QueueSetG16C},
    scheduled_transport::{Request, Requirements, Stamp},
};
use super::{compute, Completion, RenderReceipt, Memory, Phase, Session, Vm, PendingWork, RenderWave, RetainedFrame};
use core::sync::atomic::Ordering;
use kernel::{bindings, dma_fence::RawDmaFence, io::Io, prelude::*, sync::Arc};

const PAGE: u64 = 0x4000;
const ADDRESS: u64 = 0x000003ffffffc000;
const SHARED: u64 = 0x00c000000000044b;
const PRIVATE: u64 = 0x00c0000000000443;
const GPU_READ: u64 = 0x0080000000000c8b;
const DISPATCH: u64 = 0xfffffc2001550000;

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

/// Max wire backing carried by the common render template preparation phase.
/// Root/table ownership and exact revalidation follow the Neo template contract.
pub(crate) struct RenderTemplateSeedG16C {
    owner: usize,
    pool: u32,
    context: u16,
    directory: u64,
    limit: u64,
    snapshot: super::super::g17p_user_vm::TableSnapshot,
    _root: super::super::g17p_user_vm::RootLease,
}
pub(crate) struct PreparedRenderTemplateG16C {
    seed: RenderTemplateSeedG16C,
    memory: Memory,
    resource: render::ResourcesG16C,
}

/// Speculative private backing owned by the common client-preparation worker.
/// It is tied to one unpublished client root and contains no firmware identity.
pub(crate) struct OwnerPrivateG16C {
    root: u64,
    base: u64,
    limit: u64,
    memory: Memory,
    support_pages: [u64;2],
    high_root: u64,
    private: KVec<PrivateBlock>,
    operand_buffers: KVec<u64>,
}
impl OwnerPrivateG16C {
    pub(crate) fn matches(&self,client:&compute::Client,base:u64,limit:u64)->bool {
        self.root==client.root.root() && self.base==base && self.limit==limit
    }
    pub(crate) fn build(client:&mut compute::Client,base:u64,limit:u64)->Result<Self> {
        let [table,state,buffers,end,_]=owner_layout(base,limit)?;
        let mut memory=Memory::detached();
        let support_pages=[memory.allocate(PAGE as usize)?,memory.allocate(PAGE as usize)?];
        let high_root=memory.allocate(PAGE as usize)?;
        let mut private=KVec::new();
        private_span(&mut memory,&mut private,0,operands::DIRECTORY_SIZE)?;
        private_span(&mut memory,&mut private,table-base,operands::TABLE_SIZE)?;
        private_span(&mut memory,&mut private,state-base,0x7c000)?;
        private_span(&mut memory,&mut private,end-base,0x7c000)?;
        let mut operand_buffers=KVec::with_capacity(operands::INITIAL_BUFFERS as usize,GFP_KERNEL)?;
        for i in 0..operands::INITIAL_BUFFERS as u64 {
            let offset=buffers-base+i*operands::BUFFER_STRIDE;
            private_span(&mut memory,&mut private,offset,operands::BUFFER_SIZE)?;
            operand_buffers.push(offset,GFP_KERNEL)?;
        }
        let mut directory=bytes(PAGE as usize)?;let mut entries=bytes(operands::TABLE_SIZE as usize)?;
        encode_operand_directory(base,&operand_buffers,&mut directory,&mut entries,
            |offset,data|write_blocks(&mut memory,&private,offset,data))?;
        for block in &private {
            client.root.prepare(base+block.offset,block.size)?;
            memory.clean(block.physical,block.size as usize)?;
        }
        for physical in [support_pages[0],support_pages[1],high_root] {memory.clean(physical,PAGE as usize)?;}
        Ok(Self {root:client.root.root(),base,limit,memory,support_pages,high_root,private,operand_buffers})
    }
}

/// The seven fixed wrapped-work pages have no DVA until admission commits.
pub(crate) struct WorkBackingG16C {
    memory: Memory,
    pages: [u64;7],
}
impl WorkBackingG16C {
    pub(crate) fn new()->Result<Self> {
        let mut memory=Memory::detached();let mut pages=[0;7];
        for page in &mut pages {*page=memory.allocate(PAGE as usize)?;memory.clean(*page,PAGE as usize)?;}
        Ok(Self {memory,pages})
    }
}

pub(crate) struct GeometrySeedG16C {
    owner: usize,
    pool: u32,
    context: u16,
    plan: render::GeometryPlanG16C,
    root: super::super::g17p_user_vm::RootLease,
}
pub(crate) struct PreparedScratchG16C {
    owner: usize,
    pool: u32,
    context: u16,
    plan: render::GeometryPlanG16C,
    memory: Memory,
    geometry: render::PreparedGeometryG16C,
}
impl GeometrySeedG16C {
    pub(crate) fn prepare(self)->Result<PreparedScratchG16C> {
        let (memory,geometry)=self.plan.prepare(self.root)?;
        Ok(PreparedScratchG16C {owner:self.owner,pool:self.pool,context:self.context,plan:self.plan,memory,geometry})
    }
}

struct Owner {
    pool: u32,
    client: Option<compute::Client>,
    context: u16,
    high_root: u64,
    directory: u64,
    private_limit: u64,
    operand_state: [u64;2],
    support: u64,
    support_pages: [u64; 2],
    private_next: u64,
    dispatch_initial: [u8; 32],
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
    render_count: u32,
    generation: Generation,
}
pub(super) struct RenderTicket {
    id: u64,
    receipt: Arc<RenderReceipt>,
    completion: Completion,
    started: kernel::time::Instant<kernel::time::Monotonic>,
    render_owner: usize,
    storage:u32,
    growth_tokens: KVec<u32>,
    render_work: parameter_error::RenderOwnerG16C,
    parameter_error: Option<parameter_oom::RepliedError>,
}
enum WorkEvent {Growth(parameter_growth::AcceptedGrowth),ParameterError(parameter_oom::AcceptedError)}

pub(super) struct State {
    owners: KVec<Owner>,
    queues: KVec<Queue>,
    selected: Option<usize>,
    queue_next: u64,
    metadata_next: u64,
    compute_mode: bool,
    operand_allocation_faulted: bool,
    initial_allocation_faulted: bool,
    render_ordinal: u32,
    render_work_pages: KVec<(u32,[u64;7])>,
    render_status: bool,
    render_close: u32,
}
impl State {
    /// Bounded, opt-in DATA from retained owners after a submission failure.
    /// Do not follow firmware pointers or change owners while diagnosing it.
    fn dump_render_failure(&self,session:&Session,pending:&[RenderTicket])->Result {
        // Diagnose only after a terminal error, without hot-path logging.
        let memory=session.memory.as_ref().ok_or(EIO)?;
        let vm=session.vm.as_ref().ok_or(EIO)?;
        for pending in session.independent_pending.iter().take(4) {
            let ticket=&pending.ticket;
            kernel::pr_err!("G16C: FAILED_COMPUTE_DATA grid={} value={} queue={} driver={:?} outer_done={:?} age_ms={}\n",
                ticket.grid,ticket.value,ticket.queue,ticket.driver,pending.outer_done,pending.started.elapsed().as_millis());
            let layout=super::super::scheduled_generation::Layout::new(ticket.queue,4)?;
            let mut data=[0u8;0x200];
            for (name,address,size) in [
                ("record",layout.context()+u64::from(ticket.value&255)*0x200,0x200),
                ("pointers",ticket.pointers,0x80),
                ("timestamps",ticket.status[0],16),
                ("driver",ticket.driver.ok_or(EIO)?.0,8),
            ] {
                boot::read_words(memory,vm,address,&mut data[..size])?;
                for (chunk,bytes) in data[..size].chunks(0x80).enumerate() {
                    kernel::pr_err!("G16C: FAILED_COMPUTE_OBJECT grid={} value={} name={} address={:#x} offset={:#x} DATA={:02x?}\n",
                        ticket.grid,ticket.value,name,address,chunk*0x80,bytes);
                }
            }
        }
        for ticket in pending.iter().take(4) {
            let owner=self.owners.get(ticket.render_owner).ok_or(EIO)?;
            let render=owner.render.as_ref().ok_or(EIO)?;
            let queue=self.queues.iter().find(|q|q.owner==ticket.render_owner
                && q.storage.lanes[1].queue==ticket.render_work.queue).ok_or(EIO)?;
            kernel::pr_err!("G16C: FAILED_RENDER_DATA context={} pool={} storage={} growth_tokens={:?}\n",
                owner.context,owner.pool,ticket.storage,ticket.growth_tokens);
            let mut data=[0u8;0x200];
            for (name,address,size) in [
                ("manager",render.manager,0x88),
                ("control",render.parameter.block_control,0x80),
                ("region",super::super::scheduled_initgraph::BASE+0x1548000+u64::from(owner.pool)*16,16),
                ("dispatch",DISPATCH,0x20),
                ("dispatch-owner",DISPATCH+DispatchStateG16C::slot_offset(owner.context).ok_or(EIO)? as u64,0x20),
                ("dispatch-second",DISPATCH+0x20,0x20),
                ("operand-support",owner.support,0x80),
                ("render-support",render.support,0x80),
            ] {
                boot::read_words(memory,vm,address,&mut data[..size])?;
                kernel::pr_err!("G16C: FAILED_RENDER_OBJECT name={} address={:#x} DATA={:02x?}\n",name,address,&data[..size]);
            }
            let base=super::super::g17p_render_runtime::wrapped_storage_base(ticket.storage)?;
            boot::read_words(memory,vm,base+0x18100,&mut data[..32])?;
            kernel::pr_err!("G16C: FAILED_RENDER_TIMESTAMPS pool={} storage={} DATA={:02x?}\n",owner.pool,ticket.storage,&data[..32]);
            for (stage,lane) in queue.storage.lanes[..2].iter().enumerate() {
                let milestone=ticket.receipt.milestones[stage].load(Ordering::Relaxed)as u32;
                for (name,address,size) in [
                    ("record",lane.scratch_high+u64::from(milestone&255)*0x200,0x200),
                    ("wrapper",base+if stage==0 {0x4000}else{0xc000},0xc0),
                    ("pointers",lane.pointers,0x80),
                    ("driver",ticket.render_work.driver_stamps[stage],8),
                ] {
                    boot::read_words(memory,vm,address,&mut data[..size])?;
                    for (chunk,bytes) in data[..size].chunks(0x80).enumerate() {
                        kernel::pr_err!("G16C: FAILED_RENDER_STAGE stage={} name={} address={:#x} offset={:#x} DATA={:02x?}\n",
                            stage,name,address,chunk*0x80,bytes);
                    }
                }
            }
        }
        // A dependency producer may have retired out of `pending` already.
        // Its retained queue and allocator state still distinguish a bad wait
        // from a consumer that started with another owner's cursor backing.
        for (index,owner) in self.owners.iter().enumerate().rev().take(4) {
            let Some(render)=owner.render.as_ref() else {continue;};
            let Some(queue)=self.queues.iter().find(|q|q.owner==index && q.render_count!=0) else {continue;};
            let milestone=render.sequence_base.checked_add(render.sequence).ok_or(EOVERFLOW)?;
            kernel::pr_err!("G16C: RETAINED_RENDER_DATA context={} pool={} milestone={} queue_count={} published={} client={} directory={:#x}\n",
                owner.context,owner.pool,milestone,queue.render_count,owner.published,owner.client.is_some(),owner.directory);
            let mut data=[0u8;0x80];
            for (name,address,size) in [
                ("dispatch-pool",DISPATCH+u64::from(owner.pool)*0x20,0x20),
                ("dispatch-context",DISPATCH+DispatchStateG16C::slot_offset(owner.context).ok_or(EIO)? as u64,0x20),
                ("operand-support",owner.support,0x68),
            ] {
                boot::read_words(memory,vm,address,&mut data[..size])?;
                kernel::pr_err!("G16C: RETAINED_RENDER_OBJECT context={} pool={} name={} address={:#x} DATA={:02x?}\n",
                    owner.context,owner.pool,name,address,&data[..size]);
            }
            for (stage,lane) in queue.storage.lanes[..2].iter().enumerate() {
                for (name,address,size) in [
                    ("record-head",lane.scratch_high+u64::from(milestone&255)*0x200,0x48),
                    ("record-status",lane.scratch_high+u64::from(milestone&255)*0x200+0x130,0x20),
                    ("firmware-stamp",lane.queue+0x1040,8),
                ] {
                    boot::read_words(memory,vm,address,&mut data[..size])?;
                    kernel::pr_err!("G16C: RETAINED_RENDER_STAGE context={} pool={} stage={} name={} address={:#x} DATA={:02x?}\n",
                        owner.context,owner.pool,stage,name,address,&data[..size]);
                }
            }
        }
        if let Err(error)=session.independent_compute.dump_failure(memory,vm) {
            kernel::pr_err!("G16C: retained compute DATA unavailable {:?}\n",error);
        }
        Ok(())
    }
    fn release_requested(&mut self, session: &mut Session) -> Result {
        for index in 0..self.owners.len() {
            let o=&self.owners[index];
            if o.close_requested && o.client.is_some() && session.render_pool_available(o.pool)
                && !self.queues.iter().any(|q|q.owner==index && !q.generation.idle()) {
                self.retire_owner(session,index,false)?;
            }
        }
        Ok(())
    }

    /// G16C control/SGX retirement, independent of the common root's lifetime.
    /// A private-window move retains its physical root and client for the
    /// prepared PTE commit; final release drops the same owner after receipt.
    fn retire_owner(&mut self,session:&mut Session,index:usize,keep_root:bool)->Result {
        if !session.render_pool_available(self.owners[index].pool)
            || self.queues.iter().any(|q|q.owner==index && !q.generation.idle()) {return Err(EBUSY);}
        let o=&self.owners[index];
        if !o.published {
            if let Some(r) = &mut self.owners[index].render { r.detach()?; }
            for q in &mut self.queues {
                if q.owner == index {
                    q.live = false;
                }
            }
            if !keep_root {self.owners[index].client=None;}
            return Ok(());
        }
        let memory = session.memory.as_ref().ok_or(EIO)?;
        let vm = session.vm.as_ref().ok_or(EIO)?;
        let at = session.ttbs + o.context as u64 * 16;
        memory.invalidate(at, 16)?;
        let before = [memory.read64(at)?, memory.read64(at + 8)?];
        let expected = [o.client.as_ref().ok_or(EIO)?.root.root(), o.high_root]
            .map(|root| root | ((o.context as u64) << 48) | 1);
        if before != expected {
            kernel::pr_err!("G16C: RETIRE_REJECT context={} pool={} phase=initial_root actual={:x?} expected={:x?}\n",o.context,o.pool,before,expected);
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
            .filter(|q| q.live && q.owner == index && (q.render_count != 0))
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
            kernel::pr_err!("G16C: RETIRE_REJECT context={} pool={} phase=missing_queue_receipt\n",o.context,o.pool);
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
            kernel::pr_err!("G16C: RETIRE_REJECT context={} pool={} phase=changed_root\n",o.context,o.pool);
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
            }
        }
        // No client root or BO reference may drop before this receipt.
        if let Some(r) = &mut self.owners[index].render { r.detach()?; }
        if !keep_root {drop(self.owners[index].client.take());}
        // Context retirement invalidates any engine's retained binding.
        self.leave_compute_mode();
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
        Ok(())
    }

    pub(super) fn new() -> Self {
        Self {
            owners: KVec::new(),
            queues: KVec::new(),
            selected: None,
            queue_next: scheduled_queues::START,
            metadata_next: 0xfffffc2207000000,
            compute_mode: false,
            operand_allocation_faulted: false,
            initial_allocation_faulted: false,
            render_ordinal: 0, render_work_pages: KVec::new(), render_status: false, render_close: 0,
        }
    }
    fn client(&self, key: Key) -> Option<&compute::Client> {
        self.queues
            .iter()
            .rev()
            .find(|q| q.live && q.key == key)
            .and_then(|q| self.owners[q.owner].client.as_ref())
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
fn validate_mapped_private(root:&UserVm,base:u64,blocks:&[PrivateBlock])->Result {
    for block in blocks {
        for page in (0..block.size).step_by(PAGE as usize) {
            if root.pte(base+block.offset+page)?!=(block.physical+page)|GPU_READ|(1<<54) {return Err(EIO);}
        }
    }
    Ok(())
}
fn reset_mapped_private(memory:&mut Memory,root:&UserVm,base:u64,blocks:&[PrivateBlock])->Result {
    validate_mapped_private(root,base,blocks)?;
    for block in blocks {
        memory.zero(block.physical,block.size as usize)?;
        memory.clean(block.physical,block.size as usize)?;
    }
    Ok(())
}

/// Same source-built operand directory/table for cold construction and a
/// retired physical owner's new private window. Callers allocate scratch first.
fn encode_operand_directory(base:u64,offsets:&[u64],directory:&mut[u8],entries:&mut[u8],
    mut write:impl FnMut(u64,&[u8])->Result)->Result {
    directory.fill(0);entries.fill(0);
    for (i,offset) in offsets.iter().enumerate() {
        let buffer=base+offset;
        for j in 0..2048 {
            directory[j*8..j*8+8].copy_from_slice(&(buffer+j as u64*0x1000).to_le_bytes());
        }
        write(i as u64*PAGE,directory)?;
        entries[i*64..i*64+8].copy_from_slice(&(buffer|1<<63).to_le_bytes());
    }
    let table=base+operands::DIRECTORY_SIZE+2*PAGE;
    write(table-base,entries)?;
    Ok(())
}
fn initialize_operand_data(memory:&mut Memory,vm:&Vm,root:&UserVm,base:u64,support:u64,
    offsets:&[u64],directory:&mut [u8],entries:&mut [u8])->Result<[u8;32]> {
    encode_operand_directory(base,offsets,directory,entries,
        |offset,data|write_low(memory,root,base+offset,data))?;
    initialize_operand_support(memory,vm,base,support,offsets.len())
}
fn initialize_operand_support(memory:&mut Memory,vm:&Vm,base:u64,support:u64,count:usize)->Result<[u8;32]> {
    let table=base+operands::DIRECTORY_SIZE+2*PAGE;
    let count=u32::try_from(count)?;
    let record=operands::ComputeSupportG16C::new(base,table,support+PAGE,count).ok_or(EINVAL)?;
    vm.write(memory,2,support,record.as_bytes())?;
    vm.write(memory,2,support+PAGE,&1u32.to_le_bytes())?;
    Ok(*DispatchStateG16C::initial(base,count*2048).ok_or(EINVAL)?.as_bytes())
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
    if crate::module_parameters::g16c_initial_alloc_fail.value() == stage && !*fired {
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
fn prepare_work_mappings<const N:usize>(memory:&mut Memory,vm:&mut Vm,base:u64,
    flags:[u64;N],aliases:&[usize])->Result {
    let alias=0x7000000000+base-work::DESCRIPTOR_BASE_G16C;
    for (i,_)in flags.into_iter().enumerate() {vm.prepare_g16c_empty(memory,2,base+i as u64*PAGE,PAGE as usize)?;}
    for &i in aliases {
        if i>=N {return Err(EINVAL);}
        vm.prepare_g16c_empty(memory,0,alias+i as u64*PAGE,PAGE as usize)?;
    }
    Ok(())
}
fn install_work_pages<const N: usize>(memory: &mut Memory, vm: &mut Vm, base: u64,
    flags: [u64; N], aliases: &[usize], pages: [u64; N]) -> Result {
    let alias = 0x7000000000 + base - work::DESCRIPTOR_BASE_G16C;
    for (i, flag) in flags.into_iter().enumerate() { vm.span(memory, 2, base + i as u64 * PAGE, pages[i], PAGE as usize, flag)?; }
    for &i in aliases { vm.span(memory, 0, alias + i as u64 * PAGE, pages[i], PAGE as usize, GPU_READ)?; }
    Ok(())
}
fn flight_allocation_checkpoint(fired: &mut bool) -> StageResult<()> {
    if crate::module_parameters::g16c_initial_alloc_fail.value() == 8 && !*fired {
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

fn owner_layout(base:u64,limit:u64)->Result<[u64;5]> {
    if limit.checked_sub(base).is_none_or(|span|span==0) || (base|limit)&(PAGE-1)!=0 || limit>1<<42 {return Err(EINVAL);}
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
        let private_end=end.checked_add(0x80000).ok_or(EINVAL)?;
        if private_end > limit {
            return Err(EINVAL);
        }
    Ok([table,state,buffers,end,private_end])
}

impl State {
    fn reusable_owner(&self,pool:u32,base:u64,limit:u64)->Option<usize> {
        let capacity=limit.checked_sub(base)?;
        self.owners.iter().position(|o| o.pool==pool && o.client.is_none()
            && o.private.iter().all(|b| b.offset.checked_add(b.size).is_some_and(|end| end <= capacity))
            && o.operand_buffers.iter().all(|offset| offset.checked_add(operands::BUFFER_STRIDE)
                .is_some_and(|end| end <= capacity)))
    }
    fn owner_private_required(&self,pool:u32,incoming:&compute::Client,base:u64,limit:u64)->bool {
        !self.owners.iter().any(|o|o.pool==pool&&o.client.as_ref().is_some_and(|c|same(c,incoming)))
            && self.reusable_owner(pool,base,limit).is_none()
    }
    fn build_owner(
        &mut self,
        memory: &mut Memory,
        vm: &mut Vm,
        ttbs: u64,
        asid_mask:u64,
        pool:u32,
        mut client: compute::Client,
        base: u64,
        limit: u64,
        prepared:&mut Option<OwnerPrivateG16C>,
    ) -> StageResult<usize> {
        let capacity = limit.checked_sub(base).ok_or(EINVAL)?;
        if capacity == 0 || (base | limit) & (PAGE - 1) != 0 || limit > 1 << 42 {
            return Err(EINVAL.into());
        }
        // A grown dormant pool may not fit a later caller's smaller window.
        // Check all retained extents before moving ownership or clearing RAM.
        let reusable=self.reusable_owner(pool,base,limit);
        if reusable.is_none() && self.owners.len() >= 63 {
            return Err(StageError::Rejected(ENOSPC));
        }
        let context=if let Some(i)=reusable {self.owners[i].context} else {
            let mask=self.owners.iter().fold(asid_mask,|mask,o|mask|(1u64<<o.context));
            super::super::g17p_render_lifecycle::render_asid(pool,mask).ok_or(ENOSPC)?
        };
        let invalid_roots = reusable.map_or([0; 2], |i| self.owners[i].invalid_roots);
        let [_table,state,_buffers,_end,mut private_end]=owner_layout(base,limit)?;
        // Only a context with the qualified release receipt has client=None.
        // Revalidate its exact invalid TTBR words before touching cached RAM.
        let at = ttbs + context as u64 * 16;
        memory.invalidate(at, 16)?;
        if [memory.read64(at)?, memory.read64(at + 8)?] != invalid_roots {
            return Err(EBUSY.into());
        }
        let mut detached = Memory::detached();
        if reusable.is_none() { self.owners.reserve(1, GFP_KERNEL).map_err(StageError::allocation)?; }
        let (support, support_pages, high_root, private,prepared_offsets) = if let Some(i) = reusable {
            let o = &self.owners[i];
            for (j, flags) in [PRIVATE, SHARED].into_iter().enumerate() {
                if vm.pte(memory, 2, o.support + j as u64 * PAGE)? != o.support_pages[j] | flags {
                    return Err(EIO.into());
                }
            }
            let mut blocks = KVec::new();
            blocks.reserve(o.private.len(), GFP_KERNEL).map_err(StageError::allocation)?;
            for block in &o.private { blocks.push(*block, GFP_KERNEL).expect("reserved retained private blocks"); }
            (o.support, o.support_pages, o.high_root, blocks,None)
        } else {
            let support = self.metadata_next;
            if support + 2 * PAGE > 0xfffffc2208000000 { return Err(StageError::Rejected(ENOSPC)); }
            let storage=prepared.take().ok_or(EIO)?;
            if !storage.matches(&client,base,limit) {return Err(EIO.into());}
            detached=storage.memory;
            let pages=storage.support_pages;let high_root=storage.high_root;
            let private=storage.private;
            (support, pages, high_root, private,Some(storage.operand_buffers))
        };
        let mut operand_buffers = prepared_offsets.unwrap_or_else(KVec::new);
        if let Some(i) = reusable {
            operand_buffers.reserve(self.owners[i].operand_buffers.len(), GFP_KERNEL).map_err(StageError::allocation)?;
            for &offset in &self.owners[i].operand_buffers {
                operand_buffers.push(offset, GFP_KERNEL).expect("reserved retained operand offsets");
            }
        }
        // A recycled owner retains every successfully grown buffer. Save
        // slots/render backing are placed after this complete retained span.
        for offset in &operand_buffers {
            private_end=private_end.max(base.checked_add(*offset)
                .and_then(|p|p.checked_add(operands::BUFFER_STRIDE)).ok_or(EOVERFLOW)?);
        }
        // Include retained private allocations as well as grown operands;
        // detached-root recycling must never allocate a queue over this pool.
        for block in &private {
            private_end = private_end.max(base.checked_add(block.offset)
                .and_then(|p|p.checked_add(block.size)).ok_or(EOVERFLOW)?);
        }
        if private_end>limit {return Err(StageError::Rejected(ENOSPC));}
        if reusable.is_some() {
            for block in &private {client.root.prepare(base+block.offset,block.size).map_err(StageError::allocation)?;}
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
        if reusable.is_some() {map_private(memory,&mut client.root,base,&private)?;}
        else {map_prepared_private(&mut client.root,base,&private)?;}
        for physical in [support_pages[0], support_pages[1], high_root] {
            memory.zero(physical, PAGE as usize)?;
            memory.clean(physical, PAGE as usize)?;
        }
        let dispatch_initial=if reusable.is_some() {
            let mut directory=bytes(PAGE as usize).map_err(StageError::allocation)?;
            let mut entries=bytes(operands::TABLE_SIZE as usize).map_err(StageError::allocation)?;
            initialize_operand_data(memory,vm,&client.root,base,support,&operand_buffers,&mut directory,&mut entries)?
        } else {initialize_operand_support(memory,vm,base,support,operand_buffers.len())?};
        let index = reusable.unwrap_or(self.owners.len());
        let render = reusable.and_then(|i| self.owners[i].render.take());
        let value = Owner {
            pool,
            client: Some(client),
            context,
            high_root,
            directory: base,
            private_limit: limit,
            operand_state: [state,_end],
            support,
            support_pages,
            private_next: private_end,
            dispatch_initial,
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
    fn choose_queue(&self,key:Key,client:Option<&compute::Client>,base:u64,limit:u64,
        priority:u32,pool:u32)->Option<usize> {
        super::super::scheduled_generation::select(
            self.queues.iter().enumerate().map(|(index,q)| {
                let owner=&self.owners[q.owner];
                let matching=q.live && q.key==key && q.storage.lanes[2].priority as u32==priority && owner.directory==base && owner.private_limit==limit
                    && owner.client.is_some()
                    && client.is_none_or(|next|owner.client.as_ref().is_some_and(|old|same(old,next)));
                super::super::scheduled_generation::Candidate {
                    index, reserved:false, eligible:owner.pool==pool && q.generation.idle(),
                    matches:matching, same_queue:q.key==key, idle:q.generation.idle(),
                    room:q.generation.window<super::super::g17p_compute_lifecycle::TRANSPORT_INTERVAL,
                }
            }), (self.queues.len()<Generation::G16C_COUNT).then_some(self.queues.len()))
    }
    fn prepare_queue(
        &mut self,
        session: &mut Session,
        key: Key,
        client: &mut Option<compute::Client>,
        base: u64,
        limit: u64,
        priority: u32,
        pool: u32,
        private:&mut Option<OwnerPrivateG16C>,
        transport:&mut Option<scheduled_queues::QueueBackingG16C>,
    ) -> StageResult<Option<usize>> {
        // Only the selected physical pool may change roots or transports.
        // Another pool retains its exact older caller snapshot until its own
        // GPU work, CPU visibility and producer leases have retired.
        let Some(selected)=self.choose_queue(key,client.as_ref(),base,limit,priority,pool) else {
            return Ok(None);
        };
        if selected==self.queues.len() && transport.is_none() {return Ok(None);}
        if selected<self.queues.len() {
            let index=selected;
            let q=&self.queues[index];let o=&self.owners[q.owner];
            if q.live && q.key==key && q.storage.lanes[2].priority as u32==priority && o.directory==base && o.private_limit==limit
                && o.client.is_some() && client.as_ref().is_none_or(|next|
                    o.client.as_ref().is_some_and(|old|same(old,next))) {
                if self.queues[index].generation.window==super::super::g17p_compute_lifecycle::TRANSPORT_INTERVAL {
                    if !self.queues[index].generation.idle() {return Err(EBUSY.into());}
                    self.queues[index].generation.window=0;
                }
                return Ok(Some(index));
            }
            if q.live {
                let owner=q.owner;
                self.recycle_generation(session,index)?;
                if !self.queues.iter().any(|q|q.live && q.owner==owner) {
                    self.owners[owner].close_requested=true;
                    self.release_requested(session)?;
                }
            }
        }
        let next = client.take();
        let memory = session.memory.as_mut().ok_or(EIO)?;
        let vm = session.vm.as_mut().ok_or(EIO)?;
        let owner = if let Some(next) = next {
            if let Some(i) = self.owners.iter()
                .position(|o| o.pool==pool && o.client.as_ref().is_some_and(|c| same(c, &next))) { i }
            else { self.build_owner(memory, vm, session.ttbs,session.independent_compute.asid_mask(), pool, next, base, limit,private)? }
        } else {
            // Another native binding may already hold this logical queue's
            // immutable client. Keep it pinned while adding a disjoint binding.
            self.queues.iter().rfind(|q|q.live && q.key==key && self.owners[q.owner].pool==pool && self.owners[q.owner].client.is_some())
                .map(|q|q.owner).ok_or(EIO)?
        };
        if self.owners[owner].directory != base || self.owners[owner].private_limit != limit {
            return Err(EIO.into());
        }
        let context=self.owners[owner].context;
        if context>=4 && session.independent_compute.asid_mask()&(1u64<<context)==0 {
            session.independent_compute.reserve_render_asid(context)?;
        }
        // Render transports have no compute save/robustness/operand state.
        // Their only root-private resources belong to the retained render pool.
        // Commit the exact physical transport chosen by the shared Neo
        // selector. Do not run a second Max-specific free-queue search after
        // owner construction or retirement changed the old association.
        let i = if selected<self.queues.len() {
            let i=selected;
            if self.queues[i].live || !self.queues[i].generation.idle() { return Err(EIO.into()); }
            self.queues[i].storage.reset_retained(memory, vm, priority.try_into()?)?;
            i
        } else {
            self.queues.reserve(1, GFP_KERNEL).map_err(StageError::allocation)?;
            let uuid: u16 = (self.queues.len() + 1).try_into()?;
            let (staged, prepared) = QueueSetG16C::prepare_backed(memory, vm, self.queue_next,
                priority.try_into()?, uuid, None,transport).map_err(StageError::allocation)?;
            initial_allocation_checkpoint(2, &mut self.initial_allocation_faulted, &staged)?;
            initial_allocation_checkpoint(3, &mut self.initial_allocation_faulted, &staged)?;
            memory.absorb(staged).map_err(StageError::allocation)?;
            let storage = prepared.install(memory, vm, priority.try_into()?)?;
            let i = self.queues.len();
            self.queues.push(Queue { live: false, key, owner, storage,
                render_count: 0,
                generation: Generation::g16c(i)? }, GFP_KERNEL)?;
            self.queue_next += scheduled_queues::SIZE;
            i
        };
        let q = &mut self.queues[i];
        q.live = true;
        q.key = key;
        q.owner = owner;
        q.generation.window = 0;
        q.render_count = 0;
        Ok(Some(i))
    }
    /// Reassign an idle physical generation without destroying its logical
    /// API queue. The same prefix/close/TTBR receipt used by queue destruction
    /// precedes reinitializing its transport. Accepted immutable snapshots can
    /// reconstruct that logical owner later, as in Neo generation rollover.
    fn recycle_generation(&mut self,session:&mut Session,index:usize)->Result {
        if !self.queues[index].generation.idle() {return Err(EBUSY);}
        let mut roots=KVec::new();
        let memory=session.memory.as_ref().ok_or(EIO)?;
        let vm=session.vm.as_ref().ok_or(EIO)?;
        for owner in self.owners.iter().filter(|o|o.published) {
            let at=session.ttbs+u64::from(owner.context)*16;
            memory.invalidate(at,16)?;
            let words=[memory.read64(at)?,memory.read64(at+8)?];
            let expected=[owner.client.as_ref().ok_or(EIO)?.root.root(),owner.high_root]
                .map(|root|root|(u64::from(owner.context)<<48)|1);
            if words!=expected {
                kernel::pr_err!("G16C: RECYCLE_REJECT context={} pool={} actual={:x?} expected={:x?}\n",owner.context,owner.pool,words,expected);
                return Err(EIO);
            }
            roots.push((at,words),GFP_KERNEL)?;
        }
        let oi=self.queues[index].owner;
        let q=&self.queues[index];
        if q.live && (q.render_count!=0) {
            let cursors=boot::control_cursors(memory,vm)?;
            if cursors!=[cursors[0];3] {return Err(EBUSY);}
            let prefix=memory.read_firmware32(vm.physical(memory,2,q.storage.context)?)?.to_le_bytes();
            let close=ContextCloseG16C::new(q.storage.context,prefix).ok_or(EIO)?;
            if self.owners[oi].render.as_ref().is_some_and(|r|r.attached()) {
                session.g16c_control(LifetimeTickG16C::for_render(self.render_close).bytes())?;
                self.render_close=self.render_close.wrapping_add(1);
            } else {session.g16c_close_prelude()?;}
            session.g16c_control(close.bytes())?;
            self.owners[oi].retired_queues=true;
        }
        let memory=session.memory.as_ref().ok_or(EIO)?;
        for (at,words) in roots {
            memory.invalidate(at,16)?;
            if [memory.read64(at)?,memory.read64(at+8)?]!=words {return Err(EIO);}
        }
        self.queues[index].live=false;
        self.leave_compute_mode();
        Ok(())
    }
    fn activate(&mut self, session: &mut Session, index: usize) -> Result {
        // Host policy reserves one dispatch bank per retained physical root.
        // Initialize only a newly published bank, preserving neighboring
        // firmware-owned bytes. Render FList registration selects this bank;
        // native bank identities are independent of application ASIDs.
        let memory = session.memory.as_mut().ok_or(EIO)?;
        let vm = session.vm.as_ref().ok_or(EIO)?;
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
            let offset=DispatchStateG16C::slot_offset(o.context).ok_or(EINVAL)?;
            let dispatch=vm.physical(memory,2,DISPATCH+offset as u64)?;
            // Invalidate before the partial store to preserve a neighboring
            // live context's most recent cursors in this same cache line.
            memory.invalidate(dispatch,32)?;
            memory.write(dispatch,&o.dispatch_initial)?;
            memory.clean(dispatch,32)?;
            // Retain this client in State before exposing either TTBR.
            o.published = true;
            memory.write64(
                at,
                o.client.as_ref().ok_or(EIO)?.root.root() | ((o.context as u64) << 48) | 1,
            )?;
            memory.write64(at + 8, o.high_root | ((o.context as u64) << 48) | 1)?;
            memory.clean(at, 16)?;
        }
        // FList registration binds the render support to its owned bank.
        // Other banks retain their directory and firmware-written cursors.
        self.selected=Some(index);
        vm.flush_tables(memory)?;
        Vm::invalidate_gpu();
        // Seed diagnostics before this fresh pool can be used by either
        // rendering or compute, including a recycled render-first owner.
        self.seed_operand_probe(session,index)?;
        Ok(())
    }
    fn leave_compute_mode(&mut self) {
        // Native client recycling closes the logical context but retains its
        // SKSM binding/count. The next publication resumes it after new DATA
        // is ready; only the first use performs the full binding transaction.
        self.compute_mode=false;
    }
}

impl Session {
    pub(super) fn require_g16c_render_owner_idle(&self,owner:(u64,u32))->Result {
        let state=self.g16c_compute.as_ref().ok_or(EIO)?;
        if state.owners.iter().any(|o|o.client.as_ref().is_some_and(|c|c.owner==owner)
            && !self.render_pool_available(o.pool)) {return Err(EBUSY);}
        Ok(())
    }

    pub(super) fn bind_g16c_compute(&mut self,grid:u8,address:u64,resume:bool)->Result {
        let mode=self.g16c_compute.as_ref().ok_or(EIO)?.compute_mode;
        self.configure_g16c_engines(&[BindingG16C::compute(grid,address).ok_or(EINVAL)?],resume,!resume||!mode)?;
        self.g16c_compute.as_mut().ok_or(EIO)?.compute_mode=true;Ok(())
    }
    pub(crate) fn poll_scheduled_g16c_work(&mut self,dev:&kernel::device::Device)->(Result<bool>,KVec<Completion>) {
        let (result,completions)=self.poll_g16c_work(dev);
        self.completions=completions;
        self.defer_cache=true;
        let result=result.and_then(|render|self.poll_scheduled_compute(dev,None).map(|compute|render&&compute));
        self.defer_cache=false;
        if let Err(error)=result {self.fail_cpu_completion(error);}
        (result,core::mem::take(&mut self.completions))
    }

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
    pub(super) fn commit_g16c_scratch(&mut self,prepared:PreparedScratchG16C)->Result<bool> {
        let mut state=self.g16c_compute.take().ok_or(EIO)?;
        let result=state.commit_render_scratch(self,prepared);
        self.g16c_compute=Some(state);self.finish_g16c_admission(result)
    }
    pub(crate) fn g16c_render_storage_needed(&self,incoming:&compute::Client,p:&super::super::g17p_render::Parameters,
        key:Key,priority:u32)->(bool,bool) {
        let Some(state)=self.g16c_compute.as_ref() else {return (true,true);};
        let queue=state.render_pool(self,incoming,p).is_some_and(|pool|
            state.choose_queue(key,Some(incoming),p.kernel_start,p.kernel_end,priority,pool)==Some(state.queues.len()));
        let work=self.next_wrapped_render_storage(state.render_ordinal).is_some_and(|slot|
            !state.render_work_pages.iter().any(|(old,_)|*old==slot));
        (queue,work)
    }
    pub(crate) fn g16c_render_needs_private(&self,incoming:&compute::Client,p:&super::super::g17p_render::Parameters)->bool {
        let Some(state)=self.g16c_compute.as_ref() else {return true;};
        state.render_needs_private(self,incoming,p)
    }
    pub(super) fn commit_g16c_render_template(&mut self,incoming:&compute::Client,
        p:&super::super::g17p_render::Parameters,prepared:PreparedRenderTemplateG16C)->Result<bool> {
        let mut state=self.g16c_compute.take().ok_or(EIO)?;
        let result=state.commit_render_template(self,incoming,p,prepared);
        self.g16c_compute=Some(state);
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
    pub(crate) fn poll_g16c_work(&mut self,dev:&kernel::device::Device) -> (Result<bool>, KVec<Completion>) {
        self.reap_render_cpu_leases();
        self.defer_cache=true;
        let result = (|| -> Result<bool> {
            if self.phase != Phase::Running || self.peers.iter().any(|p|p.data.crashed.load(Ordering::Acquire)) {
                return Err(EIO);
            }
            let mut state=self.g16c_compute.take().ok_or(EIO)?;
            let mut wave=match self.pending.take() {
                Some(PendingWork::WrappedRender(wave))=>wave,
                None=>RenderWave {frames:KVec::new(),next:0},
                Some(other)=>{self.pending=Some(other);self.g16c_compute=Some(state);return Err(EIO);},
            };
            let result=(|| -> Result<bool> {
                let mut accepted=None;
                let ttbs=self.ttbs;
                boot::drain_with_work_event(self.memory.as_mut().ok_or(EIO)?,
                    self.vm.as_ref().ok_or(EIO)?, &mut |memory,vm,raw| {
                        let result=match u32::from_le_bytes(raw[..4].try_into().unwrap()) {
                            6=>state.accept_growth(memory,vm,ttbs,&mut wave.frames,raw).map(WorkEvent::Growth),
                            7=>state.accept_parameter_error(memory,vm,ttbs,&wave.frames,raw).map(WorkEvent::ParameterError),
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
                        WorkEvent::ParameterError(request)=>state.service_parameter_error(self,&mut wave.frames,request),
                    }.map_err(|error|{kernel::pr_err!("G16C: work event service failed {:?}\n",error);error})?;
                }
                let done=wave.poll(self,dev,None)?;
                if done {state.retire_operand_probe(self)?;}
                Ok(done)
            })();
            if let Err(error)=result {
                if let Err(dump_error)=state.dump_render_failure(self,&wave.frames) {
                    kernel::pr_err!("G16C: retained render DATA unavailable {:?}\n",dump_error);
                }
                wave.fail(error);
            }
            if !wave.frames.is_empty() {self.pending=Some(PendingWork::WrappedRender(wave));}
            self.g16c_compute=Some(state);
            result
        })();
        self.defer_cache=false;
        if let Err(error)=result {self.fail_cpu_completion(error);}
        (result,core::mem::take(&mut self.completions))
    }
}

impl RetainedFrame for RenderTicket {
    type Cache=();
    fn receipt(&self)->&Arc<RenderReceipt> {&self.receipt}
    fn fence(&self)->&kernel::dma_fence::Fence {&self.receipt.fence}
    fn pool(&self)->u32 {self.render_work.slot}
    fn storage(&self)->u32 {self.storage}
    fn poll(&mut self,session:&mut Session,_dev:&kernel::device::Device,image:Option<&super::Image>,_sole:bool)->Result<bool> {
        if image.is_some() {return Err(EIO);}
        if self.started.elapsed().as_millis()>=5000 {return Err(ETIMEDOUT);}
        session.poll_g16c(self.id).map_err(|error|{
            kernel::pr_err!("G16C: flight poll {} failed {:?}\n",self.id,error);error
        })
    }
    fn cache_seed(&self,_session:&Session)->Result {Ok(())}
    fn release(&mut self,session:&mut Session)->Result {
        session.release_g16c(self.id).map_err(|error|{
            kernel::pr_err!("G16C: flight release {} failed {:?}\n",self.id,error);error
        })?;
        if let Some(error)=self.parameter_error.take() {
            self.completion.error=Some(ENOMEM);
            kernel::dev_info!(session.peers[0].data.dev.as_ref(),
                "G16C: retired parameter ENOMEM context={} slot={} stamp={} driver_before={:?} work_quiescent=1 error9_retired=1\n",
                error.owner.context,error.owner.slot,error.owner.stamp,error.before_driver);
        }
        Ok(())
    }
    fn completion(self,_cache:())->Completion {self.completion}
}
