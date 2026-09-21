/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_P3S_CONST_H
#define _LINUX_P3S_CONST_H

#ifndef __ASSEMBLY__
#include <linux/types.h>
#include <linux/align.h>
#endif
#include <linux/sizes.h>

/* Architecture 4KB translation granule constants */
#define PAGE_SHIFT_4KB           12
#define PAGE_SIZE_4KB            (_AC(1, UL) << PAGE_SHIFT_4KB)
#define PAGE_MASK_4KB            (~(PAGE_SIZE_4KB - 1))
#ifdef CONFIG_ARM64_PER_PROCESS_PAGE_SIZE
#define __PTE_ADDR_LOW \
	(((_AC(1, ULL) << (50 - PAGE_SHIFT_4KB)) - 1) << PAGE_SHIFT_4KB)
#else
#define __PTE_ADDR_LOW           PTE_ADDR_LOW
#endif

/* 4KB 3-level page table geometry constants (39-bit VA) */
#define PMD_SHIFT_4KB            21
#define PMD_SIZE_4KB             (_AC(1, UL) << PMD_SHIFT_4KB)

#define PUD_SHIFT_4KB            30
#define PUD_SIZE_4KB             (_AC(1, UL) << PUD_SHIFT_4KB)

#define PGDIR_SHIFT_4KB          30
#define PGDIR_SIZE_4KB           (_AC(1, UL) << PGDIR_SHIFT_4KB)

/* Host kernel geometry constants */
#define PAGE_SHIFT_KERNEL        CONFIG_PAGE_SHIFT
#define PAGE_SIZE_KERNEL         (_AC(1, UL) << PAGE_SHIFT_KERNEL)
#define PAGE_MASK_KERNEL         (~(PAGE_SIZE_KERNEL - 1))

/* 4KB virtual address space limits */
#define VA_BITS_4KB              39
#define TASK_SIZE_4KB            (_AC(1, UL) << VA_BITS_4KB)
#define DEFAULT_MAP_WINDOW_4KB   TASK_SIZE_4KB
#define STACK_TOP_MAX_4KB        DEFAULT_MAP_WINDOW_4KB
#define STACK_TOP_4KB            STACK_TOP_MAX_4KB

/* Subpage slice geometry constants (4KB slices within 16KB host folios) */
#define P3S_SLICES_PER_PAGE      (PAGE_SIZE_KERNEL / PAGE_SIZE_4KB)
#define P3S_SLICE_SHIFT          (PAGE_SHIFT_KERNEL - PAGE_SHIFT_4KB)
#define P3S_SLICE_MASK           (P3S_SLICES_PER_PAGE - 1)

#ifndef __ASSEMBLY__
/* 4KB process page alignment helpers */
#define PAGE_ALIGN_4KB(addr)          ALIGN((addr), PAGE_SIZE_4KB)
#define PAGE_ALIGN_DOWN_4KB(addr)     ALIGN_DOWN((addr), PAGE_SIZE_4KB)

/*
 * In bottom-up mmap layout, start unmapped allocations at 1/4 of the
 * address space (matching upstream TASK_UNMAPPED_BASE). This reserves
 * the lower 1/4 for the binary and heap to expand upwards without
 * colliding with mmaps (0..128GB in 39-bit compat mode, 0..32TB in
 * 47-bit native mode).
 */
#define TASK_UNMAPPED_BASE_4KB        (PAGE_ALIGN_4KB(DEFAULT_MAP_WINDOW_4KB / 4))
#define TASK_UNMAPPED_BASE_KERNEL     (PAGE_ALIGN_KERNEL(DEFAULT_MAP_WINDOW / 4))

/* Host kernel page alignment helpers */
#define PAGE_ALIGN_KERNEL(addr)       ALIGN((addr), PAGE_SIZE_KERNEL)
#define PAGE_ALIGN_DOWN_KERNEL(addr)  ALIGN_DOWN((addr), PAGE_SIZE_KERNEL)
#define PAGE_ALIGNED_KERNEL(addr)     IS_ALIGNED((unsigned long)(addr), PAGE_SIZE_KERNEL)
#endif /* !__ASSEMBLY__ */

#endif /* _LINUX_P3S_CONST_H */
