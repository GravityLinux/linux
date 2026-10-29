// SPDX-License-Identifier: GPL-2.0-only OR MIT
/*
 * Apple SoC PMP power state reporting driver
 *
 * Copyright The Asahi Linux Contributors
 */

#include <linux/bitops.h>
#include <linux/iopoll.h>
#include <linux/module.h>
#include <linux/of.h>
#include <linux/of_address.h>
#include <linux/of_platform.h>
#include <linux/platform_device.h>
#include <linux/pm_domain.h>
#include <linux/unaligned.h>
#include <linux/workqueue.h>
#include <linux/jiffies.h>
#include <linux/soc/apple/pmp-report.h>

#define PMP_REPORT_READY 0x1
#define PMP_READY_WAIT_MS 120000

/* PTD driver version 1 soc-device records, transferred unchanged from ADT. */
#define PMP_SOC_DEVICE_SIZE 0x7c
#define PMP_SOC_DEVICE_FLAGS_OFFSET 8
#define PMP_SOC_DEVICE_ACK BIT(1)

struct apple_pmp_report_offsets {
	u32 tgt_read;
	u32 tgt_write;
	u32 actual;
	u32 status;
	bool finalize_after_ready;
	bool has_event_config;
};

struct apple_pmp_report {
	struct device *dev;
	const struct apple_pmp_report_offsets *offsets;
	void __iomem *base;
	void __iomem *event_config;
	struct delayed_work finalize_work;
	unsigned long probed;
	spinlock_t lock;
	u64 valid_mask;
	u64 ack_mask;
};

static int apple_pmp_report_read_device_flags(struct apple_pmp_report *rep)
{
	struct device *dev = rep->dev;
	struct device_node *pmp;
	const u8 *table, *version;
	unsigned int count, i;
	int len, ret = 0;

	/* Preserve the established behaviour of trees without this metadata. */
	rep->valid_mask = ~0ULL;
	rep->ack_mask = ~0ULL;
	if (!of_property_present(dev->of_node, "apple,pmp"))
		return 0;

	pmp = of_parse_phandle(dev->of_node, "apple,pmp", 0);
	if (!pmp)
		return dev_err_probe(dev, -EINVAL, "invalid PMP reference\n");
	/* A boot without optional PMP firmware still uses the ready-bit check. */
	if (!of_device_is_available(pmp) &&
	    !of_property_present(pmp, "apple,tunable-soc-device"))
		goto out;

	version = of_get_property(pmp, "apple,tunable-ptd-driver-version", &len);
	if (!version || len != sizeof(u32) || get_unaligned_le32(version) != 1) {
		ret = dev_err_probe(dev, -EINVAL, "unsupported PMP device table version\n");
		goto out;
	}

	table = of_get_property(pmp, "apple,tunable-soc-device", &len);
	if (!table || len <= 0 || len % PMP_SOC_DEVICE_SIZE ||
	    len / PMP_SOC_DEVICE_SIZE > 64) {
		ret = dev_err_probe(dev, -EINVAL, "invalid PMP device table\n");
		goto out;
	}

	count = len / PMP_SOC_DEVICE_SIZE;
	rep->valid_mask = GENMASK_ULL(count - 1, 0);
	rep->ack_mask = 0;
	for (i = 0; i < count; i++) {
		u32 flags = get_unaligned_le32(table + i * PMP_SOC_DEVICE_SIZE +
					     PMP_SOC_DEVICE_FLAGS_OFFSET);

		if (flags & PMP_SOC_DEVICE_ACK)
			rep->ack_mask |= BIT_ULL(i);
	}

	dev_dbg(dev, "%u PMP records, acknowledgement mask %#llx\n", count, rep->ack_mask);
out:
	of_node_put(pmp);
	return ret;
}
int apple_pmp_report_wait_ready(struct device *dev, unsigned long timeout_us)
{
	struct apple_pmp_report *rep = dev_get_drvdata(dev);
	u64 status;

	if (!rep)
		return -EPROBE_DEFER;
	return readq_poll_timeout(rep->base + rep->offsets->status, status,
				 status & PMP_REPORT_READY, 1000, timeout_us);
}
EXPORT_SYMBOL_GPL(apple_pmp_report_wait_ready);

static void apple_pmp_report_finalize_work(struct work_struct *work)
{
	struct apple_pmp_report *rep =
		container_of(to_delayed_work(work), struct apple_pmp_report,
			     finalize_work);
	u64 actual, status, target;
	unsigned long flags;
	int ret;

	status = readq(rep->base + rep->offsets->status);
	if (!(status & PMP_REPORT_READY)) {
		/* PMP may never start (apple_pmp.enable=0): stop polling. */
		if (time_after(jiffies, rep->probed + msecs_to_jiffies(PMP_READY_WAIT_MS))) {
			dev_warn(rep->dev,
				 "PMP not ready after %u ms; not finalizing (expected with apple_pmp.enable=0)\n",
				 PMP_READY_WAIT_MS);
			return;
		}
		schedule_delayed_work(&rep->finalize_work, msecs_to_jiffies(1));
		return;
	}

	/* Native T8132 stages event channel 5 before asking PMP to enable its
	 * post-ready report bit.  The other three channels are enabled only
	 * after PMP acknowledges the request. */
	if (rep->event_config)
		writel(readl(rep->event_config + 0x20) | BIT(2),
		       rep->event_config + 0x20);

	spin_lock_irqsave(&rep->lock, flags);
	target = readq(rep->base + rep->offsets->tgt_read) | BIT_ULL(12);
	writeq(target, rep->base + rep->offsets->tgt_write);
	spin_unlock_irqrestore(&rep->lock, flags);

	ret = readq_poll_timeout(rep->base + rep->offsets->actual, actual,
				 actual & BIT_ULL(12), 10, 500000);
	dev_info(rep->dev,
		 "post-ready enable bit 12: target=%#llx actual=%#llx ret=%d\n",
		 target, actual, ret);
	if (ret)
		return;

	if (rep->event_config) {
		writel(readl(rep->event_config + 0x00) | BIT(2),
		       rep->event_config + 0x00);
		writel(readl(rep->event_config + 0x04) | BIT(2),
		       rep->event_config + 0x04);
		writel(readl(rep->event_config + 0x08) | BIT(2),
		       rep->event_config + 0x08);
	}

	/* Native 26.6 drops the boot-time bit 18 vote after bit 12 is live. */
	spin_lock_irqsave(&rep->lock, flags);
	target = readq(rep->base + rep->offsets->tgt_read) & ~BIT_ULL(18);
	writeq(target, rep->base + rep->offsets->tgt_write);
	spin_unlock_irqrestore(&rep->lock, flags);

	ret = readq_poll_timeout(rep->base + rep->offsets->actual, actual,
				 !(actual & BIT_ULL(18)), 10, 500000);
	dev_info(rep->dev,
		 "post-ready disable bit 18: target=%#llx actual=%#llx ret=%d\n",
		 target, actual, ret);
	if (rep->event_config)
		writel(readl(rep->event_config + 0x20) & ~BIT(2),
		       rep->event_config + 0x20);
}

static int apple_pmp_report_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct device_node *np = dev->of_node;
	struct apple_pmp_report *rep;
	int ret;

	rep = devm_kzalloc(dev, sizeof(*rep), GFP_KERNEL);
	if (!rep)
		return -ENOMEM;

	spin_lock_init(&rep->lock);
	rep->dev = dev;
	rep->probed = jiffies;
	rep->base = devm_platform_ioremap_resource(pdev, 0);
	if (IS_ERR(rep->base))
		return PTR_ERR(rep->base);
	rep->offsets = of_device_get_match_data(dev);
	if (rep->offsets->has_event_config) {
		rep->event_config = devm_platform_ioremap_resource_byname(pdev, "event-config");
		if (IS_ERR(rep->event_config))
			return PTR_ERR(rep->event_config);
	}
	ret = apple_pmp_report_read_device_flags(rep);
	if (ret)
		return ret;
	dev_set_drvdata(dev, rep);
	ret = of_platform_populate(np, NULL, NULL, dev);
	if (ret)
		return dev_err_probe(dev, ret, "failed to create child devices\n");

	if (rep->offsets->finalize_after_ready) {
		INIT_DELAYED_WORK(&rep->finalize_work, apple_pmp_report_finalize_work);
		schedule_delayed_work(&rep->finalize_work, 0);
	}

	return 0;
}

static void apple_pmp_report_remove(struct platform_device *pdev)
{
	struct apple_pmp_report *rep = platform_get_drvdata(pdev);

	if (rep->offsets->finalize_after_ready)
		cancel_delayed_work_sync(&rep->finalize_work);
	of_platform_depopulate(&pdev->dev);
}

/* J700 native ptd-range request/ack/status data, not a new PMP protocol. */
static const struct apple_pmp_report_offsets apple_pmp_offsets_t8140 = {
	.tgt_read = 0x1880,
	.tgt_write = 0x10c40,
	.actual = 0x18c0,
	.status = 0x10,
};

static const struct apple_pmp_report_offsets apple_pmp_offsets_t600x = {
	.tgt_read = 0xf80,
	.tgt_write = 0x107c0,
	.actual = 0x1000,
	.status = 0x10,
};

static const struct apple_pmp_report_offsets apple_pmp_offsets_t602x = {
	.tgt_read = 0x2000,
	.tgt_write = 0x11000,
	.actual = 0x2080,
	.status = 0x10,
};

static const struct apple_pmp_report_offsets apple_pmp_offsets_t6030 = {
	.tgt_read = 0x1180,
	.tgt_write = 0x108c0,
	.actual = 0x11c0,
	.status = 0x10,
};

static const struct apple_pmp_report_offsets apple_pmp_offsets_t8112 = {
	.tgt_read = 0xa00,
	.tgt_write = 0x10500,
	.actual = 0xa40,
	.status = 0x10,
};

static const struct apple_pmp_report_offsets apple_pmp_offsets_t8122 = {
	.tgt_read = 0x1000,
	.tgt_write = 0x10800,
	.actual = 0x1080,
	.status = 0x10,
};

/* Native T6041 report accesses: the read bank expands each PTD entry
 * to 16 bytes, while the update bank uses 8 bytes per entry.
 */
static const struct apple_pmp_report_offsets apple_pmp_offsets_t6041 = {
	.tgt_read = 0x1e00,
	.tgt_write = 0x10f00,
	.actual = 0x1e80,
	.status = 0x10,
};

static const struct apple_pmp_report_offsets apple_pmp_offsets_t8132 = {
	.tgt_read = 0x1880,
	.tgt_write = 0x10c40,
	.actual = 0x1900,
	.status = 0x10,
	.finalize_after_ready = true,
	.has_event_config = true,
};

static const struct of_device_id apple_pmp_report_of_match[] = {
	{ .compatible = "apple,t8132-pmp-v2-report", .data = &apple_pmp_offsets_t8132 },
	{ .compatible = "apple,t8140-pmp-v2-report", .data = &apple_pmp_offsets_t8140 },
	{ .compatible = "apple,t6041-pmp-v2-report", .data = &apple_pmp_offsets_t6041 },
	{ .compatible = "apple,t6000-pmp-v2-report", .data = &apple_pmp_offsets_t600x },
	{ .compatible = "apple,t6020-pmp-v2-report", .data = &apple_pmp_offsets_t602x },
	{ .compatible = "apple,t6030-pmp-v2-report", .data = &apple_pmp_offsets_t6030 },
	{ .compatible = "apple,t8112-pmp-v2-report", .data = &apple_pmp_offsets_t8112 },
	{ .compatible = "apple,t8122-pmp-v2-report", .data = &apple_pmp_offsets_t8122 },
	{}
};

static struct platform_driver apple_pmp_report_driver = {
	.probe = apple_pmp_report_probe,
	.remove = apple_pmp_report_remove,
	.driver = {
		.name = "apple-pmp-report",
		.of_match_table = apple_pmp_report_of_match,
	},
};

struct apple_pmp_report_entry {
	struct device *dev;
	struct generic_pm_domain genpd;
	u32 id;
};

#define genpd_to_apple_pmp_report_entry(_genpd) \
	container_of(_genpd, struct apple_pmp_report_entry, genpd)

static int apple_pmp_report_set_state(struct generic_pm_domain *genpd, bool enable)
{
	struct apple_pmp_report_entry *ent = genpd_to_apple_pmp_report_entry(genpd);
	struct apple_pmp_report *rep = dev_get_drvdata(ent->dev->parent);
	u64 bit_val = BIT_ULL(ent->id);
	u64 val;
	unsigned long flags;

	spin_lock_irqsave(&rep->lock, flags);
	val = readq(rep->base + rep->offsets->tgt_read);
	val &= ~bit_val;
	if (enable)
		val |= bit_val;
	writeq(val, rep->base + rep->offsets->tgt_write);
	spin_unlock_irqrestore(&rep->lock, flags);
	/* A notification-only record does not promise an acknowledgement. */
	if (!(rep->ack_mask & bit_val))
		return 0;
	val = readq(rep->base + rep->offsets->status);
	if ((val & PMP_REPORT_READY) == 0)
		return 0;
	return readq_poll_timeout_atomic(
		rep->base + rep->offsets->actual,
		val,
		!!(val & bit_val) == !!enable,
		100,
		50000);
}

static int apple_pmp_report_entry_power_on(struct generic_pm_domain *genpd)
{
	return apple_pmp_report_set_state(genpd, true);
}

static int apple_pmp_report_entry_power_off(struct generic_pm_domain *genpd)
{
	return apple_pmp_report_set_state(genpd, false);
}

static int apple_pmp_report_entry_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct device_node *node = dev->of_node;
	struct apple_pmp_report_entry *ent;
	struct apple_pmp_report *rep = dev_get_drvdata(dev->parent);
	int ret;
	const char *name;
	struct of_phandle_iterator it;
	bool always_on, boot_on;

	ent = devm_kzalloc(dev, sizeof(*ent), GFP_KERNEL);
	if (!ent)
		return -ENOMEM;

	ent->dev = dev;
	/* All report accesses use MMIO, a spinlock and atomic polling. This
	 * domain can therefore parent the IRQ-safe PMGR power-state domains.
	 */
	ent->genpd.flags |= GENPD_FLAG_IRQ_SAFE;

	ret = of_property_read_u32(node, "reg", &ent->id);
	if (ret)
		return dev_err_probe(dev, ret, "missing reg property\n");
	if (ent->id >= 64 || !(rep->valid_mask & BIT_ULL(ent->id)))
		return dev_err_probe(dev, -EINVAL, "report index outside PMP device table\n");

	ret = of_property_read_string(node, "label", &name);
	if (ret < 0)
		return dev_err_probe(dev, ret, "missing label property\n");

	always_on = of_property_read_bool(node, "apple,always-on");
	if (always_on)
		ent->genpd.flags |= GENPD_FLAG_ACTIVE_WAKEUP | GENPD_FLAG_ALWAYS_ON;
	/* Parent report domains must start on before adding powered children. */
	boot_on = always_on || of_property_read_bool(node, "apple,boot-on");

	ent->genpd.name = name;
	ent->genpd.power_on = apple_pmp_report_entry_power_on;
	ent->genpd.power_off = apple_pmp_report_entry_power_off;

	ret = pm_genpd_init(&ent->genpd, NULL, !boot_on);
	if (ret)
		return dev_err_probe(dev, ret, "pm_genpd_init failed\n");

	if (boot_on) {
		ret = apple_pmp_report_set_state(&ent->genpd, true);
		if (ret)
			goto err_remove_genpd;
	}

	ret = of_genpd_add_provider_simple(node, &ent->genpd);
	if (ret) {
		dev_err_probe(dev, ret, "of_genpd_add_provider_simple failed\n");
		goto err_remove_genpd;
	}

	of_for_each_phandle(&it, ret, node, "power-domains", "#power-domain-cells", -1) {
		struct of_phandle_args parent, child;

		parent.np = it.node;
		parent.args_count = of_phandle_iterator_args(&it, parent.args, MAX_PHANDLE_ARGS);
		child.np = node;
		child.args_count = 0;
		ret = of_genpd_add_subdomain(&parent, &child);

		if (ret == -EPROBE_DEFER) {
			of_node_put(parent.np);
			goto err_remove;
		} else if (ret < 0) {
			dev_err(dev, "failed to add to parent domain: %d (%s -> %s)\n",
				ret, it.node->name, node->name);
			of_node_put(parent.np);
			goto err_remove;
		}
	}

	pm_genpd_remove_device(dev);

	return 0;
err_remove:
	of_genpd_del_provider(node);
err_remove_genpd:
	pm_genpd_remove(&ent->genpd);
	return ret;
}

static const struct of_device_id apple_pmp_report_entry_of_match[] = {
	{ .compatible = "apple,t6000-pmp-v2-report-entry" },
	{}
};

static struct platform_driver apple_pmp_report_entry_driver = {
	.probe = apple_pmp_report_entry_probe,
	.driver = {
		.name = "apple-pmp-report-entry",
		.of_match_table = apple_pmp_report_entry_of_match,
	},
};

MODULE_DEVICE_TABLE(of, apple_pmp_report_of_match);
MODULE_DEVICE_TABLE(of, apple_pmp_report_entry_of_match);

static int __init apple_pmp_report_init(void)
{
	platform_driver_register(&apple_pmp_report_entry_driver);
	platform_driver_register(&apple_pmp_report_driver);
	return 0;
}

static void __exit apple_pmp_report_exit(void)
{
	platform_driver_unregister(&apple_pmp_report_entry_driver);
	platform_driver_unregister(&apple_pmp_report_driver);
}

module_init(apple_pmp_report_init);
module_exit(apple_pmp_report_exit);

MODULE_DESCRIPTION("PMP power state reporting driver for Apple SoCs");
MODULE_LICENSE("Dual MIT/GPL");
