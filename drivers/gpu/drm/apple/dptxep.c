// SPDX-License-Identifier: GPL-2.0-only OR MIT
/* Copyright 2022 Sven Peter <sven@svenpeter.dev> */

#include <linux/bitfield.h>
#include <linux/completion.h>
#include <linux/phy/phy.h>
#include <linux/delay.h>
#include <linux/moduleparam.h>
#include <linux/of.h>

#include "afk.h"
#include "dcp.h"
#include "dptxep.h"
#include "parser.h"
#include "trace.h"

/*
 * Diagnostic switch for separating the DCP-side DPTX lifecycle from AP PHY
 * MMIO.  The T8132 DPTX windows are inaccessible until DCP has raised its
 * internal PHY gate, so first establish that the callback transaction itself
 * completes before allowing the callbacks to touch the PHY.
 */
static bool dptx_bypass_phy;
module_param_named(dptx_bypass_phy, dptx_bypass_phy, bool, 0644);
MODULE_PARM_DESC(dptx_bypass_phy, "Acknowledge DPTX PHY callbacks without MMIO");

static unsigned int neo_max_link_rate = LINK_RATE_HBR2;
module_param(neo_max_link_rate, uint, 0644);
MODULE_PARM_DESC(neo_max_link_rate, "Neo rate-qualification limit (0x0a or 0x14)");

struct dcpdptx_connection_cmd {
	__le32 unk;
	__le32 target;
} __attribute__((packed));

struct dcpdptx_hotplug_cmd {
	u8 _pad0[16];
	__le32 unk;
} __attribute__((packed));

struct dptxport_apcall_link_rate {
	__le32 retcode;
	u8 _unk0[12];
	__le32 link_rate;
	u8 _unk1[12];
} __attribute__((packed));

struct dptxport_apcall_lane_count {
	__le32 retcode;
	u8 _unk0[12];
	__le64 lane_count;
	u8 _unk1[8];
} __attribute__((packed));

struct dptxport_apcall_set_active_lane_count {
	__le32 retcode;
	u8 _unk0[12];
	__le64 lane_count;
	u8 _unk1[8];
} __packed;

struct dptxport_apcall_get_support {
	__le32 retcode;
	u8 _unk0[12];
	__le32 supported;
	u8 _unk1[12];
} __attribute__((packed));

struct dptxport_apcall_max_drive_settings {
	__le32 retcode;
	u8 _unk0[12];
	__le32 max_drive_settings[2];
	u8 _unk1[8];
};

struct dptxport_apcall_drive_settings {
	__le32 retcode;
	u8 _unk0[12];
	__le32 unk1;
	__le32 unk2;
	__le32 unk3;
	__le32 unk4;
	__le32 unk5;
	__le32 unk6;
	__le32 unk7;
};

int dptxport_validate_connection(struct apple_epic_service *service, u8 core,
				 u8 atc, u8 die)
{
	struct dptx_port *dptx = service->cookie;
	struct dcpdptx_connection_cmd cmd, resp;
	int ret;
	u32 target = FIELD_PREP(DCPDPTX_REMOTE_PORT_CORE, core) |
		     FIELD_PREP(DCPDPTX_REMOTE_PORT_ATC, atc) |
		     FIELD_PREP(DCPDPTX_REMOTE_PORT_DIE, die) |
		     DCPDPTX_REMOTE_PORT_CONNECTED;

	trace_dptxport_validate_connection(dptx, core, atc, die);

	cmd.target = cpu_to_le32(target);
	cmd.unk = cpu_to_le32(0x100);
	ret = afk_service_call(service, 0, 12, &cmd, sizeof(cmd), 40, &resp,
			       sizeof(resp), 40);
	if (ret)
		return ret;

	if (le32_to_cpu(resp.target) != target)
		return -EINVAL;
	if (le32_to_cpu(resp.unk) != 0x100)
		return -EINVAL;

	return 0;
}

int dptxport_connect(struct apple_epic_service *service, u8 core, u8 atc,
		     u8 die)
{
	struct dptx_port *dptx = service->cookie;
	struct dcpdptx_connection_cmd cmd, resp;
	u32 unk_field = 0x0; // seen as 0x100 under some conditions
	int ret;
	u32 target = FIELD_PREP(DCPDPTX_REMOTE_PORT_CORE, core) |
		     FIELD_PREP(DCPDPTX_REMOTE_PORT_ATC, atc) |
		     FIELD_PREP(DCPDPTX_REMOTE_PORT_DIE, die) |
		     DCPDPTX_REMOTE_PORT_CONNECTED;

	trace_dptxport_connect(dptx, core, atc, die);

	cmd.target = cpu_to_le32(target);
	cmd.unk = cpu_to_le32(unk_field);
	ret = afk_service_call(service, 0, 11, &cmd, sizeof(cmd), 24, &resp,
			       sizeof(resp), 24);
	if (ret)
		return ret;

	if (le32_to_cpu(resp.target) != target)
		return -EINVAL;
	if (le32_to_cpu(resp.unk) != unk_field)
		dev_notice(service->ep->dcp->dev, "unexpected unk field in reply: 0x%x (0x%x)\n",
			  le32_to_cpu(resp.unk), unk_field);

	return 0;
}

int dptxport_request_display(struct apple_epic_service *service)
{
	return afk_service_call(service, 0, 6, NULL, 0, 16, NULL, 0, 16);
}

int dptxport_release_display(struct apple_epic_service *service)
{
	/* Neo's native disconnect uses group 8, command 7, with 16 zero bytes. */
	u16 group = service->ep->dcp->fw_compat == DCP_FIRMWARE_V_26_6_T8140 ?
		    8 : 0;

	return afk_service_call(service, group, 7, NULL, 0, 16, NULL, 0, 16);
}

int dptxport_set_hpd(struct apple_epic_service *service, bool hpd)
{
	struct dcpdptx_hotplug_cmd cmd, resp;
	int ret;

	memset(&cmd, 0, sizeof(cmd));

	if (hpd)
		cmd.unk = cpu_to_le32(1);

	ret = afk_service_call(service, 8, 8, &cmd, sizeof(cmd), 12, &resp,
			       sizeof(resp), 12);
	if (ret)
		return ret;
	if (le32_to_cpu(resp.unk) != hpd)
		return -EINVAL;
	return 0;
}

/* Native command 8:9 follows the controller's HPD IRQ indication. */
int dptxport_hpd_irq(struct apple_epic_service *service)
{
	u8 cmd[16] = {}, reply[16];

	return afk_service_call(service, 8, 9, cmd, sizeof(cmd), 16,
				reply, sizeof(reply), 16);
}

static int
dptxport_call_get_max_drive_settings(struct apple_epic_service *service,
				     void *reply_, size_t reply_size)
{
	struct dptxport_apcall_max_drive_settings *reply = reply_;

	if (reply_size < sizeof(*reply))
		return -EINVAL;

	reply->retcode = cpu_to_le32(0);
	reply->max_drive_settings[0] = cpu_to_le32(0x3);
	reply->max_drive_settings[1] = cpu_to_le32(0x3);

	return 0;
}

static int
dptxport_call_get_drive_settings(struct apple_epic_service *service,
				     const void *request_, size_t request_size,
				     void *reply_, size_t reply_size)
{
	struct dptx_port *dptx = service->cookie;
	const struct dptxport_apcall_drive_settings *request = request_;
	struct dptxport_apcall_drive_settings *reply = reply_;

	if (reply_size < sizeof(*reply) || request_size < sizeof(*request))
		return -EINVAL;

	*reply = *request;

	/* Clear the rest of the buffer */
	memset(reply_ + sizeof(*reply), 0, reply_size - sizeof(*reply));

	/*
	 * retcode appears to be lane count, seeing 2 for USB-C dp alt mode
	 * with lanes splitted for DP/USB3.
	 */
	if (le32_to_cpu(reply->retcode) != dptx->lane_count)
		dev_err(service->ep->dcp->dev,
			"get_drive_settings: unexpected retcode %d\n",
			reply->retcode);

	reply->retcode = cpu_to_le32(dptx->lane_count);
	reply->unk5 = cpu_to_le32(dptx->drive_settings[0]);
	reply->unk6 = cpu_to_le32(0);
	reply->unk7 = cpu_to_le32(dptx->drive_settings[1]);

	return 0;
}

static int
dptxport_call_set_drive_settings(struct apple_epic_service *service,
				     const void *request_, size_t request_size,
				     void *reply_, size_t reply_size)
{
	struct dptx_port *dptx = service->cookie;
	const struct dptxport_apcall_drive_settings *request = request_;
	struct dptxport_apcall_drive_settings *reply = reply_;

	if (reply_size < sizeof(*reply) || request_size < sizeof(*request))
		return -EINVAL;

	*reply = *request;
	reply->retcode = cpu_to_le32(0);

	dev_info(service->ep->dcp->dev, "set_drive_settings: %d:%d:%d:%d:%d:%d:%d\n",
		 request->unk1, request->unk2, request->unk3, request->unk4,
		 request->unk5, request->unk6, request->unk7);

	dptx->drive_settings[0] = le32_to_cpu(reply->unk5);
	dptx->drive_settings[1] = le32_to_cpu(reply->unk7);

	return 0;
}

static int dptxport_call_get_max_link_rate(struct apple_epic_service *service,
					   void *reply_, size_t reply_size)
{
	struct dptxport_apcall_link_rate *reply = reply_;

	if (reply_size < sizeof(*reply))
		return -EINVAL;

	reply->retcode = cpu_to_le32(0);
	if (service->ep->dcp->fw_compat == DCP_FIRMWARE_V_26_6_T8140) {
		if (neo_max_link_rate != LINK_RATE_HBR && neo_max_link_rate != LINK_RATE_HBR2)
			return -EINVAL;
		reply->link_rate = cpu_to_le32(neo_max_link_rate);
	} else {
		reply->link_rate = cpu_to_le32(LINK_RATE_HBR3);
	}

	return 0;
}

static int dptxport_call_get_max_lane_count(struct apple_epic_service *service,
					   void *reply_, size_t reply_size)
{
	struct dptxport_apcall_lane_count *reply = reply_;
	struct dptx_port *dptx = service->cookie;
	struct apple_dcp *dcp = service->ep->dcp;
	union phy_configure_opts phy_ops;
	int ret;

	if (reply_size < sizeof(*reply))
		return -EINVAL;

	if (dptx_bypass_phy) {
		dptx->lane_count = 4;
		reply->retcode = cpu_to_le32(0);
		reply->lane_count = cpu_to_le64(dptx->lane_count);
		return 0;
	}

	ret = phy_validate(dptx->atcphy, PHY_MODE_DP, 0, &phy_ops);
	if (ret < 0) {
		dev_err(dcp->dev, "phy_validate failed: %d\n", ret);
		reply->retcode = cpu_to_le32(1);
		reply->lane_count = cpu_to_le64(0);
	} else {
		/* M4 firmware queries before Type-C selects a DP assignment. */
		if (dcp->fw_compat != DCP_FIRMWARE_V_26_6_T8140 && phy_ops.dp.lanes < 2)
			dptx->lane_count = 4;
		else
			dptx->lane_count = phy_ops.dp.lanes;
		reply->retcode = cpu_to_le32(!dptx->lane_count);
		reply->lane_count = cpu_to_le64(dptx->lane_count);
	}

	return 0;
}

static int dptxport_call_set_active_lane_count(struct apple_epic_service *service,
					       const void *data, size_t data_size,
					       void *reply_, size_t reply_size)
{
	struct dptx_port *dptx = service->cookie;
	struct apple_dcp *dcp = service->ep->dcp;
	const struct dptxport_apcall_set_active_lane_count *request = data;
	struct dptxport_apcall_set_active_lane_count *reply = reply_;
	int ret = 0;
	int retcode = 0;

	if (reply_size < sizeof(*reply))
		return -1;
	if (data_size < sizeof(*request))
		return -1;

	u64 lane_count = le64_to_cpu(request->lane_count);

	if (dptx->lane_count < lane_count)
		dev_err(dcp->dev, "set_active_lane_count: unexpected lane "
			"count:%llu phy: %d\n", lane_count, dptx->lane_count);

	switch (lane_count) {
	case 0 ... 2:
	case 4:
		dptx->phy_ops.dp.lanes = lane_count;
		// Use dptx phy index > 3 as indication for dptx-phy or
		// lpdptx-phy and configure the number of lanes for those
		dptx->phy_ops.dp.set_lanes = (dcp->dptx_phy > 3);
		break;
	default:
		dev_err(dcp->dev, "set_active_lane_count: invalid lane count:%llu\n", lane_count);
		retcode = 1;
		lane_count = 0;
		break;
	}

	if (dptx->phy_ops.dp.set_lanes) {
		if (dptx->atcphy && !dptx_bypass_phy) {
			ret = phy_configure(dptx->atcphy, &dptx->phy_ops);
			if (ret)
				return ret;
		}
		dptx->phy_ops.dp.set_lanes = 0;
		dptx->lane_count = lane_count;
	}

	reply->retcode = cpu_to_le32(retcode);
	reply->lane_count = cpu_to_le64(lane_count);

	if (lane_count > 0)
		complete(&dptx->linkcfg_completion);

	return ret;
}

static int dptxport_call_get_link_rate(struct apple_epic_service *service,
				       void *reply_, size_t reply_size)
{
	struct dptx_port *dptx = service->cookie;
	struct dptxport_apcall_link_rate *reply = reply_;

	if (reply_size < sizeof(*reply))
		return -EINVAL;

	reply->retcode = cpu_to_le32(0);
	reply->link_rate = cpu_to_le32(dptx->link_rate);

	return 0;
}

static int
dptxport_call_will_change_link_config(struct apple_epic_service *service)
{
	struct dptx_port *dptx = service->cookie;

	dptx->phy_ops.dp.set_lanes = 0;
	dptx->phy_ops.dp.set_rate = 0;
	dptx->phy_ops.dp.set_voltages = 0;
	dptx->neo_drive_valid = false;

	return 0;
}

static int
dptxport_call_did_change_link_config(struct apple_epic_service *service)
{
	/* assume the link config did change and wait a little bit */
	mdelay(10);

	return 0;
}

static int dptxport_call_set_link_rate(struct apple_epic_service *service,
				       const void *data, size_t data_size,
				       void *reply_, size_t reply_size)
{
	struct dptx_port *dptx = service->cookie;
	const struct dptxport_apcall_link_rate *request = data;
	struct dptxport_apcall_link_rate *reply = reply_;
	u32 link_rate, phy_link_rate;
	bool phy_set_rate = false;
	int ret;

	if (reply_size < sizeof(*reply))
		return -EINVAL;
	if (data_size < sizeof(*request))
		return -EINVAL;

	link_rate = le32_to_cpu(request->link_rate);
	trace_dptxport_call_set_link_rate(dptx, link_rate);

	switch (link_rate) {
	case LINK_RATE_RBR:
		phy_link_rate = 1620;
		phy_set_rate = true;
		break;
	case LINK_RATE_HBR:
		phy_link_rate = 2700;
		phy_set_rate = true;
		break;
	case LINK_RATE_HBR2:
		phy_link_rate = 5400;
		phy_set_rate = true;
		break;
	case LINK_RATE_HBR3:
		phy_link_rate = 8100;
		phy_set_rate = true;
		break;
	case 0:
		phy_link_rate = 0;
		phy_set_rate = true;
		break;
	default:
		dev_err(service->ep->dcp->dev,
			"DPTXPort: Unsupported link rate 0x%x requested\n",
			link_rate);
		link_rate = 0;
		phy_set_rate = false;
		break;
	}

	if (phy_set_rate) {
		dptx->phy_ops.dp.link_rate = phy_link_rate;
		dptx->phy_ops.dp.set_rate = 1;

		if (dptx->atcphy && !dptx_bypass_phy) {
			ret = phy_configure(dptx->atcphy, &dptx->phy_ops);
			if (ret)
				return ret;
		}

		//if (dptx->phy_ops.dp.set_rate)
		dptx->link_rate = dptx->pending_link_rate = link_rate;

	}

	//dptx->pending_link_rate = link_rate;
	reply->retcode = cpu_to_le32(0);
	reply->link_rate = cpu_to_le32(link_rate);

	return 0;
}

static int dptxport_call_get_supports_hpd(struct apple_epic_service *service,
					  void *reply_, size_t reply_size)
{
	struct dptxport_apcall_get_support *reply = reply_;

	if (reply_size < sizeof(*reply))
		return -EINVAL;

	reply->retcode = cpu_to_le32(0);
	reply->supported = cpu_to_le32(of_property_present(service->ep->dcp->dev->of_node,
							 "apple,typec-connector"));
	return 0;
}

static int
dptxport_call_get_supports_downspread(struct apple_epic_service *service,
				      void *reply_, size_t reply_size)
{
	struct dptxport_apcall_get_support *reply = reply_;

	if (reply_size < sizeof(*reply))
		return -EINVAL;

	reply->retcode = cpu_to_le32(0);
	reply->supported = cpu_to_le32(0);
	return 0;
}

static int
dptxport_call_activate(struct apple_epic_service *service,
		       const void *data, size_t data_size,
		       void *reply, size_t reply_size)
{
	struct dptx_port *dptx = service->cookie;
	const struct apple_dcp *dcp = service->ep->dcp;
	int ret = 0;

	// TODO: hack, use phy_set_mode to select the correct DCP(EXT) input
	// for standalone phy (i.e. not atc phy).
	dev_info(dcp->dev, "DPTXPort: activating PHY\n");
	if (!dcp->typec_mux && !dptx_bypass_phy) {
		ret = phy_set_mode_ext(dptx->atcphy, PHY_MODE_DP,
				       dcp->dptx_phy > 3 ? dcp->index : 0);
		if (ret)
			dev_err(dcp->dev, "DPTXPort: PHY activation failed: %d\n",
				ret);
	}
	dev_info(dcp->dev, "DPTXPort: PHY activation returned: %d\n", ret);

	memcpy(reply, data, min(reply_size, data_size));
	if (reply_size >= 4)
		memset(reply, 0, 4);

	return ret;
}

static int
dptxport_call_deactivate(struct apple_epic_service *service,
		       const void *data, size_t data_size,
		       void *reply, size_t reply_size)
{
	struct dptx_port *dptx = service->cookie;

	/* deactivate phy */
	if (!dptx_bypass_phy)
		phy_set_mode_ext(dptx->atcphy, PHY_MODE_INVALID, 0);

	memcpy(reply, data, min(reply_size, data_size));
	if (reply_size >= 4)
		memset(reply, 0, 4);

	return 0;
}

/*
 * Compact firmware adds a second DPTX callback group whose command numbers
 * overlap the original link/PHY callbacks. Commands 3 and 4 carry an
 * 80-byte record. Preserve the traced input prefix, clear the result word,
 * and never echo the pointer-bearing tail back to firmware.
 */
/* Neo 26.6 carries a 32-byte envelope followed by per-lane triples.
 * Native request/MMIO pairs establish words 1/2 as swing/pre-emphasis.
 * Word 0 was only observed as zero; refuse other values without guessing. */
static int dptxport_call_neo_drive(struct apple_epic_service *service, u32 idx,
				 const void *data, size_t data_size,
				 void *reply, size_t reply_size)
{
	struct dptx_port *dptx = service->cookie;
	const __le32 *request = data;
	__le32 *response = reply;
	union phy_configure_opts opts = { }, limits = { };
	u32 lanes, setting[4][3];
	size_t size;
	int ret;

	if ((idx != 3 && idx != 4) || data_size < 32 || reply_size < 32)
		return -EINVAL;
	lanes = le32_to_cpu(request[idx == 3 ? 4 : 0]);
	if (!lanes || lanes == 3 || lanes > ARRAY_SIZE(setting))
		return -EINVAL;
	size = 32 + lanes * 12;
	if (data_size != size || reply_size < size || !dptx->atcphy)
		return -EINVAL;
	if (idx == 4) {
		if (!dptx->neo_drive_valid || lanes != dptx->phy_ops.dp.lanes)
			return -EAGAIN;
		memcpy(reply, data, 16);
		memset(reply + 16, 0, reply_size - 16);
		for (u32 i = 0; i < lanes; i++)
			for (u32 j = 0; j < 3; j++)
				response[8 + i * 3 + j] = cpu_to_le32(dptx->neo_drive[i][j]);
		return 0;
	}
	if (lanes != dptx->phy_ops.dp.lanes)
		return -EINVAL;
	ret = phy_validate(dptx->atcphy, PHY_MODE_DP, 0, &limits);
	if (ret || lanes > limits.dp.lanes)
		return ret ? ret : -EINVAL;
	opts.dp.link_rate = dptx->phy_ops.dp.link_rate;
	opts.dp.lanes = lanes;
	opts.dp.set_voltages = 1;
	for (u32 i = 0; i < lanes; i++) {
		for (u32 j = 0; j < 3; j++)
			setting[i][j] = le32_to_cpu(request[8 + i * 3 + j]);
		if (setting[i][0] || setting[i][1] > 3 || setting[i][2] > 3 ||
		    setting[i][1] + setting[i][2] > 3)
			return -EINVAL;
		opts.dp.voltage[i] = setting[i][1];
		opts.dp.pre[i] = setting[i][2];
	}
	memcpy(reply, data, size);
	if (reply_size > size)
		memset(reply + size, 0, reply_size - size);
	ret = phy_configure(dptx->atcphy, &opts);
	response[0] = cpu_to_le32(ret ? 1 : 0);
	if (ret) {
		dev_warn(service->ep->dcp->dev, "Neo drive configuration failed: %d\n", ret);
		return 0;
	}
	memcpy(dptx->neo_drive, setting, lanes * sizeof(setting[0]));
	dptx->neo_drive_valid = true;
	return 0;
}

static int dptxport_call_group1(u32 idx, const void *data, size_t data_size,
				void *reply, size_t reply_size)
{
	size_t preserved = min_t(size_t, 32, min(data_size, reply_size));
	size_t result_offset;

	if (idx != 3 && idx != 4)
		return -ENOSYS;

	memcpy(reply, data, preserved);
	if (reply_size > preserved)
		memset(reply + preserved, 0, reply_size - preserved);

	result_offset = idx == 3 ? 0 : 16;
	if (reply_size < result_offset + sizeof(__le32))
		return -EINVAL;
	memset(reply + result_offset, 0, sizeof(__le32));

	return 0;
}

static int dptxport_call(struct apple_epic_service *service, u16 group, u32 idx,
			 const void *data, size_t data_size, void *reply,
			 size_t reply_size)
{
	struct dptx_port *dptx = service->cookie;

	if (group == 1 && of_device_is_compatible(service->ep->dcp->dev->of_node,
						 "apple,t8140-dcpext"))
		return dptxport_call_neo_drive(service, idx, data, data_size, reply, reply_size);
	if (group == 1)
		return dptxport_call_group1(idx, data, data_size, reply,
					    reply_size);
	if (group)
		return -ENOSYS;

	trace_dptxport_apcall(dptx, idx, data_size);

	switch (idx) {
	case DPTX_APCALL_WILL_CHANGE_LINKG_CONFIG:
		return dptxport_call_will_change_link_config(service);
	case DPTX_APCALL_DID_CHANGE_LINK_CONFIG:
		return dptxport_call_did_change_link_config(service);
	case DPTX_APCALL_GET_MAX_LINK_RATE:
		return dptxport_call_get_max_link_rate(service, reply,
						       reply_size);
	case DPTX_APCALL_GET_LINK_RATE:
		return dptxport_call_get_link_rate(service, reply, reply_size);
	case DPTX_APCALL_SET_LINK_RATE:
		return dptxport_call_set_link_rate(service, data, data_size,
						   reply, reply_size);
	case DPTX_APCALL_GET_MAX_LANE_COUNT:
		return dptxport_call_get_max_lane_count(service, reply, reply_size);
        case DPTX_APCALL_SET_ACTIVE_LANE_COUNT:
		return dptxport_call_set_active_lane_count(service, data, data_size,
							   reply, reply_size);
	case DPTX_APCALL_GET_SUPPORTS_HPD:
		return dptxport_call_get_supports_hpd(service, reply,
						      reply_size);
	case DPTX_APCALL_GET_SUPPORTS_DOWN_SPREAD:
		return dptxport_call_get_supports_downspread(service, reply,
							     reply_size);
	case DPTX_APCALL_GET_MAX_DRIVE_SETTINGS:
		return dptxport_call_get_max_drive_settings(service, reply,
							    reply_size);
	case DPTX_APCALL_GET_DRIVE_SETTINGS:
		return dptxport_call_get_drive_settings(service, data, data_size,
							reply, reply_size);
	case DPTX_APCALL_SET_DRIVE_SETTINGS:
		return dptxport_call_set_drive_settings(service, data, data_size,
							reply, reply_size);
        case DPTX_APCALL_ACTIVATE:
		return dptxport_call_activate(service, data, data_size,
					      reply, reply_size);
	case DPTX_APCALL_DEACTIVATE:
		return dptxport_call_deactivate(service, data, data_size,
						reply, reply_size);
	default:
		/* just try to ACK and hope for the best... */
		dev_info(service->ep->dcp->dev, "DPTXPort: acking unhandled call %u\n",
			idx);
		fallthrough;
	case DPTX_APCALL_GET_DOWN_SPREAD:
	case DPTX_APCALL_SET_DOWN_SPREAD:
		memcpy(reply, data, min(reply_size, data_size));
		if (reply_size >= 4)
			memset(reply, 0, 4);
		return 0;
	}
}

static void dptxport_init(struct apple_epic_service *service, const char *name,
			  const char *class, s64 unit)

{
	dev_info(service->ep->dcp->dev,
		 "DPTX init name=%s class=%s unit=%lld compact=%d\n",
		 name, class, unit, service->ep->compact);
	if (service->ep->compact) {
		if (unit < 0)
			return;
		if (unit == 0)
			service->defer_start = true;
	} else {
		if (strcmp(name, "dcpdptx-port-epic"))
			return;
		if (strcmp(class, "AppleDCPDPTXRemotePort"))
			return;
	}

	trace_dptxport_init(service->ep->dcp, unit);

	switch (unit) {
	case 0:
	case 1:
		if (service->ep->dcp->dptxport[unit].enabled) {
			dev_err(service->ep->dcp->dev,
				"DPTXPort: unit %lld already exists\n", unit);
			return;
		}
		service->ep->dcp->dptxport[unit].unit = unit;
		service->ep->dcp->dptxport[unit].service = service;
		service->ep->dcp->dptxport[unit].enabled = true;
		service->cookie = (void *)&service->ep->dcp->dptxport[unit];
		complete(&service->ep->dcp->dptxport[unit].enable_completion);
		break;
	default:
		dev_err(service->ep->dcp->dev, "DPTXPort: invalid unit %lld\n",
			unit);
	}
}

static const struct apple_epic_service_ops dptxep_ops[] = {
	{
		.name = "AppleDCPDPTXRemotePort",
		.init = dptxport_init,
		.call = dptxport_call,
	},
	{
		.name = "dcpdptx-port-epic",
		.init = dptxport_init,
		.call = dptxport_call,
	},
	{}
};

int dptxep_init(struct apple_dcp *dcp)
{
	int ret;
	u32 port;
	unsigned long timeout = msecs_to_jiffies(1000);

	init_completion(&dcp->dptxport[0].enable_completion);
	init_completion(&dcp->dptxport[1].enable_completion);
	init_completion(&dcp->dptxport[0].linkcfg_completion);
	init_completion(&dcp->dptxport[1].linkcfg_completion);

	dcp->dptxep = afk_init(dcp,
			dcp->fw_compat == DCP_FIRMWARE_V_26_6_T8140 ?
			EPIC2C_ENDPOINT : DPTX_ENDPOINT, dptxep_ops);
	if (IS_ERR(dcp->dptxep))
		return PTR_ERR(dcp->dptxep);

	ret = afk_start(dcp->dptxep);
	if (ret)
		return ret;

	dev_info(dcp->dev, "DPTX waiting for %u port services\n", dcp->hw.num_dptx_ports);
	for (port = 0; port < dcp->hw.num_dptx_ports; port++) {
		ret = wait_for_completion_timeout(&dcp->dptxport[port].enable_completion,
						timeout);
		if (!ret)
			return -ETIMEDOUT;
		else if (ret < 0)
			return ret;
		timeout = ret;
	}

	if (dcp->dptxep->compact && dcp->dptxport[0].service) {
		ret = afk_start_service(dcp->dptxport[0].service);
		if (ret)
			return ret;
	}

	return 0;
}
