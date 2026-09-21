// SPDX-License-Identifier: GPL-2.0
#include <linux/init.h>
#include <linux/kstrtox.h>
#include <linux/mm_types.h>
#include <linux/export.h>
#include <linux/mm.h>
#include <linux/p3s/mm.h>
#include "vma.h"

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
