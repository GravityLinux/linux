// SPDX-License-Identifier: GPL-2.0-only OR MIT
/* J616 AOP-configured, six-channel speaker PCM.
 *
 * Bring-up policy: verified -60dB amplifier ceiling, no mmap, per-channel
 * thermal admission, software mute before DMA teardown, and physical fault cutoff.
 * Full-volume playback needs calibrated displacement/feedback qualification.
 */
#include <linux/dmaengine.h>
#include <linux/module.h>
#include <linux/platform_device.h>
#include <linux/regmap.h>
#include <sound/soc.h>
#include "../codecs/tas2764.h"
#include <linux/gpio/consumer.h>
#include <linux/io.h>
#include <linux/ktime.h>
#include <linux/mutex.h>
#include <linux/of_address.h>
#include <linux/panic_notifier.h>
#include <linux/slab.h>
#include <linux/uio.h>
#include <linux/workqueue.h>
#include <sound/core.h>
#include <sound/control.h>
#include <sound/info.h>
#include <sound/dmaengine_pcm.h>
#include <sound/pcm_params.h>
#include "t8132/thermal.h"
#include "aop_speaker_guard.h"

#define SPKR_CHANNELS 6
#define SPKR_FRAME_BYTES (SPKR_CHANNELS * 4)
#define SPKR_PW0 0x70773020
#define SPKR_PWRD 0x70777264
#define SPKR_IDLE 0x69646c65

int apple_aop_speaker_new(struct device *dev, const void *context);

extern void apple_aop_speaker_context_put(const void *context);
extern int apple_aop_speaker_power(const void *context, u32 state, u32 unknown);
extern int apple_aop_speaker_configure(const void *context);

#if IS_REACHABLE(CONFIG_SND_SOC_TAS2764)
struct aop_speaker {
	struct snd_soc_card card;
	struct snd_soc_dai_link link;
	struct snd_soc_dai_link_component cpu, platform, codecs[SPKR_CHANNELS];
	struct snd_soc_component *amps[SPKR_CHANNELS];
	struct device *dev;
	const void *context;
	struct gpio_desc *enable;
	void __iomem *pin;
	struct mutex lock;
	struct delayed_work watchdog;
	struct notifier_block panic;
	struct snd_pcm_substream *stream;
	struct thermal thermal[SPKR_CHANNELS];
	bool running, powered, faulted, muted;
	u64 progress_ns, copies, frames, stalls;
	unsigned int fault_cursor;
	snd_pcm_uframes_t last_position;
};

static struct aop_speaker *speaker_data(struct snd_pcm_substream *stream)
{
	return snd_soc_card_get_drvdata(snd_soc_substream_to_rtd(stream)->card);
}

static void speaker_cutoff(struct aop_speaker *s)
{
	/* AOP also writes this pin. The GPIO provider caches output registers,
	 * so force the physical clear rather than trusting its cached value.
	 * Address and pin number come from the owned GPIO specifier in the FDT.
	 */
	writel((readl(s->pin) & ~0x6f) | 2, s->pin);
	readl(s->pin);
}

static void speaker_enable(struct aop_speaker *s)
{
	writel((readl(s->pin) & ~0x6f) | 3, s->pin);
	readl(s->pin);
}

static int speaker_gain(struct aop_speaker *s, unsigned int attenuation)
{
	unsigned int c, actual;
	int ret;

	if (s->faulted) {
		speaker_cutoff(s);
		return -EIO;
	}
	for (c = 0; c < SPKR_CHANNELS; c++) {
		if (!s->amps[c])
			return -ENODEV;
		ret = snd_soc_component_write(s->amps[c], TAS2764_DVC,
					      attenuation);
		if (!ret)
			ret = regmap_read_bypassed(s->amps[c]->regmap,
						   TAS2764_DVC, &actual);
		if (ret || actual != attenuation) {
			speaker_cutoff(s);
			s->faulted = true;
			return ret ? ret : -EIO;
		}
	}
	return 0;
}

static void speaker_watchdog(struct work_struct *work)
{
	struct aop_speaker *s = container_of(to_delayed_work(work),
					     struct aop_speaker, watchdog);
	struct snd_pcm_substream *stream = NULL;
	u64 now = ktime_get_ns();

	mutex_lock(&s->lock);
	if (s->running) {
		snd_pcm_uframes_t position =
			snd_dmaengine_pcm_pointer(s->stream);

		if (position != s->last_position) {
			s->progress_ns = now;
			s->last_position = position;
		} else if (now - s->progress_ns > 50000000) {
			speaker_cutoff(s);
			s->faulted = true;
			s->running = false;
			s->stalls++;
			stream = s->stream;
			dev_err(s->dev,
				"speaker DMA stalled; amplifier cut off\n");
		}
		/* Check the physical DVC readback as well as hardware faults. */
		if (s->running) {
			struct snd_soc_component *amp =
				s->amps[s->fault_cursor++ % SPKR_CHANNELS];
			unsigned int dvc;
			int ret = regmap_read_bypassed(amp->regmap, TAS2764_DVC,
						       &dvc);

			if (ret || dvc != (s->muted ? 200 : 120) ||
			    tas2764_check_fault(amp)) {
				speaker_cutoff(s);
				s->faulted = true;
				s->running = false;
				stream = s->stream;
				dev_err(s->dev,
					"speaker amplifier/gain fault; common SDZ cut off\n");
			}
		}
		if (s->running)
			schedule_delayed_work(&s->watchdog,
					      msecs_to_jiffies(10));
	}
	mutex_unlock(&s->lock);
	if (stream)
		snd_pcm_stop_xrun(stream);
}

static const struct snd_pcm_hardware speaker_hw = {
	.info = SNDRV_PCM_INFO_INTERLEAVED | SNDRV_PCM_INFO_BLOCK_TRANSFER,
	.formats = SNDRV_PCM_FMTBIT_S32_LE,
	.rates = SNDRV_PCM_RATE_48000,
	.rate_min = 48000,
	.rate_max = 48000,
	.channels_min = SPKR_CHANNELS,
	.channels_max = SPKR_CHANNELS,
	.buffer_bytes_max = 131040,
	.period_bytes_min = 1536,
	.period_bytes_max = 12288,
	.periods_min = 4,
	.periods_max = 32,
};

static int speaker_open(struct snd_pcm_substream *stream)
{
	struct aop_speaker *s = speaker_data(stream);
	struct dma_chan *chan;
	struct snd_dmaengine_dai_dma_data dma_data = {};
	struct snd_pcm_hardware hw = speaker_hw;
	int ret;

	mutex_lock(&s->lock);
	if (s->faulted) {
		ret = -EIO;
		goto unlock;
	}
	ret = speaker_gain(s, 200);
	if (ret)
		goto unlock;
	ret = apple_aop_speaker_power(s->context, SPKR_PW0, 0);
	if (ret)
		goto unlock;
	s->powered = true;
	ret = apple_aop_speaker_configure(s->context);
	if (ret)
		goto poweroff;
	chan = dma_request_chan(s->dev, "speaker-tx");
	if (IS_ERR(chan)) {
		ret = PTR_ERR(chan);
		goto poweroff;
	}
	ret = stream->dma_buffer.dev.type == SNDRV_DMA_TYPE_UNKNOWN ?
		      snd_pcm_set_managed_buffer(stream,
						 SNDRV_DMA_TYPE_DEV_IRAM,
						 chan->device->dev, 0, 0) :
		      0;
	if (ret < 0)
		goto release;
	ret = snd_dmaengine_pcm_refine_runtime_hwparams(stream, &dma_data, &hw,
							chan);
	if (ret < 0)
		goto release;
	/* The helper adds every DMA-compatible format and pause capabilities.
	 * Only S32 samples pass through our sanitizer; pause is not implemented.
	 */
	hw.formats = SNDRV_PCM_FMTBIT_S32_LE;
	hw.info &= ~(SNDRV_PCM_INFO_PAUSE | SNDRV_PCM_INFO_RESUME);
	stream->runtime->hw = hw;
	ret = snd_pcm_hw_constraint_integer(stream->runtime,
					    SNDRV_PCM_HW_PARAM_PERIODS);
	if (ret < 0)
		goto release;
	ret = snd_dmaengine_pcm_open(stream, chan);
	if (ret < 0)
		goto release;
	s->stream = stream;
	mutex_unlock(&s->lock);
	return 0;
release:
	dma_release_channel(chan);
poweroff:
	apple_aop_speaker_power(s->context, SPKR_IDLE, 0);
	s->powered = false;
unlock:
	mutex_unlock(&s->lock);
	return ret;
}

static int speaker_hw_params(struct snd_pcm_substream *stream,
			     struct snd_pcm_hw_params *params)
{
	struct dma_slave_config config = {};
	int ret = snd_hwparams_to_dma_slave_config(stream, params, &config);

	if (ret)
		return ret;
	/* Native TX0 is 32-bit words, without a DMA frame tag. Six words are
	 * serialized according to the AOP property701 routing template.
	 */
	config.dst_port_window_size = 1;
	return dmaengine_slave_config(snd_dmaengine_pcm_get_chan(stream),
				      &config);
}

static int speaker_prepare(struct snd_pcm_substream *stream)
{
	struct aop_speaker *s = speaker_data(stream);

	mutex_lock(&s->lock);
	memset(stream->runtime->dma_area, 0, stream->runtime->dma_bytes);
	mutex_unlock(&s->lock);
	return s->faulted ? -EIO : 0;
}

static int speaker_trigger(struct snd_pcm_substream *stream, int command)
{
	struct aop_speaker *s = speaker_data(stream);
	int ret;

	mutex_lock(&s->lock);
	switch (command) {
	case SNDRV_PCM_TRIGGER_START:
		if (s->faulted) {
			ret = -EIO;
			break;
		}
		/* Prime DMA with the already sanitized ring before waking the DAC. */
		ret = snd_dmaengine_pcm_trigger(stream, command);
		if (ret)
			break;
		ret = apple_aop_speaker_power(s->context, SPKR_PWRD, 3);
		if (ret) {
			snd_dmaengine_pcm_trigger(stream,
						  SNDRV_PCM_TRIGGER_STOP);
			break;
		}
		s->progress_ns = ktime_get_ns();
		s->last_position = snd_dmaengine_pcm_pointer(stream);
		s->running = true;
		ret = speaker_gain(s, s->muted ? 200 : 120);
		if (ret) {
			s->running = false;
			snd_dmaengine_pcm_trigger(stream,
						  SNDRV_PCM_TRIGGER_STOP);
			break;
		}
		schedule_delayed_work(&s->watchdog, msecs_to_jiffies(10));
		break;
	case SNDRV_PCM_TRIGGER_STOP:
		speaker_gain(s, 200);
		s->running = false;
		ret = snd_dmaengine_pcm_trigger(stream, command);
		break;
	default:
		ret = -EINVAL;
	}
	mutex_unlock(&s->lock);
	return ret;
}

static int speaker_sync_stop(struct snd_pcm_substream *stream)
{
	struct aop_speaker *s = speaker_data(stream);

	cancel_delayed_work_sync(&s->watchdog);
	return snd_dmaengine_pcm_sync_stop(stream);
}

static int speaker_close(struct snd_pcm_substream *stream)
{
	struct aop_speaker *s = speaker_data(stream);
	int ret, off_ret;

	mutex_lock(&s->lock);
	speaker_gain(s, 200);
	s->running = false;
	mutex_unlock(&s->lock);
	cancel_delayed_work_sync(&s->watchdog);
	ret = snd_dmaengine_pcm_close_release_chan(stream);
	mutex_lock(&s->lock);
	if (s->powered) {
		off_ret = apple_aop_speaker_power(s->context, SPKR_PW0, 1);
		if (!ret)
			ret = off_ret;
		off_ret = apple_aop_speaker_power(s->context, SPKR_IDLE, 0);
		if (!ret)
			ret = off_ret;
	}
	s->powered = false;
	s->stream = NULL;
	mutex_unlock(&s->lock);
	return ret;
}

static int speaker_copy(struct snd_pcm_substream *stream, int channel,
			unsigned long pos, struct iov_iter *src,
			unsigned long bytes)
{
	struct aop_speaker *s = speaker_data(stream);
	u64 energy[SPKR_CHANNELS] = {}, now = ktime_get_ns();
	u32 *samples;
	unsigned int i, c, attenuation = 0;
	int ret = 0;

	if (channel != 0 || pos % SPKR_FRAME_BYTES ||
	    bytes % SPKR_FRAME_BYTES || pos > stream->runtime->dma_bytes ||
	    bytes > stream->runtime->dma_bytes - pos)
		return -EINVAL;
	samples = kmalloc(bytes, GFP_KERNEL);
	if (!samples)
		return -ENOMEM;
	if (copy_from_iter(samples, bytes, src) != bytes) {
		ret = -EFAULT;
		goto free;
	}
	mutex_lock(&s->lock);
	if (s->faulted) {
		ret = -EIO;
		goto unlock;
	}
	for (i = 0; i < bytes / 4; i++) {
		s32 sample = aop_speaker_sample(le32_to_cpu(samples[i]));

		samples[i] = sample;
		energy[i % SPKR_CHANNELS] += (s64)sample * sample;
	}
	for (c = 0; c < SPKR_CHANNELS; c++) {
		thermal_advance(&s->thermal[c], now);
		attenuation = max(attenuation,
				  thermal_admit(&s->thermal[c], energy[c],
						8388608, 8388608, 0));
		/* Charge before DMA sees these samples. Not discounting software
		 * attenuation deliberately overestimates admitted heat.
		 */
		thermal_charge(&s->thermal[c], energy[c]);
	}
	for (i = 0; i < bytes / 4; i++) {
		s32 sample = (s64)(s32)samples[i] *
				     (s64)s->thermal[0].att[attenuation] >>
			     32;

		samples[i] = cpu_to_le32(aop_speaker_dma_word(sample));
	}
	memcpy(stream->runtime->dma_area + pos, samples, bytes);
	dma_wmb();
	s->copies++;
	s->frames += bytes / SPKR_FRAME_BYTES;
unlock:
	mutex_unlock(&s->lock);
free:
	kfree(samples);
	return ret;
}

static int soc_open(struct snd_soc_component *c, struct snd_pcm_substream *s)
{
	return speaker_open(s);
}
static int soc_close(struct snd_soc_component *c, struct snd_pcm_substream *s)
{
	return speaker_close(s);
}
static int soc_params(struct snd_soc_component *c, struct snd_pcm_substream *s,
		      struct snd_pcm_hw_params *p)
{
	return speaker_hw_params(s, p);
}
static int soc_prepare(struct snd_soc_component *c, struct snd_pcm_substream *s)
{
	return speaker_prepare(s);
}
static int soc_trigger(struct snd_soc_component *c, struct snd_pcm_substream *s,
		       int command)
{
	return speaker_trigger(s, command);
}
static int soc_sync_stop(struct snd_soc_component *c,
			 struct snd_pcm_substream *s)
{
	return speaker_sync_stop(s);
}
static snd_pcm_uframes_t soc_pointer(struct snd_soc_component *c,
				     struct snd_pcm_substream *s)
{
	return snd_dmaengine_pcm_pointer(s);
}
static int soc_copy(struct snd_soc_component *c, struct snd_pcm_substream *s,
		    int channel, unsigned long pos, struct iov_iter *src,
		    unsigned long bytes)
{
	return speaker_copy(s, channel, pos, src, bytes);
}
static int soc_mmap(struct snd_soc_component *c, struct snd_pcm_substream *s,
		    struct vm_area_struct *vma)
{
	return -ENXIO;
}

static const struct snd_soc_component_driver speaker_component = {
	.name = "aop-protected-speaker",
	.open = soc_open,
	.close = soc_close,
	.hw_params = soc_params,
	.prepare = soc_prepare,
	.trigger = soc_trigger,
	.sync_stop = soc_sync_stop,
	.pointer = soc_pointer,
	.copy = soc_copy,
	.mmap = soc_mmap,
};

static int speaker_dai_fmt(struct snd_soc_dai *dai, unsigned int format)
{
	return 0;
}
static const struct snd_soc_dai_ops speaker_dai_ops = {
	.set_fmt = speaker_dai_fmt
};
static struct snd_soc_dai_driver speaker_dai = {
	.name = "aop-speaker-tx",
	.ops = &speaker_dai_ops,
	.playback = { .stream_name = "AOP Speaker TX",
		      .channels_min = 6,
		      .channels_max = 6,
		      .rates = SNDRV_PCM_RATE_48000,
		      .formats = SNDRV_PCM_FMTBIT_S32_LE },
};

static int speaker_link_init(struct snd_soc_pcm_runtime *rtd)
{
	struct aop_speaker *s = snd_soc_card_get_drvdata(rtd->card);
	unsigned int c;
	int ret;

	for (c = 0; c < SPKR_CHANNELS; c++) {
		struct snd_soc_dai *dai = snd_soc_rtd_to_codec(rtd, c);

		s->amps[c] = dai->component;
		ret = snd_soc_dai_set_tdm_slot(dai, BIT(c), 0, 6, 32);
		if (ret)
			return ret;
		/* Native ADT amplifier gain15dBV: 11dBV + eight half-dB steps. */
		ret = snd_soc_component_update_bits(
			dai->component, TAS2764_CHNL_0, 0x3e, 8 << 1);
		if (ret < 0)
			return ret;
		ret = snd_soc_component_update_bits(
			dai->component, TAS2764_DC_BLK0, 7, c >= 4 ? 6 : 1);
		if (ret < 0)
			return ret;
	}
	return speaker_gain(s, 200);
}

static int speaker_switch_get(struct snd_kcontrol *control,
			      struct snd_ctl_elem_value *value)
{
	struct aop_speaker *s =
		snd_soc_card_get_drvdata(snd_kcontrol_chip(control));

	value->value.integer.value[0] = !READ_ONCE(s->muted) &&
					!READ_ONCE(s->faulted);
	return 0;
}

static int speaker_switch_put(struct snd_kcontrol *control,
			      struct snd_ctl_elem_value *value)
{
	struct aop_speaker *s =
		snd_soc_card_get_drvdata(snd_kcontrol_chip(control));
	long enabled = value->value.integer.value[0];
	bool changed;
	int ret = 0;

	if (enabled < 0 || enabled > 1)
		return -EINVAL;
	mutex_lock(&s->lock);
	changed = s->muted != !enabled;
	if (s->faulted && enabled) {
		ret = -EIO;
	} else if (changed) {
		s->muted = !enabled;
		if (s->running)
			ret = speaker_gain(s, s->muted ? 200 : 120);
	}
	mutex_unlock(&s->lock);
	return ret ? ret : changed;
}

static const struct snd_kcontrol_new speaker_controls[] = { {
	.iface = SNDRV_CTL_ELEM_IFACE_MIXER,
	.name = "Speakers Playback Switch",
	.info = snd_ctl_boolean_mono_info,
	.get = speaker_switch_get,
	.put = speaker_switch_put,
} };

static const struct snd_soc_dapm_widget speaker_widgets[] = {
	SND_SOC_DAPM_SPK("Speakers", NULL),
};
static const struct snd_soc_dapm_route speaker_routes[] = {
	{ "Speakers", NULL, "Left Woofer 1 OUT" },
	{ "Speakers", NULL, "Right Woofer 1 OUT" },
	{ "Speakers", NULL, "Left Woofer 2 OUT" },
	{ "Speakers", NULL, "Right Woofer 2 OUT" },
	{ "Speakers", NULL, "Left Tweeter OUT" },
	{ "Speakers", NULL, "Right Tweeter OUT" },
};

static void speaker_proc_read(struct snd_info_entry *entry,
			      struct snd_info_buffer *buffer)
{
	struct aop_speaker *s = entry->private_data;
	unsigned int c;

	mutex_lock(&s->lock);
	snd_iprintf(
		buffer,
		"running=%u powered=%u faulted=%u muted=%u hardware_ceiling_half_db=120 format=S24_LSB32 pin=%#x copies=%llu frames=%llu stalls=%llu\n",
		s->running, s->powered, s->faulted, s->muted, readl(s->pin),
		s->copies, s->frames, s->stalls);
	for (c = 0; c < SPKR_CHANNELS; c++)
		snd_iprintf(
			buffer,
			"channel=%u coil_mdeg=%lld magnet_mdeg=%lld attenuation_half_db=%u\n",
			c, div_s64(s->thermal[c].coil, 1000),
			div_s64(s->thermal[c].magnet, 1000),
			s->thermal[c].attenuation);
	mutex_unlock(&s->lock);
}

static int speaker_panic(struct notifier_block *nb, unsigned long event,
			 void *unused)
{
	struct aop_speaker *s = container_of(nb, struct aop_speaker, panic);

	speaker_cutoff(s);
	return NOTIFY_DONE;
}

static int speaker_late_probe(struct snd_soc_card *card)
{
	struct aop_speaker *s = snd_soc_card_get_drvdata(card);

	static const char *const locked[] = {
		"Speaker Volume",
		"Amp Gain Volume",
		"HPF Corner Frequency",
		"OCE Handling",
	};
	unsigned int c, i;

	for (c = 0; c < SPKR_CHANNELS; c++) {
		for (i = 0; i < ARRAY_SIZE(locked); i++) {
			char name[128];
			struct snd_kcontrol *control;

			snprintf(name, sizeof(name), "%s %s",
				 s->amps[c]->name_prefix, locked[i]);
			control = snd_soc_card_get_kcontrol(card, name);
			if (!control)
				return -ENOENT;
			control->vd[0].access &= ~SNDRV_CTL_ELEM_ACCESS_WRITE;
		}
	}
	snd_card_ro_proc_new(card->snd_card, "speaker_safety", s,
			     speaker_proc_read);
	return 0;
}

static void speaker_free(void *data)
{
	struct aop_speaker *s = data;
	unsigned int c;

	speaker_cutoff(s);
	atomic_notifier_chain_unregister(&panic_notifier_list, &s->panic);
	cancel_delayed_work_sync(&s->watchdog);
	for (c = 0; c < SPKR_CHANNELS; c++)
		of_node_put(s->codecs[c].of_node);
	gpiod_put(s->enable);
	iounmap(s->pin);
	apple_aop_speaker_context_put(s->context);
	kfree(s);
}

/* ASoC requires a bound device for component naming. The Rust audio driver
 * creates this child only after the AOP service is ready. Keep its driver and
 * card lifetime separate from the parent's Rust driver data.
 */
static int speaker_child_probe(struct platform_device *pdev)
{
	return 0;
}
static struct platform_driver speaker_child_driver = {
	.probe = speaker_child_probe,
	.driver = { .name = "aop-protected-speakers" },
};
static DEFINE_MUTEX(speaker_driver_lock);
static unsigned int speaker_driver_users;

static void speaker_driver_put(void *unused)
{
	mutex_lock(&speaker_driver_lock);
	if (!--speaker_driver_users)
		platform_driver_unregister(&speaker_child_driver);
	mutex_unlock(&speaker_driver_lock);
}

static int speaker_driver_get(struct device *parent)
{
	int ret = 0;

	mutex_lock(&speaker_driver_lock);
	if (!speaker_driver_users)
		ret = platform_driver_register(&speaker_child_driver);
	if (!ret)
		speaker_driver_users++;
	mutex_unlock(&speaker_driver_lock);
	if (ret)
		return ret;
	return devm_add_action_or_reset(parent, speaker_driver_put, NULL);
}

static void speaker_child_unregister(void *data)
{
	platform_device_unregister(data);
}

int apple_aop_speaker_new(struct device *dev, const void *context)
{
	struct platform_device *child;
	struct platform_device_info info = {
		.parent = dev,
		.name = "aop-protected-speakers",
		.id = PLATFORM_DEVID_AUTO,
		.fwnode = dev_fwnode(dev),
		.of_node_reused = true,
	};
	struct aop_speaker *s;
	struct of_phandle_args gpio;
	struct resource resource;
	unsigned int c;
	int ret;

	ret = speaker_driver_get(dev);
	if (ret)
		goto put;
	/* Keep ASoC card driver data separate from the parent Rust driver. */
	child = platform_device_register_full(&info);
	if (IS_ERR(child)) {
		ret = PTR_ERR(child);
		goto put;
	}
	ret = devm_add_action_or_reset(dev, speaker_child_unregister, child);
	if (ret)
		goto put;
	dev = &child->dev;
	s = kzalloc(sizeof(*s), GFP_KERNEL);
	if (!s) {
		ret = -ENOMEM;
		goto put;
	}
	s->dev = dev;
	s->context = context;
	mutex_init(&s->lock);
	INIT_DELAYED_WORK(&s->watchdog, speaker_watchdog);
	s->enable = gpiod_get(dev, "speaker-enable", GPIOD_OUT_LOW);
	if (IS_ERR(s->enable)) {
		ret = PTR_ERR(s->enable);
		goto free;
	}
	ret = of_parse_phandle_with_args(dev_of_node(dev),
					 "speaker-enable-gpios", "#gpio-cells",
					 0, &gpio);
	if (ret)
		goto gpio_put;
	if (!of_device_is_compatible(gpio.np, "apple,t8103-pinctrl")) {
		of_node_put(gpio.np);
		ret = -EINVAL;
		goto gpio_put;
	}
	ret = of_address_to_resource(gpio.np, 0, &resource);
	of_node_put(gpio.np);
	if (ret || gpio.args_count != 2 || gpio.args[1] ||
	    (u64)gpio.args[0] * 4 + 4 > resource_size(&resource)) {
		ret = -EINVAL;
		goto gpio_put;
	}
	s->pin = ioremap(resource.start + gpio.args[0] * 4, 4);
	if (!s->pin) {
		ret = -ENOMEM;
		goto gpio_put;
	}
	speaker_cutoff(s);
	for (c = 0; c < SPKR_CHANNELS; c++) {
		bool tweeter = c >= 4;
		/* Native J616 AID35 SPP3 preset. Electrical full scale and minimum
		 * resistance deliberately overestimate power during quiet bring-up.
		 */
		struct thermal_params p = {
			.fs_mv = 32000,
			.r_mohm = 2000,
			.margin_pct = 150,
			.ceiling = 120,
			.r_coil = tweeter ? 26910 : 26060,
			.r_magnet = tweeter ? 23930 : 25550,
			.tau_coil_ms = tweeter ? 1550 : 5530,
			.tau_magnet_ms = tweeter ? 73260 : 197250,
			.t_ambient = 50000,
			.t_limit = 140000,
			.t_window = 20000,
			.t_hysteresis = 5000,
			.t_hard = 150000,
			.t_stall = 180000,
			.delivery_ns = 120000000,
			.stall_frames = 4800,
		};

		ret = thermal_init(&s->thermal[c], &p, ktime_get_ns());
		if (ret)
			goto unmap;
	}
	for (c = 0; c < SPKR_CHANNELS; c++) {
		s->codecs[c].of_node = of_parse_phandle(
			dev_of_node(dev), "apple,speaker-codecs", c);
		s->codecs[c].dai_name = "tas2764 ASI1";
		if (!s->codecs[c].of_node) {
			ret = -EINVAL;
			goto unmap;
		}
	}
	s->cpu = (struct snd_soc_dai_link_component){
		.name = dev_name(dev), .dai_name = "aop-speaker-tx"
	};
	s->platform.name = dev_name(dev);
	s->link = (struct snd_soc_dai_link){
		.name = "Protected Speakers",
		.stream_name = "Protected Speakers",
		.playback_only = 1,
		.nonatomic = 1,
		.cpus = &s->cpu,
		.num_cpus = 1,
		.codecs = s->codecs,
		.num_codecs = SPKR_CHANNELS,
		.platforms = &s->platform,
		.num_platforms = 1,
		.dai_fmt = SND_SOC_DAIFMT_I2S | SND_SOC_DAIFMT_NB_IF |
			   SND_SOC_DAIFMT_CBC_CFC,
		.init = speaker_link_init,
	};
	s->card = (struct snd_soc_card){
		.name = "J616 Speakers",
		.driver_name = "J616Speakers",
		.owner = THIS_MODULE,
		.dev = dev,
		.dai_link = &s->link,
		.num_links = 1,
		.late_probe = speaker_late_probe,
		.controls = speaker_controls,
		.num_controls = ARRAY_SIZE(speaker_controls),
		.dapm_widgets = speaker_widgets,
		.num_dapm_widgets = ARRAY_SIZE(speaker_widgets),
		.dapm_routes = speaker_routes,
		.num_dapm_routes = ARRAY_SIZE(speaker_routes),
		.fully_routed = true,
	};
	snd_soc_card_set_drvdata(&s->card, s);
	s->panic.notifier_call = speaker_panic;
	atomic_notifier_chain_register(&panic_notifier_list, &s->panic);
	ret = devm_add_action_or_reset(dev, speaker_free, s);
	if (ret)
		return ret;
	/* The existing TAS2764 driver owns codec reset/configuration and DAPM.
	 * Keep common SDZ high during ordinary mute; faults cut it physically.
	 */
	speaker_enable(s);
	ret = devm_snd_soc_register_component(dev, &speaker_component,
					      &speaker_dai, 1);
	if (ret)
		return ret;
	return devm_snd_soc_register_card(dev, &s->card);
unmap:
	for (c = 0; c < SPKR_CHANNELS; c++)
		of_node_put(s->codecs[c].of_node);
	iounmap(s->pin);
gpio_put:
	gpiod_put(s->enable);
free:
	kfree(s);
put:
	apple_aop_speaker_context_put(context);
	return ret;
}

#else
int apple_aop_speaker_new(struct device *dev, const void *context)
{
	apple_aop_speaker_context_put(context);
	dev_info(dev, "AOP speakers require TAS2764 and I2C support\n");
	return 0;
}
#endif
