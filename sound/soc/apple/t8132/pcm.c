// SPDX-License-Identifier: GPL-2.0-only
/* J773g machine routing, protected PCM copies and in-kernel speaker limiting. */
#include <linux/module.h>
#include <linux/moduleparam.h>
#include <linux/of.h>
#include <linux/platform_device.h>
#include <linux/uio.h>
#include <linux/vmalloc.h>
#include <linux/workqueue.h>
#include <sound/soc.h>
#include <sound/pcm_params.h>
#include <sound/dmaengine_pcm.h>
#include "../../codecs/tas2764.h"
#include "playback.h"
#include "thermal.h"
#include "progress.h"

#define SPEAKER_FMT (SND_SOC_DAIFMT_I2S | SND_SOC_DAIFMT_NB_IF | SND_SOC_DAIFMT_CBC_CFC)
#define MONITOR_MS 20
#define MAX_BUFFER_FRAMES 8192 /* 32 KiB of mono S24 in 32 bits */
#define MAX_PERIOD_FRAMES 1024
#define MAX_SLOTS (MAX_BUFFER_FRAMES / 256)
#define FIFO_FRAMES 512 /* ADMAC 2 KiB per-channel carveout */
#define STALL_FAULT_FRAMES 4800 /* deficit growth within the history window: 100 ms */
/* A charged sample plays within one ring, the FIFO and a monitor tick. */
#define DELIVERY_NS ((MAX_BUFFER_FRAMES + FIFO_FRAMES + 960) * (NSEC_PER_SEC / THERMAL_RATE))
/* A held sample keeps playing until the deficit growth is seen at a tick,
 * with a tick of jitter, plus the FIFO it can hide in and the I2C mute. */
#define STALL_FRAMES (STALL_FAULT_FRAMES + 2 * 960 + FIFO_FRAMES + 240)

static unsigned int ceiling_dvc = 20;
module_param(ceiling_dvc, uint, 0444);
MODULE_PARM_DESC(ceiling_dvc, "TAS2764 digital volume ceiling in half-dB steps (default 20, -10 dB)");
static unsigned int full_scale_mv = 17000;
module_param(full_scale_mv, uint, 0444);
MODULE_PARM_DESC(full_scale_mv, "Assumed peak amplifier output at digital full scale and DVC 0");
static unsigned int resistance_mohm = 4000;
module_param(resistance_mohm, uint, 0444);
MODULE_PARM_DESC(resistance_mohm, "Assumed minimum speaker resistance");
static unsigned int power_margin_pct = 200;
module_param(power_margin_pct, uint, 0444);
MODULE_PARM_DESC(power_margin_pct, "Multiplier on the electrical power bound, percent");

struct j773g_audio {
	struct snd_soc_card card;
	struct snd_soc_dai_link links[2];
	struct snd_soc_dai_link_component cpus[2], codecs[2], platforms[2];
	struct mutex lock;
	struct delayed_work monitor;
	struct snd_pcm_substream *stream;
	struct thermal thermal;
	struct progress progress;
	bool running, faulted;
	const char *reason;
	int volume;
	u64 held_frames, replay_frames, stalls, sumsq_total, preload_sumsq;
	u32 slot_peak[MAX_SLOTS];
};

static struct j773g_audio *speaker_data(struct snd_soc_component *component)
{
	return dev_get_drvdata(component->dev);
}

/* lock is held for everything below that touches the ring or the model. */
static u32 speaker_slot_peak(struct snd_pcm_runtime *rt, unsigned int slot)
{
	const s32 *ring = (const s32 *)rt->dma_area + slot * rt->period_size;
	u32 peak = 0;
	unsigned int i;

	for (i = 0; i < rt->period_size; i++)
		peak = max_t(u32, peak, abs((s32)le32_to_cpu(ring[i])));
	return peak;
}

static u32 speaker_peak(struct j773g_audio *a)
{
	struct snd_pcm_runtime *rt = a->stream ? a->stream->runtime : NULL;
	u32 peak = 0;
	unsigned int i;

	if (rt)
		for (i = 0; i < rt->buffer_size / rt->period_size; i++)
			peak = max(peak, a->slot_peak[i]);
	return peak;
}

static void speaker_fault(struct j773g_audio *a, const char *reason)
{
	a->faulted = true;
	a->reason = reason;
	audio_amp_set_dvc(AUDIO_AMP_MUTE);
}

/* Burst-granular DMA fetch position on the monotonic hw_ptr lap. The status
 * pointer is at most one period stale, so the fresh position is ahead of it
 * by less than one ring. */
static u64 speaker_position(struct snd_pcm_substream *s)
{
	struct snd_pcm_runtime *rt = s->runtime;
	u64 hw = READ_ONCE(rt->status->hw_ptr);
	u64 fine = hw - hw % rt->buffer_size + snd_dmaengine_pcm_pointer(s);

	return fine < hw ? fine + rt->buffer_size : fine;
}

/* Charge held or replayed samples at the ring peak and volume ceiling.
 * ALSA handles underrun stopping; the STOP trigger mutes the amplifier.
 * lock is held, stream running. */
static void speaker_account(struct j773g_audio *a, u64 now)
{
	struct snd_pcm_runtime *rt = a->stream->runtime;
	u64 peak = speaker_peak(a);
	struct progress_charge c = progress_account(&a->progress, now, speaker_position(a->stream),
						    READ_ONCE(rt->control->appl_ptr), FIFO_FRAMES);

	thermal_charge(&a->thermal, peak * peak * (c.held + c.replay));
	a->held_frames += c.held;
	a->replay_frames += c.replay;
}

static void speaker_monitor(struct work_struct *work)
{
	struct j773g_audio *a = container_of(to_delayed_work(work), struct j773g_audio, monitor);
	struct snd_pcm_substream *stop = NULL;
	u64 now = ktime_get_ns();

	mutex_lock(&a->lock);
	if (!a->running)
		goto out;
	thermal_advance(&a->thermal, now);
	speaker_account(a, now);
	if (progress_tick(&a->progress, STALL_FAULT_FRAMES)) {
		/* Silence before stopping: a stalled serializer holds its sample. */
		a->stalls++;
		a->reason = "dma-stall";
		audio_amp_set_dvc(AUDIO_AMP_MUTE);
		stop = a->stream;
	} else if (audio_amp_check_fault()) {
		speaker_fault(a, "amp-fault");
		stop = a->stream;
	} else {
		thermal_admit(&a->thermal, 0, 0, speaker_peak(a), 0);
		schedule_delayed_work(&a->monitor, msecs_to_jiffies(MONITOR_MS));
	}
out:
	mutex_unlock(&a->lock);
	if (stop)
		snd_pcm_stop_xrun(stop);
}

static int speaker_open(struct snd_soc_component *component, struct snd_pcm_substream *s)
{
	int ret;

	s->runtime->hw.info &= ~(SNDRV_PCM_INFO_MMAP | SNDRV_PCM_INFO_MMAP_VALID |
				SNDRV_PCM_INFO_PAUSE | SNDRV_PCM_INFO_RESUME);
	s->runtime->hw.info |= SNDRV_PCM_INFO_NO_REWINDS;
	ret = snd_pcm_hw_constraint_mask64(s->runtime, SNDRV_PCM_HW_PARAM_FORMAT,
					 SNDRV_PCM_FMTBIT_S24_LE);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_constraint_single(s->runtime, SNDRV_PCM_HW_PARAM_RATE, THERMAL_RATE);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_constraint_single(s->runtime, SNDRV_PCM_HW_PARAM_CHANNELS, 1);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_constraint_minmax(s->runtime, SNDRV_PCM_HW_PARAM_BUFFER_BYTES,
					 4096, MAX_BUFFER_FRAMES * 4);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_constraint_minmax(s->runtime, SNDRV_PCM_HW_PARAM_PERIOD_BYTES,
					 1024, MAX_PERIOD_FRAMES * 4);
	if (ret < 0)
		return ret;
	return snd_pcm_hw_constraint_integer(s->runtime, SNDRV_PCM_HW_PARAM_PERIODS);
}

static int speaker_sync_stop(struct snd_soc_component *component, struct snd_pcm_substream *s)
{
	struct j773g_audio *a = speaker_data(component);

	cancel_delayed_work_sync(&a->monitor);
	return snd_dmaengine_pcm_sync_stop(s);
}

static int speaker_hw_free(struct snd_soc_component *component, struct snd_pcm_substream *s)
{
	struct j773g_audio *a = speaker_data(component);

	mutex_lock(&a->lock);
	a->running = false;
	audio_amp_set_dvc(AUDIO_AMP_MUTE);
	mutex_unlock(&a->lock);
	cancel_delayed_work_sync(&a->monitor);
	return 0;
}

static int speaker_prepare(struct snd_soc_component *component, struct snd_pcm_substream *s)
{
	struct j773g_audio *a = speaker_data(component);

	speaker_hw_free(component, s);
	if (a->faulted)
		return -EIO;
	mutex_lock(&a->lock);
	a->stream = s;
	memset(s->runtime->dma_area, 0, s->runtime->dma_bytes);
	memset(a->slot_peak, 0, sizeof(a->slot_peak));
	a->preload_sumsq = 0;
	mutex_unlock(&a->lock);
	return 0;
}

static int speaker_trigger(struct snd_soc_component *component,
			   struct snd_pcm_substream *s, int cmd)
{
	struct j773g_audio *a = speaker_data(component);
	u64 now = ktime_get_ns();
	int ret = 0;

	mutex_lock(&a->lock);
	switch (cmd) {
	case SNDRV_PCM_TRIGGER_START:
		if (a->faulted) {
			ret = -EIO;
			break;
		}
		ret = audio_amp_set_dvc(ceiling_dvc);
		if (ret) {
			speaker_fault(a, "i2c-error");
			break;
		}
		a->stream = s;
		progress_start(&a->progress, now, speaker_position(s));
		thermal_advance(&a->thermal, now);
		/* The preloaded ring starts playing now. */
		thermal_charge(&a->thermal, a->preload_sumsq);
		a->preload_sumsq = 0;
		a->running = true;
		schedule_delayed_work(&a->monitor, msecs_to_jiffies(MONITOR_MS));
		break;
	case SNDRV_PCM_TRIGGER_STOP:
	case SNDRV_PCM_TRIGGER_SUSPEND:
		/* The DMA is already terminated here, so its position cannot be
		 * read; at most one tick before STOP goes unaccounted. */
		a->running = false;
		cancel_delayed_work(&a->monitor);
		audio_amp_set_dvc(AUDIO_AMP_MUTE);
		break;
	default:
		ret = -EINVAL;
	}
	mutex_unlock(&a->lock);
	return ret;
}

static int speaker_copy(struct snd_soc_component *component, struct snd_pcm_substream *s,
			int channel, unsigned long pos, struct iov_iter *iter,
			unsigned long bytes)
{
	struct j773g_audio *a = speaker_data(component);
	struct snd_pcm_runtime *rt = s->runtime;
	s32 *samples;
	unsigned int i, slot, frames = bytes / sizeof(*samples), first = pos / sizeof(*samples);
	u64 raw = 0, sumsq = 0, att;
	u32 raw_peak = 0;
	int gain, ret = 0;

	if (bytes % sizeof(*samples) || pos % sizeof(*samples) || pos + bytes > rt->dma_bytes)
		return -EINVAL;
	if (!frames)
		return 0;
	samples = kvmalloc(bytes, GFP_KERNEL);
	if (!samples)
		return -ENOMEM;
	if (copy_from_iter(samples, bytes, iter) != bytes) {
		kvfree(samples);
		return -EFAULT;
	}
	/* User volume first; the limiter sees and bounds what remains. */
	gain = READ_ONCE(a->volume);
	for (i = 0; i < frames; i++) {
		s64 v = div_s64((s64)sign_extend32(le32_to_cpu(samples[i]), 23) * gain, 100);

		samples[i] = v;
		raw += v * v;
		raw_peak = max_t(u32, raw_peak, abs((s32)v));
	}
	mutex_lock(&a->lock);
	if (a->faulted) {
		ret = -EIO;
		goto out;
	}
	thermal_advance(&a->thermal, ktime_get_ns());
	/* Settle the interval before this copy changes the ring peaks. */
	if (a->running)
		speaker_account(a, a->thermal.last_ns);
	att = a->thermal.att[thermal_admit(&a->thermal, raw, raw_peak, speaker_peak(a),
					   a->running ? 0 : a->preload_sumsq)];
	for (i = 0; i < frames; i++) {
		s32 v = samples[i];
		u32 m = ((u64)abs(v) * att) >> 32;

		sumsq += (u64)m * m;
		samples[i] = cpu_to_le32(v < 0 ? -(s32)m : (s32)m);
	}
	/* Every joule is charged before DMA can see it. Preload plays at START. */
	if (a->running)
		thermal_charge(&a->thermal, sumsq);
	else
		a->preload_sumsq += sumsq;
	a->sumsq_total += sumsq;
	memcpy(rt->dma_area + pos, samples, bytes);
	dma_wmb();
	for (slot = first / rt->period_size; slot <= (first + frames - 1) / rt->period_size; slot++)
		a->slot_peak[slot] = speaker_slot_peak(rt, slot);
out:
	mutex_unlock(&a->lock);
	kvfree(samples);
	return ret;
}

static int speaker_mmap(struct snd_soc_component *component,
			struct snd_pcm_substream *s, struct vm_area_struct *vma)
{
	return -ENXIO;
}

static const struct snd_soc_component_driver speaker_component = {
	.name = "j773g-protection",
	.module_get_upon_open = 1,
	.open = speaker_open,
	.prepare = speaker_prepare,
	.hw_free = speaker_hw_free,
	.sync_stop = speaker_sync_stop,
	.trigger = speaker_trigger,
	.copy = speaker_copy,
	.mmap = speaker_mmap,
};

static int speaker_fe_init(struct snd_soc_pcm_runtime *rtd)
{
	int ret = snd_soc_dai_set_tdm_slot(snd_soc_rtd_to_cpu(rtd, 0), 1, 0, 1, 32);

	if (ret)
		return ret;
	return snd_soc_dai_set_bclk_ratio(snd_soc_rtd_to_cpu(rtd, 0), 250);
}

static int speaker_be_init(struct snd_soc_pcm_runtime *rtd)
{
	return snd_soc_dai_set_tdm_slot(snd_soc_rtd_to_codec(rtd, 0), 1, 0, 1, 32);
}

static int speaker_volume_info(struct snd_kcontrol *k, struct snd_ctl_elem_info *u)
{
	u->type = SNDRV_CTL_ELEM_TYPE_INTEGER;
	u->count = 1;
	u->value.integer.min = 0;
	u->value.integer.max = 100;
	return 0;
}

static int speaker_volume_get(struct snd_kcontrol *k, struct snd_ctl_elem_value *u)
{
	struct j773g_audio *a = snd_soc_card_get_drvdata(snd_kcontrol_chip(k));

	u->value.integer.value[0] = READ_ONCE(a->volume);
	return 0;
}

static int speaker_volume_put(struct snd_kcontrol *k, struct snd_ctl_elem_value *u)
{
	struct j773g_audio *a = snd_soc_card_get_drvdata(snd_kcontrol_chip(k));
	long volume = u->value.integer.value[0];
	bool changed;

	if (volume < 0 || volume > 100)
		return -EINVAL;
	changed = READ_ONCE(a->volume) != volume;
	WRITE_ONCE(a->volume, volume);
	return changed;
}

static const struct snd_kcontrol_new speaker_controls[] = {{
	.iface = SNDRV_CTL_ELEM_IFACE_MIXER,
	.name = "Speaker Playback Volume",
	.info = speaker_volume_info,
	.get = speaker_volume_get,
	.put = speaker_volume_put,
}};

static const struct snd_soc_dapm_widget speaker_widgets[] = {
	SND_SOC_DAPM_SPK("Speaker", NULL),
};

static const struct snd_soc_dapm_route speaker_routes[] = {
	{ "I2S0 TX", NULL, "PCM0 TX" },
	{ "Speaker", NULL, "OUT" },
};

static int speaker_fixup_controls(struct snd_soc_card *card)
{
	static const char * const locked_controls[] = {
		"Amp Gain Volume", "ASI1 Sel",
		"HPF Corner Frequency", "OCE Handling", "ISENSE Switch", "VSENSE Switch",
	};
	/* Thermal values from the J773g AID36/AUSP_0329 ATSP preset, decoded with
	 * upstream speakersafetyd's audump.py. t_hard bounds bursts above the
	 * preset's 140 C control target; t_stall is the old daemon's assert
	 * level and only applies if a sample is held during a DMA stall.
	 * Electrical values are module parameters: they are assumptions
	 * awaiting measurement. */
	struct thermal_params params = {
		.fs_mv = full_scale_mv, .r_mohm = resistance_mohm,
		.margin_pct = power_margin_pct, .ceiling = ceiling_dvc,
		.r_coil = 40000, .r_magnet = 60000, .tau_coil_ms = 3400, .tau_magnet_ms = 210000,
		.t_ambient = 50000, .t_limit = 140000, .t_window = 20000, .t_hysteresis = 5000,
		.t_hard = 150000, .t_stall = 180000,
		.delivery_ns = DELIVERY_NS, .stall_frames = STALL_FRAMES,
	};
	struct j773g_audio *a = snd_soc_card_get_drvdata(card);
	struct snd_soc_pcm_runtime *rtd;
	struct snd_soc_component *codec = NULL;
	struct snd_kcontrol *kcontrol;
	int i, ret;

	for_each_card_rtds(card, rtd)
		if (rtd->dai_link->no_pcm)
			codec = snd_soc_rtd_to_codec(rtd, 0)->component;
	if (!codec)
		return -ENODEV;
	ret = audio_amp_bind(codec);
	if (ret)
		return ret;
	ret = snd_soc_component_write(codec, TAS2764_CHNL_0, 0);
	if (ret)
		return ret;
	ret = snd_soc_set_enum_kctl(card, "ASI1 Sel", "Left");
	if (ret < 0)
		return ret;
	ret = snd_soc_set_enum_kctl(card, "HPF Corner Frequency", "2 Hz");
	if (ret < 0)
		return ret;
	/*
	 * Protection accesses amplifier attenuation directly. Hide its codec
	 * control so ALSA mixers do not merge it with our software playback
	 * volume control. Other users of the codec retain their controls.
	 */
	kcontrol = snd_soc_card_get_kcontrol(card, "Speaker Volume");
	if (!kcontrol)
		return -ENOENT;
	ret = snd_ctl_remove(card->snd_card, kcontrol);
	if (ret < 0)
		return ret;
	for (i = 0; i < ARRAY_SIZE(locked_controls); i++) {
		struct snd_kcontrol *k = snd_soc_card_get_kcontrol(card, locked_controls[i]);

		if (!k)
			return -ENOENT;
		k->vd[0].access &= ~SNDRV_CTL_ELEM_ACCESS_WRITE;
	}
	ret = thermal_init(&a->thermal, &params, ktime_get_ns());
	if (ret) {
		dev_err(card->dev, "invalid speaker limiter parameters\n");
		return ret;
	}
	dev_info(card->dev, "speaker limiter: ceiling -%u.%u dB, %u mW peak, %u mW continuous, ramp to -%u.%u dB\n",
		 ceiling_dvc / 2, ceiling_dvc % 2 * 5, a->thermal.p_ceiling_mw, a->thermal.p_max_mw,
		 a->thermal.att_min / 2, a->thermal.att_min % 2 * 5);
	return 0;
}

static ssize_t state_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	struct j773g_audio *a = dev_get_drvdata(dev);
	struct thermal *t = &a->thermal;
	ssize_t n;

	mutex_lock(&a->lock);
	thermal_advance(t, ktime_get_ns());
	n = sysfs_emit(buf, "running=%u faulted=%u reason=%s coil_mdeg=%lld magnet_mdeg=%lld attenuation=%u att_min=%u ceiling=%u p_ceiling_mw=%u p_max_mw=%u held_frames=%llu replay_frames=%llu stalls=%llu sumsq=%llu peak=%u volume=%d hardware_dvc=%d\n",
		       a->running, a->faulted, a->reason, div_s64(t->coil, 1000),
		       div_s64(t->magnet, 1000), t->attenuation, t->att_min, ceiling_dvc,
		       t->p_ceiling_mw, t->p_max_mw, a->held_frames, a->replay_frames, a->stalls,
		       a->sumsq_total, speaker_peak(a), a->volume, audio_amp_read_dvc());
	mutex_unlock(&a->lock);
	return n;
}
static DEVICE_ATTR_RO(state);
static struct attribute *speaker_attrs[] = { &dev_attr_state.attr, NULL };
ATTRIBUTE_GROUPS(speaker);

static void speaker_put_node(void *node)
{
	of_node_put(node);
}

static int speaker_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct j773g_audio *a;
	struct device_node *mca, *codec;
	int ret;

	a = devm_kzalloc(dev, sizeof(*a), GFP_KERNEL);
	if (!a)
		return -ENOMEM;
	platform_set_drvdata(pdev, a);
	mutex_init(&a->lock);
	INIT_DELAYED_WORK(&a->monitor, speaker_monitor);
	a->volume = 100;
	a->reason = "ok";
	mca = of_parse_phandle(dev->of_node, "apple,mca", 0);
	if (!mca)
		return -EINVAL;
	ret = devm_add_action_or_reset(dev, speaker_put_node, mca);
	if (ret)
		return ret;
	codec = of_parse_phandle(dev->of_node, "apple,codec", 0);
	if (!codec)
		return -EINVAL;
	ret = devm_add_action_or_reset(dev, speaker_put_node, codec);
	if (ret)
		return ret;

	a->cpus[0] = (struct snd_soc_dai_link_component){ .of_node = mca, .dai_name = "mca-pcm-0" };
	a->cpus[1] = (struct snd_soc_dai_link_component){ .of_node = mca, .dai_name = "mca-i2s-0" };
	a->codecs[0] = (struct snd_soc_dai_link_component){ .name = "snd-soc-dummy", .dai_name = "snd-soc-dummy-dai" };
	a->codecs[1] = (struct snd_soc_dai_link_component){ .of_node = codec, .dai_name = "tas2764 ASI1" };
	a->platforms[0].of_node = mca;
	a->platforms[1].of_node = dev->of_node;
	a->links[0] = (struct snd_soc_dai_link){
		.name = "Internal Speaker", .stream_name = "Internal Speaker",
		.dynamic = 1, .playback_only = 1, .nonatomic = 1,
		.dpcm_merged_rate = 1, .dpcm_merged_chan = 1, .dpcm_merged_format = 1,
		.dai_fmt = SPEAKER_FMT, .init = speaker_fe_init,
		.cpus = &a->cpus[0], .num_cpus = 1,
		.codecs = &a->codecs[0], .num_codecs = 1,
		.platforms = a->platforms, .num_platforms = 2,
	};
	a->links[1] = (struct snd_soc_dai_link){
		.name = "Speaker Amplifier", .stream_name = "Speaker Amplifier",
		.no_pcm = 1, .playback_only = 1,
		.dai_fmt = SPEAKER_FMT, .init = speaker_be_init,
		.cpus = &a->cpus[1], .num_cpus = 1,
		.codecs = &a->codecs[1], .num_codecs = 1,
	};
	a->card = (struct snd_soc_card){
		.name = "T8132Speaker", .driver_name = "T8132Speaker", .owner = THIS_MODULE,
		.dev = dev, .dai_link = a->links, .num_links = 2,
		.fixup_controls = speaker_fixup_controls,
		.component_chaining = true, .fully_routed = true,
		.controls = speaker_controls, .num_controls = ARRAY_SIZE(speaker_controls),
		.dapm_widgets = speaker_widgets, .num_dapm_widgets = ARRAY_SIZE(speaker_widgets),
		.dapm_routes = speaker_routes, .num_dapm_routes = ARRAY_SIZE(speaker_routes),
	};
	snd_soc_card_set_drvdata(&a->card, a);
	ret = devm_snd_soc_register_component(dev, &speaker_component, NULL, 0);
	if (ret)
		return ret;
	return devm_snd_soc_register_card(dev, &a->card);
}

static const struct of_device_id speaker_match[] = {
	{ .compatible = "apple,j773g-speaker" },
	{}
};
MODULE_DEVICE_TABLE(of, speaker_match);

static struct platform_driver speaker_driver = {
	.probe = speaker_probe,
	.driver = {
		.name = "j773g-speaker",
		.of_match_table = speaker_match,
		.dev_groups = speaker_groups,
	},
};
module_platform_driver(speaker_driver);
MODULE_LICENSE("GPL");
MODULE_SOFTDEP("pre: snd-soc-apple-mca");
MODULE_VERSION("0.8-kernel-thermal");
MODULE_DESCRIPTION("J773g internal speaker on Apple MCA and TAS2764 with in-kernel thermal limiting");
