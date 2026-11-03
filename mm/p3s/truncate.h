/* SPDX-License-Identifier: GPL-2.0 */
#ifndef __MM_P3S_TRUNCATE_H
#define __MM_P3S_TRUNCATE_H

#include <linux/mm.h>
#include <linux/pagemap.h>
#include <linux/rmap.h>

/*
 * Native-page truncation retains the partial last page. Unmap only the full
 * 4KB slices beyond EOF in that page from compatibility VMAs, including COWs.
 * The caller repeats this around page-cache truncation, like the full-page
 * unmap, to remove private pages COWed while the page cache is being pruned.
 */
#ifdef CONFIG_ARM64_PER_PROCESS_PAGE_SIZE
static inline void p3s_unmap_truncated_tail(struct address_space *mapping,
					    loff_t newsize)
{
	u64 first = ALIGN((u64)newsize, PAGE_SIZE_4KB);
	u64 last = ALIGN((u64)newsize, PAGE_SIZE_KERNEL);
	pgoff_t index = (u64)newsize >> PAGE_SHIFT_KERNEL;
	struct vm_area_struct *vma;

	if (first == last)
		return;

	i_mmap_lock_read(mapping);
	vma_interval_tree_foreach(vma, &mapping->i_mmap, index, index) {
		unsigned long start, end;
		u64 offset;

		if (!vma_is_p3s_4k(vma))
			continue;
		offset = vma_file_offset(vma);
		if (offset >= last)
			continue;
		start = vma->vm_start + min_t(u64, max(first, offset) - offset,
					    vma->vm_end - vma->vm_start);
		end = vma->vm_start + min_t(u64, last - offset,
					  vma->vm_end - vma->vm_start);
		if (start < end)
			zap_page_range_single(vma, start, end - start, NULL);
	}
	i_mmap_unlock_read(mapping);
}
#else
static inline void p3s_unmap_truncated_tail(struct address_space *mapping,
					    loff_t newsize)
{
}
#endif

#endif /* __MM_P3S_TRUNCATE_H */
