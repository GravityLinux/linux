/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_P3S_PROC_H
#define _LINUX_P3S_PROC_H

#include <linux/mm.h>
#include <linux/hugetlb.h>

/* Preserve architecture-specific MMU page sizes outside P3S VMAs. */
static inline unsigned long p3s_vma_mmu_pagesize(struct vm_area_struct *vma)
{
	if (vma_is_p3s_4k(vma))
		return vma_page_size(vma);
	return vma_mmu_pagesize(vma);
}

/*
 * vma_slice_ptes - Count the PTEs around @ptep that map @page
 * @vma: VMA that maps @addr
 * @ptep: PTE that maps @addr to @page
 * @addr: Address that @ptep maps
 * @page: Page that @ptep maps
 *
 * A native VMA maps a page with one PTE, so this returns 1. A 4KB VMA maps
 * slice i of the 16KB page that backs @addr, if at all, i - s 4KB pages
 * from @addr, where s is vma_slice_offset(@vma, @addr). Count the present
 * PTEs there, @ptep included, that map @page, without leaving @ptep's page
 * table. Each of them adds one to the mapcount of @page, so only this mm
 * maps @page if its mapcount is no larger than the count.
 */
static inline int vma_slice_ptes(struct vm_area_struct *vma, pte_t *ptep,
				 unsigned long addr, struct page *page)
{
	unsigned int s, i;
	int nr = 0;

	if (!vma_is_p3s_4k(vma))
		return 1;

	s = vma_slice_offset(vma, addr);
	for (i = 0; i < P3S_SLICES_PER_PAGE; i++) {
		long d = (long)i - (long)s;
		unsigned long a = addr + d * (long)PAGE_SIZE_4KB;
		pte_t pte;

		if ((a ^ addr) & ~(PMD_SIZE_4KB - 1))
			continue;
		pte = ptep_get(ptep + d);
		if (pte_present(pte) && pte_page(pte) == page)
			nr++;
	}
	return nr;
}

#endif /* _LINUX_P3S_PROC_H */
