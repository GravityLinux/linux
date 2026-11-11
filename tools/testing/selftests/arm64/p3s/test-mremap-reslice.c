/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * In a 4K process, mremap(MREMAP_FIXED) may move private anonymous memory to
 * a destination whose offset inside a 16K window differs from the source's
 * (FEX's 32-bit allocator and code cache do this). The kernel then has to
 * rebuild the destination slices in other folios. Check that a moved range
 * keeps its bytes, that it does not alias what stays behind, and that the
 * usual operations still work on it afterwards.
 *
 * Every 4K page gets a value from a sequence number and its index, so a page
 * that lands in the wrong place or shows another page's data is caught.
 *
 *   shift      move NR pages to each destination slice shift 0..3, from each
 *              source slice offset 0..3
 *   partial    move 1..7 pages out of the middle of a filled range; the pages
 *              left on both sides and the moved pages must stay independent
 *   back       move a range back and forth between slice offsets, 200 times
 *   fork       move, then fork: the child sees the moved bytes, and writes on
 *              either side stay private
 *   dontneed   MADV_DONTNEED one moved slice: it reads zero, its neighbours
 *              keep their bytes
 *   dontunmap  MREMAP_DONTUNMAP to another slice: the destination has the
 *              bytes and the still-mapped source reads zero
 *   swap       MADV_PAGEOUT the source first (needs swap; skipped otherwise)
 *   shared     a shared anonymous or file mapping moved to another slice must
 *              either fail with EINVAL or keep its bytes
 *
 * Exit 0 = PASS, 1 = FAIL, 2 = setup error, 4 = SKIP.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/personality.h>
#include <sys/wait.h>
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
#define NR 13			/* pages per move: not a whole number of windows */
#define AREA (64 * WIN)
#define KSFT_SKIP 4

static int fails, cases, printed;

#define fail(...) do { fails++; if (printed++ < 20) printf("  FAIL: " __VA_ARGS__); } while (0)

static uint32_t val(uint32_t seq, size_t page, size_t word)
{
	return (seq * 0x9e3779b1u) ^ (uint32_t)(page << 12) ^ (uint32_t)word;
}

static void fill(uint8_t *p, size_t pages, uint32_t seq)
{
	for (size_t i = 0; i < pages; i++) {
		uint32_t *w = (uint32_t *)(p + i * SL);

		for (size_t j = 0; j < SL / 4; j++)
			w[j] = val(seq, i, j);
	}
}

/* Pages [0, pages) of @p must hold sequence @seq at pages [first, ...). */
static int verify(const char *c, const uint8_t *p, size_t pages, uint32_t seq,
		  size_t first)
{
	for (size_t i = 0; i < pages; i++) {
		const uint32_t *w = (const uint32_t *)(p + i * SL);

		for (size_t j = 0; j < SL / 4; j++)
			if (w[j] != val(seq, first + i, j)) {
				fail("%s: page %zu word %zu = %08x, want %08x (addr slice %lu)\n",
				     c, i, j, w[j], val(seq, first + i, j),
				     ((uintptr_t)(p + i * SL) / SL) & 3);
				return 1;
			}
	}
	return 0;
}

static int zero(const char *c, const uint8_t *p, size_t len)
{
	for (size_t i = 0; i < len; i++)
		if (p[i]) {
			fail("%s: byte %zu = %02x, want 0\n", c, i, p[i]);
			return 1;
		}
	return 0;
}

/* A PROT_NONE reservation, aligned to a window, to carve fixed ranges from. */
static uint8_t *reserve(size_t len)
{
	uint8_t *raw = mmap(NULL, len + WIN, PROT_NONE,
			    MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);

	if (raw == MAP_FAILED) {
		perror("reserve");
		exit(2);
	}
	return (uint8_t *)(((uintptr_t)raw + WIN - 1) & ~(WIN - 1));
}

static uint8_t *anon_at(uint8_t *at, size_t len)
{
	uint8_t *p = mmap(at, len, PROT_READ | PROT_WRITE,
			  MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);

	if (p != at) {
		perror("anon_at");
		exit(2);
	}
	return p;
}

static void *move(void *src, size_t len, void *dst, int flags)
{
	return mremap(src, len, len, MREMAP_MAYMOVE | MREMAP_FIXED | flags, dst);
}

/* The source range must be gone after a plain move. */
static int unmapped(const char *c, uint8_t *p, size_t len)
{
	unsigned char vec[64];

	if (mincore(p, len, vec) == 0 || errno != ENOMEM) {
		fail("%s: source still mapped after move\n", c);
		return 1;
	}
	return 0;
}

static int case_shift(void)
{
	uint8_t *area = reserve(AREA);
	int f = 0;

	for (int s = 0; s < 4; s++)
		for (int d = 0; d < 4; d++) {
			uint8_t *src = area + s * SL;
			uint8_t *dst = area + AREA / 2 + d * SL;
			uint32_t seq = 100 + s * 4 + d;

			anon_at(area, AREA);
			fill(src, NR, seq);
			if (move(src, NR * SL, dst, 0) != dst) {
				fail("shift %d->%d: mremap: %s\n", s, d, strerror(errno));
				f = 1;
				continue;
			}
			f |= verify("shift", dst, NR, seq, 0);
			f |= unmapped("shift", src, NR * SL);
			/* Write the moved pages again: no stale write protection. */
			fill(dst, NR, seq + 50);
			f |= verify("shift rewrite", dst, NR, seq + 50, 0);
		}
	munmap(area, AREA);
	return f;
}

static int case_partial(void)
{
	uint8_t *area = reserve(AREA);
	int f = 0;

	for (size_t n = 1; n <= 7; n++)
		for (size_t off = 1; off <= 4; off++) {
			uint8_t *src = anon_at(area, 16 * SL);
			uint8_t *dst = area + AREA / 2 + ((off + n) & 3) * SL;
			uint32_t seq = 300 + n * 8 + off;

			fill(src, 16, seq);
			if (move(src + off * SL, n * SL, dst, 0) != dst) {
				fail("partial n=%zu off=%zu: mremap: %s\n", n, off, strerror(errno));
				f = 1;
				munmap(area, AREA);
				continue;
			}
			f |= verify("partial moved", dst, n, seq, off);
			f |= verify("partial left", src, off, seq, 0);
			f |= verify("partial right", src + (off + n) * SL, 16 - off - n, seq, off + n);
			/* Writes on either side must not show through on the other. */
			memset(dst, 0xa5, n * SL);
			f |= verify("partial left after dst write", src, off, seq, 0);
			f |= verify("partial right after dst write", src + (off + n) * SL,
				    16 - off - n, seq, off + n);
			memset(src, 0x5a, off * SL);
			memset(src + (off + n) * SL, 0x5a, (16 - off - n) * SL);
			for (size_t i = 0; i < n * SL; i++)
				if (dst[i] != 0xa5) {
					fail("partial n=%zu off=%zu: moved byte %zu changed to %02x\n",
					     n, off, i, dst[i]);
					f = 1;
					break;
				}
			munmap(area, AREA);
		}
	return f;
}

static int case_back(void)
{
	uint8_t *area = reserve(AREA), *pos[4];
	int f = 0;

	/* Four separate places, each at a different slice offset. */
	for (int k = 0; k < 4; k++)
		pos[k] = area + k * (AREA / 4) + ((k * 3) & 3) * SL;
	anon_at(pos[0], NR * SL);
	fill(pos[0], NR, 500);
	for (int i = 0; i < 200; i++) {
		uint8_t *from = pos[i & 3], *to = pos[(i + 1) & 3];

		if (move(from, NR * SL, to, 0) != to) {
			fail("back %d: mremap: %s\n", i, strerror(errno));
			f = 1;
			break;
		}
		if (verify("back", to, NR, 500, 0)) {
			f = 1;
			break;
		}
	}
	munmap(area, AREA);
	return f;
}

static int case_fork(void)
{
	uint8_t *area = reserve(AREA);
	uint8_t *src = anon_at(area + 2 * SL, NR * SL);
	uint8_t *dst = area + AREA / 2 + 3 * SL;
	int f = 0, status;
	pid_t pid;

	fill(src, NR, 600);
	if (move(src, NR * SL, dst, 0) != dst) {
		fail("fork: mremap: %s\n", strerror(errno));
		return 1;
	}
	pid = fork();
	if (pid < 0) {
		perror("fork");
		exit(2);
	}
	if (!pid) {
		int r = verify("fork child", dst, NR, 600, 0);

		fill(dst, NR, 601);
		r |= verify("fork child write", dst, NR, 601, 0);
		_exit(r);
	}
	if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status)) {
		fail("fork: child saw wrong data\n");
		f = 1;
	}
	f |= verify("fork parent after child write", dst, NR, 600, 0);
	fill(dst, NR, 602);
	f |= verify("fork parent write", dst, NR, 602, 0);
	munmap(area, AREA);
	return f;
}

static int case_dontneed(void)
{
	uint8_t *area = reserve(AREA);
	int f = 0;

	for (int k = 0; k < 4; k++) {
		uint8_t *src = anon_at(area, 8 * SL);
		uint8_t *dst = area + AREA / 2 + SL;

		fill(src, 8, 700 + k);
		if (move(src, 8 * SL, dst, 0) != dst) {
			fail("dontneed: mremap: %s\n", strerror(errno));
			return 1;
		}
		/* Zap page 2 + k of the moved range, then write-fault it. */
		madvise(dst + (2 + k) * SL, SL, MADV_DONTNEED);
		f |= zero("dontneed read", dst + (2 + k) * SL, SL);
		dst[(2 + k) * SL + 7] = 1;
		dst[(2 + k) * SL + 7] = 0;
		f |= zero("dontneed refault", dst + (2 + k) * SL, SL);
		f |= verify("dontneed before", dst, 2 + k, 700 + k, 0);
		f |= verify("dontneed after", dst + (3 + k) * SL, 8 - 3 - k, 700 + k, 3 + k);
		munmap(area, AREA);
	}
	return f;
}

static int case_dontunmap(void)
{
	uint8_t *area = reserve(AREA);
	uint8_t *src = anon_at(area, NR * SL);
	uint8_t *dst = area + AREA / 2 + 3 * SL;
	int f = 0;

	fill(src, NR, 800);
	if (move(src, NR * SL, dst, MREMAP_DONTUNMAP) != dst) {
		if (errno == EINVAL) {
			printf("  dontunmap: EINVAL (not supported for this range)\n");
			munmap(area, AREA);
			return 0;
		}
		fail("dontunmap: mremap: %s\n", strerror(errno));
		return 1;
	}
	f |= verify("dontunmap dst", dst, NR, 800, 0);
	f |= zero("dontunmap src", src, NR * SL);
	fill(src, NR, 801);
	f |= verify("dontunmap dst after src write", dst, NR, 800, 0);
	munmap(area, AREA);
	return f;
}

static int swap_on(void)
{
	FILE *f = fopen("/proc/swaps", "r");
	char line[256];
	int n = 0;

	if (!f)
		return 0;
	while (fgets(line, sizeof(line), f))
		n++;
	fclose(f);
	return n > 1;
}

static int case_swap(void)
{
	uint8_t *area = reserve(AREA);
	uint8_t *src = anon_at(area + SL, 32 * SL);
	uint8_t *dst = area + AREA / 2 + 2 * SL;
	int f = 0;

	fill(src, 32, 900);
	if (madvise(src, 32 * SL, MADV_PAGEOUT)) {
		fail("swap: MADV_PAGEOUT: %s\n", strerror(errno));
		return 1;
	}
	/* Bring back one page so the window is partly resident. */
	f |= verify("swap partial swap-in", src + 5 * SL, 1, 900, 5);
	if (move(src, 32 * SL, dst, 0) != dst) {
		fail("swap: mremap: %s\n", strerror(errno));
		return 1;
	}
	f |= verify("swap", dst, 32, 900, 0);
	f |= unmapped("swap", src, 32 * SL);
	munmap(area, AREA);
	return f;
}

static int case_shared(void)
{
	uint8_t *area = reserve(AREA);
	char path[] = "/tmp/p3s-mremap-XXXXXX";
	int fd = mkstemp(path), f = 0;

	if (fd < 0 || ftruncate(fd, 16 * SL)) {
		perror("scratch file");
		exit(2);
	}
	unlink(path);
	for (int kind = 0; kind < 3; kind++) {
		const char *c = kind == 0 ? "shared anon" : kind == 1 ? "shared file" : "private file";
		int flags = kind == 2 ? MAP_PRIVATE : MAP_SHARED;
		uint8_t *src = mmap(area, 16 * SL, PROT_READ | PROT_WRITE, MAP_FIXED | flags |
				    (kind == 0 ? MAP_ANONYMOUS : 0), kind ? fd : -1, 0);
		uint8_t *dst = area + AREA / 2 + SL;

		if (src != area) {
			perror("shared map");
			exit(2);
		}
		fill(src, 16, 1000 + kind);
		if (move(src, 8 * SL, dst, 0) == dst) {
			f |= verify(c, dst, 8, 1000 + kind, 0);
			f |= verify(c, src + 8 * SL, 8, 1000 + kind, 8);
			printf("  %s: moved to another slice\n", c);
		} else if (errno == EINVAL) {
			f |= verify(c, src, 16, 1000 + kind, 0);
			printf("  %s: EINVAL\n", c);
		} else {
			fail("%s: mremap: %s\n", c, strerror(errno));
			f = 1;
		}
		munmap(area, AREA);
	}
	close(fd);
	return f;
}

static void report(const char *name, int f)
{
	cases++;
	printf("%s %s\n", f ? "FAIL" : "ok  ", name);
}

static int run(void)
{
	if (getpagesize() != (int)SL) {
		printf("SKIP: not a 4K process (page size %d)\n", getpagesize());
		return KSFT_SKIP;
	}
	setvbuf(stdout, NULL, _IONBF, 0);
	report("shift", case_shift());
	report("partial", case_partial());
	report("back", case_back());
	report("fork", case_fork());
	report("dontneed", case_dontneed());
	report("dontunmap", case_dontunmap());
	if (swap_on())
		report("swap", case_swap());
	else
		printf("skip swap (no swap device)\n");
	report("shared", case_shared());
	if (fails) {
		printf("FAIL: %d checks failed in %d cases\n", fails, cases);
		return 1;
	}
	printf("PASS: %d mremap reslice cases\n", cases);
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
