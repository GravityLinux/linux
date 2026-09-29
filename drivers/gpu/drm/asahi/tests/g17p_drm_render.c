/* SPDX-License-Identifier: MIT */
/* Real caller GEMs and authored constant-eight shader; independent full image. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include "g17p_drm_render_workload.h"

static void check_unpublished(unsigned char **maps)
{
	for (unsigned target = 0; target < 8; target++)
		for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++)
			CHECK(maps[target][byte] == 0xa5);
}
int main(void)
{
	setbuf(stdout, NULL);
	int fd = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	uint32_t vm = vm_new(fd);
	struct drm_asahi_queue_create q = { .vm_id = vm, .usc_exec_base = EXEC };
	OK(fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q);
	unsigned char *outputs[8] = {0};
	for (size_t i = 0; i < sizeof(workloads) / sizeof(workloads[0]); i++) {
		const struct workload *w = &workloads[i];
		uint32_t bo = bo_new(fd, w->size, DRM_ASAHI_GEM_WRITEBACK, 0);
		void *map = bo_map(fd, bo, w->size);
		memcpy(map, w->data, w->size);
		if (w->writable) {
			memset(map, 0xa5, w->size);
			for (unsigned target = 0; target < 8; target++)
				if (w->address == render_outputs[target]) outputs[target] = map;
		} else CHECK(munmap(map, w->size) == 0);
		bind(fd, vm, bo, w->address, w->size, 0,
			DRM_ASAHI_BIND_READ | (w->writable ? DRM_ASAHI_BIND_WRITE : 0), 0);
	}
	for (unsigned i = 0; i < 8; i++) CHECK(outputs[i] != NULL);
	uint32_t ts_bo = bo_new(fd, PAGE * 3, DRM_ASAHI_GEM_WRITEBACK, 0);
	unsigned char *timestamps = bo_map(fd, ts_bo, PAGE * 3);
	memset(timestamps, 0xa5, PAGE * 3);
	memset(timestamps + PAGE + 64, 0, 32);
	struct drm_asahi_gem_bind_object object = { .op = DRM_ASAHI_BIND_OBJECT_OP_BIND,
		.flags = DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,
		.handle = ts_bo, .offset = PAGE, .range = PAGE };
	OK(fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &object);
	union { uint64_t align; unsigned char bytes[sizeof(render_command) * 2]; } buffer;
	memcpy(buffer.bytes, render_command, sizeof(render_command));
	struct drm_asahi_cmd_render *cmd = (void *)(buffer.bytes + sizeof(render_command) - sizeof(*cmd));
	CHECK(sizeof(*cmd) == 240);
	struct drm_asahi_timestamp *refs[] = { &cmd->ts_vtx.start, &cmd->ts_vtx.end,
		&cmd->ts_frag.start, &cmd->ts_frag.end };
	for (unsigned i = 0; i < 4; i++) { refs[i]->handle = object.object_handle; refs[i]->offset = 64 + i * 8; }
	struct drm_syncobj_create binary = {0}, timeline = {0};
	OK(fd, DRM_IOCTL_SYNCOBJ_CREATE, &binary); OK(fd, DRM_IOCTL_SYNCOBJ_CREATE, &timeline);
	struct drm_asahi_sync syncs[] = {
		{ .sync_type = DRM_ASAHI_SYNC_SYNCOBJ, .handle = binary.handle },
		{ .sync_type = DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ, .handle = timeline.handle, .timeline_value = 1 },
	};
	struct drm_asahi_submit submit = { .queue_id = q.queue_id, .cmdbuf = (uintptr_t)buffer.bytes,
		.cmdbuf_size = sizeof(render_command), .syncs = (uintptr_t)syncs, .out_sync_count = 2 };
	printf("G17P_NATIVE_RENDER_BEGIN vm=%u queue=%u buffers=%zu\n", vm, q.queue_id, sizeof(workloads)/sizeof(workloads[0]));
	uint32_t flags = cmd->flags; cmd->flags |= 1U << 31;
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); cmd->flags = flags;
	cmd->width_px = 0; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); cmd->width_px = 128;
	cmd->vertex_helper.binary = 4; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); cmd->vertex_helper.binary = 0;
	cmd->bg.usc = 0; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); cmd->bg.usc = 0x1e8240;
	cmd->ts_vtx.start.offset = 1; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); cmd->ts_vtx.start.offset = 64;
	cmd->ts_vtx.start.handle = UINT32_MAX; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, ENOENT); cmd->ts_vtx.start.handle = object.object_handle;
	memcpy(buffer.bytes + sizeof(render_command), buffer.bytes, sizeof(render_command));
	submit.cmdbuf_size *= 2; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EOPNOTSUPP); submit.cmdbuf_size /= 2;
	check_unpublished(outputs);
	errno = 0;
	int rc = ioctl(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit), saved_errno = errno;
	printf("RENDER SUBMIT rc=%d errno=%d\n", rc, saved_errno);
	for (unsigned target = 0; target < 8; target++) {
		float a, b;
		memcpy(&a, outputs[target] + RENDER_PIXEL_0, 4);
		memcpy(&b, outputs[target] + RENDER_PIXEL_1, 4);
		printf("OUTPUT %u pixels=%g,%g expected=%g\n", target, a, b, (target + 1)/8.0);
	}
	CHECK(rc == 0);
	uint64_t stamps[4]; memcpy(stamps, timestamps + PAGE + 64, sizeof(stamps));
	printf("RENDER_TIMESTAMPS TA=%" PRIu64 ",%" PRIu64 " FRAG=%" PRIu64 ",%" PRIu64 "\n", stamps[0], stamps[1], stamps[2], stamps[3]);
	CHECK(stamps[0] && stamps[1] > stamps[0] && stamps[2] && stamps[3] > stamps[2]);
	for (unsigned byte = 0; byte < PAGE * 3; byte++)
		if (byte < PAGE + 64 || byte >= PAGE + 96) CHECK(timestamps[byte] == 0xa5);
	for (unsigned target = 0; target < 8; target++) {
		float value = (target + 1)/8.0f;
		unsigned char expected[4]; memcpy(expected, &value, 4);
		for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++) {
			unsigned char want = 0;
			if (byte >= RENDER_PIXEL_0 && byte < RENDER_PIXEL_0 + 4) want = expected[byte - RENDER_PIXEL_0];
			if (byte >= RENDER_PIXEL_1 && byte < RENDER_PIXEL_1 + 4) want = expected[byte - RENDER_PIXEL_1];
			CHECK(outputs[target][byte] == want);
		}
	}
	struct drm_syncobj_wait wait = { .handles = (uintptr_t)&binary.handle, .count_handles = 1, .timeout_nsec = 0 };
	OK(fd, DRM_IOCTL_SYNCOBJ_WAIT, &wait);
	uint64_t point = 1;
	struct drm_syncobj_timeline_wait twait = { .handles = (uintptr_t)&timeline.handle,
		.points = (uintptr_t)&point, .count_handles = 1, .timeout_nsec = 0 };
	OK(fd, DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT, &twait);
	/* Until repeat publication is ported, refusal must retain successful output. */
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EOPNOTSUPP);
	for (unsigned target = 0; target < 8; target++) CHECK(munmap(outputs[target], RENDER_OUTPUT_SIZE) == 0);
	CHECK(munmap(timestamps, PAGE * 3) == 0);
	CHECK(close(fd) == 0);
	printf("G17P_NATIVE_RENDER_PASS eight exact full 128x128 R32F images, four timestamps and binary/timeline fences; checks=%u\n", checks);
	return 0;
}
