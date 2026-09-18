// SPDX-License-Identifier: GPL-2.0-only
/* Board protection policy; amplifier lifecycle belongs to the TAS2764 codec. */
#include <linux/mutex.h>
#include <sound/soc.h>
#include "../../codecs/tas2764.h"
#include "playback.h"

static DEFINE_MUTEX(amp_lock);
static struct snd_soc_component *amp;

int audio_amp_bind(struct snd_soc_component *component)
{
	amp = component;
	return audio_amp_set_dvc(AUDIO_AMP_MUTE);
}

int audio_amp_set_dvc(unsigned int dvc)
{
	int ret;

	if (dvc > AUDIO_AMP_MUTE)
		return -EINVAL;
	mutex_lock(&amp_lock);
	ret = tas2764_set_protected_attenuation(amp, dvc);
	mutex_unlock(&amp_lock);
	return ret;
}

int audio_amp_read_dvc(void)
{
	int ret;

	mutex_lock(&amp_lock);
	ret = tas2764_get_protected_attenuation(amp);
	mutex_unlock(&amp_lock);
	return ret;
}

int audio_amp_check_fault(void)
{
	return tas2764_check_fault(amp);
}
