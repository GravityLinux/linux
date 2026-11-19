// SPDX-License-Identifier: GPL-2.0-only
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void check(int ok, const char *why)
{
	if (!ok) { perror(why); exit(1); }
}
int main(int argc, char **argv)
{
	char buf[65536] = {0};
	int fd;
	ssize_t n;

	setvbuf(stdout, NULL, _IONBF, 0);
	check(argc == 3, "usage: test-reclaim create|drop|fill|reclaim FILE");
	if (!strcmp(argv[1], "reclaim")) {
		fd = open(argv[2], O_WRONLY); check(fd >= 0, "memory.reclaim open");
		n = write(fd, "16777216\n", 9);
		check(n == 9 || (n < 0 && errno == EAGAIN), "memory.reclaim write");
		printf("reclaim writer page=%ld result=%zd errno=%d\n", sysconf(_SC_PAGESIZE), n, n < 0 ? errno : 0);
		close(fd); return 0;
	}
	fd = open(argv[2], !strcmp(argv[1], "create") ? O_CREAT | O_TRUNC | O_RDWR : O_RDONLY, 0600);
	check(fd >= 0, "cache file open");
	if (!strcmp(argv[1], "create")) {
		for (int i = 0; i < 1024; i++) check(write(fd, buf, sizeof(buf)) == sizeof(buf), "write file");
		check(fsync(fd) == 0, "fsync");
	} else if (!strcmp(argv[1], "drop")) {
		int err = posix_fadvise(fd, 0, 0, POSIX_FADV_DONTNEED);
		errno = err; check(!err, "fadvise");
	} else {
		check(!strcmp(argv[1], "fill"), "mode");
		do { n = read(fd, buf, sizeof(buf)); check(n >= 0, "read cache"); } while (n);
		puts("READY: 64MiB file cache populated");
		for (;;) pause();
	}
	close(fd); return 0;
}
