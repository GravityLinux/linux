/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _ASM_ARM64_P3S_MMU_H
#define _ASM_ARM64_P3S_MMU_H

#include <linux/p3s/mm.h>
#include <asm/memory.h>
#include <asm/pgtable-hwdef.h>
#include <asm/processor.h>
#include <asm/sysreg.h>

static inline unsigned long mm_task_size(const struct mm_struct *mm)
{
	return mm_is_p3s_4k(mm) ? TASK_SIZE_4KB : TASK_SIZE;
}

static inline unsigned long mm_default_map_window(const struct mm_struct *mm)
{
	return mm_is_p3s_4k(mm) ? DEFAULT_MAP_WINDOW_4KB : DEFAULT_MAP_WINDOW;
}

static inline unsigned long mm_stack_top_max(const struct mm_struct *mm)
{
	return mm_is_p3s_4k(mm) ? STACK_TOP_MAX_4KB : STACK_TOP_MAX;
}

static inline unsigned long mm_stack_top(const struct mm_struct *mm)
{
	return mm_is_p3s_4k(mm) ? STACK_TOP_4KB : STACK_TOP;
}

static inline unsigned long mm_task_unmapped_base(const struct mm_struct *mm)
{
	return mm_is_p3s_4k(mm) ? TASK_UNMAPPED_BASE_4KB :
				   TASK_UNMAPPED_BASE_KERNEL;
}

/*
 * mm_init_new_context - Initialize per-MM translation granule
 * @tsk: Task struct associated with the new MM
 * @mm: New mm_struct being initialized
 *
 * Called from init_new_context() to set mm->context.pte_shift based on the
 * task personality and boot-time configuration.
 */
static inline void mm_init_new_context(const struct task_struct *tsk,
				       struct mm_struct *mm)
{
	mm_init_pte_shift(mm, tsk);
}

/*
 * mm_dup_mmap - Inherit per-MM translation granule across fork
 * @oldmm: Parent mm_struct
 * @mm: Child mm_struct being created
 *
 * Called from arch_dup_mmap() to propagate mm->context.pte_shift from parent
 * to child address space.
 */
static inline void mm_dup_mmap(const struct mm_struct *oldmm,
			       struct mm_struct *mm)
{
	mm_set_pte_shift(mm, mm_pte_shift(oldmm));
}

#endif /* _ASM_ARM64_P3S_MMU_H */
