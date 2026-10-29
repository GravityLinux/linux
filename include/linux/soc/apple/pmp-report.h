/* SPDX-License-Identifier: GPL-2.0-only OR MIT */
#ifndef _LINUX_SOC_APPLE_PMP_REPORT_H
#define _LINUX_SOC_APPLE_PMP_REPORT_H

struct device;
int apple_pmp_report_wait_ready(struct device *dev, unsigned long timeout_us);

#endif
