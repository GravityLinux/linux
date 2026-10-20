/* SPDX-License-Identifier: GPL-2.0-only */
#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

static void check(int ok, const char *what)
{
	if (!ok) { fprintf(stderr, "FAIL: %s\n", what); exit(1); }
}

/* Guard pages prevent merging with unrelated VMAs in smaps. */
static void *mapping(size_t size, int fd, off_t offset)
{
	char *raw = mmap(NULL, size + 65536, PROT_NONE,
			 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	void *p = (void *)(((uintptr_t)raw + 32767) & ~(uintptr_t)16383);

	check(raw != MAP_FAILED, "reservation");
	check(mmap(p, size, PROT_READ | PROT_WRITE,
		   MAP_FIXED | (fd < 0 ? MAP_PRIVATE | MAP_ANONYMOUS : MAP_SHARED),
		   fd, offset) == p, "mapping");
	return p;
}

static unsigned long stat(void *p, const char *field)
{
	FILE *f = fopen("/proc/self/smaps", "r");
	char line[512], name[64];
	unsigned long start, end, value;
	int matched = 0;

	check(f != NULL, "open smaps");
	while (fgets(line, sizeof(line), f)) {
		if (sscanf(line, "%lx-%lx", &start, &end) == 2)
			matched = start == (uintptr_t)p;
		else if (matched && sscanf(line, "%63[^:]: %lu", name, &value) == 2 &&
			 !strcmp(name, field)) {
			fclose(f);
			return value;
		}
	}
	fclose(f);
	check(0, "smaps field");
	return 0;
}

static void expect(void *p, unsigned long rss, unsigned long pss,
		   const char *label)
{
	unsigned long r = stat(p, "Rss"), s = stat(p, "Pss");

	printf("%s: RSS=%lu PSS=%lu KiB (expected %lu/%lu)\n", label, r, s, rss, pss);
	check(r == rss && s == pss, label);
}

int main(int argc, char **argv)
{
	size_t size = 64UL << 20, page = sysconf(_SC_PAGESIZE);
	volatile unsigned char *p;
	int ready[2], release[2], status, fd;
	pid_t child;
	char byte;

	setvbuf(stdout, NULL, _IONBF, 0);
	if (argc == 3 && !strcmp(argv[1], "--hold"))
		size = strtoul(argv[2], NULL, 10) << 20;
	p = mapping(size, -1, 0);
	for (size_t i = 0; i < size; i += page)
		p[i] = 0x73;
	printf("pid=%d pages=%zu allocated=%zu\n", getpid(), page, size);
	expect((void *)p, size >> 10, size >> 10, "private anonymous");
	if (argc == 3) {
		/* Controller reads status/cgroup/OOM scores, then terminates us. */
		for (;;)
			pause();
	}
	check(pipe(ready) == 0 && pipe(release) == 0, "pipes");
	child = fork();
	check(child >= 0, "fork");
	if (!child) {
		close(ready[0]); close(release[1]);
		check(write(ready[1], "r", 1) == 1, "child ready");
		check(read(release[0], &byte, 1) == 1, "child release");
		expect((void *)p, size >> 10, size >> 11, "fork child");
		_exit(0);
	}
	close(ready[1]); close(release[0]);
	check(read(ready[0], &byte, 1) == 1, "parent ready");
	expect((void *)p, size >> 10, size >> 11, "fork parent");
	check(write(release[1], "r", 1) == 1, "parent release");
	check(waitpid(child, &status, 0) == child && WIFEXITED(status) &&
	      WEXITSTATUS(status) == 0, "child status");
	expect((void *)p, size >> 10, size >> 10, "after child exit");

	fd = memfd_create("accounting", 0);
	check(fd >= 0 && ftruncate(fd, 16384) == 0, "memfd");
	p = mapping(16384, fd, 0);
	for (size_t i = 0; i < 16384; i += page)
		p[i] = 0x55;
	expect((void *)p, 16, 16, "single-owner shmem");
	void *alias = mapping(16384, fd, 0);
	for (size_t i = 0; i < 16384; i += page)
		check(((volatile unsigned char *)alias)[i] == 0x55, "alias bytes");
	expect((void *)p, 16, 8, "first shmem alias");
	expect(alias, 16, 8, "second shmem alias");
	check(munmap(alias, 16384) == 0 && munmap((void *)p, 16384) == 0, "unmap aliases");
	if (page == 4096) {
		/* A partial mapping owns native allocation bytes, not just RSS. */
		p = mapping(4096, fd, 4096);
		check(*p == 0x55, "nonzero slice");
		unsigned long physical_kib = stat((void *)p, "Pss");
		check(stat((void *)p, "Rss") == 4 &&
		      (physical_kib == 4 || physical_kib == 16), "lone-slice physical PSS");
		printf("lone slice: RSS=4 PSS=%lu KiB (4K host:4; 16K host:16)\n", physical_kib);
	}
	puts("PASS: private, fork sharing, shared aliases and lone-slice RSS/PSS");
	return 0;
}
