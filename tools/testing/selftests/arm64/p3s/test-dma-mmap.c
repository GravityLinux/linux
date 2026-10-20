/* SPDX-License-Identifier: GPL-2.0-only */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <unistd.h>

static void check(int ok, const char *what)
{
	if (!ok) { perror(what); exit(1); }
}

int main(void)
{
	size_t size = sysconf(_SC_PAGESIZE);
	int fd = open("/dev/p3s-dma-mmap-probe", O_RDWR);
	char *raw = mmap(NULL, 65536, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	char *p = (void *)(((uintptr_t)raw + 32767) & ~(uintptr_t)16383);
	void *mapped;

	check(fd >= 0 && raw != MAP_FAILED, "setup scratch DMA mapper");
	check(mmap(p + size, size, PROT_READ | PROT_WRITE,
		   MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) == p + size, "neighbor");
	p[size] = 0x5a;
	mapped = mmap(p, size, PROT_READ | PROT_WRITE, MAP_FIXED | MAP_SHARED, fd, 0);
	if (size == 4096) {
		check(mapped == MAP_FAILED && errno == ENXIO, "4K DMA mmap refused");
		/* A following anonymous mmap must not encounter leaked DMA PTEs. */
		check(mmap(p, size, PROT_READ | PROT_WRITE,
			   MAP_FIXED | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) == p, "after refusal");
		check(*p == 0, "after refusal zero fill");
	} else {
		check(mapped == p, "native DMA mmap");
		*p = 0x33;
		check(*p == 0x33, "native DMA bytes");
	}
	check(p[size] == 0x5a, "neighbor unchanged");
	check(munmap(raw, 65536) == 0, "unmap scratch");
	close(fd);
	puts("PASS: real DMA mapper, 4K refusal/native mapping, adjacent VMA preserved");
	return 0;
}
