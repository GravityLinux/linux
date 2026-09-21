/* SPDX-License-Identifier: GPL-2.0 */
#ifndef __MM_P3S_ANON_H
#define __MM_P3S_ANON_H

#include <linux/highmem.h>
#include <linux/mm.h>
#include <linux/pagemap.h>
#include <linux/rmap.h>
#include <linux/swap.h>
#include <linux/userfaultfd_k.h>

#include "pgtable.h"
#include "rmap.h"

#ifdef CONFIG_ARM64_PER_PROCESS_PAGE_SIZE
#include <asm/p3s/mte.h>

/*
 * p3s_anon_folio_unreferenced - Check that only its mappings hold @folio
 * @folio: Locked small anonymous folio that is not in the swap cache
 *
 * A GUP reference or pin (vmsplice(), O_DIRECT, io_uring fixed buffers)
 * or the pending TLB flush of a zap can still use a slice of @folio that
 * no PTE maps, so such a slice must not be cleared and mapped again.
 */
static inline bool p3s_anon_folio_unreferenced(struct folio *folio)
{
	if (folio_ref_count(folio) == folio_mapcount(folio))
		return true;

	/*
	 * A new folio may still sit in this CPU's LRU batch, and an mlocked
	 * one in its mlock batch after another slice of a VM_LOCKED VMA
	 * mapped it.
	 */
	if (!folio_test_lru(folio) || folio_test_mlocked(folio))
		lru_add_drain();
	return folio_ref_count(folio) == folio_mapcount(folio);
}

/*
 * p3s_anon_folio_lookaround_pte - Find adjacent anonymous folio in slice window
 * @vma: Pointer to struct vm_area_struct
 * @pte: Faulting PTE pointer
 * @addr: Faulting virtual address
 *
 * Returns a referenced sibling folio whose slice for @addr is unused, or
 * NULL. A folio qualifies only if every mapping of it is a sibling PTE in
 * this window and nothing else references it: a slice moved elsewhere (for
 * example by mremap()), still referenced by a swap entry (folio in the swap
 * cache) or by GUP, or replaced by a migration entry is in use, and mapping
 * it again would alias it. The folio lock excludes in-flight migration and
 * reclaim.
 */
static inline struct folio *p3s_anon_folio_lookaround_pte(struct vm_area_struct *vma,
							  pte_t *pte,
							  unsigned long addr)
{
	unsigned int total_slices;
	unsigned long window_start;
	pte_t *pte_base;
	unsigned int i;

	if (!vma_is_p3s_4k(vma))
		return NULL;

	vma_folio_slice_bounds(vma, addr, &total_slices, &window_start);
	if (total_slices <= 1)
		return NULL;

	pte_base = pte - ((addr - window_start) >> PAGE_SHIFT_4KB);

	for (i = 0; i < total_slices; i++) {
		unsigned long slice_addr = window_start + (i << PAGE_SHIFT_4KB);
		pte_t ptent = ptep_get(pte_base + i);
		struct folio *f;
		bool ok;

		if (!pte_present(ptent))
			continue;

		f = vm_normal_folio(vma, slice_addr, ptent);
		if (!f || !folio_test_anon(f) || folio_test_large(f) ||
		    folio_test_ksm(f) || folio_test_hwpoison(f))
			continue;

		if (!PageAnonExclusive(&f->page))
			continue;

		if (!folio_trylock(f))
			continue;
		ok = !folio_test_swapcache(f) &&
		     p3s_count_folio_slices(vma, pte_base, window_start,
					    total_slices, f) == folio_mapcount(f) &&
		     p3s_anon_folio_unreferenced(f) &&
		     folio_try_get(f);
		folio_unlock(f);
		if (ok)
			return f;
	}

	return NULL;
}

/*
 * p3s_anon_folio_reuse - Update accounting when reusing an existing folio
 * @folio: Anonymous folio being reused
 * @vma: Pointer to struct vm_area_struct
 * @addr: Virtual address of the slice being mapped
 *
 * The folio reference taken by p3s_anon_folio_lookaround_pte() is
 * transferred to the new slice mapping. Account the mapping through the
 * rmap, which also mlocks the folio once more in a VM_LOCKED VMA.
 */
static inline void p3s_anon_folio_reuse(struct folio *folio,
					struct vm_area_struct *vma,
					unsigned long addr)
{
	folio_add_anon_rmap_pte(folio, &folio->page, vma, addr, RMAP_NONE);
	inc_mm_counter(vma->vm_mm, MM_ANONPAGES);
}

#ifdef CONFIG_USERFAULTFD
/*
 * p3s_uffd_install_anon_folio - Look around and reuse existing folio slice for userfaultfd
 * @dst_vma: Destination VMA
 * @dst_pte: Destination PTE pointer
 * @dst_addr: Destination virtual address
 * @folio: Newly allocated source folio
 * @dst_pte_val: Output PTE value to install
 * @writable: Whether the mapping should be writable
 * @flags: Userfaultfd atomic flags
 *
 * Returns true if an existing slice was reused and folio_put was called on @folio.
 */
static inline bool p3s_uffd_install_anon_folio(struct vm_area_struct *dst_vma,
					       pte_t *dst_pte,
					       unsigned long dst_addr,
					       struct folio *folio,
					       pte_t *dst_pte_val,
					       bool writable,
					       uffd_flags_t flags)
{
	struct folio *existing_folio =
		p3s_anon_folio_lookaround_pte(dst_vma, dst_pte, dst_addr);
	unsigned long offset;

	if (!existing_folio)
		return false;

	offset = (unsigned long)vma_slice_offset(dst_vma, dst_addr) << PAGE_SHIFT_4KB;

	/* The data is replaced below; the old allocation tags must go too */
	if (dst_vma->vm_flags & VM_MTE)
		mte_zero_clear_slice_tags(folio_address(existing_folio) + offset);
	memcpy(folio_address(existing_folio) + offset,
	       folio_address(folio) + offset, PAGE_SIZE_4KB);
	flush_dcache_folio(existing_folio);

	/* Order the copy before the PTE, as __folio_mark_uptodate() does */
	smp_wmb();

	p3s_anon_folio_reuse(existing_folio, dst_vma, dst_addr);
	*dst_pte_val = vma_pte_add_slice(dst_vma, dst_addr,
					 folio_mk_pte(existing_folio,
						      dst_vma->vm_page_prot));
	*dst_pte_val = pte_mkdirty(*dst_pte_val);
	if (writable)
		*dst_pte_val = pte_mkwrite(*dst_pte_val, dst_vma);
	if (flags & MFILL_ATOMIC_WP)
		*dst_pte_val = pte_mkuffd_wp(*dst_pte_val);

	folio_put(folio);
	return true;
}

/*
 * p3s_uffd_move_nr_slices - Number of PTEs that UFFDIO_MOVE moves with @folio
 * @vma: Source VMA
 * @addr: Source address that @pte maps
 * @end: End of the source range of this move
 * @pte: Source PTE that maps @folio, page table locked
 * @dst_pte: Destination PTE for @addr, page table locked
 * @folio: Small anonymous folio that @pte maps, locked
 *
 * UFFDIO_MOVE points the anon_vma and index of a folio at the destination,
 * so the rmap only reaches the folio if every PTE that maps it moves in the
 * same page table lock hold. A 4KB task can map a small folio with up to
 * four slice PTEs of one 16KB window, for example slices 1 to 3:
 *
 *              @addr
 *                ▼
 *          ┌─────┬─────┬─────┬─────┐
 *          │  -  │  1  │  2  │  3  │  source window
 *          └─────┴─────┴─────┴─────┘
 *                │◄── returns 3 ──►│
 *
 * Returns the number of consecutive PTEs from @pte that map @folio, if
 * they are all of its mappings, end within @end and the window of @addr,
 * and face none destination PTEs. Otherwise returns 0 and the folio must
 * not move: moving some of its slices would leave it mapped from two
 * windows, so reclaim could not unmap it and unmapping the destination
 * could free the anon_vma that it points to.
 *
 * UFFDIO_MOVE then fails with -EBUSY, as it does for a pinned or shared
 * folio. That includes moving a single 4KB page whose window siblings
 * share its folio, so only whole folios move without a copy. Userspace
 * falls back to UFFDIO_COPY on -EBUSY, as ART's heap compaction does.
 */
static inline unsigned int p3s_uffd_move_nr_slices(struct vm_area_struct *vma,
						   unsigned long addr,
						   unsigned long end,
						   pte_t *pte, pte_t *dst_pte,
						   struct folio *folio)
{
	unsigned long window_end = PAGE_ALIGN_DOWN_KERNEL(addr) + PAGE_SIZE_KERNEL;
	unsigned int nr, i;

	if (!p3s_folio_mapped_by_slices(vma, folio))
		return 1;

	nr = folio_mapcount(folio);
	if (addr + ((unsigned long)nr << PAGE_SHIFT_4KB) > min(end, window_end))
		return 0;

	for (i = 1; i < nr; i++) {
		pte_t ptent = ptep_get(pte + i);

		if (!pte_present(ptent) || pte_pfn(ptent) != folio_pfn(folio) ||
		    !pte_none(ptep_get(dst_pte + i)))
			return 0;
	}
	return nr;
}
#endif /* CONFIG_USERFAULTFD */

#else /* !CONFIG_ARM64_PER_PROCESS_PAGE_SIZE */

#ifdef CONFIG_USERFAULTFD
static inline bool p3s_uffd_install_anon_folio(struct vm_area_struct *dst_vma,
					       pte_t *dst_pte,
					       unsigned long dst_addr,
					       struct folio *folio,
					       pte_t *dst_pte_val,
					       bool writable,
					       uffd_flags_t flags)
{
	return false;
}

static inline unsigned int p3s_uffd_move_nr_slices(struct vm_area_struct *vma,
						   unsigned long addr,
						   unsigned long end,
						   pte_t *pte, pte_t *dst_pte,
						   struct folio *folio)
{
	return 1;
}
#endif

#endif /* CONFIG_ARM64_PER_PROCESS_PAGE_SIZE */

#endif /* __MM_P3S_ANON_H */
