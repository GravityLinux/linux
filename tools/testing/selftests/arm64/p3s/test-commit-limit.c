// SPDX-License-Identifier: GPL-2.0-only
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <unistd.h>

int main(int argc, char **argv)
{
	char line[256];
	unsigned long limit = 0, expected;
	int succeeds;
	void *p;
	FILE *f;

	if (argc != 3) return 2;
	expected = strtoul(argv[1], NULL, 10);
	succeeds = atoi(argv[2]);
	f = fopen("/proc/meminfo", "r");
	if (!f) { perror("meminfo"); return 1; }
	while (fgets(line, sizeof(line), f))
		if (sscanf(line, "CommitLimit: %lu", &limit) == 1) break;
	fclose(f);
	printf("page=%ld CommitLimit=%lu KiB expected=%lu\n", sysconf(_SC_PAGESIZE), limit, expected);
	if (limit != expected) return 1;
	p = mmap(NULL, 400UL << 20, PROT_READ | PROT_WRITE,
		 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	if ((p != MAP_FAILED) != succeeds || (p == MAP_FAILED && errno != ENOMEM)) {
		perror("strict commitment decision"); return 1;
	}
	if (p != MAP_FAILED && munmap(p, 400UL << 20)) return 1;
	puts("PASS: invariant byte limit and conservative native backing commitment");
	return 0;
}
