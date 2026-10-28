// SPDX-License-Identifier: GPL-2.0-only
/* Private Neo capture-only ADMAC RX0 driver. Hardware programming follows
 * declared native MMIO captures; the existing Asahi ADMAC ring algorithm
 * supplies the descriptor/report model. No TX channel is mapped or started. */
#include <linux/dma-mapping.h>
#include <linux/interrupt.h>
#include <linux/io.h>
#include <linux/module.h>
#include <linux/of.h>
#include <linux/platform_device.h>
#include <sound/core.h>
#include <sound/pcm.h>
#include <sound/pcm_params.h>
#include "neo_mic.h"

#define CH 0xc000
#define RING_FULL BIT(9)
#define RING_EMPTY BIT(8)
#define RING_ERR BIT(10)
struct neo_pcm {
	struct device *dev;
	struct neo_mic *mic;
	struct snd_card *card;
	void __iomem *regs, *proxy;
	spinlock_t lock;
	struct snd_pcm_substream *stream;
	dma_addr_t addr;
	size_t size, period, submitted, completed;
	bool running, powered, irq_enabled;
	int irq;
	u32 descriptor_id;
};

static const struct snd_pcm_hardware neo_capture_hw = {
	.info = SNDRV_PCM_INFO_INTERLEAVED | SNDRV_PCM_INFO_BLOCK_TRANSFER,
	.formats = SNDRV_PCM_FMTBIT_FLOAT_LE,
	.rates = SNDRV_PCM_RATE_48000,
	.rate_min = 48000, .rate_max = 48000,
	.channels_min = 2, .channels_max = 2,
	.buffer_bytes_max = 256 * 1024,
	.period_bytes_min = 3840, .period_bytes_max = 32768,
	.periods_min = 4, .periods_max = 32,
};

static void neo_pcm_queue(struct neo_pcm *p)
{
	unsigned int count;
	for (count = 0; count < 4; count++) {
		dma_addr_t addr = p->addr + p->submitted;
		if (readl(p->regs + CH + 0x70) & RING_FULL) break;
		dma_wmb();
		writel(lower_32_bits(addr), p->proxy + 0x14000);
		writel(upper_32_bits(addr), p->proxy + 0x14000);
		writel(p->period, p->proxy + 0x14000);
		writel(0x110000 | (++p->descriptor_id & 0xffff), p->proxy + 0x14000);
		p->submitted = (p->submitted + p->period) % p->size;
	}
}

static void neo_pcm_stop(struct neo_pcm *p)
{
	if (p->irq_enabled) { disable_irq_nosync(p->irq); p->irq_enabled = false; }
	if (!p->powered) return;
	writel(0, p->regs + CH + 0x20);
	writel(1, p->regs + 0xc);
	writel(0, p->regs + CH + 0xa0);
	p->running = false;
}

static irqreturn_t neo_pcm_irq(int irq, void *cookie)
{
	struct neo_pcm *p = cookie;
	struct snd_pcm_substream *stream;
	unsigned long flags;
	u32 status, global, ring;
	unsigned int count, periods = 0;
	bool error = false;

	spin_lock_irqsave(&p->lock, flags);
	status = readl(p->regs + CH + 0x10);
	global = readl(p->regs + 0x50);
	if (!(readl(p->regs + 0x40) & 1) && !global) {
		spin_unlock_irqrestore(&p->lock, flags);
		return IRQ_NONE;
	}
	writel(~0U, p->regs + CH + 0x10);
	writel(~0U, p->regs + 0x50);
	stream = p->stream;
	if (p->running) {
		for (count = 0; count < 4; count++) {
			ring = readl(p->regs + CH + 0x74);
			if (ring & RING_ERR) { error = true; break; }
			if (ring & RING_EMPTY) break;
			/* Consume the complete four-word ADMAC report. */
			readl(p->regs + 0x14100);
			readl(p->regs + 0x14100);
			readl(p->regs + 0x14100);
			readl(p->regs + 0x14100);
			p->completed = (p->completed + p->period) % p->size;
			periods++;
		}
		dma_rmb();
		if (readl(p->regs + CH + 0x70) & RING_ERR) error = true;
		if (error) neo_pcm_stop(p);
		else neo_pcm_queue(p);
	}
	spin_unlock_irqrestore(&p->lock, flags);
	if (error) {
		dev_err_ratelimited(p->dev, "RX ring error status=%08x\n", status);
		if (stream) snd_pcm_stop_xrun(stream);
	} else {
		while (periods-- && stream) snd_pcm_period_elapsed(stream);
	}
	return IRQ_HANDLED;
}

static int neo_pcm_open(struct snd_pcm_substream *stream)
{
	struct neo_pcm *p = snd_pcm_substream_chip(stream);
	stream->runtime->hw = neo_capture_hw;
	WRITE_ONCE(p->stream, stream);
	return snd_pcm_hw_constraint_integer(stream->runtime, SNDRV_PCM_HW_PARAM_PERIODS);
}

static int neo_pcm_poweroff(struct neo_pcm *p)
{
	int error = 0, second;
	if (p->powered) {
		synchronize_irq(p->irq);
		error = neo_mic_power(p->mic, NEO_MIC_WARM, 1);
		second = neo_mic_power(p->mic, NEO_MIC_OFF, 0);
		if (!error) error = second;
		p->powered = false;
	}
	return error;
}

static int neo_pcm_close(struct snd_pcm_substream *stream)
{
	struct neo_pcm *p = snd_pcm_substream_chip(stream);
	unsigned long flags;
	spin_lock_irqsave(&p->lock, flags);
	neo_pcm_stop(p);
	p->stream = NULL;
	spin_unlock_irqrestore(&p->lock, flags);
	return neo_pcm_poweroff(p);
}

static int neo_pcm_hw_free(struct snd_pcm_substream *stream)
{
	return neo_pcm_poweroff(snd_pcm_substream_chip(stream));
}

static int neo_pcm_prepare(struct snd_pcm_substream *stream)
{
	struct neo_pcm *p = snd_pcm_substream_chip(stream);
	struct snd_pcm_runtime *runtime = stream->runtime;
	int i, error;

	/* Native AOP pw0 opens the gated RX controller before any MMIO. */
	if (!p->powered) {
		error = neo_mic_power(p->mic, NEO_MIC_WARM, 0);
		if (error) return error;
		p->powered = true;
	}
	neo_pcm_stop(p);
	p->addr = runtime->dma_addr;
	p->size = snd_pcm_lib_buffer_bytes(stream);
	p->period = snd_pcm_lib_period_bytes(stream);
	p->submitted = p->completed = p->descriptor_id = 0;
	if (!p->size || !p->period || p->size % p->period) return -EINVAL;
	memset(runtime->dma_area, 0, p->size);
	/* RX0 setup observed on Neo. Preserve unrelated global configuration. */
	writel(readl(p->regs + 0x10) & ~BIT(6), p->regs + 0x10);
	writel(0x30001, p->regs + CH);
	writel(0x30000, p->regs + CH);
	writel(0x500, p->regs + CH + 0x70);
	writel(0x500, p->regs + CH + 0x74);
	for (i = 0; i < 4; i++) writel(~0U, p->regs + CH + 0x10 + 4 * i);
	writel(~0U, p->regs + CH + 0x34);
	writel(0, p->proxy + CH + 0x88);
	writel(1, p->proxy + 0xc);
	writel(1, p->proxy + CH);
	writel(0, p->proxy + CH);
	writel(0xf02, p->regs + CH + 0x40);
	writel(0xc0, p->regs + CH + 0x54);
	writel(1, p->regs + 0x104);
	for (i = 0; i < 4; i++) writel(~0U, p->regs + 0x50 + 4 * i);
	writel(~0U, p->regs + 0x60);
	writel(0x29b0000, p->regs + CH + 4);
	writel(3, p->regs + CH + 0xa0);
	writel(0x302, p->regs + CH + 0x30);
	writel(0, p->regs + CH + 0xa4);
	neo_pcm_queue(p);
	dev_info(p->dev, "Capture prepared dma=%pad size=%zx period=%zx\n", &p->addr, p->size, p->period);
	error = neo_mic_power(p->mic, NEO_MIC_ON, 1);
	if (error) { neo_pcm_poweroff(p); return error; }
	return 0;
}

static int neo_pcm_trigger(struct snd_pcm_substream *stream, int command)
{
	struct neo_pcm *p = snd_pcm_substream_chip(stream);
	unsigned long flags;
	int error = 0;
	spin_lock_irqsave(&p->lock, flags);
	switch (command) {
	case SNDRV_PCM_TRIGGER_START:
		p->running = true;
		if (!p->irq_enabled) { enable_irq(p->irq); p->irq_enabled = true; }
		writel(~0U, p->regs + CH + 0x10);
		writel(0x10463, p->regs + CH + 0x20);
		writel(3, p->regs + CH + 0xa0);
		writel(1, p->regs + 8);
		break;
	case SNDRV_PCM_TRIGGER_STOP:
		neo_pcm_stop(p);
		break;
	default: error = -EINVAL;
	}
	spin_unlock_irqrestore(&p->lock, flags);
	return error;
}

static snd_pcm_uframes_t neo_pcm_pointer(struct snd_pcm_substream *stream)
{
	struct neo_pcm *p = snd_pcm_substream_chip(stream);
	return bytes_to_frames(stream->runtime, READ_ONCE(p->completed));
}
static const struct snd_pcm_ops neo_pcm_ops = {
	.open = neo_pcm_open, .close = neo_pcm_close,
	.ioctl = snd_pcm_lib_ioctl, .hw_free = neo_pcm_hw_free,
	.prepare = neo_pcm_prepare, .trigger = neo_pcm_trigger, .pointer = neo_pcm_pointer,
};

static int neo_pcm_probe(struct platform_device *pdev)
{
	struct neo_pcm *p;
	struct snd_card *card;
	struct snd_pcm *pcm;
	int error, irq;

	error = snd_devm_card_new(&pdev->dev, -1, "NeoMic", THIS_MODULE, sizeof(*p), &card);
	if (error) return error;
	p = card->private_data;
	p->dev = &pdev->dev;
	p->card = card;
	p->mic = neo_mic_get(p->dev);
	if (IS_ERR(p->mic)) return PTR_ERR(p->mic);
	spin_lock_init(&p->lock);
	error = dma_set_mask_and_coherent(p->dev, DMA_BIT_MASK(42));
	if (error) return error;
	p->regs = devm_platform_ioremap_resource(pdev, 0);
	if (IS_ERR(p->regs)) return PTR_ERR(p->regs);
	p->proxy = devm_platform_ioremap_resource(pdev, 1);
	if (IS_ERR(p->proxy)) return PTR_ERR(p->proxy);
	irq = platform_get_irq(pdev, 0);
	if (irq < 0) return irq;
	/* The RX aperture is gated until AOP accepts a pw0 request. */
	p->irq = irq;
	error = devm_request_irq(p->dev, irq, neo_pcm_irq, IRQF_NO_AUTOEN, "neo-mic-rx0", p);
	if (error) return error;
	error = snd_pcm_new(card, "Neo Microphone", 0, 0, 1, &pcm);
	if (error) return error;
	pcm->private_data = p;
	strscpy(pcm->name, "Neo built-in microphone", sizeof(pcm->name));
	snd_pcm_set_ops(pcm, SNDRV_PCM_STREAM_CAPTURE, &neo_pcm_ops);
	error = snd_pcm_set_managed_buffer_all(pcm, SNDRV_DMA_TYPE_DEV, p->dev, 128 * 1024, 256 * 1024);
	if (error) return error;
	strscpy(card->driver, "NeoMic", sizeof(card->driver));
	strscpy(card->shortname, "Neo Microphone", sizeof(card->shortname));
	strscpy(card->longname, "Neo AOP secure RX0 microphone", sizeof(card->longname));
	platform_set_drvdata(pdev, p);
	return snd_card_register(card);
}
static const struct of_device_id neo_pcm_of_match[] = {
	{ .compatible = "apple,t8140-neo-mic-capture" }, {}
};
MODULE_DEVICE_TABLE(of, neo_pcm_of_match);
static struct platform_driver neo_pcm_driver = {
	.probe = neo_pcm_probe,
	.driver = { .name = "neo-mic-capture", .of_match_table = neo_pcm_of_match },
};
module_platform_driver(neo_pcm_driver);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Private Neo ALSA microphone capture");
