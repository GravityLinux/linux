/* SPDX-License-Identifier: GPL-2.0-only */
#define _GNU_SOURCE
#include <errno.h>
#include <linux/futex.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static void fail(const char *what)
{
	perror(what);
	exit(1);
}

static uint32_t *map_slice(int fd, unsigned int quarter, off_t offset)
{
	void *reservation, *slice;
	uintptr_t base;

	reservation = mmap(NULL, 65536, PROT_NONE,
			   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	if (reservation == MAP_FAILED)
		fail("reserve");
	base = ((uintptr_t)reservation + 16383) & ~(uintptr_t)16383;
	slice = mmap((void *)(base + quarter * 4096), 4096,
		     PROT_READ | PROT_WRITE, MAP_SHARED | MAP_FIXED, fd, offset);
	if (slice == MAP_FAILED)
		fail("map slice");
	/* Check an intra-slice byte offset as well as the slice number. */
	return (uint32_t *)((char *)slice + 64);
}

static long wake(uint32_t *word)
{
	return syscall(SYS_futex, word, FUTEX_WAKE, 1, NULL, NULL, 0);
}

static void wait_word(uint32_t *word, int ready)
{
	struct timespec timeout = { .tv_sec = 5 };
	char byte = 'r';
	long rc;

	if (write(ready, &byte, 1) != 1)
		_exit(2);
	close(ready);
	rc = syscall(SYS_futex, word, FUTEX_WAIT, 0, &timeout, NULL, 0);
	if (rc) {
		dprintf(2, "wait: rc=%ld errno=%d (%s)\n", rc, errno, strerror(errno));
		_exit(1);
	}
	_exit(0);
}

static void wake_child(pid_t child, int ready, uint32_t *word, uint32_t *wrong)
{
	char byte;
	int status = 0, i;
	long rc = 0;

	if (read(ready, &byte, 1) != 1)
		fail("waiter ready");
	close(ready);
	/* Never wake another byte in the same native backing page. */
	for (i = 0; i < 100; i++) {
		if (wake(wrong) != 0)
			fail("wrong slice wake");
		usleep(1000);
	}
	/* The pipe precedes WAIT; retry until the child has actually queued. */
	for (i = 0; i < 4000 && rc == 0; i++) {
		rc = wake(word);
		if (rc < 0)
			fail("shared wake");
		if (!rc)
			usleep(1000);
	}
	if (waitpid(child, &status, 0) != child || rc != 1 ||
	    !WIFEXITED(status) || WEXITSTATUS(status)) {
		fprintf(stderr, "shared pair failed: wake=%ld child status=%d\n", rc, status);
		exit(1);
	}
}

int main(int argc, char **argv)
{
	struct rlimit no_core = { 0, 0 };
	uint32_t *word, *alias, *wrong;
	int fd, ready[2];
	pid_t child;
	long rc;

	setrlimit(RLIMIT_CORE, &no_core);
	alarm(10);
	if (argc == 4 && !strcmp(argv[1], "--child")) {
		if (sysconf(_SC_PAGESIZE) != 4096)
			return 2;
		word = map_slice(atoi(argv[2]), 1, 12288);
		wait_word(word, atoi(argv[3]));
	}

	/* Leave the memfd open across exec for the mixed-geometry child. */
	fd = memfd_create("futex-slice", 0);
	if (fd < 0 || ftruncate(fd, 16384) || pipe(ready))
		fail("setup");
	if (argc == 3 && !strcmp(argv[1], "--mixed")) {
		void *mapping = mmap(NULL, 16384, PROT_READ | PROT_WRITE,
				     MAP_SHARED, fd, 0);

		if (mapping == MAP_FAILED)
			fail("native mapping");
		word = (uint32_t *)((char *)mapping + 12288 + 64);
		wrong = (uint32_t *)((char *)mapping + 4096 + 64);
		child = fork();
		if (!child) {
			char fd_arg[32], ready_arg[32];

			close(ready[0]);
			snprintf(fd_arg, sizeof(fd_arg), "%d", fd);
			snprintf(ready_arg, sizeof(ready_arg), "%d", ready[1]);
			execl(argv[2], argv[2], argv[0], "--child", fd_arg, ready_arg, NULL);
			fail("exec 4K child");
		}
		if (child < 0)
			fail("fork");
		close(ready[1]);
		wake_child(child, ready[0], word, wrong);
		printf("PASS: %ld-byte parent wakes 4K child at backing byte 12352\n",
		       sysconf(_SC_PAGESIZE));
		return 0;
	}
	if (argc != 1 || sysconf(_SC_PAGESIZE) != 4096)
		return 2;
	word = map_slice(fd, 1, 12288);
	alias = map_slice(fd, 2, 12288);
	wrong = map_slice(fd, 1, 4096);
	*word = 0;
	*wrong = 0;
	errno = 0;
	rc = wake(word);
	printf("shared futex on lone 4K slice: rc=%ld errno=%d (%s); expected 0\n",
	       rc, errno, strerror(errno));
	if (rc != 0)
		return 1;
	if (mprotect((char *)alias - 64, 4096, PROT_READ))
		fail("read-only alias");
	child = fork();
	if (!child) {
		close(ready[0]);
		wait_word(alias, ready[1]);
	}
	if (child < 0)
		fail("fork");
	close(ready[1]);
	wake_child(child, ready[0], word, wrong);
	puts("PASS: shared file aliases match backing bytes, including read-only WAIT");
	return 0;
}
