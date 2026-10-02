// SPDX-License-Identifier: GPL-2.0-only OR MIT

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
    new_mutex,
    prelude::*,
    sync::{aref::ARef, Arc, Mutex},
    workqueue::{
        self, impl_has_delayed_work, impl_has_work, new_delayed_work, new_work, DelayedWork, Work,
        WorkItem,
    },
};

#[pin_data]
pub(crate) struct Events {
    error: AtomicI32,
    wake_sequence: AtomicU64,
    #[pin]
    jobs: kernel::sync::lock::Lock<KVec<Arc<Job>>, IrqLock>,
}
impl Events {
    pub(crate) fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {error:AtomicI32::new(0),wake_sequence:AtomicU64::new(0),jobs<-kernel::sync::lock::Lock::new(KVec::new(),kernel::c_str!("neo_events"),kernel::static_lock_class!())}),
            GFP_KERNEL,
        )
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
        self.jobs.lock().push(job, GFP_ATOMIC)?;
        Ok(())
    }
    fn remove(&self, job: &Job) {
        let mut jobs = self.jobs.lock();
        if let Some(i) = jobs.iter().position(|held| core::ptr::eq(&**held, job)) {
            jobs.swap_remove(i);
        }
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
        let jobs = self.jobs.lock();
        for job in jobs.iter() {
            if all || job.firmware_waiting.load(Ordering::SeqCst) {
                let _ = workqueue::system_unbound().enqueue::<_, 0>(job.clone());
            }
        }
    }
    fn wake_job(&self, identity: usize) {
        // The identity is compared only, never dereferenced. Membership owns
        // a live Arc through enqueue and retirement removes it before Drop.
        // A callback cannot keep its own Job alive in a reference cycle.
        let jobs = self.jobs.lock();
        if let Some(job) = jobs
            .iter()
            .find(|job| &***job as *const Job as usize == identity)
        {
            let _ = workqueue::system_unbound().enqueue::<_, 0>(job.clone());
        }
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
    committed: AtomicBool,
    firmware_waiting: AtomicBool,
    events: Arc<Events>,
}
struct Execution {
    dev: ARef<Device>,
    file: Arc<FileState>,
    prepared: Prepared,
    sync: super::super::g17p_sync::Plan,
    previous: Option<dma_fence::Fence>,
    publication: dma_fence::Fence,
    render_receipts: KVec<Arc<super::super::g17p_boot::RenderReceipt>>,
    initialized: bool,
    cursor: usize,
    waiting: bool,
    work_gate: Option<dma_fence::Fence>,
    callbacks: KVec<KBox<FenceWake>>,
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
    ) -> Result<Arc<Self>> {
        // Allocate all internal publication/receipt ownership before ioctl
        // commitment. None of these fences changes userspace completion.
        let publication = super::super::g17p_sync::work_fence()?;
        let mut render_receipts = KVec::new();
        render_receipts.reserve(prepared.parameters.len(), GFP_KERNEL)?;
        let events = dev.events.clone();
        let job = Arc::pin_init(
            try_pin_init!(Self {
                work<-new_work!("asahi_neo_event"),
                timer<-new_delayed_work!("asahi_neo_watchdog"),
                execution<-new_mutex!(Some(Execution {dev,file,prepared,sync,previous,publication,render_receipts,
                    initialized:false,cursor:0,waiting:false,work_gate:None,callbacks:KVec::new()})),
                committed:AtomicBool::new(false),firmware_waiting:AtomicBool::new(false),events,
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
            if let Some(previous) = &e.previous {
                e.callbacks.push(
                    FenceWake::new(previous.clone(), job.events.clone(), identity)?,
                    GFP_KERNEL,
                )?;
            }
        }
        job.events.add(job.clone())?;
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
        let _ = workqueue::system_unbound().enqueue::<_, 0>(job);
    }
}
impl Job {
    fn advance(this: Arc<Self>) {
        if !this.committed.load(Ordering::Acquire) {
            return;
        }
        let mut held = this.execution.lock();
        let Some(e) = Option::as_mut(&mut *held) else {
            return;
        };
        this.firmware_waiting.store(false, Ordering::SeqCst);
        let wake_sequence = this.events.wake_sequence.load(Ordering::SeqCst);
        let mut input_blocked = false;
        let result = (|| -> Result<Option<Option<Error>>> {
            if let Err(error) = e.dev.events.healthy() {
                if !e.initialized {
                    return Err(error);
                }
                return File::progress(
                    &e.dev,
                    &mut e.prepared,
                    &e.sync,
                    &mut e.initialized,
                    &mut e.cursor,
                    &mut e.waiting,
                    &mut e.work_gate,
                    &mut e.render_receipts,
                    &e.publication,
                );
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
            File::progress(
                &e.dev,
                &mut e.prepared,
                &e.sync,
                &mut e.initialized,
                &mut e.cursor,
                &mut e.waiting,
                &mut e.work_gate,
                &mut e.render_receipts,
                &e.publication,
            )
        })();
        match result {
            Ok(None) if input_blocked => {
                // Input/queue callbacks were installed before commitment.
                // A racing signal requeues the running work; no GPU deadline
                // or periodic poll applies to a job waiting on caller fences.
            }
            Ok(None) => {
                this.firmware_waiting.store(true, Ordering::SeqCst);
                if this.events.wake_sequence.load(Ordering::SeqCst) != wake_sequence {
                    let _ = workqueue::system_unbound().enqueue::<_, 0>(this.clone());
                }
                // Retain the bounded watchdog for genuinely absent/coalesced
                // notifications, even when the sequence change requeues work.
                let _ = workqueue::system_unbound()
                    .enqueue_delayed::<_, 1>(this.clone(), kernel::time::msecs_to_jiffies(10));
            }
            result => {
                let execution = held.take().unwrap();
                let error = match result {
                    Ok(Some(error)) => error,
                    Err(error) => Some(error),
                    _ => unreachable!(),
                };
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
    fence: dma_fence::Fence,
    registered: bool,
}
// SAFETY: Only the DMA-fence core mutates callback, under its own fence lock.
// events and fence are refcounted; Drop removes under the same lock.
unsafe impl Send for FenceWake {}
unsafe impl Sync for FenceWake {}
impl FenceWake {
    fn new(fence: dma_fence::Fence, events: Arc<Events>, identity: usize) -> Result<KBox<Self>> {
        let mut owned = KBox::new(
            Self {
                // SAFETY: add_callback initializes callback's list linkage.
                callback: UnsafeCell::new(unsafe { core::mem::zeroed() }),
                events,
                identity,
                fence,
                registered: false,
            },
            GFP_KERNEL,
        )?;
        // SAFETY: Stable owned allocation and fence survive until removal;
        // callback only accesses immutable events and never a sleeping lock.
        let result = unsafe {
            bindings::dma_fence_add_callback(
                owned.fence.raw(),
                owned.callback.get(),
                Some(Self::wake),
            )
        };
        if result != 0 && result != -(bindings::ENOENT as i32) {
            return Err(Error::from_errno(result));
        }
        owned.registered = true;
        Ok(owned)
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
            bindings::dma_fence_remove_callback(self.fence.raw(), self.callback.get());
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
