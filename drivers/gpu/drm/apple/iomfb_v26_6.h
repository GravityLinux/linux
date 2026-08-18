// SPDX-License-Identifier: GPL-2.0-only OR MIT
/* Copyright The Asahi Linux Contributors */

#ifndef __APPLE_IOMFB_V26_6_H__
#define __APPLE_IOMFB_V26_6_H__

#include "version_utils.h"

#define DCP_FW v26_6_0
#define DCP_FW_VER DCP_FW_VERSION(26, 6, 0)

#include "iomfb_template.h"

#undef DCP_FW_VER
#undef DCP_FW

/* D006 set_frame_sync_props grew from 32/28 bytes to 84/80 bytes in 26.6.
 * Native copies the first 80 request bytes into the response and clears the
 * three flag bytes at offsets 73..75. */
struct dcp_set_frame_sync_props_req_v26_6_0 {
	u8 props[0x50];
	u8 opaque[0x4];
} __packed;

struct dcp_set_frame_sync_props_resp_v26_6_0 {
	u8 props[0x50];
} __packed;

#endif /* __APPLE_IOMFB_V26_6_H__ */
