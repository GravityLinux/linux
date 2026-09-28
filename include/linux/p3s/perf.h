/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_P3S_PERF_H
#define _LINUX_P3S_PERF_H

#include <linux/page_size_compat_defs.h>

/*
 * Include after kernel/events/internal.h: helpers below use
 * struct perf_buffer, perf_data_size() and perf_mmap_to_page().
 */

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

/* Page size of the ring buffer layout that @mm maps. */
static inline unsigned long p3s_perf_mm_page_size(struct mm_struct *mm)
{
	return mm_is_p3s_4k(mm) ? mm_pte_size(mm) : __PAGE_SIZE;
}

/* Count the 4KB user page as the native page which backs it. */
static inline unsigned long p3s_perf_mmap_size(struct vm_area_struct *vma)
{
	unsigned long size = vma->vm_end - vma->vm_start;

	if (!vma_is_p3s_4k(vma) || vma->vm_pgoff)
		return size;
	return size - mm_pte_size(vma->vm_mm) + PAGE_SIZE_KERNEL;
}

#ifdef CONFIG_ARM64_PER_PROCESS_PAGE_SIZE
static inline unsigned long p3s_perf_mmap_page_size(struct perf_buffer *rb)
{
	return rb->mmap_page_size;
}

static inline void p3s_perf_set_mmap_page_size(struct perf_buffer *rb,
					       unsigned long page_size)
{
	rb->mmap_page_size = page_size;
}

static inline vm_fault_t p3s_perf_mmap_pfn_mkwrite(struct vm_fault *vmf)
{
	/* Only the first 4KB of the main ring is writable metadata. */
	if (vmf->vma->vm_pgoff ||
	    vmf->address >= vmf->vma->vm_start + mm_pte_size(vmf->vma->vm_mm))
		return VM_FAULT_SIGBUS;
	return 0;
}

/**
 * p3s_perf_map_main - Map the main ring of @rb into a 4KB VMA
 * @rb: Ring buffer to map
 * @vma: 4KB VMA of the main ring, at vm_pgoff 0
 *
 * Starting at vm_start, the virtual 4KB pages contain:
 *
 *   U.0 | D0.0 | D0.1 | D0.2 | D0.3 | D1.0 | ...
 *
 * U.0 is slice 0 of the user page; DN.S is slice S of data page N.
 * Data starts at virtual slice 1 but at slice 0 of its kernel page, so
 * these PTEs do not follow vma_slice_offset(). The VMA is VM_IO | VM_PFNMAP
 * and perf_mmap_vmops has no ->access: GUP, __access_remote_vm() and
 * coredumps cannot read through these PTEs.
 *
 * Return: 0 on success, or a negative error after removing partial mappings.
 */
static inline int p3s_perf_map_main(struct perf_buffer *rb,
				    struct vm_area_struct *vma)
{
	unsigned long page_size = mm_pte_size(vma->vm_mm);
	unsigned long data_size = perf_data_size(rb);
	unsigned long data_offset = 0;
	unsigned long uaddr = vma->vm_start;
	pgprot_t prot = vm_get_page_prot(vma->vm_flags & ~VM_SHARED);
	struct page *page;
	int ret;

	page = perf_mmap_to_page(rb, 0);
	if (!page) {
		ret = -EINVAL;
		goto out;
	}
	ret = remap_pfn_range(vma, uaddr, page_to_pfn(page), page_size, prot);
	if (ret)
		goto out;
	uaddr += page_size;

	while (data_offset < data_size) {
		unsigned int slice = (data_offset % PAGE_SIZE_KERNEL) / page_size;

		/* The data pages follow the one metadata page. */
		page = perf_mmap_to_page(rb, 1 + data_offset / PAGE_SIZE_KERNEL);
		if (!page) {
			ret = -EINVAL;
			goto out;
		}
		ret = p3s_remap_pfn_slice(vma, uaddr, page_to_pfn(page), slice, prot);
		if (ret)
			goto out;
		uaddr += page_size;
		data_offset += page_size;
	}

	ret = uaddr == vma->vm_end ? 0 : -EINVAL;
out:
	if (ret)
		zap_page_range_single(vma, vma->vm_start,
				      vma->vm_end - vma->vm_start, NULL);
	return ret;
}
#else
static inline unsigned long p3s_perf_mmap_page_size(struct perf_buffer *rb)
{
	return __PAGE_SIZE;
}

static inline void p3s_perf_set_mmap_page_size(struct perf_buffer *rb,
					       unsigned long page_size)
{
}

static inline vm_fault_t p3s_perf_mmap_pfn_mkwrite(struct vm_fault *vmf)
{
	return VM_FAULT_SIGBUS;
}

static inline int p3s_perf_map_main(struct perf_buffer *rb,
				    struct vm_area_struct *vma)
{
	return -EINVAL;
}
#endif

#endif /* _LINUX_P3S_PERF_H */
