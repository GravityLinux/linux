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
    select_retained(candidates.map(|candidate| (candidate, 0)), fresh, false)
}

/// Keep hot logical queues installed while the caller's backing budget has
/// room. Above that budget, recycle the least recently used retired foreign
/// owner. The budget never reduces concurrency: fresh remains the fallback
/// when every existing owner is busy. A pending cleanup always keeps its owner.
pub(crate) fn select_retained(
    candidates: impl Iterator<Item = (Candidate, u64)>,
    fresh: Option<usize>,
    prefer_fresh: bool,
) -> Option<usize> {
    let selected = candidates.filter_map(|(c, last_used)| {
        let rank = if c.reserved { 0 }
            else if !c.eligible { return None; }
            else if c.matches && c.room { 1 }
            else if c.same_queue && c.idle { 2 }
            else if c.idle { 4 }
            else { return None; };
        // Recency breaks ties only between foreign victims. Exact matches,
        // owned cleanup and own-queue rollover retain their original order.
        Some((rank, if rank == 4 { last_used } else { 0 }, c.index))
    }).min();
    if prefer_fresh && fresh.is_some() && selected.is_none_or(|(rank, _, _)| rank == 4) {
        return fresh;
    }
    selected.map(|(_, _, index)| index).or(fresh)
}

