// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! G17PShimBackend.begin_compute_cleanup / begin_compute_maintenance.
//! Receipts remain Session-owned, including ambiguous publication failures.

use super::super::{
    g17p_abi::Channel,
    g17p_lifecycle::{ContextCleanup, Host, PostCleanupMaintenance, Publication, State},
    g17p_memory::{self, Memory},
    g17p_vm::Vm,
};
use super::{compute, Image, Peer, Phase, Session};
use core::sync::atomic::Ordering;
use kernel::{dma_fence::Fence, prelude::*};

pub(super) struct Receipts {
    last: Option<Fence>,
    failed: KVec<Fence>,
    cleanups: KVec<ContextCleanup>,
    maintenance: KVec<PostCleanupMaintenance>,
}
impl Receipts {
    pub(super) fn cleanup_receipt(&self, index: usize) -> Result<&ContextCleanup> {
        self.cleanups.get(index).ok_or(EINVAL)
    }
    pub(super) fn new() -> Self {
        Self {
            last: None,
            failed: KVec::new(),
            cleanups: KVec::new(),
            maintenance: KVec::new(),
        }
    }
    pub(super) fn require_idle(&self) -> Result {
        if self.cleanups.iter().any(|r| r.state != State::Consumed)
            || self.maintenance.iter().any(|r| r.state != State::Consumed)
        {
            return Err(EBUSY);
        }
        Ok(())
    }
}
struct Live<'a> {
    memory: &'a mut Memory,
    vm: &'a Vm,
    peers: &'a mut [Peer],
    channel: Channel,
}
impl Live<'_> {
    fn healthy(&self) -> Result {
        if self.peers.len() != 2
            || self
                .peers
                .iter()
                .any(|p| !p.started || p.rtkit.is_none() || p.data.crashed.load(Ordering::Acquire))
        {
            return Err(EIO);
        }
        Ok(())
    }
}
impl Host for Live<'_> {
    fn read_control(&mut self, control: u64) -> Result<[u8; 0x40]> {
        self.healthy()?;
        let mut body = [0; 0x40];
        for (i, word) in body.chunks_exact_mut(8).enumerate() {
            let pa = self.vm.physical(self.memory, 2, control + i as u64 * 8)?;
            self.memory.invalidate(pa, 8)?;
            word.copy_from_slice(&self.memory.read64(pa)?.to_le_bytes());
        }
        Ok(body)
    }
    fn counters(&mut self) -> Result<[u32; 3]> {
        self.healthy()?;
        let mut values = [0; 3];
        for (value, va) in values.iter_mut().zip(self.channel.states) {
            *value = self
                .memory
                .read_firmware32(self.vm.physical(self.memory, 2, va)?)?;
        }
        Ok(values)
    }
    fn publish(&mut self, body: &[u8; 0x40]) -> Result<Publication> {
        let before = self.counters()?;
        if before.iter().any(|&v| v >= 256)
            || before[2] == 255
            || before[..2].iter().any(|&v| v > before[2])
        {
            return Err(EBUSY);
        }
        let target = before[2] + 1;
        self.vm.write(
            self.memory,
            2,
            self.channel.ring + before[2] as u64 * 0x40,
            body,
        )?;
        g17p_memory::sync();
        self.vm.write(
            self.memory,
            2,
            self.channel.states[2],
            &target.to_le_bytes(),
        )?;
        g17p_memory::sync();
        Ok(Publication { before, target })
    }
    fn notify(&mut self) -> Result {
        self.healthy()?;
        self.peers[0]
            .rtkit
            .as_mut()
            .ok_or(EIO)?
            .as_mut()
            .send_message(0x21, 0x0084000000000011)
    }
}
impl Session {
    pub(crate) fn destroy_vm(&mut self, owner: (u64, u32), image: &Image) -> Result {
        if self.phase == Phase::Failed {
            return Ok(());
        }
        self.independent_compute.require_owner_idle(owner)?;
        self.restore_owned_render_fault()?;
        if *crate::module_parameters::cleanup_diagnostics.value() & !15 != 0 {
            return Err(EINVAL);
        }
        if let Some(context) = self.compute_contexts.as_ref().and_then(|contexts| {
            contexts
                .contexts
                .iter()
                .find(|context| context.owner == owner)
        }) {
            // Source release_after_cleanup only qualifies slot three. Reject
            // the retained primary slot before publishing any cleanup command.
            if context.slot != 3 {
                return Err(EBUSY);
            }
            let cleanup = self.run_compute_cleanup(image)?;
            self.run_compute_maintenance(image, cleanup)?;
            self.release_native_compute_context(owner, cleanup)?;
        } else if *crate::module_parameters::cleanup_diagnostics.value() != 0
            && self
                .compute
                .as_ref()
                .is_some_and(|work| work.client.owner == owner)
        {
            let cleanup = self.run_compute_cleanup(image)?;
            self.run_compute_maintenance(image, cleanup)?;
            self.run_cleanup_diagnostics(cleanup)?;
        }
        // Logical render close does not prove that firmware released the
        // installed context. Current and inactive root owners stay retained.
        Ok(())
    }

    pub(crate) fn begin_submission(&mut self, fence: &Fence, timestamps: &[u64]) -> Result {
        self.cleanup.require_idle()?;
        self.cleanup.failed.reserve(
            self.submissions.checked_add(1).ok_or(EOVERFLOW)?,
            GFP_KERNEL,
        )?;
        self.timestamps
            .as_mut()
            .ok_or(EINVAL)?
            .retain(timestamps, fence)?;
        self.submission_error = None;
        self.submissions += 1;
        Ok(())
    }
    pub(crate) fn submission_error(&self) -> Option<kernel::error::Error> {
        self.submission_error
    }
    pub(crate) fn finish_submission(&mut self, result: Result) -> Result<Option<Error>> {
        if let Err(error) = result {
            if self.phase != Phase::Failed {
                return Err(error);
            }
            self.submission_error = Some(error);
            if let Some(timestamps) = self.timestamps.as_mut() {
                timestamps.fail_pending(error);
            }
        }
        Ok(self.submission_error)
    }
    /// A pure render job has exact owned ticket fences. Nonfatal resource
    /// limits belong to those fences, not another joined job's global flag.
    pub(crate) fn finish_owned_render_submission(&mut self, result: Result) -> Result<Option<Error>> {
        if self.phase == Phase::Failed { return self.finish_submission(result); }
        result.map(|()| None)
    }
    pub(crate) fn remember_owned_render_completed(&mut self, fence: Fence, failed: bool) {
        self.submissions = self.submissions.saturating_sub(1);
        if failed {
            self.cleanup.failed.push(fence, GFP_KERNEL).expect("reserved fence slot");
        } else {
            self.cleanup.last = Some(fence);
        }
    }
    pub(crate) fn remember_completed(&mut self, fence: Fence) {
        self.submissions = self.submissions.saturating_sub(1);
        if self.submission_error.is_some() {
            // begin_submission reserves this reference before publication.
            self.cleanup
                .failed
                .push(fence, GFP_KERNEL)
                .expect("reserved fence slot");
        } else {
            self.cleanup.last = Some(fence);
        }
    }
    pub(crate) fn begin_compute_cleanup(&mut self) -> Result<usize> {
        if self.phase != Phase::Running {
            return Err(EIO);
        }
        self.cleanup.require_idle()?;
        let work = self.compute.as_ref().ok_or(EINVAL)?;
        if work.after_render || work.has_transport_pool() {
            return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
        }
        if self.native.as_ref().is_some_and(|r| !r.complete()) {
            return Err(EBUSY);
        }
        compute::idle(
            self.memory.as_ref().ok_or(EINVAL)?,
            self.vm.as_ref().ok_or(EINVAL)?,
            work,
        )?;
        let mut fences = KVec::with_capacity(1, GFP_KERNEL)?;
        fences.push(self.cleanup.last.as_ref().ok_or(EBUSY)?.clone(), GFP_KERNEL)?;
        if let Some(contexts) = self.compute_contexts.as_mut() {
            contexts.reap();
            for context in &contexts.contexts {
                if context.quarantined || !context.published.is_empty() {
                    return Err(EBUSY);
                }
                fences.push(
                    context.last_completed_fence.as_ref().ok_or(EBUSY)?.clone(),
                    GFP_KERNEL,
                )?;
            }
        }
        let mut resources = KVec::new();
        for address in [
            0xfffffc20c07b8040,
            0xfffffc20c0000300,
            0xfffffc20c0358000,
            0x7000340000,
            0xfffffc2000278000,
            0x70004d8000,
            work.client.root.root(),
            0xfffffc20c0000000,
            0xfffffc20c07b8000,
            0xfffffc2001658000,
            0xfffffc20c08a8000,
        ] {
            resources.push(address, GFP_KERNEL)?;
        }
        if let Some(contexts) = self.compute_contexts.as_ref() {
            for context in &contexts.contexts {
                resources.push(context.root, GFP_KERNEL)?;
            }
        }
        let receipt = ContextCleanup::new(0xfffffc20c07b8040, fences, resources)?;
        if !receipt.resources_retired() {
            return Err(EBUSY);
        }
        let index = self.cleanup.cleanups.len();
        self.cleanup.cleanups.push(receipt, GFP_KERNEL)?;
        Ok(index)
    }
    pub(crate) fn begin_compute_maintenance(&mut self, cleanup: usize) -> Result<usize> {
        if self.phase != Phase::Running || cleanup + 1 != self.cleanup.cleanups.len() {
            return Err(EINVAL);
        }
        self.cleanup.require_idle()?;
        let receipt = self.cleanup.cleanups.get(cleanup).ok_or(EINVAL)?;
        use kernel::dma_fence::RawDmaFence;
        if !receipt.fences.iter().any(|f| {
            self.cleanup
                .last
                .as_ref()
                .is_some_and(|last| f.raw() == last.raw())
        }) {
            return Err(EBUSY);
        }
        let maintenance = PostCleanupMaintenance::new(receipt)?;
        let index = self.cleanup.maintenance.len();
        self.cleanup.maintenance.push(maintenance, GFP_KERNEL)?;
        Ok(index)
    }
    pub(crate) fn run_compute_cleanup(&mut self, image: &Image) -> Result<usize> {
        let index = self.begin_compute_cleanup()?;
        let result = (|| {
            for _ in 0..200 {
                if self.step_compute_cleanup(image, index)? {
                    return Ok(index);
                }
                kernel::time::delay::fsleep(kernel::time::Delta::from_millis(10));
            }
            Err(ETIMEDOUT)
        })();
        if let Err(error) = result {
            self.cleanup.cleanups[index].failure = Some(error);
            self.cleanup.cleanups[index].state = State::Quarantined;
            self.phase = Phase::Failed;
        }
        result
    }
    pub(crate) fn run_compute_maintenance(&mut self, image: &Image, cleanup: usize) -> Result {
        let index = self.begin_compute_maintenance(cleanup)?;
        let result = (|| {
            for _ in 0..200 {
                if self.step_compute_maintenance(image, index)? {
                    return Ok(());
                }
                kernel::time::delay::fsleep(kernel::time::Delta::from_millis(10));
            }
            Err(ETIMEDOUT)
        })();
        if let Err(error) = result {
            self.cleanup.maintenance[index].failure = Some(error);
            self.cleanup.maintenance[index].state = State::Quarantined;
            self.phase = Phase::Failed;
        }
        result
    }
    pub(crate) fn step_compute_cleanup(&mut self, image: &Image, index: usize) -> Result<bool> {
        let mut host = Live {
            memory: self.memory.as_mut().ok_or(EINVAL)?,
            vm: self.vm.as_ref().ok_or(EINVAL)?,
            peers: &mut self.peers,
            channel: image.graph.channels[0][12],
        };
        self.cleanup
            .cleanups
            .get_mut(index)
            .ok_or(EINVAL)?
            .step(&mut host)
    }
    pub(crate) fn release_native_compute_context(
        &mut self,
        owner: (u64, u32),
        cleanup: usize,
    ) -> Result {
        if self.phase != Phase::Running {
            return Err(EIO);
        }
        self.cleanup.require_idle()?;
        self.compute_contexts
            .as_mut()
            .ok_or(EINVAL)?
            .release_after_cleanup(
                owner,
                self.cleanup.cleanups.get(cleanup).ok_or(EINVAL)?,
                self.memory.as_ref().ok_or(EINVAL)?,
                self.ttbs,
            )
    }
    pub(crate) fn step_compute_maintenance(&mut self, image: &Image, index: usize) -> Result<bool> {
        let mut host = Live {
            memory: self.memory.as_mut().ok_or(EINVAL)?,
            vm: self.vm.as_ref().ok_or(EINVAL)?,
            peers: &mut self.peers,
            channel: image.graph.channels[0][12],
        };
        self.cleanup
            .maintenance
            .get_mut(index)
            .ok_or(EINVAL)?
            .step(&mut host)
    }
}
