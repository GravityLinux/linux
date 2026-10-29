/* SPDX-License-Identifier: GPL-2.0-only OR MIT */
#ifndef APPLE_PMP_CPU_VOTE_H
#define APPLE_PMP_CPU_VOTE_H

#include <linux/kconfig.h>
#include <linux/types.h>

struct platform_device;
struct device;
struct apple_pmp_cpu_vote;
struct apple_pmp_cpu_vote *apple_pmp_report_cpu_vote(struct device *dev);

#if IS_ENABLED(CONFIG_CPU_FREQ)
struct apple_pmp_cpu_vote *
apple_pmp_cpu_vote_probe(struct platform_device *pdev, void __iomem *events);
void apple_pmp_cpu_vote_start(struct apple_pmp_cpu_vote *vote);
void apple_pmp_cpu_vote_stop(struct apple_pmp_cpu_vote *vote);
#else
static inline struct apple_pmp_cpu_vote *
apple_pmp_cpu_vote_probe(struct platform_device *pdev, void __iomem *events)
{
	return NULL;
}

static inline void apple_pmp_cpu_vote_start(struct apple_pmp_cpu_vote *vote)
{
}

static inline void apple_pmp_cpu_vote_stop(struct apple_pmp_cpu_vote *vote)
{
}
#endif
#endif
