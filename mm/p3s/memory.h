/* SPDX-License-Identifier: GPL-2.0 */
#ifndef __MM_P3S_MEMORY_H
#define __MM_P3S_MEMORY_H

#include <linux/mm.h>

/*
 * remap_pfn_range() may discard only the unused slices of the last native
 * page.  A request that reaches another native page is an invalid mapping,
 * even if that page lies in an adjacent VMA.  Validate before changing any
 * PTEs, so the failure path never has to zap beyond @vma.
 */
static inline int p3s_remap_pfn_range_size(struct vm_area_struct *vma,
					   unsigned long addr,
					   unsigned long *size)
{
	unsigned long available, rounded, tail;
	unsigned int last_slice;

	if (!vma_is_p3s_4k(vma))
		return 0;

	if (vma_slice_off(vma) && p3s_4k_unsupported(vma->vm_mm))
		return -EINVAL;

	if (addr < vma->vm_start || addr >= vma->vm_end ||
	    !IS_ALIGNED(addr, PAGE_SIZE_4KB) ||
	    !*size || *size > ULONG_MAX - (PAGE_SIZE_4KB - 1))
		return -EINVAL;

	rounded = PAGE_ALIGN_4KB(*size);
	available = vma->vm_end - addr;
	last_slice = vma_slice_offset(vma, vma->vm_end - PAGE_SIZE_4KB);
	tail = (P3S_SLICE_MASK - last_slice) * PAGE_SIZE_4KB;
	if (rounded > available) {
		if (rounded - available > tail)
			return -EINVAL;
		rounded = available;
	}
	*size = rounded;
	return 0;
}

/* Insert or verify the slices of a native PFN that lie in this VMA. */
static inline vm_fault_t p3s_insert_pfn_prot(struct vm_area_struct *vma,
					      unsigned long addr,
					      unsigned long pfn,
					      pgprot_t prot)
{
	struct mm_struct *mm = vma->vm_mm;
	unsigned long offset, first, end;
	unsigned int first_slice, count, first_count, present = 0, i;
	spinlock_t *ptl[2], *low_ptl, *high_ptl;
	pte_t *table[2], *pte;
	pmd_t *pmd[2], pmdval[2];
	vm_fault_t ret = VM_FAULT_SIGBUS;
	bool split;

	/* Clip the native PFN window to the VMA at either edge. */
	addr &= PAGE_MASK_4KB;
	offset = (unsigned long)vma_slice_offset(vma, addr) << PAGE_SHIFT_4KB;
	first = addr - min(offset, addr - vma->vm_start);
	end = addr + min(PAGE_SIZE_KERNEL - offset, vma->vm_end - addr);
	first_slice = vma_slice_offset(vma, first);
	count = (end - first) >> PAGE_SHIFT_4KB;
	first_count = min_t(unsigned int, count,
		(PMD_SIZE_4KB - (first & (PMD_SIZE_4KB - 1))) >>
		PAGE_SHIFT_4KB);
	split = first_count != count;

	table[0] = get_locked_pte(mm, first, &ptl[0]);
	if (!table[0])
		return VM_FAULT_OOM;
	if (split) {
		unsigned long second = first + first_count * PAGE_SIZE_4KB;
		pud_t *pud;

		pte_unmap_unlock(table[0], ptl[0]);
		table[1] = get_locked_pte(mm, second, &ptl[1]);
		if (!table[1])
			return VM_FAULT_OOM;
		pte_unmap_unlock(table[1], ptl[1]);

		/* Both tables exist; pin their mappings, then lock in pointer order. */
		pud = pud_offset(p4d_offset(pgd_offset(mm, first), first), first);
		pmd[0] = mm_pmd_offset(mm, pud, first);
		pud = pud_offset(p4d_offset(pgd_offset(mm, second), second), second);
		pmd[1] = mm_pmd_offset(mm, pud, second);
		table[0] = pte_offset_map_rw_nolock(mm, pmd[0], first,
						  &pmdval[0], &ptl[0]);
		if (!table[0])
			return VM_FAULT_NOPAGE;
		table[1] = pte_offset_map_rw_nolock(mm, pmd[1], second,
						  &pmdval[1], &ptl[1]);
		if (!table[1]) {
			pte_unmap(table[0]);
			return VM_FAULT_NOPAGE;
		}
		low_ptl = ptl[0];
		high_ptl = ptl[1];
		if (low_ptl > high_ptl)
			swap(low_ptl, high_ptl);
		spin_lock(low_ptl);
		if (high_ptl != low_ptl)
			spin_lock_nested(high_ptl, SINGLE_DEPTH_NESTING);
		if (!pmd_same(pmdval[0], pmdp_get_lockless(pmd[0])) ||
		    !pmd_same(pmdval[1], pmdp_get_lockless(pmd[1]))) {
			ret = VM_FAULT_NOPAGE;
			goto out;
		}
	}

	/* Preflight under the PTE lock(s): never fill a partial/wrong map. */
	for (i = 0; i < count; i++) {
		pte_t entry;

		pte = i < first_count ? table[0] + i :
					table[1] + i - first_count;
		entry = ptep_get(pte);

		if (pte_none(entry))
			continue;
		if (!pte_present(entry) ||
		    __pte_to_phys(entry) != PFN_PHYS(pfn) +
					    (first_slice + i) * PAGE_SIZE_4KB)
			goto out;
		present++;
	}
	if (present == count) {
		ret = VM_FAULT_NOPAGE;
		goto out;
	}
	if (present)
		goto out;

	for (i = 0; i < count; i++) {
		unsigned long page_addr = first + i * PAGE_SIZE_4KB;
		pte_t entry = pte_mkspecial(mm_frame_pte(mm,
				(pfn << P3S_SLICE_SHIFT) + first_slice + i,
				prot));

		pte = i < first_count ? table[0] + i :
					table[1] + i - first_count;
		set_pte_at(mm, page_addr, pte, entry);
		update_mmu_cache(vma, page_addr, pte);
	}
	ret = VM_FAULT_NOPAGE;
out:
	if (split) {
		if (high_ptl != low_ptl)
			spin_unlock(high_ptl);
		spin_unlock(low_ptl);
		pte_unmap(table[1]);
		pte_unmap(table[0]);
	} else {
		pte_unmap_unlock(table[0], ptl[0]);
	}
	return ret;
}

/*
 * The insert helpers of mm/memory.c, the only file that includes this
 * header, so they can stay static there.
 */
static int validate_page_before_insert(struct vm_area_struct *vma,
				       struct page *page);
static int insert_page_into_pte_locked(struct vm_area_struct *vma, pte_t *pte,
				       unsigned long addr, struct page *page,
				       pgprot_t prot, bool mkwrite);

/*
 * vma_insert_page_slices - insert_page() for a 4KB compatibility VMA
 * @vma: The 4KB compatibility VMA
 * @addr: Address to map @page at
 * @page: The kernel page to map
 * @prot: Protection of the PTEs
 *
 * A 4KB compatibility VMA maps a kernel page with a PTE per 4KB slice. Map
 * every slice of @page whose address lies in @vma, @addr mapping slice
 * vma_slice_offset(@vma, @addr). Sibling slices whose PTEs are in use are
 * skipped; returns -EBUSY if the PTE at @addr is in use.
 */
static inline int vma_insert_page_slices(struct vm_area_struct *vma,
					 unsigned long addr, struct page *page,
					 pgprot_t prot)
{
	unsigned long off = (unsigned long)vma_slice_offset(vma, addr) <<
			    PAGE_SHIFT_4KB;
	unsigned long target = addr & PAGE_MASK_4KB;
	unsigned long end;
	spinlock_t *ptl;
	pte_t *pte;
	int ret = 0;

	addr &= PAGE_MASK_4KB;
	end = min(addr + PAGE_SIZE_KERNEL - off, vma->vm_end);
	addr -= min(off, addr - vma->vm_start);
	if (addr >= end)
		return -EFAULT;

	for (; addr < end && !ret; addr += PAGE_SIZE_4KB) {
		pte = get_locked_pte(vma->vm_mm, addr, &ptl);
		if (!pte)
			return -ENOMEM;
		ret = insert_page_into_pte_locked(vma, pte, addr, page, prot,
						  false);
		if (ret == -EBUSY && addr != target)
			ret = 0;
		pte_unmap_unlock(pte, ptl);
	}
	return ret;
}

/*
 * vma_insert_pages_slices - insert_pages() for a 4KB compatibility VMA
 * @vma: The 4KB compatibility VMA
 * @addr: Address to map the first page at
 * @pages: The kernel pages to map
 * @num: Number of pages; returns the number of pages not mapped
 * @prot: Protection of the PTEs
 *
 * Maps one kernel page at a time with vma_insert_page_slices(). The first
 * page maps at @addr, and each later page starts at slice 0 of its kernel
 * page, right after the last slice of the previous page.
 */
static inline int vma_insert_pages_slices(struct vm_area_struct *vma,
					  unsigned long addr,
					  struct page **pages,
					  unsigned long *num, pgprot_t prot)
{
	int ret;

	for (; *num; (*num)--, pages++) {
		unsigned long off = vma_slice_offset(vma, addr) * PAGE_SIZE_4KB;

		ret = validate_page_before_insert(vma, *pages);
		if (!ret)
			ret = vma_insert_page_slices(vma, addr, *pages, prot);
		if (ret)
			return ret;
		addr += PAGE_SIZE_KERNEL - off;
	}
	return 0;
}

/*
 * p3s_fork_wrprotect_siblings - Write-protect sibling slices of a shared folio
 * @src_vma: The VMA that fork() copies
 * @folio: Folio that fork() has just shared with the child through @src_pte
 * @src_pte: PTE of @src_vma that maps @folio, with its page table locked
 * @addr: Address of @src_pte
 *
 * Up to four slice PTEs of a 16KB window in a 4KB VMA can map one small
 * anonymous folio, and a 4KB madvise() can split the window between VMAs.
 * fork() write-protects the PTEs of the VMAs that it copies, but a sibling
 * in a VMA that it skips (MADV_DONTFORK, MADV_WIPEONFORK) would stay
 * writable on a folio that is no longer exclusive:
 *
 *              slice 0    slice 1    slice 2    slice 3
 *            ┌──────────┬──────────┬──────────┬──────────┐
 *    parent  │    RO    │ RW ─► RO │    RO    │    RO    │
 *            ├──────────┼──────────┼──────────┼──────────┤
 *    child   │    RO    │          │    RO    │    RO    │
 *            └──────────┴──────────┴──────────┴──────────┘
 *                         DONTFORK
 *
 * Write-protect every other PTE of the window that maps @folio writable,
 * as __copy_present_ptes() does for @src_pte. The window follows the slice
 * offset of @src_vma and is clipped to the PTE table of @src_pte, as in
 * vma_folio_slice_bounds(), so @src_pte's page table lock covers it. The
 * MMU notifier range of copy_page_range() covers it too, through
 * p3s_fork_widen_range(), and dup_mmap() flushes the parent's TLB at the
 * end. Its PTEs can belong to other VMAs, so match @folio by PFN rather
 * than through vm_normal_folio().
 */
static inline void p3s_fork_wrprotect_siblings(struct vm_area_struct *src_vma,
					       struct folio *folio,
					       pte_t *src_pte,
					       unsigned long addr)
{
	unsigned long off, pt, start, end;
	pte_t *pte;

	if (!vma_is_p3s_4k(src_vma) || folio_test_large(folio))
		return;

	/* The slice window of @addr, clipped to its PTE table */
	off = (unsigned long)vma_slice_offset(src_vma, addr) << PAGE_SHIFT_4KB;
	pt = addr & ~(PMD_SIZE_4KB - 1);
	start = addr - min(off, addr - pt);
	end = min(addr - off + PAGE_SIZE_KERNEL, pt + PMD_SIZE_4KB);
	pte = src_pte - ((addr - start) >> PAGE_SHIFT_4KB);
	for (; start < end; start += PAGE_SIZE_4KB, pte++) {
		pte_t ptent = ptep_get(pte);

		if (pte != src_pte && pte_present(ptent) && pte_write(ptent) &&
		    pte_pfn(ptent) == folio_pfn(folio))
			wrprotect_ptes(src_vma->vm_mm, start, pte, 1);
	}
}

/*
 * p3s_fork_widen_range - Widen the MMU notifier range of fork() to windows
 * @src_vma: The VMA that fork() copies
 * @start: In/out start of the range, initially @src_vma->vm_start
 * @end: In/out end of the range, initially @src_vma->vm_end
 *
 * In a 4KB VMA, p3s_fork_wrprotect_siblings() can write-protect PTEs of
 * other VMAs in the slice windows at either end of @src_vma, so secondary
 * MMUs must drop their translations too.
 */
static inline void p3s_fork_widen_range(const struct vm_area_struct *src_vma,
					unsigned long *start,
					unsigned long *end)
{
	unsigned long off;

	if (!vma_is_p3s_4k(src_vma))
		return;
	off = (unsigned long)vma_slice_offset(src_vma, *start) <<
	      PAGE_SHIFT_4KB;
	*start -= min(off, *start);
	off = (unsigned long)vma_slice_offset(src_vma, *end - PAGE_SIZE_4KB) <<
	      PAGE_SHIFT_4KB;
	*end += PAGE_SIZE_KERNEL - PAGE_SIZE_4KB - off;
}

#endif /* __MM_P3S_MEMORY_H */
