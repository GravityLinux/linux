/* SPDX-License-Identifier: GPL-2.0 */
#ifndef __MM_P3S_PROCESS_VM_ACCESS_H
#define __MM_P3S_PROCESS_VM_ACCESS_H

#include <linux/mm.h>
#include <linux/uio.h>
#include <linux/p3s/iov_iter.h>

/*
 * The copy loop of mm/process_vm_access.c, the only file that includes this
 * header, so it can stay static there.
 */
static int process_vm_rw_pages(struct page **pages, unsigned int offset,
			       size_t len, struct iov_iter *iter, int vm_write);

/*
 * p3s_process_vm_rw - process_vm_rw_single_vec() for a 4KB compat target
 * @mm: mm_struct of the 4KB compatibility target
 * @addr: Start of the range in the target
 * @len: Number of bytes to transfer
 * @iter: Local iterator to copy to or from
 * @pages: Page array of process_vm_rw_core()
 * @max_pages: Number of entries in @pages
 * @vm_write: Copy to the target rather than from it
 *
 * pin_user_pages_remote() returns one 16KB host page per 4KB user page of
 * the target, and which slice of the host page a user page maps depends on
 * its VMA. Pin the range in slice runs with p3s_get_user_slice_pages()
 * instead, so that each copy starts at the slice that the target address
 * maps rather than at slice 0 of its host page.
 *
 * Return: 0 on success, or a negative error.
 */
static inline int p3s_process_vm_rw(struct mm_struct *mm, unsigned long addr,
				    unsigned long len, struct iov_iter *iter,
				    struct page **pages, unsigned int max_pages,
				    int vm_write)
{
	unsigned int gup_flags = vm_write ? FOLL_WRITE : 0;
	int rc = 0;

	while (!rc && len && iov_iter_count(iter)) {
		size_t offset;
		ssize_t bytes;

		bytes = p3s_get_user_slice_pages(mm, addr, len, max_pages,
						 gup_flags, true, pages, &offset);
		if (bytes <= 0)
			return -EFAULT;

		rc = process_vm_rw_pages(pages, offset, bytes, iter, vm_write);
		unpin_user_pages_dirty_lock(pages,
					    DIV_ROUND_UP(offset + bytes,
							 PAGE_SIZE_KERNEL),
					    vm_write);
		addr += bytes;
		len -= bytes;
	}
	return rc;
}

#endif /* __MM_P3S_PROCESS_VM_ACCESS_H */
