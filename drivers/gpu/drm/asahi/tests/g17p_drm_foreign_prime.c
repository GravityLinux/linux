/* SPDX-License-Identifier: GPL-2.0-only */
/* Foreign PRIME import: DCP dumb BO -> GPU GEM/CPU mapping/UAT, including
 * lifetime after closing every exporter handle. No GPU job is submitted.
 * Build with headers_install UAPI: -static -I include/drm -I include
 */
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>
#include "asahi_drm.h"
#include "drm_mode.h"

static unsigned checks;
#define CHECK(c) do { if (!(c)) { \
 fprintf(stderr, "FOREIGN_PRIME_FAIL line=%d %s errno=%d %s\n", \
 __LINE__, #c, errno, strerror(errno)); exit(1); } checks++; } while (0)
#define OK(fd, cmd, arg) CHECK(ioctl(fd, cmd, arg) == 0)

static void close_bo(int fd, uint32_t handle)
{
 struct drm_gem_close c = { .handle = handle };
 OK(fd, DRM_IOCTL_GEM_CLOSE, &c);
}

int main(int argc, char **argv)
{
 setbuf(stdout, NULL);
 CHECK(argc == 3);
 int gpu = open(argv[1], O_RDWR | O_CLOEXEC);
 int dcp = open(argv[2], O_RDWR | O_CLOEXEC);
 CHECK(gpu >= 0 && dcp >= 0);
 char name[32] = {0};
 struct drm_version version = { .name = name, .name_len = sizeof(name)-1 };
 OK(dcp, DRM_IOCTL_VERSION, &version);
 CHECK(!strcmp(name, "apple"));
 struct drm_asahi_params_global params = {0};
 struct drm_asahi_get_params get = { .size = sizeof(params), .pointer = (uintptr_t)&params };
 OK(gpu, DRM_IOCTL_ASAHI_GET_PARAMS, &get);
 CHECK(params.chip_id == 0x6041 || params.chip_id == 0x8140);

 for (unsigned iteration = 0; iteration < 8; iteration++) {
  struct drm_mode_create_dumb dumb = { .width = 64, .height = 64, .bpp = 32 };
  OK(dcp, DRM_IOCTL_MODE_CREATE_DUMB, &dumb);
  CHECK(dumb.size == 0x4000);
  struct drm_mode_map_dumb dm = { .handle = dumb.handle };
  OK(dcp, DRM_IOCTL_MODE_MAP_DUMB, &dm);
  uint32_t *source = mmap(NULL, dumb.size, PROT_READ | PROT_WRITE, MAP_SHARED, dcp, dm.offset);
  CHECK(source != MAP_FAILED);
  for (unsigned i = 0; i < dumb.size / 4; i++) source[i] = i ^ (iteration << 24) ^ 0x136ac795;
  __atomic_thread_fence(__ATOMIC_SEQ_CST);
  struct drm_prime_handle prime = { .handle = dumb.handle, .flags = DRM_CLOEXEC | DRM_RDWR };
  OK(dcp, DRM_IOCTL_PRIME_HANDLE_TO_FD, &prime);
  struct drm_prime_handle imported = { .fd = prime.fd };
  OK(gpu, DRM_IOCTL_PRIME_FD_TO_HANDLE, &imported);
  CHECK(imported.handle != 0);
  struct drm_asahi_gem_mmap_offset gm = { .handle = imported.handle };
  OK(gpu, DRM_IOCTL_ASAHI_GEM_MMAP_OFFSET, &gm);
  uint32_t *view = mmap(NULL, dumb.size, PROT_READ | PROT_WRITE, MAP_SHARED, gpu, gm.offset);
  CHECK(view != MAP_FAILED);
  for (unsigned i = 0; i < dumb.size / 4; i++) CHECK(view[i] == source[i]);
  view[7] ^= 0xffffffff;
  __atomic_thread_fence(__ATOMIC_SEQ_CST);
  CHECK(view[7] == source[7]);
  struct drm_asahi_vm_create vm = { .kernel_start = 0x30000000000ULL, .kernel_end = 0x30020000000ULL };
  OK(gpu, DRM_IOCTL_ASAHI_VM_CREATE, &vm);
  struct drm_asahi_gem_bind_op op = {
   .flags = DRM_ASAHI_BIND_READ | DRM_ASAHI_BIND_WRITE,
   .handle = imported.handle, .addr = 0x200000000ULL, .range = dumb.size,
  };
  struct drm_asahi_vm_bind bind = {
   .vm_id = vm.vm_id, .num_binds = 1, .stride = sizeof(op), .userptr = (uintptr_t)&op,
  };
  OK(gpu, DRM_IOCTL_ASAHI_VM_BIND, &bind);
  CHECK(munmap(view, dumb.size) == 0);
  CHECK(munmap(source, dumb.size) == 0);
  close_bo(dcp, dumb.handle);
  CHECK(close(prime.fd) == 0);
  view = mmap(NULL, dumb.size, PROT_READ | PROT_WRITE, MAP_SHARED, gpu, gm.offset);
  CHECK(view != MAP_FAILED);
  CHECK(view[7] == ((7 ^ (iteration << 24) ^ 0x136ac795) ^ 0xffffffffU));
  op.flags = DRM_ASAHI_BIND_UNBIND; op.handle = 0;
  OK(gpu, DRM_IOCTL_ASAHI_VM_BIND, &bind);
  struct drm_asahi_vm_destroy destroy = { .vm_id = vm.vm_id };
  OK(gpu, DRM_IOCTL_ASAHI_VM_DESTROY, &destroy);
  CHECK(munmap(view, dumb.size) == 0);
  close_bo(gpu, imported.handle);
 }
 close(gpu); close(dcp);
 printf("FOREIGN_PRIME_PASS iterations=8 checks=%u; CPU sharing, UAT bind, exporter-close lifetime\n", checks);
 return 0;
}
