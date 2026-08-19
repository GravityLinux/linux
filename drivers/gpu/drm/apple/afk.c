// SPDX-License-Identifier: GPL-2.0-only OR MIT
/* Copyright 2022 Sven Peter <sven@svenpeter.dev> */

#include <linux/bitfield.h>
#include <linux/debugfs.h>
#include <linux/dma-mapping.h>
#include <linux/kconfig.h>
#include <linux/moduleparam.h>
#include <linux/of_platform.h>
#include <linux/slab.h>
#include <linux/workqueue.h>
#include <linux/soc/apple/rtkit.h>

#include "afk.h"
#include "trace.h"

static bool compact_trace;
module_param_named(compact_trace, compact_trace, bool, 0644);
MODULE_PARM_DESC(compact_trace,
		 "Trace compact EPIC calls and reports for DCP protocol debugging");

struct afk_receive_message_work {
	struct apple_dcp_afkep *ep;
	u64 message;
	struct work_struct work;
};

struct afk_compact_cmd {
	struct list_head node;
	struct completion done;
	u16 channel;
	u8 type;
	bool service_call;
	u16 group;
	u32 command;
	u32 request_status;
	void *output;
	size_t output_len;
	u32 retcode;
	int status;
};

static u32 afk_compact_next_status(struct apple_epic_service *service)
{
	unsigned long flags;
	u32 status;

	spin_lock_irqsave(&service->lock, flags);
	status = (u32)service->cmd_tag++ << 8;
	spin_unlock_irqrestore(&service->lock, flags);

	return status;
}

#define RBEP_TYPE GENMASK(63, 48)

enum rbep_msg_type {
	RBEP_INIT = 0x80,
	RBEP_INIT_ACK = 0xa0,
	RBEP_GETBUF = 0x89,
	RBEP_GETBUF_ACK = 0xa1,
	RBEP_INIT_TX = 0x8a,
	RBEP_INIT_RX = 0x8b,
	RBEP_START = 0xa3,
	RBEP_START_ACK = 0x86,
	RBEP_SEND = 0xa2,
	RBEP_RECV = 0x85,
	RBEP_SHUTDOWN = 0xc0,
	RBEP_SHUTDOWN_ACK = 0xc1,
};

#define BLOCK_SHIFT 6

#define GETBUF_SIZE GENMASK(31, 16)
#define GETBUF_TAG GENMASK(15, 0)
#define GETBUF_ACK_DVA GENMASK(47, 0)

#define INITRB_OFFSET GENMASK(47, 32)
#define INITRB_SIZE GENMASK(31, 16)
#define INITRB_TAG GENMASK(15, 0)

#define SEND_WPTR GENMASK(31, 0)

static void afk_send(struct apple_dcp_afkep *ep, u64 message)
{
	dcp_send_message(ep->dcp, ep->endpoint, message);
}

struct apple_dcp_afkep *afk_init(struct apple_dcp *dcp, u32 endpoint,
				 const struct apple_epic_service_ops *ops)
{
	struct apple_dcp_afkep *afkep;
	int ret;

	afkep = devm_kzalloc(dcp->dev, sizeof(*afkep), GFP_KERNEL);
	if (!afkep)
		return ERR_PTR(-ENOMEM);

	afkep->ops = ops;
	afkep->dcp = dcp;
	afkep->endpoint = endpoint;
	afkep->wq = alloc_ordered_workqueue("apple-dcp-afkep%02x",
					    WQ_MEM_RECLAIM, endpoint);
	if (!afkep->wq) {
		ret = -ENOMEM;
		goto out_free_afkep;
	}

	// TODO: devm_ for wq

	init_completion(&afkep->started);
	init_completion(&afkep->stopped);
	INIT_LIST_HEAD(&afkep->compact_cmds);
	spin_lock_init(&afkep->lock);

	return afkep;

out_free_afkep:
	devm_kfree(dcp->dev, afkep);
	return ERR_PTR(ret);
}

void afk_cancel_commands(struct apple_dcp_afkep *afkep)
{
	struct afk_compact_cmd *cmd, *tmp;
	unsigned long flags;

	spin_lock_irqsave(&afkep->lock, flags);
	afkep->compact_shutting_down = true;
	list_for_each_entry_safe(cmd, tmp, &afkep->compact_cmds, node) {
		list_del_init(&cmd->node);
		cmd->status = -ESHUTDOWN;
		complete(&cmd->done);
	}
	spin_unlock_irqrestore(&afkep->lock, flags);
}

void afk_shutdown(struct apple_dcp_afkep *afkep)
{
	afk_cancel_commands(afkep);
	afk_send(afkep, FIELD_PREP(RBEP_TYPE, RBEP_SHUTDOWN));
	int ret;

	ret = wait_for_completion_timeout(&afkep->stopped, msecs_to_jiffies(1000));
	if (ret <= 0) {
		dev_err(afkep->dcp->dev, "Timed out shutting down AFK endpoint %02x", afkep->endpoint);
	}

	destroy_workqueue(afkep->wq);
}

int afk_start(struct apple_dcp_afkep *ep)
{
	int ret;

	reinit_completion(&ep->started);
	apple_rtkit_start_ep(ep->dcp->rtk, ep->endpoint);
	afk_send(ep, FIELD_PREP(RBEP_TYPE, RBEP_INIT));

	ret = wait_for_completion_timeout(&ep->started, msecs_to_jiffies(1000));
	if (ret <= 0)
		return -ETIMEDOUT;
	else
		return 0;
}

static void afk_getbuf(struct apple_dcp_afkep *ep, u64 message)
{
	u16 size = FIELD_GET(GETBUF_SIZE, message) << BLOCK_SHIFT;
	u16 tag = FIELD_GET(GETBUF_TAG, message);
	u64 reply;

	trace_afk_getbuf(ep, size, tag);

	if (ep->bfr) {
		dev_err(ep->dcp->dev,
			"Got GETBUF message but buffer already exists\n");
		return;
	}

	ep->bfr = dmam_alloc_coherent(ep->dcp->dev, size, &ep->bfr_dma,
				      GFP_KERNEL);
	if (!ep->bfr) {
		dev_err(ep->dcp->dev, "Failed to allocate %d bytes buffer\n",
			size);
		return;
	}

	ep->bfr_size = size;
	ep->bfr_tag = tag;
	reply = FIELD_PREP(RBEP_TYPE, RBEP_GETBUF_ACK);
	reply |= FIELD_PREP(GETBUF_ACK_DVA, ep->bfr_dma);
	afk_send(ep, reply);
}

static void afk_init_rxtx(struct apple_dcp_afkep *ep, u64 message,
			  struct afk_ringbuffer *bfr)
{
	u16 base = FIELD_GET(INITRB_OFFSET, message) << BLOCK_SHIFT;
	u16 size = FIELD_GET(INITRB_SIZE, message) << BLOCK_SHIFT;
	u16 tag = FIELD_GET(INITRB_TAG, message);
	u32 bufsz, end;
	size_t block_size;

	if (tag != ep->bfr_tag) {
		dev_err(ep->dcp->dev, "AFK[ep:%02x]: expected tag 0x%x but got 0x%x\n",
			ep->endpoint, ep->bfr_tag, tag);
		return;
	}

	if (bfr->ready) {
		dev_err(ep->dcp->dev, "AFK[ep:%02x]: buffer is already initialized\n",
			ep->endpoint);
		return;
	}

	if (base >= ep->bfr_size) {
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: requested base 0x%x >= max size 0x%lx\n",
			ep->endpoint, base, ep->bfr_size);
		return;
	}

	end = base + size;
	if (end > ep->bfr_size) {
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: requested end 0x%x > max size 0x%lx\n",
			ep->endpoint, end, ep->bfr_size);
		return;
	}

	bfr->hdr = ep->bfr + base;
	bufsz = le32_to_cpu(bfr->hdr->bufsz);
	if (bufsz > size || (size - bufsz) % 3) {
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: invalid ring buffer layout: size 0x%x, data 0x%x\n",
			ep->endpoint, size, bufsz);
		return;
	}

	block_size = (size - bufsz) / 3;
	if (block_size < sizeof(*bfr->hdr) || block_size % (1 << BLOCK_SHIFT)) {
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: invalid ring buffer block size 0x%lx\n",
			ep->endpoint, block_size);
		return;
	}

	bfr->rptr = ep->bfr + base + block_size;
	bfr->wptr = ep->bfr + base + 2 * block_size;
	bfr->buf = ep->bfr + base + 3 * block_size;
	bfr->bufsz = bufsz;
	bfr->block_size = block_size;
	bfr->ready = true;
	dev_dbg(ep->dcp->dev,
		 "AFK[ep:%02x]: %s base=%#x size=%#x hdr=%pad rptr=%pad wptr=%pad data=%pad data-size=%#x block=%#lx\n",
		 ep->endpoint, bfr == &ep->txbfr ? "TX" : "RX", base, size,
		 &(dma_addr_t){ ep->bfr_dma + base },
		 &(dma_addr_t){ ep->bfr_dma + base + block_size },
		 &(dma_addr_t){ ep->bfr_dma + base + 2 * block_size },
		 &(dma_addr_t){ ep->bfr_dma + base + 3 * block_size },
		 bufsz, block_size);

	if (ep->rxbfr.ready && ep->txbfr.ready)
		afk_send(ep, FIELD_PREP(RBEP_TYPE, RBEP_START));
}

#if IS_ENABLED(CONFIG_DRM_APPLE_DEBUG)
static void afk_populate_service_debugfs(struct apple_epic_service *srv);
static void afk_remove_service_debugfs(struct apple_epic_service *srv);
#else
static void afk_populate_service_debugfs(struct apple_epic_service *srv)
{
}
static void afk_remove_service_debugfs(struct apple_epic_service *srv)
{
}
#endif

static const struct apple_epic_service_ops *
afk_match_service(struct apple_dcp_afkep *ep, const char *name)
{
	const struct apple_epic_service_ops *ops;
	const char *service_name;
	size_t len;

	if (!name[0])
		return NULL;
	if (!ep->ops)
		return NULL;

	service_name = strchr(name, ':');
	if (service_name)
		service_name++;
	else
		service_name = name;

	for (ops = ep->ops; ops->name[0]; ops++) {
		if (!strcmp(ops->name, "*"))
			return ops;

		len = strlen(ops->name);
		if (strncmp(ops->name, service_name, len))
			continue;
		if (service_name[len] && service_name[len] != ':')
			continue;

		return ops;
	}

	return NULL;
}

static struct apple_epic_service *afk_epic_find_service(struct apple_dcp_afkep *ep,
						 u32 channel)
{
    for (u32 i = 0; i < ep->num_channels; i++)
        if (ep->services[i].enabled && ep->services[i].channel == channel)
            return &ep->services[i];

    return NULL;
}

static int afk_send_compact(struct apple_dcp_afkep *ep, u16 intf_id, u8 type,
			    u8 category, const void *payload, size_t payload_size,
			    size_t wire_size)
{
	struct epic_compact_hdr *chdr;
	struct epic_compact_sub_hdr *cshdr;
	struct afk_qe *qhdr, *qhdr2 = NULL;
	unsigned long flags;
	void *message;
	size_t message_size, advance;
	u32 rptr, wptr, old_wptr;
	int ret = 0;

	if (payload_size > wire_size)
		return -EINVAL;
	if (!ep->txbfr.ready)
		return -EIO;

	message_size = sizeof(*chdr) + sizeof(*cshdr) + wire_size;
	message = kzalloc(message_size, GFP_KERNEL);
	if (!message)
		return -ENOMEM;

	chdr = message;
	cshdr = message + sizeof(*chdr);
	chdr->intf_id = cpu_to_le16(intf_id);
	chdr->length = cpu_to_le32(message_size -
					   offsetof(struct epic_compact_hdr, timestamp));
	cshdr->type = type;
	cshdr->category = category;
	cshdr->unk = type == 0x12;
	if (payload_size)
		memcpy(cshdr + 1, payload, payload_size);

	spin_lock_irqsave(&ep->lock, flags);
	chdr->seq = ep->qe_seq++;

	dma_rmb();
	rptr = le32_to_cpu(READ_ONCE(*ep->txbfr.rptr));
	wptr = le32_to_cpu(READ_ONCE(*ep->txbfr.wptr));
	old_wptr = wptr;
	advance = ALIGN(sizeof(*qhdr) + message_size, ep->txbfr.block_size);

	if (advance > ep->txbfr.bufsz) {
		ret = -EMSGSIZE;
		goto out_unlock;
	}
	if (wptr < rptr && advance >= rptr - wptr) {
		ret = -ENOMEM;
		goto out_unlock;
	}
	if (wptr >= rptr) {
		bool fits_above = advance < ep->txbfr.bufsz - wptr ||
			(advance == ep->txbfr.bufsz - wptr && rptr != 0);

		if (!fits_above && advance >= rptr) {
			ret = -ENOMEM;
			goto out_unlock;
		}
	}

	qhdr = ep->txbfr.buf + wptr;
	qhdr->magic = cpu_to_le32(QE_MAGIC);
	qhdr->size = cpu_to_le32(message_size);
	qhdr->channel = cpu_to_le32(0);
	qhdr->type = cpu_to_le32(0);
	wptr += sizeof(*qhdr);

	if (message_size > ep->txbfr.bufsz - wptr) {
		qhdr2 = ep->txbfr.buf;
		memcpy(qhdr2, qhdr, sizeof(*qhdr));
		qhdr = qhdr2;
		wptr = sizeof(*qhdr);
	}

	memcpy(qhdr->data, message, message_size);
	wptr = ALIGN(wptr + message_size, ep->txbfr.block_size);
	if (wptr == ep->txbfr.bufsz)
		wptr = 0;

	dma_wmb();
	WRITE_ONCE(*ep->txbfr.wptr, cpu_to_le32(wptr));
	dma_wmb();
	afk_send(ep, FIELD_PREP(RBEP_TYPE, RBEP_RECV) |
		     ((u64)(rptr & 0xffff) << 32) |
		     ((u64)(wptr & 0xffff) << 16) | (old_wptr & 0xffff));

out_unlock:
	spin_unlock_irqrestore(&ep->lock, flags);
	kfree(message);
	return ret;
}

int afk_start_service(struct apple_epic_service *service)
{
	int ret;

	if (!service || !service->enabled)
		return -EINVAL;
	if (!service->ep->compact)
		return 0;
	if (service->started)
		return 0;

	ret = afk_send_compact(service->ep, service->channel, 0x12, 0,
			       NULL, 0, 0);
	if (!ret)
		service->started = true;
	return ret;
}

static void afk_recv_handle_init(struct apple_dcp_afkep *ep, u32 channel,
				 u8 *payload, size_t payload_size)
{
	char name[32];
	s64 epic_unit = -1;
	u32 ch_idx;
	const char *service_name = name;
	const char *epic_name = NULL, *epic_class = NULL;
	const struct apple_epic_service_ops *ops;
	struct dcp_parse_ctx ctx;
	u8 *props = payload + sizeof(name);
	size_t props_size = payload_size - sizeof(name);

	WARN_ON(afk_epic_find_service(ep, channel));

	if (payload_size < sizeof(name)) {
		dev_err(ep->dcp->dev, "AFK[ep:%02x]: payload too small: %lx\n",
			ep->endpoint, payload_size);
		return;
	}

	if (ep->num_channels >= AFK_MAX_CHANNEL) {
		dev_err(ep->dcp->dev, "AFK[ep:%02x]: too many enabled services!\n",
			ep->endpoint);
		return;
	}

	strscpy(name, payload, sizeof(name));

	/*
	 * in DCP firmware 13.2 DCP reports interface-name as name which starts
	 * with "dispext%d" using -1 s ID for "dcp". In the 12.3 firmware
	 * EPICProviderClass was used. If the init call has props parse them and
	 * use EPICProviderClass to match the service.
	 */
	if (props_size > 36) {
		int ret = parse(props, props_size, &ctx);
		if (ret) {
			dev_err(ep->dcp->dev,
				"AFK[ep:%02x]: Failed to parse service init props for %s\n",
				ep->endpoint, name);
			return;
		}
		ret = parse_epic_service_init(&ctx, &epic_name, &epic_class, &epic_unit);
		if (ret) {
			dev_err(ep->dcp->dev,
				"AFK[ep:%02x]: failed to extract init props: %d\n",
				ep->endpoint, ret);
			return;
		}
		service_name = epic_class;
	} else {
            service_name = name;
        }

	if (ep->match_epic_name)
		ops = afk_match_service(ep, epic_name);
	else
		ops = afk_match_service(ep, service_name);

	if (!ops) {
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: unable to match service %s on channel %d\n",
			ep->endpoint, service_name, channel);
		goto free;
	}

	ch_idx = ep->num_channels++;
	spin_lock_init(&ep->services[ch_idx].lock);
	ep->services[ch_idx].enabled = true;
	ep->services[ch_idx].torndown = false;
	ep->services[ch_idx].ops = ops;
	ep->services[ch_idx].ep = ep;
	ep->services[ch_idx].channel = channel;
	ep->services[ch_idx].cmd_tag = 0;
	ops->init(&ep->services[ch_idx], epic_name, epic_class, epic_unit);
	dev_info(ep->dcp->dev, "AFK[ep:%02x]: new service %s on channel %d\n",
		 ep->endpoint, service_name, channel);

	afk_populate_service_debugfs(&ep->services[ch_idx]);

free:
	kfree(epic_name);
	kfree(epic_class);
}

static void afk_recv_handle_compact_init(struct apple_dcp_afkep *ep,
					 u16 channel, const void *payload,
					 size_t payload_size)
{
	char wire_name[33], service_name[33];
	const char *name;
	char *unit_suffix;
	const struct apple_epic_service_ops *ops;
	struct apple_epic_service *service;
	s64 unit = -1;
	u32 ch_idx;
	int ret;

	if (payload_size < 32) {
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: short compact service announcement\n",
			ep->endpoint);
		return;
	}
	if (afk_epic_find_service(ep, channel)) {
		dev_warn(ep->dcp->dev,
			 "AFK[ep:%02x]: duplicate compact service on interface %u\n",
			 ep->endpoint, channel);
		return;
	}
	memcpy(wire_name, payload, 32);
	wire_name[32] = '\0';
	name = strchr(wire_name, ':');
	if (name)
		name++;
	else
		name = wire_name;
	strscpy(service_name, name, sizeof(service_name));

	unit_suffix = strrchr(service_name, ':');
	if (unit_suffix && !kstrtos64(unit_suffix + 1, 10, &unit))
		*unit_suffix = '\0';
	else
		unit = -1;

	ops = afk_match_service(ep, wire_name);
	if (!ops) {
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: unable to match compact service %s on interface %u\n",
			ep->endpoint, wire_name, channel);
		return;
	}

	/* Compact AV services are republished under fresh interface IDs after
	 * every external-display hotplug, without a teardown message for the
	 * previous interface.  Replace the same logical service in place so the
	 * fixed service table does not fill after a handful of reconnects.
	 * This behavior is specific to DPDEV, DPAVSERV, and AV.  Other compact
	 * endpoints (notably DPAVCTRL) may publish several simultaneous services
	 * with the same name and no unit, so name/unit is not a unique key there. */
	ch_idx = ep->num_channels;
	if (ep->endpoint >= DPDEV_ENDPOINT && ep->endpoint <= AV_ENDPOINT) {
		for (u32 i = 0; i < ep->num_channels; i++) {
			service = &ep->services[i];
			if (service->ops == ops && service->unit == unit &&
			    !strcmp(service->name, service_name)) {
				ch_idx = i;
				break;
			}
		}
	}

	if (ch_idx == ep->num_channels) {
		for (u32 i = 0; i < ep->num_channels; i++) {
			if (!ep->services[i].enabled) {
				ch_idx = i;
				break;
			}
		}
	}

	if (ch_idx == ep->num_channels) {
		if (ep->num_channels >= AFK_MAX_CHANNEL) {
			dev_err(ep->dcp->dev,
				"AFK[ep:%02x]: too many enabled compact services\n",
				ep->endpoint);
			return;
		}
		ep->num_channels++;
	} else if (ep->services[ch_idx].enabled) {
		service = &ep->services[ch_idx];
		dev_info(ep->dcp->dev,
			 "AFK[ep:%02x]: replacing compact service %s interface %u with %u\n",
			 ep->endpoint, service_name, service->channel, channel);
		afk_remove_service_debugfs(service);
		service->torndown = true;
		if (service->ops->teardown)
			service->ops->teardown(service);
		service->enabled = false;
	}

	service = &ep->services[ch_idx];
	memset(service, 0, sizeof(*service));
	spin_lock_init(&service->lock);
	service->enabled = true;
	service->torndown = false;
	service->ops = ops;
	service->ep = ep;
	service->channel = channel;
	strscpy(service->name, service_name, sizeof(service->name));
	service->unit = unit;
	service->cmd_tag = 0;
	ops->init(service, service_name, service_name, unit);

	dev_info(ep->dcp->dev,
		 "AFK[ep:%02x]: new compact service %s on interface %u%s\n",
		 ep->endpoint, wire_name, channel,
		 service->defer_start ? " (deferred)" : "");
	afk_populate_service_debugfs(service);

	if (!service->defer_start) {
		ret = afk_start_service(service);
		if (ret)
			dev_err(ep->dcp->dev,
				"AFK[ep:%02x]: failed to start compact service %u: %d\n",
				ep->endpoint, channel, ret);
	}
}

static void afk_recv_handle_compact_reply(struct apple_dcp_afkep *ep,
					  u16 channel, u8 type,
					  const void *payload,
					  size_t payload_size)
{
	const struct epic_compact_desc *desc = payload;
	const struct epic_service_call *call = NULL;
	struct afk_compact_cmd *pending, *match = NULL;
	unsigned long flags;
	size_t copy_size;

	if (payload_size < sizeof(*desc)) {
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: compact reply is missing its descriptor\n",
			ep->endpoint);
		return;
	}

	if (payload_size >= sizeof(*desc) + sizeof(*call)) {
		call = (const void *)(desc + 1);
		if (le32_to_cpu(call->magic) != EPIC_SERVICE_CALL_MAGIC)
			call = NULL;
	}

	spin_lock_irqsave(&ep->lock, flags);
	list_for_each_entry(pending, &ep->compact_cmds, node) {
		if (pending->channel != channel || pending->type != type)
			continue;
		if (pending->request_status != le32_to_cpu(desc->status))
			continue;
		if (pending->service_call &&
		    (!call || pending->group != le16_to_cpu(call->group) ||
		     pending->command != le32_to_cpu(call->command)))
			continue;
		match = pending;
		break;
	}
	if (!match) {
		spin_unlock_irqrestore(&ep->lock, flags);
		dev_warn(ep->dcp->dev,
			 "AFK[ep:%02x]: unexpected compact reply %02x on interface %u\n",
			 ep->endpoint, type, channel);
		return;
	}

	list_del_init(&match->node);
	copy_size = min(match->output_len, payload_size - sizeof(*desc));
	if (copy_size && match->output)
		memcpy(match->output, desc + 1, copy_size);
	match->retcode = le32_to_cpu(desc->status);
	match->status = 0;
	complete(&match->done);
	spin_unlock_irqrestore(&ep->lock, flags);
}

static void afk_recv_handle_compact_call(struct apple_dcp_afkep *ep,
					 u16 channel, const void *payload,
					 size_t payload_size)
{
	const struct epic_compact_desc *request_desc = payload;
	const struct epic_service_call *call;
	struct epic_compact_desc *reply_desc;
	struct epic_service_call *reply_call;
	struct apple_epic_service *service;
	void *reply;
	size_t inner_size, call_size;
	int ret;

	if (payload_size < sizeof(*request_desc) + sizeof(*call))
		return;

	call = payload + sizeof(*request_desc);
	inner_size = payload_size - sizeof(*request_desc);
	call_size = le32_to_cpu(call->data_len);
	if (le32_to_cpu(call->magic) != EPIC_SERVICE_CALL_MAGIC ||
	    inner_size < sizeof(*call) + call_size) {
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: malformed compact service call on interface %u\n",
			ep->endpoint, channel);
		return;
	}

	if (compact_trace)
		dev_info(ep->dcp->dev,
			 "AFK-COMPACT call ep=%02x intf=%u group=%u command=%u status=%#x size=%zu in=%*phN\n",
			 ep->endpoint, channel, le16_to_cpu(call->group),
			 le32_to_cpu(call->command),
			 le32_to_cpu(request_desc->status), call_size,
			 min_t(int, call_size, 64), call + 1);

	reply = kzalloc(payload_size, GFP_KERNEL);
	if (!reply)
		return;
	reply_desc = reply;
	reply_desc->status = request_desc->status;
	reply_call = reply + sizeof(*reply_desc);
	memcpy(reply_call, call, sizeof(*call));

	service = afk_epic_find_service(ep, channel);
	if (!service || !service->ops->call) {
		ret = -ENOSYS;
		dev_warn(ep->dcp->dev,
			 "AFK[ep:%02x]: no handler for compact service %u call %u:%u\n",
			 ep->endpoint, channel, le16_to_cpu(call->group),
			 le32_to_cpu(call->command));
	} else {
		ret = service->ops->call(service, le16_to_cpu(call->group),
					le32_to_cpu(call->command),
					call + 1, call_size, reply_call + 1,
					call_size);
	}
	if (ret)
		reply_desc->status = cpu_to_le32(ret);
	if (compact_trace)
		dev_info(ep->dcp->dev,
			 "AFK-COMPACT reply ep=%02x intf=%u group=%u command=%u ret=%d status=%#x size=%zu out=%*phN\n",
			 ep->endpoint, channel, le16_to_cpu(call->group),
			 le32_to_cpu(call->command), ret,
			 le32_to_cpu(reply_desc->status), call_size,
			 min_t(int, call_size, 64), reply_call + 1);

	ret = afk_send_compact(ep, channel, EPIC_SUBTYPE_STD_SERVICE, 2,
			       reply, payload_size, payload_size);
	if (ret)
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: failed to reply to compact service call: %d\n",
			ep->endpoint, ret);
	kfree(reply);
}

static bool afk_recv_handle_compact(struct apple_dcp_afkep *ep, u32 channel,
				    u32 type, const void *data,
				    size_t data_size)
{
	const struct epic_compact_hdr *hdr = data;
	const struct epic_compact_sub_hdr *sub;
	struct apple_epic_service *service;
	const void *payload;
	size_t payload_size;
	u16 intf_id;

	if (channel || type ||
	    data_size < sizeof(*hdr) + sizeof(*sub))
		return false;
	sub = data + sizeof(*hdr);
	if (le32_to_cpu(hdr->length) !=
	    data_size - offsetof(struct epic_compact_hdr, timestamp))
		return false;

	ep->compact = true;
	intf_id = le16_to_cpu(hdr->intf_id);
	payload = sub + 1;
	payload_size = data_size - sizeof(*hdr) - sizeof(*sub);

	if (sub->category == 0 && sub->type == 0x11) {
		afk_recv_handle_compact_init(ep, intf_id, payload, payload_size);
		return true;
	}
	if (sub->category == 1 && sub->type == EPIC_SUBTYPE_STD_SERVICE) {
		afk_recv_handle_compact_call(ep, intf_id, payload, payload_size);
		return true;
	}
	if (sub->category == 2) {
		afk_recv_handle_compact_reply(ep, intf_id, sub->type, payload,
					      payload_size);
		return true;
	}
	if (sub->category == 0 && sub->type == EPIC_SUBTYPE_STD_SERVICE) {
		service = afk_epic_find_service(ep, intf_id);
		if (payload_size >= sizeof(struct epic_compact_desc)) {
			payload += sizeof(struct epic_compact_desc);
			payload_size -= sizeof(struct epic_compact_desc);
		}
		if (compact_trace) {
			dev_info(ep->dcp->dev,
				 "AFK-COMPACT report ep=%02x intf=%u service=%s size=%zu data=%*phN\n",
				 ep->endpoint, intf_id,
				 service ? service->ops->name : "<missing>",
				 payload_size, min_t(int, payload_size, 64),
				 payload);
			if (payload_size > 64)
				dev_info(ep->dcp->dev,
					 "AFK-COMPACT report-tail ep=%02x intf=%u data=%*phN\n",
					 ep->endpoint, intf_id,
					 min_t(int, payload_size - 64, 64),
					 payload + 64);
		}
		if (service && service->ops->report) {
			service->ops->report(service, EPIC_SUBTYPE_STD_SERVICE,
					     payload, payload_size);
		}
		return true;
	}
	if (sub->category == 0) {
		service = afk_epic_find_service(ep, intf_id);
		if (compact_trace)
			dev_info(ep->dcp->dev,
				 "AFK-COMPACT notification ep=%02x intf=%u service=%s type=%02x size=%zu data=%*phN\n",
				 ep->endpoint, intf_id,
				 service ? service->ops->name : "<missing>",
				 sub->type, payload_size,
				 min_t(int, payload_size, 64), payload);
		if (service && service->ops->report)
			service->ops->report(service, sub->type, payload,
					     payload_size);
		return true;
	}

	dev_dbg(ep->dcp->dev,
		"AFK[ep:%02x]: unhandled compact message %02x:%02x on interface %u\n",
		ep->endpoint, sub->category, sub->type, intf_id);
	return true;
}

static void afk_recv_handle_teardown(struct apple_dcp_afkep *ep, u32 channel)
{
	struct apple_epic_service *service;
	const struct apple_epic_service_ops *ops;
	unsigned long flags;

	service = afk_epic_find_service(ep, channel);
	if (!service) {
		dev_warn(ep->dcp->dev, "AFK[ep:%02x]: teardown for disabled channel %u\n",
			 ep->endpoint, channel);
		return;
	}

	afk_remove_service_debugfs(service);

	// TODO: think through what locking is necessary
	spin_lock_irqsave(&service->lock, flags);
	/*
	 * teardown must not disable the service since since it may be sent as
	 * side effect of a COMMAND which for which a reply is expected.
	 * Seen with DCP's "av" endpoint during the close afk_service_call.
	 */
	service->torndown = true;
	ops = service->ops;
	spin_unlock_irqrestore(&service->lock, flags);

	if (ops->teardown)
		ops->teardown(service);
}

static void afk_recv_handle_reply(struct apple_dcp_afkep *ep, u32 channel,
				  u16 tag, void *payload, size_t payload_size)
{
	struct epic_cmd *cmd = payload;
	struct apple_epic_service *service;
	unsigned long flags;
	u8 idx = tag & 0xff;
	void *rxbuf, *txbuf;
	dma_addr_t rxbuf_dma, txbuf_dma;
	size_t rxlen, txlen;

	service = afk_epic_find_service(ep, channel);
	if (!service) {
		dev_warn(ep->dcp->dev, "AFK[ep:%02x]: command reply on disabled channel %u\n",
			 ep->endpoint, channel);
		return;
	}

	if (payload_size < sizeof(*cmd)) {
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: command reply on channel %d too small: %ld\n",
			ep->endpoint, channel, payload_size);
		return;
	}

	if (idx >= MAX_PENDING_CMDS) {
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: command reply on channel %d out of range: %d\n",
			ep->endpoint, channel, idx);
		return;
	}

	spin_lock_irqsave(&service->lock, flags);
	if (service->cmds[idx].done) {
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: command reply on channel %d already handled\n",
			ep->endpoint, channel);
		spin_unlock_irqrestore(&service->lock, flags);
		return;
	}

	if (tag != service->cmds[idx].tag) {
		dev_err(ep->dcp->dev,
			"AFK[ep:%02x]: command reply on channel %d has invalid tag: expected 0x%04x != 0x%04x\n",
			ep->endpoint, channel, tag, service->cmds[idx].tag);
		spin_unlock_irqrestore(&service->lock, flags);
		return;
	}

	service->cmds[idx].done = true;
	service->cmds[idx].retcode = le32_to_cpu(cmd->retcode);
	if (service->cmds[idx].free_on_ack) {
		/* defer freeing until we're no longer in atomic context */
		rxbuf = service->cmds[idx].rxbuf;
		txbuf = service->cmds[idx].txbuf;
		rxlen = service->cmds[idx].rxlen;
		txlen = service->cmds[idx].txlen;
		rxbuf_dma = service->cmds[idx].rxbuf_dma;
		txbuf_dma = service->cmds[idx].txbuf_dma;
		bitmap_release_region(service->cmd_map, idx, 0);
	} else {
		rxbuf = txbuf = NULL;
		rxlen = txlen = 0;
	}
	if (service->cmds[idx].completion)
		complete(service->cmds[idx].completion);

	spin_unlock_irqrestore(&service->lock, flags);

	if (rxbuf && rxlen)
		dma_free_coherent(ep->dcp->dev, rxlen, rxbuf, rxbuf_dma);
	if (txbuf && txlen)
		dma_free_coherent(ep->dcp->dev, txlen, txbuf, txbuf_dma);
}

struct epic_std_service_ap_call {
	__le32 unk0;
	__le32 unk1;
	__le32 type;
	__le32 len;
	__le32 magic;
	u8 _unk[48];
} __attribute__((packed));

static void afk_recv_handle_std_service(struct apple_dcp_afkep *ep, u32 channel,
					u32 type, struct epic_hdr *ehdr,
					struct epic_sub_hdr *eshdr,
					void *payload, size_t payload_size)
{
	struct apple_epic_service *service = afk_epic_find_service(ep, channel);

	if (!service) {
		dev_warn(ep->dcp->dev,
			 "AFK[ep:%02x]: std service notify on disabled channel %u\n",
			 ep->endpoint, channel);
		return;
	}
	if (service->torndown) {
		dev_warn(ep->dcp->dev,
			 "AFK[ep:%02x]: std service notify on torn down service "
			 "(chan:%u)\n", ep->endpoint, channel);
		return;
	}

	if (type == EPIC_TYPE_NOTIFY && eshdr->category == EPIC_CAT_NOTIFY) {
		struct epic_std_service_ap_call *call = payload;
		size_t call_size;
		void *reply;
		int ret;

		if (payload_size < sizeof(*call))
			return;

		call_size = le32_to_cpu(call->len);
		if (payload_size < sizeof(*call) + call_size)
			return;

		if (!service->ops->call)
			return;
		reply = kzalloc(payload_size, GFP_KERNEL);
		if (!reply)
			return;

		ret = service->ops->call(service, 0, le32_to_cpu(call->type),
					 payload + sizeof(*call), call_size,
					 reply + sizeof(*call), call_size);
		if (ret) {
			kfree(reply);
			return;
		}

		memcpy(reply, call, sizeof(*call));
		afk_send_epic(ep, channel, le16_to_cpu(eshdr->tag),
			      EPIC_TYPE_NOTIFY_ACK, EPIC_CAT_REPLY,
			      EPIC_SUBTYPE_STD_SERVICE, reply, payload_size);
		kfree(reply);

		return;
	}

	if (type == EPIC_TYPE_NOTIFY && eshdr->category == EPIC_CAT_REPORT) {
		if (service->ops->report)
			service->ops->report(service, le16_to_cpu(eshdr->type),
					     payload, payload_size);
		return;
	}

	dev_err(ep->dcp->dev,
		"AFK[ep:%02x]: channel %d received unhandled standard service message: %x / %x\n",
		ep->endpoint, channel, type, eshdr->category);
	print_hex_dump(KERN_INFO, "AFK: ", DUMP_PREFIX_NONE, 16, 1, payload,
				   payload_size, true);
}

static void afk_recv_handle(struct apple_dcp_afkep *ep, u32 channel, u32 type,
			    u8 *data, size_t data_size)
{
	struct apple_epic_service *service;
	struct epic_hdr *ehdr;
	struct epic_sub_hdr *eshdr;
	u16 subtype;
	u8 *payload;
	size_t payload_size;

	if (afk_recv_handle_compact(ep, channel, type, data, data_size))
		return;

	ehdr = (struct epic_hdr *)data;
	eshdr = (struct epic_sub_hdr *)(data + sizeof(*ehdr));
	if (data_size < sizeof(*ehdr) + sizeof(*eshdr)) {
		dev_err(ep->dcp->dev, "AFK[ep:%02x]: payload too small: %lx\n",
			ep->endpoint, data_size);
		return;
	}
	subtype = le16_to_cpu(eshdr->type);
	payload = data + sizeof(*ehdr) + sizeof(*eshdr);
	payload_size = data_size - sizeof(*ehdr) - sizeof(*eshdr);

	trace_afk_recv_handle(ep, channel, type, data_size, ehdr, eshdr);

	service = afk_epic_find_service(ep, channel);

	if (!service) {
		if (type != EPIC_TYPE_NOTIFY && type != EPIC_TYPE_REPLY) {
			dev_err(ep->dcp->dev,
				"AFK[ep:%02x]: expected notify but got 0x%x on channel %d\n",
				ep->endpoint, type, channel);
			return;
		}
		if (eshdr->category != EPIC_CAT_REPORT) {
			dev_err(ep->dcp->dev,
				"AFK[ep:%02x]: expected report but got 0x%x on channel %d\n",
				ep->endpoint, eshdr->category, channel);
			return;
		}
		if (subtype == EPIC_SUBTYPE_TEARDOWN) {
			dev_dbg(ep->dcp->dev,
				"AFK[ep:%02x]: teardown without service on channel %d\n",
				ep->endpoint, channel);
			return;
		}
		if (subtype != EPIC_SUBTYPE_ANNOUNCE) {
			dev_err(ep->dcp->dev,
				"AFK[ep:%02x]: expected announce but got 0x%x on channel %d\n",
				ep->endpoint, subtype, channel);
			return;
		}

		return afk_recv_handle_init(ep, channel, payload, payload_size);
	}

	if (!service) {
		dev_err(ep->dcp->dev, "AFK[ep:%02x]: channel %d has no service\n",
			ep->endpoint, channel);
		return;
	}

	if (type == EPIC_TYPE_NOTIFY && eshdr->category == EPIC_CAT_REPORT &&
	    subtype == EPIC_SUBTYPE_TEARDOWN)
		return afk_recv_handle_teardown(ep, channel);

	if (type == EPIC_TYPE_REPLY && eshdr->category == EPIC_CAT_REPLY)
		return afk_recv_handle_reply(ep, channel,
					     le16_to_cpu(eshdr->tag), payload,
					     payload_size);

	if (subtype == EPIC_SUBTYPE_STD_SERVICE)
		return afk_recv_handle_std_service(
			ep, channel, type, ehdr, eshdr, payload, payload_size);

	dev_err(ep->dcp->dev, "AFK[ep:%02x]: channel %d received unhandled message "
		"(type %x subtype %x)\n", ep->endpoint, channel, type, subtype);
	print_hex_dump(KERN_INFO, "AFK: ", DUMP_PREFIX_NONE, 16, 1, payload,
				   payload_size, true);
}

static bool afk_recv(struct apple_dcp_afkep *ep)
{
	struct afk_qe *hdr;
	u32 rptr, wptr;
	u32 magic, size, channel, type;

	if (!ep->rxbfr.ready) {
		dev_err(ep->dcp->dev, "AFK[ep:%02x]: got RECV but not ready\n",
			ep->endpoint);
		return false;
	}

	rptr = le32_to_cpu(READ_ONCE(*ep->rxbfr.rptr));
	wptr = le32_to_cpu(READ_ONCE(*ep->rxbfr.wptr));
	trace_afk_recv_rwptr_pre(ep, rptr, wptr);

	if (rptr == wptr)
		return false;

	if (rptr > (ep->rxbfr.bufsz - sizeof(*hdr))) {
		dev_warn(ep->dcp->dev,
			 "AFK[ep:%02x]: rptr out of bounds: 0x%x > 0x%lx\n",
			 ep->endpoint, rptr, ep->rxbfr.bufsz - sizeof(*hdr));
		return false;
	}

	dma_rmb();

	hdr = ep->rxbfr.buf + rptr;
	magic = le32_to_cpu(hdr->magic);
	size = le32_to_cpu(hdr->size);
	trace_afk_recv_qe(ep, rptr, magic, size);

	if (magic != QE_MAGIC) {
		dev_warn(ep->dcp->dev, "AFK[ep:%02x]: invalid queue entry magic: 0x%x\n",
			 ep->endpoint, magic);
		return false;
	}

	/*
	 * If there's not enough space for the payload the co-processor inserted
	 * the current dummy queue entry and we have to advance to the next one
	 * which will contain the real data.
	*/
	if (rptr + size + sizeof(*hdr) > ep->rxbfr.bufsz) {
		rptr = 0;
		hdr = ep->rxbfr.buf + rptr;
		magic = le32_to_cpu(hdr->magic);
		size = le32_to_cpu(hdr->size);
		trace_afk_recv_qe(ep, rptr, magic, size);

		if (magic != QE_MAGIC) {
			dev_warn(ep->dcp->dev,
				 "AFK[ep:%02x]: invalid next queue entry magic: 0x%x\n",
				 ep->endpoint, magic);
			return false;
		}

		WRITE_ONCE(*ep->rxbfr.rptr, cpu_to_le32(rptr));
	}

	if (rptr + size + sizeof(*hdr) > ep->rxbfr.bufsz) {
		dev_warn(ep->dcp->dev,
			 "AFK[ep:%02x]: queue entry out of bounds: 0x%lx > 0x%lx\n",
			 ep->endpoint, rptr + size + sizeof(*hdr), ep->rxbfr.bufsz);
		return false;
	}

	channel = le32_to_cpu(hdr->channel);
	type = le32_to_cpu(hdr->type);

	rptr = ALIGN(rptr + sizeof(*hdr) + size, ep->rxbfr.block_size);
	if (WARN_ON(rptr > ep->rxbfr.bufsz))
		rptr = 0;
	if (rptr == ep->rxbfr.bufsz)
		rptr = 0;

	dma_mb();

	WRITE_ONCE(*ep->rxbfr.rptr, cpu_to_le32(rptr));
	trace_afk_recv_rwptr_post(ep, rptr, wptr);

	/*
	 * TODO: this is theoretically unsafe since DCP could overwrite data
	 *       after the read pointer was updated above. Do it anyway since
	 *       it avoids 2 problems in the DCP tracer:
	 *       1. the tracer sees replies before the notifies from dcp
	 *       2. the tracer tries to read buffers after they are unmapped.
	 */
	afk_recv_handle(ep, channel, type, hdr->data, size);

	return true;
}

static void afk_receive_message_worker(struct work_struct *work_)
{
	struct afk_receive_message_work *work;
	u16 type;

	work = container_of(work_, struct afk_receive_message_work, work);

	type = FIELD_GET(RBEP_TYPE, work->message);
	switch (type) {
	case RBEP_INIT_ACK:
		break;

	case RBEP_START_ACK:
		complete_all(&work->ep->started);
		break;

	case RBEP_SHUTDOWN_ACK:
		complete_all(&work->ep->stopped);
		break;

	case RBEP_GETBUF:
		afk_getbuf(work->ep, work->message);
		break;

	case RBEP_INIT_TX:
		afk_init_rxtx(work->ep, work->message, &work->ep->txbfr);
		break;

	case RBEP_INIT_RX:
		afk_init_rxtx(work->ep, work->message, &work->ep->rxbfr);
		break;

	case RBEP_RECV:
		while (afk_recv(work->ep))
			;
		break;

	default:
		dev_err(work->ep->dcp->dev,
			"Received unknown AFK message type: 0x%x\n", type);
	}

	kfree(work);
}

int afk_receive_message(struct apple_dcp_afkep *ep, u64 message)
{
	struct afk_receive_message_work *work;

	// TODO: comment why decoupling from rtkit thread is required here
	work = kzalloc(sizeof(*work), GFP_KERNEL);
	if (!work)
		return -ENOMEM;

	work->ep = ep;
	work->message = message;
	INIT_WORK(&work->work, afk_receive_message_worker);
	queue_work(ep->wq, &work->work);

	return 0;
}

int afk_send_epic(struct apple_dcp_afkep *ep, u32 channel, u16 tag,
		  enum epic_type etype, enum epic_category ecat, u8 stype,
		  const void *payload, size_t payload_len)
{
	u32 rptr, wptr;
	struct afk_qe *hdr, *hdr2;
	struct epic_hdr *ehdr;
	struct epic_sub_hdr *eshdr;
	unsigned long flags;
	size_t total_epic_size, total_size;
	int ret;

	spin_lock_irqsave(&ep->lock, flags);

	dma_rmb();
	rptr = le32_to_cpu(READ_ONCE(*ep->txbfr.rptr));
	wptr = le32_to_cpu(READ_ONCE(*ep->txbfr.wptr));
	trace_afk_send_rwptr_pre(ep, rptr, wptr);
	total_epic_size = sizeof(*ehdr) + sizeof(*eshdr) + payload_len;
	total_size = sizeof(*hdr) + total_epic_size;

	hdr = hdr2 = NULL;

	/*
	 * We need to figure out how to place the entire headers and payload
	 * into the ring buffer:
	 * - If the write pointer is in front of the read pointer we just need
	 *   enough space inbetween to store everything.
	 * - If the read pointer has already wrapper around the end of the
	 *   buffer we can
	 *    a) either store the entire payload at the writer pointer if
	 *       there's enough space until the end,
	 *    b) or just store the queue entry at the write pointer to indicate
	 *       that we need to wrap to the start and then store the headers
	 *       and the payload at the beginning of the buffer. The queue
	 *       header has to be store twice in this case.
	 * In either case we have to ensure that there's always enough space
	 * so that we don't accidentally overwrite other buffers.
	 */
	if (wptr < rptr) {
		/*
		 * If wptr < rptr we can't wrap around and only have to make
		 * sure that there's enough space for the entire payload.
		 */
		if (wptr + total_size > rptr) {
			ret = -ENOMEM;
			goto out;
		}

		hdr = ep->txbfr.buf + wptr;
		wptr += sizeof(*hdr);
	} else {
		/* We need enough space to place at least a queue entry */
		if (wptr + sizeof(*hdr) > ep->txbfr.bufsz) {
			ret = -ENOMEM;
			goto out;
		}

		/*
		 * If we can place a single queue entry but not the full payload
		 * we need to place one queue entry at the end of the ring
		 * buffer and then another one together with the entire
		 * payload at the beginning.
		 */
		if (wptr + total_size > ep->txbfr.bufsz) {
			/*
			 * Ensure there's space for the  queue entry at the
			 * beginning
			 */
			if (sizeof(*hdr) > rptr) {
				ret = -ENOMEM;
				goto out;
			}

			/*
			 * Place two queue entries to indicate we want to wrap
			 * around to the firmware.
			 */
			hdr = ep->txbfr.buf + wptr;
			hdr2 = ep->txbfr.buf;
			wptr = sizeof(*hdr);

			/* Ensure there's enough space for the entire payload */
			if (wptr + total_epic_size > rptr) {
				ret = -ENOMEM;
				goto out;
			}
		} else {
			/* We have enough space to place the entire payload */
			hdr = ep->txbfr.buf + wptr;
			wptr += sizeof(*hdr);
		}
	}
	/*
	 * At this point we're guaranteed that hdr (and possibly hdr2) point
	 * to a buffer large enough to fit the queue entry and that we have
	 * enough space at wptr to store the payload.
	 */

	hdr->magic = cpu_to_le32(QE_MAGIC);
	hdr->size = cpu_to_le32(total_epic_size);
	hdr->channel = cpu_to_le32(channel);
	hdr->type = cpu_to_le32(etype);
	if (hdr2)
		memcpy(hdr2, hdr, sizeof(*hdr));

	ehdr = ep->txbfr.buf + wptr;
	memset(ehdr, 0, sizeof(*ehdr));
	ehdr->version = 2;
	ehdr->seq = cpu_to_le16(ep->qe_seq++);
	ehdr->timestamp = cpu_to_le64(0);
	wptr += sizeof(*ehdr);

	eshdr = ep->txbfr.buf + wptr;
	memset(eshdr, 0, sizeof(*eshdr));
	eshdr->length = cpu_to_le32(payload_len);
	eshdr->version = 4;
	eshdr->category = ecat;
	eshdr->type = cpu_to_le16(stype);
	eshdr->timestamp = cpu_to_le64(0);
	eshdr->tag = cpu_to_le16(tag);
	if (ecat == EPIC_CAT_REPLY)
		eshdr->inline_len = cpu_to_le32(payload_len - 4);
	else
		eshdr->inline_len = cpu_to_le32(0);
	wptr += sizeof(*eshdr);

	memcpy(ep->txbfr.buf + wptr, payload, payload_len);
	wptr += payload_len;
	wptr = ALIGN(wptr, ep->txbfr.block_size);
	if (wptr == ep->txbfr.bufsz)
		wptr = 0;
	trace_afk_send_rwptr_post(ep, rptr, wptr);

	WRITE_ONCE(*ep->txbfr.wptr, cpu_to_le32(wptr));
	afk_send(ep, FIELD_PREP(RBEP_TYPE, RBEP_SEND) |
			     FIELD_PREP(SEND_WPTR, wptr));
	ret = 0;

out:
	spin_unlock_irqrestore(&ep->lock, flags);
	return ret;
}

static int afk_send_compact_command(struct apple_epic_service *service, u8 type,
				    const void *payload, size_t payload_len,
				    void *output, size_t output_len,
				    u32 request_status, u32 *retcode,
				    unsigned long timeout)
{
	struct apple_dcp_afkep *ep = service->ep;
	const struct epic_service_call *call = payload;
	struct afk_compact_cmd pending;
	struct epic_compact_desc *desc;
	unsigned long flags;
	void *wrapped;
	size_t wire_size = max(payload_len, output_len);
	unsigned long waited;
	int ret;

	wrapped = kzalloc(sizeof(*desc) + wire_size, GFP_KERNEL);
	if (!wrapped)
		return -ENOMEM;
	desc = wrapped;
	desc->status = cpu_to_le32(request_status);
	desc->length = cpu_to_le32(wire_size);
	if (payload_len)
		memcpy(desc + 1, payload, payload_len);

	memset(&pending, 0, sizeof(pending));
	INIT_LIST_HEAD(&pending.node);
	init_completion(&pending.done);
	pending.channel = service->channel;
	pending.type = type;
	pending.request_status = request_status;
	pending.output = output;
	pending.output_len = output_len;
	pending.status = -EINPROGRESS;
	if (payload_len >= sizeof(*call) &&
	    le32_to_cpu(call->magic) == EPIC_SERVICE_CALL_MAGIC) {
		pending.service_call = true;
		pending.group = le16_to_cpu(call->group);
		pending.command = le32_to_cpu(call->command);
	}

	spin_lock_irqsave(&ep->lock, flags);
	if (ep->compact_shutting_down) {
		spin_unlock_irqrestore(&ep->lock, flags);
		ret = -ESHUTDOWN;
		goto out;
	}
	list_add_tail(&pending.node, &ep->compact_cmds);
	spin_unlock_irqrestore(&ep->lock, flags);

	ret = afk_send_compact(ep, service->channel, type, 1, wrapped,
			       sizeof(*desc) + payload_len,
			       sizeof(*desc) + wire_size);
	if (ret)
		goto remove_pending;

	waited = wait_for_completion_timeout(&pending.done, timeout);
	spin_lock_irqsave(&ep->lock, flags);
	if (!waited && !list_empty(&pending.node)) {
		list_del_init(&pending.node);
		pending.status = -ETIMEDOUT;
	}
	ret = pending.status;
	if (retcode)
		*retcode = pending.retcode;
	spin_unlock_irqrestore(&ep->lock, flags);
	goto out;

remove_pending:
	spin_lock_irqsave(&ep->lock, flags);
	if (!list_empty(&pending.node))
		list_del_init(&pending.node);
	pending.status = ret;
	spin_unlock_irqrestore(&ep->lock, flags);
out:
	kfree(wrapped);
	return ret;
}

int afk_send_command(struct apple_epic_service *service, u8 type,
		     const void *payload, size_t payload_len, void *output,
		     size_t output_len, u32 *retcode)
{
	struct epic_cmd cmd;
	void *rxbuf, *txbuf;
	dma_addr_t rxbuf_dma, txbuf_dma;
	unsigned long flags;
	int ret, idx;
	u16 tag;
	struct apple_dcp_afkep *ep = service->ep;
	DECLARE_COMPLETION_ONSTACK(completion);

	if (ep->compact)
		return afk_send_compact_command(service, type, payload,
						payload_len, output, output_len,
						afk_compact_next_status(service),
						retcode, msecs_to_jiffies(5000));

	rxbuf = dma_alloc_coherent(ep->dcp->dev, output_len, &rxbuf_dma,
				   GFP_KERNEL);
	if (!rxbuf)
		return -ENOMEM;
	txbuf = dma_alloc_coherent(ep->dcp->dev, payload_len, &txbuf_dma,
				   GFP_KERNEL);
	if (!txbuf) {
		ret = -ENOMEM;
		goto err_free_rxbuf;
	}

	memcpy(txbuf, payload, payload_len);

	memset(&cmd, 0, sizeof(cmd));
	cmd.retcode = cpu_to_le32(0);
	cmd.rxbuf = cpu_to_le64(rxbuf_dma);
	cmd.rxlen = cpu_to_le32(output_len);
	cmd.txbuf = cpu_to_le64(txbuf_dma);
	cmd.txlen = cpu_to_le32(payload_len);

	spin_lock_irqsave(&service->lock, flags);
	idx = bitmap_find_free_region(service->cmd_map, MAX_PENDING_CMDS, 0);
	if (idx < 0) {
		ret = -ENOSPC;
		goto err_unlock;
	}

	tag = (service->cmd_tag & 0xff) << 8;
	tag |= idx & 0xff;
	service->cmd_tag++;

	service->cmds[idx].tag = tag;
	service->cmds[idx].rxbuf = rxbuf;
	service->cmds[idx].txbuf = txbuf;
	service->cmds[idx].rxbuf_dma = rxbuf_dma;
	service->cmds[idx].txbuf_dma = txbuf_dma;
	service->cmds[idx].rxlen = output_len;
	service->cmds[idx].txlen = payload_len;
	service->cmds[idx].free_on_ack = false;
	service->cmds[idx].done = false;
	service->cmds[idx].completion = &completion;
	init_completion(&completion);

	spin_unlock_irqrestore(&service->lock, flags);

	ret = afk_send_epic(service->ep, service->channel, tag,
			    EPIC_TYPE_COMMAND, EPIC_CAT_COMMAND, type, &cmd,
			    sizeof(cmd));
	if (ret)
		goto err_free_cmd;

	ret = wait_for_completion_timeout(&completion,
					  msecs_to_jiffies(MSEC_PER_SEC));

	if (ret <= 0) {
		spin_lock_irqsave(&service->lock, flags);
		/*
		 * Check again while we're inside the lock to make sure
		 * the command wasn't completed just after
		 * wait_for_completion_timeout returned.
		 */
		if (!service->cmds[idx].done) {
			service->cmds[idx].completion = NULL;
			service->cmds[idx].free_on_ack = true;
			spin_unlock_irqrestore(&service->lock, flags);
			return -ETIMEDOUT;
		}
		spin_unlock_irqrestore(&service->lock, flags);
	}

	ret = 0;
	if (retcode)
		*retcode = service->cmds[idx].retcode;
	if (output && output_len)
		memcpy(output, rxbuf, output_len);

err_free_cmd:
	spin_lock_irqsave(&service->lock, flags);
	bitmap_release_region(service->cmd_map, idx, 0);
err_unlock:
	spin_unlock_irqrestore(&service->lock, flags);
	dma_free_coherent(ep->dcp->dev, payload_len, txbuf, txbuf_dma);
err_free_rxbuf:
	dma_free_coherent(ep->dcp->dev, output_len, rxbuf, rxbuf_dma);
	return ret;
}

static int __afk_service_call(struct apple_epic_service *service, u16 group,
			      u32 command, const void *data, size_t data_len,
			      size_t data_pad, void *output, size_t output_len,
			      size_t output_pad, u32 *wire_status,
			      unsigned long timeout)
{
	struct epic_service_call *call;
	void *bfr;
	size_t bfr_len = max(data_len + data_pad, output_len + output_pad) +
			 sizeof(*call);
	int ret;
	u32 request_status = 0, retcode = 0;
	u32 retlen;

	bfr = kzalloc(bfr_len, GFP_KERNEL);
	if (!bfr)
		return -ENOMEM;

	call = bfr;

	memset(call, 0, sizeof(*call));
	call->group = cpu_to_le16(group);
	call->command = cpu_to_le32(command);
	call->data_len = cpu_to_le32(data_len + data_pad);
	call->magic = cpu_to_le32(EPIC_SERVICE_CALL_MAGIC);

	memcpy(bfr + sizeof(*call), data, data_len);

	if (service->ep->compact) {
		request_status = wire_status ? *wire_status :
			afk_compact_next_status(service);
		ret = afk_send_compact_command(service,
					       EPIC_SUBTYPE_STD_SERVICE,
					       bfr, bfr_len, bfr, bfr_len,
					       request_status, &retcode, timeout);
	} else {
		ret = afk_send_command(service, EPIC_SUBTYPE_STD_SERVICE, bfr,
				       bfr_len, bfr, bfr_len, &retcode);
	}
	if (ret)
		goto out;
	if (wire_status)
		*wire_status = retcode;
	if (service->ep->compact && retcode != request_status) {
		ret = -EIO;
		goto out;
	}
	if (!service->ep->compact && retcode) {
		ret = -EINVAL;
		goto out;
	}
	if (le32_to_cpu(call->magic) != EPIC_SERVICE_CALL_MAGIC ||
	    le16_to_cpu(call->group) != group ||
	    le32_to_cpu(call->command) != command) {
		ret = -EINVAL;
		goto out;
	}

	retlen = le32_to_cpu(call->data_len);
	if (output_len < retlen)
		retlen = output_len;
	if (output && output_len) {
		memset(output, 0, output_len);
		memcpy(output, bfr + sizeof(*call), retlen);
	}

out:
	kfree(bfr);
	return ret;
}

int afk_service_call(struct apple_epic_service *service, u16 group, u32 command,
		     const void *data, size_t data_len, size_t data_pad,
		     void *output, size_t output_len, size_t output_pad)
{
	return __afk_service_call(service, group, command, data, data_len,
				  data_pad, output, output_len, output_pad,
				  NULL, msecs_to_jiffies(5000));
}

int afk_service_call_status(struct apple_epic_service *service, u16 group,
			    u32 command, const void *data, size_t data_len,
			    size_t data_pad, void *output, size_t output_len,
			    size_t output_pad, u32 *status,
			    unsigned long timeout)
{
	if (!service->ep->compact)
		return -EOPNOTSUPP;

	return __afk_service_call(service, group, command, data, data_len,
				  data_pad, output, output_len, output_pad,
				  status, timeout);
}

#if IS_ENABLED(CONFIG_DRM_APPLE_DEBUG)

#define AFK_DEBUGFS_MAX_REPLY 8192

static ssize_t service_call_write_file(struct file *file, const char __user *user_buf,
				       size_t count, loff_t *ppos)
{
	struct apple_epic_service *srv = file->private_data;
	void *buf;
	int ret;
	struct {
		u32 group;
		u32 command;
	} call_info;

	if (count < sizeof(call_info))
		return -EINVAL;
	if (!srv->debugfs.scratch) {
		srv->debugfs.scratch = \
			devm_kzalloc(srv->ep->dcp->dev, AFK_DEBUGFS_MAX_REPLY, GFP_KERNEL);
		if (!srv->debugfs.scratch)
			return -ENOMEM;
	}

	ret = copy_from_user(&call_info, user_buf, sizeof(call_info));
	if (ret == sizeof(call_info))
		return -EFAULT;
	user_buf += sizeof(call_info);
	count -= sizeof(call_info);

	buf = kmalloc(count, GFP_KERNEL);
	if (!buf)
		return -ENOMEM;
	ret = copy_from_user(buf, user_buf, count);
	if (ret == count) {
		kfree(buf);
		return -EFAULT;
	}

	memset(srv->debugfs.scratch, 0, AFK_DEBUGFS_MAX_REPLY);
	dma_mb();

	ret = afk_service_call(srv, call_info.group, call_info.command, buf, count, 0,
			       srv->debugfs.scratch, AFK_DEBUGFS_MAX_REPLY, 0);
	kfree(buf);

	if (ret < 0)
		return ret;

	return count + sizeof(call_info);
}

static ssize_t service_call_read_file(struct file *file, char __user *user_buf,
				      size_t count, loff_t *ppos)
{
	struct apple_epic_service *srv = file->private_data;

	if (!srv->debugfs.scratch)
		return -EINVAL;

	return simple_read_from_buffer(user_buf, count, ppos,
				       srv->debugfs.scratch, AFK_DEBUGFS_MAX_REPLY);
}

static const struct file_operations service_call_fops = {
	.open = simple_open,
	.write = service_call_write_file,
	.read = service_call_read_file,
};

static ssize_t service_raw_call_write_file(struct file *file, const char __user *user_buf,
					   size_t count, loff_t *ppos)
{
	struct apple_epic_service *srv = file->private_data;
	u32 retcode;
	int ret;

	if (!srv->debugfs.scratch) {
		srv->debugfs.scratch = \
			devm_kzalloc(srv->ep->dcp->dev, AFK_DEBUGFS_MAX_REPLY, GFP_KERNEL);
		if (!srv->debugfs.scratch)
			return -ENOMEM;
	}

	memset(srv->debugfs.scratch, 0, AFK_DEBUGFS_MAX_REPLY);
	ret = copy_from_user(srv->debugfs.scratch, user_buf, count);
	if (ret == count)
		return -EFAULT;

	ret = afk_send_command(srv, EPIC_SUBTYPE_STD_SERVICE, srv->debugfs.scratch, count,
			       srv->debugfs.scratch, AFK_DEBUGFS_MAX_REPLY, &retcode);
	if (ret < 0)
		return ret;
	if (retcode)
		return -EINVAL;

	return count;
}

static ssize_t service_raw_call_read_file(struct file *file, char __user *user_buf,
					  size_t count, loff_t *ppos)
{
	struct apple_epic_service *srv = file->private_data;

	if (!srv->debugfs.scratch)
		return -EINVAL;

	return simple_read_from_buffer(user_buf, count, ppos,
				       srv->debugfs.scratch, AFK_DEBUGFS_MAX_REPLY);
}

static const struct file_operations service_raw_call_fops = {
	.open = simple_open,
	.write = service_raw_call_write_file,
	.read = service_raw_call_read_file,
};

static void afk_populate_service_debugfs(struct apple_epic_service *srv)
{
	if (!srv->ep->debugfs_entry || !srv->ops)
		return;

	if (!strcmp(srv->ops->name, "DCPAVAudioInterface") ||
	    !strcmp(srv->ops->name, "dcpav-audio-interface-")) {
		srv->debugfs.entry = debugfs_create_dir(srv->ops->name,
							srv->ep->debugfs_entry);
		debugfs_create_file("call", 0600, srv->debugfs.entry, srv,
				&service_call_fops);
		debugfs_create_file("raw_call", 0600, srv->debugfs.entry, srv,
				&service_raw_call_fops);
	}
}

static void afk_remove_service_debugfs(struct apple_epic_service *srv)
{
	if (srv->debugfs.entry) {
		debugfs_remove_recursive(srv->debugfs.entry);
		srv->debugfs.entry = NULL;
	}
}

#endif
