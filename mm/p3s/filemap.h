/* SPDX-License-Identifier: GPL-2.0 */
#ifndef __MM_P3S_FILEMAP_H
#define __MM_P3S_FILEMAP_H

#include <linux/fs.h>
#include <linux/mm.h>

/*
 * vma_slice_past_eof - Check whether a 4KB VMA faults on a slice past EOF
 * @vma: VMA of the fault
 * @addr: Faulting virtual address
 * @inode: Backing inode
 *
 * The fault handlers raise SIGBUS for a file page at or after the first page
 * that lies entirely past EOF. In a 4KB VMA, a 4KB page in the last file page
 * can also be past EOF: it is if the file offset that @addr maps,
 * vma_file_offset_at(), is at or after i_size.
 *
 * For example, with i_size = 0x9800, file page 2, at file offsets 0x8000 to
 * 0xbfff, is the last one. Its slice 1 starts at 0x9000, before i_size, so it
 * can be mapped. Its slices 2 and 3 start at 0xa000 and 0xb000, after i_size,
 * so faults on them get SIGBUS:
 *
 *               0x8000    0x9000    0xa000    0xb000    0xc000
 *               ┌─────────┬─────────┬─────────┬─────────┐
 *  file page 2  │ slice 0 │ slice 1 │ slice 2 │ slice 3 │
 *               ├─────────┴─────────┼─────────┴─────────┤
 *  fault        │       mapped      │       SIGBUS      │
 *               └───────────────────┴───────────────────┘
 *                              ▲
 *                              └─ i_size = 0x9800
 *
 * Returns false for other VMAs: their pages are whole file pages, which the
 * page index check of the callers already covers.
 */
static inline bool vma_slice_past_eof(const struct vm_area_struct *vma,
				      unsigned long addr,
				      const struct inode *inode)
{
	return vma_is_p3s_4k(vma) &&
	       vma_file_offset_at(vma, addr) >= i_size_read(inode);
}

#endif /* __MM_P3S_FILEMAP_H */
