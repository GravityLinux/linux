/* SPDX-License-Identifier: GPL-2.0-only */
#define _GNU_SOURCE
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>
#define LENGTH (1UL << 20)
static void check(int ok, const char *what)
{
	if (!ok) { perror(what); exit(1); }
}
int main(int argc, char **argv)
{
	struct rlimit core = {0, 0};
	unsigned char *p;
	int fd, status;
	check(argc >= 3, "mode/file arguments");
	setrlimit(RLIMIT_CORE, &core);
	fd = open(argv[2], O_RDWR);
	check(fd >= 0, "open scratch file");
	p = mmap(NULL, LENGTH, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	check(p != MAP_FAILED, "shared writable mapping");
	printf("mode=%s pages=%ld\n", argv[1], sysconf(_SC_PAGESIZE));
	if (!strcmp(argv[1], "--cycle")) {
		/* Keep PTEs installed across writeback/re-protection cycles. */
		for (unsigned int cycle = 1; cycle <= 4; cycle++) {
			for (size_t i = 0; i < LENGTH; i++)
				p[i] = (i * 17 + cycle * 13) & 255;
			check(msync(p, LENGTH, MS_SYNC) == 0 && fsync(fd) == 0, "cycle writeback");
		}
	} else if (!strcmp(argv[1], "--mixed")) {
		int lane;
		check(argc == 4, "mixed lane");
		lane = atoi(argv[3]);
		check(lane == 0 || lane == 1, "lane value");
		/* Distinct quarters; both mms dirty every native folio. */
		for (unsigned int cycle = 1; cycle <= 20; cycle++) {
			for (size_t i = 0; i < LENGTH; i++)
				if ((i / 4096) % 2 == (unsigned int)lane)
					p[i] = (i * 7 + cycle * 11 + lane * 31) & 255;
			check(msync(p, LENGTH, MS_SYNC) == 0 && fsync(fd) == 0, "mixed writeback");
			usleep(5000);
		}
	} else if (!strcmp(argv[1], "--truncate")) {
		int ready[2], go[2];
		pid_t child;
		char byte;
		/* Resident PTEs must be revoked, not merely new faults. */
		p[32768] = 0x52;
		check(msync(p, LENGTH, MS_SYNC) == 0, "before truncate");
		check(pipe(ready) == 0 && pipe(go) == 0, "truncate pipes");
		child = fork();
		check(child >= 0, "truncate fork");
		if (!child) {
			close(ready[0]); close(go[1]);
			check(write(ready[1], "r", 1) == 1 && read(go[0], &byte, 1) == 1, "truncate sync");
			byte = ((volatile unsigned char *)p)[32768];
			_exit(byte ? 2 : 3);
		}
		close(ready[1]); close(go[0]);
		check(read(ready[0], &byte, 1) == 1 && ftruncate(fd, 16384) == 0 &&
		      fsync(fd) == 0 && write(go[1], "g", 1) == 1, "truncate resident file");
		check(waitpid(child, &status, 0) == child && WIFSIGNALED(status) &&
		      WTERMSIG(status) == SIGBUS, "SIGBUS beyond truncated EOF");
	} else if (!strcmp(argv[1], "--slice")) {
		char *raw, *q;
		check(sysconf(_SC_PAGESIZE) == 4096, "4K slice mode");
		raw = mmap(NULL, 65536, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
		check(raw != MAP_FAILED, "slice reserve");
		q = (void *)((((uintptr_t)raw + 16383) & ~(uintptr_t)16383) + 4096);
		check(mmap(q, 4096, PROT_READ | PROT_WRITE, MAP_FIXED | MAP_SHARED,
			   fd, 12288) == q, "incongruent slice");
		memset(q, 0xab, 4096);
		check(msync(q, 4096, MS_SYNC) == 0 && fsync(fd) == 0, "slice writeback");
		check(munmap(raw, 65536) == 0, "slice unmap");
	} else if (!strcmp(argv[1], "--hole-punch")) {
		const size_t holes[][2] = {{4096, 8192}, {28672, 8192}, {65536, 16384}};

		for (size_t i = 0; i < LENGTH; i++)
			p[i] = (i * 17 + 13) & 255;
		check(msync(p, LENGTH, MS_SYNC) == 0 && fsync(fd) == 0, "before punch");
		/* Partial folios, a native-page crossing, and a full native page. */
		for (size_t h = 0; h < sizeof(holes) / sizeof(holes[0]); h++)
			check(fallocate(fd, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
					holes[h][0], holes[h][1]) == 0, "punch resident file");
		for (size_t i = 0; i < LENGTH; i++) {
			unsigned char want = (i * 17 + 13) & 255;

			for (size_t h = 0; h < sizeof(holes) / sizeof(holes[0]); h++)
				if (i >= holes[h][0] && i < holes[h][0] + holes[h][1])
					want = 0;
			check(p[i] == want, "resident punch zeroing and untouched siblings");
		}
		check(msync(p, LENGTH, MS_SYNC) == 0 && fsync(fd) == 0, "punch writeback");
	} else if (!strcmp(argv[1], "--hole-rewrite")) {
		memset(p + 4096, 0xa6, 8192);
		memset(p + 28672, 0xa6, 8192);
		memset(p + 65536, 0xa6, 16384);
		check(msync(p, LENGTH, MS_SYNC) == 0 && fsync(fd) == 0, "rewrite punched holes");
	} else if (!strcmp(argv[1], "--partial-eof")) {
		size_t end, page = sysconf(_SC_PAGESIZE), rounded;

		check(argc == 4, "partial EOF length");
		end = strtoul(argv[3], NULL, 0);
		check(end > 32768 && end < 49152, "EOF inside a native 16K page");
		rounded = (end + page - 1) & ~(page - 1);
		for (size_t i = 0; i < LENGTH; i++)
			p[i] = (i * 17 + 39) & 255;
		check(msync(p, LENGTH, MS_SYNC) == 0 && fsync(fd) == 0, "before partial truncate");
		check(ftruncate(fd, end) == 0 && fsync(fd) == 0, "partial native-page EOF");
		for (size_t i = 0; i < end; i++)
			check(p[i] == ((i * 17 + 39) & 255), "valid EOF prefix retained");
		for (size_t i = end; i < rounded; i++)
			check(p[i] == 0, "partial logical-page EOF tail zeroed");
		/* Only the last logical page's tail is legally accessible. */
		memset(p + end, 0xce, rounded - end);
		p[end - 1] = 0x6d;
		check(msync(p, rounded, MS_SYNC) == 0 && fsync(fd) == 0, "partial EOF writeback");
	} else {
		check(0, "unknown mode");
	}
	check(munmap(p, LENGTH) == 0 && close(fd) == 0, "unmap/close");
	puts("PASS: mapped writes/writeback, punch or truncate checks");
	return 0;
}
