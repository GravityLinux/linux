/* SPDX-License-Identifier: GPL-2.0-only OR MIT */
#ifndef APPLE_PMP_VOTE_POLICY_H
#define APPLE_PMP_VOTE_POLICY_H

/* Activity sources share one release deadline and one vote output. */
#define APPLE_PMP_VOTE_CPU (1U << 0)
#define APPLE_PMP_VOTE_GPU (1U << 1)

struct apple_pmp_vote_policy {
	unsigned long long last_busy_ms;
	int busy_seen;
};

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
