/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_P3S_FUTEX_H
#define _LINUX_P3S_FUTEX_H

#include <linux/mm.h>
#include <linux/mmap_lock.h>
#include <linux/rcupdate.h>
#include <linux/uaccess.h>

/*
 * p3s_futex_page_offset - Offset of a shared futex within its page of the file
 * @mm: mm_struct of the task using the futex
 * @uaddr: User address of the futex
 * @offset: Offset of @uaddr within its kernel page
 *
 * Inode based futex keys combine the index of the page that backs a futex
 * with the offset of the futex in that page. A 4KB compatibility VMA can
 * map a file at any 4KB offset, so the address does not determine that
 * offset: take the slice from the VMA, falling back to @offset if the VMA
 * was concurrently unmapped. Returns @offset for other mms.
 */
static inline unsigned long p3s_futex_page_offset(struct mm_struct *mm,
						  unsigned long uaddr,
						  unsigned long offset)
{
	struct vm_area_struct *vma;
	unsigned int seq;

	if (!mm_is_p3s_4k(mm))
		return offset;

	uaddr = untagged_addr(uaddr);
	rcu_read_lock();
	if (mmap_lock_speculate_try_begin(mm, &seq)) {
		unsigned long spec_offset = offset;

		vma = vma_lookup(mm, uaddr);
		if (vma)
			spec_offset = vma_folio_offset(vma, uaddr);
		if (!mmap_lock_speculate_retry(mm, seq)) {
			rcu_read_unlock();
			return spec_offset;
		}
	}
	rcu_read_unlock();

	mmap_read_lock(mm);
	vma = vma_lookup(mm, uaddr);
	if (vma)
		offset = vma_folio_offset(vma, uaddr);
	mmap_read_unlock(mm);
	return offset;
}

#endif /* _LINUX_P3S_FUTEX_H */
