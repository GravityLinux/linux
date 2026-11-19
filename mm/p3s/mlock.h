/* SPDX-License-Identifier: GPL-2.0 */
#ifndef __MM_P3S_MLOCK_H
#define __MM_P3S_MLOCK_H

#include <linux/p3s.h>
#include "rmap.h"

/*
 * Handle a large folio in a 4KB VMA, returning the number of slice PTEs
 * consumed, or zero to leave native and small-folio handling unchanged.
 * The caller holds the page-table lock covering [addr, end) and verifies
 * that the first PTE maps @folio.
 */
static inline unsigned int p3s_mlock_folio(struct folio *folio,
					   struct vm_area_struct *vma, pte_t *pte,
					   unsigned long addr, unsigned long end)
{
	phys_addr_t phys;
	unsigned long offset;
	unsigned int nr, max_nr;

	if (!vma_is_p3s_4k(vma) || !folio_test_large(folio))
		return 0;

	phys = __pte_to_phys(ptep_get(pte));
	offset = phys - ((phys_addr_t)folio_pfn(folio) << PAGE_SHIFT_KERNEL);
	max_nr = min(end - addr, folio_size(folio) - offset) >> PAGE_SHIFT_4KB;
	for (nr = 1; nr < max_nr; nr++) {
		pte_t next = ptep_get(pte + nr);
		unsigned long next_addr = addr + (nr << PAGE_SHIFT_4KB);

		if (!p3s_pte_maps_folio(vma, next_addr, next, folio) ||
		    __pte_to_phys(next) != phys + ((phys_addr_t)nr << PAGE_SHIFT_4KB))
			break;
	}

	/*
	 * Only a complete folio, mapped by consecutive slices under this lock,
	 * can be mlocked. Derive its coverage from the PTEs, not mixed native
	 * file offsets and process-page VMA lengths. Partial munlock is allowed,
	 * as in the native path; reclaim restores a remaining locked mapping.
	 */
	if (!(vma->vm_flags & VM_LOCKED))
		munlock_folio(folio);
	else if (!offset && nr == vma_folio_nr_ptes(vma, folio))
		mlock_folio(folio);
	return nr;
}

/* Complete the same check when the last missing file slice is faulted in. */
static inline void p3s_mlock_file_folio(struct folio *folio,
					struct vm_area_struct *vma,
					pte_t *pte, unsigned long addr)
{
	unsigned long offset, start, size;

	if (!vma_is_p3s_4k(vma) || !folio_test_large(folio) ||
	    folio_test_anon(folio) ||
	    (vma->vm_flags & (VM_LOCKED | VM_SPECIAL)) != VM_LOCKED)
		return;

	offset = __pte_to_phys(ptep_get(pte)) -
		 ((phys_addr_t)folio_pfn(folio) << PAGE_SHIFT_KERNEL);
	if (offset > addr - vma->vm_start)
		return;
	start = addr - offset;
	size = folio_size(folio);
	if (size > vma->vm_end - start ||
	    size > PMD_SIZE_4KB - (start & (PMD_SIZE_4KB - 1)))
		return;

	pte -= offset >> PAGE_SHIFT_4KB;
	if (p3s_pte_maps_folio(vma, start, ptep_get(pte), folio))
		p3s_mlock_folio(folio, vma, pte, start, start + size);
}

#endif /* __MM_P3S_MLOCK_H */
