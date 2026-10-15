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

#endif /* __MM_P3S_PGTABLE_H */
