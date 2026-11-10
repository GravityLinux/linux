// SPDX-License-Identifier: GPL-2.0-only
/* Copyright The Asahi Linux Contributors */
#ifndef __APPLE_IOMFB_V26_6_T8140_H__
#define __APPLE_IOMFB_V26_6_T8140_H__
#include "version_utils.h"
#include "iomfb_v26_6.h"
#define DCP_FW v26_6_t8140
#define DCP_FW_VER DCP_FW_VERSION(26, 6, 0)
/* T8140 omits the auxiliary surface array from the A408 argument list. */
#define DCP_FW_T8140 1
#include "iomfb_template.h"
#undef DCP_FW_T8140
#undef DCP_FW_VER
#undef DCP_FW
#endif
