/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_P3S_MMAN_H
#define _LINUX_P3S_MMAN_H

static inline void vm_acct_memory_units(long units)
{
	percpu_counter_add_batch(&vm_committed_as, units,
				 vm_committed_as_batch);
}

static inline void vm_acct_memory_mm(const struct mm_struct *mm, long pages)
{
	vm_acct_memory_units(vm_commit_mm_pages(mm, pages));
}

static inline void vm_unacct_memory_mm(const struct mm_struct *mm, long pages)
{
	vm_acct_memory_mm(mm, -pages);
}

static inline unsigned long vm_memory_committed_kbytes(void)
{
	return percpu_counter_sum_positive(&vm_committed_as) <<
		(VM_COMMIT_SHIFT - 10);
}

#endif /* _LINUX_P3S_MMAN_H */
