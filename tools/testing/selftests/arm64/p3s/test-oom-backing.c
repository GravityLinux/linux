// SPDX-License-Identifier: GPL-2.0-only
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

static void check(int ok, const char *why)
{
	if (!ok) { perror(why); exit(1); }
}
static int score(void)
{
	int n;
	FILE *f = fopen("/proc/self/oom_score", "r");
	check(f != NULL && fscanf(f, "%d", &n) == 1, "oom_score");
	fclose(f); return n;
}
int main(int argc, char **argv)
{
	size_t size = 256UL << 20;
	size_t page = sysconf(_SC_PAGESIZE), stride = page;
	unsigned char *p;
	int before, full, partial, after;

	setvbuf(stdout, NULL, _IONBF, 0);
	check(argc == 2 || (argc == 4 && !strcmp(argv[2], "--hold")),
	      "usage: test-oom-backing dense|sparse [--hold MiB]");
	if (argc == 4) size = strtoul(argv[3], NULL, 10) << 20;
	check(!strcmp(argv[1], "dense") || !strcmp(argv[1], "sparse"), "mode");
	if (!strcmp(argv[1], "sparse")) stride = 16384;
	before = score();
	p = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	check(p != MAP_FAILED, "mmap");
	(void)madvise(p, size, MADV_NOHUGEPAGE);
	if (page == 4096 && stride == 16384) {
		/* Seed anon_vma before splitting; otherwise adjacent VMAs may not merge. */
		p[0] = 1;
		check(madvise(p, 16384, MADV_DONTNEED) == 0, "discard seed");
		/* P3S batches empty siblings: limit each writable VMA to one slice. */
		for (size_t i = 0; i < size; i += stride)
			check(mprotect(p + i + page, stride - page, PROT_NONE) == 0,
			      "guard unused slices");
	}
	for (size_t i = 0; i < size; i += stride) p[i] = 0x73;
	if (page == 4096 && stride == 16384) {
		char line[256]; unsigned long anon = 0;
		FILE *f = fopen("/proc/self/status", "r");

		check(f != NULL, "status");
		while (fgets(line, sizeof(line), f))
			if (sscanf(line, "RssAnon: %lu", &anon) == 1) break;
		fclose(f);
		printf("SPARSE logical_anon=%lu KiB native16k_backing=%zu MiB\n", anon, size >> 20);
		check(anon >= size / 4096 && anon <= size / 4096 + 2048,
		      "sparse RSS must be one quarter of backing");
		check(mprotect(p, size, PROT_READ | PROT_WRITE) == 0, "restore sparse guards");
	}
	full = score();
	if (argc == 4) {
		printf("READY pid=%d page=%zu size=%zu stride=%zu score=%d\n",
		       getpid(), page, size, stride, full);
		for (;;) pause();
	}
	pid_t child = fork();
	int rc;

	check(child >= 0, "fork");
	if (!child) {
		check(abs(score() - full) <= 2, "fork backing weight");
		for (size_t j = 0; j < size; j += stride) p[j] = 0x55;
		check(abs(score() - full) <= 2, "COW backing weight");
		p = mremap(p, size, size * 2, MREMAP_MAYMOVE);
		check(p != MAP_FAILED, "mremap");
		check(abs(score() - full) <= 2, "mremap backing weight");
		check(munmap(p, size * 2) == 0, "child unmap");
		check(abs(score() - before) <= 2, "child counter baseline");
		_exit(0);
	}
	check(waitpid(child, &rc, 0) == child && WIFEXITED(rc) && WEXITSTATUS(rc) == 0,
	      "fork/COW/mremap child");
	/* Permission changes clear/reinstall PTEs and must not grow the count. */
	check(mprotect(p, size, PROT_READ) == 0, "mprotect read");
	check(mprotect(p, size, PROT_READ | PROT_WRITE) == 0, "mprotect write");
	check(abs(score() - full) <= 1, "permission change OOM weight");
	check(madvise(p + size / 4, size / 2, MADV_DONTNEED) == 0, "bulk discard");
	partial = score();
	check(partial <= full && full >= before, "discard OOM weight");
	check(munmap(p, size) == 0, "unmap"); after = score();
	check(abs(after - before) <= 1, "counter returns to baseline");
	printf("RESULT page=%zu mode=%s score_before=%d full=%d half=%d after=%d\n", page, argv[1], before, full, partial, after);
	puts("PASS: OOM weight stable across mprotect, decreases on discard/unmap");
	return 0;
}
