// SPDX-License-Identifier: GPL-2.0-only
// Copyright 2026 The Asahi Linux Contributors

//! Collecting finished jobs into a bounded, reusable list.
//!
//! The job worker moves finished jobs out of the engine's job list into a
//! list preallocated to the scheduler's credit limit, and signals them with
//! the locks dropped. Both lists are fixed-capacity: nothing here allocates,
//! and a job that does not fit stays where it is for a later pass instead of
//! being lost or panicking. Pure list logic, host-tested.

/// A fixed-capacity list as this logic sees it (`KVec` in the kernel).
pub(crate) trait Bounded<T> {
    fn len(&self) -> usize;
    fn capacity(&self) -> usize;
    fn get_at(&self, index: usize) -> Option<&T>;
    fn remove_at(&mut self, index: usize) -> Option<T>;
    /// Push without growing; gives the value back if full.
    fn push_within(&mut self, value: T) -> Result<(), T>;
    /// Insert without growing; gives the value back if full.
    fn insert_within(&mut self, index: usize, value: T) -> Result<(), T>;
}

/// Moves every job of `jobs` for which `done` holds into `finished`, keeping
/// both lists' order. Stops early, leaving the rest in `jobs`, when `finished`
/// is full. Returns whether every finished job was moved.
pub(crate) fn collect<T, L: Bounded<T>>(jobs: &mut L, finished: &mut L, done: impl Fn(&T) -> bool) -> bool {
    let mut index = 0;
    while let Some(job) = jobs.get_at(index) {
        if !done(job) {
            index += 1;
            continue;
        }
        if finished.len() >= finished.capacity() {
            return false;
        }
        let Some(job) = jobs.remove_at(index) else { return false };
        if let Err(job) = finished.push_within(job) {
            // Unreachable after the check; put the job back where it was.
            let _ = jobs.insert_within(index, job);
            return false;
        }
    }
    true
}
