// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Kernel ownership adapters for the source native render execution contexts.
//! Each logical VM keeps its low root; transport/pool selection follows its
//! reserved slot, with no eviction or reassignment of another installed root.

use super::super::{g17p_context::NativeRenderContexts, g17p_render::Parameters};
use super::{compute, render, Phase, Session};
use kernel::{dma_fence::RawDmaFence, prelude::*};

// The caller has passed render::validate_client, including private/growth
// guards. A template fallback is not a GEM binding: first-render build skips
// it when a caller owns that VA, and subsequent VM admission must do likewise.
fn retire_render_fallbacks(
    previous: &mut compute::Client,
    caller: &compute::Client,
    memory: &super::super::g17p_memory::Memory,
    vm: &super::super::g17p_vm::Vm,
) -> Result {
    retire_render_fallbacks_slot(previous, caller, memory, vm, 1, None)
}
fn retire_render_fallbacks_slot(previous: &mut compute::Client, caller: &compute::Client,
    memory: &super::super::g17p_memory::Memory, vm: &super::super::g17p_vm::Vm, slot: u16,
    foreign: Option<(&super::super::g17p_growth_runtime::Service, u32)>,
) -> Result {
    const PAGE: u64 = 0x4000;
    let address = |base: u64| if base < 0x1000000000 { base + 0x1000000000 } else { base };
    let mut changes = KVec::new();
    for &(base, size, _, _) in &caller.bindings {
        for offset in (0..size).step_by(PAGE as usize) {
            let va = address(base) + offset;
            if previous.bindings.iter().any(|&(old, length, _, _)| {
                let old = address(old);
                old <= va && va < old + length
            }) {
                continue;
            }
            let old = previous.root.pte(va)?;
            if old == 0 {
                continue;
            }
            // Only exact borrowed Source leaves or another pool's tracked
            // growth may be removed. This root is retired; neither the
            // foreign pool's live root nor its physical ownership changes.
            let fallback = super::super::g17p_topology::RENDER_RUNS.iter().any(|&(first, count, _)| {
                first <= va && va < first + count as u64 * PAGE
            }) && old & 0x000003ffffffc000 == vm.physical(memory, 1, va)?;
            let foreign_growth = foreign.is_some_and(|(service, selected)|
                service.foreign_growth_leaf(selected, va, old));
            if !fallback && !foreign_growth { continue; }
            changes.push((va, old, 0), GFP_KERNEL)?;
        }
    }
    if !changes.is_empty() {
        // Admission holds the exclusive, retired render owner. Original
        // backing stays Memory-owned; no physical page is returned here.
        previous.root.rebind(&changes, &[slot])?;
    }
    Ok(())
}

/// Remove only exact retained robustness aliases, and install them in the
/// incoming logical owner. All root plans precede the first live PTE change.
pub(super) fn prepare_robustness_handoff<'a>(
    client: &'a mut compute::Client,
    old: &[(u64, u64); 2],
    new: &[(u64, u64); 2],
    incoming: (u64, u32),
) -> Result<Option<super::super::g17p_user_vm::RebindPlan<'a>>> {
    let mut changes = KVec::with_capacity(4, GFP_KERNEL)?;
    for &(address, expected) in old {
        let before = client.root.pte(address)?;
        if before != expected { continue; }
        if client.bindings.iter().any(|&(va, size, _, _)| va <= address && address < va + size) {
            return Err(EIO);
        }
        changes.push((address, expected, 0), GFP_KERNEL)?;
    }
    if client.owner == incoming {
        for &(address, expected) in new {
            if client.bindings.iter().any(|&(va, size, _, _)| va <= address && address < va + size) {
                return Err(EBUSY);
            }
            let before = client.root.pte(address)?;
            if let Some(row) = changes.iter_mut().find(|r| r.0 == address) {
                if row.1 != before { return Err(EIO); }
                row.2 = expected;
            } else if before != expected {
                if before != 0 { return Err(EBUSY); }
                changes.push((address, 0, expected), GFP_KERNEL)?;
            }
        }
    }
    if changes.is_empty() { return Ok(None); }
    Ok(Some(client.root.prepare_rebind(&changes, &[1])?))
}

impl Session {
    /// Source _ensure_compute_robustness also installs reserved state in the
    /// compute caller's logical root. Keep those pages distinct from its BOs
    /// and retain the same physical owner across later render/compute returns.
    pub(super) fn mirror_compute_robustness(&mut self) -> Result {
        let work = self.compute.as_ref().ok_or(EINVAL)?;
        let owner = work.client.owner;
        let aliases = work.robustness_aliases()?;
        let client = if self.render.as_ref().is_some_and(|r| r.client.owner == owner) {
            Some(&mut self.render.as_mut().ok_or(EINVAL)?.client)
        } else if self.dormant_render.as_ref().is_some_and(|r| r.client.owner == owner) {
            Some(&mut self.dormant_render.as_mut().ok_or(EINVAL)?.client)
        } else {
            self.render_clients.iter_mut().find(|c| c.owner == owner)
        };
        let Some(client) = client else { return Ok(()); };
        let mut changes = KVec::with_capacity(2, GFP_KERNEL)?;
        for (address, pte) in aliases {
            let old = client.root.pte(address)?;
            if old == pte {
                continue;
            }
            if old != 0 {
                return Err(EBUSY);
            }
            changes.push((address, 0, pte), GFP_KERNEL)?;
        }
        if !changes.is_empty() {
            client.root.rebind(&changes, &[1])?;
        }
        Ok(())
    }

    /// Source _create_execution_context / _switch_registered_context_root:
    /// retain independent caller tables and select the admitted ASID-one root
    /// only after the previous synchronous render has retired.
    pub(crate) fn prepare_logical_render_context(
        &mut self,
        dev: &kernel::device::Device,
        client: compute::Client,
        p: &Parameters,
    ) -> Result {
        render::validate_client(&client, p)?;
        self.select_logical_execution_context(dev, client.owner, Some(client))
    }

    /// Source activate_execution_context selects the logical root separately
    /// from _ensure_render_vm. Compute-only calls do not install their BOs in
    /// the render root; a cloned context excludes the previous caller's BOs.
    pub(crate) fn select_logical_execution_context(
        &mut self,
        dev: &kernel::device::Device,
        owner: (u64, u32),
        client: Option<compute::Client>,
    ) -> Result {
        if self.native.is_some() || self.render_contexts.is_some() {
            return Err(EINVAL);
        }
        let work = self.render.as_mut().ok_or(EINVAL)?;
        if owner == work.client.owner {
            if let Some(client) = client {
                retire_render_fallbacks(&mut work.client, &client,
                    self.memory.as_ref().ok_or(EINVAL)?, self.vm.as_ref().ok_or(EINVAL)?)?;
                work.client.rebind(client, true)?;
            }
            // Source activate_execution_context republishes both registered
            // words and ASID1 even when the logical owner is unchanged.
            // The adopted upper root is separate from the firmware DATA root.
            let memory = self.memory.as_ref().ok_or(EINVAL)?;
            let low = memory.word64(self.ttbs + 16)?;
            let high = memory.word64(self.ttbs + 24)?;
            let selected = (1 << 48) | work.client.root.root() | 1;
            let upper = high.load();
            if low.load() != selected || upper & 1 == 0 || upper >> 48 != 1 {
                return Err(EIO);
            }
            low.store(selected);
            high.store(upper);
            super::super::g17p_user_vm::UserVm::invalidate(1);
            return Ok(());
        }
        render::quiesce(
            self.memory.as_mut().ok_or(EINVAL)?,
            self.vm.as_ref().ok_or(EINVAL)?,
            work,
        )?;
        self.render_clients.reserve(1, GFP_KERNEL)?;
        let previous_owner = work.client.owner;
        let cloned = !self.render_clients.iter().any(|c| c.owner == owner);
        let next = if let Some(index) = self.render_clients.iter().position(|c| c.owner == owner) {
            if let Some(client) = client {
                retire_render_fallbacks(&mut self.render_clients[index], &client,
                    self.memory.as_ref().ok_or(EINVAL)?, self.vm.as_ref().ok_or(EINVAL)?)?;
                self.render_clients[index].rebind_contexts(client, true, &[1])?;
            }
            work.growth.as_ref().ok_or(EINVAL)?
                .mirror_retained_mappings(&mut self.render_clients[index].root)?;
            self.render_clients.remove(index).map_err(|_| EIO)?
        } else {
            // Source _ensure_render_vm carries the current driver-private
            // graph into each logical root. Build a selected second owner
            // before cloning so its index/status/context aliases survive a
            // later return to either caller. Creating it only after the
            // handoff left the first root naming the old operand backing.
            if *crate::module_parameters::partial_independent_owner.value() == 1
                && *crate::module_parameters::alternate_queue_pairs.value() == 1
            {
                work.create_render_pools(
                    self.memory.as_mut().ok_or(EINVAL)?,
                    self.vm.as_mut().ok_or(EINVAL)?,
                )?;
            }
            let (root, _tables) = work.client.root.clone_low_tables()?;
            let mut buffers = KVec::with_capacity(work.client.buffers.len(), GFP_KERNEL)?;
            for bo in &work.client.buffers {
                buffers.push(bo.clone(), GFP_KERNEL)?;
            }
            let mut bindings = KVec::with_capacity(work.client.bindings.len(), GFP_KERNEL)?;
            bindings.extend_from_slice(&work.client.bindings, GFP_KERNEL)?;
            let mut next = compute::Client {
                root,
                buffers,
                bindings,
                owner,
                cpu_maps: crate::g17p_compute_runtime::CpuMaps::new()?,
                primer_aliases: None,
            };
            let caller = match client {
                Some(client) => client,
                None => compute::Client {
                    root: super::super::g17p_user_vm::UserVm::new()?,
                    buffers: KVec::new(),
                    bindings: KVec::new(),
                    owner,
                    cpu_maps: crate::g17p_compute_runtime::CpuMaps::new()?,
                    primer_aliases: None,
                },
            };
            retire_render_fallbacks(&mut next, &caller,
                self.memory.as_ref().ok_or(EINVAL)?, self.vm.as_ref().ok_or(EINVAL)?)?;
            next.rebind_contexts(caller, true, &[1])?;
            next
        };
        let memory = self.memory.as_ref().ok_or(EINVAL)?;
        memory.invalidate(self.ttbs + 16, 16)?;
        let low = memory.word64(self.ttbs + 16)?;
        let high = memory.word64(self.ttbs + 24)?;
        let upper = high.load();
        if low.load() != ((1 << 48) | work.client.root.root() | 1)
            || upper & 1 == 0 || upper >> 48 != 1
        {
            return Err(EIO);
        }
        // Retain both roots before a publication that later admission can
        // reject. No reachable owner is dropped on an ambiguous failure.
        let previous = core::mem::replace(&mut work.client, next);
        self.render_clients.push(previous, GFP_KERNEL)?;
        if cloned {
            // Source G17PAddressSpace.clone_for_context publishes its cloned
            // tables with VMALLE1OS before selecting the registered root.
            // The registered ASID-one publication below remains separate.
            super::super::g17p_vm::Vm::invalidate_gpu();
        }
        low.store((1 << 48) | work.client.root.root() | 1);
        // clone_for_context inherits the adopted upper tree. Ordinary
        // first_render installed an empty tree; firmware_high_root is an
        // independent DATA view and must not replace that client upper root.
        high.store(upper);
        super::super::g17p_user_vm::UserVm::invalidate(1);
        let result: Result = (|| {
            let service = work.growth.as_mut().ok_or(EINVAL)?;
            for pool in 0..service.pools.len() {
                service.bind_pool_root(memory, self.ttbs, pool as u32, &work.client.root, 1, 1)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result?;
        dev_info!(dev, "G17P: logical render VM handoff {:?} -> {:?}, root {:#x}\n",
                  previous_owner, owner, work.client.root.root());
        Ok(())
    }
    pub(super) fn prepare_native_render_context(
        &mut self,
        dev: &kernel::device::Device,
        replacement: Option<compute::Client>,
        p: &Parameters,
    ) -> Result {
        if self.phase != Phase::Running
            || self.native.is_some()
            || self.compute.is_some()
            || *crate::module_parameters::partial_independent_owner.value() != 1
        {
            return Err(Error::from_errno(-(kernel::bindings::EOPNOTSUPP as i32)));
        }
        self.cleanup.require_idle()?;
        let work = self.render.as_mut().ok_or(EINVAL)?;
        let contexts = self.render_contexts.as_mut().ok_or(EINVAL)?;
        let owner = replacement.as_ref().map_or(work.client.owner, |c| c.owner);
        contexts.require_idle(owner, work.growth.as_ref().ok_or(EINVAL)?)?;
        if owner != work.client.owner {
            if !contexts.contexts.iter().any(|c| c.owner == owner) && contexts.contexts.len() == 2 {
                return Err(EBUSY);
            }
            let client = replacement.ok_or(EINVAL)?;
            render::validate_client(&client, p)?;
            self.render_clients.reserve(1, GFP_KERNEL)?;
            // Creation precedes the second owner's clone, so every shared
            // driver-private/TVB mapping has the retained physical owner.
            work.create_second_pair(
                self.memory.as_mut().ok_or(EINVAL)?,
                self.vm.as_mut().ok_or(EINVAL)?,
            ).inspect_err(|error| {
                dev_err!(dev, "G17P: native render second transport preparation failed: {:?}\n", error);
            })?;
            let next =
                if let Some(index) = self.render_clients.iter().position(|c| c.owner == owner) {
                    // Keep the installed root owned throughout fallible refresh.
                    let slot = contexts
                        .contexts
                        .iter()
                        .find(|c| c.owner == owner)
                        .ok_or(EINVAL)?
                        .slot;
                    self.render_clients[index].rebind_contexts(client, true, &[slot])?;
                    self.render_clients.remove(index).map_err(|_| EIO)?
                } else {
                    let (root, _tables) = work.client.root.clone_low_tables()?;
                    let mut previous = KVec::with_capacity(work.client.buffers.len(), GFP_KERNEL)?;
                    for bo in &work.client.buffers {
                        previous.push(bo.clone(), GFP_KERNEL)?;
                    }
                    let mut bindings = KVec::with_capacity(work.client.bindings.len(), GFP_KERNEL)?;
                    for &binding in &work.client.bindings {
                        bindings.push(binding, GFP_KERNEL)?;
                    }
                    let mut next = compute::Client {
                        root,
                        buffers: previous,
                        bindings,
                        owner,
                        cpu_maps: crate::g17p_compute_runtime::CpuMaps::new()?,
                        primer_aliases: None,
                    };
                    // Remove every copied caller leaf before admitting this VM.
                    // All driver-owned pages and lower table attributes survive.
                    next.rebind_contexts(client, true, &[2]).inspect_err(|error| {
                        dev_err!(dev, "G17P: native render cloned caller replacement failed: {:?}\n", error);
                    })?;
                    next
                };
            let old = core::mem::replace(&mut work.client, next);
            self.render_clients.push(old, GFP_KERNEL)?;
        } else if let Some(client) = replacement {
            render::validate_client(&client, p)?;
            work.client
                .rebind_contexts(client, true, &[work.layout.context as u16])?;
        }
        let index = contexts.reserve(&work.client).inspect_err(|error| {
            dev_err!(dev, "G17P: native render slot reservation failed: {:?}\n", error);
        })?;
        let pair = contexts.contexts[index].slot as u32 - 1;
        work.select_pair(pair)?;
        work.layout.context = pair + 1;
        contexts.prepare_publication(
            &work.client,
            pair,
            self.vm.as_ref().ok_or(EINVAL)?.firmware_root(),
            self.memory.as_ref().ok_or(EINVAL)?,
            self.ttbs,
            work.growth.as_mut().ok_or(EINVAL)?,
        ).inspect_err(|error| {
            dev_err!(dev, "G17P: native render root publication failed: {:?}\n", error);
        })?;
        let fence = super::super::g17p_sync::work_fence()?;
        contexts.retain(owner, work.ordinal + 1, fence.clone())?;
        self.render_gate = Some(fence);
        Ok(())
    }
    pub(super) fn prepare_first_native_render_context(&mut self) -> Result {
        if *crate::module_parameters::native_render_vms.value() != 1 {
            return Ok(());
        }
        let work = self.render.as_mut().ok_or(EINVAL)?;
        if self.render_contexts.is_some() || work.ordinal != 0 {
            return Err(EINVAL);
        }
        let mut contexts = NativeRenderContexts::new(
            work.client.owner,
            false,
            *crate::module_parameters::partial_independent_owner.value() == 1,
            true,
        )?;
        contexts.reserve(&work.client)?;
        let service = work.growth.as_mut().ok_or(EINVAL)?;
        // The first source work is not published yet. Root admission precedes
        // binding its generation-zero growth token and revealing producers.
        service.pools[0].retired = true;
        contexts.prepare_publication(
            &work.client,
            0,
            self.vm.as_ref().ok_or(EINVAL)?.firmware_root(),
            self.memory.as_ref().ok_or(EINVAL)?,
            self.ttbs,
            service,
        )?;
        service.bind_pool_work(
            0,
            [0xfffffc2000000100, 0xfffffc2000000200],
            render::DESCRIPTORS[1],
            1,
            0,
        )?;
        let fence = super::super::g17p_sync::work_fence()?;
        contexts.retain(work.client.owner, 0, fence.clone())?;
        self.render_contexts = Some(contexts);
        self.render_gate = Some(fence);
        Ok(())
    }
    pub(super) fn finish_render_gate(&mut self, limited: bool) -> Result {
        if let Some(fence) = self.render_gate.take() {
            let work = self.render.as_ref().ok_or(EINVAL)?;
            let contexts = self.render_contexts.as_mut().ok_or(EINVAL)?;
            let service = work.growth.as_ref().ok_or(EINVAL)?;
            if limited {
                contexts.record_limit(work.client.owner, work.ordinal, service)?;
                fence.set_error(ENOMEM);
            }
            fence.signal();
            contexts.reap(service)?;
        }
        Ok(())
    }
    pub(super) fn fail_render_gate(&mut self, error: Error) {
        if let Some(control) = &self.render_control { control.receipt.fail(error); }
        match &self.pending {
            Some(super::PendingWork::Control(control)) => control.receipt.fail(error),
            Some(super::PendingWork::Render(wave)) => {
                for frame in &wave.frames[wave.next..] { frame.receipt.fail(error); }
            }
            _ => {},
        }
        self.faults.fail();
        if let Some(timestamps) = self.timestamps.as_mut() {
            timestamps.fail_pending(error);
        }
        if let Some(fence) = self.render_gate.take() {
            fence.set_error(error);
            fence.signal();
        }
        if let Some(contexts) = self.render_contexts.as_mut() {
            contexts.fail_pending(error);
        }
    }
}

/// Copy only table DATA and ownership references; never read mapped leaves.
fn clone_render_client(client: &compute::Client) -> Result<compute::Client> {
    let (root, _) = client.root.clone_low_tables()?;
    let mut buffers = KVec::with_capacity(client.buffers.len(), GFP_KERNEL)?;
    for bo in &client.buffers { buffers.push(bo.clone(), GFP_KERNEL)?; }
    let mut bindings = KVec::new();
    bindings.extend_from_slice(&client.bindings, GFP_KERNEL)?;
    Ok(compute::Client { root, buffers, bindings, owner: client.owner,
        cpu_maps: crate::g17p_compute_runtime::CpuMaps::new()?, primer_aliases: None })
}
/// The pool's own retirement was checked before this owner transfer. Preserve
/// old metadata/GEM ownership if rebind preflight fails; its commit cannot fail.
fn rebind_retired_pool(client: &mut compute::Client, desired: compute::Client, asid: u16) -> Result {
    let previous = client.owner;
    client.owner = desired.owner;
    let result = client.rebind_contexts(desired, true, &[asid]);
    if result.is_err() { client.owner = previous; }
    result
}

// Ordinary source pools retain private execution roots. Only an idle pool is
// rebound; the remaining pools' roots, tickets and growth readers stay live.
pub(crate) struct PoolTemplateSeed {
    pair: u32,
    snapshot: super::super::g17p_user_vm::TableSnapshot,
    buffers: KVec<kernel::sync::aref::ARef<super::super::g17p_drm::Object>>,
    bindings: KVec<(u64, u64, u64, u32)>,
    owner: (u64, u32),
    table_budget: usize,
}
pub(crate) struct PreparedPoolTemplate {
    pair: u32,
    snapshot: super::super::g17p_user_vm::TableSnapshot,
    client: compute::Client,
}
impl PoolTemplateSeed {
    pub(crate) fn prepare(self) -> Result<PreparedPoolTemplate> {
        let mut root = self.snapshot.clone_tree()?;
        root.prepare_spare_tables(self.table_budget)?;
        if *crate::module_parameters::submission_log.value() >= 3 {
            pr_info!("G17P: RENDER_TEMPLATE_PREPARED pair {} outside_runtime_lock\n", self.pair);
        }
        let client = compute::Client { root, buffers: self.buffers,
            bindings: self.bindings, owner: self.owner,
            cpu_maps: crate::g17p_compute_runtime::CpuMaps::new()?, primer_aliases: None };
        Ok(PreparedPoolTemplate { pair: self.pair, snapshot: self.snapshot, client })
    }
}
impl Session {
    pub(crate) fn render_selected_client(&self, incoming: &compute::Client, p: &Parameters)
        -> Result<Option<&compute::Client>> {
        let Some(work) = self.render.as_ref() else { return Ok(None); };
        if !self.independent_render_roots() { return Ok(Some(&work.client)); }
        let Some(pair) = self.next_render_pool_for_geometry(Some(incoming), p)? else { return Ok(None); };
        Ok(if pair == work.layout.pair { Some(&work.client) } else {
            self.render_pool_clients.iter().find(|(pool, _)| *pool == pair).map(|(_, client)| client)
        })
    }
    pub(crate) fn render_template_size(&self, incoming: &compute::Client, p: &Parameters)
        -> Result<Option<usize>> {
        let Some(work) = self.render.as_ref() else { return Ok(None); };
        if !self.independent_render_roots() { return Ok(None); }
        let Some(pair) = self.next_render_pool_for_geometry(Some(incoming), p)? else { return Ok(None); };
        if pair == work.layout.pair || self.render_pool_clients.iter().any(|(pool, _)| *pool == pair) {
            return Ok(None);
        }
        Ok(Some(work.client.root.table_count()))
    }
    pub(crate) fn capture_render_template(&self, incoming: &compute::Client, p: &Parameters,
        storage: super::super::g17p_user_vm::TableStorage) -> Result<Option<PoolTemplateSeed>> {
        if self.render_template_size(incoming, p)?.is_none() { return Ok(None); }
        let work = self.render.as_ref().ok_or(EIO)?;
        let pair = self.next_render_pool_for_geometry(Some(incoming), p)?.ok_or(EIO)?;
        let Some(snapshot) = work.client.root.capture_tables(storage)? else { return Ok(None); };
        let mut buffers = KVec::with_capacity(work.client.buffers.len(), GFP_KERNEL)?;
        for bo in &work.client.buffers { buffers.push(bo.clone(), GFP_KERNEL)?; }
        let mut bindings = KVec::new();
        bindings.extend_from_slice(&work.client.bindings, GFP_KERNEL)?;
        let table_budget = work.client.root.table_count()
            .checked_add(render::scratch_table_budget(p)?).ok_or(EOVERFLOW)?;
        Ok(Some(PoolTemplateSeed { pair, snapshot, buffers, bindings, owner: work.client.owner, table_budget }))
    }
    pub(crate) fn commit_render_template(&mut self, incoming: &compute::Client, p: &Parameters,
        prepared: PreparedPoolTemplate) -> Result<bool> {
        let work = self.render.as_ref().ok_or(EIO)?;
        if self.next_render_pool_for_geometry(Some(incoming), p)? != Some(prepared.pair)
            || !prepared.snapshot.matches(&work.client.root)
            || !Self::same_client(&prepared.client, &work.client) { return Ok(false); }
        if prepared.pair == work.layout.pair
            || self.render_pool_clients.iter().any(|(pool, _)| *pool == prepared.pair) { return Ok(true); }
        self.render_pool_clients.push((prepared.pair, prepared.client), GFP_KERNEL)?;
        Ok(true)
    }
    pub(super) fn independent_render_roots(&self) -> bool {
        self.independent_compute_enabled()
            && *crate::module_parameters::partial_independent_owner.value() == 1
            && *crate::module_parameters::alternate_queue_pairs.value() == 1
    }
    pub(super) fn prepare_pool_render_context(
        &mut self, dev: &kernel::device::Device, pair: u32,
        mut replacement: Option<compute::Client>, p: &Parameters,
    ) -> Result {
        use super::super::g17p_user_vm::UserVm;
        if !self.independent_render_roots() || pair >= super::super::g17p_render_lifecycle::POOL_SLOTS {
            return Err(EINVAL);
        }
        let work = self.render.as_mut().ok_or(EINVAL)?;
        let service = work.growth.as_ref().ok_or(EINVAL)?;
        if let Some(incoming) = &mut replacement {
            if pair == work.layout.pair {
                work.client.root.absorb_spare_tables(&mut incoming.root)?;
            } else if let Some((_, installed)) = self.render_pool_clients.iter_mut().find(|(pool,_)| *pool == pair) {
                installed.root.absorb_spare_tables(&mut incoming.root)?;
            }
        }
        let queued = !service.pools.get(pair as usize).ok_or(EIO)?.retired;
        if queued {
            let desired = replacement.as_ref().unwrap_or(&work.client);
            let installed = if pair == work.layout.pair { &work.client } else {
                &self.render_pool_clients.iter().find(|(pool,_)| *pool == pair).ok_or(EIO)?.1
            };
            if !Self::same_client(desired, installed) || !work.pair_same_geometry(pair,p) {
                pr_err!("G17P: queued pool mismatch pair {} desired {:?} installed {:?} same_client {} same_geometry {}\n",pair,desired.owner,installed.owner,Self::same_client(desired,installed),work.pair_same_geometry(pair,p));
                return Err(EBUSY);
            }
            // No live root or pool identity is rebound. Select the installed
            // owner and leave its firmware registration/PTEs untouched.
            if pair != work.layout.pair {
                let index=self.render_pool_clients.iter().position(|(pool,_)| *pool==pair).ok_or(EIO)?;
                core::mem::swap(&mut work.client,&mut self.render_pool_clients[index].1);
                self.render_pool_clients[index].0=work.layout.pair;
                work.select_pair(pair)?;
            }
            return Ok(());
        }
        render::validate_client(replacement.as_ref().unwrap_or(&work.client), p)?;
        let installed = if pair == work.layout.pair { Some(&work.client) } else {
            self.render_pool_clients.iter().find(|(pool,_)| *pool == pair).map(|(_,client)| client)
        };
        let pool = &service.pools[pair as usize];
        let asid = self.render_pool_asids[pair as usize];
        if asid != 0 && installed.is_some_and(|installed|
            Self::same_client(replacement.as_ref().unwrap_or(&work.client), installed)
                && pool.root == installed.root.root() && pool.slot == asid) {
            let memory=self.memory.as_ref().ok_or(EIO)?;
            let expected=(u64::from(asid)<<48) | installed.ok_or(EIO)?.root.root() | 1;
            if memory.read64(self.ttbs+u64::from(asid)*16)? != expected { return Err(EIO); }
            // This exact installed snapshot already received its owned growth
            // leaves. No PTE/ASID mutation: do not rescan or invalidate it.
            if pair != work.layout.pair {
                let index=self.render_pool_clients.iter().position(|(pool,_)| *pool==pair).ok_or(EIO)?;
                core::mem::swap(&mut work.client,&mut self.render_pool_clients[index].1);
                self.render_pool_clients[index].0=work.layout.pair;
                work.select_pair(pair)?;
            }
            return Ok(());
        }
        self.render_pool_clients.reserve(1, GFP_KERNEL)?;
        self.render_pool_asids[0] = 1;
        if self.render_pool_asids[pair as usize] == 0 {
            // Render grows downwards, compute upwards; reservations are shared.
            // Neither engine overwrites a slot retained by the other engine.
            let asid = (4..64).rev().find(|asid|
                self.independent_compute.asid_mask() & (1u64 << asid) == 0).ok_or(EBUSY)?;
            self.independent_compute.reserve_render_asid(asid)?;
            self.render_pool_asids[pair as usize] = asid;
        }
        let asid = self.render_pool_asids[pair as usize];
        let memory = self.memory.as_mut().ok_or(EINVAL)?;
        let vm = self.vm.as_ref().ok_or(EINVAL)?;
        if pair == work.layout.pair {
            if let Some(desired) = replacement.take() {
                if !Self::same_client(&desired, &work.client) {
                    retire_render_fallbacks_slot(&mut work.client, &desired, memory, vm, asid, Some((service, pair)))?;
                    rebind_retired_pool(&mut work.client, desired, asid)?;
                }
            }
        } else {
            let index = if let Some(index) = self.render_pool_clients.iter().position(|(pool, _)| *pool == pair) {
                index
            } else {
                // The clone already has the selected caller's exact snapshot.
                // It is retained before any fallible rebind or publication.
                let next = clone_render_client(&work.client)?;
                let index = self.render_pool_clients.len();
                self.render_pool_clients.push((pair, next), GFP_KERNEL)?;
                index
            };
            let next = &mut self.render_pool_clients[index].1;
            if !Self::same_client(next, replacement.as_ref().unwrap_or(&work.client)) {
                let desired = match replacement.take() {
                    Some(client) => client,
                    None => clone_render_client(&work.client)?,
                };
                retire_render_fallbacks_slot(next, &desired, memory, vm, asid, Some((service, pair)))?;
                rebind_retired_pool(next, desired, asid)?;
            }
            service.mirror_pool_mappings(pair, &mut next.root)?;
            // Both roots stay Session-owned throughout transport selection.
            core::mem::swap(&mut work.client, next);
            self.render_pool_clients[index].0 = work.layout.pair;
            work.select_pair(pair)?;
        }
        work.layout.context = u32::from(asid);
        let at = self.ttbs + u64::from(asid) * 16;
        let upper = memory.read64(self.ttbs + 24)? & 0x000003ffffffc000;
        let low = (u64::from(asid) << 48) | work.client.root.root() | 1;
        if memory.read64(at)? != low {
            // A newly reserved slot must be empty. An installed pool keeps
            // its root identity across retired mapping refreshes.
            if memory.read64(at)? != 0 { return Err(EIO); }
            memory.write64(at + 8, (u64::from(asid) << 48) | upper | 1)?;
            memory.write64(at, low)?;
            memory.clean(at, 16)?;
        } else if memory.read64(at + 8)? != ((u64::from(asid) << 48) | upper | 1) {
            return Err(EIO);
        }
        UserVm::invalidate(asid);
        work.growth.as_mut().ok_or(EINVAL)?.bind_pool_root(memory, self.ttbs, pair,
            &work.client.root, asid, u32::from(asid))?;
        if *crate::module_parameters::submission_log.value() != 0 {
            dev_info!(dev, "G17P: render pool {} owner {:?} ASID {} root {:#x} independently installed\n",
                pair, work.client.owner, asid, work.client.root.root());
        }
        Ok(())
    }
}
