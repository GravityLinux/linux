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

#endif /* __MM_P3S_MEMORY_H */
