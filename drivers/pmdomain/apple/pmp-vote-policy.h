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

/* Busy-time clock: low bit is active; upper bits hold total minus time
 * while active, total while idle. One scalar CAS publishes each transition.
 * Repeated in-flight counts do not restart the occupied interval.
 */
static inline unsigned long long
apple_pmp_vote_gpu_clock(unsigned long long old, unsigned long long now_ns,
			 int active)
{
	if (!!(old & 1) == !!active)
		return old;
	return active ? (old - (now_ns << 1)) | 1 :
			(old & ~1ULL) + (now_ns << 1);
}

struct apple_pmp_vote_run {
	unsigned long long last_ms;
	unsigned int count;
};

/* A deferred timer gap is not evidence of consecutive occupied samples. */
static inline int
apple_pmp_vote_sustained(struct apple_pmp_vote_run *run, int busy,
			 unsigned long long now_ms, unsigned int required)
{
	if (!busy || (run->count && now_ms - run->last_ms > 40))
		run->count = 0;
	if (busy && run->count < 255)
		run->count++;
	run->last_ms = now_ms;
	return busy && run->count >= required;
}

#define APPLE_PMP_GPU_HISTORY 32
struct apple_pmp_vote_gpu_window {
	unsigned long long ns[APPLE_PMP_GPU_HISTORY];
	unsigned long long busy[APPLE_PMP_GPU_HISTORY];
	unsigned int head, count, window_ms, percent;
};

/* Rolling union of in-flight GPU intervals, never a sum of overlapping jobs.
 * Actual elapsed time is used: deferrable timers need not fire exactly on time.
 * History after a long idle gap is discarded, rather than treating that gap
 * as sustained demand. A new window must fill before it can request a vote.
 */
static inline unsigned int
apple_pmp_vote_gpu_fraction(struct apple_pmp_vote_gpu_window *w,
			    unsigned long long now, unsigned long long busy,
			    unsigned int window_ms)
{
	unsigned long long target = window_ms * 1000000ULL;
	unsigned int j, latest = (w->head + APPLE_PMP_GPU_HISTORY - 1) %
				APPLE_PMP_GPU_HISTORY;

	if (window_ms != w->window_ms ||
	    (w->count && (now < w->ns[latest] || busy < w->busy[latest] ||
			 now - w->ns[latest] > 2 * target))) {
		*w = (struct apple_pmp_vote_gpu_window){0};
		w->window_ms = window_ms;
	}
	if (w->count && now - w->ns[latest] < 20000000ULL)
		return w->percent;
	w->percent = 0;
	/* Newest point at least one full window old; error < one sample. */
	for (j = 0; j < w->count; j++) {
		unsigned int n = (w->head + APPLE_PMP_GPU_HISTORY - 1 - j) %
				 APPLE_PMP_GPU_HISTORY;
		unsigned long long dt = now - w->ns[n];
		unsigned long long db = busy - w->busy[n];

		if (dt >= target) {
			if (db > dt)
				db = dt;
			w->percent = 100 * db / dt;
			break;
		}
	}
	w->ns[w->head] = now;
	w->busy[w->head] = busy;
	w->head = (w->head + 1) % APPLE_PMP_GPU_HISTORY;
	if (w->count < APPLE_PMP_GPU_HISTORY)
		w->count++;
	return w->percent;
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
