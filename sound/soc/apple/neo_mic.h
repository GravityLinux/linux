/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef NEO_MIC_H
#define NEO_MIC_H
#include <linux/device.h>
struct neo_mic;
struct neo_mic *neo_mic_get(struct device *dev);
int neo_mic_power(struct neo_mic *mic, u32 state, u32 active);
int neo_audio_set(struct neo_mic *mic, u32 device, u32 property,
		  const void *payload, size_t length);
int neo_audio_power(struct neo_mic *mic, u32 device, u32 state, u32 active);
#define NEO_SPEAKER_DEVICE 0x73706b72
#define NEO_MIC_WARM 0x70773020 /* pw0 : observed Neo intermediate state */
#define NEO_MIC_ON 0x70777264 /* pwrd */
#define NEO_MIC_OFF 0x69646c65 /* idle */
bool neo_audio_healthy(struct neo_mic *mic);
#endif
