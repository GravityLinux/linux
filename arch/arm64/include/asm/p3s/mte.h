/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _ASM_ARM64_P3S_MTE_H
#define _ASM_ARM64_P3S_MTE_H

#include <linux/bits.h>
#include <linux/p3s/vma.h>
#include <asm/mte-def.h>
#include <asm/mte-kasan.h>

static inline unsigned long mte_tag_dump_offset(unsigned long page_offset)
{
	return page_offset * MTE_TAG_SIZE /
		(MTE_GRANULE_SIZE * BITS_PER_BYTE);
}

/*
 * mte_zero_clear_slice_tags - Zero a 4KB slice and its allocation tags
 * @addr: Kernel address of a 4KB slice of a host page
 *
 * The 4KB counterpart of mte_zero_clear_page_tags(), for PROT_MTE mappings.
 * mte_sync_tags() initialises a host page's tags only the first time the
 * page is mapped tagged, so a slice of an already tagged host page that is
 * handed out again keeps the tags of its previous mapping unless they are
 * cleared here.
 */
static inline void mte_zero_clear_slice_tags(void *addr)
{
	/* Store tag 0 like mte_zero_clear_page_tags(): drop the pointer tag */
	addr = (void *)((u64)addr & GENMASK_ULL(MTE_TAG_SHIFT - 1, 0));
	mte_set_mem_tag_range(addr, PAGE_SIZE_4KB, 0, true);
}

/*
 * mte_clear_slice_tags - Clear the allocation tags of a 4KB slice
 * @addr: Kernel address of a 4KB slice of a host page
 *
 * The 4KB counterpart of mte_clear_page_tags(), for a slice whose data the
 * caller overwrites in full: like mte_zero_clear_slice_tags(), but the
 * data is left alone.
 */
static inline void mte_clear_slice_tags(void *addr)
{
	addr = (void *)((u64)addr & GENMASK_ULL(MTE_TAG_SHIFT - 1, 0));
	mte_set_mem_tag_range(addr, PAGE_SIZE_4KB, 0, false);
}

#endif /* _ASM_ARM64_P3S_MTE_H */
