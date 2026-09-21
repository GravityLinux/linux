/* SPDX-License-Identifier: GPL-2.0 */
#ifndef __MM_P3S_RMAP_H
#define __MM_P3S_RMAP_H

#include <linux/mm.h>
#include <linux/pagemap.h>

/*
 * vma_slice_walk_end - End of an rmap walk for a small folio from @addr
 * @vma: Pointer to struct vm_area_struct
 * @addr: Virtual address within @vma
 *
 * For 4KB compatibility VMAs, the end of the slice window at @addr within
 * @vma, so that the walk visits every slice PTE that can map the folio.
 * One page past @addr for other VMAs.
 */
static inline unsigned long vma_slice_walk_end(const struct vm_area_struct *vma,
					       unsigned long addr)
{
	unsigned long off;

	if (!vma_is_p3s_4k(vma))
		return addr + vma_page_size(vma);

	off = (unsigned long)vma_slice_offset(vma, addr) << PAGE_SHIFT_4KB;
	addr &= PAGE_MASK_4KB;
	return addr + min(PAGE_SIZE_KERNEL - off, vma->vm_end - addr);
}

/*
 * vma_folio_nr_ptes - Number of PTEs that map all of @folio in @vma
 * @vma: Pointer to struct vm_area_struct
 * @folio: Folio mapped by @vma
 *
 * The rmap walks mlock a large folio only once they have counted this many
 * PTEs in a VM_LOCKED VMA. A 4KB VMA maps each page of the folio with four
 * PTEs, one per slice. A small folio is still mlocked on its first PTE.
 */
static inline unsigned long vma_folio_nr_ptes(const struct vm_area_struct *vma,
					      const struct folio *folio)
{
	if (vma_is_p3s_4k(vma) && folio_test_large(folio))
		return folio_nr_pages(folio) << P3S_SLICE_SHIFT;
	return folio_nr_pages(folio);
}

#endif /* __MM_P3S_RMAP_H */
