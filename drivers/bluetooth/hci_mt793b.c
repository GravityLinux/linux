// SPDX-License-Identifier: GPL-2.0-only
/* Neo MT793B PCIe Bluetooth transport.
 *
 * Converged IPC structure layout is also documented by hci_bcm4377.c.
 * MediaTek-specific register/queue parameters are native MMIO/DMA observations.
 * This candidate uses bounded polling until MSI routing is qualified. No
 * captured HCI command sequence or pairing keys are replayed.
 */
#include <linux/bitfield.h>
#include <linux/delay.h>
#include <linux/dma-mapping.h>
#include <linux/firmware.h>
#include <linux/iopoll.h>
#include <linux/module.h>
#include <linux/pci.h>
#include <linux/property.h>
#include <linux/slab.h>
#include <linux/workqueue.h>
#include <linux/unaligned.h>
#include <net/bluetooth/bluetooth.h>
#include <net/bluetooth/hci_core.h>

#define MT_FW_NAME "mediatek/mt793b/neo-25G72.bin"
#define MT_ARENA_SIZE SZ_2M
#define MT_RING_COUNT 11
#define MT_ENTRIES 128
#define MT_REG_BOOT_ENABLE 0x33c28
#define MT_REG_FW_LO 0x33810
#define MT_REG_FW_HI 0x33814
#define MT_REG_FW_SIZE 0x33818
#define MT_REG_BOOT_STATUS 0x3381c
#define MT_REG_RTI_STATUS 0x33804
#define MT_REG_RTI_CONTROL 0x33828
#define MT_REG_CONTEXT_LO 0x33830
#define MT_REG_CONTEXT_HI 0x33834
#define MT_REG_WINDOW_LO 0x3383c
#define MT_REG_WINDOW_HI 0x33840
#define MT_REG_WINDOW_SIZE 0x33844
#define MT_REG_DOORBELL 0x19484

struct mt_context {
	__le16 version, size;
	__le32 caps;
	__le64 peripheral_info;
	__le64 completion_heads, transfer_tails, completion_tails, transfer_heads;
	__le16 completions, transfers;
	__le64 control_completion, control_transfer;
	__le16 transfer_entries, completion_entries;
	__le16 transfer_doorbell, completion_doorbell, transfer_msi, completion_msi;
	u8 transfer_header, transfer_footer, completion_header, completion_footer;
	__le16 unknown0, unknown1;
	__le64 scratch;
	__le32 scratch_size, unknown2;
} __packed;
static_assert(sizeof(struct mt_context) == 0x68);

struct mt_entry {
	u8 flags;
	__le16 length;
	u8 reserved;
	__le64 payload;
	__le16 id, extra;
} __packed;
static_assert(sizeof(struct mt_entry) == 16);

struct mt_queue {
	u8 *data;
	dma_addr_t dma;
	u16 capacity, stride, cursor, doorbell;
	u8 footer, scale;
};

struct mt_bt {
	struct pci_dev *pdev;
	void __iomem *bar;
	struct hci_dev *hdev;
	u8 *arena;
	dma_addr_t dma;
	size_t used;
	struct mt_context *ctx;
	__le16 *heads, *tails, *completion_head, *completion_tail;
	struct mt_queue q[MT_RING_COUNT];
	struct sk_buff_head pending;
	struct delayed_work poll;
	bool running;
	u64 tx_packets, rx_packets, malformed;
};

static void *mt_alloc(struct mt_bt *bt, size_t bytes, dma_addr_t *dma)
{
	size_t start = ALIGN(bt->used, 64);
	if (start > MT_ARENA_SIZE || bytes > MT_ARENA_SIZE - start)
		return NULL;
	bt->used = start + bytes;
	*dma = bt->dma + start;
	return bt->arena + start;
}

static int mt_queue_alloc(struct mt_bt *bt, unsigned id, u8 footer, u8 scale,
			 unsigned doorbell)
{
	struct mt_queue *q = &bt->q[id];
	unsigned i;
	q->footer = footer;
	q->scale = scale;
	q->capacity = (footer * 4) << scale;
	q->stride = sizeof(struct mt_entry) + q->capacity;
	q->doorbell = doorbell;
	q->data = mt_alloc(bt, MT_ENTRIES * q->stride, &q->dma);
	if (!q->data)
		return -ENOMEM;
	if (id && !(id & 1)) {
		for (i = 0; i < MT_ENTRIES; i++) {
			struct mt_entry *e = (void *)(q->data + i * q->stride);
			e->id = cpu_to_le16(i);
		}
	}
	return 0;
}

static void mt_doorbell(struct mt_bt *bt, unsigned id)
{
	dma_wmb();
	writel(BIT(bt->q[id].doorbell), bt->bar + MT_REG_DOORBELL);
}

static int mt_push(struct mt_bt *bt, unsigned id, const void *data, size_t length)
{
	struct mt_queue *q = &bt->q[id];
	struct mt_entry *e;
	u16 head = le16_to_cpu(READ_ONCE(bt->heads[id]));
	u16 tail = le16_to_cpu(READ_ONCE(bt->tails[id]));
	u16 next;
	if (head >= MT_ENTRIES || tail >= MT_ENTRIES || length > q->capacity)
		return -EINVAL;
	next = (head + 1) % MT_ENTRIES;
	if (next == tail)
		return -ENOSPC;
	e = (void *)(q->data + head * q->stride);
	memset(e, 0, q->stride);
	e->flags = id ? BIT(1) : 0;
	e->length = cpu_to_le16(length);
	e->id = id ? cpu_to_le16(head) : 0;
	e->extra = id ? cpu_to_le16(0x0200) : 0;
	memcpy((u8 *)e + sizeof(*e), data, length);
	dma_wmb();
	WRITE_ONCE(bt->heads[id], cpu_to_le16(next));
	mt_doorbell(bt, id);
	return 0;
}

static int mt_create_queue(struct mt_bt *bt, unsigned id)
{
	struct mt_queue *q = &bt->q[id];
	u8 message[52] = {};
	u16 wanted;
	unsigned i;
	int ret;
	message[0] = 1;
	message[2] = q->footer;
	message[3] = q->scale;
	put_unaligned_le16(id, message + 4);
	put_unaligned_le16(id, message + 6);
	put_unaligned_le64(q->dma, message + 8);
	put_unaligned_le16(MT_ENTRIES, message + 24);
	put_unaligned_le16(q->doorbell, message + 28);
	put_unaligned_le16(0x59, message + 30);
	put_unaligned_le32(4, message + 32);
	put_unaligned_le32(q->doorbell, message + 36);
	if (id != 3 && id != 4)
		put_unaligned_le32(1000, message + 44);
	ret = mt_push(bt, 0, message, sizeof(message));
	if (ret)
		return ret;
	wanted = le16_to_cpu(READ_ONCE(bt->heads[0]));
	for (i = 0; i < 2000; i++) {
		if (le16_to_cpu(READ_ONCE(bt->tails[0])) == wanted)
			break;
		usleep_range(500, 1000);
	}
	if (i == 2000) {
		dev_err(&bt->pdev->dev, "queue %u create timeout: control %u/%u\n",
			id, wanted, le16_to_cpu(READ_ONCE(bt->tails[0])));
		return -ETIMEDOUT;
	}
	if (!(id & 1)) {
		WRITE_ONCE(bt->heads[id], cpu_to_le16(MT_ENTRIES - 1));
		mt_doorbell(bt, id);
	}
	dev_info(&bt->pdev->dev, "queue %u ready: stride %u doorbell %u\n",
		 id, q->stride, q->doorbell);
	return 0;
}

static int mt_boot(struct mt_bt *bt)
{
	const struct firmware *fw;
	dma_addr_t ctx_dma, heads_dma, tails_dma, ch_dma, ct_dma, info_dma, fw_dma;
	void *fw_buffer;
	u32 status;
	u32 bootstrap_ms;
	unsigned i;
	int ret;
	static const struct { u8 id, footer, scale, doorbell; } queues[] = {
		{ 1, 67, 0, 21 }, { 2, 67, 0, 22 },
		{ 3, 255, 0, 23 }, { 4, 255, 0, 24 },
		{ 5, 129, 1, 25 }, { 6, 129, 1, 26 }, { 8, 129, 1, 27 },
		{ 9, 194, 0, 28 }, { 10, 194, 0, 29 },
	};
	ret = device_property_read_u32(&bt->pdev->dev, "mediatek,bootstrap-delay-ms", &bootstrap_ms);
	if (ret || !bootstrap_ms || bootstrap_ms > 1000)
		return dev_err_probe(&bt->pdev->dev, ret ?: -EINVAL, "missing/invalid board bootstrap delay\n");
	ret = request_firmware(&fw, MT_FW_NAME, &bt->pdev->dev);
	if (ret)
		return ret;
	if (!fw->size || fw->size > SZ_1M) {
		ret = -EINVAL;
		goto release;
	}
	bt->arena = dmam_alloc_coherent(&bt->pdev->dev, MT_ARENA_SIZE, &bt->dma, GFP_KERNEL);
	if (!bt->arena) {
		ret = -ENOMEM;
		goto release;
	}
	memset(bt->arena, 0, MT_ARENA_SIZE);
	bt->ctx = mt_alloc(bt, sizeof(*bt->ctx), &ctx_dma);
	bt->heads = mt_alloc(bt, MT_RING_COUNT * 2, &heads_dma);
	bt->tails = mt_alloc(bt, MT_RING_COUNT * 2, &tails_dma);
	bt->completion_head = mt_alloc(bt, 32, &ch_dma);
	bt->completion_tail = mt_alloc(bt, 32, &ct_dma);
	if (!mt_alloc(bt, 32, &info_dma)) {
		ret = -ENOMEM;
		goto release;
	}
	ret = mt_queue_alloc(bt, 0, 13, 0, 20);
	if (ret)
		goto release;
	bt->ctx->control_transfer = cpu_to_le64(bt->q[0].dma);
	{
		dma_addr_t ack_dma;
		if (!mt_alloc(bt, MT_ENTRIES * 24, &ack_dma)) {
			ret = -ENOMEM;
			goto release;
		}
		bt->ctx->control_completion = cpu_to_le64(ack_dma);
	}
	for (i = 0; i < ARRAY_SIZE(queues); i++) {
		ret = mt_queue_alloc(bt, queues[i].id, queues[i].footer, queues[i].scale, queues[i].doorbell);
		if (ret)
			goto release;
	}
	fw_buffer = mt_alloc(bt, fw->size, &fw_dma);
	if (!fw_buffer) {
		ret = -ENOMEM;
		goto release;
	}
	memcpy(fw_buffer, fw->data, fw->size);
	bt->ctx->peripheral_info = cpu_to_le64(info_dma);
	bt->ctx->transfer_heads = cpu_to_le64(heads_dma);
	bt->ctx->transfer_tails = cpu_to_le64(tails_dma);
	bt->ctx->completion_heads = cpu_to_le64(ch_dma);
	bt->ctx->completion_tails = cpu_to_le64(ct_dma);
	bt->ctx->completions = cpu_to_le16(1);
	bt->ctx->transfers = cpu_to_le16(MT_RING_COUNT);
	bt->ctx->transfer_entries = cpu_to_le16(MT_ENTRIES);
	bt->ctx->completion_entries = cpu_to_le16(MT_ENTRIES);
	bt->ctx->transfer_footer = 13;
	bt->ctx->completion_footer = 2;
	dma_wmb();
	dev_info(&bt->pdev->dev, "firmware %zu bytes, arena %pad used %zu\n", fw->size, &bt->dma, bt->used);
	dev_info(&bt->pdev->dev, "initial boot=%x runtime=%x\n",
		 readl(bt->bar + MT_REG_BOOT_STATUS), readl(bt->bar + MT_REG_RTI_STATUS));
	/* Native-observed MT793B BAR window setup, independent of Wi-Fi fn0. */
	writel(0x7000188a, bt->bar + 0xfe3b4);
	writel(0, bt->bar + 0xb3024);
	writel(1, bt->bar + MT_REG_BOOT_ENABLE);
	writel(lower_32_bits(fw_dma), bt->bar + MT_REG_FW_LO);
	writel(upper_32_bits(fw_dma), bt->bar + MT_REG_FW_HI);
	writel(fw->size, bt->bar + MT_REG_FW_SIZE);
	writel(0x188d1807, bt->bar + 0xfe3b8);
	ret = readl_poll_timeout(bt->bar + 0xc1060, status, status & 1, 100, 5000000);
	if (ret) {
		dev_err(&bt->pdev->dev, "firmware doorbell readiness timeout: %x\n", status);
		goto release;
	}
	writel(BIT(11), bt->bar + MT_REG_DOORBELL);
	ret = readl_poll_timeout(bt->bar + MT_REG_BOOT_STATUS, status, status == 1, 100, 5000000);
	if (ret) {
		dev_err(&bt->pdev->dev, "firmware readiness timeout: %x\n", status);
		goto release;
	}
	/* Download acknowledgement alone does not guarantee runtime readiness.
	 * Honor the live board's Bluetooth bootstrap delay before requesting
	 * runtime. Poll the actual runtime acknowledgements below as well. */
	msleep(bootstrap_ms);
	dev_info(&bt->pdev->dev, "board bootstrap delay %u ms complete\n", bootstrap_ms);
	writel(0x188d1807, bt->bar + 0xfe3b8);
	writel(1, bt->bar + 0xc1260);
	writel(1, bt->bar + MT_REG_RTI_CONTROL);
	writel(BIT(13), bt->bar + MT_REG_DOORBELL);
	ret = readl_poll_timeout(bt->bar + MT_REG_RTI_STATUS, status, status == 1, 100, 5000000);
	if (ret) {
		dev_err(&bt->pdev->dev, "RTI readiness timeout: %x\n", status);
		goto release;
	}
	writel(lower_32_bits(ctx_dma), bt->bar + MT_REG_CONTEXT_LO);
	writel(upper_32_bits(ctx_dma), bt->bar + MT_REG_CONTEXT_HI);
	writel(lower_32_bits(bt->dma), bt->bar + MT_REG_WINDOW_LO);
	writel(upper_32_bits(bt->dma), bt->bar + MT_REG_WINDOW_HI);
	writel(bt->used, bt->bar + MT_REG_WINDOW_SIZE);
	writel(2, bt->bar + MT_REG_RTI_CONTROL);
	writel(BIT(13), bt->bar + MT_REG_DOORBELL);
	ret = readl_poll_timeout(bt->bar + MT_REG_RTI_STATUS, status, status == 2, 100, 5000000);
	if (ret) {
		dev_err(&bt->pdev->dev, "context acceptance timeout: %x\n", status);
		goto release;
	}
	dev_info(&bt->pdev->dev, "runtime accepted context; creating queues\n");
	for (i = 0; i < ARRAY_SIZE(queues); i++) {
		ret = mt_create_queue(bt, queues[i].id);
		if (ret)
			goto release;
	}
	ret = 0;
release:
	release_firmware(fw);
	return ret;
}

static void mt_receive(struct mt_bt *bt, unsigned id, unsigned packet_type)
{
	struct mt_queue *q = &bt->q[id];
	u16 tail = le16_to_cpu(READ_ONCE(bt->tails[id]));
	u16 head = le16_to_cpu(READ_ONCE(bt->heads[id]));
	unsigned budget = MT_ENTRIES;
	if (tail >= MT_ENTRIES || head >= MT_ENTRIES) {
		bt->malformed++;
		return;
	}
	if (q->cursor == tail)
		return;
	dma_rmb();
	while (q->cursor != tail && budget--) {
		struct mt_entry *e = (void *)(q->data + q->cursor * q->stride);
		unsigned length = get_unaligned_le16(&e->length);
		if (length && length <= q->capacity && (e->flags & BIT(1)) && !(e->flags & BIT(0))) {
			if (packet_type) {
				struct sk_buff *skb = bt_skb_alloc(length, GFP_KERNEL);
				if (skb) {
					skb_put_data(skb, (u8 *)e + sizeof(*e), length);
					hci_skb_pkt_type(skb) = packet_type;
					bt->rx_packets++;
					hci_recv_frame(bt->hdev, skb);
				}
			}
		} else if (length) {
			bt->malformed++;
			dev_warn_ratelimited(&bt->pdev->dev, "RX ring %u bad flags=%x length=%u\n", id, e->flags, length);
		}
		memset(e, 0, q->stride);
		e->id = cpu_to_le16(q->cursor);
		q->cursor = (q->cursor + 1) % MT_ENTRIES;
		head = (head + 1) % MT_ENTRIES;
	}
	dma_wmb();
	WRITE_ONCE(bt->heads[id], cpu_to_le16(head));
	mt_doorbell(bt, id);
}

static void mt_poll(struct work_struct *work)
{
	struct mt_bt *bt = container_of(to_delayed_work(work), struct mt_bt, poll);
	struct sk_buff *skb;
	if (!READ_ONCE(bt->running))
		return;
	mt_receive(bt, 2, HCI_EVENT_PKT);
	mt_receive(bt, 4, HCI_SCODATA_PKT);
	mt_receive(bt, 6, HCI_ACLDATA_PKT);
	mt_receive(bt, 8, HCI_ISODATA_PKT);
	mt_receive(bt, 10, 0);
	while ((skb = skb_dequeue(&bt->pending))) {
		unsigned id = hci_skb_pkt_type(skb) == HCI_COMMAND_PKT ? 1 :
			      hci_skb_pkt_type(skb) == HCI_ACLDATA_PKT ? 5 : 3;
		int ret = mt_push(bt, id, skb->data, skb->len);
		if (ret == -ENOSPC) {
			skb_queue_head(&bt->pending, skb);
			break;
		}
		if (ret)
			dev_warn_ratelimited(&bt->pdev->dev, "TX ring %u failed %d\n", id, ret);
		else
			bt->tx_packets++;
		kfree_skb(skb);
	}
	schedule_delayed_work(&bt->poll, msecs_to_jiffies(1));
}

static int mt_open(struct hci_dev *hdev)
{
	struct mt_bt *bt = hci_get_drvdata(hdev);
	WRITE_ONCE(bt->running, true);
	schedule_delayed_work(&bt->poll, 0);
	return 0;
}

static int mt_close(struct hci_dev *hdev)
{
	struct mt_bt *bt = hci_get_drvdata(hdev);
	WRITE_ONCE(bt->running, false);
	cancel_delayed_work_sync(&bt->poll);
	skb_queue_purge(&bt->pending);
	return 0;
}

static int mt_set_address(struct hci_dev *hdev, const bdaddr_t *address)
{
	struct sk_buff *skb;
	int ret = 0;
	/* Native initialization uses MediaTek's standard write-BD-address
	 * command before Reset. The address comes from live board properties. */
	skb = __hci_cmd_sync(hdev, 0xfc1a, sizeof(*address), address, HCI_INIT_TIMEOUT);
	if (IS_ERR(skb))
		return PTR_ERR(skb);
	if (!skb->len || skb->data[0])
		ret = -EIO;
	kfree_skb(skb);
	return ret;
}

static int mt_setup(struct hci_dev *hdev)
{
	struct mt_bt *bt = hci_get_drvdata(hdev);
	bdaddr_t address;
	int ret;
	ret = device_property_read_u8_array(&bt->pdev->dev, "local-bd-address", address.b, sizeof(address));
	if (ret)
		return dev_err_probe(&bt->pdev->dev, ret, "missing live-ADT Bluetooth address\n");
	if (!bacmp(&address, BDADDR_ANY) || !bacmp(&address, BDADDR_NONE))
		return -EINVAL;
	return mt_set_address(hdev, &address);
}

static int mt_send(struct hci_dev *hdev, struct sk_buff *skb)
{
	struct mt_bt *bt = hci_get_drvdata(hdev);
	unsigned type = hci_skb_pkt_type(skb);
	if (type != HCI_COMMAND_PKT && type != HCI_ACLDATA_PKT && type != HCI_SCODATA_PKT)
		return -EOPNOTSUPP;
	if (skb->len > bt->q[type == HCI_COMMAND_PKT ? 1 : type == HCI_ACLDATA_PKT ? 5 : 3].capacity)
		return -EMSGSIZE;
	if (skb_queue_len(&bt->pending) >= MT_ENTRIES)
		return -ENOSPC;
	skb_queue_tail(&bt->pending, skb);
	mod_delayed_work(system_percpu_wq, &bt->poll, 0);
	return 0;
}

static int mt_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct mt_bt *bt;
	struct hci_dev *hdev;
	int ret;
	bt = devm_kzalloc(&pdev->dev, sizeof(*bt), GFP_KERNEL);
	if (!bt)
		return -ENOMEM;
	bt->pdev = pdev;
	pci_set_drvdata(pdev, bt);
	ret = pcim_enable_device(pdev);
	if (ret)
		return ret;
	ret = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(32));
	if (ret)
		return ret;
	if (pci_resource_len(pdev, 0) < SZ_1M)
		return -EINVAL;
	bt->bar = pcim_iomap_region(pdev, 0, "mt793b-bt");
	if (IS_ERR(bt->bar))
		return PTR_ERR(bt->bar);
	skb_queue_head_init(&bt->pending);
	INIT_DELAYED_WORK(&bt->poll, mt_poll);
	pci_set_master(pdev);
	ret = mt_boot(bt);
	if (ret)
		goto stop_dma;
	hdev = hci_alloc_dev();
	if (!hdev) {
		ret = -ENOMEM;
		goto stop_dma;
	}
	bt->hdev = hdev;
	hdev->bus = HCI_PCI;
	hdev->open = mt_open;
	hdev->close = mt_close;
	hdev->send = mt_send;
	hdev->setup = mt_setup;
	hdev->set_bdaddr = mt_set_address;
	hci_set_drvdata(hdev, bt);
	SET_HCIDEV_DEV(hdev, &pdev->dev);
	ret = hci_register_dev(hdev);
	if (ret) {
		hci_free_dev(hdev);
		goto stop_dma;
	}
	dev_info(&pdev->dev, "private MT793B HCI registered (polling, MSI unqualified)\n");
	return 0;
stop_dma:
	pci_clear_master(pdev);
	pci_wait_for_pending_transaction(pdev);
	return ret;
}

static void mt_remove(struct pci_dev *pdev)
{
	struct mt_bt *bt = pci_get_drvdata(pdev);
	hci_unregister_dev(bt->hdev);
	mt_close(bt->hdev);
	pci_clear_master(pdev);
	pci_wait_for_pending_transaction(pdev);
	dev_info(&pdev->dev, "stopped: tx=%llu rx=%llu malformed=%llu\n", bt->tx_packets, bt->rx_packets, bt->malformed);
	hci_free_dev(bt->hdev);
}

static const struct pci_device_id mt_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_MEDIATEK, 0x793b) },
	{}
};
MODULE_DEVICE_TABLE(pci, mt_ids);
static struct pci_driver mt_driver = {
	.name = "mt793b-bt-private", .id_table = mt_ids, .probe = mt_probe,
	.remove = mt_remove,
};
module_pci_driver(mt_driver);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Private Neo MT793B PCIe Bluetooth bring-up");
MODULE_FIRMWARE(MT_FW_NAME);
