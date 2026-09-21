// SPDX-License-Identifier: GPL-2.0-only OR MIT
/*
 * Apple Generic RTKit helper coprocessor
 * Copyright The Asahi Linux Contributors
 */

#include <linux/device.h>
#include <linux/dma-mapping.h>
#include <linux/io.h>
#include <linux/ioport.h>
#include <linux/of.h>
#include <linux/mutex.h>
#include <linux/of_platform.h>
#include <linux/platform_device.h>
#include <linux/soc/apple/rtkit.h>

#define APPLE_ASC_CPU_CONTROL		0x44
#define APPLE_ASC_CPU_CONTROL_RUN	BIT(4)

struct apple_rtkit_helper {
	struct device *dev;
	struct apple_rtkit *rtk;
	struct mutex lock;
	bool started;
	bool stopping;
	bool stopped;

	void __iomem *asc_base;

	struct resource *sram;
	void __iomem *sram_base;
};

static int apple_rtkit_helper_shmem_setup(void *cookie, struct apple_rtkit_shmem *bfr)
{
	struct apple_rtkit_helper *helper = cookie;
	struct resource res = {
		.start = bfr->iova,
		.end = bfr->iova + bfr->size - 1,
		.name = "rtkit_map",
	};

	if (!bfr->iova) {
		bfr->buffer = dma_alloc_coherent(helper->dev, bfr->size,
						    &bfr->iova, GFP_KERNEL);
		if (!bfr->buffer)
			return -ENOMEM;
		return 0;
	}

	if (!helper->sram) {
		dev_err(helper->dev,
			"RTKit buffer request with no SRAM region: %pR", &res);
		return -EFAULT;
	}

	res.flags = helper->sram->flags;

	if (res.end < res.start || !resource_contains(helper->sram, &res)) {
		dev_err(helper->dev,
			"RTKit buffer request outside SRAM region: %pR", &res);
		return -EFAULT;
	}

	bfr->iomem = helper->sram_base + (res.start - helper->sram->start);
	bfr->is_mapped = true;

	return 0;
}

static void apple_rtkit_helper_shmem_destroy(void *cookie, struct apple_rtkit_shmem *bfr)
{
	struct apple_rtkit_helper *helper = cookie;

	if (!bfr->buffer)
		return;

	/*
	 * RTKit destroys buffers after the acknowledged shutdown transition,
	 * or during final devres cleanup. Only the former establishes that a
	 * running coprocessor has relinquished DMA ownership.
	 */
	if (helper->started && !helper->stopping && !helper->stopped) {
		dev_crit(helper->dev, "Retaining RTKit DMA buffer after failed shutdown\n");
		get_device(helper->dev);
		return;
	}
	dma_free_coherent(helper->dev, bfr->size, bfr->buffer, bfr->iova);
}

static const struct apple_rtkit_ops apple_rtkit_helper_ops = {
	.shmem_setup = apple_rtkit_helper_shmem_setup,
	.shmem_destroy = apple_rtkit_helper_shmem_destroy,
};

bool apple_rtkit_helper_is_running(struct device *dev)
{
	struct apple_rtkit_helper *helper = dev_get_drvdata(dev);
	bool running;

	mutex_lock(&helper->lock);
	running = !helper->stopped && apple_rtkit_is_running(helper->rtk);
	mutex_unlock(&helper->lock);
	return running;
}
EXPORT_SYMBOL_GPL(apple_rtkit_helper_is_running);

int apple_rtkit_helper_stop(struct device *dev)
{
	struct apple_rtkit_helper *helper = dev_get_drvdata(dev);
	int ret = 0;

	mutex_lock(&helper->lock);
	if (helper->stopped)
		goto done;

	helper->stopping = true;
	ret = apple_rtkit_shutdown(helper->rtk);
	helper->stopping = false;
	if (ret) {
		dev_err(dev, "Coprocessor shutdown failed: %d; DMA must remain mapped\n", ret);
		goto done;
	}
	helper->stopped = true;
	writel(0, helper->asc_base + APPLE_ASC_CPU_CONTROL);
done:
	mutex_unlock(&helper->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(apple_rtkit_helper_stop);

static int apple_rtkit_helper_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct apple_rtkit_helper *helper;
	int ret;

	/* 44 bits for addresses in standard RTKit requests */
	ret = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(44));
	if (ret)
		return ret;

	helper = devm_kzalloc(dev, sizeof(*helper), GFP_KERNEL);
	if (!helper)
		return -ENOMEM;

	helper->dev = dev;
	mutex_init(&helper->lock);
	platform_set_drvdata(pdev, helper);

	helper->asc_base = devm_platform_ioremap_resource_byname(pdev, "asc");
	if (IS_ERR(helper->asc_base))
		return PTR_ERR(helper->asc_base);

	helper->sram = platform_get_resource_byname(pdev, IORESOURCE_MEM, "sram");
	if (helper->sram) {
		helper->sram_base = devm_ioremap_resource(dev, helper->sram);
		if (IS_ERR(helper->sram_base))
			return dev_err_probe(dev, PTR_ERR(helper->sram_base),
					"Failed to map SRAM region");
	}

	helper->rtk =
		devm_apple_rtkit_init(dev, helper, NULL, 0, &apple_rtkit_helper_ops);
	if (IS_ERR(helper->rtk))
		return dev_err_probe(dev, PTR_ERR(helper->rtk),
				     "Failed to intialize RTKit");

	helper->started = true;
	writel_relaxed(APPLE_ASC_CPU_CONTROL_RUN,
		       helper->asc_base + APPLE_ASC_CPU_CONTROL);

	/* Works for both wake and boot */
	ret = apple_rtkit_wake(helper->rtk);
	if (ret) {
		apple_rtkit_helper_stop(dev);
		return dev_err_probe(dev, ret, "Failed to wake up coprocessor");
	}

	return 0;
}

static void apple_rtkit_helper_remove(struct platform_device *pdev)
{
	apple_rtkit_helper_stop(&pdev->dev);
}

static const struct of_device_id apple_rtkit_helper_of_match[] = {
	{ .compatible = "apple,rtk-helper-asc4" },
	{},
};
MODULE_DEVICE_TABLE(of, apple_rtkit_helper_of_match);

static struct platform_driver apple_rtkit_helper_driver = {
	.driver = {
		.name = "rtkit-helper",
		.of_match_table = apple_rtkit_helper_of_match,
	},
	.probe = apple_rtkit_helper_probe,
	.remove = apple_rtkit_helper_remove,
	.shutdown = apple_rtkit_helper_remove,
};
module_platform_driver(apple_rtkit_helper_driver);

MODULE_AUTHOR("Hector Martin <marcan@marcan.st>");
MODULE_LICENSE("Dual MIT/GPL");
MODULE_DESCRIPTION("Apple RTKit helper driver");
