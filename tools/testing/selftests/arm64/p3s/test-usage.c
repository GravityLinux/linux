// SPDX-License-Identifier: GPL-2.0-only
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/personality.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

#define P3S_4K 0x40000000UL
#define TOL (8UL << 20)
static void check(int ok, const char *why)
{
	if (!ok) { perror(why); exit(1); }
}
static unsigned long status(pid_t pid, const char *key)
{
	char path[64], line[256], name[64];
	unsigned long n;
	FILE *f;

	snprintf(path, sizeof(path), "/proc/%d/status", pid);
	f = fopen(path, "r"); check(f != NULL, path);
	while (fgets(line, sizeof(line), f)) {
		if (sscanf(line, "%63s %lu", name, &n) == 2 && !strcmp(name, key)) {
			fclose(f); return n << 10;
		}
	}
	fclose(f); check(0, key); return 0;
}
static void near(unsigned long a, unsigned long b, unsigned long tol, const char *why)
{
	printf("%s: %lu / %lu bytes (tolerance %lu)\n", why, a, b, tol);
	check(a > b ? a - b <= tol : b - a <= tol, why);
}
static void observe(pid_t pid)
{
	char path[64], line[4096], *p, *save;
	unsigned long a[7], rss = 0, page = sysconf(_SC_PAGESIZE);
	FILE *f;

	snprintf(path, sizeof(path), "/proc/%d/statm", pid);
	f = fopen(path, "r"); check(f != NULL, path);
	check(fscanf(f, "%lu %lu %lu %lu %lu %lu %lu", &a[0], &a[1], &a[2], &a[3],
		     &a[4], &a[5], &a[6]) == 7, "statm fields"); fclose(f);
	near(a[0] * page, status(pid, "VmSize:"), page, "statm size");
	near(a[1] * page, status(pid, "VmRSS:"), TOL, "statm RSS");
	near(a[2] * page, status(pid, "RssFile:") + status(pid, "RssShmem:"), page, "statm shared");
	near(a[3] * page, status(pid, "VmExe:"), page, "statm text");
	near(a[5] * page, status(pid, "VmData:") + status(pid, "VmStk:"), page, "statm data");
	snprintf(path, sizeof(path), "/proc/%d/stat", pid);
	f = fopen(path, "r"); check(f != NULL, path);
	check(fgets(line, sizeof(line), f) != NULL, "stat read"); fclose(f);
	p = strrchr(line, ')'); check(p != NULL, "stat comm");
	for (int field = 3; (p = strtok_r(field == 3 ? p + 1 : NULL, " \n", &save)); field++)
		if (field == 24) { rss = strtoul(p, NULL, 10); break; }
	near(rss * page, status(pid, "VmRSS:"), TOL, "stat RSS");
}
/* The parent is blocked in wait4 for this observer: no lazy text faults. */
static void wait_blocked(pid_t pid)
{
	char path[64], line[4096], *end;
	FILE *f;

	snprintf(path, sizeof(path), "/proc/%d/stat", pid);
	for (int i = 0; i < 1000; i++) {
		f = fopen(path, "r"); check(f != NULL, "parent stat");
		check(fgets(line, sizeof(line), f) != NULL, "parent state");
		fclose(f); end = strrchr(line, ')');
		if (end && end[1] == ' ' && end[2] == 'S') return;
		usleep(1000);
	}
	check(0, "parent did not enter wait4");
}
static void peak(int who, unsigned long expected)
{
	struct rusage r;

	check(getrusage(who, &r) == 0, "getrusage");
	near(r.ru_maxrss * 1024UL, expected, TOL, "ru_maxrss");
}
int main(int argc, char **argv)
{
	const size_t size = 64UL << 20;
	unsigned long expected, page = sysconf(_SC_PAGESIZE);
	unsigned char *p;
	char pidstr[32], expstr[32];
	pid_t child;
	int rc;
	struct rusage r;

	setvbuf(stdout, NULL, _IONBF, 0);
	if (argc == 4 && !strcmp(argv[1], "--observe")) {
		printf("reader page=%lu target=%s\n", page, argv[2]);
		wait_blocked(atoi(argv[2]));
		observe(atoi(argv[2]));
		peak(RUSAGE_SELF, strtoul(argv[3], NULL, 10));
		return 0;
	}
	p = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	check(p != MAP_FAILED, "mmap");
	(void)madvise(p, size, MADV_NOHUGEPAGE);
	for (size_t i = 0; i < size; i += page) p[i] = 0x73;
	expected = status(getpid(), "VmHWM:");
	peak(RUSAGE_SELF, expected); peak(RUSAGE_THREAD, expected);
	snprintf(pidstr, sizeof(pidstr), "%d", getpid());
	snprintf(expstr, sizeof(expstr), "%lu", expected);
	for (int four = 0; four < 2; four++) {
		child = fork(); check(child >= 0, "fork");
		if (!child) {
			int old = personality(0xffffffffUL);
			if (old < 0 || personality(four ? (unsigned int)old | P3S_4K :
						 (unsigned int)old & ~P3S_4K) < 0) {
				if (four) { puts("SKIP: 4K personality unavailable on host"); _exit(4); }
				_exit(1);
			}
			execl("/proc/self/exe", argv[0], "--observe", pidstr, expstr, NULL);
			_exit(1);
		}
		check(wait4(child, &rc, 0, &r) == child, "wait4");
		check(WIFEXITED(rc) && (WEXITSTATUS(rc) == 0 || WEXITSTATUS(rc) == 4), "observer");
		if (WEXITSTATUS(rc) == 0) near(r.ru_maxrss * 1024UL, expected, TOL, "wait4 peak after cross-geometry exec");
	}
	peak(RUSAGE_CHILDREN, expected);
	check(munmap(p, size) == 0, "munmap");
	peak(RUSAGE_SELF, expected);
	puts("PASS: proc reader units, peak after unmap/exec, wait4 and child maxima");
	return 0;
}
