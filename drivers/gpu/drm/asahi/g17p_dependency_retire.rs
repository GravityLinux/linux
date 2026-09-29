// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Synchronous native retirement gates from G17PQueueFence and shim finishers.
//! Queue retirement, command status and owned report closure are distinct.
//! These gates authorize no storage reuse by themselves: Session must finish
//! copyback/lifetime handling before committing each returned owner.

use super::g17p_queue as q;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    Opening,
    Render,
    Closing,
}
#[derive(Clone, Copy)]
pub(crate) struct Observation {
    pub(crate) done: u32,
    pub(crate) counters: [u32; 3],
    // Compute uses its own eight-byte status_b, zero-padded here; render uses
    // the complete 0x40-byte destination recorded before publication.
    pub(crate) status: [u8; 0x40],
}
pub(crate) struct Reports {
    /// Set only after validation/service of the owned primary and secondary
    /// reports. An unsupported/unattributed report fails the whole live set.
    pub(crate) valid: bool,
    /// The source's growth predicate: a new owned render terminal or qualified
    /// owned limit closure. CL terminal counts never satisfy this predicate.
    pub(crate) render_complete: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidCounters,
    Reports,
    Order,
    Failed,
}
pub(crate) struct Retirement {
    publications: [q::Publication; 4],
    initial: [[u8; 0x40]; 4],
    transport: [bool; 4],
    next: usize,
    offered: Option<Owner>,
    failed: bool,
}
const ORDER: [Owner; 3] = [Owner::Closing, Owner::Render, Owner::Opening];
impl Retirement {
    pub(crate) fn new(publications: [q::Publication; 4], initial: [[u8; 0x40]; 4]) -> Self {
        Self {
            publications,
            initial,
            transport: [false; 4],
            next: 0,
            offered: None,
            failed: false,
        }
    }
    /// Observe all queue/channel owners, including the shared CL channel's
    /// opening prefix. Completion remains latched after backing/cursors change,
    /// as in the source completion ledger. Every command's status is separate.
    pub(crate) fn poll(
        &mut self,
        observations: &[Observation; 4],
        reports: Reports,
    ) -> core::result::Result<Option<Owner>, Error> {
        if self.failed {
            return Err(Error::Failed);
        }
        if !reports.valid {
            self.fail();
            return Err(Error::Reports);
        }
        let mut counters = [q::Counters([0; 3]); 4];
        for (i, observation) in observations.iter().enumerate() {
            counters[i] = match q::Counters::new(observation.counters) {
                Ok(counters) => counters,
                Err(_) => {
                    self.fail();
                    return Err(Error::InvalidCounters);
                }
            };
        }
        for i in 0..4 {
            self.transport[i] |= self.publications[i].completed(observations[i].done, counters[i]);
        }
        if self.next == ORDER.len() {
            return Ok(None);
        }
        if let Some(owner) = self.offered {
            return Ok(Some(owner));
        }
        let changed = |i: usize| observations[i].status != self.initial[i];
        let owner = ORDER[self.next];
        let complete = match owner {
            Owner::Closing => self.transport[3] && changed(3),
            Owner::Render => {
                self.transport[1]
                    && self.transport[2]
                    && changed(1)
                    && changed(2)
                    && reports.render_complete
            }
            Owner::Opening => self.transport[0] && changed(0),
        };
        if !complete {
            return Ok(None);
        }
        self.offered = Some(owner);
        Ok(self.offered)
    }
    /// Commit only after the corresponding synchronous finisher has succeeded.
    /// Closing CL owns compute copyback, render owns its scheduler cleanup and
    /// render copyback, and opening CL never triggers another compute copyback.
    pub(crate) fn retire(&mut self, owner: Owner) -> core::result::Result<(), Error> {
        if self.failed {
            return Err(Error::Failed);
        }
        if self.offered != Some(owner) {
            self.fail();
            return Err(Error::Order);
        }
        self.offered = None;
        self.next += 1;
        Ok(())
    }
    pub(crate) fn complete(&self) -> bool {
        !self.failed && self.next == ORDER.len()
    }
    /// No rollback, guessed report acknowledgement, or backing release. The
    /// owning Session remains failed and retains all reachable resources.
    pub(crate) fn fail(&mut self) {
        self.failed = true;
        self.offered = None;
    }
}
