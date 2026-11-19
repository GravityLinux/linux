/* SPDX-License-Identifier: GPL-2.0-only OR MIT */
#ifndef _LINUX_SOC_APPLE_PMP_REPORT_H
#define _LINUX_SOC_APPLE_PMP_REPORT_H
#include <linux/types.h>

struct device;
struct device_node;
int apple_pmp_report_snapshot(struct device *dev, u64 *target, u64 *actual, u64 *status);


/* Replace one firmware boot-only always-on vote with a balanced idle owner. */
struct device *devm_apple_pmp_report_idle_owner(struct device *owner,
					     struct device_node *node);

/* One T8132 GPU runtime; active is its aggregate unretired flight count. */
void apple_pmp_cpu_vote_gpu_activity(unsigned int active);

int apple_pmp_report_wait_ready(struct device *dev, unsigned long timeout_us);

#endif
