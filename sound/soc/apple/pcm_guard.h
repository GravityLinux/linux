/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef NEO_PCM_GUARD_H
#define NEO_PCM_GUARD_H
#ifdef __KERNEL__
#include <linux/types.h>
#else
#include <stdint.h>
typedef int32_t s32;
typedef int64_t s64;
typedef uint32_t u32;
#endif

/* Fixed 3/256 gain (-38.622 dB) and independent normalized peak ceiling.
 * TODO: Revise this temporary volume cap once waveform-dependent speaker
 * protection is validated. No mixer/module parameter can raise it today. */
#define NEO_SPEAKER_NUMERATOR 3U
#define NEO_SPEAKER_DIVISOR 256U
#define NEO_SPEAKER_PEAK_Q31 (((1U << 31) / NEO_SPEAKER_DIVISOR) * NEO_SPEAKER_NUMERATOR)

static inline s32 neo_attenuate_s32(s32 input)
{
	s32 result = ((s64)input * NEO_SPEAKER_NUMERATOR) / NEO_SPEAKER_DIVISOR;
	if (result > (s32)NEO_SPEAKER_PEAK_Q31)
		result = NEO_SPEAKER_PEAK_Q31;
	if (result < -(s32)NEO_SPEAKER_PEAK_Q31)
		result = -(s32)NEO_SPEAKER_PEAK_Q31;
	return result;
}

/* Conversion of bounded Q1.31 to the native little-endian float32
 * memory format, rounding magnitude down by at most one Q31 LSB.
 * Integer arithmetic only: no kernel floating-point use. */
static inline u32 neo_q31_float_bits(s32 sample)
{
	s64 value = sample;
	u32 sign = 0, magnitude, leading, exponent;
	if (value < 0) { sign = 0x80000000; value = -value; }
	if (value > NEO_SPEAKER_PEAK_Q31) value = NEO_SPEAKER_PEAK_Q31;
	magnitude = value;
	if (!magnitude) return 0;
	leading = 31 - __builtin_clz(magnitude);
	exponent = 127 + leading - 31;
	magnitude = leading > 23 ? magnitude >> (leading - 23) : magnitude << (23 - leading);
	return sign | (exponent << 23) | (magnitude & 0x7fffff);
}
#endif
