// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Per-command publication and completion. Queue history names accepted work,
//! including work blocked on an external fence. Dependencies never reserve an
//! engine or prevent unrelated commands in the same buffer from being scanned.

use super::{admission, Command, Device, Prepared, Runtime};
use super::super::{g17p_boot::{ComputeReceipt, RenderReceipt}, g17p_sync};
use kernel::{bindings, dma_fence::{Fence, RawDmaFence}, new_mutex, prelude::*, sync::{Arc, Mutex}};

static PREPARATION_PAUSED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

// Source timing is opt-in; default execution performs no timing/logging.
// Every interval starts only after its corresponding device-lock acquisition.
struct RuntimeInterval {
    started: Option<kernel::time::Instant<kernel::time::Monotonic>>,
    wait_ns: i64,
    key: super::super::g17p_compute_queues::Key,
    phase: &'static str,
}
impl RuntimeInterval {
    fn waiting() -> Option<kernel::time::Instant<kernel::time::Monotonic>> {
        (*crate::module_parameters::submission_log.value() >= 3)
            .then(kernel::time::Instant::now)
    }
    fn acquired(before: Option<kernel::time::Instant<kernel::time::Monotonic>>,
        key: super::super::g17p_compute_queues::Key, phase: &'static str) -> Self {
        Self { wait_ns: before.as_ref().map(|t| t.elapsed().as_nanos()).unwrap_or(0),
            started: before.map(|_| kernel::time::Instant::now()), key, phase }
    }
}
impl Drop for RuntimeInterval {
    fn drop(&mut self) {
        if let Some(started) = &self.started {
            pr_info!("G17P: RUNTIME_INTERVAL queue {:?} phase {} wait_ns {} hold_ns {}\n",
                self.key, self.phase, self.wait_ns, started.elapsed().as_nanos());
        }
    }
}

static SUBMISSION_PROFILE_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

pub(super) type History = [Option<Arc<Point>>; 2];

fn status(fence: &Fence) -> i32 {
    // SAFETY: The caller holds a live fence reference throughout this read.
    unsafe { bindings::dma_fence_get_status(fence.raw()) }
}

enum Receipt {
    Render(Arc<RenderReceipt>),
    Compute(Arc<ComputeReceipt>),
}
impl Receipt {
    fn fence(&self) -> &Fence {
        match self { Self::Render(r) => &r.fence, Self::Compute(r) => &r.fence }
    }
    fn published(&self) -> bool {
        match self { Self::Render(r) => r.is_published(), Self::Compute(r) => r.is_published() }
    }
    fn milestone(&self, stage: usize) -> Option<(u8,u32)> {
        match self {
            Self::Render(r) => r.milestone(stage),
            Self::Compute(r) => r.milestone(),
        }.map(|p| (p.grid,p.value))
    }
}

#[pin_data(PinnedDrop)]
pub(super) struct Point {
    #[pin]
    previous: Mutex<Option<Arc<Point>>>,
    publication: Fence,
    completion: Fence,
    // Fence references preserve producer errors without a recursive Point DAG.
    // Hardware publication uses milestones; CPU completion joins only the
    // dependency results before authorizing a successful userspace fence.
    dependencies: [Option<Fence>; 2],
    #[pin]
    receipt: Mutex<Option<Receipt>>,
}
#[pinned_drop]
impl PinnedDrop for Point {
    fn drop(self: Pin<&mut Self>) {
        // Failed unpublished points retain their predecessor for ordering.
        // An arbitrarily long accepted queue may fail before any GPU work is
        // published. Reclaim the unique prefix iteratively, never recursively
        // through Point -> previous -> Point on the kernel stack.
        let mut previous = self.previous.lock().take();
        while let Some(point) = previous {
            // This consumes the reference without running a destructor when
            // another owner exists, including a concurrent final-owner drop.
            let Some(point) = Arc::into_unique_or_drop(point) else { break; };
            previous = point.previous.lock().take();
            // Its own pinned destructor now sees an empty predecessor link.
            drop(point);
        }
    }
}
impl Point {
    fn new(previous: Option<Arc<Point>>, dependencies: [Option<Fence>; 2]) -> Result<Arc<Self>> {
        Arc::pin_init(try_pin_init!(Self {
            previous<-new_mutex!(previous), publication:g17p_sync::work_fence()?, completion:g17p_sync::work_fence()?,
            dependencies, receipt<-new_mutex!(None),
        }),GFP_KERNEL)
    }
    fn finish(&self, error: Option<Error>) {
        if status(&self.completion) == 0 {
            if let Some(error) = error { self.completion.set_error(error); }
            else { self.previous.lock().take(); }
            self.completion.signal();
        }
        self.publication.signal();
    }
    fn refresh(&self) {
        let held = self.receipt.lock();
        if let Some(receipt) = held.as_ref() {
            let completed = status(receipt.fence());
            if completed < 0 {
                self.finish(Some(Error::from_errno(completed)));
            } else if completed > 0 {
                let mut pending = false;
                let mut error = None;
                for fence in self.dependencies.iter().flatten() {
                    let state = status(fence);
                    pending |= state == 0;
                    if state < 0 { error.get_or_insert(Error::from_errno(state)); }
                }
                if error.is_some() || !pending { self.finish(error); }
                else {
                    // The GPU consumer retired, but producer error attribution
                    // may still be making CPU-visible results available. Do
                    // not turn an unresolved/failed producer into success.
                    self.previous.lock().take();
                    self.publication.signal();
                }
            } else if receipt.published() {
                self.previous.lock().take();
                self.publication.signal();
            }
        }
    }
    fn accepted_history(mut point: Option<Arc<Self>>) -> Option<Arc<Self>> {
        while let Some(current) = &point {
            current.refresh();
            if status(&current.completion) >= 0 || current.receipt.lock().is_some() { break; }
            // A submission rejected before publication contributes no GPU
            // history to a later ioctl. Already accepted dependents retain
            // their original point and still observe its error.
            let previous = current.previous.lock().clone();
            point = previous;
        }
        point
    }
    fn ordered(point: &Arc<Self>, retire: bool) -> Result<Option<Option<(u8,u32)>>> {
        let mut current = point.clone();
        loop {
            current.refresh();
            if status(&current.completion) >= 0 {
                return current.dependency(0,false,retire);
            }
            // A skipped/failed command contributed no engine milestone.
            // Preserve ordering through it to the last real predecessor.
            let previous = current.previous.lock().clone();
            let Some(previous) = previous else { return Ok(Some(None)); };
            current = previous;
        }
    }
    // Outer None means not published yet. Inner None means already retired:
    // never encode a stamp belonging to a context which may have been replaced.
    fn dependency(&self, stage: usize, propagate: bool, retire: bool) -> Result<Option<Option<(u8,u32)>>> {
        self.refresh();
        let completed = status(&self.completion);
        if completed < 0 && propagate { return Err(Error::from_errno(completed)); }
        if completed != 0 { return Ok(Some(None)); }
        if retire { return Ok(None); }
        let held = self.receipt.lock();
        // A Point can remain unresolved while its own GPU receipt has already
        // retired: userspace success still joins producer error attribution.
        // That join does not pin this Point's retired firmware context. Never
        // encode its old grid/value after the physical owner can be reused.
        if held.as_ref().is_some_and(|r| status(r.fence()) > 0) {
            return Ok(Some(None));
        }
        Ok(held.as_ref().filter(|r| r.published()).and_then(|r| r.milestone(stage)).map(Some))
    }
}

struct Node {
    point: Arc<Point>,
    previous: Option<Arc<Point>>,
    dependencies: History,
    staged: bool,
}
pub(super) struct Schedule {
    nodes: KVec<Node>,
    tail: History,
    control_waiting: bool,
    submission_started: Option<kernel::time::Instant<kernel::time::Monotonic>>,
    profile_id: u64,
}
impl Schedule {
    pub(super) fn enabled() -> bool {
        *crate::module_parameters::native_render_vms.value() == 0
            && *crate::module_parameters::native_compute_vms.value() == 0
            && *crate::module_parameters::native_barriers.value() == 0
    }
    pub(super) fn new(commands: &[Command], barriers: &[[u16;2]], seed: History, submission_started: Option<kernel::time::Instant<kernel::time::Monotonic>>) -> Result<Self> {
        let seed = seed.map(Point::accepted_history);
        let mut nodes: KVec<Node> = KVec::with_capacity(commands.len(),GFP_KERNEL)?;
        let mut tail = seed.clone();
        for (index,command) in commands.iter().enumerate() {
            let engine = usize::from(matches!(command,Command::Compute(_)));
            let mut dependencies: History = [None,None];
            for e in 0..2 {
                dependencies[e] = match barriers[index][e] {
                    u16::MAX => None,
                    0 => seed[e].clone(),
                    n => {
                        let prior = commands[..index].iter().enumerate()
                            .filter(|(_,c)| usize::from(matches!(c,Command::Compute(_))) == e)
                            .nth(n as usize-1).ok_or(EINVAL)?.0;
                        Some(nodes[prior].point.clone())
                    }
                };
            }
            let dependency_fences = dependencies.each_ref().map(|point|
                point.as_ref().map(|point| point.completion.clone()));
            let point = Point::new(tail[engine].clone(), dependency_fences)?;
            nodes.push(Node {point:point.clone(),previous:tail[engine].clone(),dependencies,staged:false},GFP_KERNEL)?;
            tail[engine] = Some(point);
        }
        Ok(Self {nodes,tail,control_waiting:false,submission_started,profile_id: if submission_started.is_some() { SUBMISSION_PROFILE_ID.fetch_add(1,core::sync::atomic::Ordering::Relaxed) } else { 0 }})
    }
    pub(super) fn control_waiting(&self) -> bool { self.control_waiting }
    pub(super) fn tail(&self) -> History { self.tail.clone() }
    pub(super) fn callback_fences(&self) -> Result<KVec<Fence>> {
        let mut fences = KVec::new();
        for node in &self.nodes {
            for point in [node.previous.as_ref(),node.dependencies[0].as_ref(),node.dependencies[1].as_ref()].into_iter().flatten() {
                fences.push(point.publication.clone(),GFP_KERNEL)?;
                fences.push(point.completion.clone(),GFP_KERNEL)?;
            }
        }
        Ok(fences)
    }
    pub(super) fn fail_unstaged(&self, error: Error) {
        for node in &self.nodes {
            if !node.staged { node.point.finish(Some(error)); }
        }
    }
    fn register(runtime: &mut Runtime, prepared: &Prepared, sync: &g17p_sync::Plan) -> Result {
        // All address/command validation already happened before acceptance.
        // Registration pins ownership and aliases; root activation belongs to
        // each runnable engine command, never to the aggregate submission.
        let mut addresses = KVec::new();
        for command in &prepared.parameters {
            match command {
                Command::Render(p) => {
                    for address in [p.ta_user_timestamp_start,p.ta_user_timestamp_end,
                        p.fragment_user_timestamp_start,p.fragment_user_timestamp_end] {
                        addresses.push(address,GFP_KERNEL)?;
                    }
                }
                Command::Compute(p) => {
                    for address in p.timestamps { addresses.push(address,GFP_KERNEL)?; }
                }
            }
        }
        for buffer in &prepared.buffers { runtime.session.retain_source_backing(buffer)?; }
        runtime.session.begin_submission(&sync.fence(),&addresses)
    }
    fn replacement(snapshot: &admission::Snapshot,
                   old: Option<&super::super::g17p_compute_runtime::Client>,
                   prepared: &mut Option<super::super::g17p_compute_runtime::Client>)
                   -> Result<(bool, Option<super::super::g17p_compute_runtime::Client>)> {
        // Another worker may have changed the selected owner since the seed
        // was captured. Recheck under publication locking. A missing private
        // preparation retries only this node; never allocate/copy a tree here.
        if snapshot.same_client(old) { return Ok((true, None)); }
        let Some(client) = prepared.take() else { return Ok((false, None)); };
        if !snapshot.same_client(Some(&client)) { return Err(EIO); }
        Ok((true, Some(client)))
    }
    fn prepare_clients(&self, dev: &Device, prepared: &mut Prepared) -> Result {
        let mut considered = [false; 2];
        for (index, node) in self.nodes.iter().enumerate() {
            if node.staged || status(&node.point.completion) != 0 { continue; }
            let command = &prepared.parameters[index];
            let engine = usize::from(matches!(command, Command::Compute(_)));
            if considered[engine] { continue; }
            if !matches!(Self::dependencies(node, command), Ok(Some(_))) { continue; }
            considered[engine] = true;
            let snapshot = if engine == 0 { prepared.render_snapshot.as_ref() } else { prepared.compute_snapshot.as_ref() }
                .ok_or(EIO)?;
            let (needed, private, cache) = {
                let preparation_wait = RuntimeInterval::waiting();
                let held = dev.runtime.lock();
                let _preparation_interval = RuntimeInterval::acquired(preparation_wait, (0,0), "preparation-state");
                let runtime = Option::as_ref(&*held).ok_or(ENODEV)?;
                let (old, private) = match command {
                    Command::Render(_) => (runtime.session.render_client()?, false),
                    Command::Compute(p) if runtime.session.independent_compute_enabled() =>
                        (runtime.session.independent_compute_client(prepared.queue_key, snapshot.client(), p, prepared.queue_priority),
                         runtime.session.compute_needs_private(prepared.queue_key, snapshot.client(), p, prepared.queue_priority)),
                    Command::Compute(_) => (runtime.session.compute_client()?, false),
                };
                let selected = if let Command::Render(p) = command {
                    let mut parameters = *p;
                    parameters.queue_owner = Some(prepared.queue_key);
                    runtime.session.render_selected_client(snapshot.client(), &parameters)?
                } else { old };
                (!snapshot.same_client(old) || !snapshot.same_client(selected), private,
                    selected.map(|client| client.cpu_maps.clone()))
            };
            let slot = if engine == 0 { &mut prepared.render_client } else { &mut prepared.compute_client };
            if slot.is_none() && needed {
                if *crate::module_parameters::cpu_prepare_pause_queue.value() == prepared.queue_key.1
                    && !PREPARATION_PAUSED.swap(true, core::sync::atomic::Ordering::AcqRel) {
                    pr_info!("G17P: CPU_PREPARE_PAUSE_BEGIN queue {:?} outside_runtime_lock\n", prepared.queue_key);
                    kernel::time::delay::fsleep(kernel::time::Delta::from_millis(2000));
                    pr_info!("G17P: CPU_PREPARE_PAUSE_END queue {:?} outside_runtime_lock\n", prepared.queue_key);
                }
                let mut client = snapshot.deferred_client()?;
                snapshot.materialize(&mut client, engine == 0)?;
                let extra = match command {
                    Command::Render(p) => super::super::g17p_render_runtime::scratch_table_budget(p)?,
                    // The fixed Source private save-state shape, including
                    // an unaligned aperture and its upper parent table.
                    Command::Compute(_) => (256 * 0x78000 + (1 << 25) - 1) / (1 << 25) + 2,
                };
                client.root.prepare_spare_tables(client.root.table_count().checked_add(extra).ok_or(EOVERFLOW)?)?;
                client.cache(false)?;
                *slot = Some(client);
            }
            if needed {
                if let (Some(client), Some(cache)) = (slot.as_ref(), cache.as_ref()) {
                    // Populate the selected installed owner's retained maps
                    // before its short PTE commit. Rebind then needs no vmap
                    // allocation while holding the device mutex.
                    client.cache_with(false, cache)?;
                }
            }
            if engine == 1 && !private && prepared.compute_storage.is_some() {
                // A competing worker may have installed/released an owner
                // while this job built speculative private backing. Neither
                // the pages nor their scratch PTEs were published. Drop both
                // outside locking; a later new-owner plan rematerializes its
                // immutable caller tree before allocating private leaves.
                drop(slot.take());
                drop(prepared.compute_storage.take());
            }
            if private && prepared.compute_storage.is_none() {
                let Command::Compute(parameters) = command else { return Err(EIO); };
                let mut memory = super::super::g17p_memory::Memory::detached();
                super::super::g17p_compute_runtime::build_independent_client(&mut memory,
                    slot.as_mut().ok_or(EIO)?, parameters)?;
                prepared.compute_storage = Some(memory);
            }
        }
        Ok(())
    }
    fn dependencies(node: &Node, command: &Command) -> Result<Option<(Option<(u8,u32)>,[Option<(u8,u32)>;2])>> {
        let previous = if let Some(point) = &node.previous {
            let Some(value) = Point::ordered(point,false)? else { return Ok(None); };
            value
        } else { None };
        let mut dependencies = [None,None];
        for engine in 0..2 {
            if let Some(point) = &node.dependencies[engine] {
                // A genuine dependency must not occupy the single CS queue
                // head and stall independent work behind it. Only the proven
                // fragment barrier lets its TA publish before FR is ready.
                let early_fragment = matches!(command,Command::Render(p) if p.vdm_barrier_fragment) && engine == 0;
                // A published CS predecessor is already ahead on this same
                // hardware queue, so its GPU stamp adds no head-of-line wait.
                // Keep such chains queued instead of round-tripping through
                // CPU completion for every short compute command.
                let queued_compute = matches!(command,Command::Compute(_)) && engine == 1;
                let queued_firmware = *crate::module_parameters::firmware_dependencies.value() == 1;
                let Some(value) = point.dependency(1,true,!(early_fragment || queued_compute || queued_firmware))? else { return Ok(None); };
                dependencies[engine] = value;
            }
        }
        Ok(Some((previous,dependencies)))
    }
    fn prepare_render(&self, dev: &Device, prepared: &Prepared)
        -> Option<(usize,KBox<super::super::g17p_render_runtime::PreparedAppend>)> {
        for (index,node) in self.nodes.iter().enumerate() {
            if node.staged || status(&node.point.completion) != 0 { continue; }
            let Command::Render(mut parameters) = prepared.parameters[index] else { continue; };
            let Ok(Some((previous,dependencies))) = Self::dependencies(node,&prepared.parameters[index]) else { continue; };
            parameters.queue_owner=Some(prepared.queue_key);
            parameters.prior_queue_ta=previous;
            parameters.vdm_dependency=dependencies[0];
            parameters.cdm_dependency=dependencies[1];
            let seed = {
                let preparation_wait = RuntimeInterval::waiting();
                let mut held = dev.runtime.lock();
                let _preparation_interval = RuntimeInterval::acquired(preparation_wait, (0,0), "preparation-state");
                let runtime = Option::as_mut(&mut *held)?;
                let snapshot = prepared.render_snapshot.as_ref()?;
                if !runtime.session.can_stage_render(Some(snapshot.client()),&parameters).ok()? { continue; }
                runtime.session.claim_render_preparation(&parameters,snapshot.client()).ok()??
            };
            // CPU allocation/serialization holds neither the device mutex nor
            // a hardware reservation. Commit revalidates the full seed and
            // rebuilds it if another job changed the next item in the meantime.
            match super::super::g17p_render_runtime::PreparationSeed::prepare(seed) {
                Ok(objects) => return Some((index, objects)),
                Err(error) => { node.point.finish(Some(error)); continue; },
            }
        }
        None
    }
    fn preferred(runtime: &mut Runtime, engine: usize, node: &Node) -> bool {
        let preference = &mut runtime.engine_handoff[engine];
        if preference.as_ref().is_some_and(|f| status(f)!=0) { *preference=None; }
        preference.as_ref().is_none_or(|f| f.raw()==node.point.publication.raw())
    }
    fn handoff(runtime: &mut Runtime, engine: usize, node: &Node, replacing: bool) {
        // Stop extending only the engine whose installed ASID must change.
        // This preference is taken after dependency resolution, so a job
        // waiting on caller work cannot reserve an otherwise usable engine.
        if replacing && runtime.engine_handoff[engine].is_none() {
            runtime.engine_handoff[engine]=Some(node.point.publication.clone());
        }
    }
    fn prepare_compute(&self, dev: &Device, prepared: &Prepared)
        -> Result<Option<(usize, KBox<super::super::g17p_compute_queues::PreparedWork>)>> {
        for (index, node) in self.nodes.iter().enumerate() {
            if node.staged || status(&node.point.completion) != 0 { continue; }
            let Command::Compute(parameters) = &prepared.parameters[index] else { continue; };
            // Per-command dependency errors are handled by publication's
            // scan; one failed dependency must not abort independent nodes.
            let Ok(Some((previous, dependencies))) = Self::dependencies(node, &prepared.parameters[index]) else { continue; };
            let mut waits = [(0, 0); 3]; let mut count = 0;
            for point in [previous, dependencies[0], dependencies[1]].into_iter().flatten() {
                if !waits[..count].contains(&point) { waits[count] = point; count += 1; }
            }
            let (seed, stamps) = {
                let preparation_wait = RuntimeInterval::waiting();
                let held = dev.runtime.lock();
                let _preparation_interval = RuntimeInterval::acquired(preparation_wait, (0,0), "preparation-state");
                let runtime = Option::as_ref(&*held).ok_or(ENODEV)?;
                if !runtime.session.independent_compute_enabled() { return Ok(None); }
                let snapshot = prepared.compute_snapshot.as_ref().ok_or(EIO)?;
                (runtime.session.compute_preparation_seed(prepared.queue_key, snapshot.client(),
                    parameters, prepared.queue_priority, &waits[..count])?,
                 runtime.session.timestamp_cache_seed(&parameters.timestamps)?)
            };
            if let Some(seed) = seed {
                if let Err(error) = prepared.compute_snapshot.as_ref().ok_or(EIO)?.client().cache(false)
                    .and_then(|()| stamps.run(false)) {
                    node.point.finish(Some(error)); continue;
                }
                match seed.prepare() {
                    Ok(objects) => return Ok(Some((index, objects))),
                    Err(error) => { node.point.finish(Some(error)); continue; },
                }
            }
        }
        Ok(None)
    }
    fn prepare_render_scratch(&self, dev: &Device, prepared: &Prepared) -> Result {
        for (index, node) in self.nodes.iter().enumerate() {
            if node.staged || status(&node.point.completion) != 0 { continue; }
            let Command::Render(parameters) = &prepared.parameters[index] else { continue; };
            if !matches!(Self::dependencies(node, &prepared.parameters[index]), Ok(Some(_))) { continue; }
            let seed = {
                let preparation_wait = RuntimeInterval::waiting();
                let held = dev.runtime.lock();
                let _preparation_interval = RuntimeInterval::acquired(preparation_wait, (0,0), "preparation-state");
                Option::as_ref(&*held).ok_or(ENODEV)?.session.render_scratch_seed(parameters)?
            };
            if let Some(seed) = seed {
                let scratch = match seed.prepare() {
                    Ok(scratch) => scratch,
                    Err(error) => { node.point.finish(Some(error)); continue; },
                };
                let preparation_wait = RuntimeInterval::waiting();
                let mut held = dev.runtime.lock();
                let _preparation_interval = RuntimeInterval::acquired(preparation_wait, (0,0), "preparation-state");
                Option::as_mut(&mut *held).ok_or(ENODEV)?.session.commit_render_scratch(scratch)?;
            }
        }
        Ok(())
    }
    fn prepare_render_templates(&self, dev: &Device, prepared: &Prepared) -> Result {
        for (index, node) in self.nodes.iter().enumerate() {
            if node.staged || status(&node.point.completion) != 0 { continue; }
            let Command::Render(mut parameters) = prepared.parameters[index] else { continue; };
            parameters.queue_owner = Some(prepared.queue_key);
            if !matches!(Self::dependencies(node, &prepared.parameters[index]), Ok(Some(_))) { continue; }
            let snapshot = prepared.render_snapshot.as_ref().ok_or(EIO)?;
            let count = {
                let preparation_wait = RuntimeInterval::waiting();
                let held = dev.runtime.lock();
                let _preparation_interval = RuntimeInterval::acquired(preparation_wait, (0,0), "preparation-state");
                Option::as_ref(&*held).ok_or(ENODEV)?.session.render_template_size(snapshot.client(), &parameters)?
            };
            let Some(count) = count else { continue; };
            let storage = super::super::g17p_user_vm::TableStorage::new(count)?;
            let seed = {
                let preparation_wait = RuntimeInterval::waiting();
                let held = dev.runtime.lock();
                let _preparation_interval = RuntimeInterval::acquired(preparation_wait, (0,0), "preparation-state");
                Option::as_ref(&*held).ok_or(ENODEV)?.session.capture_render_template(snapshot.client(), &parameters, storage)?
            };
            if let Some(seed) = seed {
                let template = match seed.prepare() {
                    Ok(template) => template,
                    Err(error) => { node.point.finish(Some(error)); continue; },
                };
                let preparation_wait = RuntimeInterval::waiting();
                let mut held = dev.runtime.lock();
                let _preparation_interval = RuntimeInterval::acquired(preparation_wait, (0,0), "preparation-state");
                Option::as_mut(&mut *held).ok_or(ENODEV)?.session.commit_render_template(snapshot.client(), &parameters, template)?;
            }
        }
        Ok(())
    }

    fn prepare_shared_backing(&self, dev: &Device) -> Result {
        let plan = {
            let preparation_wait = RuntimeInterval::waiting();
            let held = dev.runtime.lock();
            let _preparation_interval = RuntimeInterval::acquired(preparation_wait, (0,0), "preparation-state");
            Option::as_ref(&*held).ok_or(ENODEV)?.session.shared_backing_plan()?
        };
        if !plan.iter().any(|(_, count)| *count != 0) { return Ok(()); }
        let mut memory = super::super::g17p_memory::Memory::detached();
        for (size, count) in plan {
            if let Err(error) = memory.prepare_blocks(size, count) {
                if error == ENOMEM {
                    // Attribute a real backing shortage through the owned
                    // growth refusal/report protocol; it does not fail an
                    // unrelated worker that happened to notice the request.
                    let preparation_wait = RuntimeInterval::waiting();
                    let mut held = dev.runtime.lock();
                    let _preparation_interval = RuntimeInterval::acquired(preparation_wait, (0,0), "preparation-state");
                    Option::as_mut(&mut *held).ok_or(ENODEV)?.session.shared_backing_refused()?;
                    drop(_preparation_interval); drop(held); drop(memory); return Ok(());
                }
                return Err(error);
            }
        }
        let unused = {
            let preparation_wait = RuntimeInterval::waiting();
            let mut held = dev.runtime.lock();
            let _preparation_interval = RuntimeInterval::acquired(preparation_wait, (0,0), "preparation-state");
            Option::as_mut(&mut *held).ok_or(ENODEV)?.session.commit_shared_backing(memory)?
        };
        drop(unused); // Never free unpublished speculative blocks under locking.
        Ok(())
    }

    pub(super) fn progress(&mut self, dev: &Device, prepared: &mut Prepared,
        sync: &g17p_sync::Plan, initialized: &mut bool, cursor: &mut usize,
        publication: &Fence) -> Result<Option<Option<Error>>> {
        self.control_waiting = false;
        let profile_started = (*crate::module_parameters::submission_log.value() >= 3)
            .then(kernel::time::Instant::<kernel::time::Monotonic>::now);
        self.prepare_shared_backing(dev)?;
        self.prepare_clients(dev, prepared)?;
        self.prepare_render_scratch(dev, prepared)?;
        self.prepare_render_templates(dev, prepared)?;
        self.prepare_clients(dev, prepared)?;
        let mut prepared_compute = self.prepare_compute(dev, prepared)?;
        let mut prepared_append = self.prepare_render(dev, prepared);
        let seed_us = profile_started.as_ref().map(|s| s.elapsed().as_nanos()/1000).unwrap_or(0);
        let lock_started = RuntimeInterval::waiting();
        let mut held = dev.runtime.lock();
        let interval = RuntimeInterval::acquired(lock_started, prepared.queue_key, "retirement");
        let runtime = Option::as_mut(&mut *held).ok_or(ENODEV)?;
        if !*initialized {
            // Experimental native profiles retain their exclusive hardware
            // setup. Ordinary jobs never take an aggregate reservation.
            if runtime.active != 0 && runtime.exclusive { return Ok(None); }
            Self::register(runtime,prepared,sync)?;
            runtime.active += 1;
            runtime.exclusive = false;
            *initialized = true;
            // Metadata lives in immutable snapshots. Per-command activation
            // clones only when the installed engine root actually changes.
        }
        let poll_started = profile_started.as_ref().map(|_| kernel::time::Instant::<kernel::time::Monotonic>::now());
        let (poll_result, mut completions) = runtime.session.poll_work_deferred(dev.as_ref(), &runtime.image);
        self.control_waiting |= runtime.session.backing_waiting();
        if let Err(error) = poll_result {
            runtime.session.notify_failure(error);
            self.fail_unstaged(error);
        }
        // An independent worker can publish and retire while this worker is
        // mapping caller BOs or making timestamp writes visible to the CPU.
        // Owned roots/BOs/alias fences survive both unlock and logical unbind.
        drop(interval);
        drop(held);
        let mut cache_error = None;
        for completion in &mut completions {
            if let Err(error) = completion.cache() { cache_error.get_or_insert(error); }
        }
        let lock_started = RuntimeInterval::waiting();
        let mut held = dev.runtime.lock();
        let _interval = RuntimeInterval::acquired(lock_started, prepared.queue_key, "publication");
        let runtime = Option::as_mut(&mut *held).ok_or(ENODEV)?;
        // Release only this worker's private preparation claim under the
        // publication lock; selection and full plan validation follow here.
        if let Some((_, plan)) = &mut prepared_append { plan.release_reservation(); }
        if let Some(error) = cache_error {
            runtime.session.fail_cpu_completion(error);
            self.fail_unstaged(error);
        }
        for completion in completions { completion.signal(); }
        let poll_us = poll_started.map(|s| s.elapsed().as_nanos()/1000).unwrap_or(0);
        for index in 0..self.nodes.len() {
            let node = &mut self.nodes[index];
            node.point.refresh();
            if node.staged || status(&node.point.completion) != 0 { continue; }
            let result = (|| -> Result<Option<Receipt>> {
                let render = matches!(prepared.parameters[index],Command::Render(_));
                let Some((previous,dependencies)) = Self::dependencies(node,&prepared.parameters[index])? else { return Ok(None); };
                if render {
                    let Command::Render(mut parameters) = prepared.parameters[index] else { unreachable!() };
                    parameters.queue_owner=Some(prepared.queue_key);
                    parameters.prior_queue_ta = previous;
                    parameters.vdm_dependency = dependencies[0];
                    parameters.cdm_dependency = dependencies[1];
                    let snapshot = prepared.render_snapshot.as_ref().ok_or(EIO)?;
                    if !Self::preferred(runtime,0,node) { return Ok(None); }
                    if !runtime.session.can_stage_render(Some(snapshot.client()),&parameters)? {
                        let replacing = !snapshot.same_client(runtime.session.render_client()?);
                        Self::handoff(runtime,0,node,replacing);
                        return Ok(None);
                    }
                    if runtime.session.render_scratch_seed(&parameters)?.is_some() {
                        self.control_waiting = true; return Ok(None);
                    }
                    if runtime.session.render_template_size(snapshot.client(), &parameters)?.is_some() {
                        self.control_waiting = true; return Ok(None);
                    }
                    // Every ordinary append has an exact off-lock descriptor
                    // plan. Another worker may change its next ordinal/pool;
                    // retry this node rather than building bulk objects here.
                    if runtime.session.render_client()?.is_some() {
                        let valid = if let Some((i, plan)) = &prepared_append {
                            *i == index && runtime.session.render_prepared_matches(plan, snapshot.client(), &parameters)?
                        } else { false };
                        if !valid { self.control_waiting = true; return Ok(None); }
                    }
                    // A matching inactive pool still needs the incoming caller
                    // identity when another VM is currently selected. Omitting
                    // it would route the command back to the current caller.
                    let (ready, mut client) = Self::replacement(snapshot,runtime.session.render_client()?, &mut prepared.render_client)
                        .inspect_err(|e| kernel::dev_err!(dev.as_ref(),"G17P: render fail site replacement {:?}\n",e))?;
                    if !ready { self.control_waiting = true; return Ok(None); }
                    let objects = if prepared_append.as_ref().is_some_and(|(i,_)| *i==index) {
                        prepared_append.take().map(|(_,objects)| objects)
                    } else { None };
                    if *crate::module_parameters::firmware_dependencies.value() == 1 {
                        for point in [previous, dependencies[0], dependencies[1]].into_iter().flatten() {
                            runtime.session.retain_dependency(point, &node.point.completion)?;
                        }
                    }
                    Ok(runtime.session.stage_render_ticket(dev.as_ref(),&runtime.image,&mut client,&parameters,objects)?
                        .map(Receipt::Render))
                } else {
                    let Command::Compute(parameters) = &prepared.parameters[index] else { unreachable!() };
                    let snapshot = prepared.compute_snapshot.as_ref().ok_or(EIO)?;
                    if runtime.session.independent_compute_enabled() {
                        let key=prepared.queue_key; let priority=prepared.queue_priority;
                        let Some(grid) = runtime.session.selected_compute_grid(key,snapshot.client(),parameters,priority) else { return Ok(None); };
                        // Only a predecessor on this same physical queue is
                        // guaranteed to execute before its firmware wait. A
                        // public queue may cross owners at rollover/rebinding:
                        // publishing its new head with a wait on the old owner
                        // can occupy CS before the old owner's final dispatch.
                        // Await only those genuine dependencies; independent
                        // nodes/owners continue through the normal scan.
                        if *crate::module_parameters::firmware_dependencies.value() != 1 && previous.is_some_and(|p| p.0 != grid) {
                            if let Some(point) = &node.previous {
                                if Point::ordered(point,true)?.is_none() { return Ok(None); }
                            }
                        }
                        if *crate::module_parameters::firmware_dependencies.value() != 1 && dependencies[1].is_some_and(|p| p.0 != grid) {
                            if let Some(point) = &node.dependencies[1] {
                                if point.dependency(1,true,true)?.is_none() { return Ok(None); }
                            }
                        }
                        // Retirement can satisfy an earlier observed point.
                        // Re-read dependencies before encoding any GPU waits;
                        // never carry a retired owner's stale milestone forward.
                        let Some((previous,dependencies)) = Self::dependencies(node,&prepared.parameters[index])? else { return Ok(None); };
                        let mut waits = KVec::with_capacity(3,GFP_KERNEL)?;
                        for point in [previous,dependencies[0],dependencies[1]].into_iter().flatten() {
                            if !waits.contains(&point) { waits.push(point,GFP_KERNEL)?; }
                        }
                        let (ready,control_waiting) = runtime.session.can_stage_independent_compute(&runtime.image,key,snapshot.client(),parameters,priority)
                            .inspect_err(|e| kernel::dev_err!(dev.as_ref(),"G17P: compute admission key {:?} failed {:?}\n",key,e))?;
                        if !ready {
                            self.control_waiting |= control_waiting;
                            return Ok(None);
                        }
                        if runtime.session.compute_needs_private(key, snapshot.client(), parameters, priority)
                            != prepared.compute_storage.is_some() {
                            // Re-enter off-lock preparation to allocate or
                            // discard speculative backing. Never retain an
                            // unused private heap in the published session.
                            self.control_waiting = true; return Ok(None);
                        }
                        let valid = if let Some((i, objects)) = &prepared_compute {
                            *i == index && runtime.session.compute_prepared_matches(objects, key,
                                snapshot.client(), parameters, priority, &waits)?
                        } else { false };
                        if !valid { self.control_waiting = true; return Ok(None); }
                        let objects = prepared_compute.take().ok_or(EIO)?.1;
                        let old=runtime.session.independent_compute_client(key,snapshot.client(),parameters,priority);
                        let (ready, client)=Self::replacement(snapshot,old, &mut prepared.compute_client)?;
                        if !ready { self.control_waiting = true; return Ok(None); }
                        if *crate::module_parameters::firmware_dependencies.value() == 1 {
                            for point in &waits { runtime.session.retain_dependency(*point, &node.point.completion)?; }
                        }
                        return runtime.session.stage_independent_compute(dev.as_ref(),&runtime.image,
                            key,snapshot.client(),client,prepared.compute_storage.take(),objects,parameters,priority,&waits)
                            .map(|r|Some(Receipt::Compute(r)));
                    }
                    if !Self::preferred(runtime,1,node) { return Ok(None); }
                    if !runtime.session.can_stage_compute(Some(snapshot.client()),parameters)? {
                        let replacing = !snapshot.same_client(runtime.session.compute_client()?);
                        Self::handoff(runtime,1,node,replacing);
                        return Ok(None);
                    }
                    let mut waits = KVec::with_capacity(3,GFP_KERNEL)?;
                    for point in [previous,dependencies[0],dependencies[1]].into_iter().flatten() {
                        if !waits.contains(&point) { waits.push(point,GFP_KERNEL)?; }
                    }
                    let (ready, mut client) = Self::replacement(snapshot,runtime.session.compute_client()?, &mut prepared.compute_client)?;
                    if !ready { self.control_waiting = true; return Ok(None); }
                    Ok(runtime.session.stage_compute_ticket(dev.as_ref(),&runtime.image,&mut client,parameters,&waits)?
                        .map(Receipt::Compute))
                }
            })();
            match result {
                Ok(Some(receipt)) => {
                    if let (Some(start), Receipt::Render(render)) = (profile_started.as_ref(), &receipt) {
                        if render.ordinal % 16 == 0 {
                            kernel::dev_info!(dev.as_ref(),"G17P: render schedule timing ordinal {} seed_us {} poll_us {} total_us {}\n",
                                render.ordinal,seed_us,poll_us,start.elapsed().as_nanos()/1000);
                        }
                    }
                    if let Some(started) = self.submission_started.as_ref() {
                        pr_info!("G17P: SUBMIT_PUBLICATION id {} queue {:?} node {} latency_ns {}\n",
                            self.profile_id, prepared.queue_key, index, started.elapsed().as_nanos());
                    }
                    *node.point.receipt.lock() = Some(receipt);
                    node.staged = true;
                    *cursor += 1;
                    node.point.refresh();
                    dev.events.wake();
                }
                Ok(None) => (),
                Err(error) => {
                    runtime.session.notify_failure(error);
                    node.point.finish(Some(error));
                }
            }
        }
        let mut pending = false;
        let mut unpublished = false;
        let mut error = None;
        for node in &self.nodes {
            node.point.refresh();
            let completed = status(&node.point.completion);
            pending |= completed == 0;
            unpublished |= status(&node.point.publication) == 0;
            if completed < 0 && error.is_none() { error = Some(Error::from_errno(completed)); }
        }
        if !unpublished { publication.signal(); }
        if pending { return Ok(None); }
        let result = runtime.session.finish_owned_submission(error.map_or(Ok(()),Err));
        runtime.session.remember_owned_completed(sync.fence(),match &result {
            Ok(error) => error.is_some(), Err(_) => true,
        });
        runtime.active -= 1;
        result.map(Some)
    }
}
