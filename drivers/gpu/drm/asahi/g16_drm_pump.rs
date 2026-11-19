// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! The submission pump: the single worker that admits scheduler jobs,
//! publishes them to firmware, retires them and signals their fences.

use super::*;

impl crate::g16_queue_fail::QueueJob for Running {
    type Error = Error;
    fn same_queue(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.execution.firmware, &other.execution.firmware)
    }
    fn error_mut(&mut self) -> &mut Option<Error> {
        &mut self.error
    }
}

impl Engine {
    /// Move the finished jobs into `self.finished`, in order. The list keeps its
    /// JOB_CREDITS capacity: the caller takes it only to signal its jobs and
    /// puts it back. A job that does not fit (an accounting bug: both lists
    /// have JOB_CREDITS capacity) stays in `jobs` for a later pass.
    pub(super) fn take_finished(&mut self) {
        let done = |job: &Running| {
            job.pending == 0 && (job.error.is_some() || job.next == job.execution.parameters.len())
        };
        if !crate::g16_finished::collect(&mut self.jobs, &mut self.finished, done) {
            warn_once(&FINISHED_FULL, "finished list full; signalling deferred");
        }
        // Removal shifts later entries; keep the admission cursor valid.
        if self.cursor > self.jobs.len() {
            self.cursor = 0;
        }
    }

    /// Take the jobs drm_sched has run since the last pass, in order. The
    /// inbox keeps its capacity. On failure the jobs stay in the inbox for the
    /// error path to fail.
    pub(super) fn admit(&mut self, inbox: &Mutex<Inbox>) -> Result {
        let mut inbox = inbox.lock();
        // Inbox and engine together hold at most JOB_CREDITS jobs. If that ever
        // fails, admit what fits and leave the rest queued for a later pass.
        let room = self.jobs.capacity() - self.jobs.len();
        if room < inbox.jobs.len() {
            warn_once(&ADMIT_FULL, "engine job list full; admission deferred");
        }
        // Oldest first, removed in place: the inbox keeps its capacity.
        for _ in 0..room.min(inbox.jobs.len()) {
            let job = inbox.jobs.remove(0).map_err(|_| EIO)?;
            if let Err(job) = self.jobs.push_within_capacity(job) {
                // Unreachable within `room`; keep the job queued.
                let _ = inbox.jobs.insert_within_capacity(0, job.0);
                break;
            }
        }
        Ok(())
    }
}

impl Execution {
    /// Call only after dropping the engine and runtime locks.
    pub(super) fn signal_finished(&self, job: Running) {
        if let Some(ring) = &self.device.timeline {
            trace_record(ring, &job);
        }
        if let Some(error) = job.error {
            job.execution.completion.set_error(error);
        }
        let _ = job.execution.completion.signal();
        // A deferred GEM close of this file may wait for this fence. Pairs
        // with the SeqCst store in Object::close.
        let reaper = &job.execution.reaper;
        if reaper.armed.load(Ordering::SeqCst) {
            let _ = workqueue::system_dfl().enqueue(reaper.clone());
        }
    }
}

impl WorkItem for Execution {
    type Pointer = Arc<Self>;

    fn run(this: Arc<Self>) {
        // Exactly one pump owns publication order. It holds these locks only
        // while preparing/publishing or inspecting retirement, never asleep.
        // Scheduler callbacks append ready jobs even while hardware is busy.
        let mut wake_result = Ok(());
        loop {
            let generation = this.device.notifications.snapshot();
            let mut engine = this.device.engine.lock();
            let taken = engine.admit(&this.device.inbox);
            let mut runtime = this.device.runtime.lock();
            let result = (|| -> Result<bool> {
                let _t = crate::g16_timeline::Timer::start(&crate::g16_timeline::P_PUMP);
                taken?;
                // No notification within the watchdog window: poll the stamps.
                // Retirement's watchdog reports a real hang; a lost
                // notification simply retires the finished work. Report what
                // the poll found: retired work means a notification was late
                // or lost, nothing retired means slow work.
                let timed_out = match wake_result {
                    Err(e) if e == ETIMEDOUT => Some(runtime.oldest_flight_ms()),
                    Err(e) => return Err(e),
                    Ok(()) => None,
                };
                generation?;
                if this.device.failed.load(Ordering::Acquire) {
                    return Err(EIO);
                }
                let mut progress = false;
                let mut polled_retired = 0u32;
                runtime.set_poll_scan(timed_out.is_some());
                while let Some((id, result)) = runtime.retire().inspect_err(|error| {
                    pr_err!("G16: firmware retirement failed: {:?}\n", error);
                })? {
                    let index = engine.issued.iter().position(|r| r.id == id).ok_or(EIO)?;
                    let issued = engine.issued.remove(index).map_err(|_| EIO)?;
                    let at = engine
                        .jobs
                        .iter()
                        .position(|r| r.execution.id == issued.owner)
                        .ok_or(EIO)?;
                    let result = match result {
                        Ok(())
                            if crate::g16_timeline::take_injection(
                                &crate::g16_timeline::FAIL_RETIRE,
                                execution_tgid(&engine.jobs[at].execution),
                            ) =>
                        {
                            Err(ENOMEM)
                        }
                        result => result,
                    };
                    if let Err(error) = result {
                        pr_warn!(
                            "G16: execution {} completed with {:?}; later work on its queue is cancelled\n",
                            engine.jobs[at].execution.id,
                            error
                        );
                        // Later jobs on this queue may already be published,
                        // so the publication latch alone cannot stop them.
                        // They complete with ECANCELED instead of reporting
                        // success over this job's incomplete output.
                        engine.jobs[at].execution.firmware.lock().rejected = true;
                        crate::g16_queue_fail::cancel_later(&mut engine.jobs, at, ECANCELED);
                    }
                    let job = &mut engine.jobs[at];
                    job.pending -= 1;
                    if job.pending == 0 && crate::module_parameters::timeline.value() >= 2 {
                        let n = &this.device.notifications;
                        job.seen = ticks();
                        job.irq = n.last_irq.load(Ordering::Relaxed);
                        job.rx = n.last_rx.load(Ordering::Relaxed);
                        job.wake = n.last_wake.load(Ordering::Relaxed);
                    }
                    if let Err(error) = result {
                        job.error = Some(error);
                    }
                    progress = true;
                    polled_retired += 1;
                }
                runtime.set_poll_scan(false);
                if let Some(age) = timed_out {
                    use crate::g16_timeline as t;
                    t::POLL_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
                    t::POLL_RETIRED.fetch_add(u64::from(polled_retired), Ordering::Relaxed);
                    // A safety poll every few ms is routine; report only polls
                    // that found finished work (each such Work is also
                    // classified by the retire scan: "G16: rescue"), or long
                    // waits. Long waits with nothing retired are slow work (the
                    // retirement watchdog reports hangs): at most one line per
                    // 10 s.
                    if polled_retired != 0 {
                        pr_warn!(
                            "G16: notification wait timed out; polling retired {} Work(s), oldest flight {} ms\n",
                            polled_retired,
                            age
                        );
                    } else if age >= 1000 {
                        let now = ticks();
                        let last = t::SLOW_POLL_REPORTED.load(Ordering::Relaxed);
                        if last == 0 || now.wrapping_sub(last) >= 10 * tick_hz() {
                            t::SLOW_POLL_REPORTED.store(now, Ordering::Relaxed);
                            pr_warn!(
                                "G16: notification wait timed out; polling retired 0 Work(s), oldest flight {} ms ({} similar suppressed)\n",
                                age,
                                t::SLOW_POLL_SUPPRESSED.swap(0, Ordering::Relaxed)
                            );
                        } else {
                            t::SLOW_POLL_SUPPRESSED.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                Ok(progress)
            })();
            // Signal every finished job before admitting more work, with the
            // locks dropped: a completed fence must not wait behind a
            // publication pass. Upstream signals from its event path.
            if result.is_ok() {
                engine.take_finished();
            }
            let signalled = !engine.finished.is_empty();
            if signalled {
                // Out of the engine only while its jobs are signalled with the
                // locks dropped; always put back, keeping its capacity.
                let mut finished = core::mem::take(&mut engine.finished);
                drop(runtime);
                drop(engine);
                for job in finished.drain_all() {
                    this.signal_finished(job);
                }
                engine = this.device.engine.lock();
                engine.finished = finished;
                runtime = this.device.runtime.lock();
            }
            let result = result.and_then(|progress| (|| -> Result<bool> {
                let mut progress = progress || signalled;
                let mut admitted = 0;
                loop {
                    let previous = admitted;
                    let start = engine.cursor;
                    for offset in 0..engine.jobs.len() {
                        let index = (start + offset) % engine.jobs.len();
                        // Preserve submit-relative barrier index zero: all earlier
                        // submissions on this public queue must have published.
                        if engine.jobs[..index].iter().any(|earlier| {
                            earlier.error.is_none()
                                && earlier.next < earlier.execution.parameters.len()
                                && Arc::ptr_eq(
                                    &earlier.execution.firmware,
                                    &engine.jobs[index].execution.firmware,
                                )
                        }) {
                            continue;
                        }
                        let job = &mut engine.jobs[index];
                        if job.error.is_some() || job.next == job.execution.parameters.len() {
                            continue;
                        }
                        if job.history[0].is_empty() {
                            let firmware = job.execution.firmware.lock();
                            if firmware.rejected {
                                // Queued behind a rejected command on this
                                // queue: fail without publishing anything.
                                drop(firmware);
                                job.error = Some(ECANCELED);
                                progress = true;
                                continue;
                            }
                            let previous = firmware.last;
                            drop(firmware);
                            for (history, stamp) in job.history.iter_mut().zip(previous) {
                                // SUBMIT reserved 65: at most 64 commands plus this one.
                                history.push_within_capacity(stamp).map_err(|_| EIO)?;
                            }
                        }
                        let dependencies =
                            resolve_barriers(&job.history, job.execution.barriers[job.next])?;
                        let execution = job.execution.clone();
                        let compute =
                            matches!(execution.parameters[job.next], Parameters::Compute(_));
                        // Ordinary preparation leaves intra-queue waits to the
                        // firmware. Paired mesh stream decoding waits for those
                        // producer stamps before reading command words on CPU.
                        let job = &mut engine.jobs[index];
                        crate::mem::sync();
                        let injected = crate::g16_timeline::take_injection(
                            &crate::g16_timeline::FAIL_PREP,
                            execution_tgid(&execution),
                        );
                        let receipt = match &execution.parameters[job.next] {
                            _ if injected => Err(EINVAL),
                            Parameters::Render(p) => runtime.submit_render(
                                execution.firmware.clone(),
                                execution.render_space.clone(),
                                p,
                                execution.support.as_deref().ok_or(EIO)?,
                                execution.timestamps[job.next],
                                &dependencies,
                                // The submission's identity: with
                                // shared_notifier=1 its renders share a notifier.
                                execution.id,
                            ),
                            Parameters::Compute(p) => runtime.submit_compute(
                                execution.firmware.clone(),
                                execution.space.clone(),
                                p,
                                execution.timestamps[job.next],
                                &dependencies,
                            ),
                        };
                        let receipt = match receipt {
                            Ok(receipt) => receipt,
                            Err(EBUSY) if !runtime.render_failed() => {
                                crate::g16_timeline::DEFERRED.fetch_add(1, Ordering::Relaxed);
                                if crate::module_parameters::timeline.value() != 0 {
                                    job.busy += 1;
                                    job.why |= match runtime.take_pressure() {
                                        0 => 0x100,
                                        bits => bits,
                                    };
                                }
                                continue;
                            }
                            Err(error) if !runtime.render_failed() => {
                                pr_err!(
                                    "G16: command {} preparation failed: {:?}; later work on its queue is cancelled\n",
                                    job.next,
                                    error
                                );
                                execution.firmware.lock().rejected = true;
                                job.error = Some(error);
                                progress = true;
                                continue;
                            }
                            Err(error) => return Err(error),
                        };
                        job.history[usize::from(compute)]
                            .push_within_capacity(Some(receipt.stamp))
                            .map_err(|_| EIO)?;
                        let issued = Issued {
                            id: receipt.id,
                            owner: execution.id,
                        };
                        if crate::module_parameters::timeline.value() != 0 {
                            let now = ticks();
                            if job.next == 0 {
                                job.first = now;
                            }
                            job.last = now;
                        }
                        job.next += 1;
                        job.pending += 1;
                        // One entry per flight, and reserve_flight keeps
                        // flights below MAX_FLIGHTS: always room.
                        engine.issued.push_within_capacity(issued).map_err(|_| EIO)?;
                        // Resume after the last admitted job when a scarce slot
                        // becomes free, rather than repeatedly favoring index zero.
                        engine.cursor = index + 1;
                        progress = true;
                        admitted += 1;
                        if admitted == 64 {
                            break;
                        }
                    }
                    if admitted == previous || admitted == 64 {
                        break;
                    }
                }
                runtime.flush()?;
                if crate::module_parameters::timeline.value() >= 2 {
                    let now = ticks();
                    for job in engine.jobs.iter_mut().filter(|j| j.first != 0 && j.kick == 0) {
                        job.kick = now;
                    }
                }
                Ok(progress)
            })());
            if let Err(error) = result {
                pr_err!(
                    "G16: submission pipeline failed: {:?}, {} jobs, {} issued\n",
                    error,
                    engine.jobs.len(),
                    engine.issued.len()
                );
                runtime.poison();
                this.device.failed.store(true, Ordering::Release);
                let mut jobs = core::mem::take(&mut engine.jobs);
                engine.issued.clear();
                let queued = {
                    let mut inbox = this.device.inbox.lock();
                    inbox.running = false;
                    core::mem::take(&mut inbox.jobs)
                };
                drop(runtime);
                drop(engine);
                // Quarantined context roots remain installed and retained;
                // a failed fence never authorizes reuse of their backing.
                for job in Iterator::chain(jobs.drain_all(), queued) {
                    job.execution.completion.set_error(error);
                    let _ = job.execution.completion.signal();
                }
                return;
            }
            if let Some(index) = engine.jobs.iter().position(|j| {
                j.pending == 0 && (j.error.is_some() || j.next == j.execution.parameters.len())
            }) {
                let job = engine
                    .jobs
                    .remove(index)
                    .expect("index found under engine lock");
                drop(runtime);
                drop(engine);
                this.signal_finished(job);
                continue;
            }
            if engine.jobs.is_empty() {
                // Stop only if no job arrived since this pass drained the
                // inbox; run_job sees `running` cleared and queues a new pump.
                let mut inbox = this.device.inbox.lock();
                if inbox.jobs.is_empty() {
                    inbox.running = false;
                    return;
                }
                drop(inbox);
                drop(runtime);
                drop(engine);
                continue;
            }
            let timeout_ms = runtime.watchdog_remaining_ms();
            drop(runtime);
            drop(engine);
            if !result.unwrap_or(false) {
                wake_result = this.device.notifications.wait(
                    generation.expect("checked before servicing jobs"),
                    timeout_ms,
                );
                if crate::module_parameters::timeline.value() >= 2 {
                    this.device.notifications.last_wake.store(ticks(), Ordering::Relaxed);
                }
            }
        }
    }
}

#[pinned_drop]
impl PinnedDrop for Execution {
    fn drop(self: Pin<&mut Self>) {
        // An entity can discard a job still waiting for its dependencies.
        // Such a job never published to firmware, but its fence must finish.
        if !self.started.load(Ordering::Acquire) {
            self.completion.set_error(ECANCELED);
            let _ = self.completion.signal();
        }
    }
}

impl sched::JobImpl for SubmissionJob {
    fn run(job: &mut sched::Job<Self>) -> Result<Option<dma_fence::Fence>> {
        // SUBMIT allocated it; run_job runs once per job.
        let history = job.history.take();
        let execution = &job.execution;
        if crate::module_parameters::timeline.value() != 0 {
            execution.trace.run.store(ticks(), Ordering::Relaxed);
        }
        // Firmware jobs cannot be replayed after a fault without a reboot.
        if execution.started.swap(true, Ordering::AcqRel) {
            return Err(EIO);
        }
        let fence = execution.completion.clone();
        let running = (|| -> Result<Running> {
            let history = history.ok_or(EIO)?;
            Ok(Running {
                execution: execution.clone(),
                next: 0,
                first: 0,
                last: 0,
                busy: 0,
                why: 0,
                kick: 0,
                irq: 0,
                rx: 0,
                wake: 0,
                seen: 0,
                pending: 0,
                error: None,
                history,
            })
        })();
        let mut inbox = execution.device.inbox.lock();
        // JOB_CREDITS capacity: drm_sched never runs more jobs than that at once.
        if let Err(error) = running.and_then(|r| inbox.jobs.push_within_capacity(r).map_err(|_| EIO)) {
            drop(inbox);
            execution.completion.set_error(error);
            let _ = execution.completion.signal();
            return Err(error);
        }
        if !inbox.running {
            inbox.running = true;
            let _ = workqueue::system_dfl().enqueue(execution.clone());
        }
        drop(inbox);
        // A running worker may be asleep waiting for firmware on another
        // queue. New ready work must wake it even if no GPU event arrives.
        execution.device.notifications.notify(false);
        Ok(Some(fence))
    }

    fn timed_out(job: &mut sched::Job<Self>) -> sched::Status {
        job.execution.device.failed.store(true, Ordering::Release);
        job.execution.device.notifications.notify(false);
        // Wake the worker to quarantine firmware mappings and fail the jobs.
        // Do not signal its lifetime fence before that cleanup here.
        sched::Status::NoDevice
    }

    fn cancel(job: &mut sched::Job<Self>) {
        if !job.execution.started.load(Ordering::Acquire) {
            job.execution.completion.set_error(ECANCELED);
            let _ = job.execution.completion.signal();
        }
    }
}

#[vtable]
impl dma_fence::FenceOps for Completion {
    fn get_driver_name<'a>(self: &'a dma_fence::FenceObject<Self>) -> &'a kernel::str::CStr {
        c_str!("asahi")
    }
    fn get_timeline_name<'a>(self: &'a dma_fence::FenceObject<Self>) -> &'a kernel::str::CStr {
        c_str!("m4-execution")
    }
}
