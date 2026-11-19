/* SPDX-License-Identifier: GPL-2.0-only OR MIT */
#ifndef _LINUX_APPLE_DART_H_
#define _LINUX_APPLE_DART_H_
#include <linux/types.h>
struct device;
/* Caller holds client runtime power and has stopped every DMA issuer. */
int apple_dart_retained_streams(struct device *dev, bool enable);
int apple_dart_retained_state(struct device *dev, u32 *enabled);
#endif
