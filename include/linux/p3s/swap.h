/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_P3S_SWAP_H
#define _LINUX_P3S_SWAP_H

#include <linux/types.h>
#include <linux/swap.h>
#include <linux/sort.h>
#include <linux/p3s/const.h>
#include <linux/p3s/mm.h>

#if defined(CONFIG_ARM64_PER_PROCESS_PAGE_SIZE) && defined(CONFIG_SWAP)

/*
 * mkswap writes the 10-byte "SWAPSPACE2" signature (magic.magic of union
 * swap_header) at the very end of the header page, after the info fields
 * and badpages[].
 */
#define P3S_SWAP_MAGIC_SIZE	sizeof_field(union swap_header, magic.magic)

/*
 * Detect swap header geometry. Even though standard Android P3S deployments
 * run only user application processes in 4KB mode and system daemons format
 * swap using the native 16KB kernel page size, supporting 4KB swap headers
 * is valuable to enable running entire userspace test environments in 4KB mode
 * for full-system stability testing.
 */
static inline unsigned int p3s_swap_header_page_shift(const union swap_header *swap_header)
{
	const char *header = (const char *)swap_header;

	if (!memcmp("SWAPSPACE2", header + PAGE_SIZE_KERNEL - P3S_SWAP_MAGIC_SIZE,
		    P3S_SWAP_MAGIC_SIZE))
		return PAGE_SHIFT_KERNEL;

	/*
	 * A 4KB mkswap process formats the area in 4KB process-page units.
	 * The swapon caller need not be that process, so inspect the 4KB
	 * location independently of the caller's current page size.
	 */
	if (!memcmp("SWAPSPACE2", header + PAGE_SIZE_4KB - P3S_SWAP_MAGIC_SIZE,
		    P3S_SWAP_MAGIC_SIZE))
		return PAGE_SHIFT_4KB;

	return PAGE_SHIFT_KERNEL;
}

/*
 * MAX_SWAP_BADPAGES for the page size the header was written with:
 * badpages[] ends where the signature starts.
 */
static inline unsigned long p3s_swap_header_max_badpages(const union swap_header *swap_header)
{
	unsigned int header_page_shift = p3s_swap_header_page_shift(swap_header);

	return ((1UL << header_page_shift) - P3S_SWAP_MAGIC_SIZE -
		offsetof(union swap_header, info.badpages)) /
	       sizeof(swap_header->info.badpages[0]);
}

static inline char *p3s_swap_header_magic(const union swap_header *swap_header)
{
	return (char *)swap_header +
	       (1UL << p3s_swap_header_page_shift(swap_header)) - P3S_SWAP_MAGIC_SIZE;
}

static inline int p3s_swap_badpage_cmp(const void *left, const void *right)
{
	const __u32 left_page = *(const __u32 *)left;
	const __u32 right_page = *(const __u32 *)right;

	return (left_page > right_page) - (left_page < right_page);
}

/*
 * p3s_convert_swap_header - Convert a 4KB swap header to kernel pages
 * @swap_header: Header page read by swapon()
 *
 * The header is not moved: the info fields are at the same offsets in 4KB
 * and 16KB headers, and p3s_swap_header_magic() finds the signature at the
 * end of the first 4KB. Only the page counts written by a 4KB mkswap are
 * converted in place: last_page is rounded down to the last whole kernel
 * page, each bad 4KB page becomes the kernel page that contains it, and
 * duplicates are dropped, as several bad 4KB pages can share a kernel page.
 * Headers written in kernel pages are left untouched.
 *
 * Return: 0 on success, -EINVAL if the 4KB header is empty or malformed.
 */
static inline int p3s_convert_swap_header(union swap_header *swap_header)
{
	unsigned int header_page_shift = p3s_swap_header_page_shift(swap_header);
	unsigned int page_shift_delta = PAGE_SHIFT_KERNEL - header_page_shift;
	unsigned int nr_badpages = swap_header->info.nr_badpages;
	unsigned int i, nr_unique = 0;
	u64 nr_pages;

	if (!page_shift_delta)
		return 0;

	/*
	 * read_swap_header() bounds nr_badpages only after this conversion,
	 * and for a kernel page sized header: bound it before badpages[] is
	 * accessed.
	 */
	if (nr_badpages > p3s_swap_header_max_badpages(swap_header))
		return -EINVAL;

	nr_pages = ((u64)swap_header->info.last_page + 1) >> page_shift_delta;
	if (!nr_pages) {
		pr_warn("Empty swap-file\n");
		return -EINVAL;
	}

	swap_header->info.last_page = nr_pages - 1;
	for (i = 0; i < nr_badpages; i++)
		swap_header->info.badpages[i] >>= page_shift_delta;

	sort(swap_header->info.badpages, nr_badpages,
	     sizeof(swap_header->info.badpages[0]), p3s_swap_badpage_cmp, NULL);
	for (i = 0; i < nr_badpages; i++) {
		if (nr_unique && swap_header->info.badpages[i] ==
				 swap_header->info.badpages[nr_unique - 1])
			continue;
		swap_header->info.badpages[nr_unique++] =
			swap_header->info.badpages[i];
	}
	swap_header->info.nr_badpages = nr_unique;
	return 0;
}

#else /* !(CONFIG_ARM64_PER_PROCESS_PAGE_SIZE && CONFIG_SWAP) */

static inline unsigned long p3s_swap_header_max_badpages(const union swap_header *swap_header)
{
	return MAX_SWAP_BADPAGES;
}

static inline char *p3s_swap_header_magic(const union swap_header *swap_header)
{
	return (char *)swap_header->magic.magic;
}

static inline int p3s_convert_swap_header(union swap_header *swap_header)
{
	return 0;
}

#endif /* CONFIG_ARM64_PER_PROCESS_PAGE_SIZE && CONFIG_SWAP */

#endif /* _LINUX_P3S_SWAP_H */
