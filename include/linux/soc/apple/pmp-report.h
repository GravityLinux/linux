/* SPDX-License-Identifier: GPL-2.0-only OR MIT */
#ifndef _LINUX_SOC_APPLE_PMP_REPORT_H
#define _LINUX_SOC_APPLE_PMP_REPORT_H

struct device;

/* One T8132 GPU runtime; active is its aggregate unretired flight count. */
void apple_pmp_cpu_vote_gpu_activity(unsigned int active);

int apple_pmp_report_wait_ready(struct device *dev, unsigned long timeout_us);

#endif
