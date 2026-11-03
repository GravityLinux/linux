/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * In a 4K process, the four 4K slices of a 16K window share one 16K folio.
 * When one slice is zapped (MADV_DONTNEED, munmap(), mremap() away) while its
 * siblings keep the folio alive, the next fault on that slice must still
 * see zero-filled memory, and must not hand out a slice something else still
 * uses (a vmsplice() pipe buffer, the page that mremap() moved away).
 *
 * Every case fills all four slices of NR_WIN windows with a known pattern,
 * zaps one target slice, refaults it and checks the result:
 *
 *   write      write-fault one byte, the rest of the slice must be zero
 *   read       read-fault (zero page), then write: zero before and after
 *   threads    4 threads, one per slice of the same windows, looping
 *   unmap1     another slice of the window munmap()ed first
 *   only       all three siblings munmap()ed first
 *   holes      two siblings MADV_DONTNEED, one sibling still mapped
 *   mremap     target moved away with mremap() and the hole mapped again;
 *              refault the hole: zero, and the moved page keeps its data and
 *              does not alias the hole
 *   dontunmap  the same with MREMAP_DONTUNMAP, which leaves the hole mapped
 *   swap       window MADV_PAGEOUTed and slice 0 swapped back in before the
 *              zap (needs swap; skipped otherwise)
 *
 * With --vmsplice, also:
 *
 *   splice     control: target spliced into a pipe and read back
 *   vmsplice   target spliced into a pipe first; refault and write must not
 *              change what the pipe reads
 *
 * The vmsplice() cases are opt-in because reading back a pipe buffer spliced
 * from a 4K process oopses some kernels in copy_page_to_iter(), independently
 * of sibling reuse.
 *
 * Exit 0 = PASS, 1 = FAIL, 2 = setup error, 4 = SKIP.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/mman.h>
#include <sys/personality.h>
#include <sys/uio.h>
#include <unistd.h>

#ifndef MREMAP_DONTUNMAP
#define MREMAP_DONTUNMAP 4
#endif

#ifndef MADV_PAGEOUT
#define MADV_PAGEOUT 21
#endif

#define ADDR_PAGE_SIZE_4KB 0x40000000UL
#define SL 4096UL
#define WIN (4 * SL)
#define NR_WIN 256
#define THREAD_LOOPS 2000
#define THREAD_WIN 64
#define MARK_OFF 123
#define KSFT_SKIP 4

static int setup_err;
static int printed;

/* At most a few lines per case, so a broken kernel does not flood the log. */
#define report_line(...) do { if (printed++ < 3) printf(__VA_ARGS__); } while (0)

static uint8_t pat(int s)
{
	return 0x40 + s;
}

static uint8_t *alloc_windows(size_t nr, uint8_t **raw)
{
	*raw = mmap(NULL, nr * WIN + WIN, PROT_READ | PROT_WRITE,
		    MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	if (*raw == MAP_FAILED) {
		setup_err = 1;
		return NULL;
	}
	return (uint8_t *)(((uintptr_t)*raw + WIN - 1) & ~(WIN - 1));
}

/* Slice 0 first, so the other slices share its folio. */
static void fill(uint8_t *w)
{
	for (int s = 0; s < 4; s++)
		memset(w + s * SL, pat(s), SL);
}

/* Count bytes of @p that are not @v, skipping @skip (or -1). */
static size_t bad_bytes(const uint8_t *p, uint8_t v, long skip)
{
	size_t n = 0;

	for (size_t i = 0; i < SL; i++)
		if ((long)i != skip && p[i] != v)
			n++;
	return n;
}

/* Check the target slice (zeros + optional mark) and the mapped siblings. */
static int check_window(const char *c, size_t wi, uint8_t *w, int k,
			unsigned int sib_mask, int marked)
{
	uint8_t *t = w + k * SL;
	size_t n = bad_bytes(t, 0, marked ? MARK_OFF : -1);
	int fail = 0;

	if (n) {
		report_line("  %s: window %zu slice %d: %zu stale bytes (byte 0 = %02x, expected 00)\n",
		       c, wi, k, n, t[0]);
		fail = 1;
	}
	if (marked && t[MARK_OFF] != 0xe5) {
		report_line("  %s: window %zu slice %d: mark %02x, expected e5\n",
		       c, wi, k, t[MARK_OFF]);
		fail = 1;
	}
	for (int s = 0; s < 4; s++) {
		if (s == k || !(sib_mask & (1u << s)))
			continue;
		n = bad_bytes(w + s * SL, pat(s), -1);
		if (n) {
			report_line("  %s: window %zu sibling %d: %zu bytes changed\n",
			       c, wi, s, n);
			fail = 1;
		}
	}
	return fail;
}

static int case_write(const char *c, int k, int read_first)
{
	uint8_t *raw, *m = alloc_windows(NR_WIN, &raw);
	int fails = 0;

	if (!m)
		return 0;
	for (size_t i = 0; i < NR_WIN; i++)
		fill(m + i * WIN);
	for (size_t i = 0; i < NR_WIN; i++)
		if (madvise(m + i * WIN + k * SL, SL, MADV_DONTNEED))
			setup_err = 1;
	for (size_t i = 0; i < NR_WIN; i++) {
		uint8_t *w = m + i * WIN;

		if (read_first && check_window(c, i, w, k, 0xf, 0)) {
			fails++;
			continue;
		}
		*(volatile uint8_t *)(w + k * SL + MARK_OFF) = 0xe5;
		fails += check_window(c, i, w, k, 0xf, 1);
	}
	munmap(raw, NR_WIN * WIN + WIN);
	return fails;
}

/* Unmap or zap the siblings in @gone_mask before zapping and refaulting k. */
static int case_siblings(const char *c, int k, unsigned int gone_mask, int unmap)
{
	uint8_t *raw, *m = alloc_windows(NR_WIN, &raw);
	unsigned int keep = 0xf & ~gone_mask;
	int fails = 0;

	if (!m)
		return 0;
	for (size_t i = 0; i < NR_WIN; i++)
		fill(m + i * WIN);
	for (size_t i = 0; i < NR_WIN; i++) {
		for (int s = 0; s < 4; s++) {
			if (!(gone_mask & (1u << s)))
				continue;
			if (unmap ? munmap(m + i * WIN + s * SL, SL) :
				    madvise(m + i * WIN + s * SL, SL, MADV_DONTNEED))
				setup_err = 1;
		}
		if (madvise(m + i * WIN + k * SL, SL, MADV_DONTNEED))
			setup_err = 1;
	}
	for (size_t i = 0; i < NR_WIN; i++) {
		uint8_t *w = m + i * WIN;

		*(volatile uint8_t *)(w + k * SL + MARK_OFF) = 0xe5;
		fails += check_window(c, i, w, k, keep, 1);
	}
	munmap(raw, NR_WIN * WIN + WIN);
	return fails;
}

static int case_mremap(const char *c, int k, int dontunmap)
{
	uint8_t *raw, *m = alloc_windows(NR_WIN, &raw);
	uint8_t *draw, *d;
	int fails = 0;

	if (!m)
		return 0;
	/* Destination keeps the same slice offset: no reslicing copy. */
	draw = mmap(NULL, NR_WIN * WIN + WIN, PROT_NONE,
		    MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	if (draw == MAP_FAILED) {
		setup_err = 1;
		return 0;
	}
	d = (uint8_t *)(((uintptr_t)draw + WIN - 1) & ~(WIN - 1));
	for (size_t i = 0; i < NR_WIN; i++)
		fill(m + i * WIN);
	for (size_t i = 0; i < NR_WIN; i++)
		if (mremap(m + i * WIN + k * SL, SL, SL,
			   MREMAP_MAYMOVE | MREMAP_FIXED |
			   (dontunmap ? MREMAP_DONTUNMAP : 0),
			   d + i * WIN + k * SL) == MAP_FAILED) {
			report_line("  %s: mremap: %s\n", c, strerror(errno));
			setup_err = 1;
			goto out;
		}
	for (size_t i = 0; i < NR_WIN; i++) {
		uint8_t *w = m + i * WIN, *moved = d + i * WIN + k * SL;
		size_t n;

		/* Without DONTUNMAP the hole is unmapped: map it back first. */
		if (!dontunmap &&
		    mmap(w + k * SL, SL, PROT_READ | PROT_WRITE,
			 MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0) == MAP_FAILED) {
			setup_err = 1;
			goto out;
		}
		*(volatile uint8_t *)(w + k * SL + MARK_OFF) = 0xe5;
		memset(w + k * SL + 2048, 0xe6, 16);
		n = bad_bytes(moved, pat(k), -1);
		if (n) {
			report_line("  %s: window %zu: moved slice %d changed (%zu bytes, byte %d = %02x): aliases the refaulted hole\n",
			       c, i, k, n, MARK_OFF, moved[MARK_OFF]);
			fails++;
			continue;
		}
		memset(w + k * SL + 2048, 0, 16);
		fails += check_window(c, i, w, k, 0xf, 1);
	}
out:
	munmap(draw, NR_WIN * WIN + WIN);
	munmap(raw, NR_WIN * WIN + WIN);
	return fails;
}

static int case_vmsplice(const char *c, int k, int zap)
{
	uint8_t *raw, *m = alloc_windows(16, &raw);
	uint8_t buf[SL];
	int fails = 0;

	if (!m)
		return 0;
	for (size_t i = 0; i < 16; i++) {
		uint8_t *w = m + i * WIN;
		struct iovec iov = { w + k * SL, SL };
		int p[2];
		size_t n;

		fill(w);
		if (pipe(p)) {
			setup_err = 1;
			break;
		}
		if (vmsplice(p[1], &iov, 1, 0) != (ssize_t)SL) {
			setup_err = 1;
			close(p[0]);
			close(p[1]);
			break;
		}
		if (zap) {
			if (madvise(w + k * SL, SL, MADV_DONTNEED))
				setup_err = 1;
			*(volatile uint8_t *)(w + k * SL + MARK_OFF) = 0xe5;
			fails += check_window(c, i, w, k, 0xf, 1);
		}
		/* A spliced page stays shared with the mapping: write only after a zap. */
		if (zap)
			memset(w + k * SL, 0xee, SL);
		if (read(p[0], buf, SL) != (ssize_t)SL) {
			setup_err = 1;
		} else if ((n = bad_bytes(buf, pat(k), -1))) {
			report_line("  %s: window %zu slice %d: pipe data changed (%zu bytes, first %02x): pinned slice reused\n",
			       c, i, k, n, buf[0]);
			fails++;
		}
		close(p[0]);
		close(p[1]);
	}
	munmap(raw, 16 * WIN + WIN);
	return fails;
}

static int swap_on(void)
{
	char line[256];
	int lines = 0;
	FILE *f = fopen("/proc/swaps", "r");

	if (!f)
		return 0;
	while (fgets(line, sizeof(line), f))
		lines++;
	fclose(f);
	return lines > 1;
}

static int case_swap(const char *c, int k)
{
	uint8_t *raw, *m = alloc_windows(NR_WIN, &raw);
	int fails = 0;

	if (!m)
		return 0;
	for (size_t i = 0; i < NR_WIN; i++)
		fill(m + i * WIN);
	if (madvise(m, NR_WIN * WIN, MADV_PAGEOUT))
		setup_err = 1;
	for (size_t i = 0; i < NR_WIN; i++) {
		uint8_t *w = m + i * WIN;
		int s0 = k ? 0 : 1;

		/* Swap one sibling back in, then zap and refault the target. */
		if (*(volatile uint8_t *)(w + s0 * SL) != pat(s0)) {
			report_line("  %s: window %zu: sibling %d swapped in wrong\n",
			       c, i, s0);
			fails++;
		}
		if (madvise(w + k * SL, SL, MADV_DONTNEED))
			setup_err = 1;
		*(volatile uint8_t *)(w + k * SL + MARK_OFF) = 0xe5;
		fails += check_window(c, i, w, k, 0xf, 1);
	}
	munmap(raw, NR_WIN * WIN + WIN);
	return fails;
}

struct thr {
	uint8_t *m;
	int k;
	int fails;
	pthread_barrier_t *bar;
};

static void *thread_fn(void *arg)
{
	struct thr *t = arg;

	for (int l = 0; l < THREAD_LOOPS && t->fails < 8; l++) {
		pthread_barrier_wait(t->bar);
		for (size_t i = 0; i < THREAD_WIN; i++) {
			uint8_t *s = t->m + i * WIN + t->k * SL;

			memset(s, pat(t->k), SL);
			if (madvise(s, SL, MADV_DONTNEED))
				setup_err = 1;
			*(volatile uint8_t *)(s + MARK_OFF) = 0xe5;
			size_t n = bad_bytes(s, 0, MARK_OFF);

			if (n || s[MARK_OFF] != 0xe5) {
				if (t->fails < 4)
					report_line("  threads: loop %d window %zu slice %d: %zu stale bytes (first %02x)\n",
					       l, i, t->k, n, s[0]);
				t->fails++;
			}
		}
	}
	return NULL;
}

static int case_threads(void)
{
	uint8_t *raw, *m = alloc_windows(THREAD_WIN, &raw);
	pthread_barrier_t bar;
	pthread_t tid[4];
	struct thr t[4];
	int fails = 0;

	if (!m)
		return 0;
	pthread_barrier_init(&bar, NULL, 4);
	for (int k = 0; k < 4; k++) {
		t[k] = (struct thr){ m, k, 0, &bar };
		if (pthread_create(&tid[k], NULL, thread_fn, &t[k]))
			setup_err = 1;
	}
	for (int k = 0; k < 4; k++) {
		pthread_join(tid[k], NULL);
		fails += t[k].fails;
	}
	pthread_barrier_destroy(&bar);
	munmap(raw, THREAD_WIN * WIN + WIN);
	return fails;
}

static int total_fail;

static void report(const char *c, int k, int fails)
{
	printed = 0;
	if (k >= 0)
		printf("%s %s slice %d\n", fails ? "FAIL" : "ok  ", c, k);
	else
		printf("%s %s\n", fails ? "FAIL" : "ok  ", c);
	if (fails)
		total_fail++;
}

static int run(int vmsplice_cases)
{
	int k;

	if (getauxval(AT_PAGESZ) != 4096) {
		fprintf(stderr, "child pages=%lu, expected 4096\n", getauxval(AT_PAGESZ));
		return 2;
	}
	setvbuf(stdout, NULL, _IOLBF, 0);
	for (k = 0; k < 4; k++)
		report("write", k, case_write("write", k, 0));
	for (k = 0; k < 4; k++)
		report("read", k, case_write("read", k, 1));
	report("threads", -1, case_threads());
	for (k = 0; k < 4; k++)
		report("unmap1", k, case_siblings("unmap1", k, 1u << ((k + 2) & 3), 1));
	for (k = 0; k < 4; k++)
		report("only", k, case_siblings("only", k, 0xf & ~(1u << k), 1));
	for (k = 0; k < 4; k++)
		report("holes", k, case_siblings("holes", k,
			(1u << ((k + 1) & 3)) | (1u << ((k + 2) & 3)), 0));
	for (k = 0; k < 4; k++)
		report("mremap", k, case_mremap("mremap", k, 0));
	for (k = 0; k < 4; k++)
		report("dontunmap", k, case_mremap("dontunmap", k, 1));
	if (swap_on()) {
		for (k = 0; k < 4; k++)
			report("swap", k, case_swap("swap", k));
	} else {
		printf("skip swap (no swap device)\n");
	}
	if (vmsplice_cases) {
		for (k = 0; k < 4; k++)
			report("splice", k, case_vmsplice("splice", k, 0));
		for (k = 0; k < 4; k++)
			report("vmsplice", k, case_vmsplice("vmsplice", k, 1));
	} else {
		printf("skip splice, vmsplice (pass --vmsplice)\n");
	}

	if (setup_err) {
		printf("ERROR: setup failed\n");
		return 2;
	}
	if (total_fail) {
		printf("FAIL: %d cases saw stale or aliased data after a sibling refault\n",
		       total_fail);
		return 1;
	}
	printf("PASS: refaulted 4K slices read zeros and stay private\n");
	return 0;
}

int main(int argc, char **argv)
{
	int old;

	if (argc > 1 && !strcmp(argv[1], "--child"))
		return run(argc > 2 && !strcmp(argv[2], "--vmsplice"));

	old = personality(0xffffffffUL);
	if (old == -1 || personality((unsigned int)old | ADDR_PAGE_SIZE_4KB) == -1) {
		perror("personality: request 4K at next exec");
		return 1;
	}
	execl("/proc/self/exe", argv[0], "--child",
	      argc > 1 ? argv[1] : (char *)NULL, (char *)NULL);
	perror("execl");
	return 1;
}
