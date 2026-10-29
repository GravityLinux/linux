/* SPDX-License-Identifier: GPL-2.0-only OR MIT */
#ifndef APPLE_PMP_VOTE_POLICY_H
#define APPLE_PMP_VOTE_POLICY_H

/*
 * CPM +0x2f0: modify the bit0 request without echoing the independently
 * changing bits27:20 status field. Preserve every other control bit.
 */
static inline unsigned long long
apple_pmp_cpm_control(unsigned long long old, int raised)
{
	return (old & ~((0xffULL << 20) | 1ULL)) | !!raised;
}

/* Activity sources share one release deadline and one vote output. */
#define APPLE_PMP_VOTE_CPU (1U << 0)
#define APPLE_PMP_VOTE_GPU (1U << 1)
#define APPLE_PMP_VOTE_E_CPU (1U << 2)

struct apple_pmp_vote_cpu_time {
	unsigned long long idle, wall;
	int valid;
};

/* Non-idle percent over one counter interval, with reset/hotplug handling. */
static inline unsigned int
apple_pmp_vote_load(struct apple_pmp_vote_cpu_time *time,
		    unsigned long long idle, unsigned long long wall)
{
	unsigned int load = 0;

	if (idle == ~0ULL || wall == ~0ULL) {
		time->valid = 0;
		return 0;
	}
	if (time->valid && wall > time->wall && idle >= time->idle) {
		unsigned long long dt = wall - time->wall;
		unsigned long long di = idle - time->idle;

		if (di > dt)
			di = dt;
		load = 100 * (dt - di) / dt;
	}
	time->idle = idle;
	time->wall = wall;
	time->valid = 1;
	return load;
}

/* Summed per-core percent: 100 is one fully occupied E-core equivalent. */
static inline unsigned int
apple_pmp_vote_e_input(unsigned int load, unsigned int threshold)
{
	return load >= threshold ? APPLE_PMP_VOTE_E_CPU : 0;
}

struct apple_pmp_vote_policy {
	unsigned long long last_busy_ms;
	int busy_seen;
};

/*
 * A short GPU pulse may start and finish between two samples. Keep its
 * timestamp, and never shorten a newer CPU activity hold with an old event.
 */
static inline void
apple_pmp_vote_note_activity(struct apple_pmp_vote_policy *policy,
			    unsigned long long when_ms)
{
	if (when_ms && (!policy->busy_seen || when_ms > policy->last_busy_ms)) {
		policy->last_busy_ms = when_ms;
		policy->busy_seen = 1;
	}
}

static inline int apple_pmp_vote_valid(unsigned int value)
{
	return (value & 0xf) <= 5 && (value >> 4) <= 3;
}

static inline unsigned int
apple_pmp_vote_cpu_input(unsigned int load, unsigned int threshold)
{
	return load >= threshold ? APPLE_PMP_VOTE_CPU : 0;
}

static inline unsigned int
apple_pmp_vote_decide(struct apple_pmp_vote_policy *policy,
		     unsigned long long now_ms, unsigned int inputs,
		     unsigned int hold_ms, unsigned int value)
{
	if (inputs) {
		policy->last_busy_ms = now_ms;
		policy->busy_seen = 1;
	}
	return policy->busy_seen && now_ms - policy->last_busy_ms < hold_ms ? value : 0;
}
#endif
