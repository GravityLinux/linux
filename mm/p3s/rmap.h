/* SPDX-License-Identifier: GPL-2.0 */
#ifndef __MM_P3S_RMAP_H
#define __MM_P3S_RMAP_H

#include <linux/mm.h>
#include <linux/pagemap.h>

/*
 * p3s_pte_maps_folio - Check if a PTE maps the given normal folio at addr
 * @vma: Pointer to struct vm_area_struct
 * @addr: Virtual address corresponding to @ptent
 * @ptent: Page table entry value
 * @folio: Expected folio pointer
 */
static inline bool p3s_pte_maps_folio(struct vm_area_struct *vma,
				      unsigned long addr, pte_t ptent,
				      const struct folio *folio)
{
	return pte_present(ptent) &&
	       vm_normal_folio(vma, addr, ptent) == folio;
}

/*
 * p3s_count_folio_slices - Count how many PTEs in a slice window map @folio
 * @vma: Pointer to struct vm_area_struct
 * @pte_base: Starting PTE pointer of the slice window
 * @start_addr: Starting virtual address of the slice window
 * @nr_slices: Number of slices in the window
 * @folio: Target folio to count
 */
static inline unsigned int p3s_count_folio_slices(struct vm_area_struct *vma,
						  pte_t *pte_base,
						  unsigned long start_addr,
						  unsigned int nr_slices,
						  const struct folio *folio)
{
	unsigned int i, count = 0;

	for (i = 0; i < nr_slices; i++) {
		if (p3s_pte_maps_folio(vma, start_addr + (i << PAGE_SHIFT_4KB),
				       ptep_get(pte_base + i), folio))
			count++;
	}
	return count;
}

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
 * In a 4KB anonymous VMA without a file, whose page offsets count 4KB
 * pages, the walk starts at the first address of @folio's window in @vma.
 * A private mapping of /dev/zero indexes its folios by file page instead,
 * as vma_linear_page_index() does, so vma_address() already finds them.
 * Returns true for a VMA that holds no part of the window, which the
 * widened lookup can find in any mm.
 */
static inline bool p3s_anon_rmap_skip(const struct vm_area_struct *vma,
				      const struct folio *folio,
				      unsigned long *address)
{
	if (!IS_ENABLED(CONFIG_ARM64_PER_PROCESS_PAGE_SIZE) ||
	    folio_test_large(folio))
		return false;
	if (vma_is_p3s_4k(vma) && !vma->vm_ops && !vma->vm_file)
		*address = vma_anon_slice_window(vma, folio_pgoff(folio));
	return *address == -EFAULT;
}

/*
 * p3s_folio_mapped_by_slices - Check if other slice PTEs may map @folio
 * @vma: Pointer to struct vm_area_struct
 * @folio: Folio mapped by a PTE of @vma
 *
 * True for a small folio of a 4KB VMA with more than one mapping. Its slices
 * share one swap entry, so none of their swap PTEs may be exclusive.
 */
static inline bool p3s_folio_mapped_by_slices(const struct vm_area_struct *vma,
					      const struct folio *folio)
{
	return vma_is_p3s_4k(vma) && !folio_test_large(folio) &&
	       folio_mapcount(folio) > 1;
}

/*
 * p3s_folio_slices_in_window - Check that all mappings of @folio are here
 * @vma: Pointer to struct vm_area_struct
 * @folio: Folio about to be unmapped by reclaim or migration
 * @addr: Address of a PTE in @vma that maps @folio
 * @pte: That PTE, mapped and locked
 *
 * A 4KB compatibility task can map a small anonymous folio with several
 * slice PTEs, all covered by its one PageAnonExclusive flag. Unmapping the
 * first of them clears the flag (folio_try_share_anon_rmap_pte()), so the
 * other slices must be unmapped in the same walk: a slice left mapped
 * writable would reference a page that is no longer exclusive. Returns
 * true if every mapping of @folio is a PTE in the slice window at @addr,
 * which the walk covers under the page table lock it already holds.
 *
 * Once the first slice has cleared the flag, try_to_unmap_one() could only
 * stop at a later slice, and put back its possibly writable PTE, if
 * swap_duplicate() or arch_unmap_one() failed. Neither can: the latter
 * does nothing on arm64, and no swap PTE refers to the swap entry of an
 * exclusive folio when the walk starts, so the walk raises its count to at
 * most four, far below the count at which swap_duplicate() must allocate.
 */
static inline bool p3s_folio_slices_in_window(struct vm_area_struct *vma,
					      struct folio *folio,
					      unsigned long addr, pte_t *pte)
{
	unsigned int nr_slices;
	unsigned long start;

	if (!p3s_folio_mapped_by_slices(vma, folio))
		return true;

	vma_folio_slice_bounds(vma, addr, &nr_slices, &start);
	return p3s_count_folio_slices(vma,
				      pte - ((addr - start) >> PAGE_SHIFT_4KB),
				      start, nr_slices, folio) ==
	       folio_mapcount(folio);
}

/*
 * p3s_migration_share_slices - Keep sliced folios shared across migration
 * @vma: Pointer to struct vm_area_struct
 * @folio: Folio being migrated, whose PTE in @vma has just been cleared
 * @writable: In/out: whether to make a writable migration entry
 * @anon_exclusive: In/out: whether to make an exclusive migration entry
 *
 * remove_migration_pte() marks the folio exclusive again when it restores
 * a writable or exclusive migration entry. When several slice PTEs of a
 * 4KB VMA map a small anonymous folio, they share its single
 * PageAnonExclusive flag, so, as for their swap PTEs, none of their
 * migration entries may keep it: until all of them are restored, a 4KB
 * madvise() or mremap() can move the entry that would restore the flag out
 * of the part of the window that fork() copies, or the restore can reach
 * that entry before fork() copies the others, and the folio would come
 * back exclusive while the child maps it. Nor may a slice of a folio that
 * is not exclusive come back writable, such as one that
 * p3s_fork_wrprotect_siblings() could not reach because mremap() had moved
 * it out of the window.
 *
 * Such slices come back read-only, and do_wp_page() decides on the first
 * write whether the folio can be reused, as after a swap-in.
 */
static inline void p3s_migration_share_slices(const struct vm_area_struct *vma,
					      const struct folio *folio,
					      bool *writable,
					      bool *anon_exclusive)
{
	if (!vma_is_p3s_4k(vma) || !folio_test_anon(folio))
		return;
	if (p3s_folio_mapped_by_slices(vma, folio))
		*anon_exclusive = false;
	if (!*anon_exclusive)
		*writable = false;
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

/* Merge access bits while batching contiguous PTEs of one P3S folio. */
static inline int p3s_madvise_folio_pte_batch(struct vm_area_struct *vma,
					      unsigned long addr,
					      unsigned long end,
					      struct folio *folio, pte_t *ptep,
					      pte_t *ptentp)
{
	unsigned long size = mm_pte_size(vma->vm_mm);
	int max_nr = (end - addr) / size;
	int nr;

	/* Unlike mlock's threshold, count all slices of a small folio. */
	max_nr = min_t(int, max_nr, folio_size(folio) / size);
	for (nr = 1; nr < max_nr; nr++) {
		pte_t next = ptep_get(ptep + nr);

		if (!p3s_pte_maps_folio(vma, addr + nr * size, next, folio))
			break;
		if (pte_young(next))
			*ptentp = pte_mkyoung(*ptentp);
		if (pte_dirty(next))
			*ptentp = pte_mkdirty(*ptentp);
	}
	return nr;
}

/*
 * vma_anon_index_matches - Check an anonymous page's index against @addr
 * @vma: Pointer to struct vm_area_struct
 * @pgoff: Index of the page (page_pgoff())
 * @addr: Virtual address the page is mapped at
 *
 * In a 4KB anonymous VMA without a file, the index of a small folio is
 * that of the slice that faulted first, and its other slices are mapped in
 * the same 16KB window, so compare the windows. Elsewhere, including
 * private mappings of /dev/zero, whose folios are indexed by file page,
 * the index must match linear_page_index() exactly.
 */
static inline bool vma_anon_index_matches(const struct vm_area_struct *vma,
					  pgoff_t pgoff, unsigned long addr)
{
	unsigned long first;

	if (!vma_is_p3s_4k(vma) || vma->vm_ops || vma->vm_file)
		return pgoff == vma_linear_page_index(vma, addr);

	first = vma->vm_start + ((pgoff - vma->vm_pgoff) << PAGE_SHIFT_4KB);
	return (first & PAGE_MASK_KERNEL) == (addr & PAGE_MASK_KERNEL);
}

#endif /* __MM_P3S_RMAP_H */
