/*
 * SPDX-License-Identifier: GPL-2.0 OR MIT
 *
 * Apple DockChannel HID transport driver
 *
 * Copyright The Asahi Linux Contributors
 */
#include <linux/bitfield.h>
#include <linux/delay.h>
#include <linux/device.h>
#include <linux/dma-mapping.h>
#include <linux/firmware.h>
#include <linux/gpio/consumer.h>
#include <linux/hid.h>
#include <linux/mutex.h>
#include <linux/platform_device.h>
#include <linux/slab.h>
#include <linux/sizes.h>
#include <linux/soc/apple/dockchannel.h>
#include <linux/soc/apple/rtkit.h>
#include <linux/spinlock.h>
#include <linux/string.h>
#include <linux/unaligned.h>
#include <linux/of.h>
#include "../hid-ids.h"

#define COMMAND_TIMEOUT_MS 1000
#define START_TIMEOUT_MS 2000

#define MAX_INTERFACES 16

/* Complete frame, including the largest aligned body and its checksum. */
#define MAX_PKT_SIZE (8 + 0xfffc + 4)
#define DCHID_RING_SIZE SZ_2M
#define DCHID_RING_HEADER_SIZE 8

#define DCHID_CHANNEL_NOTIFY 0x00
#define DCHID_CHANNEL_CMD 0x11
#define DCHID_CHANNEL_REPORT 0x12

struct dchid_hdr {
	u8 hdr_len;
	u8 channel;
	__le16 length;
	u8 seq;
	u8 iface;
	__le16 pad;
} __packed;

#define IFACE_COMM 0

#define FLAGS_GROUP GENMASK(7, 6)
#define FLAGS_REQ GENMASK(5, 0)

#define REQ_SET_REPORT 0
#define REQ_GET_REPORT 1

struct dchid_subhdr {
	u8 flags;
	u8 unk;
	__le16 length;
	__le32 retcode;
} __packed;

#define EVENT_GPIO_CMD	0xa0
#define EVENT_INIT	0xf0
#define EVENT_READY	0xf1

struct dchid_init_hdr {
	u8 type;
	u8 unk1;
	u8 unk2;
	u8 iface;
	char name[16];
	u8 more_packets;
	u8 unkpad;
} __packed;

#define INIT_HID_DESCRIPTOR	0
#define INIT_GPIO_REQUEST	1
#define INIT_TERMINATOR		2
#define INIT_PRODUCT_NAME	7

#define CMD_RESET_INTERFACE 0x40
#define CMD_REGISTER_RING 0x91
#define CMD_SEND_FIRMWARE 0x95
#define CMD_ENABLE_INTERFACE 0xb4
#define CMD_ACK_GPIO_CMD 0xa1

struct dchid_init_block_hdr {
	__le16 type;
	__le16 length;
} __packed;

#define MAX_GPIO_NAME 32

struct dchid_gpio_request {
	__le16 unk;
	__le16 id;
	char name[MAX_GPIO_NAME];
} __packed;

struct dchid_gpio_cmd {
	u8 type;
	u8 iface;
	u8 gpio;
	u8 unk;
	u8 cmd;
} __packed;

struct dchid_gpio_ack {
	u8 type;
	__le32 retcode;
	u8 cmd[];
} __packed;

#define STM_REPORT_ID		0x10
#define STM_REPORT_SERIAL	0x11
#define STM_REPORT_KEYBTYPE	0x14

struct dchid_stm_id {
	u8 unk;
	u16 vendor_id;
	u16 product_id;
	u16 version_number;
	u8 unk2;
	u8 unk3;
	u8 keyboard_type;
	u8 serial_length;
	/* Serial follows, but we grab it with a different report. */
} __packed;

#define FW_MAGIC 0x46444948
#define FW_VER 1

struct fw_header {
	__le32 magic;
	__le32 version;
	__le32 hdr_length;
	__le32 data_length;
	__le32 iface_offset;
} __packed;

struct dchid_work {
	struct work_struct work;
	struct dchid_iface *iface;

	struct dchid_hdr hdr;
	u8 data[];
};

struct dchid_iface {
	struct dockchannel_hid *dchid;
	struct hid_device *hid;
	struct workqueue_struct *wq;

	bool creating;
	struct work_struct create_work;

	int index;
	const char *name;
	struct device_node *of_node;

	uint8_t tx_seq;
	bool deferred;
	bool starting;
	bool open;
	struct completion ready;

	void *hid_desc;
	size_t hid_desc_len;

	struct gpio_desc *gpio;
	char gpio_name[MAX_GPIO_NAME];
	int gpio_id;

	struct mutex out_mutex;
	spinlock_t out_lock;
	int out_error;
	bool out_pending;
	bool failed;
	u32 out_flags;
	int out_report;
	u32 retcode;
	void *resp_buf;
	size_t resp_size;
	struct completion out_complete;

	u32 keyboard_layout_id;

	/* A registration ACK does not relinquish coprocessor DMA ownership. */
	void *firmware;
	dma_addr_t firmware_dma;
	size_t firmware_size;
};

struct dockchannel_hid {
	struct device *dev;
	struct dockchannel *dc;
	struct device_link *helper_link;

	struct mutex ring_mutex;
	struct mutex tx_mutex;
	bool stopping;
	bool failed;
	bool use_ring;
	bool ring_registered;
	void *ring;
	dma_addr_t ring_dma;
	u32 ring_read;
	bool id_ready;
	struct dchid_stm_id device_id;
	char serial[64];

	struct dchid_iface *comm;
	struct dchid_iface *ifaces[MAX_INTERFACES];

	u8 pkt_buf[MAX_PKT_SIZE];
	size_t pkt_used;
	size_t pkt_size;

	/* Workqueue to asynchronously create HID devices */
	struct workqueue_struct *new_iface_wq;
};

static ssize_t apple_layout_id_show(struct device *dev,
				       struct device_attribute *attr,
				       char *buf)
{
	struct hid_device *hdev = to_hid_device(dev);
	struct dchid_iface *iface = hdev->driver_data;

	return scnprintf(buf, PAGE_SIZE, "%d\n", iface->keyboard_layout_id);
}

static DEVICE_ATTR_RO(apple_layout_id);

static struct dchid_iface *
dchid_get_interface(struct dockchannel_hid *dchid, int index, const char *name)
{
	struct dchid_iface *iface;

	if (index < 0 || index >= MAX_INTERFACES) {
		dev_err(dchid->dev, "Interface index %d out of range\n", index);
		return NULL;
	}

	if (dchid->ifaces[index])
		return dchid->ifaces[index];

	iface = devm_kzalloc(dchid->dev, sizeof(struct dchid_iface), GFP_KERNEL);
	if (!iface)
		return NULL;

	iface->index = index;
	iface->name = devm_kstrdup(dchid->dev, name, GFP_KERNEL);
	if (!iface->name)
		return NULL;
	iface->dchid = dchid;
	iface->out_report= -1;
	init_completion(&iface->out_complete);
	init_completion(&iface->ready);
	mutex_init(&iface->out_mutex);
	spin_lock_init(&iface->out_lock);
	if (index != IFACE_COMM) {
		iface->of_node = of_get_child_by_name(dchid->dev->of_node, name);
		if (!iface->of_node) {
			dev_warn(dchid->dev, "No OF node for subdevice %s, ignoring\n", name);
			return NULL;
		}
	}

	iface->wq = alloc_ordered_workqueue("dchid-%s", WQ_MEM_RECLAIM, iface->name);
	if (!iface->wq) {
		of_node_put(iface->of_node);
		return NULL;
	}

	smp_store_release(&dchid->ifaces[index], iface);
	return iface;
}

static u32 dchid_checksum(const void *p, size_t length)
{
	u32 sum = 0;

	while (length >= 4) {
		sum += get_unaligned_le32(p);
		p += 4;
		length -= 4;
	}

	WARN_ON_ONCE(length);
	return sum;
}

static void dchid_cancel_commands(struct dockchannel_hid *dchid, int error)
{
	struct dchid_iface *iface;
	unsigned long flags;
	int i;

	for (i = 0; i < MAX_INTERFACES; i++) {
		iface = smp_load_acquire(&dchid->ifaces[i]);
		if (!iface)
			continue;
		spin_lock_irqsave(&iface->out_lock, flags);
		iface->out_error = error;
		iface->out_pending = false;
		complete_all(&iface->out_complete);
		spin_unlock_irqrestore(&iface->out_lock, flags);
		complete_all(&iface->ready);
	}
}

static void dchid_fail(struct dockchannel_hid *dchid, int error)
{
	/* No speculative resynchronization after loss of a frame boundary. */
	WRITE_ONCE(dchid->failed, true);
	dchid_cancel_commands(dchid, error);
}

static int dchid_send(struct dchid_iface *iface, u32 flags, void *msg, size_t size)
{
	struct dockchannel_hid *dchid = iface->dchid;
	u32 checksum = 0xffffffff;
	__le32 wire_checksum;
	size_t wsize = round_down(size, 4);
	size_t tsize = size - wsize;
	int ret;
	struct {
		struct dchid_hdr hdr;
		struct dchid_subhdr sub;
	} __packed h = {};

	if (round_up(size, 4) > 0xfffc - sizeof(h.sub))
		return -EMSGSIZE;

	h.hdr.hdr_len = sizeof(h.hdr);
	h.hdr.channel = DCHID_CHANNEL_CMD;
	h.hdr.length = cpu_to_le16(round_up(size, 4) + sizeof(h.sub));
	h.hdr.seq = iface->tx_seq;
	h.hdr.iface = iface->index;
	h.sub.flags = flags;
	h.sub.length = cpu_to_le16(size);

	/* Interface locks do not serialize the shared byte stream. */
	mutex_lock(&dchid->tx_mutex);
	if (READ_ONCE(dchid->stopping) || READ_ONCE(dchid->failed)) {
		ret = -ESHUTDOWN;
		goto unlock;
	}

	ret = dockchannel_send(dchid->dc, &h, sizeof(h));
	if (ret < 0)
		goto failed;
	checksum -= dchid_checksum(&h, sizeof(h));

	ret = dockchannel_send(dchid->dc, msg, wsize);
	if (ret < 0)
		goto failed;
	checksum -= dchid_checksum(msg, wsize);

	if (tsize) {
		u8 tail[4] = {};

		memcpy(tail, msg + wsize, tsize);
		ret = dockchannel_send(dchid->dc, tail, sizeof(tail));
		if (ret < 0)
			goto failed;
		checksum -= dchid_checksum(tail, sizeof(tail));
	}

	wire_checksum = cpu_to_le32(checksum);
	ret = dockchannel_send(dchid->dc, &wire_checksum, sizeof(wire_checksum));
	if (ret < 0)
		goto failed;
	ret = 0;
	goto unlock;

failed:
	dchid_fail(dchid, ret);
unlock:
	mutex_unlock(&dchid->tx_mutex);
	return ret;
}

static int dchid_cmd(struct dchid_iface *iface, u32 type, u32 req,
		     void *data, size_t size, void *resp_buf, size_t resp_size)
{
	struct dockchannel_hid *dchid = iface->dchid;
	unsigned long flags;
	int ret, report_id;

	if (!size || size > 0xfffc - sizeof(struct dchid_subhdr))
		return -EINVAL;
	report_id = *(u8 *)data;

	mutex_lock(&iface->out_mutex);
	spin_lock_irqsave(&iface->out_lock, flags);
	if (READ_ONCE(dchid->stopping) || READ_ONCE(dchid->failed) || iface->failed) {
		ret = -ESHUTDOWN;
		goto unlock;
	}

	iface->out_report = report_id;
	iface->out_flags = FIELD_PREP(FLAGS_GROUP, type) | FIELD_PREP(FLAGS_REQ, req);
	iface->resp_buf = resp_buf;
	iface->resp_size = resp_size;
	iface->out_error = 0;
	iface->retcode = 0;
	iface->out_pending = true;
	reinit_completion(&iface->out_complete);
	spin_unlock_irqrestore(&iface->out_lock, flags);

	ret = dchid_send(iface, iface->out_flags, data, size);
	if (!ret && !wait_for_completion_timeout(&iface->out_complete,
						msecs_to_jiffies(COMMAND_TIMEOUT_MS)))
		ret = -ETIMEDOUT;

	spin_lock_irqsave(&iface->out_lock, flags);
	if (ret == -ETIMEDOUT) {
		/*
		 * The wire sequence is only eight bits. Do not reuse an uncertain
		 * command identity after timeout, even after sequence wraparound.
		 */
		iface->failed = true;
		dev_err(dchid->dev, "Report 0x%x to iface %d (%s) timed out\n",
			report_id, iface->index, iface->name);
	}
	if (!ret)
		ret = iface->out_error ?: (iface->retcode ? -EIO : iface->resp_size);
	iface->tx_seq++;
	iface->out_pending = false;
	iface->out_report = -1;
	iface->resp_buf = NULL;
	iface->resp_size = 0;
unlock:
	spin_unlock_irqrestore(&iface->out_lock, flags);
	mutex_unlock(&iface->out_mutex);
	return ret;
}

static int dchid_comm_cmd(struct dockchannel_hid *dchid, void *cmd, size_t size)
{
	return dchid_cmd(dchid->comm, HID_FEATURE_REPORT, REQ_SET_REPORT, cmd, size, NULL, 0);
}

static int dchid_enable_interface(struct dchid_iface *iface)
{
	u8 msg[] = { CMD_ENABLE_INTERFACE, iface->index };

	return dchid_comm_cmd(iface->dchid, msg, sizeof(msg));
}

static int dchid_reset_interface(struct dchid_iface *iface, int state)
{
	u8 msg[] = { CMD_RESET_INTERFACE, 1, iface->index, state };
	u8 msg2[] = { CMD_RESET_INTERFACE, 2, iface->index, state, 0, 0, 0, 0, 0 };
	int ret;

	if (!iface->dchid->use_ring)
		return dchid_comm_cmd(iface->dchid, msg, sizeof(msg));

	/*
	 * T8132 boot capture: state 0 then 2, each with byte 4 equal to 0
	 * then 1. This is only the observed provisioning sequence, not a
	 * contract for suspend, wake, or arbitrary interface power changes.
	 */
	ret = dchid_comm_cmd(iface->dchid, msg2, sizeof(msg2));
	if (ret < 0)
		return ret;
	msg2[4] = 1;
	return dchid_comm_cmd(iface->dchid, msg2, sizeof(msg2));
}

static int dchid_register_ring(struct dockchannel_hid *dchid)
{
	struct {
		u8 cmd;
		u8 reserved[2];
		__le64 addr;
		__le32 size;
	} __packed msg = {
		.cmd = CMD_REGISTER_RING,
		.addr = cpu_to_le64(dchid->ring_dma),
		.size = cpu_to_le32(DCHID_RING_SIZE),
	};
	/* Observed in both macOS and standalone T8132 bringup before 0x91. */
	u8 prepare[] = { 0xc1, 0x02 };
	int ret = 0;

	if (!dchid->use_ring)
		return 0;

	mutex_lock(&dchid->ring_mutex);
	if (!dchid->ring_registered) {
		ret = dchid_comm_cmd(dchid, prepare, sizeof(prepare));
		if (ret < 0) {
			dchid_fail(dchid, ret);
			mutex_unlock(&dchid->ring_mutex);
			return ret;
		}
		/* The registration ACK itself arrives through the ring. */
		dma_wmb();
		WRITE_ONCE(dchid->ring_registered, true);
		ret = dchid_comm_cmd(dchid, &msg, sizeof(msg));
		if (ret < 0)
			dchid_fail(dchid, ret);
	}
	mutex_unlock(&dchid->ring_mutex);
	return ret;
}

static int dchid_send_firmware(struct dchid_iface *iface, void *firmware, size_t size)
{
	struct {
		u8 cmd;
		u8 unk1;
		u8 unk2;
		u8 iface;
		__le64 addr;
		__le32 size;
	} __packed msg = {
		.cmd = CMD_SEND_FIRMWARE,
		.unk1 = 2,
		.unk2 = 0,
		.iface = iface->index,
		.size = cpu_to_le32(size),
	};

	if (iface->firmware)
		return -EALREADY;
	iface->firmware = dma_alloc_coherent(iface->dchid->dev, size,
					    &iface->firmware_dma, GFP_KERNEL);
	if (!iface->firmware)
		return -ENOMEM;
	iface->firmware_size = size;

	msg.addr = cpu_to_le64(iface->firmware_dma);
	memcpy(iface->firmware, firmware, size);
	dma_wmb();

	return dchid_comm_cmd(iface->dchid, &msg, sizeof(msg));
}

static int dchid_get_firmware(struct dchid_iface *iface, void **firmware, size_t *size)
{
	int ret;
	const char *fw_name;
	const struct firmware *fw;
	const struct fw_header *hdr;
	u32 hdr_length, data_length, iface_offset;
	u8 *fw_data;

	ret = of_property_read_string(iface->of_node, "firmware-name", &fw_name);
	if (ret) {
		/* Firmware is only for some devices */
		*firmware = NULL;
		*size = 0;
		return 0;
	}

	ret = request_firmware(&fw, fw_name, iface->dchid->dev);
	if (ret)
		return ret;

	if (fw->size < sizeof(*hdr)) {
		ret = -EINVAL;
		goto done;
	}
	hdr = (const struct fw_header *)fw->data;
	hdr_length = le32_to_cpu(hdr->hdr_length);
	data_length = le32_to_cpu(hdr->data_length);
	iface_offset = le32_to_cpu(hdr->iface_offset);

	if (le32_to_cpu(hdr->magic) != FW_MAGIC || le32_to_cpu(hdr->version) != FW_VER ||
	    hdr_length < sizeof(*hdr) || hdr_length > fw->size ||
	    data_length > fw->size - hdr_length || !data_length ||
	    iface_offset >= data_length) {
		dev_warn(iface->dchid->dev, "%s: invalid firmware header\n",
			 fw_name);
		ret = -EINVAL;
		goto done;
	}

	fw_data = kmemdup(fw->data + hdr_length, data_length, GFP_KERNEL);
	if (!fw_data) {
		ret = -ENOMEM;
		goto done;
	}

	if (iface_offset)
		fw_data[iface_offset] = iface->index;

	*firmware = fw_data;
	*size = data_length;

done:
	release_firmware(fw);
	return ret;
}

static int dchid_request_gpio(struct dchid_iface *iface)
{
	char prop_name[MAX_GPIO_NAME + 16];

	/*
	 * The older cmd-3 GPIO pulse has not been established for the T8132
	 * SMC reset keys. Do not substitute a guessed software pulse.
	 */
	if (iface->dchid->use_ring) {
		dev_err_once(iface->dchid->dev, "T8132 SMC reset GPIO operation is not established\n");
		return -EOPNOTSUPP;
	}

	if (iface->gpio)
		return 0;

	dev_info(iface->dchid->dev, "Requesting GPIO %s#%d: %s\n",
		 iface->name, iface->gpio_id, iface->gpio_name);

	snprintf(prop_name, sizeof(prop_name), "apple,%s", iface->gpio_name);

	iface->gpio = devm_gpiod_get_index(iface->dchid->dev, prop_name, 0, GPIOD_OUT_LOW);

	if (IS_ERR_OR_NULL(iface->gpio)) {
		dev_err(iface->dchid->dev, "Failed to request GPIO %s-gpios\n", prop_name);
		iface->gpio = NULL;
		return -1;
	}

	return 0;
}

static int dchid_start_interface(struct dchid_iface *iface)
{
	void *fw = NULL;
	size_t size;
	int ret;

	if (iface->starting) {
		dev_warn(iface->dchid->dev, "Interface %s is already starting", iface->name);
		return -EINPROGRESS;
	}

	dev_info(iface->dchid->dev, "Starting interface %s\n", iface->name);

	iface->starting = true;
	ret = dchid_register_ring(iface->dchid);
	if (ret < 0)
		goto err;


	/* Look to see if we need firmware */
	ret = dchid_get_firmware(iface, &fw, &size);
	if (ret < 0)
		goto err;

	/* T8132's proven startup has no host GPIO operations. */
	if (!iface->dchid->use_ring && iface->gpio_id) {
		ret = dchid_request_gpio(iface);
		if (ret < 0)
			goto err;
	}

	/* Only multi-touch has firmware */
	if (fw && size) {

		/* Send firmware to the device */
		dev_info(iface->dchid->dev, "Sending firmware for %s\n", iface->name);
		ret = dchid_send_firmware(iface, fw, size);
		if (ret < 0) {
			dev_err(iface->dchid->dev, "Failed to send %s firmware\n", iface->name);
			goto err;
		}

		/* After loading firmware, multi-touch needs a reset */
		dev_info(iface->dchid->dev, "Resetting %s\n", iface->name);
		ret = dchid_reset_interface(iface, 0);
		if (ret < 0)
			goto err;
		ret = dchid_reset_interface(iface, 2);
		if (ret < 0)
			goto err;
	}

	kfree(fw);
	return 0;

err:
	kfree(fw);
	iface->starting = false;
	return ret;
}

static int dchid_start(struct hid_device *hdev)
{
	struct dchid_iface *iface = hdev->driver_data;

	if (iface->keyboard_layout_id) {
		int ret = device_create_file(&hdev->dev, &dev_attr_apple_layout_id);
		if (ret) {
			dev_warn(iface->dchid->dev, "Failed to create apple_layout_id: %d", ret);
			iface->keyboard_layout_id = 0;
		}
	}

	return 0;
};

static void dchid_stop(struct hid_device *hdev)
{
	struct dchid_iface *iface = hdev->driver_data;

	if (iface->keyboard_layout_id)
		device_remove_file(&hdev->dev, &dev_attr_apple_layout_id);
}

static int dchid_open(struct hid_device *hdev)
{
	struct dchid_iface *iface = hdev->driver_data;
	int ret;

	if (READ_ONCE(iface->dchid->stopping) || READ_ONCE(iface->dchid->failed))
		return -ESHUTDOWN;

	if (!completion_done(&iface->ready)) {
		ret = dchid_start_interface(iface);
		if (ret < 0)
			return ret;

		if (!wait_for_completion_timeout(&iface->ready, msecs_to_jiffies(START_TIMEOUT_MS))) {
			dev_err(iface->dchid->dev, "iface %s start timed out\n", iface->name);
			return -ETIMEDOUT;
		}
	}

	if (READ_ONCE(iface->dchid->stopping) || READ_ONCE(iface->dchid->failed))
		return -ESHUTDOWN;

	WRITE_ONCE(iface->open, true);
	return 0;
}

static void dchid_close(struct hid_device *hdev)
{
	struct dchid_iface *iface = hdev->driver_data;

	WRITE_ONCE(iface->open, false);
}

static int dchid_parse(struct hid_device *hdev)
{
	struct dchid_iface *iface = hdev->driver_data;

	return hid_parse_report(hdev, iface->hid_desc, iface->hid_desc_len);
}

/* Note: buf excludes report number! For ease of fetching strings/etc. */
static int dchid_get_report_cmd(struct dchid_iface *iface, u8 reportnum, void *buf, size_t len)
{
	int ret = dchid_cmd(iface, HID_FEATURE_REPORT, REQ_GET_REPORT, &reportnum, 1, buf, len);

	return ret <= 0 ? ret : ret - 1;
}

/* Note: buf includes report number! */
static int dchid_set_report(struct dchid_iface *iface, void *buf, size_t len)
{
	return dchid_cmd(iface, HID_OUTPUT_REPORT, REQ_SET_REPORT, buf, len, NULL, 0);
}

static int dchid_raw_request(struct hid_device *hdev,
				unsigned char reportnum, __u8 *buf, size_t len,
				unsigned char rtype, int reqtype)
{
	struct dchid_iface *iface = hdev->driver_data;

	if (!len)
		return -EINVAL;

	switch (reqtype) {
	case HID_REQ_GET_REPORT:
		buf[0] = reportnum;
		return dchid_cmd(iface, rtype, REQ_GET_REPORT, &reportnum, 1, buf + 1, len - 1);
	case HID_REQ_SET_REPORT:
		return dchid_set_report(iface, buf, len);
	default:
		return -EIO;
	}

	return 0;
}

static struct hid_ll_driver dchid_ll = {
	.start = &dchid_start,
	.stop = &dchid_stop,
	.open = &dchid_open,
	.close = &dchid_close,
	.parse = &dchid_parse,
	.raw_request = &dchid_raw_request,
};

static void dchid_create_interface_work(struct work_struct *ws)
{
	struct dchid_iface *iface = container_of(ws, struct dchid_iface, create_work);
	struct dockchannel_hid *dchid = iface->dchid;
	struct hid_device *hid;
	int ret;

	if (READ_ONCE(dchid->stopping) || READ_ONCE(dchid->failed))
		return;

	if (iface->hid) {
		dev_warn(dchid->dev, "Interface %s already created!\n",
			 iface->name);
		return;
	}

	dev_info(dchid->dev, "New interface %s\n", iface->name);

	/* Start the interface. This is not the entire init process, as firmware is loaded later on device open. */
	ret = dchid_enable_interface(iface);
	if (ret < 0) {
		dev_warn(dchid->dev, "Failed to enable %s: %d\n", iface->name, ret);
		return;
	}

	iface->deferred = false;

	hid = hid_allocate_device();
	if (IS_ERR(hid))
		return;

	snprintf(hid->name, sizeof(hid->name), "Apple MTP %s", iface->name);
	snprintf(hid->phys, sizeof(hid->phys), "%s.%d (%s)",
		 dev_name(dchid->dev), iface->index, iface->name);
	strscpy(hid->uniq, dchid->serial, sizeof(hid->uniq));

	hid->ll_driver = &dchid_ll;
	hid->bus = BUS_HOST;
	hid->vendor = dchid->device_id.vendor_id;
	hid->product = dchid->device_id.product_id;
	hid->version = dchid->device_id.version_number;
	hid->type = HID_TYPE_OTHER;
	if (!strcmp(iface->name, "multi-touch")) {
		hid->type = HID_TYPE_SPI_MOUSE;
	} else if (!strcmp(iface->name, "keyboard")) {
		u32 country_code = 0;

		hid->type = HID_TYPE_SPI_KEYBOARD;

		/*
		 * We have to get the country code from the device tree, since the
		 * device provides no reliable way to get this info.
		 */
		if (!of_property_read_u32(iface->of_node, "hid-country-code", &country_code))
			hid->country = country_code;

		of_property_read_u32(iface->of_node, "apple,keyboard-layout-id",
			&iface->keyboard_layout_id);
	}

	hid->dev.parent = iface->dchid->dev;
	hid->driver_data = iface;

	WRITE_ONCE(iface->hid, hid);

	ret = hid_add_device(hid);
	if (ret < 0) {
		WRITE_ONCE(iface->hid, NULL);
		flush_workqueue(iface->wq);
		hid_destroy_device(hid);
		dev_warn(iface->dchid->dev, "Failed to register hid device %s", iface->name);
	}
}

static int dchid_create_interface(struct dchid_iface *iface)
{
	if (iface->creating || READ_ONCE(iface->dchid->stopping) ||
	    READ_ONCE(iface->dchid->failed))
		return -EBUSY;

	iface->creating = true;
	INIT_WORK(&iface->create_work, dchid_create_interface_work);
	return queue_work(iface->dchid->new_iface_wq, &iface->create_work);
}

static void dchid_handle_descriptor(struct dchid_iface *iface, void *hid_desc, size_t desc_len)
{
	if (iface->hid || iface->creating) {
		dev_warn(iface->dchid->dev, "Tried to initialize already started interface %s!\n",
			 iface->name);
		return;
	}

	devm_kfree(iface->dchid->dev, iface->hid_desc);
	iface->hid_desc = devm_kmemdup(iface->dchid->dev, hid_desc, desc_len, GFP_KERNEL);
	if (!iface->hid_desc)
		return;

	iface->hid_desc_len = desc_len;
}

static void dchid_handle_ready(struct dockchannel_hid *dchid, void *data, size_t length)
{
	struct dchid_iface *iface;
	u8 *pkt = data;
	u8 index;
	int i, ret;

	if (length < 2) {
		dev_err(dchid->dev, "Bad length for ready message: %zu\n", length);
		return;
	}

	index = pkt[1];

	if (index >= MAX_INTERFACES) {
		dev_err(dchid->dev, "Got ready notification for bad iface %d\n", index);
		return;
	}

	iface = dchid->ifaces[index];
	if (!iface) {
		dev_err(dchid->dev, "Got ready notification for unknown iface %d\n", index);
		return;
	}

	dev_info(dchid->dev, "Interface %s is now ready\n", iface->name);
	complete_all(&iface->ready);

	/* When STM is ready, grab global device info */
	if (!strcmp(iface->name, "stm")) {
		ret = dchid_get_report_cmd(iface, STM_REPORT_ID, &dchid->device_id,
					   sizeof(dchid->device_id));
		if (ret != sizeof(dchid->device_id)) {
			dev_err(dchid->dev, "Failed to get device ID from STM: %d\n", ret);
			dchid_fail(dchid, ret < 0 ? ret : -EPROTO);
			return;
		}
		ret = dchid_get_report_cmd(iface, STM_REPORT_SERIAL, dchid->serial,
					   sizeof(dchid->serial) - 1);
		if (ret < 0) {
			dev_warn(iface->dchid->dev, "Failed to get serial from STM!\n");
			dchid->serial[0] = 0;
		}

		dchid->id_ready = true;
		for (i = 0; i < MAX_INTERFACES; i++) {
			if (!dchid->ifaces[i] || !dchid->ifaces[i]->deferred)
				continue;
			dchid_create_interface(dchid->ifaces[i]);
		}
	}
}

static void dchid_handle_init(struct dockchannel_hid *dchid, void *data, size_t length)
{
	struct dchid_init_hdr *hdr = data;
	struct dchid_iface *iface;
	struct dchid_init_block_hdr *blk;
	u16 type, block_len;

	if (length < sizeof(*hdr) || !memchr(hdr->name, 0, sizeof(hdr->name)) ||
	    hdr->iface == IFACE_COMM)
		return;

	iface = dchid_get_interface(dchid, hdr->iface, hdr->name);
	if (!iface)
		return;

	data += sizeof(*hdr);
	length -= sizeof(*hdr);

	while (length >= sizeof(*blk)) {
		blk = data;
		data += sizeof(*blk);
		length -= sizeof(*blk);

		block_len = le16_to_cpu(blk->length);
		type = le16_to_cpu(blk->type);
		if (block_len > length)
			return;

		switch (type) {
		case INIT_HID_DESCRIPTOR:
			if (!block_len)
				return;
			dchid_handle_descriptor(iface, data, block_len);
			break;

		case INIT_GPIO_REQUEST: {
			struct dchid_gpio_request *req = data;

			if (sizeof(*req) > block_len ||
			    !memchr(req->name, 0, sizeof(req->name)))
				return;

			if (iface->gpio_id) {
				dev_err(dchid->dev,
					"Cannot request more than one GPIO per interface!\n");
				break;
			}

			strscpy(iface->gpio_name, req->name, MAX_GPIO_NAME);
			iface->gpio_id = le16_to_cpu(req->id);
			break;
		}

		case INIT_TERMINATOR:
			break;

		case INIT_PRODUCT_NAME: {
			char *product = data;

			if (!block_len || product[block_len - 1] != 0) {
				dev_warn(dchid->dev, "Unterminated product name for %s\n",
					 iface->name);
			} else {
				dev_info(dchid->dev, "Product name for %s: %s\n",
					 iface->name, product);
			}
			break;
		}

		default:
			dev_warn(dchid->dev, "Unknown init packet %d for %s\n",
				 type, iface->name);
			break;
		}

		data += block_len;
		length -= block_len;

		if (type == INIT_TERMINATOR)
			break;
	}

	if (hdr->more_packets || !iface->hid_desc)
		return;

	/* We need to enable STM first, since it'll give us the device IDs */
	if (iface->dchid->id_ready || !strcmp(iface->name, "stm")) {
		dchid_create_interface(iface);
	} else {
		iface->deferred = true;
	}
}

static void dchid_handle_gpio(struct dockchannel_hid *dchid, void *data, size_t length)
{
	struct dchid_gpio_cmd *cmd = data;
	struct dchid_iface *iface;
	u32 retcode = 0xe000f00d; /* Give it a random Apple-style error code */
	struct dchid_gpio_ack *ack;

	if (length < sizeof(*cmd))
		return;

	if (cmd->iface >= MAX_INTERFACES || !(iface = dchid->ifaces[cmd->iface])) {
		dev_err(dchid->dev, "Got GPIO command for bad inteface %d\n", cmd->iface);
		goto err;
	}

	if (dchid_request_gpio(iface) < 0)
		goto err;

	if (!iface->gpio || cmd->gpio != iface->gpio_id) {
		dev_err(dchid->dev, "Got GPIO command for bad GPIO %s#%d\n",
			iface->name, cmd->gpio);
		goto err;
	}

	dev_info(dchid->dev, "GPIO command: %s#%d: %d\n", iface->name, cmd->gpio, cmd->cmd);

	switch (cmd->cmd) {
	case 3:
		/* Pulse.  */
		gpiod_set_value_cansleep(iface->gpio, 1);
		msleep(10); /* Random guess... */
		gpiod_set_value_cansleep(iface->gpio, 0);
		retcode = 0;
		break;
	default:
		dev_err(dchid->dev, "Unknown GPIO command %d\n", cmd->cmd	);
		break;
	}

err:
	/* Ack it */
	ack = kzalloc(sizeof(*ack) + length, GFP_KERNEL);
	if (!ack)
		return;

	ack->type = CMD_ACK_GPIO_CMD;
	ack->retcode = cpu_to_le32(retcode);
	memcpy(ack->cmd, data, length);

	if (dchid_comm_cmd(dchid, ack, sizeof(*ack) + length) < 0)
		dev_err(dchid->dev, "Failed to ACK GPIO command\n");

	kfree(ack);
}

static void dchid_handle_event(struct dockchannel_hid *dchid, void *data, size_t length)
{
	u8 *p = data;

	if (!length)
		return;
	switch (*p) {
	case EVENT_INIT:
		dchid_handle_init(dchid, data, length);
		break;
	case EVENT_READY:
		dchid_handle_ready(dchid, data, length);
		break;
	case EVENT_GPIO_CMD:
		dchid_handle_gpio(dchid, data, length);
		break;
	}
}

static void dchid_handle_report(struct dchid_iface *iface, void *data, size_t length)
{
	struct dockchannel_hid *dchid = iface->dchid;
	struct hid_device *hid = READ_ONCE(iface->hid);

	if (!hid) {
		dev_warn(dchid->dev, "Report received but %s is not initialized!\n", iface->name);
		return;
	}

	if (!READ_ONCE(iface->open))
		return;

	hid_input_report(hid, HID_INPUT_REPORT, data, length, 1);
}

static void dchid_packet_work(struct work_struct *ws)
{
	struct dchid_work *work = container_of(ws, struct dchid_work, work);
	struct dchid_subhdr *shdr = (void *)work->data;
	struct dockchannel_hid *dchid = work->iface->dchid;
	u8 *payload = work->data + sizeof(*shdr);
	size_t length = le16_to_cpu(shdr->length);

	if (!READ_ONCE(dchid->stopping) && !READ_ONCE(dchid->failed)) {
		if (work->hdr.iface == IFACE_COMM)
			dchid_handle_event(dchid, payload, length);
		else
			dchid_handle_report(work->iface, payload, length);
	}
	kfree(work);
}

static void dchid_handle_ack(struct dchid_iface *iface, const struct dchid_hdr *hdr,
			     const struct dchid_subhdr *shdr)
{
	const u8 *payload = (const void *)(shdr + 1);
	size_t length = le16_to_cpu(shdr->length);
	unsigned long flags;

	spin_lock_irqsave(&iface->out_lock, flags);
	if (!iface->out_pending || !length || iface->tx_seq != hdr->seq ||
	    iface->out_flags != shdr->flags || iface->out_report != payload[0])
		goto done;

	if (iface->resp_buf) {
		length = min(length - 1, iface->resp_size);
		memcpy(iface->resp_buf, payload + 1, length);
		iface->resp_size = length + 1;
	} else {
		iface->resp_size = length;
	}
	iface->retcode = le32_to_cpu(shdr->retcode);
	iface->out_pending = false;
	complete(&iface->out_complete);
done:
	spin_unlock_irqrestore(&iface->out_lock, flags);
}

/* Return the complete frame size, or reject a lost framing boundary. */
static int dchid_frame_size(const struct dchid_hdr *hdr)
{
	size_t length = le16_to_cpu(hdr->length);

	if (hdr->hdr_len != sizeof(*hdr) || !IS_ALIGNED(length, 4))
		return -EPROTO;
	return sizeof(*hdr) + length + sizeof(__le32);
}

/*
 * Both receive transports pass complete frames here. Header bytes 6/7 and
 * application padding are opaque, checksum-covered data, not required zeroes.
 */
static int dchid_dispatch(struct dockchannel_hid *dchid, const void *packet,
			  size_t size, bool fifo)
{
	const struct dchid_hdr *hdr = packet;
	const struct dchid_subhdr *shdr = packet + sizeof(*hdr);
	struct dchid_iface *iface;
	struct dchid_work *work;
	size_t length, body;
	int frame_size;

	if (size < sizeof(*hdr))
		return -EPROTO;
	frame_size = dchid_frame_size(hdr);
	if (frame_size < 0 || frame_size != size)
		return -EPROTO;
	if (dchid_checksum(packet, size) != U32_MAX)
		return -EBADMSG;

	body = le16_to_cpu(hdr->length);
	if (hdr->channel == DCHID_CHANNEL_NOTIFY) {
		if (!fifo || body || hdr->iface != IFACE_COMM)
			return -EPROTO;
		/* Not an application ACK: bytes 6/7 are not a retcode. */
		return 1;
	}
	if (hdr->channel != DCHID_CHANNEL_CMD && hdr->channel != DCHID_CHANNEL_REPORT) {
		dev_warn_ratelimited(dchid->dev, "Unknown channel 0x%x\n", hdr->channel);
		return 0;
	}
	if (hdr->iface >= MAX_INTERFACES)
		return -EPROTO;
	iface = smp_load_acquire(&dchid->ifaces[hdr->iface]);
	if (!iface)
		return 0;
	if (body < sizeof(*shdr)) {
		/* A transport status without a report ID cannot satisfy a command. */
		dev_warn_ratelimited(dchid->dev, "Short status on iface %u channel 0x%x\n",
				    hdr->iface, hdr->channel);
		return 0;
	}
	length = le16_to_cpu(shdr->length);
	if (round_up(length, 4) + sizeof(*shdr) != body)
		return -EPROTO;
	if (hdr->channel == DCHID_CHANNEL_CMD) {
		dchid_handle_ack(iface, hdr, shdr);
		return 0;
	}
	if (!length || FIELD_GET(FLAGS_GROUP, shdr->flags) != HID_INPUT_REPORT ||
	    FIELD_GET(FLAGS_REQ, shdr->flags) || le32_to_cpu(shdr->retcode))
		return 0;

	work = kmalloc(sizeof(*work) + sizeof(*shdr) + length, GFP_KERNEL);
	if (!work)
		return -ENOMEM;
	work->hdr = *hdr;
	work->iface = iface;
	memcpy(work->data, shdr, sizeof(*shdr) + length);
	INIT_WORK(&work->work, dchid_packet_work);
	queue_work(iface->wq, &work->work);
	return 0;
}

static void dchid_ring_copy(struct dockchannel_hid *dchid, void *dst,
			    u32 offset, size_t length)
{
	size_t first = min_t(size_t, length,
			     DCHID_RING_SIZE - DCHID_RING_HEADER_SIZE - offset);
	void *data = dchid->ring + DCHID_RING_HEADER_SIZE;

	memcpy(dst, data + offset, first);
	memcpy(dst + first, data, length - first);
}

static int dchid_drain_ring(struct dockchannel_hid *dchid)
{
	const u32 capacity = DCHID_RING_SIZE - DCHID_RING_HEADER_SIZE;
	__le32 *indices = dchid->ring;
	u32 producer, consumer, available;
	int size, ret;

	if (!READ_ONCE(dchid->ring_registered))
		return -EPROTO;

	while (!READ_ONCE(dchid->stopping) && !READ_ONCE(dchid->failed)) {
		producer = le32_to_cpu(READ_ONCE(indices[0]));
		consumer = le32_to_cpu(READ_ONCE(indices[1]));
		if (producer >= capacity || consumer != dchid->ring_read ||
		    !IS_ALIGNED(producer, 4))
			return -EPROTO;
		/* Read only the bytes published by this producer snapshot. */
		dma_rmb();
		available = producer >= consumer ? producer - consumer :
			    capacity - consumer + producer;
		if (available < sizeof(struct dchid_hdr))
			return 0;
		dchid_ring_copy(dchid, dchid->pkt_buf, consumer,
				sizeof(struct dchid_hdr));
		size = dchid_frame_size((void *)dchid->pkt_buf);
		if (size < 0)
			return size;
		if (available < size)
			return 0;
		dchid_ring_copy(dchid, dchid->pkt_buf, consumer, size);
		ret = dchid_dispatch(dchid, dchid->pkt_buf, size, false);
		if (ret < 0)
			return ret;
		consumer += size;
		if (consumer >= capacity)
			consumer -= capacity;
		dchid->ring_read = consumer;
		/* All frame reads must complete before the producer can reuse it. */
		mb();
		WRITE_ONCE(indices[1], cpu_to_le32(consumer));
		cond_resched();
	}
	return 0;
}

static void dchid_handle_packet(void *cookie, size_t avail)
{
	struct dockchannel_hid *dchid = cookie;
	size_t count;
	int ret;

	while (avail && !READ_ONCE(dchid->stopping) && !READ_ONCE(dchid->failed)) {
		count = min(avail, dchid->pkt_size - dchid->pkt_used);
		ret = dockchannel_recv(dchid->dc, dchid->pkt_buf + dchid->pkt_used, count);
		if (ret != count) {
			dchid_fail(dchid, ret < 0 ? ret : -EIO);
			return;
		}
		avail -= count;
		dchid->pkt_used += count;
		if (dchid->pkt_used != dchid->pkt_size)
			continue;
		if (dchid->pkt_size == sizeof(struct dchid_hdr)) {
			ret = dchid_frame_size((void *)dchid->pkt_buf);
			if (ret < 0)
				goto failed;
			dchid->pkt_size = ret;
			continue;
		}
		ret = dchid_dispatch(dchid, dchid->pkt_buf, dchid->pkt_size, true);
		if (ret == 1)
			ret = dchid_drain_ring(dchid);
		if (ret < 0)
			goto failed;
		dchid->pkt_used = 0;
		dchid->pkt_size = sizeof(struct dchid_hdr);
	}

	/*
	 * A producer notification arriving during draining stays in the FIFO.
	 * Rearming its level/threshold IRQ observes it even if it preceded rearm.
	 */
	if (!READ_ONCE(dchid->stopping) && !READ_ONCE(dchid->failed))
		dockchannel_await(dchid->dc, dchid_handle_packet, dchid,
				  dchid->pkt_size - dchid->pkt_used);
	return;
failed:
	dev_err(dchid->dev, "Receive transport failed: %d\n", ret);
	dchid_fail(dchid, ret);
}

static int dockchannel_hid_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct dockchannel_hid *dchid;
	struct device_node *helper;
	struct fwnode_handle *stm;
	struct platform_device *helper_pdev;
	struct property *prop;
	int ret;

	ret = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(64));
	if (ret)
		return ret;

	dchid = devm_kzalloc(dev, sizeof(*dchid), GFP_KERNEL);
	if (!dchid) {
		return -ENOMEM;
	}

	dchid->dev = dev;
	stm = device_get_named_child_node(dev, "stm");
	dchid->id_ready = !stm || !fwnode_device_is_available(stm);
	fwnode_handle_put(stm);
	mutex_init(&dchid->tx_mutex);
	mutex_init(&dchid->ring_mutex);
	dchid->pkt_size = sizeof(struct dchid_hdr);
	dchid->use_ring = of_device_is_compatible(dev->of_node, "apple,t8132-dockchannel-hid");
	platform_set_drvdata(pdev, dchid);

	/*
	 * First make sure all the GPIOs are available, in cased we need to defer.
	 * This is necessary because MTP will request them by name later, and by then
	 * it's too late to defer the probe.
	 */

	for_each_property_of_node(dev->of_node, prop) {
		size_t len = strlen(prop->name);
		struct gpio_desc *gpio;
		char name[MAX_GPIO_NAME + 16];

		if (len < 12 || strncmp("apple,", prop->name, 6) ||
		    strcmp("-gpios", prop->name + len - 6))
			continue;
		if (len - 6 >= sizeof(name))
			return -EINVAL;
		memcpy(name, prop->name, len - 6);
		name[len - 6] = 0;
		gpio = gpiod_get_index(dev, name, 0, GPIOD_ASIS);
		if (IS_ERR(gpio))
			return dev_err_probe(dev, PTR_ERR(gpio), "Failed to get %s\n", prop->name);
		gpiod_put(gpio);
	}

	/*
	 * Make sure we also have the MTP coprocessor available, and
	 * defer probe if the helper hasn't probed yet.
	 */
	helper = of_parse_phandle(dev->of_node, "apple,helper-cpu", 0);
	if (!helper) {
		dev_err(dev, "Missing apple,helper-cpu property");
		return -EINVAL;
	}

	helper_pdev = of_find_device_by_node(helper);
	of_node_put(helper);
	if (!helper_pdev) {
		dev_err(dev, "Failed to find helper device");
		return -EINVAL;
	}

	dchid->helper_link = device_link_add(dev, &helper_pdev->dev,
					     DL_FLAG_AUTOREMOVE_CONSUMER);
	put_device(&helper_pdev->dev);
	if (!dchid->helper_link) {
		dev_err(dev, "Failed to link to helper device");
		return -EINVAL;
	}

	if (dchid->helper_link->supplier->links.status != DL_DEV_DRIVER_BOUND)
		return -EPROBE_DEFER;
	if (!apple_rtkit_helper_is_running(dchid->helper_link->supplier))
		return dev_err_probe(dev, -ESHUTDOWN, "Helper must be restarted before rebinding HID\n");

	/* Now it is safe to begin initializing */
	dchid->dc = dockchannel_init(pdev);
	if (IS_ERR_OR_NULL(dchid->dc)) {
		return PTR_ERR(dchid->dc);
	}
	if (dchid->use_ring) {
		dchid->ring = dma_alloc_coherent(dev, DCHID_RING_SIZE, &dchid->ring_dma,
						 GFP_KERNEL);
		if (!dchid->ring)
			return -ENOMEM;
		memset(dchid->ring, 0, DCHID_RING_SIZE);
	}
	dchid->new_iface_wq = alloc_workqueue("dchid-new", WQ_MEM_RECLAIM | WQ_PERCPU, 0);
	if (!dchid->new_iface_wq) {
		ret = -ENOMEM;
		goto free_ring;
	}

	dchid->comm = dchid_get_interface(dchid, IFACE_COMM, "comm");
	if (!dchid->comm) {
		dev_err(dchid->dev, "Failed to initialize comm interface");
		ret = -ENOMEM;
		goto destroy_wq;
	}

	dev_info(dchid->dev, "Initialized, awaiting packets\n");
	dockchannel_await(dchid->dc, dchid_handle_packet, dchid, sizeof(struct dchid_hdr));

	return 0;

destroy_wq:
	destroy_workqueue(dchid->new_iface_wq);
free_ring:
	if (dchid->ring)
		dma_free_coherent(dev, DCHID_RING_SIZE, dchid->ring, dchid->ring_dma);
	return ret;
}

static void dockchannel_hid_remove(struct platform_device *pdev)
{
	struct dockchannel_hid *dchid = platform_get_drvdata(pdev);
	struct dchid_iface *iface;
	bool dma_stopped;
	int i;

	WRITE_ONCE(dchid->stopping, true);
	dchid_cancel_commands(dchid, -ESHUTDOWN);
	dockchannel_cancel(dchid->dc);

	/* Finish discovery before enumerating interfaces, then join creators. */
	flush_workqueue(dchid->comm->wq);
	destroy_workqueue(dchid->new_iface_wq);
	for (i = 0; i < MAX_INTERFACES; i++) {
		iface = dchid->ifaces[i];
		if (!iface)
			continue;
		destroy_workqueue(iface->wq);
		if (iface->hid) {
			hid_destroy_device(iface->hid);
			iface->hid = NULL;
		}
		mutex_lock(&iface->out_mutex);
		mutex_unlock(&iface->out_mutex);
		of_node_put(iface->of_node);
	}
	mutex_lock(&dchid->tx_mutex);
	mutex_unlock(&dchid->tx_mutex);

	/* RUN-clear alone is not a DMA ownership boundary. */
	dma_stopped = !apple_rtkit_helper_stop(dchid->helper_link->supplier);
	if (!dma_stopped) {
		dev_crit(dchid->dev, "Retaining HID DMA buffers after failed shutdown; reboot required\n");
		get_device(dchid->dev);
		return;
	}
	for (i = 0; i < MAX_INTERFACES; i++) {
		iface = dchid->ifaces[i];
		if (iface && iface->firmware)
			dma_free_coherent(dchid->dev, iface->firmware_size,
					  iface->firmware, iface->firmware_dma);
	}
	if (dchid->ring)
		dma_free_coherent(dchid->dev, DCHID_RING_SIZE, dchid->ring, dchid->ring_dma);
}

static const struct of_device_id dockchannel_hid_of_match[] = {
	{ .compatible = "apple,t8132-dockchannel-hid" },
	{ .compatible = "apple,dockchannel-hid" },
	{},
};
MODULE_DEVICE_TABLE(of, dockchannel_hid_of_match);
MODULE_FIRMWARE("apple/tpmtfw-*.bin");

static struct platform_driver dockchannel_hid_driver = {
	.driver = {
		.name = "dockchannel-hid",
		.of_match_table = dockchannel_hid_of_match,
	},
	.probe = dockchannel_hid_probe,
	.remove = dockchannel_hid_remove,
	.shutdown = dockchannel_hid_remove,
};
module_platform_driver(dockchannel_hid_driver);

MODULE_DESCRIPTION("Apple DockChannel HID transport driver");
MODULE_AUTHOR("Hector Martin <marcan@marcan.st>");
MODULE_LICENSE("Dual MIT/GPL");
