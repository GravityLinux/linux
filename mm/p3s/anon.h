/* SPDX-License-Identifier: GPL-2.0 */
#ifndef __MM_P3S_ANON_H
#define __MM_P3S_ANON_H

#include <linux/highmem.h>
#include <linux/leafops.h>
#include <linux/mm.h>
#include <linux/oom.h>
#include <linux/pagemap.h>
#include <linux/rmap.h>
#include <linux/swap.h>
#include <linux/userfaultfd_k.h>

#include "../internal.h"
#include "../swap.h"
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
 * p3s_anon_folio_zero_slice - Zero the subpage slice of @folio backing @addr
 * @folio: Existing anonymous folio about to be mapped at @addr
 * @vma: Pointer to struct vm_area_struct
 * @addr: Virtual address of the slice being mapped
 *
 * A slice reused through sibling lookaround may still hold the contents of
 * an earlier mapping of the same slice that was zapped (for example by
 * MADV_DONTNEED or a partial munmap()) while sibling slices kept the folio
 * alive. Anonymous faults must observe zero-fill semantics, so clear the
 * slice before it is mapped again. In a PROT_MTE mapping clear its
 * allocation tags as well: mte_sync_tags() does not reinitialise the tags
 * of a host page that is already tagged.
 */
static inline void p3s_anon_folio_zero_slice(struct folio *folio,
					     struct vm_area_struct *vma,
					     unsigned long addr)
{
	unsigned long offset;
	void *kaddr;

	offset = (unsigned long)vma_slice_offset(vma, addr) << PAGE_SHIFT_4KB;
	kaddr = folio_address(folio) + offset;
	if (vma->vm_flags & VM_MTE)
		mte_zero_clear_slice_tags(kaddr);
	else
		memset(kaddr, 0, PAGE_SIZE_4KB);
	flush_dcache_folio(folio);

	/* Order the clear before the PTE, as __folio_mark_uptodate() does */
	smp_wmb();
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

/*
 * p3s_anon_mk_slice_pte - Map the faulting slice of a sibling folio
 * @vmf: Fault information; page table lock held, faulting slice not mapped
 * @folio: Sibling folio returned by p3s_anon_folio_lookaround_pte()
 *
 * Zero the slice and account the new mapping without allocating a new
 * folio. Returns the PTE for the caller to install.
 */
static inline pte_t p3s_anon_mk_slice_pte(struct vm_fault *vmf,
					  struct folio *folio)
{
	struct vm_area_struct *vma = vmf->vma;
	pte_t entry;

	p3s_anon_folio_zero_slice(folio, vma, vmf->address);

	entry = vma_pte_add_slice(vma, vmf->address,
				  folio_mk_pte(folio, vma->vm_page_prot));
	entry = pte_sw_mkyoung(entry);
	if (vma->vm_flags & VM_WRITE)
		entry = pte_mkwrite(pte_mkdirty(entry), vma);

	p3s_anon_folio_reuse(folio, vma, vmf->address);
	return entry;
}

/*
 * p3s_anon_reuse_sibling_slice - Map a fault onto a sibling's 16KB folio
 * @vmf: Fault information (pte not mapped, not locked)
 * @ret: Fault result if the fault was handled
 *
 * In a 4KB compatibility process, an anonymous fault on a subpage slice
 * whose 16KB host folio is already mapped by a sibling slice maps that
 * folio instead of allocating a new one. Returns true if the fault was
 * handled. Userfaultfd-missing VMAs and faults on PTE markers (e.g.
 * uffd-wp) take the regular path. A fault handled here does not reach the
 * android_vh_do_anonymous_page vendor hook, which do_anonymous_page() only
 * calls for a newly allocated folio.
 */
static inline bool p3s_anon_reuse_sibling_slice(struct vm_fault *vmf,
						vm_fault_t *ret)
{
	struct vm_area_struct *vma = vmf->vma;
	struct folio *folio;
	pte_t entry;

	*ret = 0;
	if (!vma_is_p3s_4k(vma) || userfaultfd_missing(vma) ||
	    ((vmf->flags & FAULT_FLAG_ORIG_PTE_VALID) && !pte_none(vmf->orig_pte)))
		return false;

	vmf->pte = pte_offset_map_lock(vma->vm_mm, vmf->pmd, vmf->address,
				       &vmf->ptl);
	if (!vmf->pte)
		return true;
	if (!pte_none(ptep_get(vmf->pte))) {
		update_mmu_tlb(vma, vmf->address, vmf->pte);
		goto unlock;
	}

	folio = p3s_anon_folio_lookaround_pte(vma, vmf->pte, vmf->address);
	if (!folio) {
		pte_unmap_unlock(vmf->pte, vmf->ptl);
		vmf->pte = NULL;
		return false;
	}

	*ret = check_stable_address_space(vma->vm_mm);
	if (*ret) {
		folio_put(folio);
		goto unlock;
	}
	entry = p3s_anon_mk_slice_pte(vmf, folio);
	set_pte_at(vma->vm_mm, vmf->address, vmf->pte, entry);
	update_mmu_cache_range(vmf, vma, vmf->address, vmf->pte, 1);
unlock:
	pte_unmap_unlock(vmf->pte, vmf->ptl);
	return true;
}

/*
 * p3s_anon_reuse_racing_sibling - Map a sibling folio instead of a new one
 * @vmf: Fault information; page table lock held, faulting slice not mapped
 * @foliop: Newly allocated folio, replaced by the sibling folio if found
 * @entry: Set to the PTE of the faulting slice if a sibling folio is found
 *
 * do_anonymous_page() normally finds a sibling folio before it allocates
 * one (p3s_anon_reuse_sibling_slice()). This lookaround catches a sibling
 * slice mapped while the new folio was being allocated, and serves faults
 * on uffd-wp markers, which skip the early lookaround:
 *
 *   ┌─────────┬─────────┬─────────┬─────────┐
 *   │ slice 0 │  @vmf   │ slice 2 │  none   │  PTEs of the 16KB window
 *   └────┬────┴────┬────┴────┬────┴─────────┘
 *        ▼         ▼         ▼
 *   ┌─────────┬─────────┬─────────┬─────────┐
 *   │    0    │    1    │    2    │    3    │  slices of the sibling folio
 *   └─────────┴─────────┴─────────┴─────────┘
 *
 * Returns true if the sibling folio replaced the new folio, which is then
 * freed after the android_vh_do_anonymous_page vendor hook has seen it.
 */
static inline bool p3s_anon_reuse_racing_sibling(struct vm_fault *vmf,
						 struct folio **foliop,
						 pte_t *entry)
{
	struct folio *folio;

	folio = p3s_anon_folio_lookaround_pte(vmf->vma, vmf->pte, vmf->address);
	if (!folio)
		return false;

	folio_put(*foliop);
	*foliop = folio;
	*entry = p3s_anon_mk_slice_pte(vmf, folio);
	return true;
}

/*
 * p3s_swapin_free_sibling_swap - Free the swap entries of sibling slices
 * @vmf: Fault information, with the page table lock held
 * @folio: Folio that do_swap_page() is about to map, locked
 * @entry: Swap entry of the faulting PTE
 *
 * Reclaim replaced every PTE that mapped @folio with a swap PTE for @entry.
 * Restore the siblings with the faulting slice; otherwise their swap PTEs
 * keep @folio shared and each write copies its slice into a new folio:
 *
 *             slice 0   slice 1   slice 2   slice 3
 *           ┌─────────┬─────────┬─────────┬─────────┐
 *  before   │ swap E  │ swap E  │  none   │ swap E  │  fault on slice 1
 *           ├─────────┼─────────┼─────────┼─────────┤
 *  after    │  F[0]   │  F[1]   │  none   │  F[3]   │  F: @folio, E: @entry
 *           └─────────┴─────────┴─────────┴─────────┘
 *
 * Drops the swap reference of each such sibling, so that do_swap_page()
 * can free the swap cache (the locked @folio keeps @entry allocated until
 * then), and returns their mask (0 and 3 above). None of them is
 * exclusive: try_to_unmap_one() marks a swap PTE exclusive only while a
 * single slice maps the folio.
 */
static inline unsigned long p3s_swapin_free_sibling_swap(struct vm_fault *vmf,
							 struct folio *folio,
							 swp_entry_t entry)
{
	struct vm_area_struct *vma = vmf->vma;
	unsigned long start, mask = 0;
	unsigned int nr_slices, i;
	pte_t *pte_base;

	if (!vma_is_p3s_4k(vma) || folio_test_large(folio) ||
	    folio_test_ksm(folio) || !folio_test_swapcache(folio))
		return 0;

	vma_folio_slice_bounds(vma, vmf->address, &nr_slices, &start);
	pte_base = vmf->pte - ((vmf->address - start) >> PAGE_SHIFT_4KB);
	for (i = 0; i < nr_slices; i++) {
		pte_t ptent = ptep_get(pte_base + i);

		if (pte_base + i == vmf->pte || pte_none(ptent) ||
		    pte_present(ptent) ||
		    softleaf_from_pte(ptent).val != entry.val)
			continue;
		VM_WARN_ON_ONCE(pte_swp_exclusive(ptent));
		folio_put_swap(folio, folio_page(folio, 0));
		mask |= BIT(i);
	}

	return mask;
}

/*
 * p3s_swapin_map_sibling_slices - Restore the slices swapped out with a folio
 * @vmf: Fault information, with the page table lock held
 * @folio: Folio that do_swap_page() mapped at the faulting address
 * @mask: Slices returned by p3s_swapin_free_sibling_swap()
 * @rmap_flags: RMAP_EXCLUSIVE if do_swap_page() mapped the folio exclusive
 *
 * Map each slice in @mask to its slice of @folio, keeping the soft-dirty
 * and uffd-wp bits of its swap PTE. The slices come back only because they
 * share the swap entry, so map them old where the architecture wants that
 * for prefaulted PTEs, as fault-around does. Like the faulting PTE, a
 * slice of an exclusive folio is mapped writable (clean) unless uffd-wp or
 * soft-dirty tracking must see its first write; otherwise that write
 * reuses the exclusive folio without a copy. The android_vh_do_swap_page
 * vendor hook only sees the faulting PTE, not these slices.
 */
static inline void p3s_swapin_map_sibling_slices(struct vm_fault *vmf,
						 struct folio *folio,
						 unsigned long mask,
						 rmap_t rmap_flags)
{
	struct vm_area_struct *vma = vmf->vma;
	struct page *page = folio_page(folio, 0);
	unsigned int nr_slices, i;
	unsigned long start;
	pte_t *pte_base;

	if (!mask)
		return;

	vma_folio_slice_bounds(vma, vmf->address, &nr_slices, &start);
	pte_base = vmf->pte - ((vmf->address - start) >> PAGE_SHIFT_4KB);
	for_each_set_bit(i, &mask, nr_slices) {
		unsigned long addr = start + (i << PAGE_SHIFT_4KB);
		pte_t orig_pte = ptep_get(pte_base + i);
		bool rwp_restore = false;
		pte_t pte;

		pte = vma_pte_add_slice(vma, addr,
					mk_pte(page, vma->vm_page_prot));
		if (pte_swp_soft_dirty(orig_pte))
			pte = pte_mksoft_dirty(pte);
		if (pte_swp_uffd(orig_pte))
			pte = pte_mkuffd(pte);
		/* As for the faulting PTE: keep an RWP-protected slice trapping. */
		if (pte_swp_uffd(orig_pte) && userfaultfd_rwp(vma)) {
			pte = pte_modify(pte, PAGE_NONE);
			rwp_restore = true;
		}
		if (arch_wants_old_prefaulted_pte())
			pte = pte_mkold(pte);
		if ((rmap_flags & RMAP_EXCLUSIVE) && !rwp_restore &&
		    (vma->vm_flags & VM_WRITE) &&
		    !userfaultfd_pte_wp(vma, pte) &&
		    !pte_needs_soft_dirty_wp(vma, pte))
			pte = pte_mkwrite(pte, vma);

		folio_get(folio);
		folio_add_anon_rmap_ptes(folio, page, 1, vma, addr, rmap_flags);
		add_mm_counter(vma->vm_mm, MM_ANONPAGES, 1);
		add_mm_counter(vma->vm_mm, MM_SWAPENTS, -1);
		set_pte_at(vma->vm_mm, addr, pte_base + i, pte);
		arch_do_swap_page_nr(vma->vm_mm, vma, addr, pte, pte, 1);
		update_mmu_cache_range(vmf, vma, addr, pte_base + i, 1);
	}
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

	/* The copy below replaces all of the data, but not the old tags */
	if (dst_vma->vm_flags & VM_MTE)
		mte_clear_slice_tags(folio_address(existing_folio) + offset);
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
		*dst_pte_val = pte_mkuffd(*dst_pte_val);

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

/*
 * p3s_wp_can_reuse_anon_folio - Check that a write fault can reuse @folio
 * @folio: Small anonymous folio that @pte maps
 * @vma: Pointer to struct vm_area_struct
 * @addr: Faulting virtual address
 * @pte: Faulting PTE, with the page table lock held
 *
 * The 4KB variant of wp_can_reuse_anon_folio(): up to four slice PTEs of
 * the window at @addr can map @folio, so it can be reused only if they
 * hold all of its mappings and references. As there, the folio lock is
 * only trylocked, the swap cache is freed if possible, and the anon_vma of
 * a reusable folio is moved to @vma.
 */
static inline bool p3s_wp_can_reuse_anon_folio(struct folio *folio,
					       struct vm_area_struct *vma,
					       unsigned long addr,
					       pte_t *pte)
{
	unsigned int nr_slices, local_slice_count;
	unsigned long start_addr;
	pte_t *pte_base;

	if (folio_test_ksm(folio))
		return false;

	vma_folio_slice_bounds(vma, addr, &nr_slices, &start_addr);

	/*
	 * Early check: local slices cannot exceed total folio refcount.
	 * Max expected transient refcount is nr_slices + 1 (swapcache) + 1 (LRU).
	 */
	if (folio_ref_count(folio) > nr_slices + 2)
		return false;

	if (!folio_test_lru(folio))
		lru_add_drain();

	if (folio_ref_count(folio) > nr_slices + folio_test_swapcache(folio))
		return false;

	if (!folio_trylock(folio))
		return false;

	if (folio_test_swapcache(folio))
		folio_free_swap(folio);

	if (folio_test_ksm(folio)) {
		folio_unlock(folio);
		return false;
	}

	pte_base = pte - ((addr - start_addr) >> PAGE_SHIFT_4KB);
	local_slice_count = p3s_count_folio_slices(vma, pte_base, start_addr,
						   nr_slices, folio);

	if (local_slice_count != folio_mapcount(folio) ||
	    local_slice_count != folio_ref_count(folio)) {
		folio_unlock(folio);
		return false;
	}

	folio_move_anon_rmap(folio, vma);
	folio_unlock(folio);
	return true;
}

/*
 * p3s_wp_slice_bounds - Get the slice window that a COW redirects
 * @old_folio: Folio being copied, or NULL for the zero page
 * @vmf: Fault information
 * @nr_slices: Output number of slices in the window
 * @start_addr: Output address of the first slice
 *
 * Returns true if the COW of @old_folio redirects all the slice PTEs that
 * map it: a small anonymous folio in a 4KB VMA without userfaultfd, on a
 * write fault (not an unshare) in a window of more than one slice.
 */
static inline bool p3s_wp_slice_bounds(struct folio *old_folio,
				       struct vm_fault *vmf,
				       unsigned int *nr_slices,
				       unsigned long *start_addr)
{
	struct vm_area_struct *vma = vmf->vma;

	if (!vma_is_p3s_4k(vma) || !old_folio ||
	    !folio_test_anon(old_folio) || folio_test_large(old_folio) ||
	    unlikely(userfaultfd_armed(vma)) || (vmf->flags & FAULT_FLAG_UNSHARE))
		return false;

	vma_folio_slice_bounds(vma, vmf->address, nr_slices, start_addr);
	return *nr_slices > 1;
}

/*
 * p3s_wp_mk_sibling_pte - Make the PTE of a sibling slice that a COW redirects
 * @vma: Pointer to struct vm_area_struct
 * @entry: New PTE, mapping the sibling's slice of the copy
 * @old: The sibling's previous PTE, as ptep_clear_flush() returned it
 *
 * Only the faulting PTE is being written, so a sibling keeps the access,
 * dirty and soft-dirty bits of the PTE that it replaces, as on swap-in. On
 * arm64 vm_page_prot sets the access flag and pte_sw_mkyoung() does nothing,
 * so an old sibling needs pte_mkold(). The copy is exclusive, so the sibling
 * is writable unless soft-dirty tracking must see its first write. uffd-wp
 * needs no check, as p3s_wp_slice_bounds() skips userfaultfd-armed VMAs.
 */
static inline pte_t p3s_wp_mk_sibling_pte(struct vm_area_struct *vma,
					  pte_t entry, pte_t old)
{
	if (!pte_young(old))
		entry = pte_mkold(entry);
	if (pte_dirty(old))
		entry = pte_mkdirty(entry);
	if (pte_soft_dirty(old))
		entry = pte_mksoft_dirty(entry);
	if (!pte_needs_soft_dirty_wp(vma, entry))
		entry = maybe_mkwrite(entry, vma);
	return entry;
}

/*
 * p3s_wp_widen_range - Widen the MMU notifier range of a COW to its slices
 * @old_folio: Source shared folio, or NULL for the zero page
 * @vmf: Fault information
 * @start: In/out start of the range, the faulting 4KB page
 * @end: In/out end of the range
 *
 * p3s_wp_install_folio_slices() redirects every slice PTE of @old_folio in
 * the window to the new folio, so secondary MMUs must drop their
 * translations of all of them, not only of the faulting page. Covering
 * slices that turn out not to map @old_folio only adds invalidation.
 */
static inline void p3s_wp_widen_range(struct folio *old_folio,
				      struct vm_fault *vmf,
				      unsigned long *start, unsigned long *end)
{
	unsigned int nr_slices;
	unsigned long first;

	if (!p3s_wp_slice_bounds(old_folio, vmf, &nr_slices, &first))
		return;
	*start = first;
	*end = first + ((unsigned long)nr_slices << PAGE_SHIFT_4KB);
}

/**
 * p3s_wp_reuse_zero_folio - Map a sibling's slice on zero-page COW
 * @new_folio: Zeroed folio allocated by wp_page_copy()
 * @vmf: Fault information; page table lock held, *vmf->pte == vmf->orig_pte
 *
 * Zero the faulting slice of a sibling folio that
 * p3s_anon_folio_lookaround_pte() returns, and map it at the faulting PTE
 * only, as wp_page_copy() would map @new_folio. The lookaround reference
 * goes to the PTE, and the rmap mlocks the folio once more in a VM_LOCKED
 * VMA. wp_page_copy() has already counted the PTE in MM_ANONPAGES.
 *
 * Return: true, having freed @new_folio, if a sibling's slice was mapped;
 * otherwise false, leaving @new_folio and the faulting PTE unchanged.
 */
static inline bool p3s_wp_reuse_zero_folio(struct folio *new_folio,
					   struct vm_fault *vmf)
{
	struct vm_area_struct *vma = vmf->vma;
	const bool unshare = vmf->flags & FAULT_FLAG_UNSHARE;
	struct folio *existing_folio;
	pte_t entry;

	if (!is_zero_pfn(pte_pfn(vmf->orig_pte)))
		return false;

	existing_folio = p3s_anon_folio_lookaround_pte(vma, vmf->pte,
						       vmf->address);
	if (!existing_folio)
		return false;

	p3s_anon_folio_zero_slice(existing_folio, vma, vmf->address);
	entry = vma_pte_add_slice(vma, vmf->address,
				  folio_mk_pte(existing_folio, vma->vm_page_prot));
	entry = pte_sw_mkyoung(entry);
	if (unlikely(unshare)) {
		if (pte_soft_dirty(vmf->orig_pte))
			entry = pte_mksoft_dirty(entry);
		if (pte_uffd_wp(vmf->orig_pte))
			entry = pte_mkuffd_wp(entry);
	} else {
		entry = maybe_mkwrite(pte_mkdirty(entry), vma);
	}

	flush_cache_page(vma, vmf->address, pte_pfn(vmf->orig_pte));
	ptep_clear_flush(vma, vmf->address, vmf->pte);
	folio_add_anon_rmap_pte(existing_folio, &existing_folio->page, vma,
				vmf->address, RMAP_NONE);
	set_pte_at(vma->vm_mm, vmf->address, vmf->pte, entry);
	update_mmu_cache_range(vmf, vma, vmf->address, vmf->pte, 1);

	/* The PTE keeps the lookaround reference; free the new folio. */
	folio_put(new_folio);
	return true;
}

/*
 * p3s_wp_install_folio_slices - Map a COW copy or reuse a zero-page sibling
 * @new_folio: Copy of @old_folio or a zeroed folio, holding its allocation
 *             reference
 * @old_folio: Folio that the faulting PTE maps, or NULL for the zero page
 * @vmf: Fault information; page table lock held, *vmf->pte == vmf->orig_pte
 *
 * wp_page_copy() copied the whole 16KB folio, so redirect every PTE of the
 * window at vmf->address (clamped to the VMA and the PTE table) that maps
 * @old_folio to its slice of @new_folio, not only the faulting one. PTEs
 * that map anything else are left alone:
 *
 *            slice 0    slice 1    slice 2    slice 3
 *          ┌──────────┬──────────┬──────────┬──────────┐
 *  before  │  old[0]  │  old[1]  │  other   │  old[3]  │  fault on slice 1
 *          ├──────────┼──────────┼──────────┼──────────┤
 *  after   │  new[0]  │  new[1]  │  other   │  new[3]  │  new: @new_folio
 *          └──────────┴──────────┴──────────┴──────────┘
 *
 * Only the faulting PTE is made young and dirty, as wp_page_copy() makes
 * it. The siblings keep the state of the PTEs that they replace
 * (p3s_wp_mk_sibling_pte()).
 *
 * The allocation reference of @new_folio goes to the first PTE, and one
 * more is taken per further PTE. @old_folio loses one reference per
 * redirected PTE except the faulting one, whose reference wp_page_copy()
 * drops. The caller widened its MMU notifier range to the window with
 * p3s_wp_widen_range().
 *
 * For a zero-page PTE, see p3s_wp_reuse_zero_folio().
 *
 * Returns false, having changed nothing, if wp_page_copy() must install
 * only the faulting PTE.
 */
static inline bool p3s_wp_install_folio_slices(struct folio *new_folio,
					       struct folio *old_folio,
					       struct vm_fault *vmf)
{
	struct vm_area_struct *vma = vmf->vma;
	struct mm_struct *mm = vma->vm_mm;
	unsigned int nr_slices, i, redirected = 0;
	unsigned long start_addr;
	int mapped_slices;
	pte_t *pte_base;

	if (!old_folio)
		return p3s_wp_reuse_zero_folio(new_folio, vmf);
	if (!p3s_wp_slice_bounds(old_folio, vmf, &nr_slices, &start_addr))
		return false;

	pte_base = vmf->pte - ((vmf->address - start_addr) >> PAGE_SHIFT_4KB);
	mapped_slices = p3s_count_folio_slices(vma, pte_base, start_addr,
					       nr_slices, old_folio);

	folio_add_new_anon_rmap(new_folio, vma, start_addr, RMAP_EXCLUSIVE);
	if (mapped_slices > 1)
		folio_ref_add(new_folio, mapped_slices - 1);
	folio_add_lru_vma(new_folio, vma);

	for (i = 0; i < nr_slices; i++) {
		unsigned long slice_addr = start_addr + (i << PAGE_SHIFT_4KB);
		pte_t *ptep = pte_base + i;
		pte_t ptent = ptep_get(ptep);
		pte_t entry;

		if (!p3s_pte_maps_folio(vma, slice_addr, ptent, old_folio))
			continue;

		/*
		 * folio_add_new_anon_rmap() accounted the first mapping; the
		 * rmap also mlocks the folio once per slice in a VM_LOCKED VMA.
		 */
		if (redirected++)
			folio_add_anon_rmap_pte(new_folio, &new_folio->page, vma,
						slice_addr, RMAP_EXCLUSIVE);

		flush_cache_page(vma, slice_addr, pte_pfn(ptent));
		ptent = ptep_clear_flush(vma, slice_addr, ptep);

		entry = vma_pte_add_slice(vma, slice_addr,
					  folio_mk_pte(new_folio, vma->vm_page_prot));
		if (ptep == vmf->pte) {
			entry = pte_sw_mkyoung(entry);
			entry = maybe_mkwrite(pte_mkdirty(entry), vma);
		} else {
			entry = p3s_wp_mk_sibling_pte(vma, entry, ptent);
		}

		set_pte_at(mm, slice_addr, ptep, entry);
		update_mmu_cache_range(vmf, vma, slice_addr, ptep, 1);

		folio_remove_rmap_pte(old_folio, &old_folio->page, vma);
	}

	/* The sibling PTEs held these references: drop them after unmapping */
	if (mapped_slices > 1)
		folio_put_refs(old_folio, mapped_slices - 1);

	return true;
}

#else /* !CONFIG_ARM64_PER_PROCESS_PAGE_SIZE */

static inline bool p3s_anon_reuse_sibling_slice(struct vm_fault *vmf,
						vm_fault_t *ret)
{
	return false;
}

static inline bool p3s_anon_reuse_racing_sibling(struct vm_fault *vmf,
						 struct folio **foliop,
						 pte_t *entry)
{
	return false;
}

static inline unsigned long p3s_swapin_free_sibling_swap(struct vm_fault *vmf,
							 struct folio *folio,
							 swp_entry_t entry)
{
	return 0;
}

static inline void p3s_swapin_map_sibling_slices(struct vm_fault *vmf,
						 struct folio *folio,
						 unsigned long mask,
						 rmap_t rmap_flags)
{
}

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

static inline bool p3s_wp_can_reuse_anon_folio(struct folio *folio,
					       struct vm_area_struct *vma,
					       unsigned long addr,
					       pte_t *pte)
{
	return false;
}

static inline bool p3s_wp_install_folio_slices(struct folio *new_folio,
					       struct folio *old_folio,
					       struct vm_fault *vmf)
{
	return false;
}

static inline void p3s_wp_widen_range(struct folio *old_folio,
				      struct vm_fault *vmf,
				      unsigned long *start, unsigned long *end)
{
}

#endif /* CONFIG_ARM64_PER_PROCESS_PAGE_SIZE */

#endif /* __MM_P3S_ANON_H */
