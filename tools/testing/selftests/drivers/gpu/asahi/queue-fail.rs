// SPDX-License-Identifier: GPL-2.0-only
// Included by test-queue-fail.py with the production g16_queue_fail.rs.
use g16_queue_fail::{cancel_later, QueueJob};

#[derive(Debug, PartialEq)]
struct J(u32, Option<i32>);

impl QueueJob for J {
    type Error = i32;
    fn same_queue(&self, other: &Self) -> bool {
        self.0 == other.0
    }
    fn error_mut(&mut self) -> &mut Option<i32> {
        &mut self.1
    }
}

const ENOMEM: i32 = -12;
const ECANCELED: i32 = -125;

#[test]
fn later_jobs_on_the_queue_are_cancelled() {
    let mut jobs = vec![J(1, None), J(1, Some(ENOMEM)), J(2, None), J(1, None), J(1, None)];
    assert_eq!(cancel_later(&mut jobs, 1, ECANCELED), 2);
    assert_eq!(
        jobs,
        [J(1, None), J(1, Some(ENOMEM)), J(2, None), J(1, Some(ECANCELED)), J(1, Some(ECANCELED))]
    );
}

#[test]
fn earlier_jobs_and_other_queues_are_untouched() {
    let mut jobs = vec![J(1, None), J(2, None), J(1, Some(ENOMEM)), J(2, None)];
    assert_eq!(cancel_later(&mut jobs, 2, ECANCELED), 0);
    assert_eq!(jobs, [J(1, None), J(2, None), J(1, Some(ENOMEM)), J(2, None)]);
}

#[test]
fn an_existing_error_is_kept() {
    let mut jobs = vec![J(1, Some(ENOMEM)), J(1, Some(ENOMEM)), J(1, None)];
    assert_eq!(cancel_later(&mut jobs, 0, ECANCELED), 1);
    assert_eq!(jobs, [J(1, Some(ENOMEM)), J(1, Some(ENOMEM)), J(1, Some(ECANCELED))]);
}

#[test]
fn the_last_job_or_an_out_of_range_index_changes_nothing() {
    let mut jobs = vec![J(1, None), J(1, Some(ENOMEM))];
    assert_eq!(cancel_later(&mut jobs, 1, ECANCELED), 0);
    assert_eq!(cancel_later(&mut jobs, 5, ECANCELED), 0);
    assert_eq!(cancel_later(&mut Vec::<J>::new(), 0, ECANCELED), 0);
    assert_eq!(jobs, [J(1, None), J(1, Some(ENOMEM))]);
}
