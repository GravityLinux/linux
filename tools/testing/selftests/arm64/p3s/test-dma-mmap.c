/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * Map the 64 KiB buffer of dma-mmap-probe.ko through the real
 * dma_mmap_pages() path, natively or from a 4K process (exec-4k).
 *
 * A 4K mapping is bounded in kernel pages and maps the 4K slices of the
 * buffer that its file offset selects: the whole buffer, single slices
 * inside a kernel page and ranges across a kernel page must all alias
 * the same bytes, ranges past the buffer fail with ENXIO, and no mapping
 * may install PTEs past its end (an anonymous neighbour follows each one).
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <unistd.h>

#define BUF (64UL << 10)

static void check(int ok, const char *what)
{
	if (!ok) { perror(what); exit(1); }
}

static unsigned char pattern(size_t i)
{
	return (i * 29 + 7) & 255;
}

/* Map [off, off + len) of the buffer at a fresh place with an anon neighbour. */
static unsigned char *alias(int fd, size_t off, size_t len, size_t page, int *err)
{
	char *raw = mmap(NULL, len + 4 * 16384 + page, PROT_NONE,
			 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	char *p = (void *)(((uintptr_t)raw + 16383) & ~(uintptr_t)16383);
	void *m;

	check(raw != MAP_FAILED, "reserve alias");
	/* Start inside a 16K window when the process page size allows it. */
	if (page == 4096)
		p += 4096;
	check(mmap(p + len, page, PROT_READ | PROT_WRITE,
		   MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) == p + len, "neighbour");
	p[len] = 0x5a;
	m = mmap(p, len, PROT_READ | PROT_WRITE, MAP_FIXED | MAP_SHARED, fd, off);
	*err = m == MAP_FAILED ? errno : 0;
	if (*err) {
		/* A following anonymous mmap must not meet leaked DMA PTEs. */
		check(mmap(p, len, PROT_READ | PROT_WRITE,
			   MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) == p, "after refusal");
		for (size_t i = 0; i < len; i += page)
			check(p[i] == 0, "after refusal zero fill");
	}
	check(p[len] == 0x5a, "neighbour unchanged");
	return (unsigned char *)p;
}

int main(void)
{
	size_t page = sysconf(_SC_PAGESIZE);
	int fd = open("/dev/p3s-dma-mmap-probe", O_RDWR), err, cases = 0;
	unsigned char *full, *a;
	const struct { size_t off, len; } ok[] = {
		{ 0, BUF },
		{ 0, 16384 },
		{ 16384, 16384 },
		{ 16384, 2 * 16384 },
		{ 4096, 4096 },		/* 4K only: one slice inside a kernel page */
		{ 3 * 4096, 4096 },
		{ 4096, 2 * 4096 },
		{ 16384 - 4096, 2 * 4096 },	/* across a kernel page */
		{ BUF - 4096, 4096 },
		{ 4096, BUF - 4096 },
	};
	const struct { size_t off, len; } bad[] = {
		{ 0, 2 * BUF },
		{ BUF, 16384 },
		{ BUF - 16384, 2 * 16384 },
		{ BUF - 4096, 2 * 4096 },	/* 4K only */
	};

	check(fd >= 0, "open /dev/p3s-dma-mmap-probe (insmod dma-mmap-probe.ko)");
	printf("pages=%zu\n", page);
	full = alias(fd, 0, BUF, page, &err);
	check(!err, "map whole buffer");
	for (size_t i = 0; i < BUF; i++)
		full[i] = pattern(i);

	for (size_t c = 0; c < sizeof(ok) / sizeof(ok[0]); c++) {
		size_t off = ok[c].off, len = ok[c].len;

		if ((off | len) % page)
			continue;
		a = alias(fd, off, len, page, &err);
		errno = err;
		check(!err, "map buffer range");
		for (size_t i = 0; i < len; i++)
			if (a[i] != pattern(off + i)) {
				fprintf(stderr, "FAIL: off=%#zx len=%#zx byte %#zx = %#x, want %#x\n",
					off, len, i, a[i], pattern(off + i));
				exit(1);
			}
		/* Writes through the alias must reach the same buffer bytes. */
		a[0] ^= 0xff;
		a[len - 1] ^= 0xff;
		check(full[off] == (pattern(off) ^ 0xff) &&
		      full[off + len - 1] == (pattern(off + len - 1) ^ 0xff), "alias writes");
		full[off] = pattern(off);
		full[off + len - 1] = pattern(off + len - 1);
		check(munmap(a, len) == 0, "unmap alias");
		printf("ok: off=%#zx len=%#zx\n", off, len);
		cases++;
	}
	for (size_t c = 0; c < sizeof(bad) / sizeof(bad[0]); c++) {
		size_t off = bad[c].off, len = bad[c].len;

		if ((off | len) % page)
			continue;
		alias(fd, off, len, page, &err);
		if (err != ENXIO) {
			fprintf(stderr, "FAIL: off=%#zx len=%#zx: errno %d, want ENXIO\n",
				off, len, err);
			exit(1);
		}
		printf("ENXIO: off=%#zx len=%#zx\n", off, len);
		cases++;
	}
	for (size_t i = 0; i < BUF; i++)
		check(full[i] == pattern(i), "buffer intact");
	close(fd);
	printf("PASS: pages=%zu, %d DMA mmap ranges; slices alias, out-of-range ENXIO, neighbours intact\n",
	       page, cases);
	return 0;
}
