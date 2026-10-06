// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! CPU access to the validated, permanently reserved GPU boot RAM.
//! Does not treat firmware physical addresses as kernel direct-map pointers.

use super::{g17p_platform::Platform, g17p_topology};
use kernel::{bindings, c_str, device, io::mem, io::resource::Resource, page, prelude::*, rbtree::{RBTree, RBTreeNodeReservation}, sync::Arc};

const PAGE: usize = 0x4000;

struct Allocation {
    page: *mut bindings::page,
    base: u64,
    size: usize,
    order: u32,
}

// SAFETY: Allocation metadata is immutable. Page access is performed only
// through checked spans whose runtime owner excludes overlapping GPU/CPU use.
unsafe impl Send for Allocation {}
unsafe impl Sync for Allocation {}

/// Pins allocated scratch backing for a CPU task outside the session lock.
/// Construction checks physical ownership; the render preparation reservation
/// separately proves this exact span has no live firmware reader or writer.
pub(crate) struct ZeroSpan {
    _allocation: Arc<Allocation>,
    address: u64,
    size: usize,
}
impl ZeroSpan {
    pub(crate) fn clear(&mut self) {
        let base = self.address & !(PAGE as u64 - 1);
        // SAFETY: The Arc retains the allocation and the checked one-page span.
        let page = unsafe { page::Page::borrow_phys_unchecked(&base) };
        page.with_page_mapped(|p| {
            // SAFETY: zero_span checked offset and size within this page.
            let start = unsafe { p.add((self.address - base) as usize) };
            sync();
            for at in ((start as usize) & !63..start as usize + self.size).step_by(64) {
                // SAFETY: Checked cache lines lie in this owned RAM page.
                unsafe { core::arch::asm!("dc ivac, {at}", at=in(reg)at, options(nostack,preserves_flags)); }
            }
            sync();
            // SAFETY: The selected render reservation exclusively owns these
            // retired scratch bytes until publication revalidation completes.
            unsafe { start.write_bytes(0, self.size); }
            for at in ((start as usize) & !63..start as usize + self.size).step_by(64) {
                // SAFETY: Same owned RAM lines as above; publish the zeroes.
                unsafe { core::arch::asm!("dc civac, {at}", at=in(reg)at, options(nostack,preserves_flags)); }
            }
            sync();
        });
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: This owner retains the original allocation and its order.
        // Session only drops memory after the firmware has stopped.
        unsafe { bindings::__free_pages(self.page, self.order) };
    }
}

struct Mapping {
    base: u64,
    memory: mem::Mem,
}

pub(crate) struct Memory {
    mappings: KVec<Mapping>,
    // Match the M1/M2/M4 firmware heap policy: backing only grows while the
    // session is live. Retiring objects or reusing ring slots never removes
    // allocations here. Pages are released only after session shutdown.
    allocations: KVec<Arc<Allocation>>,
    // Exclusive physical ends index the permanently retained, disjoint
    // allocations. Access checks remain exact while avoiding a heap scan.
    allocation_ends: RBTree<u64, usize>,
    prepared_blocks: KVec<(usize,u64)>,
    prepared_growth: bool,
    growth_refused: bool,
    growth_waiting: bool,
}

/// A checked aligned word in RAM retained by this Memory owner. The immutable
/// borrow prevents allocations or mappings changing during a publication pass.
pub(crate) struct Word64<'a> {
    backing: WordBacking,
    _owner: core::marker::PhantomData<&'a Memory>,
}
enum WordBacking {
    Allocated(u64),
    Reserved(*mut u64),
}
impl Word64<'_> {
    pub(crate) fn load(&self) -> u64 {
        let read = |p: *mut u64| {
            sync();
            // SAFETY: Constructor checked the live owner's aligned RAM word.
            unsafe {
                core::arch::asm!("dc ivac, {p}", p=in(reg)p, options(nostack,preserves_flags));
            }
            sync();
            // SAFETY: The borrowed owner retains this readable word.
            u64::from_le(unsafe { p.read_volatile() })
        };
        match self.backing {
            WordBacking::Allocated(address) => {
                let base = address & !(PAGE as u64 - 1);
                // SAFETY: word64 checked ownership of the physical page.
                let page = unsafe { page::Page::borrow_phys_unchecked(&base) };
                page.with_page_mapped(|p| {
                    // SAFETY: The checked offset lies wholly within the page.
                    read(unsafe { p.add((address - base) as usize).cast::<u64>() })
                })
            }
            WordBacking::Reserved(pointer) => read(pointer),
        }
    }
    /// No allocation, address lookup, or fallible operation after the first
    /// page-table break. Only checked, host-owned PTE words use this API.
    pub(crate) fn store(&self, value: u64) {
        let write = |p: *mut u64| {
            sync();
            // SAFETY: Constructor checked alignment, bounds and RAM ownership.
            // These table lines contain cleaned host-owned entries; preserve
            // adjacent entries before updating and publishing this one word.
            unsafe {
                core::arch::asm!("dc ivac, {p}", p=in(reg)p, options(nostack,preserves_flags));
            }
            sync();
            // SAFETY: The borrowed owner retains the checked writable word.
            unsafe {
                p.write_volatile(value.to_le());
                core::arch::asm!("dc civac, {p}", p=in(reg)p, options(nostack,preserves_flags));
            }
            sync();
        };
        match self.backing {
            WordBacking::Allocated(address) => {
                let base = address & !(PAGE as u64 - 1);
                // SAFETY: word64 verified this page is in a retained allocation.
                let page = unsafe { page::Page::borrow_phys_unchecked(&base) };
                page.with_page_mapped(|p| {
                    // SAFETY: Checked aligned word lies wholly in this page.
                    write(unsafe { p.add((address - base) as usize).cast::<u64>() });
                });
            }
            WordBacking::Reserved(pointer) => write(pointer),
        }
    }
}

// SAFETY: These mappings are not thread-local. Only this owner accesses them;
// writes require exclusive access. No reference into shared firmware RAM escapes.
unsafe impl Send for Memory {}

impl Memory {
    fn owns_allocation(&self, address: u64, end: u64) -> bool {
        // The first allocation ending at or beyond this span is the only
        // possible owner: the allocator's retained physical ranges disjoint.
        self.allocation_ends.cursor_lower_bound(&end).is_some_and(|cursor| {
            let allocation = &self.allocations[*cursor.current().1];
            address >= allocation.base && end <= allocation.base + allocation.size as u64
        })
    }

    pub(crate) fn word64(&self, address: u64) -> Result<Word64<'_>> {
        if address & 7 != 0 {
            return Err(EINVAL);
        }
        let end = address.checked_add(8).ok_or(EINVAL)?;
        let backing = if self.owns_allocation(address, end)
        {
            WordBacking::Allocated(address)
        } else {
            let mapping = self
                .mappings
                .iter()
                .find(|m| address >= m.base && end <= m.base + m.memory.size() as u64)
                .ok_or(EINVAL)?;
            // SAFETY: Checked ordinary reserved RAM mapping remains live for
            // the full immutable owner borrow carried by the returned word.
            WordBacking::Reserved(unsafe {
                mapping
                    .memory
                    .ptr()
                    .add((address - mapping.base) as usize)
                    .cast::<u64>()
            })
        };
        Ok(Word64 {
            backing,
            _owner: core::marker::PhantomData,
        })
    }

    pub(crate) fn zero_span(&self, address: u64, size: usize) -> Result<ZeroSpan> {
        if size == 0 || (address as usize & (PAGE - 1)) + size > PAGE { return Err(EINVAL); }
        let end = address.checked_add(size as u64).ok_or(EOVERFLOW)?;
        let cursor = self.allocation_ends.cursor_lower_bound(&end).ok_or(EINVAL)?;
        let allocation = &self.allocations[*cursor.current().1];
        if address < allocation.base || end > allocation.base + allocation.size as u64 { return Err(EINVAL); }
        Ok(ZeroSpan { _allocation: allocation.clone(), address, size })
    }

    /// Private, unpublished storage can be constructed by an independent
    /// worker without borrowing the session heap or its shared mappings.
    pub(crate) fn detached() -> Self {
        Self { mappings: KVec::new(), allocations: KVec::new(), allocation_ends: RBTree::new(),
            prepared_blocks: KVec::new(), prepared_growth: false, growth_refused: false, growth_waiting: false }
    }
    /// Transfer all backing before publishing a root that references it.
    /// Retain physical pages for the rest of the session, as with its other
    /// GPU allocations. Reuse the detached ownership-index nodes verbatim.
    pub(crate) fn absorb(&mut self, mut other: Self) -> Result {
        if !other.mappings.is_empty() { return Err(EINVAL); }
        self.prepared_blocks.reserve(other.prepared_blocks.len(), GFP_KERNEL)?;
        let offset = self.allocations.len();
        offset.checked_add(other.allocations.len()).ok_or(EOVERFLOW)?;
        self.allocations.reserve(other.allocations.len(), GFP_KERNEL)?;
        for allocation in other.allocations.drain(..) {
            self.allocations.push(allocation, GFP_KERNEL).expect("reserved allocation transfer");
        }
        for block in other.prepared_blocks.drain(..) {
            self.prepared_blocks.push(block, GFP_KERNEL).expect("reserved prepared-block transfer");
        }
        while let Some(cursor) = other.allocation_ends.cursor_front_mut() {
            let (&end, &index) = cursor.current();
            let (_, node) = cursor.remove_current();
            self.allocation_ends.insert(node.into_reservation().into_node(end, index + offset));
        }
        Ok(())
    }

    pub(crate) fn new(dev: &device::Device, platform: &Platform) -> Result<Self> {
        let node = dev.of_node().ok_or(ENODEV)?;
        let mut memory = Self {
            mappings: KVec::new(),
            allocations: KVec::new(),
            allocation_ends: RBTree::new(),
            prepared_blocks: KVec::new(), prepared_growth: false, growth_refused: false, growth_waiting: false,
        };
        for (index, name) in [
            c_str!("ttbs"),
            c_str!("pagetables"),
            c_str!("l2"),
            c_str!("handoff"),
        ]
        .iter()
        .enumerate()
        {
            let res = node.reserved_mem_region_to_resource_byname(name)?;
            let expected = platform.regions[index];
            memory.map(res, expected.base, expected.size)?;
        }
        let source = node
            .parse_phandle(c_str!("memory-region"), 6)
            .ok_or(ENODEV)?;
        for (index, &(base, size)) in g17p_topology::RESERVATIONS.iter().enumerate() {
            memory.map(source.address_to_resource(index)?, base, size)?;
        }
        dev_info!(
            dev,
            "G17P: mapped {} reserved RAM ranges for bounded CPU access\n",
            memory.mappings.len()
        );
        Ok(memory)
    }

    fn map(&mut self, resource: Resource, base: u64, size: u64) -> Result {
        if resource.start() as u64 != base || resource.size() as u64 != size {
            return Err(EINVAL);
        }
        // SAFETY: Platform validated this no-map RAM reservation. It is not
        // MMIO or Linux allocator memory. Mapping alone initiates no DMA, and
        // it creates no CPU alias with different cache attributes.
        let memory = unsafe { mem::Mem::try_new(resource, mem::MemFlag::WB.into())? };
        self.mappings.push(Mapping { base, memory }, GFP_KERNEL)?;
        Ok(())
    }

    /// Access is scoped to a mapping, including on kernels with local mappings.
    /// No pointer from the callback may be retained after it returns.
    fn access<T>(&self, address: u64, size: usize, f: impl FnOnce(*mut u8) -> T) -> Result<T> {
        let end = address.checked_add(size as u64).ok_or(EINVAL)?;
        if size == 0 || (address as usize & (PAGE - 1)) + size > PAGE {
            return Err(EINVAL);
        }
        if self.owns_allocation(address, end)
        {
            let base = address & !(PAGE as u64 - 1);
            // SAFETY: The allocation above owns this System RAM page for the
            // entire callback, and no pointer or reference escapes access.
            let page = unsafe { page::Page::borrow_phys_unchecked(&base) };
            return Ok(page.with_page_mapped(|p| {
                // SAFETY: The checked access fits this one mapped page.
                f(unsafe { p.add((address - base) as usize) })
            }));
        }
        let mapping = self
            .mappings
            .iter()
            .find(|mapping| {
                address >= mapping.base && end <= mapping.base + mapping.memory.size() as u64
            })
            .ok_or(EINVAL)?;
        // SAFETY: The range above is entirely within this live mapping.
        Ok(f(unsafe {
            mapping.memory.ptr().add((address - mapping.base) as usize)
        }))
    }

    pub(crate) fn prepared_count(&self, size: usize) -> usize {
        self.prepared_blocks.iter().filter(|(bytes,_)| *bytes == size).count()
    }
    pub(crate) fn prepare_blocks(&mut self, size: usize, count: usize) -> Result {
        self.prepared_blocks.reserve(count, GFP_KERNEL)?;
        for _ in 0..count {
            // Newly allocated blocks are zeroed and cleaned off-lock. Only
            // the ownership transfer and checked PTE publication remain.
            let pa = self.allocate_new(size)?;
            self.clean(pa, size)?;
            self.prepared_blocks.push((size,pa), GFP_KERNEL)?;
        }
        Ok(())
    }
    pub(crate) fn prepare_growth_mode(&mut self) { self.prepared_growth = true; self.growth_waiting = false; }
    pub(crate) fn refuse_growth(&mut self, refused: bool) { self.growth_refused = refused; }
    pub(crate) fn growth_waiting(&self) -> bool { self.growth_waiting }
    pub(crate) fn allocate(&mut self, size: usize) -> Result<u64> {
        if let Some(index) = self.prepared_blocks.iter().position(|(bytes,_)| *bytes == size) {
            return Ok(self.prepared_blocks.remove(index).map_err(|_| EIO)?.1);
        }
        if self.prepared_growth && size == super::g17p_growth::INCREMENT * super::g17p_growth::BLOCK as usize {
            if self.growth_refused { return Err(ENOMEM); }
            self.growth_waiting = true;
            return Err(EAGAIN);
        }
        self.allocate_new(size)
    }
    fn allocate_new(&mut self, size: usize) -> Result<u64> {
        if page::PAGE_SIZE != PAGE || size == 0 || size % PAGE != 0 {
            return Err(EINVAL);
        }
        let pages = (size / PAGE).checked_next_power_of_two().ok_or(EINVAL)?;
        let order = pages.trailing_zeros();
        self.allocations.reserve(1, GFP_KERNEL)?;
        let node = RBTreeNodeReservation::new(GFP_KERNEL)?;
        // SAFETY: Standard allocator call; the resulting owner frees exactly
        // this order. All pages are cleared before any firmware publication.
        // GPU backing has a normal ENOMEM path. Do not invoke the OOM
        // killer while immutable snapshots pin every candidate page; this
        // remains true for speculative off-lock private preparations.
        let flags = bindings::GFP_KERNEL | bindings::__GFP_NOWARN
            | (1 << bindings::___GFP_NORETRY_BIT);
        let ptr = unsafe { bindings::alloc_pages(flags, order) };
        if ptr.is_null() {
            return Err(ENOMEM);
        }
        // SAFETY: Successful alloc_pages returned a live page descriptor.
        let base = unsafe { bindings::page_to_phys(ptr) };
        self.allocations.push(
            Arc::new(Allocation {
                page: ptr,
                base,
                size: pages * PAGE,
                order,
            }, GFP_KERNEL)?,
            GFP_KERNEL,
        )?;
        // Node and vector capacity were reserved before acquiring pages;
        // installing this ownership index cannot fail or release backing.
        self.allocation_ends.insert(node.into_node(base + (pages * PAGE) as u64, self.allocations.len() - 1));
        self.zero(base, pages * PAGE)?;
        Ok(base)
    }

    pub(crate) fn read64(&self, address: u64) -> Result<u64> {
        if address & 7 != 0 {
            return Err(EINVAL);
        }
        // SAFETY: The checked address names aligned ordinary RAM. Volatile
        // access avoids assuming the firmware leaves the value unchanged.
        self.access(address, 8, |p| {
            u64::from_le(unsafe { p.cast::<u64>().read_volatile() })
        })
    }

    /// Observe a firmware-owned counter after discarding the CPU's clean copy.
    /// Callers must have cleaned every host write in the containing cache line.
    pub(crate) fn read_firmware32(&self, address: u64) -> Result<u32> {
        if address & 3 != 0 {
            return Err(EINVAL);
        }
        self.invalidate(address, 4)?;
        self.access(address, 4, |p| {
            // SAFETY: access bounds the aligned word in live shared RAM.
            u32::from_le(unsafe { p.cast::<u32>().read_volatile() })
        })
    }

    /// Only for clean lines shared with firmware, never dirty host data.
    pub(crate) fn invalidate(&self, address: u64, size: usize) -> Result {
        sync();
        self.chunks(address, size, |pointer, _, count| {
            let start = (pointer as usize) & !63;
            let end = pointer as usize + count;
            for address in (start..end).step_by(64) {
                // SAFETY: Whole lines fit this owned page. All host writes to
                // shared fields are cleaned before firmware can see them.
                unsafe {
                    core::arch::asm!("dc ivac, {address}", address = in(reg) address, options(nostack, preserves_flags))
                };
            }
        })?;
        sync();
        Ok(())
    }

    /// Read retired firmware-owned words after discarding the CPU's clean
    /// copy. Callers must have published every prior host write and observed
    /// hardware retirement; this does not replace any completion/status proof.
    pub(crate) fn read_firmware_words(&self, address: u64, bytes: &mut [u8]) -> Result {
        if address & 7 != 0 || bytes.is_empty() || bytes.len() & 7 != 0 {
            return Err(EINVAL);
        }
        // Same ordering and cache-line coverage as invalidate/read64, with
        // one synchronization pair around the complete owned span.
        self.invalidate(address, bytes.len())?;
        self.chunks(address, bytes.len(), |pointer, offset, count| {
            for index in (0..count).step_by(8) {
                // SAFETY: chunks checks the live owned RAM span, and both
                // its page-aligned split and our requested words are aligned.
                let word = unsafe { pointer.add(index).cast::<u64>().read_volatile() };
                bytes[offset + index..offset + index + 8]
                    .copy_from_slice(&u64::from_le(word).to_le_bytes());
            }
        })
    }

    pub(crate) fn write64(&mut self, address: u64, value: u64) -> Result {
        if address & 7 != 0 {
            return Err(EINVAL);
        }
        // SAFETY: Bounds/alignment checked. Only host-owned protocol fields
        // and unpublished tables are written through this interface.
        self.access(address, 8, |p| unsafe {
            p.cast::<u64>().write_volatile(value.to_le())
        })
    }

    pub(crate) fn write32(&mut self, address: u64, value: u32) -> Result {
        if address & 3 != 0 {
            return Err(EINVAL);
        }
        // SAFETY: Same ownership and range checks as write64.
        self.access(address, 4, |p| unsafe {
            p.cast::<u32>().write_volatile(value.to_le())
        })
    }

    pub(crate) fn write8(&mut self, address: u64, value: u8) -> Result {
        // SAFETY: Checked one-byte host-owned field in ordinary RAM.
        self.access(address, 1, |p| unsafe { p.write_volatile(value) })
    }

    pub(crate) fn zero(&mut self, address: u64, size: usize) -> Result {
        // SAFETY: The caller exclusively owns the unpublished range. Bounds
        // are checked against the reservation before any writes take place.
        self.chunks(address, size, |p, _, count| unsafe {
            p.write_bytes(0, count)
        })?;
        Ok(())
    }

    fn chunks(
        &self,
        address: u64,
        size: usize,
        mut f: impl FnMut(*mut u8, usize, usize),
    ) -> Result {
        address.checked_add(size as u64).ok_or(EINVAL)?;
        let mut offset = 0;
        while offset < size {
            let pa = address + offset as u64;
            let count = (size - offset).min(PAGE - (pa as usize & (PAGE - 1)));
            self.access(pa, count, |p| f(p, offset, count))?;
            offset += count;
        }
        Ok(())
    }

    pub(crate) fn write(&mut self, address: u64, bytes: &[u8]) -> Result {
        self.chunks(address, bytes.len(), |p, offset, count| {
            // SAFETY: Checked mapped destination and disjoint source slice.
            unsafe { p.copy_from_nonoverlapping(bytes.as_ptr().add(offset), count) };
        })
    }

    pub(crate) fn clean(&self, address: u64, size: usize) -> Result {
        self.chunks(address, size, |pointer, _, count| {
        let start = (pointer as usize) & !63;
        let end = pointer as usize + count;
        for address in (start..end).step_by(64) {
            // Source uses dc_civac after publication writes. Firmware may
            // update other fields in this same line before our next partial
            // write, so do not retain a stale CPU copy after handing it off.
            // SAFETY: The live RAM mappings are page aligned, so rounding
            // down to a cache line remains within the same mapped page.
            unsafe {
                core::arch::asm!("dc civac, {address}", address = in(reg) address, options(nostack, preserves_flags))
            };
        }
        })?;
        sync();
        Ok(())
    }

    /// Cold partial-world preparation, matching the Python path's ordering.
    /// Both ASC CPUs must still be stopped and no host tables published.
    pub(crate) fn prepare_cold(&mut self, platform: &Platform) -> Result {
        let low_root = g17p_topology::TABLE_TARGETS
            .iter()
            .find(|(group, path, _)| *group == 1 && path.is_empty())
            .map(|(_, _, address)| *address)
            .ok_or(EINVAL)?;
        self.zero(low_root, 0x4000)?;
        self.clean(low_root, 0x4000)?;
        let contexts = platform.regions[0].base;
        // The render-capable Python entrypoint preserves slots 0/1/2 during
        // the first management boot. Other stale table pointers must go.
        for slot in 3..64 {
            self.write64(contexts + slot * 16, 0)?;
            self.write64(contexts + slot * 16 + 8, 0)?;
        }
        self.clean(contexts, 64 * 16)
    }

    pub(crate) fn write_handoff(&mut self, platform: &Platform) -> Result {
        let handoff = platform.regions[3].base;
        // Use the existing gravity-m4 host handoff initialization.
        self.write64(handoff, 0x4b1d000000000002)?;
        self.write32(handoff + 0x18, u32::MAX)?;
        self.write64(handoff + 0x640, 0)?;
        self.clean(handoff, 0x4000)
    }
}

pub(crate) fn sync() {
    // SAFETY: Complete CPU stores/cache maintenance before mailbox publication.
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}
