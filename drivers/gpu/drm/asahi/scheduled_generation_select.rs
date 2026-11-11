// SPDX-License-Identifier: GPL-2.0-only

/// One observation per retained physical generation. The owner supplies
/// identity/lease facts; neither ABI nor engine constants influence selection.
#[derive(Clone, Copy)]
pub(crate) struct Candidate {
    pub(crate) index: usize,
    pub(crate) reserved: bool,
    pub(crate) eligible: bool,
    pub(crate) matches: bool,
    pub(crate) same_queue: bool,
    pub(crate) idle: bool,
    pub(crate) room: bool,
}

/// Original Neo ordering: continue an owned reconfiguration, append to an
/// exact generation, reuse this queue's retired generation, reuse another
/// retired generation, and only then allocate. Iterator order breaks ties.
pub(crate) fn select(
    candidates: impl Iterator<Item = Candidate>,
    fresh: Option<usize>,
) -> Option<usize> {
    candidates.filter_map(|c| {
        let rank = if c.reserved { 0 }
            else if !c.eligible { return None; }
            else if c.matches && c.room { 1 }
            else if c.same_queue && c.idle { 2 }
            else if c.idle { 3 }
            else { return None; };
        Some((rank, c.index))
    }).min_by_key(|&(rank, index)| (rank, index))
        .map(|(_, index)| index).or(fresh)
}

