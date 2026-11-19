// SPDX-License-Identifier: GPL-2.0-only
// Included by test-drain-progress.py with the production g16_drain.rs.
use g16_drain::{Clock, Stats, Verdict, CAP_MS};

#[test]
fn all_signaled_is_done_at_once() {
    assert_eq!(Clock::new(0, 12000, CAP_MS).update(0, 0), Verdict::Done);
    let mut c = Clock::new(3, 12000, CAP_MS);
    assert_eq!(c.update(5, 0), Verdict::Done);
    assert_eq!(c.stats(), Stats { waited_ms: 5, signaled: 3, remaining: 0, idle_ms: 0 });
}

#[test]
fn no_progress_fails_exactly_at_the_budget() {
    let mut c = Clock::new(32, 12000, CAP_MS);
    assert_eq!(c.update(0, 32), Verdict::Wait(12000));
    assert_eq!(c.update(11_999, 32), Verdict::Wait(1));
    assert_eq!(c.update(12_000, 32), Verdict::NoProgress);
    assert_eq!(c.stats(), Stats { waited_ms: 12_000, signaled: 0, remaining: 32, idle_ms: 12_000 });
}

#[test]
fn deep_slow_queue_that_keeps_retiring_completes() {
    // batch-0330 shape: 32 Works retiring about one per 1.1 s takes ~35 s,
    // three times the old fixed 12 s budget.
    let mut c = Clock::new(32, 12000, CAP_MS);
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
    let mut c = Clock::new(4, 12000, CAP_MS);
    assert!(matches!(c.update(3000, 3), Verdict::Wait(12000)));
    assert_eq!(c.update(14_999, 3), Verdict::Wait(1));
    assert_eq!(c.update(15_000, 3), Verdict::NoProgress);
    assert_eq!(c.stats(), Stats { waited_ms: 15_000, signaled: 1, remaining: 3, idle_ms: 12_000 });
}

#[test]
fn trickling_progress_is_capped() {
    let budget = 12000;
    let mut c = Clock::new(usize::MAX, budget, CAP_MS);
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
fn budgets_and_caps_are_independent_and_never_zero() {
    let mut c = Clock::new(1, 35_000, CAP_MS);
    assert_eq!(c.update(34_999, 1), Verdict::Wait(1));
    assert_eq!(c.update(35_000, 1), Verdict::NoProgress);
    // A cap below the budget ends the drain first, as Capped.
    let mut c = Clock::new(2, 12_000, 5_000);
    assert_eq!(c.update(4_000, 1), Verdict::Wait(1_000));
    assert_eq!(c.update(5_000, 1), Verdict::Capped);
    // Zero budget or an exhausted shared deadline behave as 1 ms, never
    // as "wait forever".
    assert_eq!(Clock::new(1, 0, CAP_MS).update(1, 1), Verdict::NoProgress);
    assert_eq!(Clock::new(1, 12_000, 0).update(1, 1), Verdict::Capped);
    assert_eq!(Clock::new(1, 12_000, -50).update(0, 1), Verdict::Wait(1));
}

#[test]
fn a_shared_deadline_bounds_several_progressing_drains() {
    // The exit path drains each VM in turn from one per-file deadline:
    // VMs that keep retiring cannot add up to more than CAP_MS.
    let mut used = 0;
    for _vm in 0..4 {
        let mut c = Clock::new(usize::MAX, 12_000, CAP_MS - used);
        let mut remaining = usize::MAX;
        let mut now = 0;
        loop {
            remaining -= 1;
            match c.update(now, remaining) {
                // Like drain_within: sleep at most what Wait allows.
                Verdict::Wait(left) => now += left.min(1000),
                Verdict::Capped => break,
                other => panic!("{other:?}"),
            }
        }
        used += now;
    }
    assert!(used <= CAP_MS + 4, "{used}");
}

#[test]
fn growth_and_clock_regressions_are_not_progress() {
    let mut c = Clock::new(2, 12000, CAP_MS);
    assert!(matches!(c.update(1000, 3), Verdict::Wait(11000)));
    assert_eq!(c.stats().remaining, 2);
    // A backwards clock reading is ignored.
    assert!(matches!(c.update(500, 2), Verdict::Wait(11000)));
    assert_eq!(c.update(12_000, 2), Verdict::NoProgress);
}
