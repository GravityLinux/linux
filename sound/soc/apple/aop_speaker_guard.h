/* SPDX-License-Identifier: GPL-2.0-only OR MIT */
#ifndef APPLE_AOP_SPEAKER_GUARD_H
#define APPLE_AOP_SPEAKER_GUARD_H
/* ALSA S32 -> native low-24-bit integer samples in 32-bit words.
 * Hardware DVC supplies the immutable -60dB ceiling; no IEEE floats enter DMA.
 */
static inline s32 aop_speaker_sample(s32 sample)
{
	return sample >> 8;
}

static inline u32 aop_speaker_dma_word(s32 sample)
{
	return (u32)sample & 0xffffff;
}
#endif
