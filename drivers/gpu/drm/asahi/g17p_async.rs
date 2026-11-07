// SPDX-License-Identifier: GPL-2.0-only

//! Firmware callbacks wake owned jobs; each pass publishes or retires without
//! waiting for GPU completion. A delayed pass covers coalesced notifications
//! and watchdog deadlines. Pending input fences have no execution deadline.

use super::{Device, File, FileState, Prepared};
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
};
use kernel::{
    bindings,
    dma_fence::{self, RawDmaFence},
    impl_has_hr_timer, new_mutex,
    prelude::*,
    sync::{aref::ARef, Arc, ArcBorrow, Mutex},
    time::{Delta, Monotonic, hrtimer::{ArcHrTimerHandle, HrTimer, HrTimerCallback,
        HrTimerCallbackContext, HrTimerPointer, HrTimerRestart, RelativeMode}},
    workqueue::{
        self, impl_has_delayed_work, impl_has_work, new_delayed_work, new_work, DelayedWork, Work,
        WorkItem,
    },
};

/// Private CPU work queue for the device. Its already allocated cleanup work is
/// handed to a different queue on last-owner release: never flush/destroy a
/// queue from its own worker, including device teardown after final job drop.
#[pin_data(PinnedDrop)]
struct JobQueue {
    #[pin]
    release: Work<JobQueue>,
    queue: *mut bindings::workqueue_struct,
}
// SAFETY: C workqueues support concurrent enqueue; the pointer is initialized
// before publication and is released only after the last Events owner drops.
unsafe impl Send for JobQueue {}
unsafe impl Sync for JobQueue {}
impl JobQueue {
    fn new() -> Result<Pin<KBox<Self>>> {
        let mut owner=KBox::pin_init(pin_init!(Self {
            release<-new_work!("agx_jobs_release"),queue:core::ptr::null_mut(),
        }),GFP_KERNEL)?;
        // SAFETY: Static format string has no substitutions. The device queue
        // retains ordinary per-job concurrency and workqueue affinity controls.
        let queue=unsafe {bindings::alloc_workqueue_noprof(
            kernel::c_str!("agx_jobs").as_char_ptr(),
            bindings::wq_flags_WQ_UNBOUND|bindings::wq_flags_WQ_SYSFS,0)};
        if queue.is_null() {return Err(ENOMEM);}
        // SAFETY: Only the unpinned pointer field changes, before publication.
        unsafe {owner.as_mut().get_unchecked_mut().queue=queue;}
        Ok(owner)
    }
}
#[pinned_drop]
impl PinnedDrop for JobQueue {
    fn drop(self: Pin<&mut Self>) {
        if !self.queue.is_null() {
            // SAFETY: Owns this allocation; initialization failures run on
            // the probe thread, normal release on the separate cleanup queue.
            unsafe {bindings::destroy_workqueue(self.queue);}
        }
    }
}
impl_has_work! {impl HasWork<Self> for JobQueue {self.release}}
impl WorkItem for JobQueue {
    type Pointer=Pin<KBox<Self>>;
    fn run(this: Self::Pointer) {drop(this);}
}

/// Concentrate the serialized firmware state and its short CPU preparation
/// bursts on one fast CPU, so cache lines and scheduler utilization stay local.
/// Other device workqueues, CPU online masks and frequency policy are untouched.
/// Prefer the first fastest frequency domain, then a later CPU within it to
/// keep its boot/interrupt CPU available and avoid crossing shared caches.
fn local_job_cpu(cpus:impl Iterator<Item=(u32,u32,u32)>)->Option<u32> {
    cpus.filter(|(_,maximum,_)|*maximum!=0)
        .max_by_key(|(cpu,maximum,domain)|(*maximum,core::cmp::Reverse(*domain),*cpu))
        .map(|(cpu,_,_)|cpu)
}

/// Queue heads are maintained at admission/retirement, never rediscovered
/// quadratically in IRQ context. Membership owns each Job through enqueue.
struct JobSet {
    jobs: KVec<Arc<Job>>,
    heads: KVec<Arc<Job>>,
    next: usize,
    preferred: [Option<(u64, u32)>; 2],
}
impl JobSet {
    fn new() -> Self { Self { jobs: KVec::new(), heads: KVec::new(), next: 0, preferred: [None; 2] } }
    fn poll_head(&self) -> Option<&Arc<Job>> {
        let ready=|h: &&Arc<Job>|h.firmware_waiting.load(Ordering::SeqCst);
        self.preferred.iter().flatten().find_map(|key|
            self.heads.iter().filter(ready).find(|h|h.queue_key==*key))
            .or_else(||self.heads.iter().find(ready))
    }
}

#[pin_data(PinnedDrop)]
pub(crate) struct Events {
    queue_owner: Option<Pin<KBox<JobQueue>>>,
    error: AtomicI32,
    wake_sequence: AtomicU64,
    // CPU fence/promotion wakes participate in the lost-wake handshake,
    // but only firmware notifications invalidate the shared backend poll.
    firmware_sequence: AtomicU64,
    queue_configured: AtomicBool,
    #[pin]
    queue_configuration: Mutex<()>,
    #[pin]
    jobs: kernel::sync::lock::Lock<JobSet, IrqLock>,
}
impl Events {
    pub(crate) fn new() -> Result<Arc<Self>> {
        let queue_owner=Some(JobQueue::new()?);
        Arc::pin_init(
            pin_init!(Self {queue_owner,queue_configured:AtomicBool::new(false),queue_configuration<-new_mutex!(()),error:AtomicI32::new(0),wake_sequence:AtomicU64::new(0),firmware_sequence:AtomicU64::new(0),jobs<-kernel::sync::lock::Lock::new(JobSet::new(),kernel::c_str!("neo_events"),kernel::static_lock_class!())}),
            GFP_KERNEL,
        )
    }
    /// Called in submission context, after CPU-frequency drivers have had
    /// an opportunity to probe. Configuration is performed once, before any
    /// accepted job can enter this workqueue; later operator changes persist.
    fn configure_queue(&self)->Result {
        if self.queue_configured.load(Ordering::Acquire) {return Ok(());}
        let _configuration=self.queue_configuration.lock();
        if self.queue_configured.load(Ordering::Relaxed) {return Ok(());}
        // SAFETY: CPU hotplug read exclusion keeps the online mask stable.
        // cpufreq returns the rated maximum, not current governor/thermal state.
        #[cfg(CONFIG_CPU_FREQ)]
        let selected=unsafe {
            #[cfg(CONFIG_HOTPLUG_CPU)]
            bindings::cpus_read_lock();
            let online=core::ptr::addr_of!(bindings::__cpu_online_mask);
            let count=bindings::nr_cpu_ids;
            let bits=core::mem::size_of::<kernel::ffi::c_ulong>()*8;
            let cpu=local_job_cpu((0..count).filter(|cpu| {
                (*online).bits.get(*cpu as usize/bits)
                    .is_some_and(|word|*word&(1<<(*cpu as usize%bits))!=0)
            }).map(|cpu| {
                let policy=bindings::cpufreq_cpu_get(cpu);
                if policy.is_null() {return (cpu,0,cpu);}
                // A policy reference keeps its immutable rated maximum live;
                // CPU hotplug exclusion also stabilizes its representative.
                let row=(cpu,(*policy).cpuinfo.max_freq,(*policy).cpu);
                bindings::cpufreq_cpu_put(policy);
                row
            }));
            #[cfg(CONFIG_HOTPLUG_CPU)]
            bindings::cpus_read_unlock();
            cpu
        };
        #[cfg(not(CONFIG_CPU_FREQ))]
        let selected=None::<u32>;
        let Some(cpu)=selected else {
            // No frequency information: preserve the kernel's default pool
            // placement rather than guessing the platform's CPU topology.
            self.queue_configured.store(true,Ordering::Release);return Ok(());
        };
        // SAFETY: These kernel routines allocate/free an owned attributes
        // object. apply_workqueue_attrs copies it and handles CPU hotplug.
        let attrs=unsafe {bindings::alloc_workqueue_attrs_noprof()};
        if attrs.is_null() {return Err(ENOMEM);}
        let result=unsafe {
            #[cfg(CONFIG_CPUMASK_OFFSTACK)]
            let mask=(*attrs).cpumask;
            #[cfg(not(CONFIG_CPUMASK_OFFSTACK))]
            let mask=(*attrs).cpumask.as_mut_ptr();
            for word in &mut (*mask).bits {*word=0;}
            let bits=core::mem::size_of::<kernel::ffi::c_ulong>()*8;
            (*mask).bits[cpu as usize/bits]=1<<(cpu as usize%bits);
            let result=bindings::apply_workqueue_attrs(
                self.queue_owner.as_ref().unwrap().queue,attrs);
            bindings::free_workqueue_attrs(attrs);
            result
        };
        if result<0 {return Err(Error::from_errno(result));}
        self.queue_configured.store(true,Ordering::Release);
        Ok(())
    }
    pub(crate) fn healthy(&self) -> Result {
        let error = self.error.load(Ordering::Acquire);
        if error < 0 {
            Err(Error::from_errno(error))
        } else {
            Ok(())
        }
    }
    pub(crate) fn fail(&self, error: Error) {
        self.error.store(error.to_errno(), Ordering::Release);
        self.wake_all();
    }
    fn add(&self, job: Arc<Job>) -> Result {
        let mut held = self.jobs.lock();
        let head = !held.heads.iter().any(|h| h.queue_key == job.queue_key);
        // Reserve both collections before publishing either membership.
        held.jobs.reserve(1, GFP_ATOMIC)?;
        if head { held.heads.reserve(1, GFP_ATOMIC)?; }
        held.jobs.push(job.clone(), GFP_ATOMIC)?;
        if head { held.heads.push(job, GFP_ATOMIC)?; }
        Ok(())
    }
    fn remove(&self, job: &Job) {
        let mut held = self.jobs.lock();
        if let Some(i) = held.jobs.iter().position(|h| core::ptr::eq(&**h, job)) {
            let _ = held.jobs.remove(i);
            if let Some(head) = held.heads.iter().position(|h| core::ptr::eq(&**h, job)) {
                let mut promoted=None;
                if let Some(next) = held.jobs.iter().find(|h| h.queue_key == job.queue_key).cloned() {
                    held.heads[head] = next.clone();
                    // A successor may already have run without arming a
                    // timer. Promotion and is_head use this same IRQ lock.
                    promoted=Some(next.clone());
                    Job::request(next);
                } else {
                    let _ = held.heads.remove(head);
                    for preferred in &mut held.preferred {
                        if *preferred == Some(job.queue_key) { *preferred = None; }
                    }
                }
                // Poll ownership can move to a DIFFERENT public queue. Its
                // head may have waited without arming a fallback timer. The
                // retiring worker withdrew firmware_waiting before progress,
                // so always re-elect here rather than testing its old flag.
                if let Some(poller)=held.poll_head() {
                    if promoted.as_ref().is_none_or(|next|!core::ptr::eq(&**next,&**poller)) {
                        Job::request(poller.clone());
                    }
                }
            }
        }
    }
    fn is_head(&self, job: &Job) -> bool {
        self.jobs.lock().heads.iter().any(|h| core::ptr::eq(&**h, job))
    }
    pub(super) fn prefer_queue(&self, engine: usize, key: (u64, u32)) {
        self.jobs.lock().preferred[engine] = Some(key);
    }
    pub(super) fn release_queue(&self, engine: usize, key: (u64, u32)) {
        let mut held = self.jobs.lock();
        if held.preferred[engine] == Some(key) {
            held.preferred[engine] = None;
            if let Some(poller)=held.poll_head() {
                Job::request(poller.clone());
            }
        }
    }
    // Every scheduled progress pass polls the complete session. One waiting
    // queue head covers coalesced notifications and watchdogs for all owners.
    // Removal/promotion wakes a successor so another waiter can take over.
    fn is_poll_head(&self, job: &Job) -> bool {
        let held = self.jobs.lock();
        held.poll_head().is_some_and(|h|core::ptr::eq(&**h,job))
    }
    pub(super) fn sequence(&self) -> u64 { self.firmware_sequence.load(Ordering::SeqCst) }
    pub(crate) fn firmware_wake(&self) {
        self.firmware_sequence.fetch_add(1, Ordering::SeqCst);
        self.wake();
    }
    pub(crate) fn wake(&self) {
        self.wake_backend(false);
    }
    fn wake_all(&self) {
        self.wake_backend(true);
    }
    fn wake_backend(&self, all: bool) {
        // Callback context never takes a sleeping lock or touches caller RAM.
        // Firmware events cannot satisfy external input fences. Keep blocked
        // jobs dormant; a fatal error must nevertheless reach every owner.
        // With the worker's waiting publication/sequence recheck, a wake
        // racing backend progress is either enqueued here or noticed by that
        // pass. SC orders this handshake across both atomic locations.
        self.wake_sequence.fetch_add(1, Ordering::SeqCst);
        let mut held = self.jobs.lock();
        if all {
            for job in &held.jobs {
                Job::request(job.clone());
            }
        } else {
            let count = held.heads.len();
            if count != 0 {
                let first = held.next % count;
                // A full-session poll services all receipts. Wake the
                // selected admission owners and one rotating head instead of
                // making every client compete for the same runtime mutex.
                // The rotating head guarantees progress for other engines
                // and for ready work which has not claimed admission yet.
                let mut selected = [None; 2];
                for (engine, key) in held.preferred.iter().enumerate() {
                    if let Some(key) = key {
                        if let Some(index) = held.heads.iter().position(|h|
                            h.queue_key == *key && h.firmware_waiting.load(Ordering::SeqCst)) {
                            if !selected.contains(&Some(index)) {
                                Job::request(held.heads[index].clone());
                            }
                            selected[engine] = Some(index);
                        }
                    }
                }
                for offset in 0..count {
                    let index = (first + offset) % count;
                    let job = &held.heads[index];
                    if job.firmware_waiting.load(Ordering::SeqCst) {
                        if !selected.contains(&Some(index)) {
                            Job::request(job.clone());
                        }
                        held.next = (index + 1) % count;
                        break;
                    }
                }
            }
        }
    }

    fn wake_job(&self, identity: usize) {
        // The identity is compared only, never dereferenced. Membership owns
        // a live Arc through enqueue and retirement removes it before Drop.
        // A callback cannot keep its own Job alive in a reference cycle.
        let jobs = self.jobs.lock();
        if let Some(job) = jobs.jobs
            .iter()
            .find(|job| &***job as *const Job as usize == identity)
        {
            Job::request(job.clone());
        }
    }
}

#[pinned_drop]
impl PinnedDrop for Events {
    fn drop(self:Pin<&mut Self>) {
        // SAFETY: Move only the unpinned handle, never a pinned field.
        let owner=unsafe {self.get_unchecked_mut()}.queue_owner.take();
        if let Some(owner)=owner {let _=workqueue::system_long().enqueue::<_,0>(owner);}
    }
}

#[pin_data]
pub(crate) struct Job {
    #[pin]
    work: Work<Job>,
    #[pin]
    timer: DelayedWork<Job, 1>,
    #[pin]
    execution: Mutex<Option<Execution>>,
    #[pin]
    control_timer: HrTimer<Job>,
    committed: AtomicBool,
    firmware_waiting: AtomicBool,
    // Immutable admission identity, readable in IRQ context without taking
    // the sleeping execution lock. Queue IDs are scoped by their DRM file.
    queue_key: super::super::g17p_compute_queues::Key,
    events: Arc<Events>,
}
struct Execution {
    file: Arc<FileState>,
    prepared: Prepared,
    sync: super::super::g17p_sync::Plan,
    previous: Option<dma_fence::Fence>,
    publication: dma_fence::Fence,
    render_receipts: KVec<Arc<super::super::g17p_boot::RenderReceipt>>,
    schedule: Option<super::scheduling::Schedule>,
    initialized: bool,
    cursor: usize,
    waiting: bool,
    work_gate: Option<dma_fence::Fence>,
    callbacks: KVec<KBox<FenceWake>>,
    // Stable callback storage is allocated before ioctl commitment. Binding
    // a newly staged receipt requires no allocation in the execution path.
    receipt_callbacks: KVec<[KBox<FenceWake>; 2]>,
    control_timer_handle: Option<ArcHrTimerHandle<Job>>,
    // Fields drop in declaration order. File/Prepared/receipts retain GEM
    // objects whose DMA cleanup needs this device, even after driver unbind.
    dev: ARef<Device>,
}
impl Execution {
    fn progress(&mut self) -> Result<Option<Option<Error>>> {
        if let Some(schedule) = &mut self.schedule {
            schedule.progress(&self.dev,&mut self.prepared,&self.sync,
                &mut self.initialized,&mut self.cursor,&self.publication)
        } else {
            File::progress(&self.dev,&mut self.prepared,&self.sync,&mut self.initialized,
                &mut self.cursor,&mut self.waiting,&mut self.work_gate,
                &mut self.render_receipts,&self.publication)
        }
    }
}
impl_has_hr_timer! {
    impl HasHrTimer<Self> for Job { mode: RelativeMode<Monotonic>, field: self.control_timer }
}
impl HrTimerCallback for Job {
    type Pointer<'a> = Arc<Self>;
    fn run(this: ArcBorrow<'_, Self>, _ctx: HrTimerCallbackContext<'_, Self>) -> HrTimerRestart {
        // IRQ context only schedules this owner. All receipt reads and mapping
        // edits remain in the existing sleeping worker.
        Job::request(this.into());
        HrTimerRestart::NoRestart
    }
}
impl_has_work! {impl HasWork<Self> for Job {self.work}}
impl_has_delayed_work! {impl HasDelayedWork<Self,1> for Job {self.timer}}
impl Job {
    pub(super) fn new(
        dev: ARef<Device>,
        file: Arc<FileState>,
        prepared: Prepared,
        sync: super::super::g17p_sync::Plan,
        previous: Option<dma_fence::Fence>,
        schedule: Option<super::scheduling::Schedule>,
    ) -> Result<Arc<Self>> {
        // Allocate all internal publication/receipt ownership before ioctl
        // commitment. None of these fences changes userspace completion.
        let publication = super::super::g17p_sync::work_fence()?;
        let mut render_receipts = KVec::new();
        render_receipts.reserve(prepared.parameters.len(), GFP_KERNEL)?;
        let events = dev.events.clone();
        events.configure_queue()?;
        let queue_key = prepared.queue_key;
        let job = Arc::pin_init(
            try_pin_init!(Self {
                work<-new_work!("asahi_neo_event"),
                timer<-new_delayed_work!("asahi_neo_watchdog"),
                execution<-new_mutex!(Some(Execution {dev,file,prepared,sync,previous,publication,render_receipts,schedule,control_timer_handle:None,receipt_callbacks:KVec::new(),
                    initialized:false,cursor:0,waiting:false,work_gate:None,callbacks:KVec::new()})),
                control_timer<-HrTimer::new(),
                committed:AtomicBool::new(false),firmware_waiting:AtomicBool::new(false),queue_key,events,
            }),
            GFP_KERNEL,
        )?;
        // All fallible admission precedes queue-tail/output publication.
        let identity = &*job as *const Job as usize;
        {
            let mut held = job.execution.lock();
            let e = Option::as_mut(&mut *held).unwrap();
            for fence in e.sync.inputs() {
                e.callbacks.push(
                    FenceWake::new(fence.clone(), job.events.clone(), identity)?,
                    GFP_KERNEL,
                )?;
            }
            if let Some(schedule) = &e.schedule {
                e.receipt_callbacks.reserve(e.prepared.parameters.len(), GFP_KERNEL)?;
                for _ in &e.prepared.parameters {
                    e.receipt_callbacks.push([
                        FenceWake::unbound(job.events.clone(), identity)?,
                        FenceWake::unbound(job.events.clone(), identity)?,
                    ], GFP_KERNEL)?;
                }
                for fence in schedule.callback_fences()? {
                    e.callbacks.push(FenceWake::new(fence,job.events.clone(),identity)?,GFP_KERNEL)?;
                }
            }
            if let Some(previous) = &e.previous {
                e.callbacks.push(
                    FenceWake::new(previous.clone(), job.events.clone(), identity)?,
                    GFP_KERNEL,
                )?;
            }
        }
        job.events.add(job.clone())?;
        // The caller still holds FileState.state, so the exact timestamp
        // tokens validated at admission cannot be unbound concurrently. Do
        // this after all other fallible allocations: an ioctl which fails
        // admission must not leave an unsignaled alias lease behind.
        let admission = (|| -> Result {
            let held = job.execution.lock();
            let e = Option::as_ref(&*held).ok_or(EIO)?;
            let addresses = e.prepared.timestamp_addresses()?;
            let mut runtime = e.dev.runtime.lock();
            Option::as_mut(&mut *runtime).ok_or(ENODEV)?
                .session.admit_timestamps(&e.sync.fence(), &addresses)
        })();
        if let Err(error) = admission {
            job.events.remove(&job);
            return Err(error);
        }
        Ok(job)
    }
    pub(super) fn publication_fence(&self) -> dma_fence::Fence {
        self.execution.lock().as_ref().unwrap().publication.clone()
    }
    pub(super) fn enqueue(job: Arc<Self>) {
        {
            let mut held = job.execution.lock();
            let execution = Option::as_mut(&mut *held).unwrap();
            // File::submit still holds FileState.state: admission cannot race
            // this VM's destructive mapping ioctl before output publication.
            execution
                .prepared
                .vm_pending
                .fetch_add(1, Ordering::Release);
            execution.file.pending.fetch_add(1, Ordering::Release);
            execution.sync.publish_outputs();
            job.committed.store(true, Ordering::Release);
        }
        Job::request(job);
    }
}
impl Job {
    fn request(this:Arc<Self>) {
        let queue=this.events.queue_owner.as_ref().unwrap().queue;
        // SAFETY: This Job keeps Events and its queue alive through enqueue;
        // a queued/running Job keeps them alive when enqueue coalesces.
        unsafe {let _=workqueue::Queue::from_raw(queue).enqueue::<_,0>(this);}
    }
    fn watchdog(this:Arc<Self>) {
        let queue=this.events.queue_owner.as_ref().unwrap().queue;
        // SAFETY: The delayed work owns this Job, which retains the queue.
        unsafe {let _=workqueue::Queue::from_raw(queue).enqueue_delayed::<_,1>(
            this,kernel::time::msecs_to_jiffies(10));}
    }
    fn advance(this: Arc<Self>) {
        if !this.committed.load(Ordering::Acquire) {
            return;
        }
        let mut held = this.execution.lock();
        let Some(e) = Option::as_mut(&mut *held) else {
            return;
        };
        // Cancel the previous one-shot before considering a new retry. The
        // callback never takes execution, so cancellation cannot invert locks.
        drop(e.control_timer_handle.take());
        this.firmware_waiting.store(false, Ordering::SeqCst);
        let wake_sequence = this.events.wake_sequence.load(Ordering::SeqCst);
        let mut input_blocked = false;
        let result = (|| -> Result<Option<Option<Error>>> {
            if let Err(error) = e.dev.events.healthy() {
                if !e.initialized {
                    return Err(error);
                }
                return e.progress();
            }
            if !e.sync.inputs_ready()? {
                input_blocked = true;
                return Ok(None);
            }
            if let Some(fence) = &e.previous {
                // SAFETY: Execution owns the immutable queue-tail reference.
                if unsafe { bindings::dma_fence_get_status(fence.raw()) } == 0 {
                    input_blocked = true;
                    return Ok(None);
                }
            }
            let result = e.progress()?;
            // A retiring job will remove its membership and signal outputs.
            // Binding already-retired receipt fences only queues this same
            // job again after its Execution has been taken and dropped.
            if result.is_none() {
                if let Some(schedule) = &e.schedule {
                    for (index, callbacks) in e.receipt_callbacks.iter_mut().enumerate() {
                        for (phase, callback) in callbacks.iter_mut().enumerate() {
                            if callback.fence.is_none() {
                                if let Some(fence) = schedule.receipt_fence(index, phase == 0) {
                                    callback.bind(fence)?;
                                }
                            }
                        }
                    }
                }
            }
            Ok(result)
        })();
        match result {
            Ok(None) if input_blocked => {
                // Input/queue callbacks were installed before commitment.
                // A racing signal requeues the running work; no GPU deadline
                // or periodic poll applies to a job waiting on caller fences.
            }
            Ok(None) => {
                this.firmware_waiting.store(true, Ordering::SeqCst);
                // Non-head jobs keep their input/publication callbacks, but
                // must not all contend for runtime on every firmware event.
                // Head promotion and this check share the events lock, so a
                // successor cannot miss both promotion and timer arming.
                if !this.events.is_head(&this) {
                    return;
                }
                let poll_head = this.events.is_poll_head(&this);
                // Receipt callbacks wake their exact owners. Only the elected
                // poller needs to repeat a full-session poll when an IRQ raced
                // this pass; otherwise one event regenerates the wake storm.
                if poll_head && this.events.wake_sequence.load(Ordering::SeqCst) != wake_sequence {
                    Job::request(this.clone());
                }
                // Retained-pool admission can be blocked without a pending
                // control receipt. Poll that case at the same bounded cadence
                // so a missed/coalesced wake never adds a 10 ms client switch.
                // Published work also needs this cadence: its retirement and
                // growth receipts can coalesce without another mailbox IRQ.
                // Input fences stay callback-only in the branch above.
                if poll_head {
                    if let Some(schedule) = e.schedule.as_ref().filter(|schedule| schedule.fast_retry()) {
                        e.control_timer_handle = Some(this.clone().start(Delta::from_micros(schedule.fast_retry_us())));
                    }
                }
                // The high-resolution retry already polls all owners and
                // checks their deadlines. A second delayed work item survives
                // short jobs and wakes again after retirement for no work.
                // Keep the legacy watchdog, and the slow fallback only when
                // this elected head did not arm a high-resolution retry.
                if e.schedule.is_none() || (poll_head && e.control_timer_handle.is_none()) {
                    Self::watchdog(this.clone());
                }
            }
            result => {
                let execution = held.take().unwrap();
                let error = match result {
                    Ok(Some(error)) => error,
                    Err(error) => Some(error),
                    _ => unreachable!(),
                };
                if let (Some(schedule),Some(error)) = (&execution.schedule,error) {
                    schedule.fail_unstaged(error);
                }
                if let Some(error) = error {
                    kernel::dev_err!(
                        execution.dev.as_ref(),
                        "G17P: asynchronous submission {:?} at command {}/{} failed {:?}\n",
                        execution.prepared.owner,
                        execution.cursor,
                        execution.prepared.parameters.len(),
                        error
                    );
                }
                this.events.remove(&this);
                // Taking Execution above makes every success/error retirement
                // unique, including input errors and accepted queue/file close.
                execution
                    .prepared
                    .vm_pending
                    .fetch_sub(1, Ordering::Release);
                execution.file.pending.fetch_sub(1, Ordering::Release);
                // A rejected/unpublished job still releases queue ordering.
                // Its public fence retains the actual completion/error.
                execution.publication.signal();
                execution.sync.complete(error);
                drop(held);
                this.events.wake();
            }
        }
    }
}

impl WorkItem for Job {
    type Pointer = Arc<Self>;
    fn run(this: Arc<Self>) {
        Self::advance(this);
    }
}
impl WorkItem<1> for Job {
    type Pointer = Arc<Self>;
    fn run(this: Arc<Self>) {
        Self::advance(this);
    }
}

// The callback's allocation remains stable from add_callback until Drop.
// remove_callback takes the fence lock and therefore synchronizes with any
// callback currently running under that same lock before freeing the owner.
#[repr(C)]
struct FenceWake {
    callback: UnsafeCell<bindings::dma_fence_cb>,
    events: Arc<Events>,
    identity: usize,
    fence: Option<dma_fence::Fence>,
    registered: bool,
}
// SAFETY: Only the DMA-fence core mutates callback, under its own fence lock.
// events and fence are refcounted; Drop removes under the same lock.
unsafe impl Send for FenceWake {}
unsafe impl Sync for FenceWake {}
impl FenceWake {
    fn unbound(events: Arc<Events>, identity: usize) -> Result<KBox<Self>> {
        Ok(KBox::new(Self {
            // SAFETY: add_callback initializes callback's list linkage.
            callback: UnsafeCell::new(unsafe { core::mem::zeroed() }),
            events, identity, fence: None, registered: false,
        }, GFP_KERNEL)?)
    }
    fn new(fence: dma_fence::Fence, events: Arc<Events>, identity: usize) -> Result<KBox<Self>> {
        let mut owned = Self::unbound(events, identity)?;
        owned.bind(fence)?;
        Ok(owned)
    }
    fn bind(&mut self, fence: dma_fence::Fence) -> Result {
        debug_assert!(self.fence.is_none());
        self.fence = Some(fence);
        // SAFETY: Stable owned allocation and fence survive until removal;
        // callback only accesses immutable events and never a sleeping lock.
        let result = unsafe {
            bindings::dma_fence_add_callback(
                self.fence.as_ref().unwrap().raw(),
                self.callback.get(),
                Some(Self::wake),
            )
        };
        if result == -(bindings::ENOENT as i32) {
            // Retirement raced registration. No callback will run, so the
            // owning job must recheck its receipt explicitly.
            self.events.wake_job(self.identity);
        } else if result != 0 {
            return Err(Error::from_errno(result));
        } else {
            self.registered = true;
        }
        Ok(())
    }
    unsafe extern "C" fn wake(
        _fence: *mut bindings::dma_fence,
        callback: *mut bindings::dma_fence_cb,
    ) {
        // SAFETY: repr(C)'s first field is callback, whose lifetime is kept
        // until remove_callback synchronizes with this invocation.
        let owned = unsafe { &*callback.cast::<Self>() };
        owned.events.wake_job(owned.identity);
    }
}
impl Drop for FenceWake {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }
        // SAFETY: Owned stable callback/fence; removal serializes with wake.
        unsafe {
            bindings::dma_fence_remove_callback(self.fence.as_ref().unwrap().raw(), self.callback.get());
        }
    }
}

struct IrqLock;
// SAFETY: These wrappers preserve the kernel spinlock's mutual exclusion,
// save local interrupt state on acquisition and restore exactly that state.
unsafe impl kernel::sync::lock::Backend for IrqLock {
    type State = bindings::spinlock_t;
    type GuardState = kernel::ffi::c_ulong;
    unsafe fn init(
        ptr: *mut Self::State,
        name: *const kernel::ffi::c_char,
        key: *mut bindings::lock_class_key,
    ) {
        // SAFETY: Backend caller provides initialized storage and static key.
        unsafe {
            bindings::__spin_lock_init(ptr, name, key);
        }
    }
    unsafe fn lock(ptr: *mut Self::State) -> Self::GuardState {
        // SAFETY: Backend caller provides a live initialized spinlock.
        unsafe { bindings::spin_lock_irqsave(ptr) }
    }
    unsafe fn unlock(ptr: *mut Self::State, flags: &Self::GuardState) {
        // SAFETY: Backend caller owns this lock and its saved interrupt state.
        unsafe {
            bindings::spin_unlock_irqrestore(ptr, *flags);
        }
    }
    unsafe fn try_lock(ptr: *mut Self::State) -> Option<Self::GuardState> {
        let mut flags = 0;
        // SAFETY: Initialized live lock; wrapper restores IRQs on failure.
        if unsafe { bindings::spin_trylock_irqsave(ptr, &mut flags) } {
            Some(flags)
        } else {
            None
        }
    }
    unsafe fn assert_is_held(ptr: *mut Self::State) {
        // SAFETY: Initialized live lock as required by the backend contract.
        unsafe {
            bindings::spin_assert_is_held(ptr);
        }
    }
}
