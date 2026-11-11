/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_P3S_SHMEM_H
#define _LINUX_P3S_SHMEM_H

#include <linux/mm.h>
#include <linux/pagemap.h>
#include <linux/rcupdate_wait.h>
#include <linux/xarray.h>

#ifdef CONFIG_ARM64_PER_PROCESS_PAGE_SIZE
/**
 * vma_shmem_swap_usage - Swapped bytes of the file range a 4KB VMA maps
 * @vma: 4KB VMA of a shmem file
 * @start: Start of the address range in @vma
 * @end: End of the address range in @vma
 *
 * A 4KB VMA can start or end partway through a 16KB file page, so count
 * only the bytes of each swapped entry that [@start, @end) maps, rather
 * than whole entries.
 *
 * Return: Number of swapped bytes covered by the address range.
 */
static inline unsigned long vma_shmem_swap_usage(struct vm_area_struct *vma,
						 unsigned long start,
						 unsigned long end)
{
	u64 start_byte = vma_file_offset_at(vma, start);
	u64 end_byte = vma_file_offset_at(vma, end);
	pgoff_t first = start_byte >> PAGE_SHIFT_KERNEL;
	pgoff_t last;
	XA_STATE(xas, &vma->vm_file->f_mapping->i_pages, first);
	struct folio *folio;
	unsigned long swapped = 0;

	if (end_byte <= start_byte)
		return 0;
	last = (end_byte - 1) >> PAGE_SHIFT_KERNEL;

	rcu_read_lock();
	xas_for_each(&xas, folio, last) {
		if (xas_retry(&xas, folio))
			continue;
		if (xa_is_value(folio)) {
			unsigned long nr_pages = 1UL << xas_get_order(&xas);
			pgoff_t base = round_down(xas.xa_index, nr_pages);
			u64 entry_start = (u64)base << PAGE_SHIFT_KERNEL;
			u64 entry_end = (u64)(base + nr_pages) << PAGE_SHIFT_KERNEL;

			swapped += min(end_byte, entry_end) - max(start_byte, entry_start);
		}
		if (xas.xa_index == last)
			break;
		if (need_resched()) {
			xas_pause(&xas);
			cond_resched_rcu();
		}
	}
	rcu_read_unlock();

	return swapped;
}
#else /* !CONFIG_ARM64_PER_PROCESS_PAGE_SIZE */
static inline unsigned long vma_shmem_swap_usage(struct vm_area_struct *vma,
						 unsigned long start,
						 unsigned long end)
{
	return 0;
}
#endif /* CONFIG_ARM64_PER_PROCESS_PAGE_SIZE */

#endif /* _LINUX_P3S_SHMEM_H */
