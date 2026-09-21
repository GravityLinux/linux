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

#endif /* _ASM_ARM64_P3S_MTE_H */
