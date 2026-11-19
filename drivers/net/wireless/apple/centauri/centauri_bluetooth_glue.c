// SPDX-License-Identifier: GPL-2.0-only
/* Linux HCI registration/skb scheduling; the queue protocol is Rust. */
#include <linux/dma-mapping.h>
#include <linux/io.h>
#include <linux/iommu.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/property.h>
#include <linux/sizes.h>
#include <linux/ktime.h>
#include <linux/pci.h>
#include <linux/of.h>
#include <linux/skbuff.h>
#include <linux/workqueue.h>
#include <net/bluetooth/bluetooth.h>
#include <net/bluetooth/hci_core.h>
#include "centauri_glue.h"

#define CENTAURI_BT_BAR_MIN_SIZE 0x1080
#define CENTAURI_HCI_SET_FACTORY_ADDRESS 0xfc06
#define CENTAURI_HCI_RADIO_INITIALIZE 0xff0b

u64 centauri_bt_now_ms(void)
{
	return ktime_get_mono_fast_ns() / NSEC_PER_MSEC;
}

struct centauri_bluetooth {
	struct pci_dev *pdev;
	void __iomem *bar;
	void *resource;
	dma_addr_t resource_dma;
	size_t resource_bytes;
	u8 *arena;
	dma_addr_t arena_dma;
	bdaddr_t factory;
	bool setting_up;
	bool shutting_down;
	struct hci_dev *hdev;
	struct delayed_work work;
	struct sk_buff_head tx;
	struct sk_buff_head rx;
	/* Serializes process-context HCI queue operations and reset setup. */
	struct mutex lock;
	void *rust;
	bool running;
};

void centauri_bt_queue_rx(void *glue, u8 type, const u8 *frame, size_t length)
{
	struct centauri_bluetooth *priv = glue;
	struct sk_buff *skb = bt_skb_alloc(length, GFP_KERNEL);

	if (!skb) {
		priv->hdev->stat.err_rx++;
		return;
	}
	skb_put_data(skb, frame, length);
	hci_skb_pkt_type(skb) = type;
	skb_queue_tail(&priv->rx, skb);
}
static void centauri_bt_work(struct work_struct *work)
{
	struct centauri_bluetooth *priv = container_of(to_delayed_work(work),
						struct centauri_bluetooth, work);
	struct sk_buff *skb;
	int error, sent;

	if (!READ_ONCE(priv->running))
		return;
	mutex_lock(&priv->lock);
	if (!READ_ONCE(priv->running)) {
		mutex_unlock(&priv->lock);
		return;
	}
	for (sent = 0; sent < 8; sent++) {
		skb = skb_dequeue(&priv->tx);
		if (!skb)
			break;
		error = centauri_bt_rust_send(priv->rust, hci_skb_pkt_type(skb),
					      skb->data, skb->len,
					      READ_ONCE(priv->setting_up));
		if (error == -EAGAIN) {
			skb_queue_head(&priv->tx, skb);
			break;
		}
		if (error)
			priv->hdev->stat.err_tx++;
		else
			priv->hdev->stat.byte_tx += skb->len;
		kfree_skb(skb);
	}
	error = centauri_bt_rust_poll(priv->rust, 64);
	mutex_unlock(&priv->lock);
	while ((skb = skb_dequeue(&priv->rx))) {
		priv->hdev->stat.byte_rx += skb->len;
		hci_recv_frame(priv->hdev, skb);
	}
	if (error < 0) {
		bt_dev_err(priv->hdev, "Rust transport error: %d", error);
		WRITE_ONCE(priv->running, false);
		pci_clear_master(priv->pdev);
		return;
	}
	if (READ_ONCE(priv->running))
		queue_delayed_work(system_percpu_wq, &priv->work, msecs_to_jiffies(10));
}
static int centauri_bt_open(struct hci_dev *hdev)
{
	struct centauri_bluetooth *priv = hci_get_drvdata(hdev);

	if (READ_ONCE(priv->shutting_down))
		return -ENODEV;
	WRITE_ONCE(priv->running, true);
	queue_delayed_work(system_percpu_wq, &priv->work, 0);
	return 0;
}
static int centauri_bt_close(struct hci_dev *hdev)
{
	struct centauri_bluetooth *priv = hci_get_drvdata(hdev);

	WRITE_ONCE(priv->running, false);
	cancel_delayed_work_sync(&priv->work);
	skb_queue_purge(&priv->tx);
	skb_queue_purge(&priv->rx);
	return 0;
}
static int centauri_bt_flush(struct hci_dev *hdev)
{
	struct centauri_bluetooth *priv = hci_get_drvdata(hdev);

	skb_queue_purge(&priv->tx);
	return 0;
}
static int centauri_bt_send(struct hci_dev *hdev, struct sk_buff *skb)
{
	struct centauri_bluetooth *priv = hci_get_drvdata(hdev);

	if (!READ_ONCE(priv->running))
		return -ENETDOWN;
	if (skb_linearize(skb))
		return -ENOMEM;
	switch (hci_skb_pkt_type(skb)) {
	case HCI_COMMAND_PKT:
		hdev->stat.cmd_tx++;
		break;
	case HCI_ACLDATA_PKT:
		hdev->stat.acl_tx++;
		break;
	default:
		return -EOPNOTSUPP;
	}
	skb_queue_tail(&priv->tx, skb);
	mod_delayed_work(system_percpu_wq, &priv->work, 0);
	return 0;
}

/* The FC06 address parameter is network/display order, unlike standard HCI
 * Read BD_ADDR and local-bd-address. Python factory/readback probes qualify it.
 */
static int centauri_bt_setup(struct hci_dev *hdev)
{
	struct centauri_bluetooth *priv = hci_get_drvdata(hdev);
	struct sk_buff *reply;
	u8 address[6];
	int error, i;

	for (i = 0; i < sizeof(address); i++)
		address[i] = priv->factory.b[sizeof(address) - i - 1];
	WRITE_ONCE(priv->setting_up, true);
	error = __hci_cmd_sync_status(hdev, CENTAURI_HCI_SET_FACTORY_ADDRESS,
				      sizeof(address), address, HCI_INIT_TIMEOUT);
	if (error)
		goto done;
	reply = __hci_cmd_sync(hdev, HCI_OP_READ_BD_ADDR, 0, NULL, HCI_INIT_TIMEOUT);
	if (IS_ERR(reply)) {
		error = PTR_ERR(reply);
		goto done;
	}
	if (reply->len != 7 || reply->data[0] || memcmp(reply->data + 1, priv->factory.b, 6))
		error = -EIO;
	kfree_skb(reply);
	if (error)
		goto done;
	/* Empty FF0B is qualified; no captured startup mask or parameter record. */
	error = __hci_cmd_sync_status(hdev, CENTAURI_HCI_RADIO_INITIALIZE,
				      0, NULL, HCI_INIT_TIMEOUT);
	if (!error)
		bt_dev_info(hdev, "factory address verified; empty radio initialization complete");
done:
	WRITE_ONCE(priv->setting_up, false);
	return error;
}

static void centauri_bt_free(struct centauri_bluetooth *priv)
{
	pci_clear_master(priv->pdev);
	if (priv->rust)
		centauri_bt_rust_destroy(priv->rust);
	if (priv->arena)
		dma_free_coherent(&priv->pdev->dev, SZ_16K, priv->arena, priv->arena_dma);
	if (priv->resource)
		dma_free_coherent(&priv->pdev->dev, priv->resource_bytes,
				  priv->resource, priv->resource_dma);
}

/* No HCI_QUIRK_SYNC_FLOWCTL_SUPPORTED: SCO transport is unimplemented. */
static int centauri_bt_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct centauri_bluetooth *priv;
	struct iommu_domain *domain;
	struct hci_dev *hdev;
	int error;

	if (!of_device_is_available(pdev->dev.of_node) ||
	    !of_device_is_compatible(pdev->dev.of_node, "apple,t2026-centauri-bluetooth"))
		return -ENODEV;
	domain = iommu_get_domain_for_dev(&pdev->dev);
	if (!domain || !iommu_is_dma_domain(domain))
		return dev_err_probe(&pdev->dev, -EOPNOTSUPP,
				     "Bluetooth requires Linux DMA domain\n");
	if (!(pci_resource_flags(pdev, 0) & IORESOURCE_MEM) ||
	    pci_resource_len(pdev, 0) < CENTAURI_BT_BAR_MIN_SIZE)
		return -EINVAL;
	error = pcim_enable_device(pdev);
	if (error)
		return error;
	pci_clear_master(pdev);
	error = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(32));
	if (error)
		return error;
	priv = devm_kzalloc(&pdev->dev, sizeof(*priv), GFP_KERNEL);
	if (!priv)
		return -ENOMEM;
	priv->pdev = pdev;
	error = device_property_read_u8_array(&pdev->dev, "local-bd-address", priv->factory.b, 6);
	if (error || !bacmp(&priv->factory, BDADDR_ANY) || !bacmp(&priv->factory, BDADDR_NONE))
		return dev_err_probe(&pdev->dev, -EINVAL, "valid local-bd-address required\n");
	priv->bar = pcim_iomap_region(pdev, 0, KBUILD_MODNAME);
	if (IS_ERR(priv->bar))
		return PTR_ERR(priv->bar);
	mutex_init(&priv->lock);
	skb_queue_head_init(&priv->tx);
	skb_queue_head_init(&priv->rx);
	INIT_DELAYED_WORK(&priv->work, centauri_bt_work);
	error = centauri_control_copy_radio(pdev, &priv->resource,
					    &priv->resource_dma, &priv->resource_bytes);
	if (error)
		return dev_err_probe(&pdev->dev, error, "Bluetooth firmware resource\n");
	error = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(36));
	if (error)
		goto free;
	priv->arena = dma_alloc_coherent(&pdev->dev, SZ_16K, &priv->arena_dma, GFP_KERNEL);
	if (!priv->arena) {
		error = -ENOMEM;
		goto free;
	}
	error = centauri_bt_rust_create(priv, (void __force *)priv->bar,
					priv->arena, priv->arena_dma, SZ_16K,
					priv->resource_dma, priv->resource_bytes, &priv->rust);
	if (error)
		goto free;
	hdev = hci_alloc_dev();
	if (!hdev) {
		error = -ENOMEM;
		goto free;
	}
	priv->hdev = hdev;
	hdev->bus = HCI_PCI;
	hdev->open = centauri_bt_open;
	hdev->close = centauri_bt_close;
	hdev->flush = centauri_bt_flush;
	hdev->send = centauri_bt_send;
	hdev->setup = centauri_bt_setup;
	hci_set_drvdata(hdev, priv);
	SET_HCIDEV_DEV(hdev, &pdev->dev);
	error = hci_register_dev(hdev);
	if (error) {
		hci_free_dev(hdev);
		goto free;
	}
	pci_set_drvdata(pdev, priv);
	dev_info(&pdev->dev, "cold Rust Bluetooth HCI registered with generated DMA page\n");
	return 0;
free:
	centauri_bt_free(priv);
	return error;
}

void centauri_bt_enable_dma(void *owner)
{
	struct centauri_bluetooth *priv = owner;

	dma_wmb();
	pci_set_master(priv->pdev);
}

int centauri_bt_config_read(void *owner, u16 offset, u32 *value)
{
	struct centauri_bluetooth *priv = owner;

	return pcibios_err_to_errno(pci_read_config_dword(priv->pdev, offset, value));
}

int centauri_bt_config_write(void *owner, u16 offset, u32 value)
{
	struct centauri_bluetooth *priv = owner;

	return pcibios_err_to_errno(pci_write_config_dword(priv->pdev, offset, value));
}

static void centauri_bt_remove(struct pci_dev *pdev)
{
	struct centauri_bluetooth *priv = pci_get_drvdata(pdev);

	WRITE_ONCE(priv->shutting_down, true);
	hci_unregister_dev(priv->hdev);
	centauri_bt_close(priv->hdev);
	hci_free_dev(priv->hdev);
	centauri_bt_free(priv);
}
static const struct of_device_id centauri_bt_of_match[] = {
	{ .compatible = "apple,t2026-centauri-bluetooth" },
	{ }
};
MODULE_DEVICE_TABLE(of, centauri_bt_of_match);
static const struct pci_device_id centauri_bt_pci_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_APPLE, 0x1903) },
	{ }
};
MODULE_DEVICE_TABLE(pci, centauri_bt_pci_ids);
static struct pci_driver centauri_bt_driver = {
	.name = "centauri-bluetooth",
	.id_table = centauri_bt_pci_ids,
	.probe = centauri_bt_probe,
	.remove = centauri_bt_remove,
	.driver = { .of_match_table = centauri_bt_of_match },
};
int centauri_bt_register(void)
{
	return pci_register_driver(&centauri_bt_driver);
}
void centauri_bt_unregister(void)
{
	pci_unregister_driver(&centauri_bt_driver);
}
