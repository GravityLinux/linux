// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Owned 42-bit user UAT tree, including quiescent caller-binding updates.

use kernel::{
    alloc::flags::__GFP_ZERO,
    page::{self, Page},
    prelude::*,
    sync::Arc,
    types::Owned,
};
const PAGE: u64 = 0x4000;
const ADDRESS: u64 = 0x000003ffffffc000;
use core::sync::atomic::{AtomicU64, Ordering};

/// Mapping batches can contain arbitrarily many leaves. Keep sorting stack
/// usage constant even in the deep render-publication call path: the standard
/// recursive slice sort overflowed a kernel worker's stack on Mesa images.
#[inline(never)]
pub(crate) fn sort_by_key<T, K: Ord>(rows: &mut [T], key: impl Fn(&T) -> K) {
    fn sift_down<T, K: Ord>(rows: &mut [T], mut root: usize, end: usize,
        key: &impl Fn(&T) -> K) {
        // This bound also prevents overflow in the child-index calculation.
        while root < end / 2 {
            let mut child = root * 2 + 1;
            if child + 1 < end && key(&rows[child]) < key(&rows[child + 1]) {
                child += 1;
            }
            if key(&rows[root]) >= key(&rows[child]) { break; }
            rows.swap(root, child);
            root = child;
        }
    }
    for root in (0..rows.len() / 2).rev() {
        sift_down(rows, root, rows.len(), &key);
    }
    for end in (1..rows.len()).rev() {
        rows.swap(0, end);
        sift_down(rows, 0, end, &key);
    }
}

pub(crate) struct UserVm {
    tables: KVec<Arc<Owned<Page>>>,
    generation: AtomicU64,
    spare_tables: KVec<Arc<Owned<Page>>>,
}
/// Pins the actual executable root and its current child tables. A live root
/// owner remains responsible for mapping/growth writes; this lease only keeps
/// backing alive and never provides a second mutable UserVm.
pub(crate) struct RootLease {
    root: u64,
    tables: KVec<Arc<Owned<Page>>>,
}
impl RootLease {
    pub(crate) fn root(&self) -> u64 { self.root }
    /// Growth may add tables while firmware owns this root. Refresh before
    /// releasing its execution owner so every newly reachable table is pinned.
    pub(crate) fn refresh(&mut self, owner: &UserVm) -> Result {
        if self.root != owner.root() { return Err(EIO); }
        // UserVm table ownership is append-only after its root is installed.
        // Validate the pinned prefix once, then retain only new growth pages.
        // Avoid two quadratic membership scans on every live render poll.
        if self.tables.len() > owner.tables.len() || self.tables.iter().zip(&owner.tables)
            .any(|(old, page)| old.phys() != page.phys()) { return Err(EIO); }
        let first = self.tables.len();
        self.tables.reserve(owner.tables.len() - first, GFP_KERNEL)?;
        for page in &owner.tables[first..] { self.tables.push(page.clone(), GFP_KERNEL)?; }
        Ok(())
    }
}
/// Fully allocated/validated quiescent PTE publication. Commit cannot fail.
pub(crate) struct RebindPlan<'a> {
    stores: KVec<(usize, &'a Page, usize, u64, u64)>,
    contexts: KVec<u16>,
    generation: &'a AtomicU64,
}
impl RebindPlan<'_> {
    /// Only a separately allocated, unpublished table copy may use this path.
    /// It prepares DATA for a later version-checked executable-root commit;
    /// it must not invalidate the ASID belonging to the installed root.
    pub(crate) fn commit_unpublished(self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        for depth in [2, 1, 0] {
            for &(_, page, index, _, new) in self.stores.iter().filter(|r| r.0 == depth) {
                UserVm::store(page, index, new);
            }
        }
    }
    pub(crate) fn commit(self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        for &(depth, page, index, old, _) in &self.stores {
            if depth == 2 && old != 0 {
                UserVm::store(page, index, 0);
            }
        }
        for &context in &self.contexts {
            UserVm::invalidate(context);
        }
        for depth in [2, 1, 0] {
            for &(_, page, index, _, new) in self.stores.iter().filter(|r| r.0 == depth) {
                UserVm::store(page, index, new);
            }
            super::g17p_memory::sync();
        }
        for &context in &self.contexts {
            UserVm::invalidate(context);
        }
    }
}
pub(crate) struct TableWord<'a> {
    page: &'a Page,
    index: usize,
    generation: &'a AtomicU64,
}
impl TableWord<'_> {
    pub(crate) fn load(&self) -> u64 {
        self.page.with_page_mapped(|p| {
            // SAFETY: The constructor checks the owned table and qword index.
            u64::from_le(unsafe { p.cast::<u64>().add(self.index).read_volatile() })
        })
    }
    pub(crate) fn store(&self, value: u64) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        UserVm::store(self.page, self.index, value);
    }
}
/// Allocated by the worker before acquiring shared state. Capture performs
/// only bounded table DATA copies into this already-owned host storage.
pub(crate) struct TableStorage { rows: KVec<(u64, KVVec<u64>)> }
pub(crate) struct TableSnapshot {
    root: u64,
    generation: u64,
    rows: KVec<(u64, KVVec<u64>)>,
}

/// Immutable table DATA and actual backing ownership captured under the
/// device lock. All mapping walks, collision checks and allocation can then
/// operate on a separate copy without touching an installed executable root.
pub(crate) struct RebindSnapshot {
    snapshot: TableSnapshot,
    lease: RootLease,
}
pub(crate) struct RebindRelocation {
    source: RebindSnapshot,
    copies: KVec<(u64, u32, u64)>,
}
pub(crate) struct PreparedRebind {
    source: RebindSnapshot,
    tables: KVec<Arc<Owned<Page>>>,
    stores: KVec<(usize, Arc<Owned<Page>>, usize, u64, u64)>,
    contexts: KVec<u16>,
}
impl RebindSnapshot {
    pub(crate) fn clone_tree(self) -> Result<(UserVm, RebindRelocation)> {
        let (root, copies) = self.snapshot.clone_with_addresses()?;
        Ok((root, RebindRelocation { source: self, copies }))
    }
}
impl RebindRelocation {
    /// Translate a completed unpublished table copy back to the original root
    /// identity. Only parent-table links are relocated; caller/program DATA
    /// leaves retain their exact physical addresses and attributes.
    pub(crate) fn prepare(self, shadow: &UserVm, contexts: &[u16]) -> Result<PreparedRebind> {
        if contexts.is_empty() || contexts.iter().enumerate().any(|(i, &c)|
            c == 0 || c >= 64 || contexts[..i].contains(&c)) { return Err(EINVAL); }
        let mut levels = KVec::new();
        levels.push((shadow.root(), 0usize), GFP_KERNEL)?;
        let mut cursor = 0;
        while cursor < levels.len() {
            let (pa, depth) = levels[cursor]; cursor += 1;
            if depth == 2 { continue; }
            for index in 0..if depth == 0 { 64 } else { 2048 } {
                let word = shadow.read(pa, index)?;
                if word == 0 { continue; }
                if word & 3 != 3 { return Err(EINVAL); }
                let child = word & ADDRESS;
                if let Some(&(_, prior)) = levels.iter().find(|row| row.0 == child) {
                    if prior != depth + 1 { return Err(EINVAL); }
                } else { levels.push((child, depth + 1), GFP_KERNEL)?; }
            }
        }
        let original_child = |word: u64| {
            if word & 3 != 3 { return word; }
            self.copies.iter().find(|row| row.2 == word & ADDRESS)
                .map_or(word, |row| (word & !ADDRESS) | row.0)
        };
        let capacity = self.source.lease.tables.len().checked_add(levels.len()).ok_or(EOVERFLOW)?;
        let mut tables = KVec::with_capacity(capacity, GFP_KERNEL)?;
        for page in &self.source.lease.tables { tables.push(page.clone(), GFP_KERNEL)?; }
        let mut stores = KVec::new();
        for &(pa, depth) in &levels {
            let count = if depth == 0 { 64 } else { 2048 };
            if let Some(&(original, level, _)) = self.copies.iter().find(|row| row.2 == pa) {
                if level as usize != depth { return Err(EINVAL); }
                let old = self.source.snapshot.rows.iter().find(|row| row.0 == original).ok_or(EIO)?;
                let page = self.source.lease.tables.iter().find(|page| page.phys() == original).ok_or(EIO)?;
                for index in 0..count {
                    let before = u64::from_le(old.1[index]);
                    let mut after = shadow.read(pa, index)?;
                    if depth < 2 { after = original_child(after); }
                    if before != after { stores.push((depth, page.clone(), index, before, after), GFP_KERNEL)?; }
                }
            } else {
                // This parent table was allocated only in the speculative
                // copy. It becomes owned by the installed root at commit.
                let page = shadow.tables.iter().find(|page| page.phys() == pa).ok_or(EIO)?;
                if depth < 2 {
                    page.with_page_mapped(|pointer| {
                        for index in 0..count {
                            // SAFETY: This unpublished page is exclusively
                            // owned by this preparation, with checked indices.
                            unsafe {
                                let at = pointer.cast::<u64>().add(index);
                                at.write(original_child(u64::from_le(at.read())).to_le());
                            }
                        }
                    });
                    UserVm::clean(page);
                }
                tables.push(page.clone(), GFP_KERNEL)?;
            }
        }
        sort_by_key(&mut stores, |row| (row.0, row.1.phys(), row.2));
        if stores.windows(2).any(|rows| rows[0].1.phys() == rows[1].1.phys()
            && rows[0].2 == rows[1].2) { return Err(EINVAL); }
        let mut held_contexts = KVec::new(); held_contexts.extend_from_slice(contexts, GFP_KERNEL)?;
        Ok(PreparedRebind { source: self.source, tables, stores, contexts: held_contexts })
    }
}
impl PreparedRebind {
    pub(crate) fn matches(&self, root: &UserVm) -> bool { self.source.snapshot.matches(root) }
    /// The caller additionally proves this pool is retired and exclusively
    /// selected. All allocation and preflight already completed off-lock.
    /// Keep this plan owned by the worker so host storage drops off-lock.
    pub(crate) fn commit(&mut self, root: &mut UserVm) -> Result {
        if !self.matches(root) { return Err(EBUSY); }
        core::mem::swap(&mut root.tables, &mut self.tables);
        root.generation.fetch_add(1, Ordering::Relaxed);
        self.store_batch(2, true);
        for &context in &self.contexts { UserVm::invalidate(context); }
        for depth in [2, 1, 0] {
            self.store_batch(depth, false);
        }
        for &context in &self.contexts { UserVm::invalidate(context); }
        Ok(())
    }
    /// Sort/grouping was prepared off-lock. Map each table once and publish
    /// each modified cache line once, with a barrier before the next TLBI or
    /// parent-link level. This preserves break-before-make without a system
    /// barrier for every individual PTE.
    fn store_batch(&self, depth: usize, clear: bool) {
        let mut cursor = 0;
        while cursor < self.stores.len() {
            let first = cursor;
            let page = &self.stores[first].1;
            cursor += 1;
            while cursor < self.stores.len() && self.stores[cursor].0 == self.stores[first].0
                && self.stores[cursor].1.phys() == page.phys() { cursor += 1; }
            if self.stores[first].0 != depth { continue; }
            page.with_page_mapped(|pointer| {
                let mut line: Option<usize> = None;
                for &(_, _, index, before, after) in &self.stores[first..cursor] {
                    if clear && before == 0 { continue; }
                    let next = index & !7;
                    if line != Some(next) {
                        if let Some(previous) = line {
                            // SAFETY: A checked 64-byte line in this retained
                            // table page; all its stores precede this clean.
                            unsafe { core::arch::asm!("dc civac, {p}",p=in(reg)pointer.add(previous*8),options(nostack,preserves_flags)); }
                        }
                        line = Some(next);
                    }
                    // SAFETY: The off-lock plan checked all owned indices;
                    // the exclusive retired owner was revalidated at commit.
                    unsafe { pointer.cast::<u64>().add(index).write_volatile(if clear { 0 } else { after.to_le() }); }
                }
                if let Some(last) = line {
                    // SAFETY: Same checked, retained table cache line.
                    unsafe { core::arch::asm!("dc civac, {p}",p=in(reg)pointer.add(last*8),options(nostack,preserves_flags)); }
                }
            });
        }
        super::g17p_memory::sync();
    }
}
impl TableStorage {
    pub(crate) fn new(count: usize) -> Result<Self> {
        let mut rows = KVec::with_capacity(count, GFP_KERNEL)?;
        for _ in 0..count {
            let mut body = KVVec::with_capacity(2048, GFP_KERNEL)?;
            body.resize(2048, 0, GFP_KERNEL)?;
            rows.push((0, body), GFP_KERNEL)?;
        }
        Ok(Self { rows })
    }
}
impl TableSnapshot {
    pub(crate) fn matches(&self, root: &UserVm) -> bool {
        self.root == root.root() && self.generation == root.generation.load(Ordering::Relaxed)
            && self.rows.len() == root.tables.len()
    }
    pub(crate) fn clone_tree(&self) -> Result<UserVm> {
        self.clone_with_addresses().map(|(root, _)| root)
    }
    fn clone_with_addresses(&self) -> Result<(UserVm, KVec<(u64, u32, u64)>)> {
        fn clone(
            source: &TableSnapshot,
            destination: &mut UserVm,
            pa: u64,
            depth: u32,
            copies: &mut KVec<(u64, u32, u64)>,
            active: &mut KVec<u64>,
            root: Option<u64>,
        ) -> Result<u64> {
            if pa == 0 || pa & !ADDRESS != 0 || depth > 2 {
                return Err(EINVAL);
            }
            if let Some(&(_, _, copy)) = copies
                .iter()
                .find(|&&(old, level, _)| old == pa && level == depth)
            {
                return Ok(copy);
            }
            if copies.iter().any(|&(old, level, _)| old == pa && level != depth) {
                return Err(EINVAL);
            }
            if active.contains(&pa) {
                return Err(EINVAL);
            }
            let row = source.rows.iter().find(|row| row.0 == pa).ok_or(EINVAL)?;
            active.push(pa, GFP_KERNEL)?;
            let mut body = KVVec::with_capacity(2048, GFP_KERNEL)?;
            body.resize(2048, 0u64, GFP_KERNEL)?;
            body.copy_from_slice(&row.1);
            for word in body.iter_mut() { *word = u64::from_le(*word); }
            if depth < 2 {
                for index in 0..if depth == 0 { 64 } else { 2048 } {
                    let word = body[index];
                    if word & 1 == 0 {
                        continue;
                    }
                    if word & 3 != 3 {
                        return Err(EINVAL);
                    }
                    let child = clone(
                        source,
                        destination,
                        word & ADDRESS,
                        depth + 1,
                        copies,
                        active,
                        None,
                    )?;
                    body[index] = (word & !ADDRESS) | child;
                }
            }
            let copy = match root {
                Some(copy) => copy,
                None => destination.table()?,
            };
            let page = destination.page(copy)?;
            page.with_page_mapped(|pointer| {
                for (index, word) in body.iter().enumerate() {
                    // SAFETY: The new table owns all 2048 words. Child links
                    // already refer to separately retained, cleaned copies.
                    unsafe { pointer.cast::<u64>().add(index).write(word.to_le()); }
                }
            });
            UserVm::clean(page);
            copies.push((pa, depth, copy), GFP_KERNEL)?;
            active.pop();
            Ok(copy)
        }
        let mut destination = UserVm::new()?;
        let mut copies = KVec::new();
        let mut active = KVec::new();
        let root = destination.root();
        clone(self, &mut destination, self.root, 0, &mut copies, &mut active, Some(root))?;
        Ok((destination, copies))
    }
}

impl UserVm {
    pub(crate) fn table_word(&self, address: u64) -> Result<TableWord<'_>> {
        if address & 7 != 0 {
            return Err(EINVAL);
        }
        Ok(TableWord {
            page: self.page(address & !(PAGE - 1))?,
            index: ((address & (PAGE - 1)) / 8) as usize,
            generation: &self.generation,
        })
    }
    /// Retain the old root and all child tables while creating its exact data
    /// copy. Publication is separate so both TTB words can be checked first.
    pub(crate) fn clone_root_page(&mut self) -> Result<(usize, u64)> {
        let old = self.root();
        let copy = self.table()?;
        for index in 0..2048 {
            self.write(copy, index, self.read(old, index)?)?;
        }
        Ok((self.tables.len() - 1, copy))
    }
    pub(crate) fn publish_root_clone(&mut self, index: usize) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.tables.swap(0, index);
    }
    pub(crate) fn new() -> Result<Self> {
        if page::PAGE_SIZE != PAGE as usize {
            return Err(EINVAL);
        }
        let mut vm = Self {
            tables: KVec::new(),
            generation: AtomicU64::new(0),
            spare_tables: KVec::new(),
        };
        vm.table()?;
        Ok(vm)
    }
    pub(crate) fn root(&self) -> u64 {
        self.tables[0].phys()
    }
    pub(crate) fn lease(&self) -> Result<RootLease> {
        let mut lease = RootLease { root: self.root(), tables: KVec::new() };
        lease.refresh(self)?;
        Ok(lease)
    }
    fn table(&mut self) -> Result<u64> {
        self.tables.reserve(1, GFP_KERNEL)?;
        let (page, prepared) = if let Some(page) = self.spare_tables.pop() { (page, true) } else {
            (Arc::new(Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?, GFP_KERNEL)?, false)
        };
        let pa = page.phys();
        if pa & !ADDRESS != 0 {
            return Err(EINVAL);
        }
        if !prepared { Self::clean(&page); }
        self.tables.push(page, GFP_KERNEL)?;
        Ok(pa)
    }
    /// Prepare zeroed parent tables without borrowing an installed owner.
    /// The caller's complete sparse tree bounds its missing parent count.
    pub(crate) fn prepare_spare_tables(&mut self, count: usize) -> Result {
        while self.spare_tables.len() < count {
            let page = Arc::new(Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?, GFP_KERNEL)?;
            if page.phys() & !ADDRESS != 0 { return Err(EINVAL); }
            Self::clean(&page);
            self.spare_tables.push(page, GFP_KERNEL)?;
        }
        Ok(())
    }
    /// Transfer unpublished table ownership before the first PTE publication.
    pub(crate) fn absorb_spare_tables(&mut self, other: &mut Self) -> Result {
        self.spare_tables.reserve(other.spare_tables.len(), GFP_KERNEL)?;
        for page in other.spare_tables.drain(..) { self.spare_tables.push(page, GFP_KERNEL)?; }
        Ok(())
    }
    fn page(&self, pa: u64) -> Result<&Page> {
        self.tables
            .iter()
            .find(|p| p.phys() == pa)
            .map(|p| &***p)
            .ok_or(EINVAL)
    }
    fn clean(page: &Page) {
        page.with_page_mapped(|p| {
            for offset in (0..PAGE as usize).step_by(64) {
                // SAFETY: Every complete line lies inside this owned page.
                unsafe {core::arch::asm!("dc civac, {p}",p=in(reg)p.add(offset),options(nostack,preserves_flags))};
            }
        });
        super::g17p_memory::sync();
    }
    fn read(&self, pa: u64, index: usize) -> Result<u64> {
        if index >= 2048 {
            return Err(EINVAL);
        }
        Ok(self.page(pa)?.with_page_mapped(|p| {
            // SAFETY: Aligned qword within the owned table, serialized by File.
            u64::from_le(unsafe { p.cast::<u64>().add(index).read_volatile() })
        }))
    }
    fn write(&mut self, pa: u64, index: usize, word: u64) -> Result {
        if index >= 2048 {
            return Err(EINVAL);
        }
        let page = self.page(pa)?;
        self.generation.fetch_add(1, Ordering::Relaxed);
        Self::store(page, index, word);
        Ok(())
    }
    // Only checked, owned table pages and indices reach the publication pass.
    fn store(page: &Page, index: usize, word: u64) {
        page.with_page_mapped(|p| {
            // SAFETY: Exclusive VM access and checked aligned table index.
            unsafe {p.cast::<u64>().add(index).write_volatile(word.to_le());
                core::arch::asm!("dc civac, {p}",p=in(reg)p.add(index*8),options(nostack,preserves_flags));}
        });
        super::g17p_memory::sync();
    }
    /// g17p_context.clone_low_tables: clone table DATA only. Data/code leaves
    /// retain their exact physical owners and attributes; none is dereferenced.
    pub(crate) fn table_count(&self) -> usize { self.tables.len() }
    pub(crate) fn capture_tables(&self, mut storage: TableStorage) -> Result<Option<TableSnapshot>> {
        if storage.rows.len() != self.tables.len() { return Ok(None); }
        for (row, page) in storage.rows.iter_mut().zip(&self.tables) {
            row.0 = page.phys();
            page.with_page_mapped(|pointer| {
                // SAFETY: The runtime mutex excludes host writes. Firmware
                // only reads these tables; the destination is distinct storage.
                unsafe { core::ptr::copy_nonoverlapping(pointer.cast::<u64>(), row.1.as_mut_ptr(), 2048); }
            });
        }
        Ok(Some(TableSnapshot { root: self.root(),
            generation: self.generation.load(Ordering::Relaxed), rows: storage.rows }))
    }
    pub(crate) fn capture_rebind(&self, storage: TableStorage) -> Result<Option<RebindSnapshot>> {
        let Some(snapshot) = self.capture_tables(storage)? else { return Ok(None); };
        Ok(Some(RebindSnapshot { snapshot, lease: self.lease()? }))
    }
    pub(crate) fn clone_low_tables(&self) -> Result<(Self, KVec<(u64, u32, u64)>)> {
        let storage = TableStorage::new(self.table_count())?;
        self.capture_tables(storage)?.ok_or(EIO)?.clone_with_addresses()
    }
    /// Copy an immutable, unpublished caller-only admission tree. Child
    /// table links point to independently owned copies; caller data leaves
    /// retain their physical addresses and exact attributes. No live root is
    /// shared or changed. Whole-page cache publication replaces per-PTE work.
    pub(crate) fn clone_admission_tree(&self) -> Result<Self> {
        fn clone(
            source: &UserVm,
            destination: &mut UserVm,
            pa: u64,
            depth: u32,
            copies: &mut KVec<(u64, u32, u64)>,
            active: &mut KVec<u64>,
            root: Option<u64>,
        ) -> Result<u64> {
            if pa == 0 || pa & !ADDRESS != 0 || depth > 2 {
                return Err(EINVAL);
            }
            if let Some(&(_, _, copy)) = copies
                .iter()
                .find(|&&(old, level, _)| old == pa && level == depth)
            {
                return Ok(copy);
            }
            if active.contains(&pa) {
                return Err(EINVAL);
            }
            let page = source.page(pa)?;
            active.push(pa, GFP_KERNEL)?;
            let mut body = KVVec::with_capacity(2048, GFP_KERNEL)?;
            body.resize(2048, 0u64, GFP_KERNEL)?;
            page.with_page_mapped(|pointer| {
                // SAFETY: The immutable source owns this entire table page;
                // body has distinct allocated storage for all 2048 words.
                unsafe {
                    core::ptr::copy_nonoverlapping(pointer.cast::<u64>(), body.as_mut_ptr(), 2048);
                }
            });
            for word in body.iter_mut() {
                *word = u64::from_le(*word);
            }
            if depth < 2 {
                for index in 0..if depth == 0 { 64 } else { 2048 } {
                    let word = body[index];
                    if word & 1 == 0 {
                        continue;
                    }
                    if word & 3 != 3 {
                        return Err(EINVAL);
                    }
                    let child = clone(
                        source,
                        destination,
                        word & ADDRESS,
                        depth + 1,
                        copies,
                        active,
                        None,
                    )?;
                    body[index] = (word & !ADDRESS) | child;
                }
            }
            let copy = match root {
                Some(copy) => copy,
                None => destination.table()?,
            };
            let page = destination.page(copy)?;
            page.with_page_mapped(|pointer| {
                for (index, word) in body.iter().enumerate() {
                    // SAFETY: All words fit this exclusively owned unpublished
                    // page. Its child pages were copied and cleaned first.
                    unsafe {
                        pointer.cast::<u64>().add(index).write(word.to_le());
                    }
                }
            });
            UserVm::clean(page);
            copies.push((pa, depth, copy), GFP_KERNEL)?;
            active.pop();
            Ok(copy)
        }
        let mut destination = Self::new()?;
        let mut copies = KVec::new();
        let mut active = KVec::new();
        let root = destination.root();
        clone(
            self,
            &mut destination,
            self.root(),
            0,
            &mut copies,
            &mut active,
            Some(root),
        )?;
        Ok(destination)
    }

    pub(crate) fn pte(&self, va: u64) -> Result<u64> {
        if va >= 1 << 42 || va & (PAGE - 1) != 0 {
            return Err(EINVAL);
        }
        let mut table = self.root();
        for index in [((va >> 36) & 63) as usize, ((va >> 25) & 2047) as usize] {
            let entry = self.read(table, index)?;
            if entry == 0 {
                return Ok(0);
            }
            if entry & 3 != 3 {
                return Err(EINVAL);
            }
            table = entry & ADDRESS;
        }
        self.read(table, ((va >> 14) & 2047) as usize)
    }

    pub(crate) fn invalidate(context: u16) {
        super::g17p_memory::sync();
        // SAFETY: Called under the runtime lock with the source's quiescent
        // execution context. T8140 implements FEAT_TLBIOS.
        unsafe {
            core::arch::asm!(
                ".inst 0xd5088140", // TLBI ASIDE1OS, X0
                "dsb sy", "isb",
                in("x0") (context as u64) << 48,
                options(nostack, preserves_flags)
            );
        }
    }

    /// Replace caller leaves after completion, preserving private/grown pages.
    /// Each change names its expected old leaf (zero for an addition). Allocate
    /// and validate the complete plan before breaking any live mapping. Failed
    /// preparation leaves active tables intact, including all parent entries.
    pub(crate) fn rebind(&mut self, changes: &[(u64, u64, u64)], contexts: &[u16]) -> Result {
        self.prepare_rebind(changes, contexts)?.commit();
        Ok(())
    }

    /// Stage all allocation and expected-leaf checks before any live store.
    /// Several independent roots may be prepared before committing them all.
    pub(crate) fn prepare_rebind<'a>(
        &'a mut self,
        changes: &[(u64, u64, u64)],
        contexts: &[u16],
    ) -> Result<RebindPlan<'a>> {
        if contexts.is_empty()
            || contexts.iter().enumerate().any(|(i, &context)| {
                context == 0 || context >= 64 || contexts[..i].contains(&context)
            })
        {
            return Err(EINVAL);
        }
        let mut writes: KVec<(usize, u64, usize, u64)> = KVec::with_capacity(changes.len(), GFP_KERNEL)?;
        // Only newly created parent links participate in a later walk. Keep
        // them separate: searching all preceding leaf writes made retired-root
        // reassignment quadratic in the caller's mapped page count.
        let mut parents: KVec<(usize, u64, usize, u64)> = KVec::new();
        let mut leaf_keys: KVec<(u64, usize)> = KVec::with_capacity(changes.len(), GFP_KERNEL)?;
        for &(va, old, new) in changes {
            if va >= 1 << 42
                || va & (PAGE - 1) != 0
                || [old, new].into_iter().any(|pte| {
                    pte != 0
                        && (pte & ADDRESS == 0
                            || pte & !(ADDRESS | 0x00c0000000000fff) != 0
                            || pte & 3 != 3)
                })
                || self.pte(va)? != old
            {
                return Err(EINVAL);
            }
            let mut table = self.root();
            for (depth, index) in [((va >> 36) & 63) as usize, ((va >> 25) & 2047) as usize]
                .into_iter()
                .enumerate()
            {
                let mut entry =
                    if let Some(row) = parents.iter().find(|r| r.1 == table && r.2 == index) {
                        row.3
                    } else {
                        self.read(table, index)?
                    };
                if entry == 0 {
                    entry = self.table()? | 3;
                    parents.push((depth, table, index, entry), GFP_KERNEL)?;
                }
                if entry & 3 != 3 {
                    return Err(EINVAL);
                }
                table = entry & ADDRESS;
                self.page(table)?;
            }
            let index = ((va >> 14) & 2047) as usize;
            leaf_keys.push((table, index), GFP_KERNEL)?;
            writes.push((2, table, index, new), GFP_KERNEL)?;
        }
        // Distinct VAs can alias a table leaf; retain the original rejection
        // rule without scanning every previous write for every changed page.
        sort_by_key(&mut leaf_keys, |row| *row);
        if leaf_keys.windows(2).any(|rows| rows[0] == rows[1]) { return Err(EINVAL); }
        writes.extend_from_slice(&parents, GFP_KERNEL)?;
        // Resolve every owned page reference before publication. The following
        // two store passes have no allocation, lookup or other fallible step.
        let mut stores = KVec::new();
        for &(depth, table, index, new) in &writes {
            let old = self.read(table, index)?;
            if old != new {
                stores.push((depth, self.page(table)?, index, old, new), GFP_KERNEL)?;
            }
        }
        let mut held_contexts = KVec::new();
        held_contexts.extend_from_slice(contexts, GFP_KERNEL)?;
        Ok(RebindPlan { stores, contexts: held_contexts, generation: &self.generation })
    }
    fn leaf(&mut self, va: u64, create: bool) -> Result<Option<(u64, usize)>> {
        if va >= 1 << 42 || va & (PAGE - 1) != 0 {
            return Err(EINVAL);
        }
        let mut pa = self.root();
        for index in [((va >> 36) & 63) as usize, ((va >> 25) & 2047) as usize] {
            let mut entry = self.read(pa, index)?;
            if entry == 0 {
                if !create {
                    return Ok(None);
                }
                let next = self.table()?;
                entry = next | 3;
                self.write(pa, index, entry)?;
            }
            if entry & 3 != 3 {
                return Err(EINVAL);
            }
            pa = entry & ADDRESS;
        }
        self.page(pa)?;
        Ok(Some((pa, ((va >> 14) & 2047) as usize)))
    }
    /// Preallocate the table paths before modifying leaves. Failed allocation
    /// leaves only empty tables; the previous bindings remain valid.
    pub(crate) fn prepare(&mut self, va: u64, size: u64) -> Result {
        if size == 0 || size & (PAGE - 1) != 0 || va.checked_add(size).ok_or(EINVAL)? > 1 << 42 {
            return Err(EINVAL);
        }
        for offset in (0..size).step_by(PAGE as usize) {
            let (table, index) = self.leaf(va + offset, true)?.ok_or(EINVAL)?;
            if self.read(table, index)? != 0 {
                return Err(EBUSY);
            }
        }
        Ok(())
    }
    pub(crate) fn map_page(&mut self, va: u64, pa: u64, writable: bool) -> Result {
        let flags = 0x0080000000000c8b | if writable { 1 << 54 } else { 0 };
        self.map_owned_page(va, pa, flags)
    }
    /// Preserve the source attributes of driver-owned private render pages.
    /// This is an internal mapping API; userspace cannot supply PTE flags.
    pub(crate) fn map_owned_page(&mut self, va: u64, pa: u64, flags: u64) -> Result {
        if pa & !ADDRESS != 0 {
            return Err(EINVAL);
        }
        if flags & !0x00c0000000000fff != 0 || flags & 3 != 3 {
            return Err(EINVAL);
        }
        let (table, index) = self.leaf(va, false)?.ok_or(EINVAL)?;
        if self.read(table, index)? != 0 {
            return Err(EBUSY);
        }
        // Current source uses Shared/AP=2/nG, with UXN for writable resources.
        let pte = pa | flags;
        self.write(table, index, pte)?;
        if self.read(table, index)? != pte {
            return Err(EIO);
        }
        Ok(())
    }
    pub(crate) fn unmap(&mut self, va: u64, size: u64) -> Result {
        if size == 0 || size & (PAGE - 1) != 0 || va.checked_add(size).ok_or(EINVAL)? > 1 << 42 {
            return Err(EINVAL);
        }
        for offset in (0..size).step_by(PAGE as usize) {
            if let Some((table, index)) = self.leaf(va + offset, false)? {
                self.write(table, index, 0)?;
            }
        }
        Ok(())
    }

    /// Add disjoint growth pages to a live retained root. Validate every leaf
    /// and allocate every child first; an allocation failure publishes nothing.
    /// Each new child is populated/cleaned before its parent link is visible.
    pub(crate) fn grow(&mut self, pages: &[(u64, u64)]) -> Result {
        if pages.is_empty() {
            return Err(EINVAL);
        }
        let mut writes: KVec<(usize, u64, usize, u64)> = KVec::new();
        for &(va, physical) in pages {
            if va >= 1 << 42 || va & (PAGE - 1) != 0 || physical == 0 || physical & !ADDRESS != 0 {
                return Err(EINVAL);
            }
            let mut table = self.root();
            for (depth, index) in [((va >> 36) & 63) as usize, ((va >> 25) & 2047) as usize]
                .into_iter()
                .enumerate()
            {
                let mut entry =
                    if let Some(row) = writes.iter().find(|r| r.1 == table && r.2 == index) {
                        row.3
                    } else {
                        self.read(table, index)?
                    };
                if entry == 0 {
                    entry = self.table()? | 3;
                    writes.push((depth, table, index, entry), GFP_KERNEL)?;
                }
                if entry & 3 != 3 {
                    return Err(EINVAL);
                }
                table = entry & ADDRESS;
                self.page(table)?;
            }
            let index = ((va >> 14) & 2047) as usize;
            if self.read(table, index)? != 0 || writes.iter().any(|r| r.1 == table && r.2 == index)
            {
                return Err(EBUSY);
            }
            writes.push((2, table, index, physical | 0x00c0000000000c8b), GFP_KERNEL)?;
        }
        // No allocation or user-controlled validation after this boundary.
        for depth in [2, 1, 0] {
            for &(_, table, index, entry) in writes.iter().filter(|r| r.0 == depth) {
                self.write(table, index, entry)?;
            }
            super::g17p_memory::sync();
        }
        Ok(())
    }
}
