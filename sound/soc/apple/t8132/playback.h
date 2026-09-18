/* SPDX-License-Identifier: GPL-2.0-only */
struct snd_soc_component;
#define AUDIO_AMP_MUTE 200 /* -100 dB, the TAS2764 digital volume floor */
int audio_amp_bind(struct snd_soc_component *component);
int audio_amp_check_fault(void);
int audio_amp_set_dvc(unsigned int dvc);
int audio_amp_read_dvc(void);
