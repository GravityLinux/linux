/* SPDX-License-Identifier: MIT */
/* Real GEM/VM/QUEUE/SUBMIT path, with an authored add3 program and independent
 * expected output. No firmware objects are supplied by this test. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include "g17p_drm_workload.h"
#include "g17p_drm_sync.h"
#include "g17p_drm_timestamp.h"

static uint32_t workload_bind(int fd, uint32_t vm, uint64_t address,
			  const void *data, size_t size, int writable)
{
	uint32_t bo = bo_new(fd, size, DRM_ASAHI_GEM_WRITEBACK, 0);
	void *map = bo_map(fd, bo, size);
	memcpy(map, data, size);
	CHECK(munmap(map, size) == 0);
	bind(fd, vm, bo, address, size, 0,
	     DRM_ASAHI_BIND_READ | (writable ? DRM_ASAHI_BIND_WRITE : 0), 0);
	return bo;
}

int main(int argc, char **argv)
{
	int use_sync = 0, use_timestamps = 0;
	for (int i = 1; i < argc; i++) {
		if (!strcmp(argv[i], "--sync")) use_sync = 1;
		else if (!strcmp(argv[i], "--timestamps")) use_timestamps = 1;
		else CHECK(0);
	}
	struct sync_test sync = {0};
	struct timestamp_test timestamps = {0};
	setbuf(stdout, NULL);
	int fd = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	uint32_t vm = vm_new(fd);
	struct drm_asahi_queue_create q = { .vm_id = vm, .usc_exec_base = EXEC };
	OK(fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q);
	for (size_t i = 0; i < sizeof(workloads) / sizeof(workloads[0]); i++) {
		const struct workload *w = &workloads[i];
		workload_bind(fd, vm, w->address, w->data, w->size, w->writable);
	}
	float *values = calloc(1, PAGE);
	CHECK(values != NULL);
	for (unsigned i = 0; i < 64; i++) values[i] = 1000.0f + i;
	uint32_t input_a = workload_bind(fd, vm, COMPUTE_INPUT_A, values, PAGE, 1);
	for (unsigned i = 0; i < 64; i++) values[i] = 0.5f;
	uint32_t input_b = workload_bind(fd, vm, COMPUTE_INPUT_B, values, PAGE, 1);
	free(values);
	float *a = bo_map(fd, input_a, PAGE), *b = bo_map(fd, input_b, PAGE);
	uint32_t output = bo_new(fd, PAGE, DRM_ASAHI_GEM_WRITEBACK, 0);
	float *result = bo_map(fd, output, PAGE);
	memset(result, 0xa5, PAGE);
	bind(fd, vm, output, COMPUTE_OUTPUT, PAGE, 0, RW, 0);
	struct {
		struct drm_asahi_cmd_header attachment_header;
		struct drm_asahi_attachment attachment;
		struct drm_asahi_cmd_header compute_header;
		struct drm_asahi_cmd_compute compute;
	} commands = {
		.attachment_header = { .cmd_type = DRM_ASAHI_SET_COMPUTE_ATTACHMENTS,
			.size = sizeof(struct drm_asahi_attachment),
			.vdm_barrier = DRM_ASAHI_BARRIER_NONE, .cdm_barrier = DRM_ASAHI_BARRIER_NONE },
		.attachment = { .pointer = COMPUTE_OUTPUT, .size = PAGE },
		.compute_header = { .cmd_type = DRM_ASAHI_CMD_COMPUTE,
			.size = sizeof(struct drm_asahi_cmd_compute),
			.vdm_barrier = DRM_ASAHI_BARRIER_NONE, .cdm_barrier = DRM_ASAHI_BARRIER_NONE },
		.compute = { .cdm_ctrl_stream_base = COMPUTE_CDM,
			.cdm_ctrl_stream_end = COMPUTE_CDM + COMPUTE_CDM_SIZE },
	};
	struct drm_asahi_submit submit = { .queue_id = q.queue_id,
		.cmdbuf = (uintptr_t)&commands, .cmdbuf_size = sizeof(commands) };
	printf("G17P_NATIVE_COMPUTE_BEGIN vm=%u queue=%u CDM=%llx output=%llx\n",
		vm, q.queue_id, COMPUTE_CDM, COMPUTE_OUTPUT);
	if (use_timestamps) timestamp_setup(fd, &timestamps, &commands.compute, &submit);
	if (use_sync) sync_setup(fd, &sync, &submit);
	for (unsigned n = 0; n < 32; n++) {
		for (unsigned i = 0; i < 64; i++) {
			a[i] = 1000.0f + n * 100.0f + i;
			b[i] = 0.5f + n * 0.25f;
		}
		memset(result, 0xa5, PAGE);
		if (use_timestamps) timestamp_before(fd, &timestamps, &commands.compute, n);
		if (use_sync) sync_before(fd, &sync, n, vm, (const unsigned char *)result);
		errno = 0;
		int rc = ioctl(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit);
		int saved_errno = errno;
		printf("SUBMIT %u rc=%d errno=%d output[0..3]=%g,%g,%g,%g\n", n, rc,
			saved_errno, result[0], result[1], result[2], result[3]);
		CHECK(rc == 0);
		if (use_sync) sync_after(fd, &sync, n);
		if (use_timestamps) timestamp_after(&timestamps, n);
		for (unsigned i = 0; i < 64; i++) CHECK(result[i] == 1000.5f + n * 100.25f + i);
		for (unsigned i = 256; i < PAGE; i++) CHECK(((unsigned char *)result)[i] == 0xa5);
	}
	/* This admission stage must refuse exhaustion before publishing work. */
	memset(result, 0xa5, PAGE);
	if (use_sync) sync.entries[3].timeline_value = 33;
	CHECK(ioctl(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit) == -1 && errno == EOPNOTSUPP);
	if (use_sync) sync_finish(fd, &sync);
	if (use_timestamps) timestamp_finish(fd, &timestamps);
	for (unsigned i = 0; i < PAGE; i++) CHECK(((unsigned char *)result)[i] == 0xa5);
	CHECK(munmap(a, PAGE) == 0);
	CHECK(munmap(b, PAGE) == 0);
	CHECK(munmap(result, PAGE) == 0);
	CHECK(close(fd) == 0);
	printf("G17P_NATIVE_COMPUTE_PASS 32 distinct submissions, exact 2048 floats and intact tails; checks=%u\n", checks);
	return 0;
}
