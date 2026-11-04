// SPDX-License-Identifier: GPL-2.0-only
/* Copyright The Asahi Linux Contributors
 * J813/T8142 27.2 AP IOMFB protocol integration. Reuse the existing Gravity/
 * Asahi implementation; offsets and wire tags come from bounded runtime
 * control observations, without inspecting Apple executable contents.
 * Internal-panel bring-up; external links and suspend are not qualified.
 */
#include <linux/build_bug.h>
#include "iomfb_v12_3.h"
#include "iomfb_v13_3.h"
#include "iomfb_v14_7.h"
#include "iomfb_v26_6.h"
#include "iomfb_v26_6_t8140.h"
#include "iomfb_v27_2_t8142.h"
#include "version_utils.h"

/* Scalar contracts and initialization order are recorded in dcp-macos-04.
 * The main frame methods are additionally correlated by native swap IDs.
 * Client-close/abort entries remain unset: the modern internal-panel power
 * path does not call those unqualified legacy methods. */
static const struct dcp_method_entry dcp_methods[dcpep_num_methods] = {
	IOMFB_METHOD("A000", dcpep_late_init_signal),
	IOMFB_METHOD("A027", dcpep_setup_video_limits),
	IOMFB_METHOD("A136", iomfbep_a131_pmu_service_matched),
	IOMFB_METHOD("A137", iomfbep_a132_backlight_service_matched),
	IOMFB_METHOD("A390", dcpep_set_create_dfb),
	IOMFB_METHOD("A391", iomfbep_a358_vi_set_temperature_hint),
	IOMFB_METHOD("A400", dcpep_start_signal),
	IOMFB_METHOD("A456", dcpep_swap_start),
	IOMFB_METHOD("A457", dcpep_swap_submit),
	IOMFB_METHOD("A461", dcpep_set_display_device),
	IOMFB_METHOD("A462", dcpep_is_main_display),
	IOMFB_METHOD("A473", iomfbep_set_matrix),
	IOMFB_METHOD("A477", iomfbep_get_color_remap_mode),
	IOMFB_METHOD("A500", dcpep_set_parameter_dcp),
	IOMFB_METHOD("A504", dcpep_create_default_fb),
	IOMFB_METHOD("A508", dcpep_enable_disable_video_power_savings),
	IOMFB_METHOD("A451", dcpep_first_client_open),
	IOMFB_METHOD("A525", dcpep_set_display_refresh_properties),
	IOMFB_METHOD("A528", dcpep_flush_supports_power),
	IOMFB_METHOD("A534", dcpep_set_power_state),
};

#define DCP_FW v27_2_t8142
#define DCP_FW_VER DCP_FW_VERSION(27, 2, 0)
#define DCP_FW_T8142 1
#include "iomfb_template.c"

/* D003 is the same compact20-byte response used by Neo, but J813 advertises
 * no doorbell. Keep its resource in DT and return only the scratch address. */
struct dcp_rt_bandwidth_t8142 {
	u64 scratch;
	u64 doorbell;
	u32 bit;
} __packed;
static_assert(sizeof(struct dcp_rt_bandwidth_t8142) == 20);

static bool trampoline_rt_bandwidth_t8142(struct apple_dcp *dcp, int tag,
					void *out, void *in)
{
	struct dcp_rt_bandwidth_t8142 *response = out;

	memset(response, 0, sizeof(*response));
	if (!dcp_disable_rt_bandwidth)
		response->scratch = dcp->disp_registers[dcp->disp_bw_scratch_index]->start +
				    dcp->disp_bw_scratch_offset;
	return true;
}

static bool trampoline_power_up_dart_t8142(struct apple_dcp *dcp, int tag,
					  void *out, void *in)
{
	put_unaligned_le64(0, out);
	return true;
}

static const iomfb_cb_handler cb_handlers[IOMFB_MAX_CB] = {
	[0] = trampoline_true,
	[1] = trampoline_true,
	[2] = trampoline_nop, /* will_power_off_signal, 0/0 notification */
	[3] = trampoline_rt_bandwidth_t8142,
	[6] = trampoline_set_frame_sync_props,
	[100] = iomfbep_cb_match_pmu_service,
	[101] = trampoline_zero,
	[102] = trampoline_nop,
	[104] = trampoline_nop,
	[106] = trampoline_nop, /* removeProperty: unmodeled AP property */
	[107] = trampoline_true,
	[108] = trampoline_true,
	[109] = trampoline_false,
	[110] = trampoline_true,
	[111] = trampoline_false,
	[112] = trampoline_create_backlight_service,
	[116] = trampoline_nop,
	[120] = dcpep_cb_boot_1,
	[121] = trampoline_false, /* is_dark_boot: development Linux boot */
	[122] = trampoline_false,
	[124] = trampoline_read_edt_data,
	[126] = trampoline_prop_start,
	[127] = trampoline_prop_chunk,
	[128] = trampoline_prop_end,
	[201] = trampoline_map_piodma,
	[204] = trampoline_true,
	[206] = iomfbep_cb_match_pmu_service_2,
	[207] = iomfbep_cb_match_backlight_service,
	[208] = trampoline_nop,
	[209] = trampoline_get_time,
	[300] = trampoline_pr_publish,
	[400] = trampoline_nop,
	[401] = trampoline_get_uint_prop,
	[405] = trampoline_nop,
	[406] = trampoline_nop,
	[408] = trampoline_get_frequency,
	[413] = trampoline_true,
	[414] = trampoline_sr_set_property_int,
	[415] = trampoline_true,
	[451] = trampoline_allocate_buffer,
	[552] = trampoline_true,
	[561] = trampoline_true,
	[563] = trampoline_true,
	[565] = trampoline_true,
	[572] = trampoline_zero, /* start/stop event: zero status */
	[574] = trampoline_power_up_dart_t8142,
	[575] = trampoline_hotplug,
	[576] = trampoline_nop, /* power-path notification, 4/0 */
	[579] = trampoline_true,
	[580] = trampoline_nop,
	/*12-byte swap-associated notification; its flags are still unknown. */
	[582] = trampoline_nop,
	[586] = trampoline_true,
	[594] = trampoline_swap_complete,
	[596] = trampoline_swap_complete_intent_gated,
	[598] = trampoline_nop, /* display re-enable notification, 4/0 */
	[603] = trampoline_nop,
};

static void start_after_first_client(struct apple_dcp *dcp, void *out,
                                     void *cookie)
{
	dcp_start_signal(dcp, false, dcp_started, NULL);
}

void DCP_FW_NAME(iomfb_start)(struct apple_dcp *dcp)
{
	dcp->cb_handlers = cb_handlers;
	/* Native27.2 opens the client before A400. Its open reply is deferred
	 * until D120 and the nested boot/property callbacks have completed. */
	dcp_first_client_open(dcp, false, start_after_first_client, NULL);
}
#undef DCP_FW_T8142
#undef DCP_FW_VER
#undef DCP_FW
