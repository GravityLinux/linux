// SPDX-License-Identifier: GPL-2.0-only OR MIT
/* Copyright The Asahi Linux Contributors */

#include <linux/build_bug.h>

#include "iomfb_v12_3.h"
#include "iomfb_v13_3.h"
#include "iomfb_v26_6.h"
#include "version_utils.h"

static const struct dcp_method_entry dcp_methods[dcpep_num_methods] = {
	IOMFB_METHOD("A000", dcpep_late_init_signal),
	IOMFB_METHOD("A029", dcpep_setup_video_limits),
	IOMFB_METHOD("A131", iomfbep_a131_pmu_service_matched),
	IOMFB_METHOD("A132", iomfbep_a132_backlight_service_matched),
	IOMFB_METHOD("A385", dcpep_set_create_dfb),
	IOMFB_METHOD("A386", iomfbep_a358_vi_set_temperature_hint),
	IOMFB_METHOD("A401", dcpep_start_signal),
	IOMFB_METHOD("A406", dcpep_swap_start),
	IOMFB_METHOD("A407", dcpep_swap_submit),
	IOMFB_METHOD("A411", dcpep_set_display_device),
	IOMFB_METHOD("A412", dcpep_is_main_display),
	IOMFB_METHOD("A413", dcpep_set_digital_out_mode),
	IOMFB_METHOD("A423", iomfbep_set_matrix),
	IOMFB_METHOD("A427", iomfbep_get_color_remap_mode),
	IOMFB_METHOD("A442", dcpep_set_parameter_dcp),
	IOMFB_METHOD("A446", dcpep_create_default_fb),
	IOMFB_METHOD("A450", dcpep_enable_disable_video_power_savings),
	IOMFB_METHOD("A457", dcpep_first_client_open),
	IOMFB_METHOD("A458", iomfbep_last_client_close),
	IOMFB_METHOD("A465", dcpep_set_display_refresh_properties),
	IOMFB_METHOD("A468", dcpep_flush_supports_power),
	IOMFB_METHOD("A469", iomfbep_abort_swaps_dcp),
	IOMFB_METHOD("A473", dcpep_set_power_state),
};

#define DCP_FW v26_6_0
#define DCP_FW_VER DCP_FW_VERSION(26, 6, 0)

#include "iomfb_template.c"

struct dcp_dict_passthrough_req_v26_6 {
	u8 key[0x40];
	u8 value[0x1000];
	u8 value_null;
	u8 padding[3];
} __packed;

struct dcp_dict_passthrough_resp_v26_6 {
	u8 value[0x1000];
	u8 ret;
	u8 padding[3];
} __packed;

/*
 * D114 periodically publishes dictionary-valued DCP statistics.  The value
 * is an in/out pointer: native macOS returns the complete serialized buffer
 * byte-for-byte and reports false.  Returning an all-zero output is not a
 * harmless false result because DCP deserializes the in/out value first.
 */
static bool trampoline_passthrough_dict_v26_6(struct apple_dcp *dcp, int tag,
					       void *out, void *in)
{
	const struct dcp_dict_passthrough_req_v26_6 *req = in;
	struct dcp_dict_passthrough_resp_v26_6 *resp = out;

	trace_iomfb_callback(dcp, tag, "dcpep_cb_passthrough_dict");

	if (!req->value_null)
		memcpy(resp->value, req->value, sizeof(resp->value));

	resp->ret = false;
	memset(resp->padding, 0, sizeof(resp->padding));

	return true;
}

static_assert(sizeof(struct dcp_swap_v26_6_0) == 0x588);
static_assert(offsetof(struct dcp_swap_v26_6_0, bl_update) == 0x354);
static_assert(offsetof(struct dcp_swap_v26_6_0, bl_flags) == 0x358);
static_assert(offsetof(struct dcp_swap_v26_6_0, bl_nits) == 0x35e);
static_assert(sizeof(struct dcp_swap_start_req_v26_6_0) == 0x10);
static_assert(sizeof(struct dcp_swap_start_resp_v26_6_0) == 0x8);
static_assert(sizeof(struct dcp_swap_submit_req_v26_6_0) == 0x1bd8);
static_assert(sizeof(struct dcp_swap_submit_resp_v26_6_0) == 0xc);
static_assert(sizeof(struct dc_swap_complete_resp_v26_6_0) == 0x730);
static_assert(sizeof(struct dcp_map_buf_resp_v26_6_0) == 0x10);
static_assert(sizeof(struct dcp_set_power_state_req_v26_6) == 0x10);
static_assert(sizeof(struct dcp_set_parameter_dcp) == 0x28);
static_assert(sizeof(struct dcp_get_mode_info_req_v26_6) == 0x7c);
static_assert(sizeof(struct dcp_get_mode_info_resp_v26_6) == 0x74);
static_assert(sizeof(struct iomfb_abort_swaps_dcp_req_v26_6) == 0x8);
static_assert(sizeof(struct iomfb_abort_swaps_dcp_resp_v26_6) == 0x4);
static_assert(sizeof(struct dcp_swap_complete_intent_gated_v26_6) == 0x14);
static_assert(sizeof(struct dcp_hotplug_req_v26_6) == 0x58);
static_assert(sizeof(struct dcp_hotplug_resp_v26_6) == 0x4c);
static_assert(sizeof(struct iomfb_property) == 0x10);
static_assert(sizeof(struct dcp_read_edt_data_resp_v26_6) == 0x24);
static_assert(sizeof(struct dcp_bool_arg) == 0x4);
static_assert(sizeof(struct dcp_set_frame_sync_props_req_v26_6_0) == 0x54);
static_assert(sizeof(struct dcp_set_frame_sync_props_resp_v26_6_0) == 0x50);
static_assert(sizeof(struct dcp_dict_passthrough_req_v26_6) == 0x1044);
static_assert(sizeof(struct dcp_dict_passthrough_resp_v26_6) == 0x1004);

static const iomfb_cb_handler cb_handlers[IOMFB_MAX_CB] = {
	[0] = trampoline_true,
	[1] = trampoline_true,
	[2] = trampoline_nop,
	[3] = trampoline_rt_bandwidth,
	[6] = trampoline_set_frame_sync_props,
	[100] = iomfbep_cb_match_pmu_service,
	[101] = trampoline_zero,
	[102] = trampoline_nop,
	[104] = trampoline_nop,
	[107] = trampoline_nop,
	[108] = trampoline_true,
	[109] = trampoline_true,
	[110] = trampoline_true,
	[111] = trampoline_true,
	[112] = trampoline_create_backlight_service,
	[113] = trampoline_true,
	[114] = trampoline_passthrough_dict_v26_6,
	/* set_tiling_state: 26.6 returns the value followed by success. */
	[115] = trampoline_get_tiling_state,
	[121] = dcpep_cb_boot_1,
	[122] = trampoline_false, /* native display sleep/wake query */
	[123] = trampoline_false,
	[125] = trampoline_read_edt_data,
	[127] = trampoline_prop_start,
	[128] = trampoline_prop_chunk,
	[129] = trampoline_prop_end,
	[134] = trampoline_zero,
	[201] = trampoline_map_piodma,
	[206] = iomfbep_cb_match_pmu_service_2,
	[207] = iomfbep_cb_match_backlight_service,
	[208] = trampoline_nop,
	[209] = trampoline_get_time,
	[300] = trampoline_pr_publish,
	[400] = trampoline_nop,
	[401] = trampoline_get_uint_prop,
	[406] = trampoline_nop, /* set_fx_prop: host property notification */
	[408] = trampoline_get_frequency,
	[411] = trampoline_map_reg,
	[413] = trampoline_true,
	[414] = trampoline_sr_set_property_int,
	[415] = trampoline_true,
	[451] = trampoline_allocate_buffer,
	[552] = trampoline_true,
	[561] = trampoline_true,
	[563] = trampoline_true,
	[565] = trampoline_true,
	[567] = trampoline_true,
	/* Native 26.6 returns a zero status for both start and stop events. */
	[572] = trampoline_zero,
	[575] = trampoline_hotplug,
	[576] = trampoline_nop,
	[581] = trampoline_nop,
	[582] = trampoline_true, /* create_default_fb_surface */
	[590] = trampoline_swap_complete,
	[592] = trampoline_swap_complete_intent_gated,
	[594] = trampoline_nop, /* native display re-enable notification: empty ACK */
	[595] = trampoline_nop,
	[598] = trampoline_nop,
	[599] = trampoline_nop,
};

void DCP_FW_NAME(iomfb_start)(struct apple_dcp *dcp)
{
	dcp->cb_handlers = cb_handlers;
	dcp_start_signal(dcp, false, dcp_started, NULL);
}

#undef DCP_FW_VER
#undef DCP_FW
