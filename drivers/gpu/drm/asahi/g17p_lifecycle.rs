// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! g17p_lifecycle.py cleanup encodings and bounded publication/receipt gates.
//! Consumption retains every resource and does not qualify physical reclaim.

use super::g17p_compute::{u32_at, u64_at};
use kernel::{
    bindings,
    dma_fence::{Fence, RawDmaFence},
    prelude::*,
};

pub(crate) fn build_context_cleanup(control: u64, incarnation: u32) -> Result<[u8; 0x40]> {
    if control == 0 || control & 0x3f != 0 || incarnation > 1 {
        return Err(EINVAL);
    }
    let mut body = [0; 0x40];
    u32_at(&mut body, 0, 0x14);
    u32_at(&mut body, 8, 0x04000002 | incarnation << 16);
    u64_at(&mut body, 0xc, control);
    Ok(body)
}
pub(crate) fn build_live_context_cleanup(control: u64, data: &[u8; 0x40]) -> Result<[u8; 0x40]> {
    if data[0x26] != 2 || data[4] != 4 || data[0] >= 64 || data[1] >= 64 {
        return Err(EINVAL);
    }
    let mut body = build_context_cleanup(control, 0)?;
    body[8..12].copy_from_slice(&[data[0x26], data[0], data[1], data[4]]);
    Ok(body)
}
pub(crate) fn build_post_cleanup_control(kind: u32, phase: u32) -> Result<[u8; 0x40]> {
    if kind > 2 || !(1..=2).contains(&phase) {
        return Err(EINVAL);
    }
    let mut body = [0; 0x40];
    for (at, value) in [(0, 0x1b), (4, kind), (8, phase)] {
        u32_at(&mut body, at, value);
    }
    Ok(body)
}

/// These callbacks correspond to the Python live reader, counters, publish
/// and notify closures. They use the retained kernel Session, never a proxy.
pub(crate) trait Host {
    fn read_control(&mut self, control: u64) -> Result<[u8; 0x40]>;
    fn counters(&mut self) -> Result<[u32; 3]>;
    fn publish(&mut self, body: &[u8; 0x40]) -> Result<Publication>;
    fn notify(&mut self) -> Result;
}
#[derive(Clone, Copy)]
pub(crate) struct Publication {
    pub(crate) before: [u32; 3],
    pub(crate) target: u32,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    WaitingForWork,
    Publishing,
    Published,
    Consumed,
    Quarantined,
}

fn status(fence: &Fence) -> i32 {
    // SAFETY: The receipt owns this live fence reference for the full read.
    unsafe { bindings::dma_fence_get_status(fence.raw()) }
}
fn counters(values: [u32; 3]) -> Result {
    if values.iter().any(|&v| v >= 256) || values[..2].iter().any(|&v| v > values[2]) {
        return Err(EIO);
    }
    Ok(())
}

pub(crate) struct ContextCleanup {
    pub(crate) control: u64,
    pub(crate) fences: KVec<Fence>,
    pub(crate) resources: KVec<u64>,
    pub(crate) state: State,
    pub(crate) body: Option<[u8; 0x40]>,
    pub(crate) publication: Option<Publication>,
    pub(crate) failure: Option<Error>,
    post: Option<(u32, u32)>,
}
impl ContextCleanup {
    pub(crate) fn new(control: u64, fences: KVec<Fence>, resources: KVec<u64>) -> Result<Self> {
        build_context_cleanup(control, 0)?;
        if fences.is_empty() || resources.is_empty() {
            return Err(EINVAL);
        }
        Ok(Self {
            control,
            fences,
            resources,
            state: State::WaitingForWork,
            body: None,
            publication: None,
            failure: None,
            post: None,
        })
    }
    pub(crate) fn resources_retired(&self) -> bool {
        self.fences.iter().all(|f| status(f) > 0)
    }
    pub(crate) fn step<H: Host>(&mut self, host: &mut H) -> Result<bool> {
        if self.state == State::Consumed || self.state == State::Quarantined {
            return Ok(self.state == State::Consumed);
        }
        let result = self.step_inner(host);
        if let Err(error) = result {
            self.failure = Some(error);
            self.state = State::Quarantined;
        }
        result
    }
    fn step_inner<H: Host>(&mut self, host: &mut H) -> Result<bool> {
        if self.fences.iter().any(|f| status(f) < 0) {
            return Err(EIO);
        }
        if self.state == State::WaitingForWork {
            if !self.resources_retired() {
                return Ok(false);
            }
            let before = host.counters()?;
            counters(before)?;
            if before[2] == 255 || before[..2].iter().any(|&head| head == before[2] + 1) {
                return Ok(false);
            }
            let body = if let Some((kind, phase)) = self.post {
                build_post_cleanup_control(kind, phase)?
            } else {
                build_live_context_cleanup(self.control, &host.read_control(self.control)?)?
            };
            self.body = Some(body);
            self.state = State::Publishing;
            let publication = host.publish(&body)?;
            self.publication = Some(publication);
            if publication.before != before || publication.target != before[2] + 1 {
                return Err(EIO);
            }
            self.state = State::Published;
            host.notify()?;
            return Ok(false);
        }
        if self.state != State::Published {
            return Err(EIO);
        }
        let current = host.counters()?;
        counters(current)?;
        let publication = self.publication.ok_or(EIO)?;
        if current
            .iter()
            .zip(publication.before)
            .any(|(&now, old)| now < old)
        {
            return Err(EIO);
        }
        if current[..2].iter().all(|&head| head >= publication.target) {
            self.state = State::Consumed;
            return Ok(true);
        }
        Ok(false)
    }
    fn post_record(cleanup: &Self, kind: u32, phase: u32) -> Result<Self> {
        let mut fences = KVec::with_capacity(cleanup.fences.len(), GFP_KERNEL)?;
        for fence in &cleanup.fences {
            fences.push(fence.clone(), GFP_KERNEL)?;
        }
        let mut resources = KVec::new();
        resources.extend_from_slice(&cleanup.resources, GFP_KERNEL)?;
        let mut record = Self::new(cleanup.control, fences, resources)?;
        record.post = Some((kind, phase));
        Ok(record)
    }
}
pub(crate) struct PostCleanupMaintenance {
    pub(crate) records: KVec<ContextCleanup>,
    pub(crate) index: usize,
    pub(crate) state: State,
    pub(crate) failure: Option<Error>,
}
impl PostCleanupMaintenance {
    pub(crate) fn new(cleanup: &ContextCleanup) -> Result<Self> {
        if cleanup.state != State::Consumed || !cleanup.resources_retired() {
            return Err(EBUSY);
        }
        let mut records = KVec::with_capacity(6, GFP_KERNEL)?;
        for kind in 0..3 {
            for phase in 1..=2 {
                records.push(
                    ContextCleanup::post_record(cleanup, kind, phase)?,
                    GFP_KERNEL,
                )?;
            }
        }
        Ok(Self {
            records,
            index: 0,
            state: State::WaitingForWork,
            failure: None,
        })
    }
    pub(crate) fn step<H: Host>(&mut self, host: &mut H) -> Result<bool> {
        if self.state == State::Consumed || self.state == State::Quarantined {
            return Ok(self.state == State::Consumed);
        }
        match self.records.get_mut(self.index).ok_or(EIO)?.step(host) {
            Ok(true) => {
                self.index += 1;
                if self.index == self.records.len() {
                    self.state = State::Consumed;
                }
                Ok(self.state == State::Consumed)
            }
            Ok(false) => Ok(false),
            Err(error) => {
                self.state = State::Quarantined;
                self.failure = Some(error);
                Err(error)
            }
        }
    }
}

// Source diagnostic migration routines. The host only admits RAM retained by
// the kernel's source graph and UAT table owners; code/data leaves are never
// read by the alias walker.
const PAGE: u64 = 0x4000;
const ADDRESS: u64 = 0x000003ffffffc000;
#[derive(Clone, Copy)]
pub(crate) struct AliasMove {
    pub(crate) address: u64,
    pub(crate) before: u64,
    pub(crate) after: u64,
}
pub(crate) struct Relocation {
    pub(crate) old_pa: u64,
    pub(crate) new_pa: u64,
    pub(crate) aliases: KVec<AliasMove>,
    pub(crate) roots: KVec<u64>,
    pub(crate) table_pages: usize,
}
pub(crate) trait RelocationHost {
    fn firmware_root(&self) -> u64;
    fn compute_root(&self) -> u64;
    fn ttbs(&self) -> Result<[u64; 128]>;
    fn leaf(&self, group: usize, va: u64) -> Result<(u64, u64)>;
    fn table_word(&self, address: u64) -> Result<u64>;
    fn copy_page(&mut self, pa: u64) -> Result<u64>;
    fn publish_aliases(&mut self, aliases: &[AliasMove]) -> Result;
    fn publish_compute_root(&mut self, before: &[u64; 128]) -> Result<u64>;
}
fn qualified(cleanup: &ContextCleanup, resources: &[u64]) -> Result {
    if cleanup.state != State::Consumed
        || !cleanup.resources_retired()
        || resources
            .iter()
            .any(|resource| !cleanup.resources.contains(resource))
    {
        return Err(EBUSY);
    }
    Ok(())
}
pub(crate) fn relocate_completed_compute_page<H: RelocationHost>(
    host: &mut H,
    cleanup: &ContextCleanup,
    high: u64,
    low: u64,
) -> Result<Relocation> {
    qualified(cleanup, &[high, low])?;
    if (high | low) & (PAGE - 1) != 0 {
        return Err(EINVAL);
    }
    let high_leaf = host.leaf(2, high)?;
    let low_leaf = host.leaf(0, low)?;
    let old_pa = high_leaf.1 & ADDRESS;
    if old_pa == 0 || low_leaf.1 & ADDRESS != old_pa {
        return Err(EIO);
    }
    let new_pa = host.copy_page(old_pa)?;
    let mut aliases = KVec::with_capacity(2, GFP_KERNEL)?;
    aliases.push(
        AliasMove {
            address: high_leaf.0,
            before: high_leaf.1,
            after: new_pa | 0x00c0000000000443,
        },
        GFP_KERNEL,
    )?;
    aliases.push(
        AliasMove {
            address: low_leaf.0,
            before: low_leaf.1,
            after: new_pa | 0x0080000000000c8b,
        },
        GFP_KERNEL,
    )?;
    host.publish_aliases(&aliases)?;
    if host.leaf(2, high)?.1 & ADDRESS != new_pa || host.leaf(0, low)?.1 & ADDRESS != new_pa {
        return Err(EIO);
    }
    Ok(Relocation {
        old_pa,
        new_pa,
        aliases,
        roots: KVec::new(),
        table_pages: 0,
    })
}
pub(crate) fn relocate_completed_compute_descriptor<H: RelocationHost>(
    host: &mut H,
    cleanup: &ContextCleanup,
    high: u64,
    low: u64,
) -> Result<Relocation> {
    relocate_completed_compute_page(host, cleanup, high, low)
}
pub(crate) fn relocate_completed_transport_page<H: RelocationHost>(
    host: &mut H,
    cleanup: &ContextCleanup,
    high: u64,
) -> Result<Relocation> {
    qualified(cleanup, &[high])?;
    if high & (PAGE - 1) != 0 {
        return Err(EINVAL);
    }
    let old_pa = host.leaf(2, high)?.1 & ADDRESS;
    if old_pa == 0 {
        return Err(EIO);
    }
    let before = host.ttbs()?;
    let mut roots = KVec::new();
    roots.push(host.firmware_root(), GFP_KERNEL)?;
    for value in before {
        if value & 1 != 0 && !roots.contains(&(value & ADDRESS)) {
            roots.push(value & ADDRESS, GFP_KERNEL)?;
        }
    }
    roots.sort_unstable();
    let mut visited: KVec<(u64, u32)> = KVec::new();
    let mut active = KVec::new();
    let mut aliases = KVec::new();
    fn walk<H: RelocationHost>(
        host: &H,
        pa: u64,
        depth: u32,
        old: u64,
        visited: &mut KVec<(u64, u32)>,
        active: &mut KVec<u64>,
        aliases: &mut KVec<AliasMove>,
    ) -> Result {
        if active.contains(&pa)
            || visited
                .iter()
                .any(|&(at, level)| at == pa && level != depth)
        {
            return Err(EIO);
        }
        if visited.iter().any(|&(at, _)| at == pa) {
            return Ok(());
        }
        if pa == 0 || pa & (PAGE - 1) != 0 || visited.len() >= 2048 || depth > 2 {
            return Err(EIO);
        }
        active.push(pa, GFP_KERNEL)?;
        visited.push((pa, depth), GFP_KERNEL)?;
        for index in 0..if depth == 0 { 64 } else { 2048 } {
            let address = pa + index * 8;
            let value = host.table_word(address)?;
            if value & 1 == 0 {
                continue;
            }
            if value & 3 != 3 {
                return Err(EIO);
            }
            if depth == 2 {
                if value & ADDRESS == old {
                    aliases.push(
                        AliasMove {
                            address,
                            before: value,
                            after: 0,
                        },
                        GFP_KERNEL,
                    )?;
                }
            } else {
                walk(
                    host,
                    value & ADDRESS,
                    depth + 1,
                    old,
                    visited,
                    active,
                    aliases,
                )?;
            }
        }
        active.pop();
        Ok(())
    }
    for &root in &roots {
        walk(
            host,
            root,
            0,
            old_pa,
            &mut visited,
            &mut active,
            &mut aliases,
        )?;
    }
    if aliases.is_empty() || visited.iter().any(|&(pa, _)| pa == old_pa) {
        return Err(EIO);
    }
    for alias in &aliases {
        if host.table_word(alias.address)? != alias.before {
            return Err(EIO);
        }
    }
    let new_pa = host.copy_page(old_pa)?;
    for alias in &mut aliases {
        alias.after = (alias.before & !ADDRESS) | new_pa;
    }
    host.publish_aliases(&aliases)?;
    if host.ttbs()? != before {
        return Err(EIO);
    }
    Ok(Relocation {
        old_pa,
        new_pa,
        aliases,
        roots,
        table_pages: visited.len(),
    })
}
pub(crate) fn relocate_completed_compute_root<H: RelocationHost>(
    host: &mut H,
    cleanup: &ContextCleanup,
) -> Result<Relocation> {
    let old_pa = host.compute_root();
    qualified(cleanup, &[old_pa])?;
    if old_pa == 0 || old_pa & (PAGE - 1) != 0 || old_pa == host.firmware_root() {
        return Err(EINVAL);
    }
    let before = host.ttbs()?;
    let mut references = KVec::new();
    for (index, &value) in before.iter().enumerate() {
        if value & 1 != 0 && value & ADDRESS == old_pa {
            references.push(index, GFP_KERNEL)?;
        }
    }
    if references.as_slice() != [4, 6] || before[4] >> 48 != 2 || before[6] >> 48 != 3 {
        return Err(EIO);
    }
    let new_pa = host.publish_compute_root(&before)?;
    let mut expected = before;
    for index in [4, 6] {
        expected[index] = (before[index] & !ADDRESS) | new_pa;
    }
    if host.ttbs()? != expected || host.compute_root() != new_pa {
        return Err(EIO);
    }
    Ok(Relocation {
        old_pa,
        new_pa,
        aliases: KVec::new(),
        roots: KVec::new(),
        table_pages: 1,
    })
}
