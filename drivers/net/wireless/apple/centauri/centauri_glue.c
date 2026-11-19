// SPDX-License-Identifier: GPL-2.0-only
/* net_device/NAPI and generic mappings for the Rust Centauri queue driver. */
#include <linux/etherdevice.h>
#include <linux/hrtimer.h>
#include <linux/io.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/netdevice.h>
#include <linux/of.h>
#include <linux/pci.h>
#include <linux/of_address.h>
#include <linux/unaligned.h>
#include <linux/skbuff.h>
#include <linux/slab.h>
#include <linux/sizes.h>
#include <linux/vmalloc.h>
#include "centauri_glue.h"

/* Polling is a software bring-up policy until PCIe MSI ownership is wired. */
static unsigned int poll_interval_us = 10000;
module_param(poll_interval_us, uint, 0600);
MODULE_PARM_DESC(poll_interval_us, "NAPI polling interval in microseconds (1000..100000)");
static ktime_t centauri_poll_interval(void)
{
	return ns_to_ktime((u64)clamp_val(READ_ONCE(poll_interval_us), 1000U, 100000U) * 1000);
}
struct centauri_net {
	struct net_device *netdev;
	struct napi_struct napi;
	struct hrtimer timer;
	spinlock_t lock;
	struct sk_buff_head received;
	void *rust;
	bool faulted;
};

/* Observed TX descriptor bit 17; its meaning awaits a controlled bit probe.
 * Kept writable to test its necessity without replaying captured descriptors.
 */
static unsigned int tx_metadata = BIT(17);
module_param(tx_metadata, uint, 0600);
MODULE_PARM_DESC(tx_metadata, "Provisional TX descriptor metadata, default observed bit 17");
u32 centauri_tx_metadata(void) { return READ_ONCE(tx_metadata); }

void centauri_barrier(void) { dsb(sy); }
void *centauri_map_io(u64 physical, size_t length)
{
	return (void __force *)ioremap(physical, length);
}
void centauri_unmap_io(void *address) { iounmap((void __iomem *)address); }

void *centauri_map_ram(u64 physical, size_t length)
{
	struct page **pages;
	unsigned long first = physical >> PAGE_SHIFT;
	unsigned int count, i;
	void *address;

	if (!length || !PAGE_ALIGNED(physical) || !PAGE_ALIGNED(length))
		return NULL;
	count = length >> PAGE_SHIFT;
	pages = kvmalloc_array(count, sizeof(*pages), GFP_KERNEL);
	if (!pages)
		return NULL;
	for (i = 0; i < count; i++) {
		if (!pfn_valid(first + i) || !PageReserved(pfn_to_page(first + i))) {
			kvfree(pages);
			return NULL;
		}
		pages[i] = pfn_to_page(first + i);
	}
	/* The loader reserved these pages; no kernel allocator owns them.
	 * Match the successful arm64 O_SYNC prototype's Normal-NC mapping.
	 */
	address = vmap(pages, count, VM_MAP, pgprot_writecombine(PAGE_KERNEL));
	kvfree(pages);
	return address;
}
void centauri_unmap_ram(void *address) { vunmap(address); }

int centauri_pci_prepare(struct pci_dev *pdev, struct centauri_resources *resources)
{
	struct device_node *node;
	struct resource dart;
	int error;

	/* Firmware still owns the DART translation domain in this warm handoff.
	 * Resolve its status resource without attaching or resetting that domain.
	 */
	node = of_parse_phandle(pdev->dev.of_node, "apple,firmware-dart", 0);
	if (!node)
		return -EINVAL;
	if (of_device_is_available(node)) {
		of_node_put(node);
		return -EBUSY;
	}
	error = of_address_to_resource(node, 0, &dart);
	of_node_put(node);
	if (error)
		return error;
	if (!(pci_resource_flags(pdev, 0) & IORESOURCE_MEM) ||
	    pci_resource_len(pdev, 0) < 0x1054 || resource_size(&dart) < 0x104)
		return -EINVAL;
	error = pcim_enable_device(pdev);
	if (error)
		return error;
	error = pcim_request_region(pdev, 0, KBUILD_MODNAME);
	if (error)
		return error;
	resources->bar = pci_resource_start(pdev, 0);
	resources->bar_length = pci_resource_len(pdev, 0);
	resources->dart = dart.start;
	resources->dart_length = resource_size(&dart);
	return 0;
}

int centauri_load_handoff(struct device *dev, struct centauri_handoff *handoff)
{
	struct device_node *node;
	struct resource region;
	void *data;
	u32 size;
	int error;

	node = of_parse_phandle(dev->of_node, "memory-region", 0);
	if (!node)
		return -EINVAL;
	error = of_address_to_resource(node, 0, &region);
	of_node_put(node);
	if (error)
		return error;
	/* This immutable, host-generated metadata is distinct from device DMA.
	 * map_ram checks that all pages were reserved before Linux allocation.
	 */
	if (resource_size(&region) < 16 || resource_size(&region) > SZ_1M)
		return -EINVAL;
	data = centauri_map_ram(region.start, resource_size(&region));
	if (!data)
		return -ENOMEM;
	centauri_barrier();
	size = get_unaligned_le32(data + 8);
	if (size < 16 || size > resource_size(&region)) {
		centauri_unmap_ram(data);
		return -EINVAL;
	}
	handoff->data = data;
	handoff->size = size;
	return 0;
}

void centauri_release_handoff(struct centauri_handoff *handoff)
{
	centauri_unmap_ram((void *)handoff->data);
}

void centauri_queue_rx(void *glue, const u8 *frame, size_t length)
{
	struct centauri_net *priv = glue;
	struct sk_buff *skb = napi_alloc_skb(&priv->napi, length);

	if (!skb) {
		priv->netdev->stats.rx_dropped++;
		return;
	}
	skb_put_data(skb, frame, length);
	skb->protocol = eth_type_trans(skb, priv->netdev);
	skb_queue_tail(&priv->received, skb);
	priv->netdev->stats.rx_packets++;
	priv->netdev->stats.rx_bytes += length;
}

static int centauri_poll(struct napi_struct *napi, int budget)
{
	struct centauri_net *priv = container_of(napi, struct centauri_net, napi);
	struct sk_buff *skb;
	unsigned long flags;
	u32 tx_done = 0;
	int done;

	if (READ_ONCE(priv->faulted)) {
		napi_complete_done(napi, 0);
		return 0;
	}
	spin_lock_irqsave(&priv->lock, flags);
	done = centauri_rust_poll(priv->rust, budget, &tx_done);
	spin_unlock_irqrestore(&priv->lock, flags);
	while ((skb = skb_dequeue(&priv->received)))
		napi_gro_receive(napi, skb);
	if (tx_done)
		netif_wake_queue(priv->netdev);
	if (done < 0) {
		netdev_err(priv->netdev, "Rust queue poll failed: %d\n", done);
		WRITE_ONCE(priv->faulted, true);
		netif_stop_queue(priv->netdev);
		netif_carrier_off(priv->netdev);
		done = 0;
	}
	if (done < budget)
		napi_complete_done(napi, done);
	return done;
}

static enum hrtimer_restart centauri_timer(struct hrtimer *timer)
{
	struct centauri_net *priv = container_of(timer, struct centauri_net, timer);

	napi_schedule(&priv->napi);
	hrtimer_forward_now(timer, centauri_poll_interval());
	return HRTIMER_RESTART;
}
static int centauri_open(struct net_device *netdev)
{
	struct centauri_net *priv = netdev_priv(netdev);

	napi_enable(&priv->napi);
	netif_carrier_on(netdev);
	netif_start_queue(netdev);
	hrtimer_start(&priv->timer, centauri_poll_interval(), HRTIMER_MODE_REL);
	return 0;
}
static int centauri_stop(struct net_device *netdev)
{
	struct centauri_net *priv = netdev_priv(netdev);

	netif_stop_queue(netdev);
	hrtimer_cancel(&priv->timer);
	napi_disable(&priv->napi);
	netif_carrier_off(netdev);
	return 0;
}
static netdev_tx_t centauri_xmit(struct sk_buff *skb, struct net_device *netdev)
{
	struct centauri_net *priv = netdev_priv(netdev);
	unsigned long flags;
	int error;

	if (unlikely(skb_linearize(skb))) {
		dev_kfree_skb_any(skb);
		netdev->stats.tx_dropped++;
		return NETDEV_TX_OK;
	}
	netif_stop_queue(netdev);
	spin_lock_irqsave(&priv->lock, flags);
	error = centauri_rust_xmit(priv->rust, skb->data, skb->len);
	spin_unlock_irqrestore(&priv->lock, flags);
	if (error == -EAGAIN) {
		netif_wake_queue(netdev);
		return NETDEV_TX_BUSY;
	}
	if (error) {
		netdev->stats.tx_errors++;
		netif_carrier_off(netdev);
	} else {
		netdev->stats.tx_packets++;
		netdev->stats.tx_bytes += skb->len;
	}
	dev_kfree_skb_any(skb);
	return NETDEV_TX_OK;
}
static const struct net_device_ops centauri_netdev_ops = {
	.ndo_open = centauri_open,
	.ndo_stop = centauri_stop,
	.ndo_start_xmit = centauri_xmit,
	.ndo_validate_addr = eth_validate_addr,
};

static int centauri_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct net_device *netdev;
	struct centauri_net *priv;
	struct centauri_handoff metadata;
	struct centauri_resources resources;
	u8 mac[ETH_ALEN];
	int error;

	if (!of_device_is_available(pdev->dev.of_node) ||
	    of_device_is_compatible(pdev->dev.of_node,
				    "apple,t2026-centauri-wifi") ||
	    !of_device_is_compatible(pdev->dev.of_node, "pci106b,1902"))
		return -ENODEV;
	error = centauri_pci_prepare(pdev, &resources);
	if (error)
		return error;
	error = centauri_load_handoff(&pdev->dev, &metadata);
	if (error)
		return error;
	netdev = alloc_etherdev(sizeof(*priv));
	if (!netdev) {
		centauri_release_handoff(&metadata);
		return -ENOMEM;
	}
	priv = netdev_priv(netdev);
	priv->netdev = netdev;
	spin_lock_init(&priv->lock);
	skb_queue_head_init(&priv->received);
	error = centauri_rust_create(priv, metadata.data, metadata.size, &priv->rust, mac, &resources);
	centauri_release_handoff(&metadata);
	if (error)
		goto free_netdev;
	pci_set_master(pdev);
	strscpy(netdev->name, "cen%d");
	SET_NETDEV_DEV(netdev, &pdev->dev);
	netdev->netdev_ops = &centauri_netdev_ops;
	netdev->max_mtu = ETH_DATA_LEN;
	eth_hw_addr_set(netdev, mac);
	netif_carrier_off(netdev);
	netif_napi_add(netdev, &priv->napi, centauri_poll);
	hrtimer_setup(&priv->timer, centauri_timer, CLOCK_MONOTONIC, HRTIMER_MODE_REL);
	error = register_netdev(netdev);
	if (error)
		goto destroy_rust;
	pci_set_drvdata(pdev, netdev);
	netdev_info(netdev, "Rust Centauri retained-state queue driver\n");
	return 0;
destroy_rust:
	netif_napi_del(&priv->napi);
	centauri_rust_destroy(priv->rust);
free_netdev:
	free_netdev(netdev);
	return error;
}
static void centauri_remove(struct pci_dev *pdev)
{
	struct net_device *netdev = pci_get_drvdata(pdev);
	struct centauri_net *priv = netdev_priv(netdev);

	unregister_netdev(netdev);
	netif_napi_del(&priv->napi);
	skb_queue_purge(&priv->received);
	centauri_rust_destroy(priv->rust);
	free_netdev(netdev);
}
static const struct of_device_id centauri_of_match[] = {
	{ .compatible = "pci106b,1902" },
	{ }
};
MODULE_DEVICE_TABLE(of, centauri_of_match);
static const struct pci_device_id centauri_pci_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_APPLE, 0x1902) },
	{ }
};
MODULE_DEVICE_TABLE(pci, centauri_pci_ids);
static struct pci_driver centauri_driver = {
	.name = "centauri-wifi",
	.id_table = centauri_pci_ids,
	.probe = centauri_probe,
	.remove = centauri_remove,
	.driver = { .of_match_table = centauri_of_match },
};
int centauri_register(void)
{
	return pci_register_driver(&centauri_driver);
}
void centauri_unregister(void)
{
	pci_unregister_driver(&centauri_driver);
}
