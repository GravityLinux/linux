// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Retained synchronous growth service. Report pointers never select storage.

use super::{
    g17p_abi::Channel,
    g17p_growth as g,
    g17p_memory::{self, Memory},
    g17p_user_vm::UserVm,
    g17p_vm::Vm,
};
use kernel::prelude::*;
const STATE: u64 = 0xfffffc2001610000;
const LIST: u64 = 0xfffffc20c0838000;
const CAPACITY: u32 = 0x4000 / 8;

pub(crate) enum Action {
    Idle,
    Consumed,
    Reply {
        pool: u32,
        vm: u32,
        counter: u32,
        old: u32,
        new: u32,
        refused: bool,
    },
    Limit,
}
/// Owned analogue of GrowthService's pool_counters, pool_storage and work_owners.
pub(crate) struct Pool {
    pub(crate) identity: g::Owner,
    pub(crate) root: u64,
    pub(crate) slot: u16,
    pub(crate) counter: u32,
    pub(crate) generation: u32,
    pub(crate) retired: bool,
    pub(crate) limit_report: Option<[u8; 0x48]>,
    pub(crate) terminals: u32,
    state: u64,
    list: u64,
    growth_base: u64,
    request_limit: u32,
    refused: bool,
    limited: bool,
    counter_baseline: u32,
    work: [u64; 2],
    fragment: u64,
    fragment_event: u32,
    terminal_mask: u32,
    // Every retained initial and newly allocated TVB leaf, for root admission.
    mappings: KVec<(u64, u64)>,
}
/// Exact accepted render identity inside a retained pool. A different pool's
/// report or a later generation must never retire this publication.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkToken {
    pool: u32,
    root: u64,
    generation: u32,
    fragment: u64,
    event: u32,
}
struct WorkOwner {
    token: WorkToken,
    work: [u64; 2],
    counter_baseline: u32,
    limit_report: Option<[u8; 0x48]>,
}
pub(crate) struct Service {
    work_owners: KVec<WorkOwner>,
    command: Channel,
    report: Channel,
    cursor: u32,
    pub(crate) pools: KVec<Pool>,
    active_pool: usize,
    pub(crate) failed: Option<Error>,
    compute_terminals: u32,
    independent_compute_mask: u128,
    independent_compute_terminals: [u32; 128],
    compute_owners: KVec<u32>,
    receipt: Option<super::g17p_dependency::Receipt>,
    dependency: bool,
    pool_limits: [u32; super::g17p_render_lifecycle::POOL_SLOTS as usize],
}

fn word(memory: &Memory, vm: &Vm, va: u64) -> Result<u32> {
    memory.read_firmware32(vm.physical(memory, 2, va)?)
}
fn read(memory: &Memory, vm: &Vm, va: u64, body: &mut [u8]) -> Result {
    if va & 7 != 0 || body.len() % 8 != 0 {
        return Err(EINVAL);
    }
    for (i, qword) in body.chunks_exact_mut(8).enumerate() {
        let pa = vm.physical(memory, 2, va + i as u64 * 8)?;
        memory.invalidate(pa, 8)?;
        qword.copy_from_slice(&memory.read64(pa)?.to_le_bytes());
    }
    Ok(())
}
fn verify_root(memory: &Memory, ttbs: u64, root: u64) -> Result {
    memory.invalidate(ttbs + 16, 8)?;
    if memory.read64(ttbs + 16)? != ((1 << 48) | root | 1) {
        return Err(EIO);
    }
    Ok(())
}
impl Service {
    pub(crate) fn set_source_pool_limits(&mut self, global: u32, overrides: [u32; 2]) -> Result {
        let limits = g::source_pool_limits(global, overrides).ok_or(EINVAL)?;
        if self.pools.iter().any(|pool| pool.counter != 0) { return Err(EBUSY); }
        self.pool_limits.fill(if global == 0 { CAPACITY } else { global });
        self.pool_limits[..2].copy_from_slice(&limits);
        Ok(())
    }
    fn initial_mappings(
        memory: &Memory,
        vm: &Vm,
        list: u64,
        root: &UserVm,
    ) -> Result<KVec<(u64, u64)>> {
        let mut mappings = KVec::with_capacity(8 * (g::BLOCK / g::PAGE) as usize, GFP_KERNEL)?;
        for i in 0..8 {
            let mut bytes = [0; 8];
            read(memory, vm, list + i * 8, &mut bytes)?;
            let address = u64::from_le_bytes(bytes)
                .checked_mul(g::UNIT)
                .and_then(|offset| g::CONTEXT_BASE.checked_add(offset))
                .ok_or(EIO)?;
            g::block_id(address).ok_or(EIO)?;
            for offset in (0..g::BLOCK).step_by(g::PAGE as usize) {
                let pte = root.pte(address + offset)?;
                // Initial source IDs describe the sparse retained bootstrap
                // shape, unlike a newly allocated full growth block. Caller
                // leaves in that shape retain their GEM owner independently.
                if pte == 0 {
                    continue;
                }
                if pte & 3 != 3 {
                    return Err(EIO);
                }
                let pa = pte & 0x000003ffffffc000;
                if memory.word64(pa).is_ok() {
                    mappings.push((address + offset, pa), GFP_KERNEL)?;
                }
            }
        }
        Ok(mappings)
    }
    fn verify_pool_root(memory: &Memory, ttbs: u64, pool: &Pool) -> Result {
        let at = ttbs + pool.slot as u64 * 16;
        memory.invalidate(at, 8)?;
        if memory.read64(at)? != ((pool.slot as u64) << 48 | pool.root | 1) {
            return Err(EIO);
        }
        Ok(())
    }
    pub(crate) fn register_pool(
        &mut self,
        memory: &Memory,
        vm: &Vm,
        root: &UserVm,
        state: u64,
        list: u64,
        shared: u64,
        request_limit: u32,
        layout: super::g17p_render_lifecycle::Layout,
    ) -> Result {
        if self.failed.is_some()
            || self.dependency
            || layout.pair as usize != self.pools.len()
            || super::g17p_render_lifecycle::pool_grids(layout.pair).map_err(|_| EINVAL)? != layout.grids
            || !(1..=512).contains(&request_limit)
            || word(memory, vm, shared + 0xc)? != layout.pair
            || word(memory, vm, state)? != 8
            || word(memory, vm, state + 4)? != 8
        {
            return Err(EINVAL);
        }
        let length = (request_limit as u64).checked_mul(g::INCREMENT as u64)
            .and_then(|n| n.checked_mul(0x28000)).ok_or(EOVERFLOW)?;
        let mut growth_base = self.reserved_growth_end()?;
        // Select a whole tranche absent from the executable caller/private
        // root. Valid caller bindings (including future shader encoders) are
        // not rejected merely because they occupy an old fixed candidate VA.
        // No PTE is changed here; allocation retains its checked grow() path.
        loop {
            let end = growth_base.checked_add(length).ok_or(EOVERFLOW)?;
            if end > (1u64 << 42) { return Err(ENOMEM); }
            let mut conflict = None;
            for address in (growth_base..end).step_by(g::PAGE as usize) {
                if root.pte(address)? != 0 {
                    conflict = Some(address);
                    break;
                }
            }
            if let Some(address) = conflict {
                growth_base = address.checked_add(g::PAGE).and_then(|v| v.checked_add(g::UNIT - 1))
                    .ok_or(EOVERFLOW)? & !(g::UNIT - 1);
            } else { break; }
        }
        self.pools.push(
            Pool {
                identity: g::Owner { vm: 1, pool: layout.pair },
                root: root.root(),
                slot: 1,
                counter: 0,
                generation: 0,
                retired: true,
                limit_report: None,
                terminals: 0,
                state,
                list,
                growth_base,
                request_limit,
                refused: false,
                limited: false,
                counter_baseline: 0,
                work: [0; 2],
                fragment: 0,
                fragment_event: layout.grids[1],
                terminal_mask: (1 << layout.grids[0]) | (1 << layout.grids[1]),
                mappings: Self::initial_mappings(memory, vm, list, root)?,
            },
            GFP_KERNEL,
        )?;
        Ok(())
    }
    /// None means empty: the caller must stop, because firmware can append
    /// before step_ordinary reads the tail again. A published nonempty head
    /// remains immutable until our consumer advances. Some(None) needs no
    /// particular root; Some(Some(root)) is this head's growth owner.
    pub(crate) fn requested_root(&self, memory: &Memory, vm: &Vm) -> Result<Option<Option<u64>>> {
        let tail = word(memory, vm, self.report.states[0] + 0x20)?;
        if tail >= 256 { return Err(EIO); }
        if tail == self.cursor { return Ok(None); }
        let mut body = [0; 0x48];
        read(memory, vm, self.report.states[1] + self.cursor as u64 * 0x48, &mut body)?;
        if u32::from_le_bytes(body[..4].try_into().unwrap()) != 6 { return Ok(Some(None)); }
        let pool = u32::from_le_bytes(body[8..12].try_into().unwrap());
        Ok(Some(Some(self.pools.iter().find(|p| p.identity.pool == pool).ok_or(EIO)?.root)))
    }
    /// Retained driver growth pages are shared by logical render roots. A
    /// returning root may predate an allocation made while another VM ran.
    /// Install only absent owned pages; a different physical owner is fatal.
    pub(crate) fn mirror_retained_mappings(&self, root: &mut UserVm) -> Result {
        if self.failed.is_some() || self.pools.iter().any(|pool| !pool.retired) {
            return Err(EBUSY);
        }
        let mut missing = KVec::new();
        for pool in &self.pools {
            for &(va, pa) in &pool.mappings {
                let leaf = root.pte(va)?;
                if leaf == 0 {
                    missing.push((va, pa), GFP_KERNEL)?;
                } else if leaf & 0x000003ffffffc003 != pa | 3 {
                    return Err(EIO);
                }
            }
        }
        if !missing.is_empty() {
            root.grow(&missing)?;
        }
        Ok(())
    }
    /// A retired clone may still borrow a different pool's grown leaves.
    /// Their exact backing stays owned by that pool; the clone need not keep
    /// its aliases when admitting caller BOs at those otherwise valid DVAs.
    pub(crate) fn foreign_growth_leaf(&self, selected: u32, va: u64, pte: u64) -> bool {
        self.pools.iter().any(|pool| pool.identity.pool != selected
            && va >= pool.growth_base
            && va - pool.growth_base < u64::from(pool.counter) * g::INCREMENT as u64 * 0x28000
            && pool.mappings.iter().any(|&(address, pa)| address == va
                && pte == (pa | 0x00c0000000000c8b)))
    }
    /// Snapshot only the Source-owned leaf identities, never backing bytes.
    /// The selected pool is retired/reserved before this seed leaves locking.
    pub(crate) fn render_rebind_leaves(&self, selected: u32)
        -> Result<(KVec<(u64, u64)>, KVec<(u64, u64)>)> {
        let own = self.pools.iter().find(|pool| pool.identity.pool == selected).ok_or(EINVAL)?;
        if self.failed.is_some() || !own.retired { return Err(EBUSY); }
        let mut own_leaves = KVec::new(); own_leaves.extend_from_slice(&own.mappings, GFP_KERNEL)?;
        let mut foreign = KVec::new();
        for pool in self.pools.iter().filter(|pool| pool.identity.pool != selected) {
            for &(va, pa) in &pool.mappings {
                if va >= pool.growth_base && va - pool.growth_base
                    < u64::from(pool.counter) * g::INCREMENT as u64 * 0x28000 {
                    foreign.push((va, pa | 0x00c0000000000c8b), GFP_KERNEL)?;
                }
            }
        }
        Ok((own_leaves, foreign))
    }
    /// An incoming independently retained pool root needs only that pool's
    /// immutable growth leaves. Never modify another live root or TVB owner.
    pub(crate) fn mirror_pool_mappings(&self, pool_id: u32, root: &mut UserVm) -> Result {
        let pool = self.pools.iter().find(|p| p.identity.pool == pool_id).ok_or(EINVAL)?;
        if self.failed.is_some() || !pool.retired { return Err(EBUSY); }
        let mut missing = KVec::new();
        for &(va, pa) in &pool.mappings {
            let leaf = root.pte(va)?;
            if leaf == 0 { missing.push((va, pa), GFP_KERNEL)?; }
            else if leaf & 0x000003ffffffc003 != pa | 3 { return Err(EIO); }
        }
        if !missing.is_empty() { root.grow(&missing)?; }
        Ok(())
    }
    pub(crate) fn bind_pool_root(
        &mut self,
        memory: &Memory,
        ttbs: u64,
        pool_id: u32,
        root: &UserVm,
        slot: u16,
        firmware_vm: u32,
    ) -> Result {
        let pool = self
            .pools
            .iter_mut()
            .find(|p| p.identity.pool == pool_id)
            .ok_or(EINVAL)?;
        let identity = g::Owner {
            vm: firmware_vm,
            pool: pool_id,
        };
        if self.failed.is_some()
            || !pool.retired
            || !(1..64).contains(&slot)
            || !identity.valid()
            || (identity != pool.identity && pool.counter != 0)
        {
            return Err(EBUSY);
        }
        let at = ttbs + slot as u64 * 16;
        memory.invalidate(at, 8)?;
        if memory.read64(at)? != ((slot as u64) << 48 | root.root() | 1) {
            return Err(EIO);
        }
        for &(va, pa) in &pool.mappings {
            if root.pte(va)? & 0x000003ffffffc003 != pa | 3 {
                return Err(EIO);
            }
        }
        pool.identity = identity;
        pool.root = root.root();
        pool.slot = slot;
        Ok(())
    }
    pub(crate) fn bind_work(&mut self, fragment: u64) -> Result {
        let pool = &self.pools[self.active_pool];
        self.bind_pool_work(
            pool.identity.pool,
            pool.work,
            fragment,
            pool.fragment_event,
            pool.generation.checked_add(1).ok_or(EIO)?,
        )
    }
    pub(crate) fn bind_pool_work(
        &mut self,
        pool_id: u32,
        work: [u64; 2],
        fragment: u64,
        event: u32,
        generation: u32,
    ) -> Result {
        self.bind_pool_work_owned(pool_id, work, fragment, event, generation, false)
    }
    pub(crate) fn bind_pool_work_owned(
        &mut self, pool_id: u32, work: [u64; 2], fragment: u64,
        event: u32, generation: u32, append: bool,
    ) -> Result {
        if self.failed.is_some() || fragment == 0 || work.contains(&0) {
            return Err(EINVAL);
        }
        let index = self
            .pools
            .iter()
            .position(|p| p.identity.pool == pool_id)
            .ok_or(EINVAL)?;
        let pool = &mut self.pools[index];
        if !pool.retired && !append { return Err(EBUSY); }
        if event != pool.fragment_event {
            return Err(EINVAL);
        }
        self.work_owners.reserve(2, GFP_KERNEL)?;
        if !pool.retired && !self.work_owners.iter().any(|w| w.token.pool == pool_id) {
            self.work_owners.push(WorkOwner { token: WorkToken { pool: pool_id,
                root: pool.root, generation: pool.generation, fragment: pool.fragment,
                event: pool.fragment_event }, work: pool.work,
                counter_baseline: pool.counter_baseline, limit_report: pool.limit_report }, GFP_KERNEL)?;
        }
        self.work_owners.push(WorkOwner { token: WorkToken { pool: pool_id,
            root: pool.root, generation, fragment, event }, work,
            counter_baseline: pool.counter, limit_report: None }, GFP_KERNEL)?;
        let was_retired = pool.retired;
        pool.generation = generation;
        pool.counter_baseline = pool.counter;
        pool.retired = false;
        if was_retired { pool.refused = false; pool.limited = false; pool.limit_report = None; }
        pool.work = work;
        pool.fragment = fragment;
        pool.fragment_event = event;
        self.active_pool = index;
        Ok(())
    }
    pub(crate) fn work_token(&self) -> Result<WorkToken> {
        let pool = self.pools.get(self.active_pool).ok_or(EIO)?;
        if pool.retired || self.failed.is_some() { return Err(EIO); }
        Ok(WorkToken { pool: pool.identity.pool, root: pool.root,
            generation: pool.generation, fragment: pool.fragment,
            event: pool.fragment_event })
    }
    fn token_index(&self, token: WorkToken) -> Result<usize> {
        let index = self.pools.iter().position(|pool| pool.identity.pool == token.pool)
            .ok_or(EIO)?;
        let pool = &self.pools[index];
        if self.failed.is_some() || pool.retired || pool.root != token.root
            || (!self.work_owners.iter().any(|w| w.token == token)
                && (pool.generation != token.generation || pool.fragment != token.fragment
                    || pool.fragment_event != token.event)) {
            return Err(EIO);
        }
        Ok(index)
    }
    pub(crate) fn token_terminals(&self, token: WorkToken) -> Result<u32> {
        Ok(self.pools[self.token_index(token)?].terminals)
    }
    pub(crate) fn token_limited(&self, token: WorkToken) -> Result<bool> {
        let index = self.token_index(token)?;
        Ok(self.work_owners.iter().find(|w| w.token == token)
            .map_or(self.pools[index].limit_report.is_some(), |w| w.limit_report.is_some()))
    }
    pub(crate) fn retire_token(&mut self, token: WorkToken) -> Result {
        let index = self.token_index(token)?;
        if let Some(owner) = self.work_owners.iter().position(|w| w.token == token) {
            self.work_owners.remove(owner).map_err(|_| EIO)?;
        }
        self.pools[index].retired = !self.work_owners.iter().any(|w| w.token.pool == token.pool);
        Ok(())
    }
    pub(crate) fn retire_work(&mut self) -> Result {
        let pool = &mut self.pools[self.active_pool];
        if pool.retired || self.failed.is_some() {
            return Err(EIO);
        }
        let pool_id = pool.identity.pool;
        pool.retired = true;
        while let Some(index) = self.work_owners.iter().position(|owner| owner.token.pool == pool_id) {
            self.work_owners.remove(index).map_err(|_| EIO)?;
        }
        Ok(())
    }
    pub(crate) fn limit_report(&self) -> Option<&[u8; 0x48]> {
        self.pools[self.active_pool].limit_report.as_ref()
    }
    pub(crate) fn reserved_growth_end(&self) -> Result<u64> {
        self.pools.iter().try_fold(0, |last, pool| {
            let size = (pool.request_limit as u64).checked_mul(g::INCREMENT as u64)
                .and_then(|n| n.checked_mul(0x28000)).ok_or(EOVERFLOW)?;
            Ok(last.max(pool.growth_base.checked_add(size).ok_or(EOVERFLOW)?))
        })
    }
    pub(crate) fn command_channel(&self) -> Channel { self.command }
    pub(crate) fn cursor(&self) -> u32 {
        self.cursor
    }
    pub(crate) fn terminals(&self) -> u32 {
        self.pools[self.active_pool].terminals
    }
    pub(crate) fn register_independent_compute(&mut self, mask: u128) {
        self.independent_compute_mask |= mask;
    }
    pub(crate) fn independent_compute_terminals(&self,grid:u8)->u32 {
        self.independent_compute_terminals[usize::from(grid)]
    }
    pub(crate) fn compute_terminals(&self) -> u32 {
        self.compute_terminals
    }
    pub(crate) fn begin_compute(&mut self, ordinal: u32) -> Result {
        if self.compute_owners.len() == 36 || self.compute_owners.contains(&ordinal) {
            return Err(EBUSY);
        }
        self.compute_owners.push(ordinal, GFP_KERNEL)?;
        Ok(())
    }
    pub(crate) fn finish_compute(&mut self, ordinal: u32) -> Result {
        if let Some(index) = self.compute_owners.iter().position(|v| *v == ordinal) {
            self.compute_owners.remove(index).map_err(|_| EIO)?;
        }
        Ok(())
    }
    pub(crate) fn new(
        memory: &Memory,
        vm: &Vm,
        ttbs: u64,
        root: &UserVm,
        command: Channel,
        report: Channel,
    ) -> Result<Self> {
        Self::new_graph(memory, vm, ttbs, root, command, report, STATE, LIST)
    }
    #[allow(dead_code)]
    pub(crate) fn new_dependency(
        memory: &Memory,
        vm: &Vm,
        ttbs: u64,
        root: &UserVm,
        command: Channel,
        report: Channel,
    ) -> Result<Self> {
        use super::g17p_dependency::LEAVES;
        let mut service = Self::new_graph(
            memory, vm, ttbs, root, command, report, LEAVES[4], LEAVES[1],
        )?;
        service.dependency = true;
        service.pools[0].retired = true;
        // The fragment queue moves from grid 1 to grid 2 in the native
        // four-queue graph. Growth report identity follows the queue grid,
        // independently of the render context's event_slot (which stays 1).
        service.pools[0].fragment_event = super::g17p_dependency::LAYOUTS[2].grid;
        // Terminal subtypes are queue-grid bitmasks. Native C/R/C inserts CL
        // at grid zero, moving the retained render pair to grids one/two.
        // Keep its terminal separate from both CL owners (masks one/eight).
        service.pools[0].terminal_mask = (1 << super::g17p_dependency::LAYOUTS[1].grid)
            | (1 << super::g17p_dependency::LAYOUTS[2].grid);
        Ok(service)
    }
    #[allow(dead_code)]
    pub(crate) fn expect_dependency_receipt(&mut self, sequence: u32) -> Result {
        if !self.dependency {
            return Err(EINVAL);
        }
        if self.receipt.as_ref().is_some_and(|r| r.pending()) {
            return Err(EBUSY);
        }
        self.receipt = Some(super::g17p_dependency::Receipt::new(sequence));
        Ok(())
    }
    #[allow(dead_code)]
    pub(crate) fn require_dependency(&self, memory: &Memory, ttbs: u64, root: &UserVm) -> Result {
        if !self.dependency || root.root() != self.pools[0].root {
            return Err(EINVAL);
        }
        verify_root(memory, ttbs, self.pools[0].root)
    }
    fn new_graph(
        memory: &Memory,
        vm: &Vm,
        ttbs: u64,
        root: &UserVm,
        command: Channel,
        report: Channel,
        state: u64,
        list: u64,
    ) -> Result<Self> {
        verify_root(memory, ttbs, root.root())?;
        if report.states[1].checked_add(256 * 0x48) != Some(report.ring)
            || word(memory, vm, state)? != 8
            || word(memory, vm, state + 4)? != 8
        {
            return Err(EIO);
        }
        // Skip the already-observed opening receipt, as GrowthService does.
        let cursor = word(memory, vm, report.states[0] + 0x20)?;
        if cursor >= 256 {
            return Err(EIO);
        }
        let mut pools = KVec::with_capacity(2, GFP_KERNEL)?;
        pools.push(
            Pool {
                identity: g::Owner { vm: 1, pool: 0 },
                root: root.root(),
                slot: 1,
                counter: 0,
                refused: false,
                limited: false,
                terminals: 0,
                fragment: super::g17p_render_lifecycle::DESCRIPTORS[1],
                fragment_event: 1,
                terminal_mask: 3,
                generation: 0,
                counter_baseline: 0,
                retired: false,
                limit_report: None,
                work: [0xfffffc2000000100, 0xfffffc2000000200],
                state,
                list,
                growth_base: g::GROWTH_BASE,
                request_limit: g::REQUEST_LIMIT,
                mappings: Self::initial_mappings(memory, vm, list, root)?,
            },
            GFP_KERNEL,
        )?;
        Ok(Self {
            work_owners: KVec::new(),
            command,
            report,
            cursor,
            pools,
            active_pool: 0,
            failed: None,
            compute_terminals: 0,
            independent_compute_mask: 0,
            independent_compute_terminals: [0; 128],
            compute_owners: KVec::with_capacity(36, GFP_KERNEL)?,
            receipt: None,
            dependency: false,
            pool_limits: [CAPACITY; super::g17p_render_lifecycle::POOL_SLOTS as usize],
        })
    }

    fn consume(&mut self, memory: &mut Memory, vm: &Vm, next: u32) -> Result {
        vm.write(memory, 2, self.report.states[0], &next.to_le_bytes())?;
        g17p_memory::sync();
        self.cursor = next;
        Ok(())
    }
    fn allocate(
        &mut self,
        memory: &mut Memory,
        vm: &Vm,
        root: &mut UserVm,
        old: u32,
        index: usize,
    ) -> Result<u32> {
        if old > CAPACITY {
            return Err(EIO);
        }
        let new = old.checked_add(g::INCREMENT as u32).ok_or(EIO)?;
        if new > self.pool_limits[index] {
            return Err(ENOMEM);
        }
        let pool = &mut self.pools[index];
        if pool.counter >= pool.request_limit {
            return Err(EIO);
        }
        let addresses: [u64; g::INCREMENT] = core::array::from_fn(|i| {
            pool.growth_base + (pool.counter as u64 * g::INCREMENT as u64 + i as u64) * 0x28000
        });
        let mut prior = KVec::with_capacity(old as usize, GFP_KERNEL)?;
        for i in 0..old {
            let mut body = [0; 8];
            read(memory, vm, pool.list + i as u64 * 8, &mut body)?;
            let id = u64::from_le_bytes(body);
            let address = id
                .checked_mul(g::UNIT)
                .and_then(|a| a.checked_add(g::CONTEXT_BASE))
                .ok_or(EIO)?;
            prior.push(address, GFP_KERNEL)?;
        }
        if !g::block_list(&prior, &addresses) {
            return Err(EIO);
        }
        let size = g::INCREMENT * g::BLOCK as usize;
        let physical = memory.allocate(size)?;
        if physical % g::UNIT != 0 {
            return Err(EIO);
        }
        memory.clean(physical, size)?;
        let mut pages = KVec::with_capacity(size / g::PAGE as usize, GFP_KERNEL)?;
        for (i, va) in addresses.into_iter().enumerate() {
            for offset in (0..g::BLOCK).step_by(g::PAGE as usize) {
                pages.push(
                    (va + offset, physical + i as u64 * g::BLOCK + offset),
                    GFP_KERNEL,
                )?;
            }
        }
        // grow() allocates/preflights all table paths before any live store.
        // Backing stays owned by Memory even if preparation/publication fails.
        pool.mappings.reserve(pages.len(), GFP_KERNEL)?;
        root.grow(&pages)?;
        UserVm::invalidate(pool.slot);
        for &(va, pa) in &pages {
            pool.mappings.push((va, pa), GFP_KERNEL)?;
        }
        let mut ids = [0; g::INCREMENT * 8];
        for (i, address) in addresses.into_iter().enumerate() {
            ids[i * 8..i * 8 + 8].copy_from_slice(&g::block_id(address).ok_or(EIO)?.to_le_bytes());
        }
        vm.write(memory, 2, pool.list + old as u64 * 8, &ids)?;
        g17p_memory::sync();
        let mut counts = [0; 8];
        counts[..4].copy_from_slice(&new.to_le_bytes());
        counts[4..].copy_from_slice(&new.to_le_bytes());
        vm.write(memory, 2, pool.state, &counts)?;
        g17p_memory::sync();
        Ok(new)
    }
    pub(crate) fn step(
        &mut self,
        memory: &mut Memory,
        vm: &Vm,
        root: &mut UserVm,
        ttbs: u64,
        compute_ordinal: Option<u32>,
    ) -> Result<Action> {
        if self.dependency {
            return Err(EINVAL);
        }
        self.checked_step(memory, vm, root, ttbs, compute_ordinal, false)
    }
    /// Ordinary lanes share one report stream. Pool tokens own growth;
    /// compute terminal ownership follows its admitted publication FIFO.
    pub(crate) fn step_ordinary(
        &mut self, memory: &mut Memory, vm: &Vm, root: &mut UserVm, ttbs: u64,
    ) -> Result<Action> {
        if self.dependency { return Err(EINVAL); }
        self.checked_step(memory, vm, root, ttbs, None, true)
    }
    /// One native report stream has both live CL owners and the render pair.
    /// Route CL terminals through their owned FIFO, and keep serving render
    /// growth while CL is pending. The command being waited on is not an
    /// ownership discriminator for this mixed stream.
    #[allow(dead_code)]
    pub(crate) fn step_dependency(
        &mut self,
        memory: &mut Memory,
        vm: &Vm,
        root: &mut UserVm,
        ttbs: u64,
    ) -> Result<Action> {
        if !self.dependency || self.receipt.is_none() {
            return Err(EINVAL);
        }
        self.checked_step(memory, vm, root, ttbs, None, true)
    }
    /// The same mixed reader services later renders on the retained native
    /// pair. Its receipt/cursor history and TVB inventory must survive CL.
    pub(crate) fn step_dependency_render(
        &mut self,
        memory: &mut Memory,
        vm: &Vm,
        root: &mut UserVm,
        ttbs: u64,
    ) -> Result<Action> {
        self.require_dependency(memory, ttbs, root)?;
        self.checked_step(memory, vm, root, ttbs, None, false)
    }
    /// After the native wave, the retained report reader still owns its
    /// cursor/receipt history. A subsequent standalone CL command cannot
    /// attribute a new growth/limit request to the completed render.
    pub(crate) fn step_dependency_compute(
        &mut self,
        memory: &mut Memory,
        vm: &Vm,
        root: &mut UserVm,
        ttbs: u64,
        ordinal: u32,
    ) -> Result<Action> {
        self.require_dependency(memory, ttbs, root)?;
        self.checked_step(memory, vm, root, ttbs, Some(ordinal), false)
    }

    fn checked_step(
        &mut self,
        memory: &mut Memory,
        vm: &Vm,
        root: &mut UserVm,
        ttbs: u64,
        compute_ordinal: Option<u32>,
        mixed: bool,
    ) -> Result<Action> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        let result = self.step_inner(memory, vm, root, ttbs, compute_ordinal, mixed);
        if let Err(error) = &result {
            let mut body = [0; 0x48];
            let _ = read(memory, vm, self.report.states[1] + self.cursor as u64 * 0x48, &mut body);
            pr_err!("G17P: growth service failed {:?}, cursor {}, report {:02x?}\n", error, self.cursor, body);
            for pool in &self.pools {
                pr_err!("G17P: growth pool {} root {:#x} counter {} baseline {} retired {} refused {} limited {} fragment {:#x} event {}\n", pool.identity.pool, pool.root, pool.counter, pool.counter_baseline, pool.retired, pool.refused, pool.limited, pool.fragment, pool.fragment_event);
            }
            self.failed = Some(*error);
        }
        result
    }

    fn step_inner(
        &mut self,
        memory: &mut Memory,
        vm: &Vm,
        root: &mut UserVm,
        ttbs: u64,
        compute_ordinal: Option<u32>,
        mixed: bool,
    ) -> Result<Action> {
        let tail = word(memory, vm, self.report.states[0] + 0x20)?;
        if tail >= 256 {
            return Err(EIO);
        }
        if tail == self.cursor {
            return Ok(Action::Idle);
        }
        let mut body = [0; 0x48];
        read(
            memory,
            vm,
            self.report.states[1] + self.cursor as u64 * 0x48,
            &mut body,
        )?;
        let opcode = u32::from_le_bytes(body[..4].try_into().unwrap());
        let next = (self.cursor + 1) & 255;
        if opcode == 13 && self.receipt.as_mut().is_some_and(|r| r.consume(0, &body)) {
            self.consume(memory, vm, next)?;
            return Ok(Action::Consumed);
        }
        if compute_ordinal.is_some() && !mixed && opcode != 1 {
            // No live render can request growth here. Retain unexpected
            // evidence without allocating, replying, or advancing credits.
            return Err(EIO);
        }
        if opcode == 7 {
            let mut matched = None;
            for (index, pool) in self.pools.iter().enumerate() {
                if !pool.retired
                    && pool.refused
                    && (self.work_owners.iter().any(|w| w.token.pool == pool.identity.pool
                        && w.limit_report.is_none() && pool.counter > w.counter_baseline
                        && pool.identity.limit(&body, &w.work, w.token.fragment, w.token.event).is_some())
                        || (!self.work_owners.iter().any(|w| w.token.pool == pool.identity.pool)
                            && pool.counter > pool.counter_baseline && !pool.limited
                            && pool.identity.limit(&body, &pool.work, pool.fragment, pool.fragment_event).is_some()))
                {
                    if matched.replace(index).is_some() {
                        return Err(EIO);
                    }
                }
            }
            let index = matched.ok_or(EIO)?;
            let pool = &self.pools[index];
            for owner in &mut self.work_owners {
                if owner.token.pool == pool.identity.pool && owner.limit_report.is_none()
                    && pool.identity.limit(&body, &owner.work, owner.token.fragment, owner.token.event).is_some() {
                    owner.limit_report = Some(body);
                }
            }
            self.pools[index].limited = true;
            self.pools[index].limit_report = Some(body);
            // Qualified consume-only closure: no command or doorbell.
            self.consume(memory, vm, next)?;
            return Ok(Action::Limit);
        }
        if opcode == 4 {
            // The ABI has no fault address/owner. Retain this report and all
            // active backing; the sole live render fails rather than guessing.
            let _payload_free = g::fatal(&body);
            return Err(EIO);
        }
        if opcode == 1 {
            let flags=u128::from_le_bytes(body[4..20].try_into().unwrap());
            let independent=flags & self.independent_compute_mask;
            if independent != 0 {
                for grid in 0..128 {
                    if independent & (1u128<<grid)!=0 {
                        self.independent_compute_terminals[grid]=self.independent_compute_terminals[grid]
                            .checked_add(1).ok_or(EOVERFLOW)?;
                    }
                }
                let remaining=flags & !self.independent_compute_mask;
                if remaining==0 {
                    self.consume(memory,vm,next)?;
                    return Ok(Action::Consumed);
                }
                // A coalesced notification can contain render and compute
                // progress. Retire jobs only from their immutable own tickets.
                body[4..20].copy_from_slice(&remaining.to_le_bytes());
            }
            // The same physical report ring serves both engines. A CL2
            // terminal belongs only to the active synchronous compute owner;
            // it cannot satisfy the render pair's terminal baseline.
            let subtype = u32::from_le_bytes(body[4..8].try_into().unwrap());
            let combined = self
                .pools
                .iter()
                .fold(0, |bits, pool| bits | pool.terminal_mask);
            let mut mask = 0;
            for (index, pool) in self.pools.iter().enumerate() {
                if subtype & !combined == 0 && subtype & pool.terminal_mask == pool.terminal_mask {
                    mask |= 1 << index;
                }
            }
            let recognized = self.pools.iter().enumerate().fold(0, |bits, (index, pool)|
                bits | if mask & (1 << index) != 0 { pool.terminal_mask } else { 0 });
            if mask != 0 && subtype != recognized { return Err(EIO); }
            if mask == 0 && body[8..16] == [0; 8] {
                if (!mixed && compute_ordinal.is_none()) || self.compute_owners.is_empty() {
                    return Err(EIO);
                }
                self.compute_owners.remove(0).map_err(|_| EIO)?;
                self.compute_terminals = self.compute_terminals.checked_add(1).ok_or(EIO)?;
                self.consume(memory, vm, next)?;
                return Ok(Action::Consumed);
            }
            if mask == 0 || body[8..16] != [0; 8] {
                return Err(EIO);
            }
            for (index, pool) in self.pools.iter_mut().enumerate() {
                if mask & (1 << index) != 0 {
                    pool.terminals = pool.terminals.checked_add(1).ok_or(EIO)?;
                }
            }
            self.consume(memory, vm, next)?;
            return Ok(Action::Consumed);
        }
        let pool_id = u32::from_le_bytes(body[8..12].try_into().unwrap());
        let index = self
            .pools
            .iter()
            .position(|p| p.identity.pool == pool_id)
            .ok_or(EIO)?;
        let pool = &self.pools[index];
        let owner = pool.identity;
        if pool.retired
            || !owner.request(&body, pool.counter)
            || pool.counter >= pool.request_limit
            || pool.limited
        {
            return Err(EIO);
        }
        Self::verify_pool_root(memory, ttbs, pool)?;
        if root.root() != pool.root {
            return Err(EIO);
        }
        let head = word(memory, vm, self.command.states[0])?;
        let slot = word(memory, vm, self.command.states[2])?;
        if head >= 256 || slot >= 256 || (slot + 1) & 255 == head {
            return Err(EIO);
        }
        let old = word(memory, vm, self.pools[index].state)?;
        if old != word(memory, vm, self.pools[index].state + 4)? {
            return Err(EIO);
        }
        let (new, refused) = match self.allocate(memory, vm, root, old, index) {
            Ok(new) => (new, false),
            // No report credit, command body, counter or producer changes.
            // An ordinary worker replenishes detached backing off-lock and
            // retries this exact owned report; independent owners still run.
            Err(e) if e == EAGAIN => return Ok(Action::Idle),
            Err(e) if e == ENOMEM => (old, true),
            Err(e) => return Err(e),
        };
        let command = owner
            .reply(&body, self.pools[index].counter, !refused)
            .ok_or(EIO)?;
        vm.write(memory, 2, self.command.ring + slot as u64 * 0x40, &command)?;
        // Command body -> report credit -> barrier -> command producer ->
        // barrier -> caller's doorbell. Match the source service ordering.
        self.consume(memory, vm, next)?;
        vm.write(
            memory,
            2,
            self.command.states[2],
            &((slot + 1) & 255).to_le_bytes(),
        )?;
        g17p_memory::sync();
        let counter = self.pools[index].counter;
        self.pools[index].counter += 1;
        self.pools[index].refused = refused;
        Ok(Action::Reply {
            pool: self.pools[index].identity.pool,
            vm: self.pools[index].identity.vm,
            counter,
            old,
            new,
            refused,
        })
    }
}
