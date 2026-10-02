// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! g17p_context.py: two retained native CL roots, slot publication, owned
//! fences and slot-three cleanup. Table clones never read code/data leaves.

use super::{
    g17p_compute_runtime::Client,
    g17p_lifecycle::{ContextCleanup, State},
    g17p_memory::Memory,
    g17p_user_vm::UserVm,
};
use kernel::{
    bindings,
    dma_fence::{Fence, RawDmaFence},
    prelude::*,
};
const PAGE: u64 = 0x4000;
const ADDRESS: u64 = 0x000003ffffffc000;
type Owner = (u64, u32);
fn status(fence: &Fence) -> i32 {
    // SAFETY: Each publication retains this live fence reference.
    unsafe { bindings::dma_fence_get_status(fence.raw()) }
}

pub(crate) struct RenderPublication {
    pub(crate) generation: u32,
    pub(crate) fence: Fence,
    // Equivalent to fence.metadata.memory_limit: the report is copied from
    // the exact owned consume-only transaction before the fence is signaled.
    pub(crate) memory_limit: Option<[u8; 0x48]>,
}
pub(crate) struct NativeRenderContext {
    pub(crate) owner: Owner,
    pub(crate) slot: u16,
    pub(crate) root: u64,
    pub(crate) installed: bool,
    pub(crate) quarantined: bool,
    pub(crate) published: KVec<RenderPublication>,
    pub(crate) retained_limits: KVec<RenderPublication>,
}
pub(crate) struct NativeRenderContexts {
    primary: Owner,
    pub(crate) contexts: KVec<NativeRenderContext>,
}
impl NativeRenderContexts {
    pub(crate) fn fail_pending(&mut self, error: Error) {
        for context in &mut self.contexts {
            for publication in &context.published {
                if status(&publication.fence) == 0 {
                    publication.fence.set_error(error);
                    publication.fence.signal();
                }
            }
            if !context.published.is_empty() {
                context.quarantined = true;
            }
        }
    }
    pub(crate) fn new(
        primary: Owner,
        mirror_registered_vm: bool,
        independent_owner: bool,
        reactive_growth: bool,
    ) -> Result<Self> {
        if mirror_registered_vm || !independent_owner || !reactive_growth {
            return Err(EINVAL);
        }
        Ok(Self {
            primary,
            contexts: KVec::with_capacity(2, GFP_KERNEL)?,
        })
    }
    pub(crate) fn reserve(&mut self, client: &Client) -> Result<usize> {
        if let Some(index) = self.contexts.iter().position(|c| c.owner == client.owner) {
            if self.contexts[index].quarantined {
                return Err(EIO);
            }
            return Ok(index);
        }
        if self.contexts.len() == 2 {
            return Err(EBUSY);
        }
        let slot = 1 + self.contexts.len() as u16;
        if slot == 1 && client.owner != self.primary {
            return Err(EINVAL);
        }
        let root = client.root.root();
        if self.contexts.iter().any(|c| c.root == root) {
            return Err(EINVAL);
        }
        let index = self.contexts.len();
        self.contexts.push(
            NativeRenderContext {
                owner: client.owner,
                slot,
                root,
                installed: false,
                quarantined: false,
                published: KVec::new(),
                retained_limits: KVec::new(),
            },
            GFP_KERNEL,
        )?;
        Ok(index)
    }
    pub(crate) fn reap(&mut self, service: &super::g17p_growth_runtime::Service) -> Result {
        for context in &mut self.contexts {
            let mut index = 0;
            while index < context.published.len() {
                let publication = &context.published[index];
                let completed = status(&publication.fence);
                if completed == 0 {
                    index += 1;
                    continue;
                }
                if completed > 0 {
                    context.published.remove(index).map_err(|_| EIO)?;
                    continue;
                }
                let consumed = completed == ENOMEM.to_errno()
                    && service.failed.is_none()
                    && service.pools.iter().any(|p| {
                        p.identity.pool == context.slot as u32 - 1
                            && p.identity.vm == context.slot as u32
                            && p.retired
                            && p.generation == publication.generation
                            && publication.memory_limit.is_some()
                            && p.limit_report == publication.memory_limit
                    });
                if consumed {
                    // Admission may advance; failed fences/backing stay owned.
                    context.retained_limits.reserve(1, GFP_KERNEL)?;
                    let publication = context.published.remove(index).map_err(|_| EIO)?;
                    context.retained_limits.push(publication, GFP_KERNEL)?;
                } else {
                    context.quarantined = true;
                    index += 1;
                }
            }
        }
        Ok(())
    }
    pub(crate) fn require_idle(
        &mut self,
        owner: Owner,
        service: &super::g17p_growth_runtime::Service,
    ) -> Result {
        self.reap(service)?;
        if self
            .contexts
            .iter()
            .any(|c| c.owner == owner && (c.quarantined || !c.published.is_empty()))
        {
            return Err(EBUSY);
        }
        Ok(())
    }
    pub(crate) fn prepare_publication(
        &mut self,
        client: &Client,
        pair: u32,
        high_root: u64,
        memory: &Memory,
        ttbs: u64,
        service: &mut super::g17p_growth_runtime::Service,
    ) -> Result {
        self.require_idle(client.owner, service)?;
        let context = self
            .contexts
            .iter_mut()
            .find(|c| c.owner == client.owner)
            .ok_or(EINVAL)?;
        if pair != context.slot as u32 - 1
            || context.root != client.root.root()
            || high_root == 0
            || high_root & !ADDRESS != 0
        {
            return Err(EINVAL);
        }
        let at = ttbs + context.slot as u64 * 16;
        memory.invalidate(at, 16)?;
        let low = memory.read64(at)?;
        let high = memory.read64(at + 8)?;
        if context.installed || context.slot == 1 {
            if low != ((context.slot as u64) << 48 | context.root | 1)
                || high != ((context.slot as u64) << 48 | high_root | 1)
            {
                return Err(EIO);
            }
        } else if low != 0 || high != 0 {
            return Err(EBUSY);
        }
        let low_word = memory.word64(at)?;
        let high_word = memory.word64(at + 8)?;
        if !context.installed && context.slot == 2 {
            low_word.store((2 << 48) | context.root | 1);
            high_word.store((2 << 48) | high_root | 1);
        }
        UserVm::invalidate(context.slot);
        context.installed = true;
        if let Err(error) = service.bind_pool_root(
            memory,
            ttbs,
            pair,
            &client.root,
            context.slot,
            context.slot as u32,
        ) {
            context.quarantined = true;
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn retain(&mut self, owner: Owner, generation: u32, fence: Fence) -> Result {
        let context = self
            .contexts
            .iter_mut()
            .find(|c| c.owner == owner)
            .ok_or(EINVAL)?;
        if !context.installed
            || context.quarantined
            || context.published.iter().any(|p| p.generation == generation)
        {
            return Err(EIO);
        }
        context.published.push(
            RenderPublication {
                generation,
                fence,
                memory_limit: None,
            },
            GFP_KERNEL,
        )?;
        Ok(())
    }
    pub(crate) fn record_limit(
        &mut self,
        owner: Owner,
        generation: u32,
        service: &super::g17p_growth_runtime::Service,
    ) -> Result {
        let context = self
            .contexts
            .iter_mut()
            .find(|c| c.owner == owner)
            .ok_or(EINVAL)?;
        let pool = service
            .pools
            .iter()
            .find(|p| p.identity.pool == context.slot as u32 - 1)
            .ok_or(EINVAL)?;
        if service.failed.is_some()
            || pool.generation != generation
            || pool.identity.vm != context.slot as u32
        {
            return Err(EIO);
        }
        let report = pool.limit_report.ok_or(EIO)?;
        let publication = context
            .published
            .iter_mut()
            .find(|p| p.generation == generation)
            .ok_or(EINVAL)?;
        publication.memory_limit = Some(report);
        Ok(())
    }
}

pub(crate) struct NativeComputeContext {
    pub(crate) owner: Owner,
    pub(crate) slot: u16,
    pub(crate) root: u64,
    pub(crate) tables: KVec<(u64, u32, u64)>,
    pub(crate) installed: bool,
    pub(crate) published: KVec<(u32, Fence)>,
    pub(crate) quarantined: bool,
    pub(crate) vm_pages: KVec<(u64, u64)>,
    pub(crate) last_completed_fence: Option<Fence>,
    // The initial root is retained by compute::Submission.client. A clone
    // owns table allocations and the caller GEM references it maps.
    owned_root: Option<UserVm>,
    buffers: KVec<kernel::sync::aref::ARef<super::g17p_drm::Object>>,
}
impl NativeComputeContext {
    pub(crate) fn retained_root<'a>(&'a self, template: &'a UserVm) -> Result<&'a UserVm> {
        let root = self.owned_root.as_ref().unwrap_or(template);
        if root.root() != self.root {
            return Err(EIO);
        }
        Ok(root)
    }
    fn low_root<'a>(&'a mut self, template: &'a mut UserVm) -> Result<&'a mut UserVm> {
        let root = if let Some(root) = self.owned_root.as_mut() {
            root
        } else {
            template
        };
        if root.root() != self.root {
            return Err(EIO);
        }
        Ok(root)
    }
}
pub(crate) struct NativeComputeContexts {
    pub(crate) template_root: u64,
    pub(crate) contexts: KVec<NativeComputeContext>,
    pub(crate) retired_slots: KVec<(u16, u64)>,
    pub(crate) retired_contexts: KVec<NativeComputeContext>,
}
impl NativeComputeContexts {
    pub(crate) fn new(template: &UserVm) -> Self {
        Self {
            template_root: template.root(),
            contexts: KVec::new(),
            retired_slots: KVec::new(),
            retired_contexts: KVec::new(),
        }
    }
    pub(crate) fn reserve(
        &mut self,
        owner: Owner,
        template: &UserVm,
        adopt_template: bool,
    ) -> Result<usize> {
        if owner.0 == 0 || owner.1 == 0 || template.root() != self.template_root {
            return Err(EINVAL);
        }
        if let Some(index) = self.contexts.iter().position(|c| c.owner == owner) {
            if self.contexts[index].quarantined {
                return Err(EIO);
            }
            return Ok(index);
        }
        if self.contexts.len() >= 2 {
            return Err(EBUSY);
        }
        let slot = 2 + self.contexts.len() as u16;
        if adopt_template && slot != 2 {
            return Err(EINVAL);
        }
        let (owned_root, tables) = if adopt_template {
            (None, KVec::new())
        } else {
            let (root, tables) = template.clone_low_tables()?;
            (Some(root), tables)
        };
        let root = owned_root.as_ref().map_or(self.template_root, UserVm::root);
        let index = self.contexts.len();
        self.contexts.push(
            NativeComputeContext {
                owner,
                slot,
                root,
                tables,
                installed: false,
                published: KVec::new(),
                quarantined: false,
                vm_pages: KVec::new(),
                last_completed_fence: None,
                owned_root,
                buffers: KVec::new(),
            },
            GFP_KERNEL,
        )?;
        Ok(index)
    }
    pub(crate) fn publish_roots(&mut self, memory: &Memory, ttbs: u64) -> Result {
        let mut pending = KVec::new();
        for (index, context) in self.contexts.iter().enumerate() {
            if context.installed {
                continue;
            }
            if context.quarantined {
                return Err(EIO);
            }
            let address = ttbs + context.slot as u64 * 16;
            let low = memory.read64(address)?;
            if let Some(&(_, retired_high)) = self
                .retired_slots
                .iter()
                .find(|&&(slot, _)| slot == context.slot)
            {
                if low != 0 || memory.read64(address + 8)? != retired_high {
                    return Err(EIO);
                }
            } else if low & 1 == 0 || low & ADDRESS != self.template_root {
                return Err(EIO);
            }
            pending.push((index, memory.word64(address)?), GFP_KERNEL)?;
        }
        // All fallible ownership checks and table clones precede publication.
        for &(index, ref word) in &pending {
            let context = &self.contexts[index];
            word.store((context.slot as u64) << 48 | context.root | 1);
        }
        for &(index, _) in &pending {
            let context = &mut self.contexts[index];
            UserVm::invalidate(context.slot);
            context.installed = true;
            self.retired_slots.retain(|row| row.0 != context.slot);
        }
        Ok(())
    }
    pub(crate) fn retain(&mut self, owner: Owner, generation: u32, fence: Fence) -> Result {
        let context = self
            .contexts
            .iter_mut()
            .find(|c| c.owner == owner)
            .ok_or(ENOENT)?;
        if !context.installed
            || context.quarantined
            || context.published.iter().any(|&(g, _)| g == generation)
        {
            return Err(EINVAL);
        }
        context.published.push((generation, fence), GFP_KERNEL)?;
        Ok(())
    }
    pub(crate) fn reap(&mut self) {
        for context in &mut self.contexts {
            let mut index = 0;
            while index < context.published.len() {
                let result = status(&context.published[index].1);
                if result < 0 {
                    context.quarantined = true;
                    index += 1;
                } else if result == 0 {
                    index += 1;
                } else {
                    context.last_completed_fence = Some(context.published[index].1.clone());
                    // Keep publication ordering for the last-successful fence.
                    let _ = context.published.remove(index);
                }
            }
        }
    }
    pub(crate) fn fail_pending(&mut self, error: Error) {
        for context in &mut self.contexts {
            for (_, fence) in &context.published {
                if status(fence) == 0 {
                    fence.set_error(error);
                    fence.signal();
                }
            }
        }
        self.reap();
    }
    pub(crate) fn bind_vm(
        &mut self,
        client: &Client,
        template: &mut UserVm,
        kernel_start: u64,
        robustness_base: u64,
    ) -> Result<usize> {
        if kernel_start != robustness_base {
            return Err(EINVAL);
        }
        let existing = self.contexts.iter().position(|c| c.owner == client.owner);
        let first = self.contexts.is_empty();
        let index = self.reserve(client.owner, template, first)?;
        let mut inherited = KVec::new();
        if existing.is_none() && !first {
            for (other, previous) in self.contexts.iter().enumerate() {
                if other != index {
                    for &(va, pte) in &previous.vm_pages {
                        inherited.push((va, pte, 0), GFP_KERNEL)?;
                    }
                }
            }
        }
        let context = &mut self.contexts[index];
        if !context.published.is_empty() || context.quarantined {
            return Err(EBUSY);
        }
        if !inherited.is_empty() {
            let slot = context.slot;
            context.low_root(template)?.rebind(&inherited, &[slot])?;
        }
        let mut changes = KVec::new();
        let mut pages = KVec::new();
        let mut buffers = KVec::with_capacity(client.buffers.len(), GFP_KERNEL)?;
        for bo in &client.buffers {
            buffers.push(bo.clone(), GFP_KERNEL)?;
        }
        for &(base, size, _, _) in &client.bindings {
            for offset in (0..size).step_by(PAGE as usize) {
                let va = base + offset;
                let new = client.root.pte(va)?;
                if new == 0 {
                    return Err(EIO);
                }
                let old = context.low_root(template)?.pte(va)?;
                if let Some(&(_, retained)) =
                    context.vm_pages.iter().find(|&&(address, _)| address == va)
                {
                    if retained != new || old != retained {
                        return Err(EIO);
                    }
                } else if old != 0 && old != new {
                    return Err(EBUSY);
                }
                if old != new {
                    changes.push((va, old, new), GFP_KERNEL)?;
                }
                pages.push((va, new), GFP_KERNEL)?;
            }
        }
        // Kernel GEMs are the actual GPU backing: cleaning replaces Python's
        // proxy CPU-image upload while preserving caller cache visibility.
        client.cache(false)?;
        context.vm_pages.reserve(pages.len(), GFP_KERNEL)?;
        context.buffers.reserve(buffers.len(), GFP_KERNEL)?;
        // Pin every new GEM before installing a reachable PTE. Reservations
        // also make the metadata pass infallible after publication.
        for bo in buffers {
            if !context
                .buffers
                .iter()
                .any(|old| core::ptr::eq(&**old, &*bo))
            {
                context.buffers.push(bo, GFP_KERNEL)?;
            }
        }
        let slot = context.slot;
        context.low_root(template)?.rebind(&changes, &[slot])?;
        for row in pages {
            if !context.vm_pages.iter().any(|r| r.0 == row.0) {
                context.vm_pages.push(row, GFP_KERNEL)?;
            }
        }
        Ok(index)
    }
    pub(crate) fn unbind_vm(
        &mut self,
        owner: Owner,
        template: &mut UserVm,
        start: u64,
        size: u64,
        expected: &[(u64, u64)],
    ) -> Result {
        self.reap();
        let Some(context) = self.contexts.iter_mut().find(|c| c.owner == owner) else {
            return Ok(());
        };
        if context.quarantined || !context.published.is_empty() {
            return Err(EBUSY);
        }
        if size == 0 || (start | size) & (PAGE - 1) != 0 {
            return Err(EINVAL);
        }
        let end = start.checked_add(size).ok_or(EINVAL)?;
        let mut changes = KVec::new();
        for &(va, pte) in &context.vm_pages {
            if va < start || va >= end {
                continue;
            }
            if !expected.contains(&(va, pte)) {
                return Err(EIO);
            }
            changes.push((va, pte, 0), GFP_KERNEL)?;
        }
        let slot = context.slot;
        context.low_root(template)?.rebind(&changes, &[slot])?;
        context.vm_pages.retain(|row| row.0 < start || row.0 >= end);
        Ok(())
    }
    pub(crate) fn release_after_cleanup(
        &mut self,
        owner: Owner,
        cleanup: &ContextCleanup,
        memory: &Memory,
        ttbs: u64,
    ) -> Result {
        self.reap();
        let index = self
            .contexts
            .iter()
            .position(|c| c.owner == owner)
            .ok_or(ENOENT)?;
        let context = &self.contexts[index];
        if context.slot != 3
            || context.quarantined
            || !context.published.is_empty()
            || !context.installed
            || cleanup.state != State::Consumed
            || !cleanup.resources.contains(&context.root)
            || !cleanup.resources_retired()
            || !context
                .last_completed_fence
                .as_ref()
                .is_some_and(|last| cleanup.fences.iter().any(|f| f.raw() == last.raw()))
        {
            return Err(EBUSY);
        }
        let mut before = [0; 128];
        for (i, value) in before.iter_mut().enumerate() {
            *value = memory.read64(ttbs + i as u64 * 8)?;
        }
        let references = before
            .iter()
            .enumerate()
            .filter(|&(_, value)| *value & 1 != 0 && *value & ADDRESS == context.root);
        if references.count() != 1 || before[6] & ADDRESS != context.root || before[6] >> 48 != 3 {
            return Err(EIO);
        }
        self.retired_slots.reserve(1, GFP_KERNEL)?;
        self.retired_contexts.reserve(1, GFP_KERNEL)?;
        memory.word64(ttbs + 48)?.store(0);
        UserVm::invalidate(3);
        before[6] = 0;
        for (i, value) in before.iter().enumerate() {
            if memory.read64(ttbs + i as u64 * 8)? != *value {
                self.contexts[index].quarantined = true;
                return Err(EIO);
            }
        }
        self.contexts[index].installed = false;
        self.retired_slots.push((3, before[7]), GFP_KERNEL)?;
        let context = self.contexts.remove(index).map_err(|_| EIO)?;
        self.retired_contexts.push(context, GFP_KERNEL)?;
        Ok(())
    }
}
