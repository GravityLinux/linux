/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_P3S_H
#define _LINUX_P3S_H

#include <linux/p3s/const.h>
#include <linux/p3s/mm.h>

/* asm/p3s.h is architecture-specific to arm64 */
#ifdef CONFIG_ARM64
#include <asm/p3s.h>
#else
#ifndef __ASSEMBLY__
#define P3S_CONTEXT_REMOTE_MM(remote_mm)	do { } while (0)
static inline struct mm_struct *p3s_current_mm(void)
{
	return current ? current->mm : NULL;
}
static inline void p3s_assert_matching_pte_shift(const struct mm_struct *mm)
{
}
#endif /* !__ASSEMBLY__ */
#endif /* CONFIG_ARM64 */

#endif /* _LINUX_P3S_H */
