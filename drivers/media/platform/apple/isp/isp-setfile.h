/* SPDX-License-Identifier: GPL-2.0-only */

#ifndef __ISP_SETFILE_H__
#define __ISP_SETFILE_H__

#include <linux/types.h>

struct apple_isp;

int isp_setfile_prepare_lsc(struct apple_isp *isp, u32 chan, void *data,
			    size_t size);

#endif /* __ISP_SETFILE_H__ */
