// SPDX-License-Identifier: GPL-2.0-only
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <sys/ipc.h>
#include <sys/personality.h>
#include <sys/resource.h>
#include <sys/shm.h>
#include <sys/wait.h>
#include <unistd.h>

#define P3S_4K 0x40000000UL
static int ids[2] = {-1,-1};
static void cleanup(void)
{
	for (int i = 0; i < 2; i++)
		if (ids[i] >= 0) (void)shmctl(ids[i], IPC_RMID, NULL);
}
static void check(int ok, const char *why)
{
	if (!ok) { perror(why); exit(1); }
}
int main(int argc, char **argv)
{
	struct rlimit lim = {65536,65536};
	char a[32], b[32];
	pid_t pid;
	int rc;

	setvbuf(stdout, NULL, _IONBF, 0);
	if (!getuid()) { puts("SKIP: run as an unprivileged user without CAP_IPC_LOCK"); return 4; }
	if (argc == 3) {
		int first = atoi(argv[1]), second = atoi(argv[2]);
		printf("child page=%ld\n", sysconf(_SC_PAGESIZE));
		check(shmctl(second, SHM_LOCK, NULL) == 0, "cross-geometry second 32K lock");
		check(shmctl(first, SHM_UNLOCK, NULL) == 0, "cross-geometry unlock first");
		check(shmctl(first, SHM_LOCK, NULL) == 0, "32K budget returned");
		return 0;
	}
	atexit(cleanup);
	check(setrlimit(RLIMIT_MEMLOCK, &lim) == 0, "setrlimit");
	for (int i = 0; i < 2; i++) {
		ids[i] = shmget(IPC_PRIVATE, 32768, IPC_CREAT | 0600);
		check(ids[i] >= 0, "shmget");
	}
	check(shmctl(ids[0], SHM_LOCK, NULL) == 0, "first 32K lock");
	printf("parent page=%ld\n", sysconf(_SC_PAGESIZE));
	snprintf(a, sizeof(a), "%d", ids[0]); snprintf(b, sizeof(b), "%d", ids[1]);
	pid = fork(); check(pid >= 0, "fork");
	if (!pid) {
		int old = personality(0xffffffffUL);
		check(old >= 0 && personality((unsigned int)old ^ P3S_4K) >= 0, "toggle geometry");
		execl("/proc/self/exe", argv[0], a, b, NULL); _exit(1);
	}
	check(waitpid(pid, &rc, 0) == pid && WIFEXITED(rc) && WEXITSTATUS(rc) == 0, "child");
	check(shmctl(ids[0], SHM_UNLOCK, NULL) == 0 && shmctl(ids[1], SHM_UNLOCK, NULL) == 0, "final unlock");
	puts("PASS: shared per-user SHM_LOCK limit across mixed page sizes");
	return 0;
}
