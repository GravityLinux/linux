/* SPDX-License-Identifier: GPL-2.0-only OR MIT */
#ifndef APPLE_PMP_VOTE_POLICY_H
#define APPLE_PMP_VOTE_POLICY_H

/* Pure policy shared by the driver and its standalone host test. */
struct apple_pmp_vote_policy {
	unsigned long long last_busy_ms;
	int busy_seen;
};

static inline int apple_pmp_vote_valid(unsigned int value)
{
	return (value & 0xf) <= 5 && (value >> 4) <= 3;
}

static inline unsigned int
apple_pmp_vote_decide(struct apple_pmp_vote_policy *policy,
		      unsigned long long now_ms, unsigned int peak,
		     unsigned int threshold, unsigned int hold_ms,
		     unsigned int value)
{
	if (peak >= threshold) {
		policy->last_busy_ms = now_ms;
		policy->busy_seen = 1;
	}
	return policy->busy_seen && now_ms - policy->last_busy_ms < hold_ms ? value : 0;
}
#endif
