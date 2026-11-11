/* SPDX-License-Identifier: GPL-2.0 */
#ifndef __MM_P3S_MEMORY_H
#define __MM_P3S_MEMORY_H

#include <linux/mm.h>

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
