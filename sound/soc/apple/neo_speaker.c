// SPDX-License-Identifier: GPL-2.0-only
/* Protected AOP playback. Immutable 3/256 gain, per-stream DMA representation,
 * two-node thermal admission, and amplifier-first fail-closed cutoff.
 *
 * TODO: This quiet fixed volume cap is temporary. Replace it with calibrated
 * waveform-dependent mechanical/thermal limits to support higher playback
 * levels, while retaining an absolute peak ceiling and amplifier cutoff. */
#include <linux/bitfield.h>
#include <linux/delay.h>
#include <linux/dma-mapping.h>
#include <linux/dmaengine.h>
#include <linux/gpio/consumer.h>
#include <linux/io.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/platform_device.h>
#include <linux/panic_notifier.h>
#include <linux/of.h>
#include <linux/of_device.h>
#include <linux/hrtimer.h>
#include <linux/uio.h>
#include <linux/workqueue.h>
#include <sound/core.h>
#include <sound/pcm.h>
#include <sound/soc.h>
#include "../codecs/tas2764.h"
#include "neo_mic.h"
#include "pcm_guard.h"
#include "neo_thermal.h"

#define CH 0x8000
#define NEO_SPEAKER_MAX_CHANNELS 4
/* Shared ADMAC register fields, also documented in apple-admac.c. */
#define TX_SRAM_INFO 0x94 /* Evolution layout differs from legacy size register. */
#define CH_BUS_WIDTH 0x40
#define CH_SRAM_CARVEOUT 0x50
#define SRAM_SIZE GENMASK(31, 16)
#define SRAM_BASE GENMASK(15, 0)
#define CH_FIFO_CONTROL 0x54
#define FIFO_LIMIT GENMASK(31, 16)
#define FIFO_THRESHOLD GENMASK(15, 0)
#define TX_SRAM_BYTES 384
#define PERIOD 126720 /* native block size; zeros keep the active-stop test silent */
#define ZERO_DMA_PERIOD 3840 /* 240 four-lane frames,5ms at48kHz; divides PERIOD */
static bool zero_only;
module_param(zero_only, bool, 0400);
MODULE_PARM_DESC(zero_only, "Run immutable-zero DMA validation without exposing playback");
struct neo_speaker_layout {
	bool float_dma;
	bool start_before_power;
};

/* LEAP consumes floats; the MCA path carries signed 24-bit PCM in 32-bit words. */
static const struct neo_speaker_layout neo_leap_layout = {
	.float_dma = true, .start_before_power = true,
};
static const struct neo_speaker_layout neo_mca_layout = {
	.float_dma = false, .start_before_power = false,
};

struct neo_speaker {
	/* ASoC replaces this device's driver_data with &soc_card. An initial
	 * member has the same address as its containing object, so the existing
	 * platform/PCM private-data access also remains valid on this path. */
	struct snd_soc_card soc_card;
	struct snd_soc_dai_link soc_link;
	struct snd_soc_dai_link_component soc_cpu, soc_platform;
	struct snd_soc_dai_link_component soc_codecs[NEO_SPEAKER_MAX_CHANNELS];
	struct snd_soc_codec_conf codec_conf[NEO_SPEAKER_MAX_CHANNELS];
	struct snd_soc_component *codecs[NEO_SPEAKER_MAX_CHANNELS];
	unsigned int num_codecs;
	bool codecs_bound;
	struct device *dev;
	struct neo_mic *aop;
	const struct neo_speaker_layout *layout;
	struct gpio_desc *enable;
	void __iomem *regs;
	struct dma_chan *dma;
	struct device *dma_dev;
	dma_cookie_t cookie;
	bool dma_queued;
	u32 dma_periods;
	struct mutex lock;
	void *silence;
	dma_addr_t iova;
	bool powered, cutoff_verified;
	bool mute_unavailable;
	u32 descriptor;
	spinlock_t dma_lock;
	struct hrtimer timer;
	struct delayed_work notify_work;
	bool notify_pending, fault_pending;
	struct notifier_block panic;
	struct snd_pcm_substream *stream;
	struct thermal thermal[NEO_SPEAKER_MAX_CHANNELS];
	u32 channels, frame_bytes, tx_sram_base;
	bool opened, running, active, disabled;
	u32 rptr, period_bytes, size, active_bytes, desc_bytes[4];
	u64 submitted, completed, hwbytes, notified, progress_ns;
	u64 last_appl;
	u32 polls, notify_runs, notify_busy;
	u64 nonzero_samples, faults;
};

static void neo_codecs_reset(struct neo_speaker *p)
{
#if IS_ENABLED(CONFIG_SND_SOC_APPLE_AOP_SPEAKER_CODECS)
	unsigned int i;

	if (p->codecs_bound)
		for (i = 0; i < p->num_codecs; i++)
			tas2764_external_reset(p->codecs[i]);
#endif
}

static int neo_codecs_restore(struct neo_speaker *p)
{
#if IS_ENABLED(CONFIG_SND_SOC_APPLE_AOP_SPEAKER_CODECS)
	unsigned int i;
	int ret;

	if (p->codecs_bound) {
		usleep_range(1000, 2000);
		for (i = 0; i < p->num_codecs; i++) {
			ret = tas2764_external_restore(p->codecs[i]);
			if (ret)
				return ret;
		}
	}
#endif
	return 0;
}

static int neo_amp_level(struct neo_speaker *p)
{
	return gpiod_get_value(p->enable);
}

static bool neo_pin_is_output(struct neo_speaker *p, bool enabled)
{
	return gpiod_get_direction(p->enable) == GPIO_LINE_DIRECTION_OUT &&
	       neo_amp_level(p) == enabled;
}

static void neo_amp_off(struct neo_speaker *p)
{
	gpiod_set_value(p->enable, 0);
}

static void neo_speaker_cutoff(struct neo_speaker *p)
{
	neo_amp_off(p);
	/* GPIO cutoff precedes all transport calls and DMA manipulation. */
	if (!p->powered)
		return;
	if (p->dma) {
		if (p->dma_queued) {
			dmaengine_terminate_async(p->dma);
			p->dma_queued = false;
		}
		return;
	}
	writel(0, p->regs + CH + 0x20);
	writel(1, p->regs + 4);
	writel(0, p->regs + CH + 0xa0);
	readl(p->regs + 0); /* flush posted disable */
}

static int neo_speaker_set_mute(struct neo_speaker *p, bool muted)
{
	__le32 value = cpu_to_le32(muted);
	int error;

	if (p->mute_unavailable)
		return 0;
	error = neo_audio_set(p->aop, NEO_SPEAKER_DEVICE, 700, &value, sizeof(value));
	if (error == -ENOENT || error == -EOPNOTSUPP) {
		p->mute_unavailable = true;
		dev_info(p->dev, "Firmware mute property unavailable; amplifier GPIO supplies cutoff\n");
		return 0;
	}
	return error;
}

static int neo_speaker_mute(struct neo_speaker *p)
{
	return neo_speaker_set_mute(p, true);
}

static int neo_speaker_off(struct neo_speaker *p)
{
	int first, second;
	neo_speaker_cutoff(p);
	first = neo_speaker_mute(p);
	second = neo_audio_power(p->aop, NEO_SPEAKER_DEVICE, NEO_MIC_WARM, 2);
	neo_speaker_cutoff(p);
	if (!first) first = second;
	p->powered = false; /* idle closes the MMIO aperture before it replies */
	second = neo_audio_power(p->aop, NEO_SPEAKER_DEVICE, NEO_MIC_OFF, 0);
	/* Firmware power transitions may also program the pin. Assert it last. */
	neo_speaker_cutoff(p);
	if (!first) first = second;
	return first;
}

static void neo_queue_zero(struct neo_speaker *p)
{
	dma_wmb();
	writel(lower_32_bits(p->iova), p->regs + 0x10000);
	writel(upper_32_bits(p->iova), p->regs + 0x10000);
	writel(PERIOD, p->regs + 0x10000);
	/* Native NS TX0 flags differ from secure RX0; REPEAT remains clear. */
	writel(0xc0000 | (++p->descriptor & 0xffff), p->regs + 0x10000);
}

static void neo_tx_setup(struct neo_speaker *p)
{
	int i;
	/* Native TX0 setup. The aperture is accessed only after AOP pw0. */
	writel(0x18080001, p->regs + 0x10);
	writel(0x30001, p->regs + CH);
	writel(0x30000, p->regs + CH);
	writel(0x500, p->regs + CH + 0x70);
	writel(0x500, p->regs + CH + 0x74);
	for (i = 0; i < 4; i++) writel(~0U, p->regs + CH + 0x10 + 4 * i);
	writel(~0U, p->regs + CH + 0x34);
	writel(0, p->regs + CH + 0x88);
	writel(1, p->regs + 4);
	writel(readl(p->regs + CH) | BIT(0), p->regs + CH);
	writel(readl(p->regs + CH) & ~BIT(0), p->regs + CH);
	writel(2, p->regs + CH + CH_BUS_WIDTH); /* 32-bit DMA words */
	writel(FIELD_PREP(SRAM_SIZE, TX_SRAM_BYTES) |
	       FIELD_PREP(SRAM_BASE, p->tx_sram_base),
	       p->regs + CH + CH_SRAM_CARVEOUT);
	writel(FIELD_PREP(FIFO_LIMIT, TX_SRAM_BYTES) |
	       FIELD_PREP(FIFO_THRESHOLD, TX_SRAM_BYTES / 2),
	       p->regs + CH + CH_FIFO_CONTROL);
	writel(1, p->regs + 0x104);
	writel(~0U, p->regs + 0x30);
	writel(0x29b0000, p->regs + CH + 4);
	writel(3, p->regs + CH + 0xa0);
	writel(0x302, p->regs + CH + 0x30);
	writel(0, p->regs + CH + 0xa4);
}

static int neo_zero_prepare(struct neo_speaker *p)
{
	int i, error;
	neo_speaker_cutoff(p);
	/* 702/508 are also rejected by native macOS; they are not prerequisites.
	 * Mute 700 is issued after pwrd in the native sequence, not while idle. */
	error = neo_audio_power(p->aop, NEO_SPEAKER_DEVICE, NEO_MIC_WARM, 0);
	if (error) return error;
	p->powered = true;
	neo_speaker_cutoff(p);
	dev_info(p->dev, "TX SRAM info=%08x previous_carveout=%08x previous_fifo=%08x requested_base=%u size=%u\n",
		 readl(p->regs + TX_SRAM_INFO),
		 readl(p->regs + CH + CH_SRAM_CARVEOUT),
		 readl(p->regs + CH + CH_FIFO_CONTROL), p->tx_sram_base, TX_SRAM_BYTES);
	neo_tx_setup(p);
	for (i = 0; i < 4; i++) neo_queue_zero(p);
	/* LEAP enables TX before pwrd; MCA permits TX after the power reply.
	 * Keep the amplifier off; descriptors contain only immutable zeros. */
	if (p->layout->start_before_power)
		writel(1, p->regs);
	error = neo_audio_power(p->aop, NEO_SPEAKER_DEVICE, NEO_MIC_ON, 2);
	neo_amp_off(p);
	if (error) return error;
	{
		error = neo_speaker_set_mute(p, false);
		neo_amp_off(p);
		if (error) return error;
	}
	return 0;
}

/* First exercise the shared controller with immutable zero data. No DMA
 * registers are mapped by this client; the existing ADMAC driver owns them.
 */
static void neo_dma_period(void *cookie)
{
	struct neo_speaker *p = cookie;
	unsigned long flags;
	bool notify = false;

	spin_lock_irqsave(&p->dma_lock, flags);
	p->dma_periods++;
	if (p->running && p->stream) {
		p->completed += p->period_bytes;
		p->hwbytes = max(p->hwbytes, p->completed);
		p->progress_ns = ktime_get_ns();
		p->notify_pending = true;
		notify = true;
	}
	spin_unlock_irqrestore(&p->dma_lock, flags);
	/* The card's power RPCs require a non-atomic stream. Never take its
	 * sleeping ALSA mutex in the DMA tasklet.
	 */
	if (notify)
		mod_delayed_work(system_percpu_wq, &p->notify_work, 0);
}

static int neo_dma_prepare(struct neo_speaker *p, dma_addr_t address,
			   size_t bytes, size_t period)
{
	struct dma_slave_config config = {
		.direction = DMA_MEM_TO_DEV,
		.dst_addr_width = DMA_SLAVE_BUSWIDTH_4_BYTES,
		.dst_port_window_size = 1,
	};
	struct dma_async_tx_descriptor *descriptor;
	int error;

	error = dmaengine_terminate_sync(p->dma);
	if (error)
		return error;
	error = dmaengine_slave_config(p->dma, &config);
	if (error)
		return error;
	p->dma_periods = 0;
	descriptor = dmaengine_prep_dma_cyclic(p->dma, address, bytes, period,
					     DMA_MEM_TO_DEV, DMA_PREP_INTERRUPT | DMA_CTRL_ACK);
	if (!descriptor)
		return -ENOMEM;
	descriptor->callback = neo_dma_period;
	descriptor->callback_param = p;
	p->cookie = dmaengine_submit(descriptor);
	error = dma_submit_error(p->cookie);
	if (error)
		return error;
	/* No hardware transfer has started yet; mark the queued transaction for
	 * cleanup even if a subsequent firmware call fails.
	 */
	p->dma_queued = true;
	return 0;
}

static int neo_dma_zero_test(struct neo_speaker *p)
{
	struct dma_tx_state state = {};
	u32 initial, remaining, stopped, stable;
	u64 begin, elapsed;
	int error;

	error = neo_audio_power(p->aop, NEO_SPEAKER_DEVICE, NEO_MIC_WARM, 0);
	neo_amp_off(p);
	if (error)
		return error;
	p->powered = true;
	error = neo_dma_prepare(p, p->iova, PERIOD, ZERO_DMA_PERIOD);
	if (error)
		return error;
	error = neo_audio_power(p->aop, NEO_SPEAKER_DEVICE, NEO_MIC_ON, 2);
	neo_amp_off(p);
	if (error)
		return error;
	error = neo_speaker_set_mute(p, false);
	neo_amp_off(p);
	if (error)
		return error;
	gpiod_set_value(p->enable, 1);
	if (!neo_pin_is_output(p, true))
		return -EIO;
	dma_async_issue_pending(p->dma);
	if (dmaengine_tx_status(p->dma, p->cookie, &state) != DMA_IN_PROGRESS)
		return -EIO;
	initial = state.residue;
	begin = ktime_get_ns();
	/* Run beyond the four-descriptor hardware ring: progress must depend
	 * on the shared driver's report IRQ and descriptor refill handling.
	 */
	msleep(80);
	if (dmaengine_tx_status(p->dma, p->cookie, &state) != DMA_IN_PROGRESS)
		return -EIO;
	remaining = state.residue;
	elapsed = ktime_get_ns() - begin;
	/* Cut amplifier power before pausing. Keep the transaction intact while
	 * checking its physical residue; terminated-cookie bookkeeping cannot
	 * establish that the DMA hardware actually stopped.
	 */
	neo_amp_off(p);
	error = dmaengine_pause(p->dma);
	if (error)
		return error;
	dmaengine_tx_status(p->dma, p->cookie, &state);
	stopped = state.residue;
	msleep(5);
	dmaengine_tx_status(p->dma, p->cookie, &state);
	stable = state.residue;
	if (!initial || initial > PERIOD || !remaining || remaining >= initial ||
	    stopped != stable || !neo_pin_is_output(p, false) || READ_ONCE(p->dma_periods) < 4)
		error = -EIO;
	dev_info(p->dev, "DMAengine zero cutoff initial=%u remaining=%u elapsed_ns=%llu stopped=%u stable=%u periods=%u gpio=%u result=%d\n",
		 initial, remaining, elapsed, stopped, stable, READ_ONCE(p->dma_periods), neo_amp_level(p), error);
	return error;
}

static ssize_t cutoff_test_store(struct device *dev, struct device_attribute *attr,
				const char *buf, size_t size)
{
	struct neo_speaker *p = dev_get_drvdata(dev);
	u32 before, after, control, initial, residue, stopped, stable, desc;
	u64 begin_ns, elapsed_ns;
	int error = 0, off_error;
	bool run;
	if (kstrtobool(buf, &run) || !run) return -EINVAL;
	mutex_lock(&p->lock);
	if (p->opened) { mutex_unlock(&p->lock); return -EBUSY; }
	neo_codecs_reset(p);
	p->cutoff_verified = false;
	if (p->dma) {
		error = neo_dma_zero_test(p);
		goto off;
	}
	error = neo_zero_prepare(p);
	if (error) goto off;
	/* The sole buffer is allocated zeroed and never written with user data. */
	gpiod_set_value(p->enable, 1);
	before = neo_amp_level(p);
	if (!neo_pin_is_output(p, true)) { error = -EIO; goto off; }
	writel(0, p->regs + CH + 0x20); /* poll-only test; no IRQ enabled */
	writel(3, p->regs + CH + 0xa0);
	writel(1, p->regs + 0);
	dev_info(dev, "TX active formatter=%08x bus_width=%08x carveout=%08x fifo=%08x channels=%u\n",
		 readl(p->regs + CH + 4), readl(p->regs + CH + CH_BUS_WIDTH),
		 readl(p->regs + CH + CH_SRAM_CARVEOUT),
		 readl(p->regs + CH + CH_FIFO_CONTROL), p->channels);
	dev_info(dev, "Zero-PCM TX start ctl=%08x en=%08x desc=%08x report=%08x residue=%08x\n",
		 readl(p->regs + CH), readl(p->regs), readl(p->regs + CH + 0x70),
		 readl(p->regs + CH + 0x74), readl(p->regs + CH + 0x64));
	initial = readl(p->regs + CH + 0x64);
	begin_ns = ktime_get_ns();
	msleep(20);
	residue = readl(p->regs + CH + 0x64);
	elapsed_ns = ktime_get_ns() - begin_ns;
	dev_info(dev, "Zero-PCM DMA observation initial=%u remaining=%u elapsed_ns=%llu\n",
		 initial, residue, elapsed_ns);
	desc = readl(p->regs + CH + 0x70);
	if (!residue || residue >= initial || desc & BIT(10)) error = -EIO;
	neo_speaker_cutoff(p);
	after = neo_amp_level(p);
	control = readl(p->regs + 0);
	stopped = readl(p->regs + CH + 0x64);
	msleep(5);
	stable = readl(p->regs + CH + 0x64);
	if (!neo_pin_is_output(p, false) || control & 1) error = -EIO;
	if (stopped != stable || readl(p->regs) & 1) error = -EIO;
	dev_info(dev, "Zero-PCM cutoff test pin=%08x->%08x tx_en=%08x desc=%08x remaining=%u->%u stopped=%u stable=%u result=%d\n",
		 before, after, control, desc, initial, residue, stopped, stable, error);
off:
	off_error = neo_speaker_off(p);
	if (p->dma)
		dmaengine_synchronize(p->dma);
	if (!error) error = off_error;
	if (!error && !neo_pin_is_output(p, false)) error = -EIO;
	p->cutoff_verified = !error;
	if (!error) p->disabled = false;
	mutex_unlock(&p->lock);
	return error ? error : size;
}
static DEVICE_ATTR_WO(cutoff_test);

static void neo_pcm_stop_locked(struct neo_speaker *p)
{
	neo_speaker_cutoff(p);
	p->running = false;
}

/* Two millisecond polling is deliberate: this NS controller does not produce
 * legacy report-FIFO completions with the native descriptor flags. */
static void neo_play_queue(struct neo_speaker *p)
{
	struct snd_pcm_runtime *rt = p->stream->runtime;
	u64 appl = READ_ONCE(rt->control->appl_ptr);
	u64 sent = (p->submitted / p->frame_bytes) % rt->boundary;
	u64 ready = (appl + rt->boundary - sent) % rt->boundary;
	unsigned int count = 0;
	/* Never let a forged appl_ptr or a fast-draining ring spin unbounded. */
	ready = min_t(u64, ready, p->size / p->frame_bytes);
	while (count++ < 4 && ready &&
	       (ready >= p->period_bytes / p->frame_bytes ||
		READ_ONCE(rt->state) == SNDRV_PCM_STATE_DRAINING) &&
	       !(readl(p->regs + CH + 0x70) & BIT(9))) {
		u32 bytes = min_t(u64, ready * p->frame_bytes, p->period_bytes);
		u32 slot = readl(p->regs + CH + 0x70) & 3;
		dma_addr_t addr = rt->dma_addr + p->submitted % p->size;
		dma_wmb();
		writel(lower_32_bits(addr), p->regs + 0x10000);
		writel(upper_32_bits(addr), p->regs + 0x10000);
		writel(bytes, p->regs + 0x10000);
		p->desc_bytes[slot] = bytes;
		writel(0xc0000 | (++p->descriptor & 0xffff), p->regs + 0x10000);
		p->submitted += bytes;
		ready -= bytes / p->frame_bytes;
	}
}

/* Non-atomic PCM permits native power-on at START after samples are primed.
 * Never block an ALSA mutex in hrtimer context. The worker uses trylock so
 * sync_stop can cancel it even while core holds the stream mutex. */
static void neo_play_notify(struct work_struct *work)
{
	struct neo_speaker *p = container_of(to_delayed_work(work), struct neo_speaker, notify_work);
	struct snd_pcm_substream *stream = READ_ONCE(p->stream);
	unsigned long flags;
	bool fault, notify;
	if (!stream) return;
	if (!mutex_trylock(&stream->self_group.mutex)) {
		p->notify_busy++;
		mod_delayed_work(system_percpu_wq, &p->notify_work, msecs_to_jiffies(1));
		return;
	}
	spin_lock_irqsave(&p->dma_lock, flags);
	p->notify_runs++;
	fault = p->fault_pending;
	notify = p->notify_pending;
	p->fault_pending = p->notify_pending = false;
	spin_unlock_irqrestore(&p->dma_lock, flags);
	if (stream->runtime) {
		if (fault && snd_pcm_running(stream)) snd_pcm_stop(stream, SNDRV_PCM_STATE_XRUN);
		else if (notify) snd_pcm_period_elapsed_under_stream_lock(stream);
	}
	mutex_unlock(&stream->self_group.mutex);
}

/* dma_lock held. IRQ callbacks count whole periods; the DMAengine residue
 * supplies the position within the current ring. No descriptor-index guesses
 * or direct controller register accesses are needed by the PCM client.
 */
static bool neo_dma_poll(struct neo_speaker *p, struct snd_pcm_substream *stream,
			 u64 now, bool *notify)
{
	struct snd_pcm_runtime *runtime = stream->runtime;
	struct dma_tx_state state = {};
	enum dma_status status;
	u64 appl = READ_ONCE(runtime->control->appl_ptr);
	u64 advance = (appl + runtime->boundary - p->last_appl) % runtime->boundary;
	u64 old = p->hwbytes, position;
	bool fatal, starved;

	status = dmaengine_tx_status(p->dma, p->cookie, &state);
	fatal = advance > p->size / p->frame_bytes || state.residue > p->size ||
		status == DMA_ERROR || !neo_audio_healthy(p->aop);
	if (!fatal) {
		p->submitted += advance * p->frame_bytes;
		p->last_appl = appl;
		position = p->completed - p->completed % p->size +
			(p->size - state.residue) % p->size;
		if (position < p->completed)
			position += p->size;
		if (position < old)
			fatal = true;
		else
			p->hwbytes = position;
	}
	if (p->hwbytes != old)
		p->progress_ns = now;
	fatal |= now - p->progress_ns > 50000000;
	starved = p->hwbytes >= p->submitted;
	if (fatal || starved) {
		/* A cyclic transfer can revisit old ring contents. Cut off before
		 * any sleeping ALSA notification; the existing thermal model bounds
		 * the ring and held/replayed samples through the stall window.
		 */
		neo_pcm_stop_locked(p);
		if (fatal) {
			p->disabled = true;
			p->cutoff_verified = false;
			p->faults++;
		}
		if (!fatal && READ_ONCE(runtime->state) == SNDRV_PCM_STATE_DRAINING) {
			p->hwbytes = p->submitted;
			*notify = true;
			return false;
		}
		dev_info(p->dev, "DMAengine playback stopped fatal=%u status=%u residue=%u periods=%u submitted=%llu completed=%llu hwbytes=%llu appl=%llu\n",
			 fatal, status, state.residue, p->dma_periods, p->submitted,
			 p->completed, p->hwbytes, appl);
		return true;
	}
	return false;
}

static enum hrtimer_restart neo_play_poll(struct hrtimer *timer)
{
	struct neo_speaker *p = container_of(timer, struct neo_speaker, timer);
	struct snd_pcm_substream *stream;
	unsigned long flags;
	u32 ring, residue, rptr, delta, slot;
	u64 queued;
	u64 now, old;
	bool fault = false, notify = false, running, starved, fatal;
	spin_lock_irqsave(&p->dma_lock, flags);
	stream = p->stream;
	if (!p->running || !p->powered || !stream) goto unlock;
	/* Serialize the time sample with IRQ updates to progress_ns, including
	 * callbacks on another CPU, so unsigned elapsed time cannot underflow.
	 */
	now = ktime_get_ns();
	p->polls++;
	if (p->dma) {
		fault = neo_dma_poll(p, stream, now, &notify);
		goto unlock;
	}
	ring = readl(p->regs + CH + 0x70);
	residue = readl(p->regs + CH + 0x64);
	rptr = (ring >> 4) & 3;
	delta = (rptr - p->rptr) & 3;
	slot = p->rptr;
	p->rptr = rptr;
	old = p->hwbytes;
	while (delta--) {
		if (p->active) p->completed += p->active_bytes;
		p->active_bytes = p->desc_bytes[slot];
		slot = (slot + 1) & 3;
		p->active = true;
	}
	if ((ring & BIT(8)) && !residue) {
		p->completed = p->submitted;
		p->active = false;
	}
	p->hwbytes = p->completed;
	if (p->active) p->hwbytes += p->active_bytes - min(residue, p->active_bytes);
	if (p->hwbytes != old) p->progress_ns = now;
	starved = (ring & BIT(8)) && !residue;
	queued = p->submitted;
	if (neo_audio_healthy(p->aop) && !(ring & BIT(10))) neo_play_queue(p);
	if (starved && p->submitted != queued) {
		starved = false;
		p->progress_ns = now;
	}
	fatal = !neo_audio_healthy(p->aop) || (ring & BIT(10)) ||
		p->hwbytes > p->submitted || (!starved && now - p->progress_ns > 50000000);
	if (fatal || starved) {
		neo_pcm_stop_locked(p);
		if (fatal || READ_ONCE(stream->runtime->state) != SNDRV_PCM_STATE_DRAINING)
			dev_info(p->dev,
				 "Playback stopped fatal=%u ring=%08x residue=%u submitted=%llu completed=%llu hwbytes=%llu appl=%lu polls=%u notify_runs=%u notify_busy=%u\n",
				 fatal, ring, residue, p->submitted, p->completed,
				 p->hwbytes, (unsigned long)READ_ONCE(stream->runtime->control->appl_ptr),
				 p->polls, p->notify_runs, READ_ONCE(p->notify_busy));
		if (fatal) {
			p->faults++;
			p->disabled = true;
			p->cutoff_verified = false;
		}
		/* Normal drain and application underrun stop EN too, but do not
		 * invalidate a successful hardware cutoff test. */
		if (!fatal && READ_ONCE(stream->runtime->state) == SNDRV_PCM_STATE_DRAINING)
			notify = true;
		else fault = true;
	} else {
		if (p->hwbytes / p->period_bytes != p->notified) {
			p->notified = p->hwbytes / p->period_bytes;
			notify = true;
		}
	}
unlock:
	running = p->running;
	p->fault_pending |= fault;
	p->notify_pending |= notify;
	spin_unlock_irqrestore(&p->dma_lock, flags);
	if (fault || notify) mod_delayed_work(system_percpu_wq, &p->notify_work, 0);
	if (!running) return HRTIMER_NORESTART;
	hrtimer_forward_now(timer, ns_to_ktime(2000000));
	return HRTIMER_RESTART;
}

static int neo_play_poweroff(struct neo_speaker *p)
{
	unsigned long flags;
	int error = 0;
	neo_codecs_reset(p);
	spin_lock_irqsave(&p->dma_lock, flags);
	neo_pcm_stop_locked(p);
	spin_unlock_irqrestore(&p->dma_lock, flags);
	hrtimer_cancel(&p->timer);
	if (p->dma)
		dmaengine_synchronize(p->dma);
	cancel_delayed_work_sync(&p->notify_work);
	if (p->powered) error = neo_speaker_off(p);
	return error;
}

static const struct snd_pcm_hardware neo_play_hw = {
	.info = SNDRV_PCM_INFO_INTERLEAVED | SNDRV_PCM_INFO_NO_REWINDS,
	.formats = SNDRV_PCM_FMTBIT_S32_LE,
	.rates = SNDRV_PCM_RATE_48000, .rate_min = 48000, .rate_max = 48000,
	.channels_min = 2, .channels_max = 2,
	.buffer_bytes_max = 32768, .period_bytes_min = 3840, .period_bytes_max = 8192,
	.periods_min = 4, .periods_max = 8,
};

static int neo_play_open(struct snd_pcm_substream *stream)
{
	struct neo_speaker *p = stream->runtime->private_data;
	int error;
	mutex_lock(&p->lock);
	if (!p->cutoff_verified || p->disabled) { error = -EPERM; goto done; }
	stream->runtime->hw = neo_play_hw;
	stream->runtime->hw.channels_min = p->channels;
	stream->runtime->hw.channels_max = p->channels;
	error = snd_pcm_hw_constraint_integer(stream->runtime, SNDRV_PCM_HW_PARAM_PERIODS);
	if (error >= 0) { p->stream = stream; p->opened = true; error = 0; }
done:
	mutex_unlock(&p->lock);
	return error;
}

static int neo_play_free(struct snd_pcm_substream *stream)
{
	struct neo_speaker *p = stream->runtime->private_data;
	int error;
	mutex_lock(&p->lock);
	error = neo_play_poweroff(p);
	mutex_unlock(&p->lock);
	return error;
}

static int neo_play_close(struct snd_pcm_substream *stream)
{
	struct neo_speaker *p = stream->runtime->private_data;
	int error;
	mutex_lock(&p->lock);
	error = neo_play_poweroff(p);
	p->opened = false;
	p->stream = NULL;
	mutex_unlock(&p->lock);
	return error;
}

static int neo_play_prepare(struct snd_pcm_substream *stream)
{
	struct neo_speaker *p = stream->runtime->private_data;
	int error;
	mutex_lock(&p->lock);
	if (!p->cutoff_verified || p->disabled) { error = -EPERM; goto done; }
	error = neo_play_poweroff(p);
	if (error) goto done;
	p->size = snd_pcm_lib_buffer_bytes(stream);
	p->period_bytes = snd_pcm_lib_period_bytes(stream);
	if (!p->size || !p->period_bytes || p->size % p->period_bytes) { error = -EINVAL; goto done; }
	memset(stream->runtime->dma_area, 0, p->size);
	/* Open the aperture and prepare TX while AOP is warm. START will prime
	 * guarded user samples and request pwrd without resetting an active
	 * formatter. EN stays physically low throughout preparation. */
	error = neo_audio_power(p->aop, NEO_SPEAKER_DEVICE, NEO_MIC_WARM, 0);
	neo_amp_off(p);
	if (error) goto done;
	p->powered = true;
	neo_speaker_cutoff(p);
	if (!p->dma) {
		neo_tx_setup(p);
		writel(0, p->regs + CH + 0x20);
	}
	p->submitted = p->completed = p->hwbytes = p->notified = 0;
	p->active = false;
	p->active_bytes = 0;
	memset(p->desc_bytes, 0, sizeof(p->desc_bytes));
	p->notify_pending = p->fault_pending = false;
	p->polls = p->notify_runs = p->notify_busy = 0;
	if (p->dma) {
		error = neo_dma_prepare(p, stream->runtime->dma_addr, p->size, p->period_bytes);
		if (error)
			goto done;
	} else {
		p->rptr = (readl(p->regs + CH + 0x70) >> 4) & 3;
	}
	dev_info(p->dev, "Protected playback prepared backend=%s size=%u period=%u fixed_gain=3/256\n",
		 p->dma ? "dmaengine" : "raw", p->size, p->period_bytes);
done:
	mutex_unlock(&p->lock);
	return error;
}

static int neo_play_trigger(struct snd_pcm_substream *stream, int command)
{
	struct neo_speaker *p = stream->runtime->private_data;
	unsigned long flags;
	int error = 0;
	if (command == SNDRV_PCM_TRIGGER_STOP || command == SNDRV_PCM_TRIGGER_SUSPEND) {
		neo_codecs_reset(p);
		spin_lock_irqsave(&p->dma_lock, flags);
		neo_pcm_stop_locked(p);
		spin_unlock_irqrestore(&p->dma_lock, flags);
		return 0;
	}
	if (command != SNDRV_PCM_TRIGGER_START) return -EINVAL;
	/* This card is explicitly non-atomic: native control RPCs may sleep. */
	mutex_lock(&p->lock);
	spin_lock_irqsave(&p->dma_lock, flags);
	if (!p->powered || p->disabled || !p->cutoff_verified ||
	    p->stream != stream || !neo_audio_healthy(p->aop)) error = -EPERM;
	if (!error) {
		if (p->dma) {
			p->last_appl = READ_ONCE(stream->runtime->control->appl_ptr);
			p->submitted = p->last_appl * p->frame_bytes;
		} else {
			neo_play_queue(p);
		}
		if (!p->submitted)
			error = -EPIPE;
		else if (!p->dma && p->layout->start_before_power)
			writel(1, p->regs);
	}
	spin_unlock_irqrestore(&p->dma_lock, flags);
	if (error) goto abort;
	error = neo_audio_power(p->aop, NEO_SPEAKER_DEVICE, NEO_MIC_ON, 2);
	neo_amp_off(p);
	if (error || READ_ONCE(p->disabled)) goto abort;
	error = neo_speaker_set_mute(p, false);
	neo_amp_off(p);
	if (error || READ_ONCE(p->disabled)) goto abort;
	/* Firmware power RPCs can reset the shared line. Restore all codecs
	 * after the last RPC, before starting DMA or admitting any sound. */
	if (p->codecs_bound) {
		gpiod_set_value(p->enable, 1);
		if (!neo_pin_is_output(p, true)) { error = -EIO; goto abort; }
		error = neo_codecs_restore(p);
		if (error) goto abort;
	}
	spin_lock_irqsave(&p->dma_lock, flags);
	if (p->disabled || !neo_audio_healthy(p->aop)) error = -EPERM;
	if (!error) {
		if (!p->dma)
			writel(0, p->regs + CH + 0x20);
		gpiod_set_value(p->enable, 1);
		if (!neo_pin_is_output(p, true)) error = -EIO;
	}
	if (!error) {
		/* MCA may power up before TX, avoiding consumption during the RPC. */
		p->running = true;
		p->progress_ns = ktime_get_ns();
		if (p->dma)
			dma_async_issue_pending(p->dma);
		else if (!p->layout->start_before_power)
			writel(1, p->regs);
		hrtimer_start(&p->timer, ns_to_ktime(2000000), HRTIMER_MODE_REL);
	}
	spin_unlock_irqrestore(&p->dma_lock, flags);
	if (!error) { mutex_unlock(&p->lock); return 0; }
abort:
	neo_codecs_reset(p);
	if (!error) error = -EPERM;
	spin_lock_irqsave(&p->dma_lock, flags);
	p->disabled = true;
	p->cutoff_verified = false;
	neo_pcm_stop_locked(p);
	spin_unlock_irqrestore(&p->dma_lock, flags);
	mutex_unlock(&p->lock);
	return error;
}

static int neo_play_sync_stop(struct snd_pcm_substream *stream)
{
	struct neo_speaker *p = stream->runtime->private_data;
	hrtimer_cancel(&p->timer);
	if (p->dma)
		dmaengine_synchronize(p->dma);
	cancel_delayed_work_sync(&p->notify_work);
	return 0;
}

static snd_pcm_uframes_t neo_play_pointer(struct snd_pcm_substream *stream)
{
	struct neo_speaker *p = stream->runtime->private_data;
	return p->size ? (READ_ONCE(p->hwbytes) % p->size) / p->frame_bytes : 0;
}

static int neo_play_copy(struct snd_pcm_substream *stream, int channel,
		unsigned long pos, struct iov_iter *iter, unsigned long bytes)
{
	struct neo_speaker *p = stream->runtime->private_data;
	s32 temp[256];
	unsigned long flags, done = 0;
	if (pos % p->frame_bytes || bytes % p->frame_bytes ||
	    pos > p->size || bytes > p->size - pos)
		return -EINVAL;
	while (done < bytes) {
		u64 energy[NEO_SPEAKER_MAX_CHANNELS] = {0}, now;
		unsigned int n = min_t(unsigned long, sizeof(temp), bytes - done), i, a;
		u64 gain;
		if (copy_from_iter(temp, n, iter) != n) return -EFAULT;
		for (i = 0; i < n / 4; i++) {
			s64 q;
			q = temp[i];
			temp[i] = neo_attenuate_s32(temp[i]);
			q = ((q < 0 ? -q : q) + 255) / 256; /* raw S24 energy; immutable gain is in model ceiling */
			energy[i % p->channels] += q * q;
		}
		spin_lock_irqsave(&p->dma_lock, flags);
		if (p->disabled) { spin_unlock_irqrestore(&p->dma_lock, flags); return -EPERM; }
		now = ktime_get_ns();
		a = 0;
		for (i = 0; i < p->channels; i++) {
			thermal_advance(&p->thermal[i], now);
			/* Overestimate all committed/held samples at the immutable peak. */
			a = max(a, thermal_admit(&p->thermal[i], energy[i], 1U << 23, 1U << 23,
				(u64)(p->size / p->frame_bytes) * (1U << 23) * (1U << 23)));
		}
		gain = p->thermal[0].att[a];
		for (i = 0; i < n / 4; i++) {
			s64 v = temp[i], magnitude = v < 0 ? -v : v;
			u32 sample;

			magnitude = mul_u64_u64_shr(magnitude, gain, 32);
			v = v < 0 ? -magnitude : magnitude;
			sample = p->layout->float_dma ? neo_q31_float_bits(v) :
				(s32)(v / (1 << (32 - 24)));
			((__le32 *)stream->runtime->dma_area)[(pos + done) / 4 + i] =
				cpu_to_le32(sample);
			if (v) p->nonzero_samples++;
		}
		/* Charging the pre-thermal-limit energy is a conservative overestimate. */
		for (i = 0; i < p->channels; i++)
			thermal_charge(&p->thermal[i], energy[i]);
		dma_wmb();
		spin_unlock_irqrestore(&p->dma_lock, flags);
		done += n;
	}
	return 0;
}

static int neo_play_silence(struct snd_pcm_substream *stream, int channel,
		unsigned long pos, unsigned long bytes)
{
	struct neo_speaker *p = stream->runtime->private_data;
	if (pos % p->frame_bytes || bytes % p->frame_bytes ||
	    pos > p->size || bytes > p->size - pos)
		return -EINVAL;
	memset(stream->runtime->dma_area + pos, 0, bytes);
	dma_wmb();
	return 0;
}
static int neo_raw_open(struct snd_pcm_substream *stream)
{
	stream->runtime->private_data = snd_pcm_substream_chip(stream);
	return neo_play_open(stream);
}

static const struct snd_pcm_ops neo_play_ops = {
	.open = neo_raw_open, .close = neo_play_close, .hw_free = neo_play_free,
	.ioctl = snd_pcm_lib_ioctl, .prepare = neo_play_prepare,
	.trigger = neo_play_trigger, .sync_stop = neo_play_sync_stop, .pointer = neo_play_pointer,
	.copy = neo_play_copy, .fill_silence = neo_play_silence,
};


#if IS_ENABLED(CONFIG_SND_SOC_APPLE_AOP_SPEAKER_CODECS)
static int neo_soc_open(struct snd_soc_component *component,
			struct snd_pcm_substream *stream)
{
	stream->runtime->private_data = snd_soc_component_get_drvdata(component);
	return neo_play_open(stream);
}

#define NEO_SOC_PCM_WRAPPER(name, fn) \
static int name(struct snd_soc_component *component, \
		struct snd_pcm_substream *stream) \
{ return fn(stream); }
NEO_SOC_PCM_WRAPPER(neo_soc_close, neo_play_close)
NEO_SOC_PCM_WRAPPER(neo_soc_free, neo_play_free)
NEO_SOC_PCM_WRAPPER(neo_soc_prepare, neo_play_prepare)
NEO_SOC_PCM_WRAPPER(neo_soc_sync_stop, neo_play_sync_stop)

static int neo_soc_trigger(struct snd_soc_component *component,
			   struct snd_pcm_substream *stream, int command)
{
	return neo_play_trigger(stream, command);
}

static snd_pcm_uframes_t neo_soc_pointer(struct snd_soc_component *component,
				       struct snd_pcm_substream *stream)
{
	return neo_play_pointer(stream);
}

static int neo_soc_copy(struct snd_soc_component *component,
			struct snd_pcm_substream *stream, int channel,
			unsigned long pos, struct iov_iter *iter, unsigned long bytes)
{
	return neo_play_copy(stream, channel, pos, iter, bytes);
}

static int neo_soc_pcm_new(struct snd_soc_component *component,
			   struct snd_soc_pcm_runtime *rtd)
{
	struct neo_speaker *p = snd_soc_component_get_drvdata(component);

	return snd_pcm_set_managed_buffer_all(rtd->pcm, SNDRV_DMA_TYPE_DEV,
					     p->dma_dev, 32768, 32768);
}

static int neo_soc_fixup_controls(struct snd_soc_component *component);

static const struct snd_soc_component_driver neo_soc_component = {
	.name = "apple-aop-protected-pcm",
	.open = neo_soc_open, .close = neo_soc_close, .hw_free = neo_soc_free,
	.prepare = neo_soc_prepare, .trigger = neo_soc_trigger,
	.sync_stop = neo_soc_sync_stop, .pointer = neo_soc_pointer,
	.copy = neo_soc_copy, .pcm_new = neo_soc_pcm_new,
	.fixup_controls = neo_soc_fixup_controls,
};

static struct snd_soc_dai_driver neo_soc_dai = {
	.name = "aop-speaker-tx",
	.playback = {
		.stream_name = "Speaker Playback",
		.channels_min = 2, .channels_max = NEO_SPEAKER_MAX_CHANNELS,
		.rates = SNDRV_PCM_RATE_48000, .formats = SNDRV_PCM_FMTBIT_S32_LE,
	},
};

static int neo_soc_link_init(struct snd_soc_pcm_runtime *rtd)
{
	struct neo_speaker *p = snd_soc_card_get_drvdata(rtd->card);
	unsigned int i;
	int ret;

	for (i = 0; i < p->num_codecs; i++) {
		struct snd_soc_dai *dai = snd_soc_rtd_to_codec(rtd, i);

		p->codecs[i] = dai->component;
		ret = snd_soc_dai_set_tdm_slot(dai, BIT(i), 0, p->channels, 32);
		if (ret)
			return ret;
	}
	return 0;
}

static int neo_soc_fixup_controls(struct snd_soc_component *component)
{
	struct snd_soc_card *card = component->card;
	/* The existing immutable gain and thermal model require fixed codec
	 * gain/routing. Lock mixer controls before exposing the card. */
	static const char * const controls[] = {
		"Speaker Volume", "Amp Gain Volume", "ASI1 Sel",
		"HPF Corner Frequency", "OCE Handling", "ISENSE Switch", "VSENSE Switch",
	};
	struct neo_speaker *p = snd_soc_card_get_drvdata(card);
	struct snd_ctl_elem_value *selection;
	unsigned int i, j;
	int ret;

	selection = devm_kzalloc(p->dev, sizeof(*selection), GFP_KERNEL);
	if (!selection)
		return -ENOMEM;
	/* TAS2764 ASI1 enum: I2C offset, Left, Right, LeftRightDiv2. */
	selection->value.enumerated.item[0] = 1;
	for (i = 0; i < p->num_codecs; i++) {
		struct snd_soc_component *codec = p->codecs[i];
		struct snd_kcontrol *source;
		char name[80];

		ret = snd_soc_component_write(codec, TAS2764_CHNL_0, 15 << 1);
		if (ret < 0)
			return ret;
		ret = snd_soc_component_write(codec, TAS2764_DVC, 0);
		if (ret < 0)
			return ret;
		ret = snd_soc_component_update_bits(codec, TAS2764_TDM_CFG0,
						    TAS2764_TDM_CFG0_CLASSD_SYNC,
						    TAS2764_TDM_CFG0_CLASSD_SYNC);
		if (ret < 0)
			return ret;
		snprintf(name, sizeof(name), "Amp%u ASI1 Sel", i);
		source = snd_soc_card_get_kcontrol(card, name);
		if (!source || !source->put) {
			struct snd_kcontrol *k;
			dev_err(p->dev, "Missing codec source control %s\n", name);
			list_for_each_entry(k, &card->snd_card->controls, list)
				dev_info(p->dev, "Codec control: %s\n", k->id.name);
			return -ENOENT;
		}
		ret = source->put(source, selection);
		if (ret < 0)
			return ret;
		for (j = 0; j < ARRAY_SIZE(controls); j++) {
			struct snd_kcontrol *k;

			snprintf(name, sizeof(name), "Amp%u %s", i, controls[j]);
			k = snd_soc_card_get_kcontrol(card, name);
			if (!k) {
				dev_err(p->dev, "Missing fixed codec control %s\n", name);
				return -ENOENT;
			}
			/* DAPM retains pointers to its controls. Preserve them with
			 * read-only access, as the existing T8132 machine does. */
			k->vd[0].access &= ~SNDRV_CTL_ELEM_ACCESS_WRITE;
		}
	}
	p->codecs_bound = true;
	neo_codecs_reset(p);
	neo_amp_off(p);
	dev_info(p->dev, "Bound %u codecs; common shutdown and regcache coordinated\n",
		 p->num_codecs);
	return 0;
}

static int neo_soc_remove(struct snd_soc_card *card)
{
	struct neo_speaker *p = snd_soc_card_get_drvdata(card);

	mutex_lock(&p->lock);
	neo_play_poweroff(p);
	p->codecs_bound = false;
	mutex_unlock(&p->lock);
	return 0;
}

static void neo_soc_put_nodes(void *cookie)
{
	struct neo_speaker *p = cookie;
	unsigned int i;

	for (i = 0; i < p->num_codecs; i++)
		of_node_put(p->soc_codecs[i].of_node);
}

static int neo_soc_register(struct neo_speaker *p)
{
	struct device_node *node = p->dev->of_node;
	unsigned int i;
	int ret;

	ret = of_count_phandle_with_args(node, "audio-codecs", NULL);
	if (ret != p->channels)
		return -EINVAL;
	p->num_codecs = ret;
	for (i = 0; i < p->num_codecs; i++) {
		p->soc_codecs[i].of_node = of_parse_phandle(node, "audio-codecs", i);
		p->soc_codecs[i].dai_name = "tas2764 ASI1";
		p->codec_conf[i].dlc.of_node = p->soc_codecs[i].of_node;
		p->codec_conf[i].name_prefix = devm_kasprintf(p->dev, GFP_KERNEL, "Amp%u", i);
		if (!p->codec_conf[i].name_prefix) {
			neo_soc_put_nodes(p);
			return -ENOMEM;
		}
	}
	ret = devm_add_action_or_reset(p->dev, neo_soc_put_nodes, p);
	if (ret)
		return ret;
	p->soc_cpu.of_node = node;
	p->soc_cpu.dai_name = "aop-speaker-tx";
	p->soc_platform.of_node = node;
	p->soc_link = (struct snd_soc_dai_link) {
		.name = "AOP Speakers", .stream_name = "Speaker Playback",
		.cpus = &p->soc_cpu, .num_cpus = 1,
		.platforms = &p->soc_platform, .num_platforms = 1,
		.codecs = p->soc_codecs, .num_codecs = p->num_codecs,
		.dai_fmt = SND_SOC_DAIFMT_I2S | SND_SOC_DAIFMT_NB_IF | SND_SOC_DAIFMT_CBC_CFC,
		.init = neo_soc_link_init, .nonatomic = 1, .ignore_pmdown_time = 1,
	};
	p->soc_card = (struct snd_soc_card) {
		.owner = THIS_MODULE, .dev = p->dev,
		.name = "AOP protected speakers", .driver_name = "AOPSpkr",
		.dai_link = &p->soc_link, .num_links = 1,
		.codec_conf = p->codec_conf, .num_configs = p->num_codecs,
		.remove = neo_soc_remove,
	};
	snd_soc_card_set_drvdata(&p->soc_card, p);
	ret = devm_snd_soc_register_component(p->dev, &neo_soc_component, &neo_soc_dai, 1);
	if (ret)
		return ret;
	/* Codec probe resets/programs each chip while its DAC is shut down.
	 * Register devres cutoff before card registration: it runs after the
	 * card is removed but before DMA/GPIO resources are released. */
	gpiod_set_value(p->enable, 1);
	ret = devm_snd_soc_register_card(p->dev, &p->soc_card);
	if (ret)
		neo_amp_off(p);
	return ret;
}
#else
static int neo_soc_register(struct neo_speaker *p)
{
	return -EOPNOTSUPP;
}
#endif

static ssize_t force_disable_store(struct device *dev, struct device_attribute *attr,
		const char *buf, size_t size)
{
	struct neo_speaker *p = dev_get_drvdata(dev);
	struct snd_pcm_substream *stream;
	unsigned long flags;
	bool disable;
	int error;
	if (kstrtobool(buf, &disable) || !disable) return -EINVAL;
	/* Physical cutoff must not wait behind an in-flight AOP RPC. */
	spin_lock_irqsave(&p->dma_lock, flags);
	p->disabled = true;
	p->cutoff_verified = false;
	neo_pcm_stop_locked(p);
	stream = p->stream;
	spin_unlock_irqrestore(&p->dma_lock, flags);
	hrtimer_cancel(&p->timer);
	cancel_delayed_work_sync(&p->notify_work);
	if (stream) snd_pcm_stop_xrun(stream);
	mutex_lock(&p->lock);
	error = p->powered ? neo_speaker_off(p) : 0;
	if (!neo_pin_is_output(p, false)) error = -EIO;
	mutex_unlock(&p->lock);
	return error ? error : size;
}
static DEVICE_ATTR_WO(force_disable);

/* Test the independent stall detector only while all samples this boot were
 * zero. Physical audio tests use force_disable; this cannot stall live sound. */
static ssize_t fault_inject_stall_store(struct device *dev, struct device_attribute *attr,
		const char *buf, size_t size)
{
	struct neo_speaker *p = dev_get_drvdata(dev);
	unsigned long flags;
	bool inject;
	int error = 0;
	if (kstrtobool(buf, &inject) || !inject) return -EINVAL;
	spin_lock_irqsave(&p->dma_lock, flags);
	if (!p->running || p->nonzero_samples) error = -EPERM;
	else if (p->dma) error = dmaengine_pause(p->dma);
	else { writel(1, p->regs + 4); readl(p->regs); }
	spin_unlock_irqrestore(&p->dma_lock, flags);
	return error ? error : size;
}
static DEVICE_ATTR_WO(fault_inject_stall);

static ssize_t amplifier_state_show(struct device *dev,
				    struct device_attribute *attr, char *buf)
{
#if IS_ENABLED(CONFIG_SND_SOC_APPLE_AOP_SPEAKER_CODECS)
	struct neo_speaker *p = dev_get_drvdata(dev);
	unsigned int i;
	ssize_t count = 0;
	int ret = 0;

	mutex_lock(&p->lock);
	if (!p->codecs_bound || !p->running || p->disabled ||
	    !neo_pin_is_output(p, true)) {
		ret = -EAGAIN;
		goto done;
	}
	for (i = 0; i < p->num_codecs; i++) {
		struct tas2764_live_state state;

		ret = tas2764_read_live_state(p->codecs[i], &state);
		if (ret)
			goto done;
		count += sysfs_emit_at(buf, count,
			"amp=%u power=%02x gain=%02x dvc=%02x tdm_rate=%02x tdm_start=%02x tdm_word=%02x tdm_slots=%02x live_faults=%02x\n",
			i, state.power, state.analog_gain, state.attenuation,
			state.tdm_rate, state.tdm_start, state.tdm_word,
			state.tdm_slots, state.live_faults);
	}
done:
	mutex_unlock(&p->lock);
	return ret ? ret : count;
#else
	return -EOPNOTSUPP;
#endif
}
static DEVICE_ATTR_RO(amplifier_state);

static ssize_t safety_status_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	struct neo_speaker *p = dev_get_drvdata(dev);
	unsigned long flags;
	ssize_t size;
	unsigned int i;

	spin_lock_irqsave(&p->dma_lock, flags);
	size = sysfs_emit(buf, "cutoff_verified=%u disabled=%u gpio=%08x running=%u "
		"playback_exposed=%u backend=%s periods=%u channels=%u dma_format=%s fixed_gain=3/256 peak_q31=%u "
		"nonzero_samples=%llu faults=%llu coil_uc=",
		p->cutoff_verified, p->disabled, neo_amp_level(p), p->running,
		!zero_only, p->dma ? "dmaengine" : "raw", p->dma_periods,
		p->channels, p->layout->float_dma ? "float32" : "s24-le",
		NEO_SPEAKER_PEAK_Q31, p->nonzero_samples, p->faults);
	for (i = 0; i < p->channels; i++)
		size += sysfs_emit_at(buf, size, "%s%lld", i ? "," : "", p->thermal[i].coil);
	size += sysfs_emit_at(buf, size, " magnet_uc=");
	for (i = 0; i < p->channels; i++)
		size += sysfs_emit_at(buf, size, "%s%lld", i ? "," : "", p->thermal[i].magnet);
	size += sysfs_emit_at(buf, size, "\n");
	spin_unlock_irqrestore(&p->dma_lock, flags);
	return size;
}
static DEVICE_ATTR_RO(safety_status);

static struct attribute *neo_speaker_attrs[] = {
	&dev_attr_cutoff_test.attr, &dev_attr_safety_status.attr, &dev_attr_amplifier_state.attr, &dev_attr_force_disable.attr, &dev_attr_fault_inject_stall.attr, NULL,
};
ATTRIBUTE_GROUPS(neo_speaker);

static int neo_panic_cutoff(struct notifier_block *nb, unsigned long event, void *unused)
{
	struct neo_speaker *p = container_of(nb, struct neo_speaker, panic);
	/* The GPIO provider is atomic; avoid transport and the gated DMA aperture. */
	gpiod_set_value(p->enable, 0);
	return NOTIFY_DONE;
}

static void neo_cleanup(void *cookie)
{
	struct neo_speaker *p = cookie;
	p->disabled = true;
	p->cutoff_verified = false;
	neo_play_poweroff(p);
	atomic_notifier_chain_unregister(&panic_notifier_list, &p->panic);
}
static void neo_free_silence(void *cookie)
{
	struct neo_speaker *p = cookie;
	dma_free_coherent(p->dma_dev, PERIOD, p->silence, p->iova);
}
static void neo_release_dma(void *cookie)
{
	struct neo_speaker *p = cookie;

	dma_release_channel(p->dma);
}
static int neo_speaker_probe(struct platform_device *pdev)
{
	struct neo_speaker *p;
	struct snd_card *card;
	struct snd_pcm *pcm;
	const struct thermal_params params = {
		.fs_mv = 12000, .r_mohm = 1000, .margin_pct = 200, .ceiling = 77, /* -38.5 dB bounds immutable -38.622 dB gain */
		.r_coil = 76000, .r_magnet = 52000, .tau_coil_ms = 3300, .tau_magnet_ms = 100000,
		.t_ambient = 65000, .t_limit = 100000, .t_window = 20000, .t_hysteresis = 5000,
		.t_hard = 130000, .t_stall = 130000, .delivery_ns = 160000000, .stall_frames = 9600,
	};
	int error, i;
	p = devm_kzalloc(&pdev->dev, sizeof(*p), GFP_KERNEL);
	if (!p) return -ENOMEM;
	p->dev = &pdev->dev;
	p->layout = of_device_get_match_data(p->dev);
	if (!p->layout)
		return -EINVAL;
	/* Board topology, not SoC generation or a serial-number gate. */
	p->channels = 2;
	if (of_find_property(p->dev->of_node, "num-channels", NULL)) {
		error = of_property_read_u32(p->dev->of_node, "num-channels", &p->channels);
		if (error)
			return error;
	}
	if (p->channels != 2 && p->channels != 4)
		return -EINVAL;
	p->frame_bytes = p->channels * sizeof(u32);
	if (of_find_property(p->dev->of_node, "apple,tx-sram-base", NULL)) {
		error = of_property_read_u32(p->dev->of_node, "apple,tx-sram-base",
				     &p->tx_sram_base);
		if (error)
			return error;
	}
	if (p->tx_sram_base > U16_MAX - TX_SRAM_BYTES)
		return -EINVAL;
	mutex_init(&p->lock);
	spin_lock_init(&p->dma_lock);
	INIT_DELAYED_WORK(&p->notify_work, neo_play_notify);
	hrtimer_setup(&p->timer, neo_play_poll, CLOCK_MONOTONIC, HRTIMER_MODE_REL);
	for (i = 0; i < p->channels; i++) {
		error = thermal_init(&p->thermal[i], &params, ktime_get_ns());
		if (error) return error;
		/* Assume a very hot reboot; admission retains this estimated heat. */
		p->thermal[i].coil = 129000000;
		p->thermal[i].magnet = 99000000;
		p->thermal[i].coil_hyst = p->thermal[i].coil;
		p->thermal[i].magnet_hyst = p->thermal[i].magnet;
	}
	p->enable = devm_gpiod_get(p->dev, "enable", GPIOD_OUT_LOW);
	if (IS_ERR(p->enable)) return dev_err_probe(p->dev, PTR_ERR(p->enable), "Speaker EN unavailable\n");
	if (gpiod_cansleep(p->enable)) return -EINVAL;
	if (!neo_pin_is_output(p, false)) return dev_err_probe(p->dev, -EIO, "Speaker EN is not low\n");
	p->aop = neo_mic_get(p->dev);
	if (IS_ERR(p->aop)) return PTR_ERR(p->aop);
	p->dma_dev = p->dev;
	if (of_find_property(p->dev->of_node, "dmas", NULL)) {
		p->dma = dma_request_chan(p->dev, "tx");
		if (IS_ERR(p->dma))
			return dev_err_probe(p->dev, PTR_ERR(p->dma), "Speaker DMA channel unavailable\n");
		p->dma_dev = dmaengine_get_dma_device(p->dma);
		error = devm_add_action_or_reset(p->dev, neo_release_dma, p);
		if (error)
			return error;
	} else {
		p->regs = devm_platform_ioremap_resource(pdev, 0);
		if (IS_ERR(p->regs))
			return PTR_ERR(p->regs);
		error = dma_set_mask_and_coherent(p->dev, DMA_BIT_MASK(42));
		if (error)
			return error;
	}
	p->silence = dma_alloc_coherent(p->dma_dev, PERIOD, &p->iova, GFP_KERNEL);
	if (!p->silence) return -ENOMEM;
	memset(p->silence, 0, PERIOD);
	error = devm_add_action_or_reset(p->dev, neo_free_silence, p);
	if (error) return error;
	platform_set_drvdata(pdev, p);
	error = devm_device_add_group(p->dev, neo_speaker_groups[0]);
	if (error) return error;
	dev_info(p->dev, "Protected speaker probe GPIO=%08x; playback locked until zero-PCM cutoff test\n",
		 neo_amp_level(p));
	p->panic.notifier_call = neo_panic_cutoff;
	p->panic.priority = INT_MAX;
	error = atomic_notifier_chain_register(&panic_notifier_list, &p->panic);
	if (error) return error;
	error = devm_add_action_or_reset(p->dev, neo_cleanup, p);
	if (error) return error;
	error = cutoff_test_store(p->dev, NULL, "1", 1);
	if (error < 0) return error;
	if (zero_only)
		return 0;
	if (of_find_property(p->dev->of_node, "audio-codecs", NULL))
		return neo_soc_register(p);
	error = snd_devm_card_new(p->dev, -1, "NeoSpkr", THIS_MODULE, 0, &card);
	if (error) return error;
	error = snd_pcm_new(card, "Neo protected speakers", 0, 1, 0, &pcm);
	if (error) return error;
	pcm->private_data = p;
	pcm->nonatomic = true;
	strscpy(pcm->name, "Neo fixed -38.6 dB speakers", sizeof(pcm->name));
	snd_pcm_set_ops(pcm, SNDRV_PCM_STREAM_PLAYBACK, &neo_play_ops);
	error = snd_pcm_set_managed_buffer_all(pcm, SNDRV_DMA_TYPE_DEV, p->dma_dev, 32768, 32768);
	if (error) return error;
	strscpy(card->driver, "NeoSpkr", sizeof(card->driver));
	strscpy(card->shortname, "Neo protected speakers", sizeof(card->shortname));
	strscpy(card->longname, "Neo fixed -38.6 dB protected NS TX0 speakers", sizeof(card->longname));
	return snd_card_register(card);
}
static void neo_speaker_shutdown(struct platform_device *pdev)
{
	struct neo_speaker *p = platform_get_drvdata(pdev);
	unsigned long flags;
	mutex_lock(&p->lock);
	spin_lock_irqsave(&p->dma_lock, flags);
	p->disabled = true;
	p->cutoff_verified = false;
	neo_pcm_stop_locked(p);
	spin_unlock_irqrestore(&p->dma_lock, flags);
	neo_play_poweroff(p);
	mutex_unlock(&p->lock);
}
static int neo_speaker_suspend(struct device *dev)
{
	neo_speaker_shutdown(to_platform_device(dev));
	return 0;
}
static DEFINE_SIMPLE_DEV_PM_OPS(neo_speaker_pm, neo_speaker_suspend, NULL);

static const struct of_device_id neo_speaker_of_match[] = {
	{ .compatible = "apple,t8142-speaker", .data = &neo_mca_layout },
	{ .compatible = "apple,t8140-neo-speaker", .data = &neo_leap_layout }, {}
};
MODULE_DEVICE_TABLE(of, neo_speaker_of_match);
static struct platform_driver neo_speaker_driver = {
	.probe = neo_speaker_probe,
	.shutdown = neo_speaker_shutdown,
	.driver = { .name = "neo-speaker", .of_match_table = neo_speaker_of_match, .pm = pm_sleep_ptr(&neo_speaker_pm) },
};
module_platform_driver(neo_speaker_driver);
MODULE_LICENSE("GPL");
