/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * vmsplice() from a 4K process: page extraction hands back native 16K pages
 * with the slice's offset inside them, and the pipe buffers built from them
 * must carry exactly the bytes the user range holds.
 *
 * Every byte of a few 16K windows gets a value derived from its offset. Each
 * range below is vmsplice()d into a pipe and then:
 *
 *   read     read() back from the pipe and compared;
 *   splice   splice()d from the pipe into a memfd and compared with pread().
 *
 * Ranges: each 4K slice, a whole window, two windows, and odd offsets and
 * lengths, including ones that start or end mid-slice and ones that cross a
 * window boundary, plus a call with several iovecs. Each set runs on fresh
 * windows and on windows where one slice was zapped and refaulted, so that
 * the slices of a window may come from different folios.
 *
 *   to-user  the other direction: bytes written into a pipe are vmsplice()d
 *            out to the same odd ranges, and the bytes around the range must
 *            stay unchanged.
 *
 * Exit 0 = PASS, 1 = FAIL, 2 = setup error.
 */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/mman.h>
#include <sys/personality.h>
#include <sys/uio.h>
#include <unistd.h>

#define ADDR_PAGE_SIZE_4KB 0x40000000UL
#define SL 4096L
#define WIN (4 * SL)
#define NR_WIN 6
#define MAX_IOV 3

struct range {
	const char *name;
	int nr;
	long off[MAX_IOV];	/* from the start of window 1 */
	long len[MAX_IOV];
};

static const struct range ranges[] = {
	{ "slice0",            1, { 0 * SL },          { SL } },
	{ "slice1",            1, { 1 * SL },          { SL } },
	{ "slice2",            1, { 2 * SL },          { SL } },
	{ "slice3",            1, { 3 * SL },          { SL } },
	{ "window",            1, { 0 },               { WIN } },
	{ "two-windows",       1, { 0 },               { 2 * WIN } },
	{ "byte1",             1, { 1 },               { 1 } },
	{ "slice-edge",        1, { SL - 1 },          { 2 } },
	{ "slice1-odd",        1, { SL + 17 },         { SL } },
	{ "slice-cross",       1, { SL - 1 },          { SL + 1 } },
	{ "mid-window",        1, { 2 * SL - 1 },      { 3 } },
	{ "last-byte",         1, { WIN - 1 },         { 1 } },
	{ "window-odd",        1, { 3 },               { WIN - 3 } },
	{ "window-cross",      1, { 3 * SL + 100 },    { 2 * SL } },
	{ "window-cross-back", 1, { -5 },              { 10 } },
	{ "long-odd",          1, { 1 },               { WIN + 2 * SL - 1 } },
	{ "three-iovs",        3, { 100, 5000, WIN + 3 }, { 50, 9000, 4000 } },
};

static uint8_t *base;		/* window 0, 16K aligned */
static uint8_t *expect, *got;
static int fails, setup_err;

static uint8_t pat(long o, int gen)
{
	return (uint8_t)(o ^ (o >> 8) * 7 ^ (o >> 12) * 31 ^ gen * 0x5b);
}

static void fill(int gen, int zap_slice)
{
	long o;
	int w;

	if (zap_slice >= 0)
		for (w = 0; w < NR_WIN; w++) {
			uint8_t *s = base + w * WIN + zap_slice * SL;

			memset(base + w * WIN, 1, WIN);
			madvise(s, SL, MADV_DONTNEED);
		}
	for (o = 0; o < NR_WIN * WIN; o++)
		base[o] = pat(o, gen);
}

static long build(const struct range *r, struct iovec *iov, int gen)
{
	long total = 0;
	int i;

	for (i = 0; i < r->nr; i++) {
		long o = WIN + r->off[i];

		iov[i].iov_base = base + o;
		iov[i].iov_len = r->len[i];
		for (long j = 0; j < r->len[i]; j++)
			expect[total + j] = pat(o + j, gen);
		total += r->len[i];
	}
	return total;
}

static int new_pipe(int p[2])
{
	if (pipe(p)) {
		perror("pipe");
		setup_err = 1;
		return -1;
	}
	if (fcntl(p[1], F_SETPIPE_SZ, 1 << 20) < 0) {
		perror("F_SETPIPE_SZ");
		setup_err = 1;
		return -1;
	}
	return 0;
}

static int check(const char *what, const char *layout, const struct range *r,
		 long total)
{
	for (long j = 0; j < total; j++) {
		if (got[j] != expect[j]) {
			printf("FAIL %s %s %s: byte %ld of %ld is 0x%02x, expected 0x%02x\n",
			       what, layout, r->name, j, total, got[j], expect[j]);
			fails++;
			return 1;
		}
	}
	return 0;
}

static ssize_t do_vmsplice(int fd, const struct range *r, struct iovec *iov,
			   long total)
{
	ssize_t n = vmsplice(fd, iov, r->nr, 0);

	if (n != total) {
		printf("FAIL vmsplice %s: returned %zd, expected %ld\n", r->name, n, total);
		fails++;
		return -1;
	}
	return n;
}

static void case_read(const char *layout, const struct range *r, int gen)
{
	struct iovec iov[MAX_IOV];
	long total = build(r, iov, gen), have = 0;
	int p[2];

	if (new_pipe(p))
		return;
	if (do_vmsplice(p[1], r, iov, total) < 0)
		goto out;
	memset(got, 0xee, total);
	while (have < total) {
		ssize_t n = read(p[0], got + have, total - have);

		if (n <= 0) {
			printf("FAIL read %s %s: read returned %zd after %ld of %ld bytes\n",
			       layout, r->name, n, have, total);
			fails++;
			goto out;
		}
		have += n;
	}
	check("read", layout, r, total);
out:
	close(p[0]);
	close(p[1]);
}

static void case_splice(const char *layout, const struct range *r, int gen)
{
	struct iovec iov[MAX_IOV];
	long total = build(r, iov, gen), have = 0;
	int p[2], fd;

	fd = memfd_create("vmsplice-slices", 0);
	if (fd < 0) {
		perror("memfd_create");
		setup_err = 1;
		return;
	}
	if (new_pipe(p))
		goto out_fd;
	if (do_vmsplice(p[1], r, iov, total) < 0)
		goto out;
	while (have < total) {
		ssize_t n = splice(p[0], NULL, fd, NULL, total - have, 0);

		if (n <= 0) {
			printf("FAIL splice %s %s: splice returned %zd after %ld of %ld bytes\n",
			       layout, r->name, n, have, total);
			fails++;
			goto out;
		}
		have += n;
	}
	memset(got, 0xee, total);
	if (pread(fd, got, total, 0) != total) {
		printf("FAIL splice %s %s: short pread\n", layout, r->name);
		fails++;
		goto out;
	}
	check("splice", layout, r, total);
out:
	close(p[0]);
	close(p[1]);
out_fd:
	close(fd);
}

/* Pipe to user memory: only the range changes, its neighbours keep pat(). */
static void case_to_user(const char *layout, const struct range *r, int gen)
{
	struct iovec iov[MAX_IOV];
	long total = build(r, iov, gen), done = 0, o;
	int p[2], i;

	if (total <= 0 || total > NR_WIN * WIN || new_pipe(p))
		return;
	for (long j = 0; j < total; j++)
		got[j] = (uint8_t)~expect[j];
	if (write(p[1], got, total) != total) {
		printf("FAIL to-user %s %s: short pipe write\n", layout, r->name);
		fails++;
		goto out;
	}
	while (done < total) {
		ssize_t n = vmsplice(p[0], iov, r->nr, 0);

		if (n <= 0) {
			printf("FAIL to-user %s %s: vmsplice returned %zd after %ld of %ld bytes\n",
			       layout, r->name, n, done, total);
			fails++;
			goto out;
		}
		done += n;
		/* Advance the iovecs past what was copied. */
		for (i = 0; i < r->nr && n; i++) {
			long step = (long)iov[i].iov_len < n ? (long)iov[i].iov_len : n;

			iov[i].iov_base = (uint8_t *)iov[i].iov_base + step;
			iov[i].iov_len -= step;
			n -= step;
		}
	}
	/* Expected memory: pat() everywhere, ~pat() inside the ranges. */
	for (o = 0; o < NR_WIN * WIN; o++) {
		uint8_t want = pat(o, gen);

		for (i = 0; i < r->nr; i++)
			if (o >= WIN + r->off[i] && o < WIN + r->off[i] + r->len[i])
				want = (uint8_t)~want;
		if (base[o] != want) {
			printf("FAIL to-user %s %s: offset %ld is 0x%02x, expected 0x%02x\n",
			       layout, r->name, o - WIN, base[o], want);
			fails++;
			break;
		}
	}
out:
	close(p[0]);
	close(p[1]);
}

static int run(void)
{
	static const struct { const char *name; int zap; } layouts[] = {
		{ "fresh", -1 }, { "zap0", 0 }, { "zap1", 1 }, { "zap3", 3 },
	};
	unsigned int l, r;
	int gen = 0, before;
	uint8_t *raw;

	if (getauxval(AT_PAGESZ) != 4096) {
		fprintf(stderr, "child pages=%lu, expected 4096\n", getauxval(AT_PAGESZ));
		return 2;
	}
	setvbuf(stdout, NULL, _IOLBF, 0);
	raw = mmap(NULL, (NR_WIN + 1) * WIN, PROT_READ | PROT_WRITE,
		   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	expect = malloc(NR_WIN * WIN);
	got = malloc(NR_WIN * WIN);
	if (raw == MAP_FAILED || !expect || !got) {
		perror("setup");
		return 2;
	}
	base = (uint8_t *)(((uintptr_t)raw + WIN - 1) & ~(uintptr_t)(WIN - 1));

	for (l = 0; l < sizeof(layouts) / sizeof(layouts[0]); l++) {
		for (r = 0; r < sizeof(ranges) / sizeof(ranges[0]); r++) {
			before = fails;
			fill(++gen, layouts[l].zap);
			case_read(layouts[l].name, &ranges[r], gen);
			fill(++gen, layouts[l].zap);
			case_splice(layouts[l].name, &ranges[r], gen);
			fill(++gen, layouts[l].zap);
			case_to_user(layouts[l].name, &ranges[r], gen);
			printf("%s %s %s\n", fails == before ? "ok  " : "FAIL",
			       layouts[l].name, ranges[r].name);
			if (setup_err)
				break;
		}
	}

	if (setup_err) {
		printf("ERROR: setup failed\n");
		return 2;
	}
	if (fails) {
		printf("FAIL: %d vmsplice checks returned wrong bytes or lengths\n", fails);
		return 1;
	}
	printf("PASS: vmsplice of 4K-process ranges carries the right bytes\n");
	return 0;
}

int main(int argc, char **argv)
{
	int old;

	if (argc > 1 && !strcmp(argv[1], "--child"))
		return run();

	old = personality(0xffffffffUL);
	if (old == -1 || personality((unsigned int)old | ADDR_PAGE_SIZE_4KB) == -1) {
		perror("personality: request 4K at next exec");
		return 1;
	}
	execl("/proc/self/exe", argv[0], "--child", (char *)NULL);
	perror("execl");
	return 1;
}
