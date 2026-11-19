/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * Two-node voice-coil thermal model and limiter in integer arithmetic.
 *
 * Heat arrives as sums of squares of normalized S24 samples that are about
 * to enter the DMA ring, so every joule is charged before it can play and
 * the model leads the speaker. Charged heat therefore starts cooling up to
 * one delivery window early; the delivery scale folded into each charge
 * makes that early cooling conservative for both nodes.
 *
 * Temperatures are micro-degrees Celsius. A sum-of-squares unit is 2^-46 of
 * one full-scale sample. Only math64 helpers are used so that the userspace
 * regression test can include this header unchanged.
 */
#ifndef T8132_THERMAL_H
#define T8132_THERMAL_H
#ifdef __KERNEL__
#include <linux/types.h>
#include <linux/math64.h>
#include <linux/minmax.h>
#include <linux/string.h>
#include <linux/errno.h>
#include <linux/limits.h>
#endif

#define THERMAL_MUTE 200 /* half-dB steps of attenuation; 200 writes silence */
#define THERMAL_RATE 48000
#define THERMAL_SUBSTEP_NS (250ULL * 1000 * 1000)

struct thermal_params {
	/* Electrical worst case at DVC 0 and the hardware ceiling in half-dB. */
	unsigned int fs_mv, r_mohm, margin_pct, ceiling;
	/* Two-node model in m°C/W, ms and m°C. */
	unsigned int r_coil, r_magnet, tau_coil_ms, tau_magnet_ms;
	/* t_hard bounds heat that will certainly play; t_stall additionally
	 * bounds a sample held for the stall window before the driver mutes. */
	unsigned int t_ambient, t_limit, t_window, t_hysteresis, t_hard, t_stall;
	/* Latest a charged sample can play, and how long a held sample can
	 * keep playing before the driver has muted the amplifier. */
	unsigned int delivery_ns, stall_frames;
};

struct thermal {
	u64 att[THERMAL_MUTE + 1]; /* amplitude per half-dB step, Q32 */
	u64 k_coil, k_magnet;	   /* µ°C per sum-of-squares unit, Q60 */
	u64 tau_coil_ns, tau_magnet_ns;
	s64 t_ambient, t_limit, t_window, t_hysteresis, t_hard, t_stall;
	unsigned int att_min; /* steady-state attenuation at t_limit */
	unsigned int stall_frames;
	unsigned int p_ceiling_mw, p_max_mw;
	/* State: coil >= magnet >= t_ambient always holds. */
	s64 coil, magnet, coil_hyst, magnet_hyst;
	u64 last_ns;
	unsigned int attenuation;
};

static inline u64 thermal_pow(const struct thermal *t, unsigned int a)
{
	return mul_u64_u64_shr(t->att[a], t->att[a], 32);
}

/* e^x for 0 <= x < 1 in Q32, never below the true value: the tail of the
 * series after x^3/6 is smaller than x^4/12. */
static inline u64 thermal_exp_q32(u64 x)
{
	u64 x2 = mul_u64_u64_shr(x, x, 32);
	u64 x3 = mul_u64_u64_shr(x2, x, 32);
	u64 x4 = mul_u64_u64_shr(x3, x, 32);

	return (1ULL << 32) + x + x2 / 2 + x3 / 6 + x4 / 12 + 4;
}

static inline int thermal_init(struct thermal *t, const struct thermal_params *p, u64 now_ns)
{
	const u64 step = 4054710590ULL; /* ceil(10^(-1/40) * 2^32) */
	u64 p_full, p_ceiling, scale, rise;
	unsigned int a;

	memset(t, 0, sizeof(*t));
	if (!p->fs_mv || !p->r_mohm || p->margin_pct < 100 || p->ceiling > THERMAL_MUTE ||
	    !p->r_coil || !p->r_magnet || !p->tau_coil_ms || p->tau_coil_ms >= p->tau_magnet_ms ||
	    (u64)p->tau_coil_ms * 1000000 < THERMAL_SUBSTEP_NS || !p->t_window ||
	    p->t_limit <= p->t_ambient + p->t_window || p->t_hard < p->t_limit ||
	    p->t_stall < p->t_hard ||
	    (u64)p->delivery_ns * 2 >= (u64)p->tau_coil_ms * 1000000 || !p->stall_frames)
		return -EINVAL;

	t->att[0] = 1ULL << 32;
	for (a = 1; a < THERMAL_MUTE; a++)
		t->att[a] = mul_u64_u64_shr(t->att[a - 1], step, 32) + 1;
	t->att[THERMAL_MUTE] = 0;

	/* Milliwatts at full scale and at the ceiling, rounded up. */
	p_full = div64_u64((u64)p->fs_mv * p->fs_mv * p->margin_pct, (u64)p->r_mohm * 100) + 1;
	p_ceiling = mul_u64_u64_shr(p_full, thermal_pow(t, p->ceiling), 32) + 1;
	if (p_ceiling > U32_MAX)
		return -EINVAL;
	t->p_ceiling_mw = p_ceiling;
	t->tau_coil_ns = (u64)p->tau_coil_ms * 1000000;
	t->tau_magnet_ns = (u64)p->tau_magnet_ms * 1000000;

	/* One full-scale sample raises a node by P_mW * R / (rate * tau_ms)
	 * milli-degrees; a sum-of-squares unit is 2^-46 of a sample, so the
	 * Q60 coefficient per unit is that value * 1000 * 2^14. */
	scale = thermal_exp_q32(div64_u64((u64)p->delivery_ns << 32, t->tau_coil_ns));
	rise = mul_u64_u64_div_u64(p_ceiling * p->r_coil, 1000ULL << 14,
				   (u64)THERMAL_RATE * p->tau_coil_ms);
	t->k_coil = mul_u64_u64_shr(rise, scale, 32) + 1;
	scale = thermal_exp_q32(div64_u64((u64)p->delivery_ns << 32, t->tau_magnet_ns));
	rise = mul_u64_u64_div_u64(p_ceiling * p->r_magnet, 1000ULL << 14,
				   (u64)THERMAL_RATE * p->tau_magnet_ms);
	t->k_magnet = mul_u64_u64_shr(rise, scale, 32) + 1;
	if (t->k_coil < t->k_magnet)
		return -EINVAL;

	t->t_ambient = (s64)p->t_ambient * 1000;
	t->t_limit = (s64)p->t_limit * 1000;
	t->t_window = (s64)p->t_window * 1000;
	t->t_hysteresis = (s64)p->t_hysteresis * 1000;
	t->t_hard = (s64)p->t_hard * 1000;
	t->t_stall = (s64)p->t_stall * 1000;
	t->stall_frames = p->stall_frames;

	/* Continuous power that settles exactly at t_limit, and the smallest
	 * attenuation that keeps a full-scale square wave below it. */
	t->p_max_mw = div64_u64((u64)(p->t_limit - p->t_ambient) * 1000, p->r_coil + p->r_magnet);
	for (a = 0; a < THERMAL_MUTE; a++)
		if (mul_u64_u64_shr(p_ceiling, thermal_pow(t, a), 32) <= t->p_max_mw)
			break;
	t->att_min = a;

	/* Start warm but below the ramp, so a hot reboot cannot add much. */
	t->coil = t->t_limit - t->t_window - 1000000;
	t->magnet = t->t_ambient + div64_u64((u64)(t->coil - t->t_ambient) * p->r_magnet,
					     p->r_coil + p->r_magnet);
	t->coil_hyst = t->coil;
	t->magnet_hyst = t->magnet;
	t->last_ns = now_ns;
	return 0;
}

/* Relax both nodes toward their targets for the time since the last call. */
static inline void thermal_advance(struct thermal *t, u64 now_ns)
{
	u64 dt = now_ns > t->last_ns ? now_ns - t->last_ns : 0;

	t->last_ns = now_ns;
	if (dt > 64 * t->tau_magnet_ns) {
		t->coil = t->t_ambient;
		t->magnet = t->t_ambient;
		return;
	}
	while (dt) {
		u64 step = min(dt, THERMAL_SUBSTEP_NS);
		u64 hc = div64_u64(step << 32, t->tau_coil_ns);
		u64 hm = div64_u64(step << 32, t->tau_magnet_ns);

		/* 1 - h + h^2/2 >= e^-h, so relaxing by h - h^2/2 never cools
		 * faster than the exponential. The coil relaxes toward the old
		 * magnet temperature, which keeps coil >= magnet. */
		hc -= mul_u64_u64_shr(hc, hc, 33);
		hm -= mul_u64_u64_shr(hm, hm, 33);
		t->coil -= mul_u64_u64_shr(t->coil - t->magnet, hc, 32);
		t->magnet -= mul_u64_u64_shr(t->magnet - t->t_ambient, hm, 32);
		dt -= step;
	}
}

/* Charge heat that will play at the hardware ceiling, as an impulse. */
static inline void thermal_charge(struct thermal *t, u64 sumsq)
{
	t->coil += mul_u64_u64_shr(sumsq, t->k_coil, 60) + 1;
	t->magnet += mul_u64_u64_shr(sumsq, t->k_magnet, 60) + 1;
}

/* Coil rise from sum-of-squares units, µ°C. */
static inline s64 thermal_rise(const struct thermal *t, u64 sumsq)
{
	return mul_u64_u64_shr(sumsq, t->k_coil, 60) + 1;
}

/* Whether raw written at attenuation a fits: its certain heat plus committed
 * heat under t_hard, and additionally the larger of its own peak or the
 * ring's peak held for the stall window under t_stall. */
static inline bool thermal_fits(const struct thermal *t, u64 raw, u64 held, u64 ring_held,
				u64 committed, unsigned int a)
{
	u64 p = thermal_pow(t, a);
	u64 certain = mul_u64_u64_shr(raw, p, 32) + committed;
	u64 conditional = max(mul_u64_u64_shr(held, p, 32), ring_held);

	return t->coil + thermal_rise(t, certain) <= t->t_hard &&
	       t->coil + thermal_rise(t, certain + conditional) <= t->t_stall;
}

/* Choose the attenuation for raw about to be written: the upstream ramp for
 * steady state, and the smallest step that keeps a burst within bounds. */
static inline unsigned int thermal_admit(struct thermal *t, u64 raw, u32 raw_peak,
					 u32 ring_peak, u64 committed)
{
	u64 held = (u64)raw_peak * raw_peak * t->stall_frames;
	u64 ring_held = (u64)ring_peak * ring_peak * t->stall_frames;
	s64 temp;
	unsigned int a, lo = 0, hi = THERMAL_MUTE;

	t->coil_hyst = clamp(t->coil_hyst, t->coil, t->coil + t->t_hysteresis);
	t->magnet_hyst = clamp(t->magnet_hyst, t->magnet, t->magnet + t->t_hysteresis);
	temp = max(t->coil_hyst, t->magnet_hyst);
	if (temp <= t->t_limit - t->t_window) {
		a = 0;
	} else if (temp >= t->t_limit) {
		a = t->att_min;
	} else {
		u64 red = div64_u64((u64)(temp - (t->t_limit - t->t_window)) << 32, t->t_window);

		a = (t->att_min * red + (1ULL << 32) - 1) >> 32;
	}
	while (lo < hi) {
		unsigned int mid = lo + (hi - lo) / 2;

		if (thermal_fits(t, raw, held, ring_held, committed, mid))
			hi = mid;
		else
			lo = mid + 1;
	}
	t->attenuation = max(a, lo);
	return t->attenuation;
}
#endif
