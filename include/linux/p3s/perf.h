/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_P3S_PERF_H
#define _LINUX_P3S_PERF_H

#include <linux/page_size_compat_defs.h>

/* A 4KB mm has PTE leaves only; native huge/contiguous leaves are unchanged. */
static inline u64 p3s_perf_pte_size(struct mm_struct *mm, u64 size)
{
	return size == PAGE_SIZE_KERNEL ? mm_pte_size(mm) : size;
}

/* File offsets include the slice; anonymous 4KB VMAs count vm_pgoff in 4KBs. */
static inline u64 p3s_mmap_pgoff(const struct vm_area_struct *vma)
{
	return vma->vm_file ? vma_file_offset(vma) :
		(u64)vma->vm_pgoff << mm_pte_shift(vma->vm_mm);
}

#endif /* _LINUX_P3S_PERF_H */
