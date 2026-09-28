// SPDX-License-Identifier: GPL-2.0
#include <linux/init.h>
#include <linux/kstrtox.h>
#include <linux/mm_types.h>
#include <linux/export.h>
#include <linux/mm.h>
#include <linux/p3s/mm.h>
#include <linux/cacheflush.h>
#include "vma.h"
#include "p3s/pgtable.h"

enum p3s_4k_mode p3s_4k_mode __ro_after_init = P3S_4K_MODE_OFF;
EXPORT_SYMBOL(p3s_4k_mode);

static int __init parse_p3s_4k(char *str)
{
	bool enabled;

	if (!str)
		return -EINVAL;

	if (!kstrtobool(str, &enabled)) {
		p3s_4k_mode = enabled ? P3S_4K_MODE_ON : P3S_4K_MODE_OFF;
		return 0;
	}

	if (!strcmp(str, "alternate")) {
		p3s_4k_mode = P3S_4K_MODE_ALTERNATE;
		return 0;
	}

	return -EINVAL;
}
early_param("p3s_4k", parse_p3s_4k);

/* Map one explicit physical slice when it differs from the VMA phase. */
int p3s_remap_pfn_slice(struct vm_area_struct *vma, unsigned long addr,
			unsigned long pfn, unsigned int slice, pgprot_t prot)
{
	P3S_CONTEXT_REMOTE_MM(vma->vm_mm);
	struct mm_struct *mm = vma->vm_mm;
	unsigned long frame;
	pte_t *pte;
	spinlock_t *ptl;
	int err = 0;

	if (!mm_is_p3s_4k(mm) ||
	    !mm_pte_aligned(mm, addr) || addr < vma->vm_start ||
	    addr >= vma->vm_end || PAGE_SIZE_4KB > vma->vm_end - addr ||
	    slice >= mm_slices_per_page(mm))
		return -EINVAL;
	if (!pfn_modify_allowed(pfn, prot))
		return -EACCES;
	vm_flags_set(vma, VM_IO | VM_PFNMAP | VM_DONTEXPAND | VM_DONTDUMP);
	flush_cache_range(vma, addr, addr + PAGE_SIZE_4KB);
	pte = get_locked_pte(mm, addr, &ptl);
	if (!pte)
		return -ENOMEM;
	if (!pte_none(ptep_get(pte))) {
		err = -EBUSY;
		goto unlock;
	}
	frame = (pfn << P3S_SLICE_SHIFT) + slice;
	set_pte_at(mm, addr, pte, pte_mkspecial(mm_frame_pte(mm, frame, prot)));
unlock:
	pte_unmap_unlock(pte, ptl);
	return err;
}

#define vmg_is_p3s_4k(vmg)	mm_is_p3s_4k((vmg)->mm)

/*
 * vma_can_merge_offsets - Check that the range at @boundary continues @left
 * @left: Left (lower-address) VMA
 * @right_pgoff: Page offset of right (higher-address) range
 * @right_slice: Starting subpage slice of right range (0..3)
 * @boundary: Virtual address where left ends and right begins
 * @is_p3s_4k: Whether process is running in 4KB compat mode
 * @is_file: Whether mapping is file-backed
 */
static bool vma_can_merge_offsets(const struct vm_area_struct *left,
				  pgoff_t right_pgoff,
				  unsigned short right_slice,
				  unsigned long boundary,
				  bool is_p3s_4k, bool is_file)
{
	return vma_linear_page_index(left, boundary) == right_pgoff &&
	       (!is_p3s_4k || !is_file ||
		vma_slice_offset(left, boundary) == right_slice);
}

/*
 * vmg_can_merge_offsets - Verify offset & slice continuity for VMA merge
 * @vmg: VMA merge structure
 * @merge_next: True if merging with vmg->next, false if merging with vmg->prev
 */
bool vmg_can_merge_offsets(struct vma_merge_struct *vmg, bool merge_next)
{
	if (merge_next) {
		unsigned long total_slices;

		if (!vmg_is_p3s_4k(vmg))
			return vmg->next->vm_pgoff ==
			       vmg->pgoff + PHYS_PFN(vmg->end - vmg->start);

		/*
		 * Count pages the way next does (see vma_linear_page_index()):
		 * 4KB pages if it is anonymous, else slices from vmg->slice_off.
		 */
		if (vma_is_anonymous(vmg->next))
			return vmg->next->vm_pgoff ==
			       vmg->pgoff + ((vmg->end - vmg->start) >> PAGE_SHIFT_4KB);

		total_slices = vmg->slice_off + ((vmg->end - vmg->start) >> PAGE_SHIFT_4KB);

		return vmg->next->vm_pgoff == vmg->pgoff + (total_slices >> P3S_SLICE_SHIFT) &&
		       vma_slice_off(vmg->next) == (total_slices & P3S_SLICE_MASK);
	}

	return vma_can_merge_offsets(vmg->prev, vmg->pgoff, vmg->slice_off,
				     vmg->start, vmg_is_p3s_4k(vmg),
				     !vma_is_anonymous(vmg->prev));
}
