// SPDX-License-Identifier: GPL-2.0-only
/* J713 headphone-only routing. No speaker links or amplifier dependencies. */
#include <linux/module.h>
#include <linux/of_platform.h>
#include <sound/jack.h>
#include <sound/pcm_params.h>
#include <sound/soc.h>

#define J713_HEADPHONE_FMT (SND_SOC_DAIFMT_I2S | SND_SOC_DAIFMT_IB_IF | \
			    SND_SOC_DAIFMT_CBC_CFC)

struct j713_headphones {
	struct snd_soc_card card;
	struct snd_soc_dai_link links[2];
	struct snd_soc_dai_link_component cpus[2];
	struct snd_soc_dai_link_component codecs[2];
	struct snd_soc_dai_link_component platforms[2];
	struct snd_soc_jack jack;
	struct snd_soc_jack_pin jack_pin;
};

static int j713_headphone_fe_init(struct snd_soc_pcm_runtime *rtd)
{
	struct snd_soc_dai *dai = snd_soc_rtd_to_cpu(rtd, 0);
	int ret;

	ret = snd_soc_dai_set_tdm_slot(dai, 3, 0, 2, 32);
	if (ret)
		return ret;
	return snd_soc_dai_set_bclk_ratio(dai, 250);
}

static int j713_headphone_startup(struct snd_pcm_substream *substream)
{
	struct snd_pcm_runtime *runtime = substream->runtime;
	int ret;

	/* Preserve the qualified T8132/CS42L84 transport format. */
	ret = snd_pcm_hw_constraint_single(runtime, SNDRV_PCM_HW_PARAM_RATE, 48000);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_constraint_single(runtime, SNDRV_PCM_HW_PARAM_CHANNELS, 2);
	if (ret < 0)
		return ret;
	return snd_pcm_hw_constraint_mask64(runtime, SNDRV_PCM_HW_PARAM_FORMAT,
					   SNDRV_PCM_FMTBIT_S24_LE);
}

static const struct snd_soc_ops j713_headphone_ops = {
	.startup = j713_headphone_startup,
};

static int j713_headphone_be_init(struct snd_soc_pcm_runtime *rtd)
{
	struct j713_headphones *a = snd_soc_card_get_drvdata(rtd->card);
	struct snd_soc_dai *dai = snd_soc_rtd_to_codec(rtd, 0);
	int ret;

	ret = snd_soc_dai_set_sysclk(dai, 0, 12000000, SND_SOC_CLOCK_IN);
	if (ret)
		return ret;
	ret = snd_soc_dai_set_tdm_slot(dai, 0, 3, 2, 32);
	if (ret)
		return ret;
	a->jack_pin = (struct snd_soc_jack_pin) {
		.pin = "Headphone", .mask = SND_JACK_HEADPHONE,
	};
	ret = snd_soc_card_jack_new_pins(rtd->card, "Headphone Jack",
				       SND_JACK_HEADPHONE, &a->jack,
				       &a->jack_pin, 1);
	if (ret)
		return ret;
	return snd_soc_component_set_jack(dai->component, &a->jack, NULL);
}

static void j713_headphone_be_exit(struct snd_soc_pcm_runtime *rtd)
{
	snd_soc_component_set_jack(snd_soc_rtd_to_codec(rtd, 0)->component, NULL, NULL);
}

static const struct snd_soc_dapm_widget j713_headphone_widgets[] = {
	SND_SOC_DAPM_HP("Headphone", NULL),
};

static const struct snd_soc_dapm_route j713_headphone_routes[] = {
	{ "I2S2 TX", NULL, "PCM2 TX" },
	{ "Headphone", NULL, "Jack HP" },
};

static void j713_headphone_put_node(void *node)
{
	of_node_put(node);
}

/* Card construction requests the DMA channel, without opening a PCM.
 * ASoC alone cannot retry a parked card when a late DMA controller binds.
 */
static int j713_headphone_wait_for_dma(struct device *dev, struct device_node *mca)
{
	struct device_node *np = of_parse_phandle(mca, "dmas", 0);
	struct platform_device *dma;
	bool bound;

	if (!np)
		return -EINVAL;
	dma = of_find_device_by_node(np);
	of_node_put(np);
	if (!dma)
		return dev_err_probe(dev, -EPROBE_DEFER, "waiting for audio DMA\n");
	bound = device_is_bound(&dma->dev);
	put_device(&dma->dev);
	return bound ? 0 : dev_err_probe(dev, -EPROBE_DEFER, "waiting for audio DMA\n");
}

/* Use the FE's explicit link ID as its userspace PCM device number. This
 * reserves devices 0/1 without registering dummy speaker/capture PCMs.
 */
static const struct snd_soc_component_driver j713_headphone_component = {
	.name = "j713-headphones",
	.use_dai_pcm_id = true,
};

static int j713_headphone_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct j713_headphones *a;
	struct device_node *mca, *codec;
	int ret;

	a = devm_kzalloc(dev, sizeof(*a), GFP_KERNEL);
	if (!a)
		return -ENOMEM;
	mca = of_parse_phandle(dev->of_node, "apple,mca", 0);
	if (!mca)
		return -EINVAL;
	ret = devm_add_action_or_reset(dev, j713_headphone_put_node, mca);
	if (ret)
		return ret;
	ret = j713_headphone_wait_for_dma(dev, mca);
	if (ret)
		return ret;
	codec = of_parse_phandle(dev->of_node, "apple,headphone-codec", 0);
	if (!codec)
		return -EINVAL;
	ret = devm_add_action_or_reset(dev, j713_headphone_put_node, codec);
	if (ret)
		return ret;

	a->cpus[0] = (struct snd_soc_dai_link_component) {
		.of_node = mca, .dai_name = "mca-pcm-2",
	};
	a->cpus[1] = (struct snd_soc_dai_link_component) {
		.of_node = mca, .dai_name = "mca-i2s-2",
	};
	a->codecs[0] = (struct snd_soc_dai_link_component) {
		.name = "snd-soc-dummy", .dai_name = "snd-soc-dummy-dai",
	};
	a->codecs[1] = (struct snd_soc_dai_link_component) {
		.of_node = codec, .dai_name = "cs42l84",
	};
	a->platforms[0].of_node = mca;
	a->platforms[1].of_node = dev->of_node;
	a->links[0] = (struct snd_soc_dai_link) {
		.id = 2, .name = "Headphones", .stream_name = "Headphones",
		.dynamic = 1, .playback_only = 1, .nonatomic = 1,
		.dpcm_merged_rate = 1, .dpcm_merged_chan = 1, .dpcm_merged_format = 1,
		.dai_fmt = J713_HEADPHONE_FMT, .init = j713_headphone_fe_init,
		.ops = &j713_headphone_ops,
		.cpus = &a->cpus[0], .num_cpus = 1,
		.codecs = &a->codecs[0], .num_codecs = 1,
		.platforms = a->platforms, .num_platforms = ARRAY_SIZE(a->platforms),
	};
	a->links[1] = (struct snd_soc_dai_link) {
		.id = 3, .name = "Headphone Codec", .stream_name = "Headphone Codec",
		.no_pcm = 1, .playback_only = 1, .dai_fmt = J713_HEADPHONE_FMT,
		.init = j713_headphone_be_init, .exit = j713_headphone_be_exit,
		.cpus = &a->cpus[1], .num_cpus = 1,
		.codecs = &a->codecs[1], .num_codecs = 1,
	};
	a->card = (struct snd_soc_card) {
		.name = "AppleJ713", .driver_name = "AppleJ713", .owner = THIS_MODULE,
		.dev = dev, .dai_link = a->links, .num_links = ARRAY_SIZE(a->links),
		.component_chaining = true, .fully_routed = true,
		.dapm_widgets = j713_headphone_widgets,
		.num_dapm_widgets = ARRAY_SIZE(j713_headphone_widgets),
		.dapm_routes = j713_headphone_routes,
		.num_dapm_routes = ARRAY_SIZE(j713_headphone_routes),
	};
	snd_soc_card_set_drvdata(&a->card, a);
	platform_set_drvdata(pdev, a);
	ret = devm_snd_soc_register_component(dev, &j713_headphone_component, NULL, 0);
	if (ret)
		return ret;
	return devm_snd_soc_register_card(dev, &a->card);
}

static const struct of_device_id j713_headphone_match[] = {
	{ .compatible = "apple,j713-headphones" },
	{}
};
MODULE_DEVICE_TABLE(of, j713_headphone_match);

static struct platform_driver j713_headphone_driver = {
	.probe = j713_headphone_probe,
	.driver = {
		.name = "j713-headphones",
		.of_match_table = j713_headphone_match,
	},
};
module_platform_driver(j713_headphone_driver);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Apple J713 headphone-only sound card");
