// SPDX-License-Identifier: GPL-2.0
/*
 * Repack 4KB compatibility anonymous PTEs after an mremap() that changes
 * their slice within the 16KB window.
 *
 * A small folio of a 4KB VMA backs the slices of one 16KB window, each PTE at
 * the 4KB of the folio that matches its virtual slice (vma_slice_offset()).
 * move_page_tables() keeps the physical address of each PTE, so a move to
 * another slice would leave PTEs at the wrong 4KB for every later fault, COW,
 * swap and migration. Instead, before the move, fault in the source range and
 * lock its folios; after it, copy each destination window's slices into a new
 * folio at their new positions and point the PTEs there.
 */

#include <linux/highmem.h>
#include <linux/memcontrol.h>
#include <linux/mm.h>
#include <linux/mm_inline.h>
#include <linux/leafops.h>
#include <linux/sort.h>
#include <linux/swap.h>
#include <linux/userfaultfd_k.h>

#include <asm/cacheflush.h>
#include <asm/tlbflush.h>
#include "internal.h"

struct p3s_mremap_window {
	unsigned long base;
	struct folio *folio;
	bool populated;
};

struct p3s_mremap_ctx {
	struct folio **sources;
	struct p3s_mremap_window *windows;
	unsigned int nr_sources;
	unsigned int nr_locked;
	unsigned int nr_windows;
};

bool p3s_mremap_can_reslice(struct vm_area_struct *vma)
{
	return vma_is_p3s_4k(vma) && vma_is_anonymous(vma) && !vma->vm_file &&
	       !(vma->vm_flags & VM_MTE) && !userfaultfd_armed(vma);
}

static int p3s_folio_ptr_cmp(const void *a, const void *b)
{
	unsigned long x = (unsigned long)*(struct folio * const *)a;
	unsigned long y = (unsigned long)*(struct folio * const *)b;

	return (x > y) - (x < y);
}

void p3s_mremap_finish(struct p3s_mremap_ctx *ctx)
{
	unsigned int i;

	if (!ctx)
		return;
	for (i = 0; i < ctx->nr_locked; i++)
		folio_unlock(ctx->sources[i]);
	for (i = 0; i < ctx->nr_sources; i++)
		folio_put(ctx->sources[i]);
	for (i = 0; i < ctx->nr_windows; i++)
		if (ctx->windows[i].folio)
			folio_put(ctx->windows[i].folio);
	kvfree(ctx->windows);
	kvfree(ctx->sources);
	kfree(ctx);
}

/*
 * The anonymous folio mapped at @addr, with a reference, or NULL. -EAGAIN if
 * the PTE is a swap or migration entry that must be faulted in first, -EBUSY
 * for an entry this cannot move (device private or exclusive).
 */
static int p3s_mremap_source_folio(struct vm_area_struct *vma,
		unsigned long addr, struct folio **foliop)
{
	struct folio *folio = NULL;
	spinlock_t *ptl;
	pte_t *ptep;
	pmd_t *pmd;
	pte_t pte;
	int ret = 0;

	*foliop = NULL;
	pmd = mm_find_pmd(vma->vm_mm, addr);
	if (!pmd)
		return 0;
	ptep = pte_offset_map_lock(vma->vm_mm, pmd, addr, &ptl);
	if (!ptep)
		return 0;
	pte = ptep_get(ptep);
	if (pte_present(pte)) {
		struct page *page = vm_normal_page(vma, addr, pte);

		folio = page ? page_folio(page) : NULL;
		if (folio && folio_test_anon(folio))
			folio_get(folio);
		else
			folio = NULL;
	} else if (!pte_none(pte)) {
		softleaf_t entry = softleaf_from_pte(pte);

		if (softleaf_is_swap(entry) || softleaf_is_migration(entry))
			ret = -EAGAIN;
		else if (softleaf_is_device_private(entry) ||
			 softleaf_is_device_exclusive(entry))
			ret = -EBUSY;
	}
	pte_unmap_unlock(ptep, ptl);
	*foliop = folio;
	return ret;
}

static int p3s_mremap_fault_source(struct vm_area_struct *vma,
		unsigned long addr)
{
	vm_fault_t fault = handle_mm_fault(vma, addr, FAULT_FLAG_REMOTE, NULL);

	if (fault & VM_FAULT_ERROR)
		return vm_fault_to_errno(fault, 0);
	if (WARN_ON_ONCE(fault & (VM_FAULT_RETRY | VM_FAULT_COMPLETED)))
		return -EAGAIN;
	return 0;
}

static struct folio *p3s_mremap_alloc_folio(struct vm_area_struct *vma,
		unsigned long addr)
{
	struct folio *folio;

	folio = vma_alloc_zeroed_movable_folio(vma, addr);
	if (!folio)
		return NULL;
	if (mem_cgroup_charge(folio, vma->vm_mm, GFP_KERNEL)) {
		folio_put(folio);
		return NULL;
	}
	folio_throttle_swaprate(folio, GFP_KERNEL);
	return folio;
}

static bool p3s_mremap_has_source(const struct p3s_mremap_ctx *ctx,
		struct folio *folio)
{
	unsigned int lo = 0, hi = ctx->nr_sources;

	while (lo < hi) {
		unsigned int mid = lo + (hi - lo) / 2;

		if ((unsigned long)ctx->sources[mid] < (unsigned long)folio)
			lo = mid + 1;
		else
			hi = mid;
	}
	return lo < ctx->nr_sources && ctx->sources[lo] == folio;
}

/* After locking: every folio in the range is one of the locked sources. */
static int p3s_mremap_validate_sources(struct p3s_mremap_ctx *ctx,
		struct vm_area_struct *vma, unsigned long addr, unsigned long len)
{
	for (; len; addr += PAGE_SIZE_4KB, len -= PAGE_SIZE_4KB) {
		struct folio *folio;
		bool found;
		int ret;

		ret = p3s_mremap_source_folio(vma, addr, &folio);
		if (ret)
			return ret;
		if (!folio)
			continue;
		found = p3s_mremap_has_source(ctx, folio);
		folio_put(folio);
		if (!found)
			return -EAGAIN;
	}
	return 0;
}

/**
 * p3s_mremap_prepare - Get ready to move anonymous slices to other slices
 * @src_vma: VMA being moved
 * @dst_vma: Its copy at the destination
 * @old_addr: Start of the moved range
 * @new_addr: Its destination
 * @len: Length of the range
 *
 * Called with the mmap lock held for writing, before move_page_tables().
 * Returns NULL if the move keeps every slice in place, a context for
 * p3s_mremap_reslice() and p3s_mremap_finish(), or an ERR_PTR: -EBUSY if a
 * source folio may be pinned for DMA (a copy would detach it from the pin),
 * -ENOMEM if the destination folios cannot be allocated.
 */
struct p3s_mremap_ctx *p3s_mremap_prepare(
		struct vm_area_struct *src_vma, struct vm_area_struct *dst_vma,
		unsigned long old_addr, unsigned long new_addr, unsigned long len)
{
	unsigned long first, last, addr;
	unsigned int max_sources, i, nr = 0;
	struct p3s_mremap_ctx *ctx;
	int ret;

	if (!len || !((old_addr ^ new_addr) & ~PAGE_MASK_KERNEL) ||
	    !p3s_mremap_can_reslice(src_vma))
		return NULL;

	ctx = kzalloc(sizeof(*ctx), GFP_KERNEL);
	if (!ctx)
		return ERR_PTR(-ENOMEM);
	max_sources = len >> PAGE_SHIFT_4KB;
	ctx->sources = kvcalloc(max_sources, sizeof(*ctx->sources), GFP_KERNEL);
	first = new_addr & PAGE_MASK_KERNEL;
	last = ALIGN(new_addr + len, PAGE_SIZE_KERNEL);
	ctx->nr_windows = (last - first) >> PAGE_SHIFT_KERNEL;
	ctx->windows = kvcalloc(ctx->nr_windows, sizeof(*ctx->windows),
				GFP_KERNEL);
	if (!ctx->sources || !ctx->windows) {
		ret = -ENOMEM;
		goto err;
	}
	for (i = 0; i < ctx->nr_windows; i++)
		ctx->windows[i].base = first + ((unsigned long)i << PAGE_SHIFT_KERNEL);

	/* Bring every slice in: no swap or migration entry moves to a new slice */
	for (addr = old_addr; addr < old_addr + len; addr += PAGE_SIZE_4KB) {
		struct folio *folio;

		for (;;) {
			ret = p3s_mremap_source_folio(src_vma, addr, &folio);
			if (ret != -EAGAIN)
				break;
			ret = p3s_mremap_fault_source(src_vma, addr);
			if (ret)
				break;
		}
		if (ret)
			goto err;
		if (!folio)
			continue;
		ctx->sources[nr++] = folio;
		ctx->windows[((new_addr + addr - old_addr) - first) >>
			     PAGE_SHIFT_KERNEL].populated = true;
	}
	ctx->nr_sources = nr;
	if (!nr)
		return ctx;

	sort(ctx->sources, nr, sizeof(*ctx->sources), p3s_folio_ptr_cmp, NULL);
	for (i = 0, nr = 0; i < ctx->nr_sources; i++) {
		if (nr && ctx->sources[nr - 1] == ctx->sources[i])
			folio_put(ctx->sources[i]);
		else
			ctx->sources[nr++] = ctx->sources[i];
	}
	ctx->nr_sources = nr;

	/* Locked, the folios can be neither reclaimed nor migrated meanwhile */
	for (i = 0; i < nr; i++) {
		ret = folio_lock_killable(ctx->sources[i]);
		if (ret)
			goto err;
		ctx->nr_locked++;
		if (folio_maybe_dma_pinned(ctx->sources[i])) {
			ret = -EBUSY;
			goto err;
		}
	}
	ret = p3s_mremap_validate_sources(ctx, src_vma, old_addr, len);
	if (ret)
		goto err;
	for (i = 0; i < ctx->nr_windows; i++) {
		if (!ctx->windows[i].populated)
			continue;
		ctx->windows[i].folio = p3s_mremap_alloc_folio(dst_vma,
					max(ctx->windows[i].base, new_addr));
		if (!ctx->windows[i].folio) {
			ret = -ENOMEM;
			goto err;
		}
	}
	return ctx;
err:
	p3s_mremap_finish(ctx);
	return ERR_PTR(ret);
}

static pte_t p3s_mremap_new_pte(struct vm_area_struct *vma,
		struct folio *folio, unsigned long addr, pte_t old)
{
	pte_t entry = vma_pte_add_slice(vma, addr,
					folio_mk_pte(folio, vma->vm_page_prot));

	if (pte_young(old))
		entry = pte_mkyoung(entry);
	if (pte_dirty(old))
		entry = pte_mkdirty(entry);
	if (pte_soft_dirty(old))
		entry = pte_mksoft_dirty(entry);
	if (pte_uffd(old))
		entry = pte_mkuffd(entry);
	if (pte_write(old))
		entry = maybe_mkwrite(entry, vma);
	return entry;
}

static void p3s_mremap_copy_slice(struct folio *dst, unsigned int dst_slice,
		struct page *src, unsigned int src_slice)
{
	void *from = kmap_local_page(src);
	void *to = kmap_local_page(&dst->page);

	memcpy(to + ((unsigned long)dst_slice << PAGE_SHIFT_4KB),
	       from + ((unsigned long)src_slice << PAGE_SHIFT_4KB),
	       PAGE_SIZE_4KB);
	kunmap_local(to);
	kunmap_local(from);
	flush_dcache_page(&dst->page);
}

/**
 * p3s_mremap_reslice - Move the slices of a moved range to their new slices
 * @ctx: Context from p3s_mremap_prepare()
 * @vma: The destination VMA, after move_page_tables()
 * @new_addr: Start of the moved range
 * @len: Its length
 *
 * For each destination window, write-protect its moved PTEs, copy their
 * slices into the window's new folio at their new positions, and replace the
 * PTEs. GUP-fast is held off through write_protect_seq, as for fork().
 */
void p3s_mremap_reslice(struct p3s_mremap_ctx *ctx,
		struct vm_area_struct *vma, unsigned long new_addr,
		unsigned long len)
{
	struct mm_struct *mm = vma->vm_mm;
	unsigned int w;

	if (!ctx || !ctx->nr_sources)
		return;
	raw_write_seqcount_begin(&mm->write_protect_seq);
	smp_mb();
	for (w = 0; w < ctx->nr_windows; w++) {
		struct p3s_mremap_window *win = &ctx->windows[w];
		unsigned long start, end, addr;
		unsigned int mapped = 0;

		if (!win->folio)
			continue;
		start = max(win->base, new_addr);
		end = min(win->base + PAGE_SIZE_KERNEL, new_addr + len);

		/* Stop userspace writes before copying any slice */
		for (addr = start; addr < end; addr += PAGE_SIZE_4KB) {
			spinlock_t *ptl;
			pmd_t *pmd = mm_find_pmd(mm, addr);
			pte_t *ptep;
			pte_t pte;

			if (!pmd)
				continue;
			ptep = pte_offset_map_lock(mm, pmd, addr, &ptl);
			if (!ptep)
				continue;
			pte = ptep_get(ptep);
			if (pte_present(pte) && vm_normal_page(vma, addr, pte)) {
				ptep_set_wrprotect(mm, addr, ptep);
				flush_tlb_page(vma, addr);
			}
			pte_unmap_unlock(ptep, ptl);
		}

		for (addr = start; addr < end; addr += PAGE_SIZE_4KB) {
			struct folio *old_folio;
			struct page *old_page;
			unsigned int src_slice, dst_slice;
			spinlock_t *ptl;
			pmd_t *pmd = mm_find_pmd(mm, addr);
			pte_t *ptep;
			pte_t old, entry;

			if (!pmd)
				continue;
			ptep = pte_offset_map_lock(mm, pmd, addr, &ptl);
			if (!ptep)
				continue;
			old = ptep_get(ptep);
			old_page = pte_present(old) ? vm_normal_page(vma, addr, old) : NULL;
			old_folio = old_page ? page_folio(old_page) : NULL;
			if (!old_folio || !folio_test_anon(old_folio)) {
				pte_unmap_unlock(ptep, ptl);
				continue;
			}
			src_slice = (__pte_to_phys(old) & ~PAGE_MASK_KERNEL) >>
				    PAGE_SHIFT_4KB;
			dst_slice = vma_slice_offset(vma, addr);
			p3s_mremap_copy_slice(win->folio, dst_slice, old_page,
					      src_slice);
			ptep_clear_flush(vma, addr, ptep);
			if (!mapped) {
				/* Orders the copy before the PTE, as on a fault */
				__folio_mark_uptodate(win->folio);
				folio_add_new_anon_rmap(win->folio, vma, addr,
							RMAP_EXCLUSIVE);
				folio_add_lru_vma(win->folio, vma);
			} else {
				smp_wmb();
				atomic_inc(&win->folio->_mapcount);
				folio_ref_add(win->folio, 1);
			}
			entry = p3s_mremap_new_pte(vma, win->folio, addr, old);
			set_pte_at(mm, addr, ptep, entry);
			update_mmu_cache(vma, addr, ptep);
			folio_remove_rmap_pte(old_folio, old_page, vma);
			folio_put(old_folio);
			mapped++;
			pte_unmap_unlock(ptep, ptl);
		}
		if (mapped)
			win->folio = NULL;	/* the PTEs own it now */
	}
	raw_write_seqcount_end(&mm->write_protect_seq);
}
