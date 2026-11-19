/* SPDX-License-Identifier: GPL-2.0-only OR MIT */
#ifndef _LINUX_SOC_APPLE_PMGR_H
#define _LINUX_SOC_APPLE_PMGR_H
#include <linux/types.h>
struct device_node;
/* Read-only state for a DT-described power domain. */
int apple_pmgr_read_state(struct device_node *node, u32 *value);
#endif
