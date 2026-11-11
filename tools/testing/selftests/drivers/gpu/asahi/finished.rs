// SPDX-License-Identifier: GPL-2.0-only
// Included by test-finished.py with the production g16_finished.rs.
use g16_finished::{collect, Bounded};

/// A fixed-capacity list, as KVec is used in the worker: it never grows.
#[derive(Default)]
struct List(Vec<u32>, usize);

impl List {
    fn with_capacity(n: usize) -> Self {
        List(Vec::new(), n)
    }
}

impl Bounded<u32> for List {
    fn len(&self) -> usize {
        self.0.len()
    }
    fn capacity(&self) -> usize {
        self.1
    }
    fn get_at(&self, index: usize) -> Option<&u32> {
        self.0.get(index)
    }
    fn remove_at(&mut self, index: usize) -> Option<u32> {
        (index < self.0.len()).then(|| self.0.remove(index))
    }
    fn push_within(&mut self, value: u32) -> Result<(), u32> {
        if self.0.len() < self.1 { self.0.push(value); Ok(()) } else { Err(value) }
    }
    fn insert_within(&mut self, index: usize, value: u32) -> Result<(), u32> {
        if self.0.len() < self.1 && index <= self.0.len() { self.0.insert(index, value); Ok(()) } else { Err(value) }
    }
}

const CREDITS: usize = 128;
const DONE: u32 = 1 << 31;

/// One worker pass, as Execution::run does it: collect in place, then take the
/// finished list only when it is non-empty, signal, and always put it back.
fn pass(jobs: &mut List, finished: &mut List, signalled: &mut Vec<u32>) {
    collect(jobs, finished, |j| j & DONE != 0);
    if finished.len() != 0 {
        let mut taken = core::mem::take(finished);
        signalled.extend(taken.0.drain(..).map(|j| j & !DONE));
        *finished = taken;
    }
}

#[test]
fn passes_with_nothing_finished_keep_the_list_capacity() {
    // The p2 boot panic: a pass that found nothing used to drop the list.
    let (mut jobs, mut finished, mut signalled) = (List::with_capacity(CREDITS), List::with_capacity(CREDITS), vec![]);
    jobs.push_within(1).unwrap();
    pass(&mut jobs, &mut finished, &mut signalled);
    assert_eq!(finished.capacity(), CREDITS);
    jobs.0[0] |= DONE;
    pass(&mut jobs, &mut finished, &mut signalled);
    assert_eq!((signalled, jobs.len(), finished.capacity()), (vec![1], 0, CREDITS));
}

#[test]
fn the_old_handoff_loses_the_capacity_and_collect_fails_closed() {
    // Replay of the panicking protocol (take every pass, restore only when
    // something was signalled): the list loses its capacity after one empty pass.
    let (mut jobs, mut finished) = (List::with_capacity(CREDITS), List::with_capacity(CREDITS));
    jobs.push_within(7).unwrap();
    let taken = core::mem::take(&mut finished);
    assert!(collect(&mut jobs, &mut List(taken.0, taken.1), |j| j & DONE != 0));
    // Not restored: `finished` is now the zero-capacity default.
    jobs.0[0] |= DONE;
    // The new collect does not panic or lose the job: it stays in `jobs`.
    assert!(!collect(&mut jobs, &mut finished, |j| j & DONE != 0));
    assert_eq!(jobs.0, vec![7 | DONE]);
}

#[test]
fn a_long_random_sequence_never_loses_jobs_or_capacity() {
    let (mut jobs, mut finished, mut signalled) = (List::with_capacity(CREDITS), List::with_capacity(CREDITS), vec![]);
    let (mut next, mut x) = (0u32, 0x2545f491u32);
    for _ in 0..200_000 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        // drm_sched admits a job only with a free credit.
        if x % 3 == 0 && jobs.len() < CREDITS {
            jobs.push_within(next).unwrap();
            next += 1;
        }
        // Some jobs finish (in any order), then a pass runs.
        if !jobs.0.is_empty() && x % 5 < 2 {
            let i = (x as usize >> 8) % jobs.0.len();
            jobs.0[i] |= DONE;
        }
        pass(&mut jobs, &mut finished, &mut signalled);
        assert_eq!(finished.capacity(), CREDITS);
        assert_eq!(finished.len(), 0);
    }
    // Every job is signalled exactly once or still pending.
    let mut seen: Vec<u32> = signalled.iter().copied().chain(jobs.0.iter().map(|j| j & !DONE)).collect();
    seen.sort_unstable();
    assert_eq!(seen, (0..next).collect::<Vec<_>>());
}

#[test]
fn collect_keeps_order_and_leaves_what_does_not_fit() {
    let mut jobs = List(vec![1 | DONE, 2, 3 | DONE, 4 | DONE, 5], 8);
    let mut finished = List::with_capacity(2);
    assert!(!collect(&mut jobs, &mut finished, |j| j & DONE != 0));
    assert_eq!(finished.0, vec![1 | DONE, 3 | DONE]);
    assert_eq!(jobs.0, vec![2, 4 | DONE, 5]);
}
