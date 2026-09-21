/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_P3S_VMA_H
#define _LINUX_P3S_VMA_H

#include <linux/p3s/mm.h>
#include <linux/minmax.h>

struct vm_area_struct;

/*
 * VMA Geometry Abstractions:
 *
 * ┌─────────────────────────────────────────────────────────┐
 * │ 16KB Host Folio (Kernel Page)                           │
 * ├─────────────┬─────────────┬─────────────┬───────────────┤
 * │   Slice 0   │   Slice 1   │   Slice 2   │    Slice 3    │
 * │   (4 KB)    │   (4 KB)    │   (4 KB)    │    (4 KB)     │
 * └─────────────┴─────────────┴─────────────┴───────────────┘
 *               ▲                           ▲
 *               ├─── vma->vm_start          └─── vma->vm_end
 *               │    (Slice 1)                   (Slice 3)
 *               │
 *               ◄─────────── vma_size: 8 KB ────────►
 */

static inline unsigned long vma_size(const struct vm_area_struct *vma)
{
	return vma->vm_end - vma->vm_start;
}

static inline unsigned long vma_page_size(const struct vm_area_struct *vma)
{
	return mm_pte_size(vma->vm_mm);
}

static inline unsigned long vma_page_mask(const struct vm_area_struct *vma)
{
	return mm_pte_mask(vma->vm_mm);
}

static inline unsigned long vma_phys_pfn(const struct vm_area_struct *vma,
					phys_addr_t x)
{
	return mm_phys_pfn(vma->vm_mm, x);
}

static inline bool vma_is_p3s_4k(const struct vm_area_struct *vma)
{
	return mm_is_p3s_4k(vma->vm_mm);
}

static inline unsigned long vma_offset_in_page(const struct vm_area_struct *vma,
					       unsigned long addr)
{
	return mm_offset_in_page(vma->vm_mm, addr);
}

#ifdef CONFIG_ARM64_PER_PROCESS_PAGE_SIZE
static inline unsigned short vma_slice_off(const struct vm_area_struct *vma)
{
	return vma->vm_slice_off;
}

static inline void vma_set_slice_off(struct vm_area_struct *vma,
				     unsigned short slice_off)
{
	vma->vm_slice_off = slice_off;
}
#else
static inline unsigned short vma_slice_off(const struct vm_area_struct *vma)
{
	return 0;
}

static inline void vma_set_slice_off(struct vm_area_struct *vma,
				     unsigned short slice_off)
{
}
#endif

/*
 * vma_slice_offset - Slice of the 16KB page that backs @addr
 * @vma: Pointer to struct vm_area_struct
 * @addr: Virtual address within @vma
 *
 * Each 4KB page of a 4KB VMA is backed by one 4KB slice (0..3) of a 16KB
 * page. Returns the slice that backs @addr, or 0 for native VMAs.
 *
 * A file VMA maps its first page from slice vm_slice_off of file page
 * vm_pgoff and each following page from the next slice, so the slice is
 * the number of slices from the start of file page vm_pgoff to @addr,
 * modulo the slices per page.
 *
 * For example, with vm_slice_off = 1 and @addr = vm_start + 0x4000, @addr
 * is (0x4000 >> PAGE_SHIFT_4KB) = 4 pages after vm_start, or 1 + 4 = 5
 * slices after the start of file page vm_pgoff. So it is slice
 * 5 & P3S_SLICE_MASK = 1 of file page vm_pgoff + 1:
 *
 *                vm_start                     @addr
 *                    ▼                          ▼
 *                    ┌─────┬─────┬─────┬─────┬─────┐
 *  4KB VMA pages     │  0  │  1  │  2  │  3  │  4  │
 *                    └─────┴─────┴─────┴─────┴─────┘
 *              ┌─────┬─────┬─────┬─────┬─────┬─────┬─────┬─────┐
 *  4KB slices  │  0  │  1  │  2  │  3  │  0  │  1  │  2  │  3  │
 *              ├─────┴─────┴─────┴─────┼─────┴─────┴─────┴─────┤
 *  16KB file   │        vm_pgoff       │      vm_pgoff + 1     │
 *  pages       └───────────────────────┴───────────────────────┘
 *
 * An anonymous folio covers a 16KB-aligned window of virtual addresses,
 * so the slice is the index of @addr's 4KB page within its window.
 */
static inline unsigned int vma_slice_offset(const struct vm_area_struct *vma,
					    unsigned long addr)
{
	/* Native VMAs map whole kernel pages, so there is no slice. */
	if (!vma_is_p3s_4k(vma))
		return 0;

	/*
	 * An anonymous folio covers a 16KB-aligned window of virtual
	 * addresses, so the index of @addr's 4KB page in that window is its
	 * slice.
	 */
	if (!vma->vm_ops)
		return (addr >> PAGE_SHIFT_4KB) & P3S_SLICE_MASK;

	/*
	 * Other VMAs (files, shmem) map folios by offset: vm_start maps slice
	 * vm_slice_off of its folio. Count the 4KB pages from vm_start to
	 * @addr on top of that slice; the low bits are the slice of the folio
	 * that backs @addr.
	 */
	return (vma_slice_off(vma) + ((addr - vma->vm_start) >> PAGE_SHIFT_4KB)) &
	       P3S_SLICE_MASK;
}

/*
 * vma_folio_offset - Byte offset of @addr within the 16KB page backing it
 * @vma: Pointer to struct vm_area_struct
 * @addr: Virtual address within @vma
 *
 * This is the start of the slice that backs @addr plus @addr's offset
 * within its 4KB page. For example, for an address 0x234 bytes into a 4KB
 * page that slice 1 backs:
 *
 *              0x0000    0x1000    0x2000    0x3000    0x4000
 *              ┌─────────┬─────────┬─────────┬─────────┐
 *  16KB page   │ slice 0 │ slice 1 │ slice 2 │ slice 3 │
 *              └─────────┴─────────┴─────────┴─────────┘
 *                        ▲ ▲
 *                        │ └─ @addr is at 0x1000 + 0x234 = 0x1234
 *                        └─── slice 1 starts at 1 << PAGE_SHIFT_4KB = 0x1000
 *
 * For a native VMA the slice is 0, so this is @addr's offset within its
 * 16KB page.
 */
static inline unsigned long vma_folio_offset(const struct vm_area_struct *vma,
					     unsigned long addr)
{
	/*
	 * The slice shifted by PAGE_SHIFT_4KB is the start of @addr's 4KB
	 * page within the 16KB folio, and vma_offset_in_page() is @addr's
	 * offset within that page. Native VMAs have slice 0, which leaves the
	 * offset within the 16KB page.
	 */
	return ((unsigned long)vma_slice_offset(vma, addr) << PAGE_SHIFT_4KB) |
	       vma_offset_in_page(vma, addr);
}

/*
 * p3s_adjust_unmapped_area_info - Align unmapped area allocations to host page
 * @info: Pointer to struct vm_unmapped_area_info
 *
 * In a 4KB compatibility process, virtual memory is backed by 16KB host
 * folios. When searching for an unmapped address range (mmap), aligning the
 * allocation to 16KB host page boundaries (~PAGE_MASK_KERNEL) ensures new
 * VMAs naturally start at slice 0 (folio boundary) unless the caller
 * requested an explicit alignment. This prevents initial subpage slice offsets
 * (vm_slice_off), avoids wasting the host folio at the start, and minimizes
 * fragmentation.
 */
#ifdef CONFIG_ARM64_PER_PROCESS_PAGE_SIZE
#define p3s_adjust_unmapped_area_info(info) do { \
	if (!(info)->align_mask && current->mm && mm_is_p3s_4k(current->mm)) \
		(info)->align_mask = ~PAGE_MASK_KERNEL; \
} while (0)
#else
#define p3s_adjust_unmapped_area_info(info) ((void)(info))
#endif

/*
 * vma_folio_slices_exclusive - Small anon folio mapped only by this process
 * @vma: Pointer to struct vm_area_struct mapping @folio
 * @folio: Folio found through a PTE of @vma
 *
 * In a 4KB compatibility process a small anonymous folio can be mapped by
 * up to four 4KB slice PTEs, so its mapcount can exceed its page count while
 * every mapping still belongs to this process (PageAnonExclusive). A macro
 * because the folio flag helpers are not yet defined where this header is
 * included from <linux/mm_types.h>.
 */
#define vma_folio_slices_exclusive(vma, folio)				\
	(vma_is_p3s_4k(vma) && folio_test_anon(folio) &&		\
	 !folio_test_large(folio) && PageAnonExclusive(&(folio)->page))

#endif /* _LINUX_P3S_VMA_H */
