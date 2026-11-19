/* SPDX-License-Identifier: GPL-2.0 */
#ifndef __MM_P3S_PAGE_TABLE_CHECK_H
#define __MM_P3S_PAGE_TABLE_CHECK_H

#include <linux/p3s/mm.h>

#ifdef CONFIG_ARM64_PER_PROCESS_PAGE_SIZE
#define P3S_PTC_NATIVE_MAPS P3S_SLICES_PER_PAGE
#else
#define P3S_PTC_NATIVE_MAPS 1
#endif

/* Count anon mappings in 4KB slices, with the same charge on set and clear. */
static inline unsigned int p3s_ptc_nr_maps(const struct mm_struct *mm)
{
	return mm_is_p3s_4k(mm) ? 1 : P3S_PTC_NATIVE_MAPS;
}

#endif /* __MM_P3S_PAGE_TABLE_CHECK_H */
