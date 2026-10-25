// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! A VM's GPU mappings: binding, publishing and unbinding objects, and
//! detaching the VM from the runtime.

use super::*;

impl Vm {
    pub(super) fn detach(&self, runtime: &mut crate::g16::Bootstrap) -> Result {
        {
            let space = self.space.lock();
            runtime.report_vm_detach(space.roots().low);
            runtime.release_render_vm(space.roots())?;
        }
        let compute = self.compute_space.lock();
        runtime.release_render_vm(compute.roots())?;
        Ok(())
    }

    pub(super) fn map(
        &mut self,
        range: Range<u64>,
        mut offset: u64,
        obj: ARef<Object>,
        sgt: &scatterlist::SGTable,
        access: crate::pgtable::Prot,
        permissions: u32,
        single: bool,
        runtime: &crate::g16::Bootstrap,
    ) -> Result {
        self.snapshot = None;
        if self
            .mappings
            .iter()
            .any(|m| range.start < m.range.end && range.end > m.range.start)
        {
            return Err(EEXIST);
        }
        let bo_offset = offset;
        let mut chunks = KVVec::new();
        let mut va = range.start;
        for segment in sgt.iter() {
            let length = u64::from(segment.dma_len());
            if offset >= length {
                offset -= length;
                continue;
            }
            let pa = segment.dma_address().checked_add(offset).ok_or(EINVAL)?;
            let length = (length - offset).min(range.end - va);
            offset = 0;
            if (va | pa | length) & 0x3fff != 0 {
                return Err(EINVAL);
            }
            let end = if single { range.end } else { va + length };
            chunks.push((va, end, pa), GFP_KERNEL)?;
            va = end;
            if va == range.end {
                break;
            }
        }
        if va != range.end {
            return Err(EINVAL);
        }
        self.mappings.push(
            Mapping {
                range: range.clone(),
                permissions,
                offset: bo_offset,
                single,
                obj,
                closing: false,
            },
            GFP_KERNEL,
        )?;
        // Keep the list sorted by address for lookups.
        let last = self.mappings.len() - 1;
        let index = self.mappings[..last].partition_point(|m| m.range.start < range.start);
        self.mappings[index..].rotate_right(1);
        let result = (|| {
            // The shim uses distinct roots for the fixed render and compute
            // private namespaces. Both views share the caller's GEM pages.
            for view in [&self.space, &self.compute_space] {
                let mut space = view.lock();
                for &(start, end, pa) in &chunks {
                    space.low.map_pages(start..end, pa, access, single)?;
                    let last = if single {
                        pa
                    } else {
                        pa + end - start - 0x4000
                    };
                    if space.low.translate(start)? != Some(pa)
                        || space.low.translate(end - 0x4000)? != Some(last)
                    {
                        return Err(EIO);
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            // Keep the backing pinned if rollback itself cannot finish.
            self.space.lock().low.unmap_pages(range.clone())?;
            self.compute_space.lock().low.unmap_pages(range)?;
            self.publish_mappings(runtime);
            self.mappings[index..].rotate_left(1);
            self.mappings.pop();
            return Err(e);
        }
        self.publish_mappings(runtime);
        Ok(())
    }

    /// Publish translation changes before another job can use this VM.
    pub(super) fn publish_mappings(&self, runtime: &crate::g16::Bootstrap) {
        for view in [&self.space, &self.compute_space] {
            let space = view.lock();
            space.sync();
            runtime.invalidate_client(&space);
        }
    }

    /// Unmap every mapping of `obj`: a closed object whose deferred
    /// snapshot completed, or whose VM has drained.
    pub(super) fn unmap_object(&mut self, obj: &Object, runtime: &crate::g16::Bootstrap) -> Result {
        while let Some(range) = self.mappings.iter()
            .find(|m| core::ptr::eq(&*m.obj, obj))
            .map(|m| m.range.clone())
        {
            self.unmap(range, runtime)?;
        }
        Ok(())
    }

    pub(super) fn unmap(&mut self, range: Range<u64>, runtime: &crate::g16::Bootstrap) -> Result {
        self.snapshot = None;
        // Callers drain this VM's jobs and hold file state against new uses.
        // A failure may have signaled those fences while we waited. The
        // runtime lock prevents a concurrent failure during this teardown.
        if runtime.render_failed() {
            return Err(EIO);
        }
        // Mappings do not overlap, so removing one interval can split at most
        // one mapping into two. Reserve that one extra record before editing
        // PTEs. Whole-mapping teardown needs no new ownership allocation,
        // especially when GEM close follows an allocation failure.
        let right = self.mappings.iter().find(|m| {
            m.range.start < range.start && range.end < m.range.end
        }).map(|m| Mapping {
            range: range.end..m.range.end,
            offset: m.offset + if m.single { 0 } else { range.end - m.range.start },
            permissions: m.permissions,
            single: m.single,
            obj: m.obj.clone(),
            closing: m.closing,
        });
        if right.is_some() {
            self.mappings.reserve(1, GFP_KERNEL)?;
        }
        self.space.lock().low.reserve_invalidation()?;
        self.compute_space.lock().low.reserve_invalidation()?;
        // Unbinds may span holes, such as unbound sparse pages. Clear only
        // the parts which mappings cover.
        for m in self.mappings.iter() {
            let start = m.range.start.max(range.start);
            let end = m.range.end.min(range.end);
            if start < end {
                self.space.lock().low.unmap_pages(start..end)?;
                self.compute_space.lock().low.unmap_pages(start..end)?;
            }
        }
        self.publish_mappings(runtime);
        // Only release backing after both views and their invalidations are
        // complete. Any earlier failure preserves all original references.
        self.mappings.retain(|m| {
            if m.range.end <= range.start || m.range.start >= range.end {
                return true;
            }
            if m.range.start < range.start {
                m.range.end = range.start;
                return true;
            }
            if m.range.end > range.end {
                if !m.single {
                    m.offset += range.end - m.range.start;
                }
                m.range.start = range.end;
                return true;
            }
            false
        });
        if let Some(right) = right {
            let start = right.range.start;
            self.mappings.push_within_capacity(right)
                .unwrap_or_else(|_| unreachable!("reserved split mapping"));
            let last = self.mappings.len() - 1;
            let index = self.mappings[..last].partition_point(|m| m.range.start < start);
            self.mappings[index..].rotate_right(1);
        }
        Ok(())
    }
}
