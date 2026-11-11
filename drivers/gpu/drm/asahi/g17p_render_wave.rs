// SPDX-License-Identifier: GPL-2.0-only

//! Retained render publications. Firmware-specific tickets supply completion
//! checks; one wave owns independent retirement and deferred CPU visibility.

use super::{Completion, Image, RenderPending, RenderReceipt, Session};
use kernel::{dma_fence::{Fence, RawDmaFence}, prelude::*, sync::Arc};

pub(super) trait RetainedFrame {
    type Cache;
    fn receipt(&self) -> &Arc<RenderReceipt>;
    fn fence(&self) -> &Fence;
    fn pool(&self) -> u32;
    fn storage(&self) -> u32;
    fn control_done(&self) -> bool { false }
    fn poll(&mut self, session: &mut Session, dev: &kernel::device::Device,
        image: Option<&Image>, sole: bool) -> Result<bool>;
    fn cache_seed(&self, session: &Session) -> Result<Self::Cache>;
    fn release(&mut self, _session: &mut Session) -> Result { Ok(()) }
    fn completion(self, cache: Self::Cache) -> Completion;
}

pub(super) struct RenderWave<T = RenderPending> {
    pub(super) frames: KVec<T>,
    pub(super) next: usize,
}

impl<T: RetainedFrame> RenderWave<T> {
    /// RX must resolve to one retained publication before any ACK or reply.
    /// A pool-only report cannot choose arbitrarily between queued owners.
    pub(super) fn find_owner(frames:&[T],matches:impl Fn(&T)->bool)->Result<usize> {
        let mut owners=frames.iter().enumerate().filter(|(_,frame)|matches(frame));
        let (index,_)=owners.next().ok_or(EIO)?;
        if owners.next().is_some() {return Err(EIO);}
        Ok(index)
    }
    pub(super) fn pool_busy(&self,pool:u32)->bool {
        self.frames[self.next..].iter().any(|frame|frame.pool()==pool)
    }
    pub(super) fn next_storage(&self,next:u32,conflicts:impl Fn(u32,u32)->bool)->Option<u32> {
        self.next_storage_available(next,conflicts,|_|true)
    }
    pub(super) fn next_storage_available(&self,next:u32,conflicts:impl Fn(u32,u32)->bool,
        available:impl Fn(u32)->bool)->Option<u32> {
        super::super::g17p_render_lifecycle::select_storage(next,|slot|available(*slot)
            && self.frames[self.next..].iter().all(|frame|!conflicts(*slot,frame.storage())))
    }
    pub(super) fn reserve(&mut self) -> Result { Ok(self.frames.reserve(1, GFP_KERNEL)?) }

    pub(super) fn fail(&self, error: Error) {
        for state in &self.frames[self.next..] {
            state.receipt().fail(error);
            if unsafe { kernel::bindings::dma_fence_get_status(state.fence().raw()) } == 0 {
                state.fence().set_error(error);
                state.fence().signal();
            }
        }
    }

    pub(super) fn poll(&mut self, session: &mut Session, dev: &kernel::device::Device,
        image: Option<&Image>) -> Result<bool> {
        if self.next != 0 {
            drop(self.frames.drain(..self.next));
            self.next = 0;
        }
        // A blocked scene does not retain another pool's credit. This is the
        // ordinary Neo retirement order, including the fresh control recheck.
        let mut index = 0;
        while index < self.frames.len() {
            let sole = self.frames.len() == 1;
            let state = &mut self.frames[index];
            let control_before = state.control_done();
            let mut done = state.poll(session, dev, image, sole)?;
            if !done && !control_before && state.control_done() {
                done = state.poll(session, dev, image, sole)?;
            }
            if done {
                let cache = if session.defer_cache {
                    session.completions.reserve(1, GFP_KERNEL)?;
                    session.render_cpu_leases.reserve(1, GFP_KERNEL)?;
                    Some(state.cache_seed(session)?)
                } else { None };
                state.release(session)?;
                let state = self.frames.remove(index).map_err(|_| EIO)?;
                if let Some(cache) = cache {
                    // GPU retirement does not permit pool reuse while CPU
                    // visibility or a dependent consumer still owns its fence.
                    session.render_cpu_leases.push((state.pool(), state.fence().clone()), GFP_KERNEL)?;
                    session.completions.push(state.completion(cache), GFP_KERNEL)?;
                } else { state.fence().signal(); }
            } else {
                index += 1;
            }
        }
        Ok(self.frames.is_empty())
    }
}

impl RetainedFrame for RenderPending {
    type Cache = super::super::g17p_timestamp::Cache;
    fn receipt(&self) -> &Arc<RenderReceipt> { &self.receipt }
    fn fence(&self) -> &Fence { &self.gate }
    fn pool(&self) -> u32 { self.ticket.item.layout.pair }
    fn storage(&self) -> u32 { self.ticket.item.storage_ordinal() }
    fn control_done(&self) -> bool { self.control_done }
    fn poll(&mut self, session: &mut Session, dev: &kernel::device::Device,
        image: Option<&Image>, sole: bool) -> Result<bool> {
        session.poll_render(dev, image.ok_or(EIO)?, self, sole)
    }
    fn cache_seed(&self, session: &Session) -> Result<Self::Cache> {
        session.timestamps.as_ref().ok_or(EIO)?.cache_seed(&self.ticket.timestamps)
    }
    fn completion(self, stamps: Self::Cache) -> Completion {
        Completion { client: self.ticket.client, stamps, fence: self.gate,
            grid: self.ticket.item.layout.grids[1] as u8, error: None }
    }
}
