/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * Playback progress accounting for the protected ring, in frames and
 * nanoseconds only, so the userspace regression test can drive it.
 *
 * Held: wall-clock frames the DMA fetch position has not covered. The
 * high-water mark follows the deficit down within a small band, so progress
 * running ahead of the clock is never banked as credit against a later
 * stall. Replay: the fetch position passing the application pointer; each
 * fetched frame is charged at most once. A stall is a deficit that grows by
 * the fault threshold within the history window, which a slow clock cannot.
 */
#ifndef T8132_PROGRESS_H
#define T8132_PROGRESS_H
#ifdef __KERNEL__
#include <linux/types.h>
#include <linux/math64.h>
#include <linux/minmax.h>
#include <linux/string.h>
#endif

#define PROGRESS_RATE 48000
#define PROGRESS_BAND 256   /* frames of burst granularity and start latency */
#define PROGRESS_HISTORY 6  /* monitor ticks of deficit history: at least 100 ms even with fast ticks */

struct progress {
	u64 start_ns, fine_start, replay_mark;
	s64 deficit, high;
	s64 history[PROGRESS_HISTORY];
	unsigned int tick;
};

struct progress_charge {
	u64 held, replay;     /* frames to charge at the ring peak */
};

static inline void progress_start(struct progress *p, u64 now, u64 fine)
{
	memset(p, 0, sizeof(*p));
	p->start_ns = now;
	p->fine_start = fine;
	p->high = PROGRESS_BAND;
}

static inline struct progress_charge progress_account(struct progress *p, u64 now, u64 fine,
						      u64 appl, unsigned int fifo)
{
	struct progress_charge c = { 0 };
	u64 expected = mul_u64_u64_div_u64(now - p->start_ns, PROGRESS_RATE, 1000000000ULL);
	u64 mark = max(appl, p->replay_mark);

	p->deficit = (s64)expected - (s64)(fine - p->fine_start);
	if (p->deficit > p->high)
		c.held = p->deficit - p->high;
	p->high = clamp(p->high, p->deficit, p->deficit + PROGRESS_BAND);
	if (fine + fifo > mark) {
		c.replay = fine + fifo - mark;
		p->replay_mark = fine + fifo;
	}
	return c;
}

/* Once per monitor tick, after progress_account(). */
static inline bool progress_tick(struct progress *p, unsigned int fault_frames)
{
	unsigned int slot = p->tick++ % PROGRESS_HISTORY;
	s64 oldest = p->history[slot];

	p->history[slot] = p->deficit;
	return p->deficit - oldest >= (s64)fault_frames;
}
#endif
