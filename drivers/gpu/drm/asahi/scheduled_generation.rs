// SPDX-License-Identifier: GPL-2.0-only

//! Retirement leases shared by ordinary scheduled queue generations.
//!
//! These are host ownership records, not firmware ABI objects. A receipt fence
//! is signalled only after the existing transport/status and CPU visibility
//! checks. A negative fence is terminal, but never authorizes hardware reuse
//! after session failure; the caller still owns that phase/cleanup decision.

use kernel::{
    dma_fence::{Fence, RawDmaFence},
    prelude::*,
};

#[path = "scheduled_generation_select.rs"]
mod selection;
pub(crate) use selection::{select, Candidate};

pub(crate) struct Leases {
    fences: KVec<Fence>,
}

impl Leases {
    pub(crate) fn new() -> Self {
        Self { fences: KVec::new() }
    }

    pub(crate) fn idle(&self) -> bool {
        self.fences.iter().all(|fence| {
            // SAFETY: The vector owns every referenced fence.
            let status = unsafe { kernel::bindings::dma_fence_get_status(fence.raw()) };
            status != 0
        })
    }

    /// Reserve retirement ownership before any producer write or engine arm.
    pub(crate) fn prepare(&mut self) -> Result {
        self.fences.retain(|fence| {
            // SAFETY: The vector retains the reference throughout this query.
            let status = unsafe { kernel::bindings::dma_fence_get_status(fence.raw()) };
            status == 0
        });
        self.fences.reserve(1, GFP_KERNEL)?;
        Ok(())
    }

    /// `prepare` reserved this slot under the same serialized admission lock.
    pub(crate) fn published(&mut self, fence: Fence) -> Result {
        self.fences.push(fence, GFP_KERNEL)?;
        Ok(())
    }

    /// Reserve both a consumer lease and a possible publication by this
    /// same physical generation. The later visible transaction must not
    /// need an allocation after the consumer took the prepared fence slot.
    pub(crate) fn prepare_dependency(&mut self) -> Result {
        self.fences.reserve(2, GFP_KERNEL)?;
        Ok(())
    }

    pub(crate) fn retain_dependency(&mut self, fence: &Fence) -> Result {
        if !self.fences.iter().any(|held| held.raw() == fence.raw()) {
            self.fences.push(fence.clone(), GFP_KERNEL)?;
        }
        Ok(())
    }

    pub(crate) fn fail(&self, error: Error) {
        for fence in &self.fences {
            // SAFETY: The vector owns the reference during error publication.
            if unsafe { kernel::bindings::dma_fence_get_status(fence.raw()) } == 0 {
                fence.set_error(error);
                fence.signal();
            }
        }
    }
}

/// The retained physical owner used by both scheduled backends. Count and
/// backing survive logical queue/client reassignment; only an idle transport
/// window rotates. Consumer leases pin the producer's physical identity.
pub(crate) struct Generation {
    pub(crate) layout: Layout,
    pub(crate) count: u32,
    pub(crate) window: u32,
    pub(crate) leases: Leases,
}
impl Generation {
    pub(crate) fn new(layout: Layout) -> Self {
        Self { layout, count: 0, window: 0, leases: Leases::new() }
    }
    pub(crate) const G16C_COUNT: usize = 126;
    pub(crate) fn g16c(index: usize) -> Result<Self> {
        if index >= Self::G16C_COUNT { return Err(EBUSY); }
        let mut layout=Layout::g16c(index)?;
        layout.grid=(index+2) as u8;
        Ok(Self::new(layout))
    }
    pub(crate) fn idle(&self) -> bool { self.leases.idle() }
}

// Host placement policy, independent of firmware structure layout. The
// original Neo geometry provides disjoint per-command allocations. Max uses
// the same geometry with its established context-zero descriptor alias.
const FIRST_ASID: u16 = 4;
const CONTEXTS: usize = 64 - FIRST_ASID as usize;
const FIRST_GRID: u8 = 32;
const FW_BASE: u64 = 0xfffffc20e0000000;
const LOW_BASE: u64 = 0x7200000000;
const STRIDE: u64 = 0x200000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) high: u64,
    pub(crate) low: u64,
    pub(crate) grid: u8,
    pub(crate) asid: u16,
}
impl Layout {
    pub(crate) fn new(index: usize, asid: u16) -> Result<Self> {
        if index >= CONTEXTS {
            return Err(EBUSY);
        }
        Ok(Self {
            high: FW_BASE + index as u64 * STRIDE,
            low: LOW_BASE + index as u64 * STRIDE,
            grid: FIRST_GRID + index as u8,
            asid,
        })
    }
    pub(crate) fn g16c(index: usize) -> Result<Self> {
        // QueueSet's own retained pool fits in this reserved address range;
        // do not inherit Neo's ASID count as a Max API queue-count limit.
        if index >= 256 { return Err(EBUSY); }
        let high = FW_BASE + index as u64 * STRIDE;
        Ok(Self { high, low: 0x7000000000 + high - 0xfffffc20c0018000,
            grid: 0, asid: 0 })
    }
    pub(crate) fn queue(self) -> u64 {
        self.high + 0x100000
    }
    pub(crate) fn pointers(self) -> u64 {
        self.high + 0x104000
    }
    pub(crate) fn ring(self) -> u64 {
        self.high + 0x108000
    }
    pub(crate) fn context(self) -> u64 {
        self.high + 0x110000
    }
    pub(crate) fn context_low(self) -> u64 {
        self.low + 0x110000
    }
    pub(crate) fn support(self) -> u64 {
        self.high + 0x140000
    }
    pub(crate) fn support_state(self) -> u64 {
        self.high + 0x144000
    }
    pub(crate) fn zero(self) -> u64 {
        self.high + 0x148000
    }
    pub(crate) fn completion_scratch(self, slot: u32) -> u64 {
        self.zero() + slot as u64 * 0x40
    }
    pub(crate) fn control(self) -> u64 {
        self.high + 0x14c000
    }
    pub(crate) fn job_list(self) -> u64 {
        self.high + 0x150000
    }
    pub(crate) fn optional(self) -> u64 {
        self.high + 0x154000
    }
    pub(crate) fn scheduler(self, slot: u32) -> u64 {
        self.high + 0x160000 + slot as u64 * 0x100
    }
    pub(crate) fn scheduler_slot(self, slot: u32) -> u64 {
        self.high + 0x170000 + slot as u64 * 64
    }
    pub(crate) fn event(self, slot: u32) -> u64 {
        self.high + 0x1a0000 + slot as u64 * 0x400
    }
    pub(crate) fn status(self, slot: u32) -> u64 {
        self.high + 0x178000 + slot as u64 * 64
    }
    pub(crate) fn dispatch(self, slot: u32) -> [u64; 2] {
        [
            self.high + 0x17c000 + slot as u64 * 64,
            self.high + 0x180000 + slot as u64 * 64,
        ]
    }
    pub(crate) fn descriptor(self, slot: u32) -> u64 {
        self.high + slot as u64 * 0x1040
    }
    pub(crate) fn descriptor_low(self, slot: u32) -> u64 {
        self.low + slot as u64 * 0x1040
    }
}
