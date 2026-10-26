/* SPDX-License-Identifier: GPL-2.0 */
#ifndef __MM_P3S_MREMAP_H
#define __MM_P3S_MREMAP_H

#include <linux/mm_types.h>

struct p3s_mremap_ctx;

#ifdef CONFIG_ARM64_PER_PROCESS_PAGE_SIZE
bool p3s_mremap_can_reslice(struct vm_area_struct *vma);
struct p3s_mremap_ctx *p3s_mremap_prepare(struct vm_area_struct *src_vma,
		struct vm_area_struct *dst_vma, unsigned long old_addr,
		unsigned long new_addr, unsigned long len);
void p3s_mremap_reslice(struct p3s_mremap_ctx *ctx,
		struct vm_area_struct *vma, unsigned long new_addr,
		unsigned long len);
void p3s_mremap_finish(struct p3s_mremap_ctx *ctx);
#else
static inline bool p3s_mremap_can_reslice(struct vm_area_struct *vma)
{
	return false;
}

static inline struct p3s_mremap_ctx *p3s_mremap_prepare(
		struct vm_area_struct *src_vma, struct vm_area_struct *dst_vma,
		unsigned long old_addr, unsigned long new_addr, unsigned long len)
{
	return NULL;
}

static inline void p3s_mremap_reslice(struct p3s_mremap_ctx *ctx,
		struct vm_area_struct *vma, unsigned long new_addr,
		unsigned long len)
{
}

static inline void p3s_mremap_finish(struct p3s_mremap_ctx *ctx)
{
}
#endif

#endif /* __MM_P3S_MREMAP_H */
