// SPDX-License-Identifier: GPL-2.0-only
// Included by test-drain-progress.py with the production g16_drain.rs.
use g16_drain::{Clock, Stats, Verdict, CAP_MS};

#[test]
fn all_signaled_is_done_at_once() {
    assert_eq!(Clock::new(0, 12000).update(0, 0), Verdict::Done);
    let mut c = Clock::new(3, 12000);
    assert_eq!(c.update(5, 0), Verdict::Done);
    assert_eq!(c.stats(), Stats { waited_ms: 5, signaled: 3, remaining: 0, idle_ms: 0 });
}

#[test]
fn no_progress_fails_exactly_at_the_budget() {
    let mut c = Clock::new(32, 12000);
    assert_eq!(c.update(0, 32), Verdict::Wait(12000));
    assert_eq!(c.update(11_999, 32), Verdict::Wait(1));
    assert_eq!(c.update(12_000, 32), Verdict::NoProgress);
    assert_eq!(c.stats(), Stats { waited_ms: 12_000, signaled: 0, remaining: 32, idle_ms: 12_000 });
}

#[test]
fn deep_slow_queue_that_keeps_retiring_completes() {
    // batch-0330 shape: 32 Works retiring about one per 1.1 s takes ~35 s,
    // three times the old fixed 12 s budget.
    let mut c = Clock::new(32, 12000);
    let mut now = 0;
    for remaining in (1..=32).rev() {
        assert!(matches!(c.update(now, remaining), Verdict::Wait(_)), "at {now} ms");
        now += 1100;
    }
    assert_eq!(c.update(now, 0), Verdict::Done);
    assert_eq!(c.stats().signaled, 32);
    assert!(now > 12000);
}

#[test]
fn a_stall_after_progress_fails_one_budget_after_the_last_signal() {
    let mut c = Clock::new(4, 12000);
    assert!(matches!(c.update(3000, 3), Verdict::Wait(12000)));
    assert_eq!(c.update(14_999, 3), Verdict::Wait(1));
    assert_eq!(c.update(15_000, 3), Verdict::NoProgress);
    assert_eq!(c.stats(), Stats { waited_ms: 15_000, signaled: 1, remaining: 3, idle_ms: 12_000 });
}

#[test]
fn trickling_progress_is_capped() {
    let budget = 12000;
    let mut c = Clock::new(usize::MAX, budget);
    let mut remaining = usize::MAX;
    let mut now = 0;
    loop {
        remaining -= 1;
        match c.update(now, remaining) {
            Verdict::Wait(left) => {
                assert!(left <= i64::from(budget) && left <= CAP_MS - now && left > 0);
            }
            Verdict::Capped => break,
            other => panic!("{other:?} at {now} ms"),
        }
        now += 1000;
    }
    assert_eq!(now, CAP_MS);
}

#[test]
fn the_cap_is_at_least_the_budget() {
    let mut c = Clock::new(1, 200_000);
    assert_eq!(c.update(150_000, 1), Verdict::Wait(50_000));
    assert_eq!(c.update(200_000, 1), Verdict::NoProgress);
    let mut c = Clock::new(1, 35_000);
    assert_eq!(c.update(34_999, 1), Verdict::Wait(1));
    assert_eq!(c.update(35_000, 1), Verdict::NoProgress);
    // A zero budget behaves as 1 ms, never as "wait forever".
    let mut c = Clock::new(1, 0);
    assert_eq!(c.update(1, 1), Verdict::NoProgress);
}

#[test]
fn growth_and_clock_regressions_are_not_progress() {
    let mut c = Clock::new(2, 12000);
    assert!(matches!(c.update(1000, 3), Verdict::Wait(11000)));
    assert_eq!(c.stats().remaining, 2);
    // A backwards clock reading is ignored.
    assert!(matches!(c.update(500, 2), Verdict::Wait(11000)));
    assert_eq!(c.update(12_000, 2), Verdict::NoProgress);
}
