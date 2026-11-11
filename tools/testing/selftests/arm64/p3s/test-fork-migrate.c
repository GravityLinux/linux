/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * Fork a 4K process whose 16K windows are split by a 4K MADV_DONTFORK, let
 * compaction migrate the shared folios, and check that parent and child
 * writes stay apart. Sibling slices share one folio, so the parent keeps a
 * writable slice on a folio the child maps; migration must not make that
 * folio exclusive again. Needs root for vm.compact_memory.
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
#include <sys/wait.h>
#include <unistd.h>

#define ADDR_PAGE_SIZE_4KB 0x40000000UL
#define SL 4096UL
#define WIN (4 * SL)
#define NR_WIN 8192
#define ROUNDS 20
#define KSFT_SKIP 4

static uint8_t val(int round, int slice)
{
	return 0x10 + slice + 4 * (round & 3);
}

static int child_check(uint8_t *m, int round, int fd)
{
	char b;

	if (read(fd, &b, 1) != 1)
		return 2;
	for (size_t w = 0; w < NR_WIN; w++)
		for (int s = 0; s < 4; s++) {
			uint8_t *q = m + w * WIN + s * SL;

			if (s == 2)
				continue;
			for (size_t i = 0; i < SL; i++)
				if (q[i] != val(round, s)) {
					printf("FAIL: round %d window %zu slice %d: child sees %02x, expected %02x\n",
					       round, w, s, q[i], val(round, s));
					return 1;
				}
		}
	return 0;
}

static int round_once(int round, int compact)
{
	uint8_t *raw = mmap(NULL, NR_WIN * WIN + WIN, PROT_READ | PROT_WRITE,
			    MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	int go[2], status;
	uint8_t *m;
	pid_t pid;

	if (raw == MAP_FAILED || pipe(go))
		return 2;
	m = (uint8_t *)(((uintptr_t)raw + WIN - 1) & ~(WIN - 1));
	/* Slice 0 first, so slices 1-3 reuse its folio. */
	for (size_t w = 0; w < NR_WIN; w++)
		for (int s = 0; s < 4; s++)
			memset(m + w * WIN + s * SL, val(round, s), SL);
	for (size_t w = 0; w < NR_WIN; w++)
		if (madvise(m + w * WIN + 2 * SL, SL, MADV_DONTFORK))
			return 2;
	pid = fork();
	if (pid < 0)
		return 2;
	if (!pid)
		_exit(child_check(m, round, go[0]));
	for (size_t w = 0; w < NR_WIN; w++)
		m[w * WIN + 2 * SL] ^= 1;
	if (write(compact, "1", 1) != 1)
		return 2;
	for (size_t w = 0; w < NR_WIN; w++) {
		memset(m + w * WIN, 0xa0, SL);
		memset(m + w * WIN + SL, 0xa1, SL);
	}
	if (write(go[1], "g", 1) != 1)
		return 2;
	waitpid(pid, &status, 0);
	munmap(raw, NR_WIN * WIN + WIN);
	close(go[0]);
	close(go[1]);
	return WIFEXITED(status) ? WEXITSTATUS(status) : 1;
}

int main(int argc, char **argv)
{
	int compact, old, r, rc;

	if (argc > 1 && !strcmp(argv[1], "--child")) {
		if (getauxval(AT_PAGESZ) != 4096) {
			fprintf(stderr, "child pages=%lu, expected 4096\n", getauxval(AT_PAGESZ));
			return 1;
		}
		compact = open("/proc/sys/vm/compact_memory", O_WRONLY);
		if (compact < 0) {
			printf("SKIP: vm.compact_memory not writable\n");
			return KSFT_SKIP;
		}
		for (r = 0; r < ROUNDS; r++) {
			rc = round_once(r, compact);
			if (rc)
				return rc;
		}
		printf("PASS: %d rounds, no parent/child aliasing\n", ROUNDS);
		return 0;
	}

	old = personality(0xffffffffUL);
	if (old == -1 || personality((unsigned int)old | ADDR_PAGE_SIZE_4KB) == -1) {
		perror("personality: request 4K at next exec");
		return 1;
	}
	execl("/proc/self/exe", argv[0], "--child", (char *)NULL);
	perror("execl");
	return 1;
}
