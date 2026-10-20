/* SPDX-License-Identifier: GPL-2.0-only */
#define _GNU_SOURCE
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

extern const unsigned char slice_code[], slice_code_end[];
#if defined(__aarch64__)
asm(".text\n.global slice_code,slice_code_end\nslice_code:\n"
    "bti c\nmov w0,#42\nret\nslice_code_end:\n");
#elif defined(__x86_64__)
asm(".text\n.global slice_code,slice_code_end\nslice_code:\n"
    "mov $42,%eax\nret\nslice_code_end:\n");
#endif

static void check(int ok, const char *what)
{
	if (!ok) { perror(what); exit(1); }
}

int main(void)
{
	struct rlimit core = {0, 0};
	unsigned char block[16384], *raw, *p, *q;
	size_t code_size = slice_code_end - slice_code;
	int fd, status;
	pid_t pid;

	setvbuf(stdout, NULL, _IONBF, 0);
	setrlimit(RLIMIT_CORE, &core);
	check(sysconf(_SC_PAGESIZE) == 4096, "4K process");
	memset(block, 0x31, sizeof(block));
	memcpy(block + 4096, slice_code, code_size);
	fd = memfd_create("exec-slice", 0);
	check(fd >= 0 && write(fd, block, sizeof(block)) == sizeof(block), "file");
	raw = mmap(NULL, 65536, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	check(raw != MAP_FAILED, "reserve");
	p = (void *)(((uintptr_t)raw + 16383) & ~(uintptr_t)16383);
	/* ELF-like incongruence: VA quarter3, file quarter1. */
	q = mmap(p + 12288, 4096, PROT_READ | PROT_EXEC,
		 MAP_FIXED | MAP_PRIVATE, fd, 4096);
	check(q == p + 12288, "file RX");
	check(((int (*)(void))q)() == 42, "file RX execute");
	check(mprotect(q, 4096, PROT_READ | PROT_WRITE) == 0, "RX to RW");
	memcpy(q, slice_code, code_size); /* private-file COW */
	q[64] = 0x77;
	check(mprotect(q, 4096, PROT_READ | PROT_EXEC) == 0, "RW to RX");
	__builtin___clear_cache((char *)q, (char *)q + code_size);
	check(((int (*)(void))q)() == 42 && q[64] == 0x77, "COW RX execute");
	check(pread(fd, block, sizeof(block), 0) == sizeof(block) && block[4096+64] == 0x31,
	      "COW preserves backing file");
	check(mmap(p + 8192, 4096, PROT_READ | PROT_WRITE,
		   MAP_FIXED | MAP_PRIVATE, fd, 4096) == p + 8192, "adjacent alias");
	check(p[8192+64] == 0x31, "alias initial bytes");
	p[8192+64] = 0x55;
	check(q[64] == 0x77, "private aliases independent");
	check(mprotect(p + 8192, 4096, PROT_NONE) == 0, "neighbor NONE");
	check(((int (*)(void))q)() == 42, "neighbor protection preserves RX");
	check(mprotect(q, 4096, PROT_READ) == 0, "remove execute");
	pid = fork();
	check(pid >= 0, "fork NX probe");
	if (!pid) { ((int (*)(void))q)(); _exit(2); }
	check(waitpid(pid, &status, 0) == pid && WIFSIGNALED(status) &&
	      WTERMSIG(status) == SIGSEGV, "NX fault");
	check(mprotect(q, 4096, PROT_READ | PROT_EXEC) == 0, "restore RX");
	check(((int (*)(void))q)() == 42, "restored execute");
	puts("PASS: incongruent 4K file RX, private COW, adjacent protection, NX and restored RX");
	return 0;
}
