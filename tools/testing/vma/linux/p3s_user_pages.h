/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _VMA_TEST_P3S_USER_PAGES_H
#define _VMA_TEST_P3S_USER_PAGES_H

/* CONFIG_ARM64_PER_PROCESS_PAGE_SIZE is disabled in this native harness. */
#define P3S_CONTEXT_REMOTE_MM(mm) do { (void)(mm); } while (0)

#endif
