// SPDX-License-Identifier: GPL-2.0-only
/* Native AP IOMFB packet layout measured on J813/T8142 firmware27.2.
 * This describes RPC data, without inspecting Apple executable contents.
 * The driver selects this internal-panel bring-up profile by exact metadata.
 */
#ifndef __APPLE_IOMFB_V27_2_T8142_H__
#define __APPLE_IOMFB_V27_2_T8142_H__

#include <linux/build_bug.h>
#include "version_utils.h"
#include "iomfb_v26_6.h"

#define DCP_FW v27_2_t8142
#define DCP_FW_VER DCP_FW_VERSION(27, 2, 0)
#define DCP_FW_T8142 1
#include "iomfb_template.h"
#undef DCP_FW_T8142
#undef DCP_FW_VER
#undef DCP_FW

static_assert(sizeof(struct dcp_swap_v27_2_t8142) == 0x6e0);
static_assert(offsetof(struct dcp_swap_v27_2_t8142, swap_id) == 0x98);
static_assert(offsetof(struct dcp_swap_v27_2_t8142, src_rect) == 0xac);
static_assert(offsetof(struct dcp_swap_v27_2_t8142, dst_rect) == 0x10c);
static_assert(offsetof(struct dcp_swap_v27_2_t8142, bl_update) == 0x354);
static_assert(offsetof(struct dcp_swap_v27_2_t8142, bl_nits) == 0x35e);
static_assert(offsetof(struct dcp_swap_v27_2_t8142, bl_limit_nits) == 0x376);
static_assert(offsetof(struct dcp_swap_v27_2_t8142, bl_scale) == 0x37e);
static_assert(offsetof(struct dcp_swap_v27_2_t8142, bl_scale_inverse) == 0x3e6);
static_assert(offsetof(struct dcp_swap_v27_2_t8142, presents_surface) == 0x52e);
static_assert(offsetof(struct dcp_swap_v27_2_t8142, has_completion) == 0x6c1);
static_assert(sizeof(struct dcp_surface_v27_2_t8142) == 0x22c);
static_assert(offsetof(struct dcp_swap_submit_req_v27_2_t8142, surf) == 0x6e0);
static_assert(offsetof(struct dcp_swap_submit_req_v27_2_t8142, surf_iova) == 0xf90);
static_assert(sizeof(struct dcp_swap_submit_req_v27_2_t8142) == 0x1d30);
static_assert(sizeof(struct dc_swap_complete_resp_v27_2_t8142) == 0x730);

static_assert(sizeof(struct dcp_allocate_buffer_resp_v27_2) == 20);
static_assert(offsetof(struct dcp_allocate_buffer_resp_v27_2, dva_size) == 8);
static_assert(offsetof(struct dcp_allocate_buffer_resp_v27_2, mem_desc_id) == 16);
static_assert(sizeof(struct dcp_allocate_buffer_resp) == 28);

#endif
