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
 * vma_file_offset - Byte offset in the file that vm_start maps
 * @vma: Pointer to struct vm_area_struct
 *
 * In a 4KB VMA, vm_pgoff still counts 16KB pages, and vm_slice_off is the
 * slice of file page vm_pgoff that vm_start maps. The file offset of
 * vm_start is the start of that page plus the start of the slice. For
 * example, with vm_slice_off = 1:
 *
 *                                vm_start
 *                                    ▼
 *                                    ┌─────┬─────┬─────┐
 *  4KB VMA pages                     │  0  │  1  │  2  │
 *                                    └─────┴─────┴─────┘
 *                              ┌─────┬─────┬─────┬─────┐
 *  4KB slices                  │  0  │  1  │  2  │  3  │
 *              ┌─────┬─────────┼─────┴─────┴─────┴─────┤
 *  16KB file   │  0  │   ...   │        vm_pgoff       │
 *  pages       └─────┴─────────┴───────────────────────┘
 *                              ▲     ▲
 *                              │◄───►│ vm_slice_off << PAGE_SHIFT_4KB
 *                              │     └─ vma_file_offset()
 *                              └─ vm_pgoff << PAGE_SHIFT_KERNEL
 *
 * A native VMA has no slice, so this is the usual vm_pgoff << PAGE_SHIFT.
 */
static inline loff_t vma_file_offset(const struct vm_area_struct *vma)
{
	return ((loff_t)vma->vm_pgoff << PAGE_SHIFT_KERNEL) +
	       ((loff_t)vma_slice_off(vma) << PAGE_SHIFT_4KB);
}

/*
 * vma_file_offset_at - Byte offset in the file that @addr maps
 * @vma: Pointer to struct vm_area_struct
 * @addr: Virtual address within @vma
 *
 * vm_start maps the file byte at vma_file_offset(), and @addr maps the
 * byte (@addr - vm_start) bytes after it. In a 4KB VMA, vm_start can map
 * any 4KB slice of a 16KB file page.
 *
 * For example, with vm_slice_off = 1, vma_file_offset() is
 * (1 << PAGE_SHIFT_4KB) = 0x1000 bytes into file page vm_pgoff. So
 * @addr = vm_start + 0x4000 maps 0x1000 + 0x4000 = 0x5000 bytes from the
 * start of file page vm_pgoff, which is 0x1000 bytes into file page
 * vm_pgoff + 1:
 *
 *                vm_start                     @addr
 *                    ▼                          ▼
 *                    ┌─────┬─────┬─────┬─────┬─────┐
 *  4KB VMA pages     │  0  │  1  │  2  │  3  │  4  │
 *                    └─────┴─────┴─────┴─────┴─────┘
 *              ┌───────────────────────┬───────────────────────┐
 *  16KB file   │        vm_pgoff       │      vm_pgoff + 1     │
 *  pages       └───────────────────────┴───────────────────────┘
 *                    ▲                          ▲
 *                    │◄─── @addr - vm_start ───►│
 *                    │                          └─ vma_file_offset_at()
 *                    └─ vma_file_offset()
 *
 * For a native VMA, vma_file_offset() is the usual vm_pgoff << PAGE_SHIFT.
 */
static inline loff_t vma_file_offset_at(const struct vm_area_struct *vma,
					unsigned long addr)
{
	return vma_file_offset(vma) + (addr - vma->vm_start);
}

/*
 * vma_linear_page_index - Page offset that @addr maps
 * @vma: Pointer to struct vm_area_struct
 * @addr: Virtual address within @vma
 *
 * For a 4KB file VMA, this is the file page that backs @addr. vm_start maps
 * slice vm_slice_off of file page vm_pgoff and each following 4KB page maps
 * the next slice, so count the slices from the start of file page vm_pgoff
 * to @addr and divide by the slices per page.
 *
 * For example, with vm_slice_off = 1 and @addr = vm_start + 0x4000, @addr
 * is (0x4000 >> PAGE_SHIFT_4KB) = 4 pages after vm_start, or 1 + 4 = 5
 * slices after the start of file page vm_pgoff. So it is in file page
 * vm_pgoff + (5 >> P3S_SLICE_SHIFT) = vm_pgoff + 1:
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
 *                                               ▲
 *                                               └─ vma_linear_page_index()
 *
 * Anonymous 4KB VMAs count 4KB pages from vm_pgoff instead. Native VMAs,
 * and 4KB VMAs with vm_ops but no file, count 16KB pages.
 */
static inline pgoff_t vma_linear_page_index(const struct vm_area_struct *vma,
					    unsigned long addr)
{
	if (!vma_is_p3s_4k(vma) || (vma->vm_ops && !vma->vm_file))
		return vma->vm_pgoff +
		       ((addr - vma->vm_start) >> PAGE_SHIFT_KERNEL);

	if (!vma->vm_ops)
		return vma->vm_pgoff +
		       ((addr - vma->vm_start) >> PAGE_SHIFT_4KB);

	return vma->vm_pgoff +
	       ((vma_slice_off(vma) + ((addr - vma->vm_start) >> PAGE_SHIFT_4KB)) >>
		P3S_SLICE_SHIFT);
}

/*
 * vma_pgoff_pages - Number of page offsets that @vma covers
 * @vma: Pointer to struct vm_area_struct
 *
 * @vma covers page offsets vm_pgoff to vm_pgoff + vma_pgoff_pages() - 1.
 * A 4KB file VMA can start and end partway through a 16KB file page, so
 * count the slices from the start of file page vm_pgoff to vm_end and
 * round up to whole file pages.
 *
 * For example, a VMA with vm_slice_off = 2 and vm_end = vm_start + 0x4000
 * is four 4KB pages long (0x4000 >> PAGE_SHIFT_4KB), so it covers two 16KB
 * file pages (DIV_ROUND_UP(2 + 4, P3S_SLICES_PER_PAGE)) although it is
 * only 16KB:
 *
 *                      vm_start                 vm_end
 *                          ▼                       ▼
 *                          ┌─────┬─────┬─────┬─────┐
 *  4KB VMA pages           │  0  │  1  │  2  │  3  │
 *                          └─────┴─────┴─────┴─────┘
 *              ┌─────┬─────┬─────┬─────┬─────┬─────┬─────┬─────┐
 *  4KB slices  │  0  │  1  │  2  │  3  │  0  │  1  │  2  │  3  │
 *              ├─────┴─────┴─────┴─────┼─────┴─────┴─────┴─────┤
 *  16KB file   │        vm_pgoff       │      vm_pgoff + 1     │
 *  pages       └───────────────────────┴───────────────────────┘
 *              │◄────────── vma_pgoff_pages() = 2 ────────────►│
 *
 * Anonymous 4KB VMAs count 4KB pages, and native VMAs count 16KB pages.
 */
static inline pgoff_t vma_pgoff_pages(const struct vm_area_struct *vma)
{
	unsigned long nr_slices = vma_size(vma) >> mm_pte_shift(vma->vm_mm);

	if (!vma_is_p3s_4k(vma) || !vma->vm_ops)
		return nr_slices;

	return DIV_ROUND_UP(vma_slice_off(vma) + nr_slices,
			    P3S_SLICES_PER_PAGE);
}

/*
 * vma_pgoff_to_address - Address in @vma where page offset @pgoff starts
 * @vma: Pointer to struct vm_area_struct
 * @pgoff: Page offset to look up
 *
 * For a file VMA, returns the address that maps slice 0 of file page
 * @pgoff, clamped to the VMA: vm_start, plus (@pgoff - vm_pgoff) file
 * pages, minus the vm_slice_off slices of file page vm_pgoff that are
 * before vm_start.
 *
 * For example, with vm_slice_off = 1, @pgoff = vm_pgoff + 1 gives vm_start
 * plus one 16KB file page (0x4000) minus one slice (0x1000), which is
 * vm_start + 0x3000, the start of VMA page 3. File page vm_pgoff starts
 * before vm_start, so @pgoff = vm_pgoff gives vm_start:
 *
 *  @pgoff        vm_pgoff        vm_pgoff + 1
 *                    ▼                 ▼
 *                    ┌─────┬─────┬─────┬─────┬─────┐
 *  4KB VMA pages     │  0  │  1  │  2  │  3  │  4  │
 *                    └─────┴─────┴─────┴─────┴─────┘
 *              ┌─────┬─────┬─────┬─────┬─────┬─────┬─────┬─────┐
 *  4KB slices  │  0  │  1  │  2  │  3  │  0  │  1  │  2  │  3  │
 *              ├─────┴─────┴─────┴─────┼─────┴─────┴─────┴─────┤
 *  16KB file   │        vm_pgoff       │      vm_pgoff + 1     │
 *  pages       └───────────────────────┴───────────────────────┘
 *
 * A page after the VMA gives vm_end. For an anonymous VMA, @pgoff
 * counts pages of the process page size.
 */
static inline unsigned long
vma_pgoff_to_address(const struct vm_area_struct *vma, pgoff_t pgoff)
{
	unsigned long addr;

	if (pgoff <= vma->vm_pgoff)
		return vma->vm_start;

	if (vma->vm_ops) {
		addr = vma->vm_start +
			((pgoff - vma->vm_pgoff) << PAGE_SHIFT_KERNEL) -
			((unsigned long)vma_slice_off(vma) << PAGE_SHIFT_4KB);
	} else {
		addr = vma->vm_start +
			((pgoff - vma->vm_pgoff) << mm_pte_shift(vma->vm_mm));
	}

	return min(addr, vma->vm_end);
}

/*
 * vma_anon_slice_window - First address of the slices of a page in @vma
 * @vma: Anonymous VMA
 * @pgoff: Index of a small anonymous folio
 *
 * In a 4KB anonymous VMA, @pgoff counts 4KB pages and the slices of the
 * folio are mapped within one 16KB-aligned window of addresses. A VMA split
 * or a 4KB mremap() can leave some of them in a VMA whose page range does
 * not contain @pgoff; unsigned wrap-around then gives the address of the
 * window below vm_start. Returns the first address of the window within
 * @vma, or -EFAULT if @vma holds no part of the window.
 */
static inline unsigned long vma_anon_slice_window(const struct vm_area_struct *vma,
						  pgoff_t pgoff)
{
	unsigned long win = (vma->vm_start +
			     ((pgoff - vma->vm_pgoff) << PAGE_SHIFT_4KB)) &
			    PAGE_MASK_KERNEL;

	if (win >= vma->vm_end || win + PAGE_SIZE_KERNEL <= vma->vm_start)
		return -EFAULT;
	return max(win, vma->vm_start);
}

/*
 * vma_folio_slice_bounds - 4KB pages around @addr that map the same 16KB page
 * @vma: Pointer to struct vm_area_struct
 * @addr: Virtual address within @vma
 * @nr_slices: Output number of contiguous slices (1..4)
 * @start_addr: Output starting virtual address of the slice span
 *
 * In a 4KB VMA, finds the 4KB pages around @addr that map slices of the
 * 16KB page backing @addr, without crossing the VMA or the PTE table that
 * maps @addr. Sets @start_addr to the first of them and @nr_slices to
 * their number.
 *
 * For example, with vm_slice_off = 1 and @addr = vm_start + 0x2000, @addr
 * is at slice 1 + (0x2000 >> PAGE_SHIFT_4KB) = 3 of file page vm_pgoff.
 * Slices 1 to 3 of that page are in the VMA, but slice 0 is before
 * vm_start, so @start_addr is vm_start and @nr_slices is 3:
 *
 *                vm_start         @addr
 *                    ▼              ▼
 *                    ┌─────┬─────┬─────┬─────┬─────┐
 *  4KB VMA pages     │  0  │  1  │  2  │  3  │  4  │
 *                    └─────┴─────┴─────┴─────┴─────┘
 *                    │◄───────────────►│ @nr_slices = 3
 *              ┌─────┬─────┬─────┬─────┬─────┬─────┬─────┬─────┐
 *  4KB slices  │  0  │  1  │  2  │  3  │  0  │  1  │  2  │  3  │
 *              ├─────┴─────┴─────┴─────┼─────┴─────┴─────┴─────┤
 *  16KB file   │        vm_pgoff       │      vm_pgoff + 1     │
 *  pages       └───────────────────────┴───────────────────────┘
 *
 * For a native VMA, @nr_slices is 1 and @start_addr is @addr.
 */
static inline void vma_folio_slice_bounds(const struct vm_area_struct *vma,
					  unsigned long addr,
					  unsigned int *nr_slices,
					  unsigned long *start_addr)
{
	unsigned long start, end, pt;

	if (!vma_is_p3s_4k(vma)) {
		*nr_slices = 1;
		*start_addr = addr;
		return;
	}

	addr &= PAGE_MASK_4KB;
	start = addr - (vma_slice_offset(vma, addr) << PAGE_SHIFT_4KB);
	pt = addr & ~(PMD_SIZE_4KB - 1);
	end = min3(start + PAGE_SIZE_KERNEL, vma->vm_end, pt + PMD_SIZE_4KB);
	start = max3(start, vma->vm_start, pt);

	*start_addr = start;
	*nr_slices = (end - start) >> PAGE_SHIFT_4KB;
}

/*
 * vma_remote_access_chunk - Compute page offset and clamped length for remote access
 * @vma: Pointer to struct vm_area_struct
 * @addr: Virtual address being accessed
 * @len: Maximum bytes to transfer
 * @offset: Pointer to output byte offset within host page
 * @bytes: Pointer to output transferable byte count within virtual page
 */
static inline void vma_remote_access_chunk(const struct vm_area_struct *vma,
					   unsigned long addr, int len,
					   int *offset, int *bytes)
{
	*offset = vma_folio_offset(vma, addr);
	*bytes = min_t(int, len, vma_page_size(vma) - vma_offset_in_page(vma, addr));
}

/*
 * vma_set_split_offset - Update pgoff and slice offset after VMA split
 * @target: Newly created or adjusted VMA
 * @src: Source VMA being split
 * @addr: Split boundary address
 */
static inline void vma_set_split_offset(struct vm_area_struct *target,
					const struct vm_area_struct *src,
					unsigned long addr)
{
	target->vm_pgoff = vma_linear_page_index(src, addr);
	vma_set_slice_off(target, vma_slice_offset(src, addr));
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
