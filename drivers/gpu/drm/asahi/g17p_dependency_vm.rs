// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Low-root joins from G17PShimBackend._ensure_dependency_compute_state.
//! The caller supplies live, owned leaves, not the bootstrap's PA inventory.
//! All four queues must be quiescent; no root or producer is published here.

const PAGE: u64 = 0x4000;
const ADDRESS: u64 = 0x000003ffffffc000;
const PRIVATE_FLAGS: u64 = 0x00c0000000000c8b;
pub(crate) const ACTIVE_RENDER: [u64; 20] = [
    0x7000000000,
    0x7000098000,
    0x7000460000,
    0x7000464000,
    0x7000468000,
    0x700046c000,
    0x7000470000,
    0x7000474000,
    0x7000478000,
    0x700047c000,
    0x7000488000,
    0x700048c000,
    0x7000490000,
    0x7000494000,
    0x7000498000,
    0x700049c000,
    0x70004a0000,
    0x70004a4000,
    0x70017e0000,
    0x70019e8000,
];

#[derive(Clone, Copy)]
pub(crate) enum Root {
    Compute,
    Render,
}
pub(crate) trait Space {
    type Error;
    fn pte(&self, root: Root, address: u64) -> core::result::Result<u64, Self::Error>;
    fn caller_overlap(&self, address: u64) -> bool;
    /// Replace only a quiescent, driver-owned leaf, preserving its PA owner.
    fn replace(&mut self, address: u64, pte: u64) -> core::result::Result<(), Self::Error>;
    /// The physical owner must be retained even if a later operation fails.
    fn zero_page(&mut self) -> core::result::Result<u64, Self::Error>;
}
#[derive(Debug)]
pub(crate) enum Error<E> {
    Invalid,
    Unmapped(u64),
    Overlap(u64),
    Access(E),
}
type Result<T, E> = core::result::Result<T, Error<E>>;

fn address<E>(va: u64) -> Result<(), E> {
    if va >= 1 << 42 || va & (PAGE - 1) != 0 {
        Err(Error::Invalid)
    } else {
        Ok(())
    }
}
fn leaf<S: Space>(space: &S, root: Root, va: u64) -> Result<u64, S::Error> {
    address(va)?;
    let pte = space.pte(root, va).map_err(Error::Access)?;
    if pte != 0
        && (pte & 3 != 3 || pte & ADDRESS == 0 || pte & !(ADDRESS | 0x00c0000000000fff) != 0)
    {
        return Err(Error::Invalid);
    }
    Ok(pte)
}
fn owned<S: Space>(space: &S, root: Root, va: u64) -> Result<u64, S::Error> {
    let pte = leaf(space, root, va)?;
    if pte == 0 {
        Err(Error::Unmapped(va))
    } else {
        Ok(pte)
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct Join {
    pub(crate) imported: usize,
    pub(crate) shared: usize,
    pub(crate) active_replaced: usize,
    pub(crate) dormant_collisions: usize,
}
fn import<S: Space>(space: &mut S, va: u64, active: bool, join: &mut Join) -> Result<(), S::Error> {
    let render = owned(space, Root::Render, va)?;
    let compute = leaf(space, Root::Compute, va)?;
    // Source iotranslate compares the PA, retaining existing compute attributes
    // when both roots already refer to the same backing.
    if compute & ADDRESS == render & ADDRESS {
        join.shared += 1;
    } else if compute != 0 && !active {
        join.dormant_collisions += 1;
    } else {
        if compute != 0 {
            if space.caller_overlap(va) {
                return Err(Error::Overlap(va));
            }
            join.active_replaced += 1;
        } else {
            join.imported += 1;
        }
        space.replace(va, render).map_err(Error::Access)?;
    }
    Ok(())
}

/// `inventory` is the sorted set of retained render-owned DVAs. Its old PA
/// values are intentionally absent. Support outside that set is read live too.
pub(crate) fn join_render<S: Space>(space: &mut S, inventory: &[u64]) -> Result<Join, S::Error> {
    // Validate the complete inventory and all active owners before replacing
    // anything. A stale or malformed inventory must not partially join roots.
    for (i, &va) in inventory.iter().enumerate() {
        if i != 0 && inventory[i - 1] >= va {
            return Err(Error::Invalid);
        }
        owned(space, Root::Render, va)?;
        let old = leaf(space, Root::Compute, va)?;
        if ACTIVE_RENDER.contains(&va)
            && old != 0
            && old & ADDRESS != owned(space, Root::Render, va)? & ADDRESS
            && space.caller_overlap(va)
        {
            return Err(Error::Overlap(va));
        }
    }
    for va in ACTIVE_RENDER {
        let render = owned(space, Root::Render, va)?;
        let old = leaf(space, Root::Compute, va)?;
        if old != 0 && old & ADDRESS != render & ADDRESS && space.caller_overlap(va) {
            return Err(Error::Overlap(va));
        }
    }
    let mut join = Join::default();
    for &va in inventory {
        import(space, va, ACTIVE_RENDER.contains(&va), &mut join)?;
    }
    for va in ACTIVE_RENDER {
        if inventory.binary_search(&va).is_err() {
            import(space, va, true, &mut join)?;
        }
    }
    Ok(join)
}

fn private_alias<S: Space>(space: &mut S, va: u64, pa: u64) -> Result<(), S::Error> {
    address(va)?;
    if pa == 0 || pa & !ADDRESS != 0 {
        return Err(Error::Invalid);
    }
    let old = leaf(space, Root::Compute, va)?;
    if old & ADDRESS == pa {
        return Ok(());
    }
    if old != 0 && space.caller_overlap(va) {
        return Err(Error::Overlap(va));
    }
    space.replace(va, pa | PRIVATE_FLAGS).map_err(Error::Access)
}

/// Alias the source's two live five-page CL state blocks in source order.
/// Opening replaces STATE_BASE before closing reads that same live address.
/// Snapshotting both sources before that replacement changes source behavior.
pub(crate) fn join_compute<S: Space>(space: &mut S, cdms: [u64; 2]) -> Result<[u64; 2], S::Error> {
    for cdm in cdms {
        address(cdm)?;
        // This admitted native launch uses one caller-owned CDM page, with its
        // already-owned Tier-2 resource page exactly 0x30000 bytes after it.
        owned(space, Root::Compute, cdm)?;
        owned(
            space,
            Root::Compute,
            cdm.checked_add(0x30000).ok_or(Error::Invalid)?,
        )?;
    }
    for (source, destination) in [(0x70017e0000, 0x7000220000), (0x7000220000, 0x70035d8000)] {
        for offset in (0..0x14000).step_by(PAGE as usize) {
            let pa = owned(space, Root::Compute, source + offset)? & ADDRESS;
            private_alias(space, destination + offset, pa)?;
        }
    }
    let robustness = [
        space.zero_page().map_err(Error::Access)?,
        space.zero_page().map_err(Error::Access)?,
    ];
    if robustness[0] == robustness[1] {
        return Err(Error::Invalid);
    }
    for (va, pa) in [0x1000078000, 0x1000238000].into_iter().zip(robustness) {
        private_alias(space, va, pa)?;
    }
    // The two Tier-2 inputs already occupy their final caller DVAs. Only the
    // opening CDM needs the source's fixed alias; an occupied different owner
    // must reject rather than overwrite a compiler image or caller BO.
    let pa = owned(space, Root::Compute, cdms[0])? & ADDRESS;
    let alias = 0x100000b0000;
    let old = leaf(space, Root::Compute, alias)?;
    if old & ADDRESS != pa {
        if old != 0 {
            return Err(Error::Overlap(alias));
        }
        space
            .replace(alias, pa | PRIVATE_FLAGS)
            .map_err(Error::Access)?;
    }
    Ok(robustness)
}
