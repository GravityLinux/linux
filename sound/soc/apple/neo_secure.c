// SPDX-License-Identifier: GPL-2.0-only
/* Private Neo AOP second-host transport, derived from declared mailbox data.
 * Own the link on Linux; no ExclaveOS guest or speaker path is started. */
#include <linux/completion.h>
#include <linux/dma-mapping.h>
#include <linux/module.h>
#include <linux/of.h>
#include <linux/of_platform.h>
#include <linux/platform_device.h>
#include <linux/soc/apple/mailbox.h>
#include <linux/workqueue.h>
#include "neo_secure.h"

#define NEO_SECURE_ENDPOINTS 0x40
struct neo_secure_buffer {
	void *rx, *tx;
	dma_addr_t rx_iova, tx_iova;
	size_t size;
};
struct neo_secure {
	struct device *dev;
	struct apple_mbox *mbox;
	struct workqueue_struct *wq;
	struct completion ready;
	struct neo_secure_buffer buffers[NEO_SECURE_ENDPOINTS];
	int error;
};
struct neo_secure_message {
	struct work_struct work;
	struct neo_secure *secure;
	struct apple_mbox_msg msg;
};

static int neo_secure_send(struct neo_secure *s, u8 ep, u64 value)
{
	struct apple_mbox_msg msg = { .msg0 = value, .msg1 = ep };
	dma_wmb();
	return apple_mbox_send(s->mbox, msg, false);
}

static int neo_secure_allocate(struct neo_secure *s, u8 ep, size_t size)
{
	struct neo_secure_buffer *b = &s->buffers[ep];
	u64 rx, tx;
	int error;

	if (!size || size > 0x10000 || size % 1024 || b->size)
		return -EINVAL;
	b->size = size;
	b->rx = dma_alloc_coherent(s->dev, size, &b->rx_iova, GFP_KERNEL);
	b->tx = dma_alloc_coherent(s->dev, size, &b->tx_iova, GFP_KERNEL);
	if (!b->rx || !b->tx)
		return -ENOMEM;
	if ((b->rx_iova >> 36) != 0x20 || (b->tx_iova >> 36) != 0x20)
		return -ERANGE;
	/* This protocol encodes a 36-bit offset and a host window selector,
	 * rather than the canonical 42-bit SID-1 IOVA used by DART. */
	rx = 0x2000000000ULL | (b->rx_iova & GENMASK_ULL(35, 0));
	tx = 0x2000000000ULL | (b->tx_iova & GENMASK_ULL(35, 0));
	dev_info(s->dev, "Second-host queue ep=%x size=%zx rx=%pad tx=%pad\n",
		 ep, size, &b->rx_iova, &b->tx_iova);
	error = neo_secure_send(s, ep, 0x1200ULL << 48);
	if (!error) error = neo_secure_send(s, ep, (0x5041ULL << 48) | rx);
	if (!error) error = neo_secure_send(s, ep,
			((0x5000ULL | (size / 1024)) << 48) | tx);
	return error;
}

static void neo_secure_work(struct work_struct *work)
{
	struct neo_secure_message *message = container_of(work, struct neo_secure_message, work);
	struct neo_secure *s = message->secure;
	u64 value = message->msg.msg0;
	u8 ep = message->msg.msg1;
	u16 type = value >> 48;
	int error = 0;

	dev_info(s->dev, "Second-host RX ep=%x value=%016llx\n", ep, value);
	if (!ep) {
		switch ((value >> 52) & 0xff) {
		case 1: /* HELLO: native firmware advertises protocol 12. */
			if ((value & 0xffffffff) != 0xc000c) { error = -EPROTO; break; }
			error = neo_secure_send(s, 0, 0x200000000c000cULL);
			break;
		case 8: /* Unlike normal RTKit, this host echoes the endpoint map. */
			error = neo_secure_send(s, 0, value);
			if (!error && (value & BIT_ULL(51)))
				error = neo_secure_send(s, 0, 0xb0000000000220ULL);
			break;
		case 0xb:
			if ((value & 0xffff) != 0x20) error = -EPROTO;
			complete(&s->ready);
			break;
		default:
			error = -EPROTO;
		}
	} else if (ep >= 0x20 && ep < NEO_SECURE_ENDPOINTS) {
		switch (type) {
		case 0x1200:
			error = neo_secure_allocate(s, ep, value & 0xffffffff);
			break;
		case 0x2000:
			/* No requests are sent on this link during transport bring-up.
			 * Acknowledge received notifications so it cannot stall AOP. */
			dma_rmb();
			error = neo_secure_send(s, ep, 0x4000ULL << 48);
			break;
		default:
			error = -EPROTO;
		}
	} else {
		error = -EPROTO;
	}
	if (error) {
		s->error = error;
		dev_err(s->dev, "Second-host error %d ep=%x value=%llx\n", error, ep, value);
		complete_all(&s->ready);
	}
	kfree(message);
}

static void neo_secure_rx(struct apple_mbox *mbox, struct apple_mbox_msg msg, void *cookie)
{
	struct neo_secure *s = cookie;
	struct neo_secure_message *message = kzalloc(sizeof(*message), GFP_ATOMIC);
	if (!message) { s->error = -ENOMEM; complete_all(&s->ready); return; }
	message->secure = s;
	message->msg = msg;
	INIT_WORK(&message->work, neo_secure_work);
	queue_work(s->wq, &message->work);
}

static void neo_secure_cleanup(void *cookie)
{
	struct neo_secure *s = cookie;
	int i;
	apple_mbox_stop(s->mbox);
	destroy_workqueue(s->wq);
	for (i = 0; i < NEO_SECURE_ENDPOINTS; i++) {
		struct neo_secure_buffer *b = &s->buffers[i];
		if (b->rx) dma_free_coherent(s->dev, b->size, b->rx, b->rx_iova);
		if (b->tx) dma_free_coherent(s->dev, b->size, b->tx, b->tx_iova);
	}
}

struct neo_secure *neo_secure_get(struct device *dev)
{
	struct device_node *node = of_parse_phandle(dev->of_node, "apple,second-host", 0);
	struct platform_device *pdev;
	struct neo_secure *s;
	if (!node) return ERR_PTR(-ENODEV);
	pdev = of_find_device_by_node(node);
	of_node_put(node);
	if (!pdev) return ERR_PTR(-EPROBE_DEFER);
	s = platform_get_drvdata(pdev);
	if (s && !device_link_add(dev, &pdev->dev, DL_FLAG_AUTOREMOVE_CONSUMER))
		s = ERR_PTR(-ENOMEM);
	put_device(&pdev->dev);
	return s ?: ERR_PTR(-EPROBE_DEFER);
}

int neo_secure_wait(struct neo_secure *s)
{
	if (!wait_for_completion_timeout(&s->ready, msecs_to_jiffies(5000)))
		return -ETIMEDOUT;
	return s->error;
}

static int neo_secure_probe(struct platform_device *pdev)
{
	struct neo_secure *s = devm_kzalloc(&pdev->dev, sizeof(*s), GFP_KERNEL);
	int error;
	if (!s) return -ENOMEM;
	s->dev = &pdev->dev;
	init_completion(&s->ready);
	error = dma_set_mask_and_coherent(s->dev, DMA_BIT_MASK(42));
	if (error) return error;
	s->mbox = apple_mbox_get_byname(s->dev, "mbox");
	if (IS_ERR(s->mbox)) return PTR_ERR(s->mbox);
	s->wq = alloc_ordered_workqueue("neo-aop-second-host", WQ_MEM_RECLAIM);
	if (!s->wq) return -ENOMEM;
	s->mbox->rx = neo_secure_rx;
	s->mbox->cookie = s;
	error = devm_add_action_or_reset(s->dev, neo_secure_cleanup, s);
	if (error) return error;
	error = apple_mbox_start(s->mbox);
	if (error) return error;
	platform_set_drvdata(pdev, s);
	dev_info(s->dev, "AOP second-host receiver armed\n");
	return 0;
}
static const struct of_device_id neo_secure_of_match[] = {
	{ .compatible = "apple,t8140-neo-aop-second-host" }, {}
};
MODULE_DEVICE_TABLE(of, neo_secure_of_match);
static struct platform_driver neo_secure_driver = {
	.probe = neo_secure_probe,
	.driver = { .name = "neo-aop-second-host", .of_match_table = neo_secure_of_match },
};
module_platform_driver(neo_secure_driver);
MODULE_LICENSE("GPL");
