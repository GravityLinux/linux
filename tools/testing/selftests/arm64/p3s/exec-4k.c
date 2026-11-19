/* SPDX-License-Identifier: GPL-2.0-only */
#define _GNU_SOURCE
#include <stdio.h>
#include <sys/personality.h>
#include <unistd.h>

/* Research ABI from warthog; the host's libc headers do not define it yet. */
#define ADDR_PAGE_SIZE_4KB 0x40000000UL

int main(int argc, char **argv)
{
	int old;

	if (argc < 2) {
		fprintf(stderr, "usage: %s PROGRAM [ARG ...]\n", argv[0]);
		return 2;
	}
	old = personality(0xffffffffUL);
	if (old == -1 || personality((unsigned int)old | ADDR_PAGE_SIZE_4KB) == -1) {
		perror("personality: request 4K at next exec");
		return 1;
	}
	fprintf(stderr, "launcher pages=%ld; requesting 4K for %s\n",
		sysconf(_SC_PAGESIZE), argv[1]);
	execvp(argv[1], argv + 1);
	perror("execvp");
	return 1;
}
