/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __ASM_P3S_H
#define __ASM_P3S_H

#ifndef __ASSEMBLY__

#include <linux/compiler.h>
#include <linux/sched.h>
#include <linux/mm_types.h>
#include <linux/mmdebug.h>
#include <linux/cleanup.h>
#include <linux/p3s/mm.h>

extern struct mm_struct init_mm;

#ifdef CONFIG_ARM64_PER_PROCESS_PAGE_SIZE

static __always_inline struct mm_struct *p3s_current_mm(void)
{
	if (likely(current))
		return unlikely(current->p3s_remote_mm) ?
		       current->p3s_remote_mm : current->mm;
	return NULL;
}

/*
 * Assert that the active execution context (p3s_current_mm()) has a
 * translation granule matching @mm. We check pte_shift rather than
 * pointer equality because dual-MM operations like fork() (copy_page_range)
 * traverse two distinct MMs simultaneously while sharing the same page
 * size.
 */
static __always_inline void
p3s_assert_matching_pte_shift(const struct mm_struct *mm)
{
	if (unlikely(!mm || mm == &init_mm))
		return;
	VM_WARN_ON_ONCE(mm_pte_shift(p3s_current_mm()) != mm_pte_shift(mm));
}

static inline struct mm_struct *p3s_enter_remote_mm(struct mm_struct *mm)
{
	struct mm_struct *prev = NULL;

	if (likely(current)) {
		prev = current->p3s_remote_mm;
		current->p3s_remote_mm = mm;
	}
	return prev;
}

static inline void p3s_exit_remote_mm(struct mm_struct *prev)
{
	if (likely(current))
		current->p3s_remote_mm = prev;
}

DEFINE_CLASS(p3s_remote_mm, struct mm_struct *,
	     p3s_exit_remote_mm(_T),
	     p3s_enter_remote_mm(_mm),
	     struct mm_struct *_mm);

#define P3S_CONTEXT_REMOTE_MM(remote_mm) \
	CLASS(p3s_remote_mm, __UNIQUE_ID(p3s_rmm))(remote_mm)

#else
#define P3S_CONTEXT_REMOTE_MM(remote_mm)	do { } while (0)
static inline struct mm_struct *p3s_current_mm(void)
{
	return current ? current->mm : NULL;
}
static inline void p3s_assert_matching_pte_shift(const struct mm_struct *mm)
{
}
#endif /* CONFIG_ARM64_PER_PROCESS_PAGE_SIZE */

#endif /* !__ASSEMBLY__ */

#endif /* __ASM_P3S_H */
