// SPDX-License-Identifier: GPL-2.0-only
// Copyright 2026 The Asahi Linux Contributors

//! Failure of a public queue.
//!
//! Userspace may split one submission into several jobs whose later parts
//! rely on queue order, and only the last one carries the application's
//! fence. Once a command on a queue fails, later jobs on that queue must not
//! report success over its missing or incomplete output: they complete with
//! ECANCELED, and the queue refuses new work. Pure list logic, host-tested.

/// A queued job as this logic sees it.
pub(crate) trait QueueJob {
    type Error: Copy;
    /// Whether both jobs were submitted to the same public queue.
    fn same_queue(&self, other: &Self) -> bool;
    fn error_mut(&mut self) -> &mut Option<Self::Error>;
}

/// Gives every job after `jobs[at]` (in submission order) on the same queue
/// `cancelled`, unless it already failed. Returns how many jobs changed.
pub(crate) fn cancel_later<J: QueueJob>(jobs: &mut [J], at: usize, cancelled: J::Error) -> usize {
    if at >= jobs.len() {
        return 0;
    }
    let (head, later) = jobs.split_at_mut(at + 1);
    let failed = &head[at];
    let mut changed = 0;
    for job in later.iter_mut().filter(|job| job.same_queue(failed)) {
        let error = job.error_mut();
        if error.is_none() {
            *error = Some(cancelled);
            changed += 1;
        }
    }
    changed
}
