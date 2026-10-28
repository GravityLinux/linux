// SPDX-License-Identifier: GPL-2.0-only
/* Private Neo protected playback. Immutable 3/256 gain, float32 DMA,
 * two-node thermal admission, and amplifier-first fail-closed cutoff.
 *
 * TODO: This quiet fixed volume cap is temporary. Replace it with calibrated
 * waveform-dependent mechanical/thermal limits to support higher playback
 * levels, while retaining an absolute peak ceiling and amplifier cutoff. */
#include <linux/delay.h>
#include <linux/dma-mapping.h>
#include <linux/gpio/consumer.h>
#include <linux/io.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/platform_device.h>
#include <linux/panic_notifier.h>
#include <linux/of.h>
#include <linux/hrtimer.h>
#include <linux/uio.h>
#include <linux/workqueue.h>
#include <sound/core.h>
#include <sound/pcm.h>
#include "neo_mic.h"
#include "pcm_guard.h"
#include "neo_thermal.h"

#define CH 0x8000
#define PERIOD 126720 /* native block size; zeros keep the active-stop test silent */
struct neo_speaker {
	struct device *dev;
	struct neo_mic *aop;
	struct gpio_desc *enable;
	void __iomem *regs, *pin;
	struct mutex lock;
	void *silence;
	dma_addr_t iova;
	bool powered, cutoff_verified;
	u32 descriptor;
	spinlock_t dma_lock;
	struct hrtimer timer;
	struct delayed_work notify_work;
	bool notify_pending, fault_pending;
	struct notifier_block panic;
	struct snd_pcm_substream *stream;
	struct thermal thermal[2];
	bool opened, running, active, disabled;
	u32 rptr, period_bytes, size, active_bytes, desc_bytes[4];
	u64 submitted, completed, hwbytes, notified, progress_ns;
	u64 nonzero_samples, faults;
};

static u32 neo_pin(struct neo_speaker *p)
{
	return readl(p->pin);
}

static bool neo_pin_is_output(struct neo_speaker *p, bool enabled)
{
	u32 pin = neo_pin(p);
	return (pin & 0x7f) == (2 | enabled);
}

static void neo_amp_off(struct neo_speaker *p)
{
	gpiod_set_value(p->enable, 0);
	/* The GPIO provider caches output registers. Force the physical clear
	 * even if its cache already says low after a firmware transition. */
	writel(readl(p->pin) & ~BIT(0), p->pin);
	readl(p->pin);
}

static void neo_speaker_cutoff(struct neo_speaker *p)
{
	neo_amp_off(p);
	/* GPIO cutoff precedes all transport calls and DMA manipulation. */
	if (!p->powered)
		return;
	writel(0, p->regs + CH + 0x20);
	writel(1, p->regs + 4);
	writel(0, p->regs + CH + 0xa0);
	readl(p->regs + 0); /* flush posted disable */
}

static int neo_speaker_mute(struct neo_speaker *p)
{
	__le32 muted = cpu_to_le32(1);
	return neo_audio_set(p->aop, NEO_SPEAKER_DEVICE, 700, &muted, sizeof(muted));
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
	writel(2, p->regs + CH + 0x40);
	writel(0x1800000, p->regs + CH + 0x50);
	writel(0x18000c0, p->regs + CH + 0x54);
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
	neo_tx_setup(p);
	for (i = 0; i < 4; i++) neo_queue_zero(p);
	/* Native startup primes/enables TX before requesting pwrd. Keep the
	 * amplifier off; every descriptor points at the immutable zero buffer. */
	writel(1, p->regs);
	error = neo_audio_power(p->aop, NEO_SPEAKER_DEVICE, NEO_MIC_ON, 2);
	neo_amp_off(p);
	if (error) return error;
	{
		__le32 unmuted = 0;
		error = neo_audio_set(p->aop, NEO_SPEAKER_DEVICE, 700, &unmuted, sizeof(unmuted));
		neo_amp_off(p);
		if (error) return error;
	}
	return 0;
}

static ssize_t cutoff_test_store(struct device *dev, struct device_attribute *attr,
				const char *buf, size_t size)
{
	struct neo_speaker *p = dev_get_drvdata(dev);
	u32 before, after, control, initial, residue, stopped, stable, desc;
	int error = 0, off_error;
	bool run;
	if (kstrtobool(buf, &run) || !run) return -EINVAL;
	mutex_lock(&p->lock);
	if (p->opened) { mutex_unlock(&p->lock); return -EBUSY; }
	p->cutoff_verified = false;
	error = neo_zero_prepare(p);
	if (error) goto off;
	/* The sole buffer is allocated zeroed and never written with user data. */
	gpiod_set_value(p->enable, 1);
	before = neo_pin(p);
	if (!neo_pin_is_output(p, true)) { error = -EIO; goto off; }
	writel(0, p->regs + CH + 0x20); /* poll-only test; no IRQ enabled */
	writel(3, p->regs + CH + 0xa0);
	writel(1, p->regs + 0);
	initial = readl(p->regs + CH + 0x64);
	dev_info(dev, "Zero-PCM TX start ctl=%08x en=%08x desc=%08x report=%08x residue=%08x\n",
		 readl(p->regs + CH), readl(p->regs), readl(p->regs + CH + 0x70),
		 readl(p->regs + CH + 0x74), readl(p->regs + CH + 0x64));
	msleep(5);
	residue = readl(p->regs + CH + 0x64);
	desc = readl(p->regs + CH + 0x70);
	if (!residue || residue >= initial || desc & BIT(10)) error = -EIO;
	neo_speaker_cutoff(p);
	after = neo_pin(p);
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
	u64 sent = (p->submitted / 8) % rt->boundary;
	u64 ready = (appl + rt->boundary - sent) % rt->boundary;
	unsigned int count = 0;
	/* Never let a forged appl_ptr or a fast-draining ring spin unbounded. */
	ready = min_t(u64, ready, p->size / 8);
	while (count++ < 4 && ready &&
	       (ready >= p->period_bytes / 8 || READ_ONCE(rt->state) == SNDRV_PCM_STATE_DRAINING) &&
	       !(readl(p->regs + CH + 0x70) & BIT(9))) {
		u32 bytes = min_t(u64, ready * 8, p->period_bytes);
		u32 slot = readl(p->regs + CH + 0x70) & 3;
		dma_addr_t addr = rt->dma_addr + p->submitted % p->size;
		dma_wmb();
		writel(lower_32_bits(addr), p->regs + 0x10000);
		writel(upper_32_bits(addr), p->regs + 0x10000);
		writel(bytes, p->regs + 0x10000);
		p->desc_bytes[slot] = bytes;
		writel(0xc0000 | (++p->descriptor & 0xffff), p->regs + 0x10000);
		p->submitted += bytes;
		ready -= bytes / 8;
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
		mod_delayed_work(system_percpu_wq, &p->notify_work, msecs_to_jiffies(1));
		return;
	}
	spin_lock_irqsave(&p->dma_lock, flags);
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

static enum hrtimer_restart neo_play_poll(struct hrtimer *timer)
{
	struct neo_speaker *p = container_of(timer, struct neo_speaker, timer);
	struct snd_pcm_substream *stream;
	unsigned long flags;
	u32 ring, residue, rptr, delta, slot;
	u64 queued;
	u64 now = ktime_get_ns(), old;
	bool fault = false, notify = false, running, starved, fatal;
	spin_lock_irqsave(&p->dma_lock, flags);
	stream = p->stream;
	if (!p->running || !p->powered || !stream) goto unlock;
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
	spin_lock_irqsave(&p->dma_lock, flags);
	neo_pcm_stop_locked(p);
	spin_unlock_irqrestore(&p->dma_lock, flags);
	hrtimer_cancel(&p->timer);
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
	struct neo_speaker *p = snd_pcm_substream_chip(stream);
	int error;
	mutex_lock(&p->lock);
	if (!p->cutoff_verified || p->disabled) { error = -EPERM; goto done; }
	stream->runtime->hw = neo_play_hw;
	error = snd_pcm_hw_constraint_integer(stream->runtime, SNDRV_PCM_HW_PARAM_PERIODS);
	if (error >= 0) { p->stream = stream; p->opened = true; error = 0; }
done:
	mutex_unlock(&p->lock);
	return error;
}

static int neo_play_free(struct snd_pcm_substream *stream)
{
	struct neo_speaker *p = snd_pcm_substream_chip(stream);
	int error;
	mutex_lock(&p->lock);
	error = neo_play_poweroff(p);
	mutex_unlock(&p->lock);
	return error;
}

static int neo_play_close(struct snd_pcm_substream *stream)
{
	struct neo_speaker *p = snd_pcm_substream_chip(stream);
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
	struct neo_speaker *p = snd_pcm_substream_chip(stream);
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
	neo_tx_setup(p);
	writel(0, p->regs + CH + 0x20);
	p->submitted = p->completed = p->hwbytes = p->notified = 0;
	p->active = false;
	p->active_bytes = 0;
	memset(p->desc_bytes, 0, sizeof(p->desc_bytes));
	p->notify_pending = p->fault_pending = false;
	p->rptr = (readl(p->regs + CH + 0x70) >> 4) & 3;
	dev_info(p->dev, "Protected playback prepared size=%u period=%u fixed_gain=3/256\n", p->size, p->period_bytes);
done:
	mutex_unlock(&p->lock);
	return error;
}

static int neo_play_trigger(struct snd_pcm_substream *stream, int command)
{
	struct neo_speaker *p = snd_pcm_substream_chip(stream);
	unsigned long flags;
	__le32 unmuted = 0;
	int error = 0;
	if (command == SNDRV_PCM_TRIGGER_STOP || command == SNDRV_PCM_TRIGGER_SUSPEND) {
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
		neo_play_queue(p);
		if (!p->submitted) error = -EPIPE;
		else writel(1, p->regs);
	}
	spin_unlock_irqrestore(&p->dma_lock, flags);
	if (error) goto abort;
	error = neo_audio_power(p->aop, NEO_SPEAKER_DEVICE, NEO_MIC_ON, 2);
	neo_amp_off(p);
	if (error || READ_ONCE(p->disabled)) goto abort;
	error = neo_audio_set(p->aop, NEO_SPEAKER_DEVICE, 700, &unmuted, sizeof(unmuted));
	neo_amp_off(p);
	if (error || READ_ONCE(p->disabled)) goto abort;
	spin_lock_irqsave(&p->dma_lock, flags);
	if (p->disabled || !neo_audio_healthy(p->aop)) error = -EPERM;
	if (!error) {
		writel(0, p->regs + CH + 0x20);
		gpiod_set_value(p->enable, 1);
		if (!neo_pin_is_output(p, true)) error = -EIO;
	}
	if (!error) {
		p->running = true;
		p->progress_ns = ktime_get_ns();
		hrtimer_start(&p->timer, ns_to_ktime(2000000), HRTIMER_MODE_REL);
	}
	spin_unlock_irqrestore(&p->dma_lock, flags);
	if (!error) { mutex_unlock(&p->lock); return 0; }
abort:
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
	struct neo_speaker *p = snd_pcm_substream_chip(stream);
	hrtimer_cancel(&p->timer);
	cancel_delayed_work_sync(&p->notify_work);
	return 0;
}

static snd_pcm_uframes_t neo_play_pointer(struct snd_pcm_substream *stream)
{
	struct neo_speaker *p = snd_pcm_substream_chip(stream);
	return p->size ? (READ_ONCE(p->hwbytes) % p->size) / 8 : 0;
}

static int neo_play_copy(struct snd_pcm_substream *stream, int channel,
		unsigned long pos, struct iov_iter *iter, unsigned long bytes)
{
	struct neo_speaker *p = snd_pcm_substream_chip(stream);
	s32 temp[256];
	unsigned long flags, done = 0;
	if ((pos | bytes) & 7 || pos > p->size || bytes > p->size - pos) return -EINVAL;
	while (done < bytes) {
		u64 energy[2] = {0}, now;
		unsigned int n = min_t(unsigned long, sizeof(temp), bytes - done), i, a;
		u64 gain;
		if (copy_from_iter(temp, n, iter) != n) return -EFAULT;
		for (i = 0; i < n / 4; i++) {
			s64 q;
			q = temp[i];
			temp[i] = neo_attenuate_s32(temp[i]);
			q = ((q < 0 ? -q : q) + 255) / 256; /* raw S24 energy; immutable gain is in model ceiling */
			energy[i & 1] += q * q;
		}
		spin_lock_irqsave(&p->dma_lock, flags);
		if (p->disabled) { spin_unlock_irqrestore(&p->dma_lock, flags); return -EPERM; }
		now = ktime_get_ns();
		a = 0;
		for (i = 0; i < 2; i++) {
			thermal_advance(&p->thermal[i], now);
			/* Overestimate all committed/held samples at the immutable peak. */
			a = max(a, thermal_admit(&p->thermal[i], energy[i], 1U << 23, 1U << 23,
				(u64)(p->size / 8) * (1U << 23) * (1U << 23)));
		}
		gain = p->thermal[0].att[a];
		for (i = 0; i < n / 4; i++) {
			s64 v = temp[i], magnitude = v < 0 ? -v : v;
			magnitude = mul_u64_u64_shr(magnitude, gain, 32);
			v = v < 0 ? -magnitude : magnitude;
			((__le32 *)stream->runtime->dma_area)[(pos + done) / 4 + i] = cpu_to_le32(neo_q31_float_bits(v));
			if (v) p->nonzero_samples++;
		}
		/* Charging the pre-thermal-limit energy is a conservative overestimate. */
		for (i = 0; i < 2; i++) thermal_charge(&p->thermal[i], energy[i]);
		dma_wmb();
		spin_unlock_irqrestore(&p->dma_lock, flags);
		done += n;
	}
	return 0;
}

static int neo_play_silence(struct snd_pcm_substream *stream, int channel,
		unsigned long pos, unsigned long bytes)
{
	struct neo_speaker *p = snd_pcm_substream_chip(stream);
	if ((pos | bytes) & 7 || pos > p->size || bytes > p->size - pos) return -EINVAL;
	memset(stream->runtime->dma_area + pos, 0, bytes);
	dma_wmb();
	return 0;
}
static const struct snd_pcm_ops neo_play_ops = {
	.open = neo_play_open, .close = neo_play_close, .hw_free = neo_play_free,
	.ioctl = snd_pcm_lib_ioctl, .prepare = neo_play_prepare,
	.trigger = neo_play_trigger, .sync_stop = neo_play_sync_stop, .pointer = neo_play_pointer,
	.copy = neo_play_copy, .fill_silence = neo_play_silence,
};

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
	else { writel(1, p->regs + 4); readl(p->regs); }
	spin_unlock_irqrestore(&p->dma_lock, flags);
	return error ? error : size;
}
static DEVICE_ATTR_WO(fault_inject_stall);

static ssize_t safety_status_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	struct neo_speaker *p = dev_get_drvdata(dev);
	unsigned long flags;
	ssize_t size;
	spin_lock_irqsave(&p->dma_lock, flags);
	size = sysfs_emit(buf, "cutoff_verified=%u disabled=%u gpio=%08x running=%u playback_exposed=1 fixed_gain=3/256 peak_q31=%u nonzero_samples=%llu faults=%llu coil_uc=%lld,%lld magnet_uc=%lld,%lld\n",
		p->cutoff_verified, p->disabled, neo_pin(p), p->running,
		NEO_SPEAKER_PEAK_Q31, p->nonzero_samples, p->faults,
		p->thermal[0].coil, p->thermal[1].coil, p->thermal[0].magnet, p->thermal[1].magnet);
	spin_unlock_irqrestore(&p->dma_lock, flags);
	return size;
}
static DEVICE_ATTR_RO(safety_status);

static struct attribute *neo_speaker_attrs[] = {
	&dev_attr_cutoff_test.attr, &dev_attr_safety_status.attr, &dev_attr_force_disable.attr, &dev_attr_fault_inject_stall.attr, NULL,
};
ATTRIBUTE_GROUPS(neo_speaker);

static int neo_panic_cutoff(struct notifier_block *nb, unsigned long event, void *unused)
{
	struct neo_speaker *p = container_of(nb, struct neo_speaker, panic);
	/* No GPIO locks, transport, DMA aperture or timers in panic context. */
	writel(readl(p->pin) & ~BIT(0), p->pin);
	readl(p->pin);
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
	dma_free_coherent(p->dev, PERIOD, p->silence, p->iova);
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
	mutex_init(&p->lock);
	spin_lock_init(&p->dma_lock);
	INIT_DELAYED_WORK(&p->notify_work, neo_play_notify);
	hrtimer_setup(&p->timer, neo_play_poll, CLOCK_MONOTONIC, HRTIMER_MODE_REL);
	for (i = 0; i < 2; i++) {
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
	p->pin = devm_ioremap(p->dev, 0x31a0001a4, 4);
	if (!p->pin) return -ENOMEM;
	if (!neo_pin_is_output(p, false)) return dev_err_probe(p->dev, -EIO, "Speaker EN is not low\n");
	p->aop = neo_mic_get(p->dev);
	if (IS_ERR(p->aop)) return PTR_ERR(p->aop);
	p->regs = devm_platform_ioremap_resource(pdev, 0);
	if (IS_ERR(p->regs)) return PTR_ERR(p->regs);
	error = dma_set_mask_and_coherent(p->dev, DMA_BIT_MASK(42));
	if (error) return error;
	p->silence = dma_alloc_coherent(p->dev, PERIOD, &p->iova, GFP_KERNEL);
	if (!p->silence) return -ENOMEM;
	memset(p->silence, 0, PERIOD);
	error = devm_add_action_or_reset(p->dev, neo_free_silence, p);
	if (error) return error;
	platform_set_drvdata(pdev, p);
	error = devm_device_add_group(p->dev, neo_speaker_groups[0]);
	if (error) return error;
	error = snd_devm_card_new(p->dev, -1, "NeoSpkr", THIS_MODULE, 0, &card);
	if (error) return error;
	error = snd_pcm_new(card, "Neo protected speakers", 0, 1, 0, &pcm);
	if (error) return error;
	pcm->private_data = p;
	pcm->nonatomic = true;
	strscpy(pcm->name, "Neo fixed -38.6 dB speakers", sizeof(pcm->name));
	snd_pcm_set_ops(pcm, SNDRV_PCM_STREAM_PLAYBACK, &neo_play_ops);
	error = snd_pcm_set_managed_buffer_all(pcm, SNDRV_DMA_TYPE_DEV, p->dev, 32768, 32768);
	if (error) return error;
	strscpy(card->driver, "NeoSpkr", sizeof(card->driver));
	strscpy(card->shortname, "Neo protected speakers", sizeof(card->shortname));
	strscpy(card->longname, "Neo fixed -38.6 dB protected NS TX0 speakers", sizeof(card->longname));
	dev_info(p->dev, "Protected speaker probe GPIO=%08x; playback locked until zero-PCM cutoff test\n", neo_pin(p));
	p->panic.notifier_call = neo_panic_cutoff;
	p->panic.priority = INT_MAX;
	error = atomic_notifier_chain_register(&panic_notifier_list, &p->panic);
	if (error) return error;
	error = devm_add_action_or_reset(p->dev, neo_cleanup, p);
	if (error) return error;
	error = cutoff_test_store(p->dev, NULL, "1", 1);
	if (error < 0) return error;
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
	{ .compatible = "apple,t8140-neo-speaker" }, {}
};
MODULE_DEVICE_TABLE(of, neo_speaker_of_match);
static struct platform_driver neo_speaker_driver = {
	.probe = neo_speaker_probe,
	.shutdown = neo_speaker_shutdown,
	.driver = { .name = "neo-speaker", .of_match_table = neo_speaker_of_match, .pm = pm_sleep_ptr(&neo_speaker_pm) },
};
module_platform_driver(neo_speaker_driver);
MODULE_LICENSE("GPL");
