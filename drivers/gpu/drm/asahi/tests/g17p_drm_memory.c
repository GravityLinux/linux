/* SPDX-License-Identifier: MIT */
/* Native Asahi memory-UAPI integration test; no GPU work is submitted.
 * Build: aarch64-linux-gnu-gcc -O2 -Wall -Wextra -Werror -static
 *        -I <headers_install>/include/drm -I <headers_install>/include
 *        -o g17p-drm-memory this-file.c
 * Run from a RAM initramfs on the explicitly selected development target.
 */
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>
#include "asahi_drm.h"

#define PAGE 0x4000ULL
#define VA 0x200000000ULL
#define EXEC 0x10000000000ULL
#define KSTART 0x30000000000ULL
#define RW (DRM_ASAHI_BIND_READ | DRM_ASAHI_BIND_WRITE)
static unsigned checks;
static void require(int condition, const char *what, int line)
{
	if (!condition) {
		fprintf(stderr, "FAIL line %d: %s (errno=%d %s)\n", line, what,
			errno, strerror(errno));
		exit(1);
	}
	checks++;
}
#define CHECK(c) require(!!(c), #c, __LINE__)
#define OK(fd, cmd, arg) CHECK(ioctl(fd, cmd, arg) == 0)
#define BAD(fd, cmd, arg, err) do { \
	errno = 0; int rc = ioctl(fd, cmd, arg); \
	CHECK(rc == -1 && errno == (err)); \
} while (0)

static uint32_t vm_new(int fd)
{
	struct drm_asahi_vm_create v = {
		.kernel_start = KSTART, .kernel_end = KSTART + 0x20000000,
	};
	OK(fd, DRM_IOCTL_ASAHI_VM_CREATE, &v);
	CHECK(v.vm_id != 0);
	return v.vm_id;
}
static void vm_destroy(int fd, uint32_t id, int error)
{
	struct drm_asahi_vm_destroy v = { .vm_id = id };
	if (error) BAD(fd, DRM_IOCTL_ASAHI_VM_DESTROY, &v, error);
	else OK(fd, DRM_IOCTL_ASAHI_VM_DESTROY, &v);
}
static uint32_t bo_new(int fd, uint64_t size, uint32_t flags, uint32_t vm)
{
	struct drm_asahi_gem_create c = { .size = size, .flags = flags, .vm_id = vm };
	OK(fd, DRM_IOCTL_ASAHI_GEM_CREATE, &c);
	CHECK(c.handle != 0);
	return c.handle;
}
static void bo_close(int fd, uint32_t handle)
{
	struct drm_gem_close c = { .handle = handle };
	OK(fd, DRM_IOCTL_GEM_CLOSE, &c);
}
static void *bo_map(int fd, uint32_t handle, size_t size)
{
	struct drm_asahi_gem_mmap_offset m = { .handle = handle };
	OK(fd, DRM_IOCTL_ASAHI_GEM_MMAP_OFFSET, &m);
	void *p = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, m.offset);
	CHECK(p != MAP_FAILED);
	return p;
}
static void bind(int fd, uint32_t vm, uint32_t bo, uint64_t va, uint64_t size,
		 uint64_t offset, uint32_t flags, int error)
{
	struct drm_asahi_gem_bind_op op = {
		.flags = flags, .handle = bo, .offset = offset, .addr = va, .range = size,
	};
	struct drm_asahi_vm_bind b = {
		.vm_id = vm, .num_binds = 1, .stride = sizeof(op), .userptr = (uintptr_t)&op,
	};
	if (error) BAD(fd, DRM_IOCTL_ASAHI_VM_BIND, &b, error);
	else OK(fd, DRM_IOCTL_ASAHI_VM_BIND, &b);
}

int main(int argc, char **argv)
{
	setbuf(stdout, NULL);
	const char *path = argc > 1 ? argv[1] : "/dev/dri/renderD128";
	int fd = open(path, O_RDWR | O_CLOEXEC), other = open(path, O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0 && other >= 0);
	char name[32] = {0};
	struct drm_version version = { .name_len = sizeof(name) - 1, .name = name };
	OK(fd, DRM_IOCTL_VERSION, &version);
	CHECK(!strcmp(name, "asahi"));
	struct drm_asahi_params_global p = {0};
	struct drm_asahi_get_params gp = { .size = sizeof(p), .pointer = (uintptr_t)&p };
	OK(fd, DRM_IOCTL_ASAHI_GET_PARAMS, &gp);
	CHECK(p.chip_id == 0x8140 && p.gpu_generation == 17 && p.gpu_variant == 'P');
	CHECK(p.num_dies == 1 && p.num_clusters_total == 1 && p.num_cores_per_cluster == 6);
	CHECK(p.core_masks[0] == 0x3d && p.max_frequency_khz == 1470000);
	CHECK(p.vm_start == PAGE && p.vm_end == (1ULL << 42) - 2 * PAGE);
	printf("PARAMS G%u%c rev=%x chip=%x mask=%" PRIx64 " max_khz=%u\n",
		p.gpu_generation, p.gpu_variant, p.gpu_revision, p.chip_id,
		(uint64_t)p.core_masks[0], p.max_frequency_khz);
	uint64_t short_query[2] = {0, UINT64_MAX};
	gp.pointer = (uintptr_t)short_query; gp.size = 8;
	OK(fd, DRM_IOCTL_ASAHI_GET_PARAMS, &gp);
	CHECK(short_query[0] == p.features && short_query[1] == UINT64_MAX);
	gp.pad = 1;
	BAD(fd, DRM_IOCTL_ASAHI_GET_PARAMS, &gp, EINVAL);
	gp.pad = 0; gp.pointer = 1;
	BAD(fd, DRM_IOCTL_ASAHI_GET_PARAMS, &gp, EFAULT);
	struct drm_asahi_get_time time = {0};
	OK(fd, DRM_IOCTL_ASAHI_GET_TIME, &time);
	uint64_t before = time.gpu_timestamp;
	usleep(20000);
	OK(fd, DRM_IOCTL_ASAHI_GET_TIME, &time);
	CHECK(time.gpu_timestamp - before >= 10000000 && time.gpu_timestamp - before < 1000000000);
	printf("COUNTER delta_ns=%" PRIu64 "\n", (uint64_t)time.gpu_timestamp - before);
	uint32_t vm = vm_new(fd), vm2 = vm_new(fd);
	uint32_t bo = bo_new(fd, 3 * PAGE, DRM_ASAHI_GEM_WRITEBACK, 0);
	struct drm_asahi_gem_mmap_offset missing = { .handle = bo };
	BAD(other, DRM_IOCTL_ASAHI_GEM_MMAP_OFFSET, &missing, ENOENT);
	uint64_t *a = bo_map(fd, bo, 3 * PAGE), *b = bo_map(fd, bo, 3 * PAGE);
	for (size_t i = 0; i < 3 * PAGE / 8; i++) a[i] = 0x4e454f0000000000ULL ^ (i * 0x10001);
	for (size_t i = 0; i < 3 * PAGE / 8; i++) CHECK(b[i] == (0x4e454f0000000000ULL ^ (i * 0x10001)));
	bind(fd, vm, bo, VA, 3 * PAGE, 0, RW, 0);
	bind(fd, vm, bo, VA, PAGE, 0, RW, EINVAL);
	bind(fd, vm, bo, KSTART, PAGE, 0, RW, EINVAL);
	bind(fd, vm, bo, VA + 1, PAGE, 0, RW, EINVAL);
	bind(fd, vm, bo, VA + 4 * PAGE, PAGE, 3 * PAGE, RW, EINVAL);
	vm_destroy(fd, vm, EBUSY);
	bind(fd, vm, 0, VA + PAGE, PAGE, 0, DRM_ASAHI_BIND_UNBIND, 0);
	bind(fd, vm, bo, VA + PAGE, PAGE, 2 * PAGE, RW, 0);
	bind(fd, vm, bo, VA, PAGE, 0, RW, EINVAL);
	bind(fd, vm, bo, VA + 2 * PAGE, PAGE, 0, RW, EINVAL);
	bind(fd, vm2, bo, VA, 3 * PAGE, 0, DRM_ASAHI_BIND_READ, 0);
	bind(fd, vm, bo, VA + 16 * PAGE, 8 * PAGE, PAGE, RW | DRM_ASAHI_BIND_SINGLE_PAGE, 0);
	bind(fd, vm, 0, VA + 18 * PAGE, 3 * PAGE, 0, DRM_ASAHI_BIND_UNBIND, 0);
	bind(fd, vm, bo, VA + 18 * PAGE, 3 * PAGE, 0, RW, 0);
	bind(fd, vm, bo, VA + 21 * PAGE, PAGE, 0, RW, EINVAL);
	uint32_t priv = bo_new(fd, PAGE, DRM_ASAHI_GEM_VM_PRIVATE, vm);
	bind(fd, vm2, priv, VA + 32 * PAGE, PAGE, 0, RW, EINVAL);
	bind(fd, vm, priv, VA + 32 * PAGE, PAGE, 0, RW, 0);
	struct drm_prime_handle prime = { .handle = priv, .flags = DRM_CLOEXEC | DRM_RDWR };
	BAD(fd, DRM_IOCTL_PRIME_HANDLE_TO_FD, &prime, EINVAL);
	prime.handle = bo;
	OK(fd, DRM_IOCTL_PRIME_HANDLE_TO_FD, &prime);
	struct drm_prime_handle imported = { .fd = prime.fd };
	OK(other, DRM_IOCTL_PRIME_FD_TO_HANDLE, &imported);
	uint64_t *shared = bo_map(other, imported.handle, 3 * PAGE);
	CHECK(shared[0] == a[0]); shared[1] = 0x12345678; CHECK(a[1] == 0x12345678);
	CHECK(munmap(shared, 3 * PAGE) == 0);
	bo_close(other, imported.handle);
	CHECK(close(prime.fd) == 0);
	/* Closing a handle on another file must preserve this file's mappings. */
	bind(fd, vm, bo, VA, PAGE, 0, RW, EINVAL);
	struct drm_asahi_queue_create q = { .vm_id = vm, .usc_exec_base = EXEC + (1ULL << 32) };
	BAD(fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q, EINVAL);
	q.usc_exec_base = EXEC; q.priority = 2;
	BAD(fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q, EINVAL);
	q.priority = 1;
	OK(fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q);
	struct drm_asahi_queue_destroy qd = { .queue_id = q.queue_id };
	OK(fd, DRM_IOCTL_ASAHI_QUEUE_DESTROY, &qd);
	BAD(fd, DRM_IOCTL_ASAHI_QUEUE_DESTROY, &qd, ENOENT);
	struct drm_asahi_gem_bind_object obj = {
		.op = DRM_ASAHI_BIND_OBJECT_OP_BIND, .flags = DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,
		.handle = bo, .offset = PAGE, .range = PAGE,
	};
	OK(fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &obj);
	struct drm_asahi_gem_bind_object unobj = {
		.op = DRM_ASAHI_BIND_OBJECT_OP_UNBIND, .object_handle = obj.object_handle,
	};
	OK(fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &unobj);
	BAD(fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &unobj, ENOENT);
	OK(fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &obj);
	uint32_t sync_handle;
	struct drm_syncobj_create sync = {0};
	OK(fd, DRM_IOCTL_SYNCOBJ_CREATE, &sync); sync_handle = sync.handle;
	struct drm_syncobj_array sa = { .handles = (uintptr_t)&sync_handle, .count_handles = 1 };
	OK(fd, DRM_IOCTL_SYNCOBJ_SIGNAL, &sa);
	struct drm_syncobj_wait sw = { .handles = (uintptr_t)&sync_handle, .count_handles = 1 };
	OK(fd, DRM_IOCTL_SYNCOBJ_WAIT, &sw);
	struct drm_syncobj_destroy sd = { .handle = sync_handle };
	OK(fd, DRM_IOCTL_SYNCOBJ_DESTROY, &sd);
	/* This milestone must fail submission explicitly. No fabricated completion. */
	struct drm_asahi_submit submit = {0};
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EOPNOTSUPP);
	if (argc > 2) {
		printf("G17P_UAT_AUDIT_READY vm=%u vm2=%u; press Enter to finish\n", vm, vm2);
		CHECK(getchar() != EOF);
	}
	bo_close(fd, priv);
	bo_close(fd, bo);
	unobj.object_handle = obj.object_handle;
	BAD(fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &unobj, ENOENT);
	CHECK(a[1] == 0x12345678 && b[1] == 0x12345678);
	CHECK(munmap(a, 3 * PAGE) == 0 && munmap(b, 3 * PAGE) == 0);
	vm_destroy(fd, vm, 0); vm_destroy(fd, vm2, 0);
	vm_destroy(fd, vm, ENOENT);
	/* Exercise teardown with outstanding roots, mappings, queue and GEM handle. */
	vm = vm_new(other);
	bo = bo_new(other, PAGE + 1, 0, 0);
	a = bo_map(other, bo, 2 * PAGE);
	a[0] = 0xa18; a[PAGE / 8] = 0x8140;
	bind(other, vm, bo, VA, 2 * PAGE, 0, RW, 0);
	q = (struct drm_asahi_queue_create) { .vm_id = vm, .usc_exec_base = EXEC };
	OK(other, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q);
	CHECK(close(other) == 0);
	CHECK(a[0] == 0xa18 && a[PAGE / 8] == 0x8140);
	CHECK(munmap(a, 2 * PAGE) == 0);
	CHECK(close(fd) == 0);
	printf("G17P_DRM_MEMORY_PASS checks=%u; native memory UAPI only; GPU submit unsupported\n", checks);
	return 0;
}
