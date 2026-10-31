// SPDX-License-Identifier: GPL-2.0-only

//! Retired render-root switches using immutable caller admission leaves.
//! The executable root/ASID stay fixed. Mixed leaves retain their private
//! entries; only caller-only 32 MiB ranges can borrow an incoming leaf table.

use super::*;

fn address(va: u64) -> u64 { if va < 0x1000000000 { va + 0x1000000000 } else { va } }
fn covers(ranges: &[(u64,u64)], va: u64) -> bool {
    ranges.partition_point(|&(base,_)|base<=va).checked_sub(1)
        .is_some_and(|i|va<ranges[i].1)
}
impl TableSnapshot {
    fn word(&self, table: u64, index: usize) -> Result<u64> {
        if index >= 2048 { return Err(EINVAL); }
        let row = self.rows.iter().find(|row| row.0 == table).ok_or(EIO)?;
        Ok(u64::from_le(row.1[index]))
    }
    fn middle(&self, key: u64) -> Result<u64> {
        let entry = self.word(self.root, (key >> 11) as usize)?;
        if entry != 0 && entry & !ADDRESS != 3 { return Err(EINVAL); }
        Ok(entry & ADDRESS)
    }
    fn leaf_table(&self, key: u64) -> Result<u64> {
        let middle = self.middle(key)?;
        if middle == 0 { return Ok(0); }
        let entry = self.word(middle, (key & 2047) as usize)?;
        if entry != 0 && entry & !ADDRESS != 3 { return Err(EINVAL); }
        Ok(entry & ADDRESS)
    }
    fn leaf_keys(&self, keys: &mut KVec<u64>) -> Result {
        for i in 0..64 {
            let middle = self.middle((i as u64) << 11)?;
            if middle == 0 { continue; }
            let row = self.rows.iter().find(|row|row.0==middle).ok_or(EIO)?;
            for j in 0..2048 {
                let entry = u64::from_le(row.1[j]);
                if entry == 0 { continue; }
                if entry & !ADDRESS != 3 { return Err(EINVAL); }
                keys.push(((i as u64)<<11) | j as u64,GFP_KERNEL)?;
            }
        }
        Ok(())
    }
}
impl UserVm {
    fn caller_leaf_table(&self, key: u64) -> Result<u64> {
        let middle = self.read(self.root(),(key>>11) as usize)?;
        if middle == 0 { return Ok(0); }
        if middle & !ADDRESS != 3 { return Err(EINVAL); }
        let leaf = self.read(middle & ADDRESS,(key&2047) as usize)?;
        if leaf != 0 && leaf & !ADDRESS != 3 { return Err(EINVAL); }
        Ok(leaf & ADDRESS)
    }
    /// A private allocator must not turn a frozen caller-table hole into a
    /// mutable mapping. Treat the whole borrowed range as unavailable; data
    /// access and ordinary PTE queries continue to see its exact real values.
    pub(crate) fn private_pte(&self, va: u64) -> Result<u64> {
        if va >= 1<<42 || va & (PAGE-1) != 0 {return Err(EINVAL);}
        let table = self.caller_leaf_table(va >> 25)?;
        if self.borrowed.contains(&table) { return Ok(1); }
        self.pte(va)
    }
}
impl RebindSnapshot {
    /// `incoming` is the frontend's immutable caller-only admission tree,
    /// never a backend execution root. Each borrowed page is held separately
    /// from its snapshot; the prepared pool plan also retains all incoming BOs.
    pub(crate) fn prepare_leaf_switch(self, incoming: &UserVm,
        old_bindings: &[(u64,u64,u64,u32)], fallback: &[(u64,u64)],
        foreign_growth: &[(u64,u64)], contexts: &[u16]) -> Result<PreparedRebind> {
        if contexts.is_empty() || contexts.iter().enumerate().any(|(i,&c)|
            c == 0 || c >= 64 || contexts[..i].contains(&c)) { return Err(EINVAL); }
        let mut ranges = KVec::with_capacity(old_bindings.len(),GFP_KERNEL)?;
        for &(base,size,_,_) in old_bindings {
            let start = address(base);
            let end = start.checked_add(size).ok_or(EOVERFLOW)?;
            ranges.push((start,end),GFP_KERNEL)?;
        }
        sort_by_key(&mut ranges,|row|row.0);
        if ranges.windows(2).any(|r|r[0].1>r[1].0) {return Err(EINVAL);}
        let mut keys = KVec::new();
        self.snapshot.leaf_keys(&mut keys)?;
        for i in 0..64 {
            let middle = incoming.read(incoming.root(),i)?;
            if middle == 0 { continue; }
            if middle & !ADDRESS != 3 { return Err(EINVAL); }
            let body = incoming.read_table(middle & ADDRESS)?;
            for j in 0..2048 {
                if body[j] == 0 { continue; }
                if body[j] & !ADDRESS != 3 { return Err(EINVAL); }
                keys.push(((i as u64)<<11) | j as u64,GFP_KERNEL)?;
            }
        }
        sort_by_key(&mut keys,|key| *key);
        let mut tables = KVec::new();
        for page in &self.lease.tables {
            if !self.borrowed.contains(&page.phys()) { tables.push(page.clone(),GFP_KERNEL)?; }
        }
        let mut borrowed = KVec::new();
        let mut stores = KVec::new();
        let mut new_middle: KVec<(usize,Arc<Owned<Page>>)> = KVec::new();
        let mut previous = None;
        for &key in &keys {
            if previous == Some(key) { continue; }
            previous = Some(key);
            let old = self.snapshot.leaf_table(key)?;
            let next = incoming.caller_leaf_table(key)?;
            let pure = if old == 0 || self.borrowed.contains(&old) { true } else {
                let row = self.snapshot.rows.iter().find(|row|row.0==old).ok_or(EIO)?;
                row.1.iter().enumerate().all(|(index,&word)| word == 0
                    || covers(&ranges,(key<<25) + index as u64*PAGE))
            };
            if pure {
                if next != 0 && !borrowed.contains(&next) {
                    let page = incoming.tables.iter().find(|page|page.phys()==next).ok_or(EIO)?;
                    tables.push(page.clone(),GFP_KERNEL)?;
                    borrowed.push(next,GFP_KERNEL)?;
                }
                if old == next { continue; }
                let root_index = (key>>11) as usize;
                let middle = self.snapshot.middle(key)?;
                let parent = if middle != 0 {
                    self.lease.tables.iter().find(|page|page.phys()==middle).ok_or(EIO)?.clone()
                } else if let Some((_,page)) = new_middle.iter().find(|(i,_)|*i==root_index) {
                    page.clone()
                } else {
                    if next == 0 { continue; }
                    let page = Arc::new(Page::alloc_page(GFP_KERNEL | __GFP_ZERO)?,GFP_KERNEL)?;
                    if page.phys() & !ADDRESS != 0 { return Err(EINVAL); }
                    UserVm::clean(&page);
                    tables.push(page.clone(),GFP_KERNEL)?;
                    let root = self.lease.tables.iter().find(|page|page.phys()==self.snapshot.root).ok_or(EIO)?.clone();
                    stores.push((0,root,root_index,0,page.phys()|3,self.snapshot.root),GFP_KERNEL)?;
                    new_middle.push((root_index,page.clone()),GFP_KERNEL)?;
                    page
                };
                let before = if old==0 {0}else{old|3};
                let after = if next==0 {0}else{next|3};
                let physical = parent.phys();
                stores.push((1,parent,(key&2047) as usize,before,after,physical),GFP_KERNEL)?;
            } else {
                let row = self.snapshot.rows.iter().find(|row|row.0==old).ok_or(EIO)?;
                let page = self.lease.tables.iter().find(|page|page.phys()==old).ok_or(EIO)?.clone();
                let data = if next==0 {None}else{Some(incoming.read_table(next)?)};
                for index in 0..2048 {
                    let va = (key<<25) + index as u64*PAGE;
                    let before = u64::from_le(row.1[index]);
                    let new = data.as_ref().map_or(0,|data|data[index]);
                    if before == 0 && new == 0 {continue;}
                    let caller = covers(&ranges,va);
                    let after = if new != 0 {
                        if before!=0 && !caller
                            && !fallback.iter().any(|&(address,pa)|address==va && before&ADDRESS==pa)
                            && !foreign_growth.contains(&(va,before)) { return Err(EBUSY); }
                        new
                    } else if caller {0}else{before};
                    if before!=after { stores.push((2,page.clone(),index,before,after,old),GFP_KERNEL)?; }
                }
            }
        }
        sort_by_key(&mut stores,|row|(row.0,row.5,row.2));
        if stores.windows(2).any(|rows|rows[0].5==rows[1].5 && rows[0].2==rows[1].2) {
            return Err(EINVAL);
        }
        let mut held = KVec::new();held.extend_from_slice(contexts,GFP_KERNEL)?;
        let mut plan=PreparedRebind {source:self,tables,stores,contexts:held,borrowed};
        plan.prune_tables()?;
        Ok(plan)
    }
}
