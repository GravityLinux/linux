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
 * p3s_anon_rmap_range - Widen the anon_vma lookup of @folio to its window
 * @folio: Folio being walked
 * @start: First page offset to look up
 * @end: Last page offset to look up
 *
 * A small folio in a 4KB anonymous VMA can be mapped from the four slices
 * of one 16KB window, and a VMA split or a 4KB mremap() can leave some of
 * them in a VMA whose page range does not contain the folio's index. Look
 * up every page offset of the window.
 */
static inline void p3s_anon_rmap_range(const struct folio *folio,
				       pgoff_t *start, pgoff_t *end)
{
	if (!IS_ENABLED(CONFIG_ARM64_PER_PROCESS_PAGE_SIZE) ||
	    folio_test_large(folio))
		return;
	*start -= min_t(pgoff_t, *start, P3S_SLICE_MASK);
	*end += P3S_SLICE_MASK;
}

/*
 * p3s_anon_rmap_skip - Check whether an anon rmap walk of @folio skips @vma
 * @vma: VMA found by the lookup that p3s_anon_rmap_range() widened
 * @folio: Folio being walked
 * @address: vma_address() of @folio in @vma; the start of the walk on return
 *
 * In a 4KB anonymous VMA, whose page offsets count 4KB pages, the walk
 * starts at the first address of @folio's window in @vma. Returns true
 * for a VMA that holds no part of the window, which the widened lookup
 * can find in any mm.
 */
static inline bool p3s_anon_rmap_skip(const struct vm_area_struct *vma,
				      const struct folio *folio,
				      unsigned long *address)
{
	if (!IS_ENABLED(CONFIG_ARM64_PER_PROCESS_PAGE_SIZE) ||
	    folio_test_large(folio))
		return false;
	if (vma_is_p3s_4k(vma) && !vma->vm_ops)
		*address = vma_anon_slice_window(vma, folio_pgoff(folio));
	return *address == -EFAULT;
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
