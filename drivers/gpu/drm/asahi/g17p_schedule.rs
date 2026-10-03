// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Per-command publication and completion. Queue history names accepted work,
//! including work blocked on an external fence. Dependencies never reserve an
//! engine or prevent unrelated commands in the same buffer from being scanned.

use super::{admission, Command, Device, Prepared, Runtime};
use super::super::{g17p_boot::{ComputeReceipt, RenderReceipt}, g17p_sync};
use kernel::{bindings, dma_fence::{Fence, RawDmaFence}, new_mutex, prelude::*, sync::{Arc, Mutex}};

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
    fn new(previous: Option<Arc<Point>>) -> Result<Arc<Self>> {
        Arc::pin_init(try_pin_init!(Self {
            previous<-new_mutex!(previous), publication:g17p_sync::work_fence()?, completion:g17p_sync::work_fence()?,
            receipt<-new_mutex!(None),
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
            if completed != 0 {
                self.finish((completed < 0).then(|| Error::from_errno(completed)));
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
    fn ordered(point: &Arc<Self>) -> Result<Option<Option<(u8,u32)>>> {
        let mut current = point.clone();
        loop {
            current.refresh();
            if status(&current.completion) >= 0 {
                return current.dependency(0,false,false);
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
}
impl Schedule {
    pub(super) fn enabled() -> bool {
        *crate::module_parameters::native_render_vms.value() == 0
            && *crate::module_parameters::native_compute_vms.value() == 0
            && *crate::module_parameters::native_barriers.value() == 0
    }
    pub(super) fn new(commands: &[Command], barriers: &[[u16;2]], seed: History) -> Result<Self> {
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
            let point = Point::new(tail[engine].clone())?;
            nodes.push(Node {point:point.clone(),previous:tail[engine].clone(),dependencies,staged:false},GFP_KERNEL)?;
            tail[engine] = Some(point);
        }
        Ok(Self {nodes,tail})
    }
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
                   old: Option<&super::super::g17p_compute_runtime::Client>, render: bool)
                   -> Result<Option<super::super::g17p_compute_runtime::Client>> {
        if snapshot.same_client(old) { return Ok(None); }
        let mut client = snapshot.deferred_client()?;
        snapshot.materialize(&mut client,render)?;
        Ok(Some(client))
    }
    fn dependencies(node: &Node, command: &Command) -> Result<Option<(Option<(u8,u32)>,[Option<(u8,u32)>;2])>> {
        let previous = if let Some(point) = &node.previous {
            let Some(value) = Point::ordered(point)? else { return Ok(None); };
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
                let Some(value) = point.dependency(1,true,!(early_fragment || queued_compute))? else { return Ok(None); };
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
            parameters.prior_queue_ta=previous;
            parameters.vdm_dependency=dependencies[0];
            parameters.cdm_dependency=dependencies[1];
            let seed = {
                let held = dev.runtime.lock();
                let runtime = Option::as_ref(&*held)?;
                let snapshot = prepared.render_snapshot.as_ref()?;
                if !snapshot.same_client(runtime.session.render_client().ok()?) { return None; }
                if !runtime.session.can_stage_render(Some(snapshot.client()),&parameters).ok()? { continue; }
                runtime.session.render_preparation_seed(&parameters,1).ok()??
            };
            // CPU allocation/serialization holds neither the device mutex nor
            // a hardware reservation. Commit revalidates the full seed and
            // rebuilds it if another job changed the next item in the meantime.
            return super::super::g17p_render_runtime::PreparationSeed::prepare(seed)
                .ok().map(|objects| (index,objects));
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
    pub(super) fn progress(&mut self, dev: &Device, prepared: &mut Prepared,
        sync: &g17p_sync::Plan, initialized: &mut bool, cursor: &mut usize,
        publication: &Fence) -> Result<Option<Option<Error>>> {
        let mut prepared_append = if *initialized { self.prepare_render(dev,prepared) } else { None };
        let mut held = dev.runtime.lock();
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
            prepared.render_client = None;
            prepared.compute_client = None;
        }
        if let Err(error) = runtime.session.poll_work(dev.as_ref(),&runtime.image) {
            runtime.session.notify_failure(error);
            self.fail_unstaged(error);
        }
        for index in 0..self.nodes.len() {
            let node = &mut self.nodes[index];
            node.point.refresh();
            if node.staged || status(&node.point.completion) != 0 { continue; }
            let result = (|| -> Result<Option<Receipt>> {
                let render = matches!(prepared.parameters[index],Command::Render(_));
                let Some((previous,dependencies)) = Self::dependencies(node,&prepared.parameters[index])? else { return Ok(None); };
                if render {
                    let Command::Render(mut parameters) = prepared.parameters[index] else { unreachable!() };
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
                    let mut client = Self::replacement(snapshot,runtime.session.render_client()?,true)?;
                    let objects = if prepared_append.as_ref().is_some_and(|(i,_)| *i==index) {
                        prepared_append.take().map(|(_,objects)| objects)
                    } else { None };
                    Ok(runtime.session.stage_render_ticket(dev.as_ref(),&runtime.image,&mut client,&parameters,objects)?
                        .map(Receipt::Render))
                } else {
                    let Command::Compute(parameters) = &prepared.parameters[index] else { unreachable!() };
                    let snapshot = prepared.compute_snapshot.as_ref().ok_or(EIO)?;
                    if runtime.session.independent_compute_enabled() {
                        let mut waits = KVec::with_capacity(3,GFP_KERNEL)?;
                        for point in [previous,dependencies[0],dependencies[1]].into_iter().flatten() {
                            if !waits.contains(&point) { waits.push(point,GFP_KERNEL)?; }
                        }
                        let key=prepared.queue_key; let priority=prepared.queue_priority;
                        if !runtime.session.can_stage_independent_compute(&runtime.image,key,snapshot.client(),parameters,priority)? {
                            return Ok(None);
                        }
                        let old=runtime.session.independent_compute_client(key,snapshot.client(),parameters,priority);
                        let client=Self::replacement(snapshot,old,false)?;
                        return runtime.session.stage_independent_compute(dev.as_ref(),&runtime.image,
                            key,snapshot.client(),client,parameters,priority,&waits).map(|r|Some(Receipt::Compute(r)));
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
                    let mut client = Self::replacement(snapshot,runtime.session.compute_client()?,false)?;
                    Ok(runtime.session.stage_compute_ticket(dev.as_ref(),&runtime.image,&mut client,parameters,&waits)?
                        .map(Receipt::Compute))
                }
            })();
            match result {
                Ok(Some(receipt)) => {
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
