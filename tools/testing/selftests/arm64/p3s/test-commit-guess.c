// SPDX-License-Identifier: GPL-2.0-only
#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

/* Reserve only; never fault these deliberately large virtual ranges. */
int main(int argc, char **argv)
{
	unsigned long ram = 0, swap = 0, bytes;
	char line[256];
	int mode = -1, want_mode, succeeds, shared;
	FILE *f;
	void *p;

	if (argc != 5) {
		fprintf(stderr,
			"usage: %s mode half|above-total|bytes succeeds private|shared\n",
			argv[0]);
		return 2;
	}
	want_mode = atoi(argv[1]);
	succeeds = atoi(argv[3]);
	shared = !strcmp(argv[4], "shared");
	if (!shared && strcmp(argv[4], "private"))
		return 2;
	f = fopen("/proc/sys/vm/overcommit_memory", "r");
	if (!f || fscanf(f, "%d", &mode) != 1)
		return 1;
	fclose(f);
	if (mode != want_mode) {
		fprintf(stderr, "wrong overcommit mode %d (want %d)\n", mode, want_mode);
		return 1;
	}
	f = fopen("/proc/meminfo", "r");
	if (!f)
		return 1;
	while (fgets(line, sizeof(line), f)) {
		if (sscanf(line, "MemTotal: %lu", &ram) == 1)
			continue;
		if (sscanf(line, "SwapTotal: %lu", &swap) == 1)
			continue;
	}
	fclose(f);
	if (!ram)
		return 1;
	if (!strcmp(argv[2], "half"))
		bytes = (ram * 1024 / 2 + 65535) & ~65535UL;
	else if (!strcmp(argv[2], "above-total"))
		bytes = (ram + swap) * 1024 + (1UL << 20);
	else
		bytes = strtoul(argv[2], NULL, 0);
	printf("page=%ld mode=%d RAM=%luKiB swap=%luKiB %s bytes=%lu expect=%s\n",
	       sysconf(_SC_PAGESIZE), mode, ram, swap, shared ? "shared" : "private",
	       bytes, succeeds ? "success" : "ENOMEM");
	p = mmap(NULL, bytes, PROT_READ | PROT_WRITE,
		 MAP_ANONYMOUS | (shared ? MAP_SHARED : MAP_PRIVATE), -1, 0);
	if ((p != MAP_FAILED) != succeeds || (p == MAP_FAILED && errno != ENOMEM)) {
		perror("commit decision");
		if (p != MAP_FAILED)
			munmap(p, bytes);
		return 1;
	}
	if (p != MAP_FAILED && munmap(p, bytes))
		return 1;
	puts("PASS: reservation decision, no bytes faulted");
	return 0;
}
