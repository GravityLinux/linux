/* SPDX-License-Identifier: GPL-2.0-only */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <linux/perf_event.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

static void check(int ok, const char *what)
{
	if (!ok) { fprintf(stderr, "FAIL: %s (errno=%d)\n", what, errno); exit(1); }
}

static unsigned char pattern(size_t i)
{
	return (i * 17 + 53) & 255;
}

static void resident(int fd, char *p, size_t length, size_t page)
{
	uint64_t entry;

	for (size_t i = 0; i < length; i += page) {
		off_t offset = ((uintptr_t)(p + i) / page) * sizeof(entry);

		check(pread(fd, &entry, sizeof(entry), offset) == sizeof(entry), "pagemap read");
		check(!!(entry & (1ULL << 63)), "neighbor PTE still present without refault");
	}
}

/*
 * Map a self-monitoring perf ring natively or from a 4K process (exec-4k).
 * Perf buffers are kernel pages: a 4K process maps them in 16K multiples at
 * file offset 0. Each attempt sits between shared-file neighbours whose
 * dirty PTEs must survive both a successful map and an error unwind.
 */
static int try_map(int perf, char *base, size_t length, off_t offset,
		   char *neighbor, size_t neighbor_length, int pagemap, size_t page,
		   char **mapped)
{
	int saved_errno;

	*mapped = mmap(base, length, PROT_READ | PROT_WRITE, MAP_FIXED | MAP_SHARED,
		       perf, offset);
	saved_errno = *mapped == MAP_FAILED ? errno : 0;
	/* Check before touching the bytes: a refault can hide an illicit zap. */
	resident(pagemap, neighbor, neighbor_length, page);
	for (size_t i = 0; i < neighbor_length; i++)
		check((unsigned char)neighbor[i] == pattern(i), "neighbor bytes unchanged");
	return saved_errno;
}

int main(int argc, char **argv)
{
	struct perf_event_attr attr = {
		.size = sizeof(attr), .type = PERF_TYPE_SOFTWARE,
		.config = PERF_COUNT_SW_CPU_CLOCK, .disabled = 1,
		.exclude_kernel = 1, .exclude_hv = 1,
	};
	size_t page = sysconf(_SC_PAGESIZE), native = 16384;
	size_t length = 2 * native, neighbor_length = 3 * length;
	size_t reserve = 4 * length + native;
	char *raw, *base, *neighbor, *mapped;
	struct perf_event_mmap_page *user;
	unsigned char buf[4096];
	int perf, file, pagemap, err;

	check(argc == 2, "usage: test-perf-mmap NEW-SCRATCH-FILE");
	perf = syscall(__NR_perf_event_open, &attr, 0, -1, -1, PERF_FLAG_FD_CLOEXEC);
	if (perf < 0 && (errno == EACCES || errno == EPERM || errno == ENOSYS)) {
		printf("SKIP: self-monitoring software perf event unavailable (errno=%d)\n", errno);
		return 4;
	}
	check(perf >= 0, "open self-monitoring software perf event");
	file = open(argv[1], O_CREAT | O_EXCL | O_RDWR, 0600);
	check(file >= 0, "create scratch file");
	check(ftruncate(file, neighbor_length) == 0 && fsync(file) == 0, "initial file zeros");
	raw = mmap(NULL, reserve, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	check(raw != MAP_FAILED, "reserve test range");
	base = (void *)(((uintptr_t)raw + native - 1) & ~(uintptr_t)(native - 1));
	neighbor = base + length;
	check(mmap(neighbor, neighbor_length, PROT_READ | PROT_WRITE,
		   MAP_FIXED | MAP_SHARED, file, 0) == neighbor, "shared file neighbor");
	/* Leave dirty data in PTEs until after perf's potential error unwind. */
	for (size_t i = 0; i < neighbor_length; i++)
		neighbor[i] = pattern(i);
	pagemap = open("/proc/self/pagemap", O_RDONLY);
	check(pagemap >= 0, "open self pagemap");
	resident(pagemap, neighbor, neighbor_length, page);

	if (page == 4096) {
		/* Not a whole number of kernel pages. */
		err = try_map(perf, base, length - 4096, 0, neighbor, neighbor_length,
			      pagemap, page, &mapped);
		check(err == EINVAL, "4K perf mmap of 28K rejected");
		err = try_map(perf, base, 4096, 0, neighbor, neighbor_length,
			      pagemap, page, &mapped);
		check(err == EINVAL, "4K perf mmap of 4K rejected");
		/* A non-zero slice offset is rejected. */
		err = try_map(perf, base, length, 4096, neighbor, neighbor_length,
			      pagemap, page, &mapped);
		check(err == EINVAL, "4K perf mmap at slice offset rejected");
		printf("rejected: 28K, 4K and slice-offset 4K perf mmaps (EINVAL)\n");
	}
	err = try_map(perf, base, length, 0, neighbor, neighbor_length, pagemap, page,
		      &mapped);
	errno = err;
	check(!err && mapped == base, "perf ring mmap succeeds");
	user = (void *)mapped;
	/* The ring's control page is one kernel page, then one data page. */
	check(user->data_offset == native && user->data_size == native,
	      "perf ring layout in kernel pages");
	/* Every slice of both kernel pages is mapped. */
	for (size_t i = 0; i < length; i += page)
		(void)*(volatile char *)(mapped + i);
	check(munmap(mapped, length) == 0, "unmap perf ring");
	for (size_t i = 0; i < neighbor_length; i++)
		check((unsigned char)neighbor[i] == pattern(i), "neighbor bytes after unmap");
	check(msync(neighbor, neighbor_length, MS_SYNC) == 0 && fsync(file) == 0, "neighbor writeback");
	check(munmap(raw, reserve) == 0, "unmap test range");
	check(posix_fadvise(file, 0, 0, POSIX_FADV_DONTNEED) == 0, "evict neighbor cache");
	for (size_t i = 0; i < neighbor_length; i += sizeof(buf)) {
		check(pread(file, buf, sizeof(buf), i) == sizeof(buf), "neighbor file readback");
		for (size_t j = 0; j < sizeof(buf); j++)
			check(buf[j] == pattern(i + j), "neighbor writeback preserved");
	}
	check(close(pagemap) == 0 && close(file) == 0 && close(perf) == 0, "close test fds");
	printf("PASS: pages=%zu perf ring mapped (data_offset=%llu); adjacent shared-file PTEs/bytes/writeback intact\n",
	       page, (unsigned long long)native);
	return 0;
}
