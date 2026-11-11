/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _ASM_ARM64_P3S_TLB_H
#define _ASM_ARM64_P3S_TLB_H

#include <linux/p3s/mm.h>
#include <asm-generic/tlb.h>

static inline unsigned long mm_tlb_get_unmap_size(struct mmu_gather *tlb)
{
	if (tlb->cleared_ptes)
		return mm_pte_size(tlb->mm);
	if (tlb->cleared_pmds)
		return mm_pmd_size(tlb->mm);
	if (tlb->cleared_puds || tlb->cleared_p4ds)
		return mm_pgdir_size(tlb->mm);

	return mm_pte_size(tlb->mm);
}

#endif /* _ASM_ARM64_P3S_TLB_H */
