// SPDX-License-Identifier: GPL-2.0-only
// Copyright 2026 The Asahi Linux Contributors

//! Progress-based drain policy for a VM's pending fences.
//!
//! A drain fails when none of the VM's own fences has signaled for the
//! no-progress budget, not after a fixed total time: a deep queue of slow
//! but retiring work (several parallel clients, long shaders) can need far
//! longer than one budget, while a stuck or starved VM still fails within
//! it. An overall cap keeps every drain bounded even if progress trickles.
//! Pure arithmetic on caller-supplied times, so it is host-testable.

/// Longest any drain may take, however steadily its fences signal.
pub(crate) const CAP_MS: i64 = 120_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Every fence has signaled.
    Done,
    /// Keep waiting, for at most this many ms before re-checking.
    Wait(i64),
    /// No fence signaled for the whole no-progress budget.
    NoProgress,
    /// The overall cap ran out while fences were still signaling.
    Capped,
}

/// What a drain observed, for its caller's log line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Stats {
    /// Time since the drain started (ms).
    pub(crate) waited_ms: i64,
    /// Fences that signaled during the drain.
    pub(crate) signaled: usize,
    /// Fences still pending at the last check.
    pub(crate) remaining: usize,
    /// Time since the last fence signaled, or since the start (ms).
    pub(crate) idle_ms: i64,
}

pub(crate) struct Clock {
    budget_ms: i64,
    cap_ms: i64,
    initial: usize,
    remaining: usize,
    last_progress_ms: i64,
    now_ms: i64,
}

impl Clock {
    /// Start a drain of `remaining` fences with a no-progress budget of
    /// `budget_ms` and an overall cap of `cap_ms` (callers sharing one
    /// deadline pass what is left of it; at least 1 ms).
    pub(crate) fn new(remaining: usize, budget_ms: u32, cap_ms: i64) -> Self {
        let budget_ms = i64::from(budget_ms).max(1);
        Self {
            budget_ms,
            cap_ms: cap_ms.max(1),
            initial: remaining,
            remaining,
            last_progress_ms: 0,
            now_ms: 0,
        }
    }

    /// Record the pending count at `now_ms` (ms since the start, monotonic)
    /// and decide. A smaller count is progress. The pending set only
    /// shrinks during a drain: new submissions are blocked or impossible.
    pub(crate) fn update(&mut self, now_ms: i64, remaining: usize) -> Verdict {
        self.now_ms = now_ms.max(self.now_ms);
        if remaining < self.remaining {
            self.last_progress_ms = self.now_ms;
        }
        self.remaining = remaining.min(self.remaining);
        if self.remaining == 0 {
            return Verdict::Done;
        }
        let idle_left = self.budget_ms - (self.now_ms - self.last_progress_ms);
        let cap_left = self.cap_ms - self.now_ms;
        if idle_left <= 0 {
            Verdict::NoProgress
        } else if cap_left <= 0 {
            Verdict::Capped
        } else {
            Verdict::Wait(idle_left.min(cap_left))
        }
    }

    pub(crate) fn stats(&self) -> Stats {
        Stats {
            waited_ms: self.now_ms,
            signaled: self.initial - self.remaining,
            remaining: self.remaining,
            idle_ms: self.now_ms - self.last_progress_ms,
        }
    }
}
