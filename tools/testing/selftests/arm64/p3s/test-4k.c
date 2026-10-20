/* SPDX-License-Identifier: GPL-2.0-only */
#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/auxv.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

#define P 4096UL
#define N (4 * P)

static void require(int ok, const char *what)
{
	if (!ok) {
		fprintf(stderr, "FAIL: %s (errno=%d: %s)\n", what, errno,
			strerror(errno));
		exit(1);
	}
}

/* A child makes a fault probe harmless to the main test and its siblings. */
static void expect_fault(volatile unsigned char *p, int write_access)
{
	int status;
	pid_t child = fork();

	require(child >= 0, "fork fault probe");
	if (!child) {
		if (write_access)
			*p = 0xff;
		else
			(void)*p;
		_exit(0);
	}
	require(waitpid(child, &status, 0) == child, "wait fault probe");
	require(WIFSIGNALED(status) && WTERMSIG(status) == SIGSEGV,
		"access raises SIGSEGV");
}

static void expect_bytes(const unsigned char *p, unsigned char value)
{
	for (size_t i = 0; i < P; i++)
		require(p[i] == value, "quarter contents preserved");
}

int main(void)
{
	struct rlimit core = {0, 0};
	unsigned char *reservation, *p, *alias;
	unsigned char data[P];
	int fd, status;
	pid_t child;
	long pages = sysconf(_SC_PAGESIZE);
	unsigned long aux = getauxval(AT_PAGESZ);

	setbuf(stdout, NULL);
	require(setrlimit(RLIMIT_CORE, &core) == 0, "disable core dumps");
	printf("test pages: sysconf=%ld auxv=%lu\n", pages, aux);
	require(pages == P && aux == P, "exec selected a 4K process ABI");
	reservation = mmap(NULL, 4 * N, PROT_NONE,
			   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	require(reservation != MAP_FAILED, "reserve aligned test address");
	p = (unsigned char *)(((uintptr_t)reservation + N - 1) & ~(N - 1));
	require(mmap(p, N, PROT_READ | PROT_WRITE,
		     MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0) == p,
		"map four 4K quarters");
	for (unsigned int i = 0; i < 4; i++)
		memset(p + i * P, 0x10 + i, P);

	require(mprotect(p + P, P, PROT_READ) == 0, "mprotect one 4K quarter R");
	expect_fault(p + P, 1);
	expect_bytes(p + P, 0x11);
	p[0] = 0x41;
	p[2 * P] = 0x42;
	require(mprotect(p + P, P, PROT_NONE) == 0, "mprotect one quarter NONE");
	expect_fault(p + P, 0);
	require(p[0] == 0x41 && p[2 * P] == 0x42, "adjacent quarters remain RW");
	require(mprotect(p + P, P, PROT_READ | PROT_WRITE) == 0,
		"restore quarter permissions");
	expect_bytes(p + P, 0x11);

	require(munmap(p + 2 * P, P) == 0, "munmap one 4K quarter");
	expect_fault(p + 2 * P, 0);
	expect_bytes(p + P, 0x11);
	expect_bytes(p + 3 * P, 0x13);
	require(mmap(p + 2 * P, P, PROT_READ | PROT_WRITE,
		     MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0) == p + 2 * P,
		"replace one quarter");
	expect_bytes(p + 2 * P, 0);
	memset(p + 2 * P, 0x22, P);

	child = fork();
	require(child >= 0, "fork COW");
	if (!child) {
		memset(p + 2 * P, 0x55, P);
		expect_bytes(p + P, 0x11);
		expect_bytes(p + 3 * P, 0x13);
		_exit(0);
	}
	require(waitpid(child, &status, 0) == child && WIFEXITED(status) &&
		WEXITSTATUS(status) == 0, "child COW and neighboring slices");
	expect_bytes(p + 2 * P, 0x22);
	puts("PASS: 4K protection, unmap/replacement and fork COW");

	fd = memfd_create("warthog-slices", MFD_CLOEXEC);
	require(fd >= 0 && ftruncate(fd, N) == 0, "create native-sized shmem file");
	for (unsigned int i = 0; i < 4; i++) {
		memset(data, 0x30 + i, P);
		require(pwrite(fd, data, P, i * P) == P, "initialize file slice");
	}
	/* VA quarter 1 uses file quarter 3: deliberately incongruent at 16K. */
	require(mmap(p + P, P, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED,
		     fd, 3 * P) == p + P, "map incongruent 4K file offset");
	expect_bytes(p + P, 0x33);
	expect_bytes(p + 2 * P, 0x22);
	alias = mmap(NULL, P, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 3 * P);
	require(alias != MAP_FAILED, "map shared file alias");
	p[P + 17] = 0x7e;
	require(alias[17] == 0x7e, "shared alias sees the correct byte");
	require(pread(fd, data, P, 3 * P) == P && data[17] == 0x7e,
		"kernel read sees slice write");
	require(pread(fd, data, P, P) == P && data[17] == 0x31,
		"unrelated file slice unchanged");
	require(munmap(alias, P) == 0, "unmap file alias");
	require(close(fd) == 0, "close file");
	require(munmap(reservation, 4 * N) == 0, "release complete reservation");
	puts("PASS: 4K file offset, shared aliases and kernel I/O");
	puts("PASS: warthog 4K process smoke");
	return 0;
}
