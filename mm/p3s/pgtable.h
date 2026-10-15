/* SPDX-License-Identifier: GPL-2.0 */
#ifndef __MM_P3S_PGTABLE_H
#define __MM_P3S_PGTABLE_H

#include <linux/mm.h>

/*
 * vma_pte_add_slice - Apply subpage slice offset to a PTE value
 * @vma: Pointer to struct vm_area_struct
 * @addr: Virtual address within @vma
 * @pte: Base page table entry
 *
 * In 4KB compatibility mode, encodes the subpage slice index (0..3) into
 * the PTE physical frame bits.
 */
static inline pte_t vma_pte_add_slice(const struct vm_area_struct *vma,
				      unsigned long addr,
				      pte_t pte)
{
	if (!vma_is_p3s_4k(vma))
		return pte;
	return pte_advance_phys(pte,
				vma_slice_offset(vma, addr) << PAGE_SHIFT_4KB);
}

/*
 * vma_remap_frame - First PTE frame for remap_pfn_range() at @addr
 * @vma: VMA being remapped
 * @addr: First virtual address being remapped
 * @pfn: Kernel page frame number to map at @addr
 *
 * remap_pfn_range() maps consecutive frames, one per PTE. The PTEs of a
 * 4KB compatibility VMA map 4KB frames: start at the slice of @pfn that
 * vma_slice_offset() assigns to @addr, as insert_pfn() does.
 */
static inline unsigned long vma_remap_frame(const struct vm_area_struct *vma,
					    unsigned long addr,
					    unsigned long pfn)
{
	if (!vma_is_p3s_4k(vma))
		return pfn;
	return (pfn << P3S_SLICE_SHIFT) + vma_slice_offset(vma, addr);
}

/*
 * mm_frame_pfn - Kernel page frame number of PTE frame @frame of @mm
 * @mm: Pointer to struct mm_struct
 * @frame: Frame number in units of the page size of @mm
 */
static inline unsigned long mm_frame_pfn(const struct mm_struct *mm,
					 unsigned long frame)
{
	return mm_is_p3s_4k(mm) ? frame >> P3S_SLICE_SHIFT : frame;
}

/*
 * mm_frame_pte - Construct a PTE of @mm that maps PTE frame @frame
 * @mm: Pointer to struct mm_struct
 * @frame: Frame number in units of the page size of @mm
 * @prot: Page protection
 */
static inline pte_t mm_frame_pte(const struct mm_struct *mm,
				 unsigned long frame, pgprot_t prot)
{
	pte_t pte = pfn_pte(mm_frame_pfn(mm, frame), prot);

	if (!mm_is_p3s_4k(mm))
		return pte;
	return pte_advance_phys(pte,
				(frame & P3S_SLICE_MASK) << PAGE_SHIFT_4KB);
}

#endif /* __MM_P3S_PGTABLE_H */
