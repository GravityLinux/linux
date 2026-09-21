/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_P3S_IOV_ITER_H
#define _LINUX_P3S_IOV_ITER_H

#include <linux/kernel.h>
#include <linux/uio.h>
#include <linux/mm.h>
#include <linux/slab.h>
#include <linux/sched.h>
#include <linux/uaccess.h>
#include <linux/p3s/mm.h>

/* want_pages_array() with one entry per page of the current process. */
static inline int p3s_want_user_pages_array(struct page ***res, size_t size,
					    size_t start, unsigned int maxpages)
{
	unsigned int count = DIV_ROUND_UP(size + start, mm_pte_size(current->mm));

	if (count > maxpages)
		count = maxpages;
	WARN_ON(!count);
	if (!*res) {
		*res = kvmalloc_array(count, sizeof(struct page *), GFP_KERNEL);
		if (!*res)
			return 0;
	}
	return count;
}

/* iov_npages() for user iovecs, counting pages of the current process. */
static inline int p3s_user_iov_npages(const struct iov_iter *i, int maxpages)
{
	size_t skip = i->iov_offset, size = i->count;
	const struct iovec *p;
	int npages = 0;

	for (p = iter_iov(i); size; skip = 0, p++) {
		unsigned long addr = (unsigned long)p->iov_base + skip;
		unsigned long offs = mm_offset_in_page(current->mm, addr);
		size_t len = min(p->iov_len - skip, size);

		if (len) {
			size -= len;
			npages += DIV_ROUND_UP(offs + len, mm_pte_size(current->mm));
			if (unlikely(npages > maxpages))
				return maxpages;
		}
	}
	return npages;
}

static inline void p3s_release_user_page(struct page *page, bool pin)
{
	if (pin)
		unpin_user_page(page);
	else
		put_page(page);
}

static inline void p3s_release_user_pages(struct page **pages, long nr,
					  bool pin)
{
	long i;

	for (i = 0; i < nr; i++)
		p3s_release_user_page(pages[i], pin);
}

static inline long p3s_gup_remote(struct mm_struct *mm, unsigned long start,
				  unsigned long nr, unsigned int gup_flags,
				  bool pin, struct page **pages, int *locked)
{
	if (pin)
		return pin_user_pages_remote(mm, start, nr, gup_flags, pages,
					     locked);
	return get_user_pages_remote(mm, start, nr, gup_flags, pages, locked);
}

/*
 * struct p3s_slice_run - Host page vector built from 4KB user pages
 * @pages: The host pages, one reference or pin each
 * @max_pages: Number of entries in @pages
 * @nr_pages: Number of entries used in @pages
 * @slice: Slice of the last user page added
 * @offset: Byte offset of the first byte in pages[0]; the offset of the
 *          start address in its 4KB page until the first page is added
 * @bytes: Number of bytes from @offset to the end of the last user page
 */
struct p3s_slice_run {
	struct page **pages;
	unsigned int max_pages;
	unsigned int nr_pages;
	unsigned int slice;
	size_t offset;
	size_t bytes;
};

/*
 * p3s_slice_run_add - Append the next user page to a slice run
 * @run: The run so far
 * @page: Host page that GUP returned for the user page
 * @slice: Slice of @page that the user page maps
 * @pin: @page is pinned rather than referenced
 *
 * The run continues with the next slice of its last host page, or with
 * slice 0 of a new host page after slice 3 if @run->pages has room for it.
 * Returns false, and leaves @page to the caller, if the user page does not
 * continue the run.
 */
static inline bool p3s_slice_run_add(struct p3s_slice_run *run,
				     struct page *page, unsigned int slice,
				     bool pin)
{
	if (!run->nr_pages) {
		/* First user page: the run starts at its slice */
		run->pages[run->nr_pages++] = page;
		run->bytes = PAGE_SIZE_4KB - run->offset;
		run->offset += (size_t)slice << PAGE_SHIFT_4KB;
	} else if (page == run->pages[run->nr_pages - 1] &&
		   slice == run->slice + 1) {
		/* Next slice of the same host page, which the run holds */
		p3s_release_user_page(page, pin);
		run->bytes += PAGE_SIZE_4KB;
	} else if (run->slice == P3S_SLICE_MASK && !slice &&
		   run->nr_pages < run->max_pages) {
		/* Slice 0 of the next host page */
		run->pages[run->nr_pages++] = page;
		run->bytes += PAGE_SIZE_4KB;
	} else {
		return false;
	}
	run->slice = slice;
	return true;
}

/*
 * p3s_slice_run_merge - Append a batch of user pages to a slice run
 * @run: The run so far
 * @mm: mm_struct whose mmap_lock is held
 * @va: Address of the first user page of the batch
 * @slots: The host pages that GUP returned, in the unused tail of
 *         run->pages or in a scratch array
 * @nr: Number of entries in @slots
 * @pin: The pages are pinned rather than referenced
 *
 * In the tail of run->pages, merging only moves entries towards its start,
 * so it never overwrites an entry of @slots that it has not read yet.
 *
 * Return: the number of user pages added, from the start of @slots. The
 * caller drops the rest.
 */
static inline long p3s_slice_run_merge(struct p3s_slice_run *run,
				       struct mm_struct *mm, unsigned long va,
				       struct page **slots, long nr, bool pin)
{
	struct vm_area_struct *vma = NULL;
	long i;

	for (i = 0; i < nr; i++, va += PAGE_SIZE_4KB) {
		if (!vma || va >= vma->vm_end)
			vma = vma_lookup(mm, va);
		if (!vma)
			break;
		if (!p3s_slice_run_add(run, slots[i],
				       vma_slice_offset(vma, va), pin))
			break;
	}
	return i;
}

/*
 * p3s_get_user_slice_pages - Get or pin a user range of a 4KB compat mm
 * @mm: mm_struct of the 4KB compatibility process owning the range
 * @addr: Start of the user range, which may be tagged
 * @maxsize: Maximum number of bytes to cover
 * @maxpages: Number of entries available in @pages
 * @gup_flags: FOLL_* flags for get_user_pages() or pin_user_pages()
 * @pin: Pin the pages instead of taking references
 * @pages: Returns the host pages
 * @offset0: Returns the byte offset of @addr within pages[0]
 *
 * GUP returns one 16KB host page per 4KB user page, and which 4KB slice of
 * that host page a user page maps depends on its VMA. Users of page vectors
 * instead expect the first byte at *@offset0 in pages[0], and every
 * following page to continue the range from its start. So merge the user
 * pages into a run of consecutive slices (see p3s_slice_run_add()):
 *
 *   user pages      va0   va1   va2   va3   va4
 *   (page:slice)    P:1   P:2   P:3   Q:0   R:2
 *   returned        pages = { P, Q }, *offset0 = 4KB, va0..va3 covered
 *
 * The range stops at the first user page that does not continue the run
 * (va4 above), where the caller extracts again.
 *
 * The user pages are grabbed in doubling batches, starting with one host
 * page's worth, so that a short run does not cost GUP on the whole range.
 * GUP returns them in the unused entries of @pages, or in a small scratch
 * array once fewer than a host page's worth of entries are left, so that
 * the last host pages can still take all their slices. mmap_lock is held
 * across GUP and the slice lookups so that both see the same mappings.
 *
 * Return: the number of bytes covered, with DIV_ROUND_UP(*@offset0 + bytes,
 * PAGE_SIZE_KERNEL) pages held in @pages, or a negative error.
 */
static inline ssize_t p3s_get_user_slice_pages(struct mm_struct *mm,
		unsigned long addr, size_t maxsize, unsigned int maxpages,
		unsigned int gup_flags, bool pin, struct page **pages,
		size_t *offset0)
{
	struct p3s_slice_run run = {
		.pages = pages,
		.max_pages = maxpages,
	};
	struct page *scratch[P3S_SLICES_PER_PAGE];
	unsigned long batch = P3S_SLICES_PER_PAGE, done = 0;
	unsigned long start, nr;
	int locked = 0, retries = 0;
	long ret = 0, used;

	/* GUP untags the address, and the slice lookups need it untagged too. */
	addr = untagged_addr(addr);
	run.offset = addr & ~PAGE_MASK_4KB;
	start = addr & PAGE_MASK_4KB;
	nr = DIV_ROUND_UP(run.offset + maxsize, PAGE_SIZE_4KB);

	while (done < nr) {
		unsigned long va = start + done * PAGE_SIZE_4KB;
		unsigned long avail = maxpages - run.nr_pages;
		struct page **slots = pages + run.nr_pages;
		unsigned long chunk;

		if (!avail) {
			/* @pages is full: only the last host page can grow. */
			if (!run.nr_pages || run.slice == P3S_SLICE_MASK)
				break;
			avail = P3S_SLICE_MASK - run.slice;
			slots = scratch;
		} else if (avail < P3S_SLICES_PER_PAGE) {
			/* Merging may fit more user pages than free entries. */
			avail = P3S_SLICES_PER_PAGE;
			slots = scratch;
		}
		chunk = min3(batch, nr - done, avail);

		/* Not locked only before the first batch, with nothing held. */
		if (!locked) {
			if (mmap_read_lock_killable(mm))
				return -EINTR;
			locked = 1;
		}

		/* Let faults drop mmap_lock, but retry only a few times. */
		ret = p3s_gup_remote(mm, va, chunk, gup_flags, pin, slots,
				     retries < 3 ? &locked : NULL);
		if (!locked) {
			/*
			 * A fault dropped mmap_lock, so the slice lookups would
			 * not see the mappings GUP saw. Retry the first batch
			 * under a new lock; after that, return what is merged.
			 */
			p3s_release_user_pages(slots, ret, pin);
			if (done || ret <= 0)
				break;
			retries++;
			continue;
		}
		if (ret <= 0)
			break;

		used = p3s_slice_run_merge(&run, mm, va, slots, ret, pin);
		p3s_release_user_pages(slots + used, ret - used, pin);
		if (used < chunk)
			break;
		done += chunk;
		batch *= 2;
	}
	if (locked)
		mmap_read_unlock(mm);

	if (!run.bytes)
		return ret < 0 ? ret : -EFAULT;
	*offset0 = run.offset;
	return min(run.bytes, maxsize);
}

/*
 * p3s_iov_iter_extract_pages - Extract the user pages of an iov_iter
 * @i: The user-backed iterator of a 4KB compatibility process
 * @pages: Where to return the pages; allocated if *@pages is NULL
 * @addr: Address of the first byte, from first_iovec_segment()
 * @maxsize: Maximum number of bytes to extract
 * @maxpages: Maximum number of pages to return
 * @gup_flags: FOLL_* flags
 * @pin: Pin the pages instead of taking references
 * @offset0: Returns the offset of the first byte within (*@pages)[0]
 *
 * Gets or pins the pages with p3s_get_user_slice_pages() and advances @i by
 * the number of bytes extracted, which may be less than @maxsize.
 */
static inline ssize_t p3s_iov_iter_extract_pages(struct iov_iter *i,
		struct page ***pages, unsigned long addr, size_t maxsize,
		unsigned int maxpages, unsigned int gup_flags, bool pin,
		size_t *offset0)
{
	ssize_t ret;
	int n;

	n = p3s_want_user_pages_array(pages, maxsize, addr & ~PAGE_MASK_4KB,
				      maxpages);
	if (!n)
		return -ENOMEM;
	ret = p3s_get_user_slice_pages(current->mm, addr, maxsize, n,
				       gup_flags, pin, *pages, offset0);
	if (ret > 0)
		iov_iter_advance(i, ret);
	return ret;
}

#endif /* _LINUX_P3S_IOV_ITER_H */
