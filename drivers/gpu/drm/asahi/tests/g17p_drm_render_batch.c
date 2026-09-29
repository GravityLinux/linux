/* SPDX-License-Identifier: MIT */
/* Sixty-four two-command synchronous batches with independent caller resources. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include "g17p_drm_render_batch_workload.h"

static void images_check(unsigned char **images, int completed)
{
	for (unsigned target = 0; target < 16; target++) {
		float value = (target < 8 ? RENDER_TRIANGLES : RENDER_SECOND_TRIANGLES) * ((target % 8 + 1) / 8.0f);
		unsigned char expected[4]; memcpy(expected, &value, 4);
		for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++) {
			unsigned char want = completed ? 0 : 0xa5;
			if (completed && byte >= RENDER_PIXEL_0 && byte < RENDER_PIXEL_0 + 4) want = expected[byte - RENDER_PIXEL_0];
			if (completed && byte >= RENDER_PIXEL_1 && byte < RENDER_PIXEL_1 + 4) want = expected[byte - RENDER_PIXEL_1];
			CHECK(images[target][byte] == want);
		}
	}
}
int main(void)
{
	setbuf(stdout, NULL);
	int fd = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC); CHECK(fd >= 0);
	uint32_t vm = vm_new(fd);
	struct drm_asahi_queue_create q = { .vm_id = vm, .usc_exec_base = EXEC };
	OK(fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q);
	unsigned char *images[16] = {0};
	for (unsigned i = 0; i < sizeof(workloads) / sizeof(workloads[0]); i++) {
		const struct workload *w = &workloads[i];
		uint32_t bo = bo_new(fd, w->size, DRM_ASAHI_GEM_WRITEBACK, 0);
		void *map = bo_map(fd, bo, w->size); memcpy(map, w->data, w->size);
		if (w->writable) {
			for (unsigned target = 0; target < 16; target++)
				if (w->address == render_outputs[target]) images[target] = map;
		} else CHECK(munmap(map, w->size) == 0);
		bind(fd, vm, bo, w->address, w->size, 0, DRM_ASAHI_BIND_READ | (w->writable ? DRM_ASAHI_BIND_WRITE : 0), 0);
	}
	for (unsigned i = 0; i < 16; i++) CHECK(images[i] != NULL);
	uint32_t ts_bo = bo_new(fd, PAGE * 3, DRM_ASAHI_GEM_WRITEBACK, 0);
	unsigned char *timestamps = bo_map(fd, ts_bo, PAGE * 3); memset(timestamps, 0xa5, PAGE * 3);
	struct drm_asahi_gem_bind_object object = { .op = DRM_ASAHI_BIND_OBJECT_OP_BIND,
		.flags = DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS, .handle = ts_bo, .offset = PAGE, .range = PAGE };
	OK(fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &object);
	CHECK(sizeof(render_command) == sizeof(render_second_command));
	union { uint64_t align; unsigned char bytes[sizeof(render_command) * 3]; } buffer;
	memcpy(buffer.bytes, render_command, sizeof(render_command));
	memcpy(buffer.bytes + sizeof(render_command), render_second_command, sizeof(render_command));
	struct drm_asahi_cmd_render *commands[2];
	struct drm_asahi_cmd_header *headers[2];
	for (unsigned i = 0; i < 2; i++) {
		commands[i] = (void *)(buffer.bytes + (i + 1) * sizeof(render_command) - sizeof(*commands[i]));
		headers[i] = (void *)((unsigned char *)commands[i] - sizeof(*headers[i]));
		headers[i]->vdm_barrier = i; headers[i]->cdm_barrier = DRM_ASAHI_BARRIER_NONE;
	}
	struct drm_syncobj_create binary = {0}, timeline = {0};
	OK(fd, DRM_IOCTL_SYNCOBJ_CREATE, &binary); OK(fd, DRM_IOCTL_SYNCOBJ_CREATE, &timeline);
	struct drm_asahi_sync syncs[] = {
		{ .sync_type = DRM_ASAHI_SYNC_SYNCOBJ, .handle = binary.handle },
		{ .sync_type = DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ, .handle = timeline.handle, .timeline_value = 1 },
	};
	struct drm_asahi_submit submit = { .queue_id = q.queue_id, .cmdbuf = (uintptr_t)buffer.bytes,
		.cmdbuf_size = sizeof(render_command) * 2, .syncs = (uintptr_t)syncs, .out_sync_count = 2 };
	struct drm_syncobj_array reset = { .handles = (uintptr_t)&binary.handle, .count_handles = 1 };
	struct drm_syncobj_wait wait = { .handles = (uintptr_t)&binary.handle, .count_handles = 1, .timeout_nsec = 0 };
	uint64_t point = 0, history[128][4] = {{0}};
	struct drm_syncobj_timeline_wait twait = { .handles = (uintptr_t)&timeline.handle,
		.points = (uintptr_t)&point, .count_handles = 1, .timeout_nsec = 0 };
	printf("G17P_NATIVE_RENDER_BATCH_BEGIN 64 batches; triangles=%u,%u; 16 distinct images per batch\n", RENDER_TRIANGLES, RENDER_SECOND_TRIANGLES);
	for (unsigned batch = 0; batch < 64; batch++) {
		for (unsigned target = 0; target < 16; target++) memset(images[target], 0xa5, RENDER_OUTPUT_SIZE);
		memset(timestamps + PAGE + 64 + batch * 64, 0, 64);
		for (unsigned i = 0; i < 2; i++) {
			struct drm_asahi_timestamp *refs[] = { &commands[i]->ts_vtx.start, &commands[i]->ts_vtx.end,
				&commands[i]->ts_frag.start, &commands[i]->ts_frag.end };
			for (unsigned j = 0; j < 4; j++) { refs[j]->handle = object.object_handle; refs[j]->offset = 64 + (batch * 2 + i) * 32 + j * 8; }
		}
		syncs[1].timeline_value = point = batch + 1;
		OK(fd, DRM_IOCTL_SYNCOBJ_RESET, &reset);
		if (!batch) {
			commands[1]->flags |= 1U << 31;
			BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); commands[1]->flags &= ~(1U << 31);
			headers[1]->vdm_barrier = 2;
			BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); headers[1]->vdm_barrier = 1;
			images_check(images, 0);
			for (unsigned j = 0; j < 64; j++) CHECK(timestamps[PAGE + 64 + j] == 0);
		}
		if (batch == 63) {
			memcpy(buffer.bytes + sizeof(render_command) * 2, buffer.bytes, sizeof(render_command));
			submit.cmdbuf_size += sizeof(render_command);
			BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EOPNOTSUPP);
			submit.cmdbuf_size -= sizeof(render_command);
			images_check(images, 0);
			CHECK(memcmp(history, timestamps + PAGE + 64, batch * 64) == 0);
			for (unsigned j = 0; j < 64; j++) CHECK(timestamps[PAGE + 64 + batch * 64 + j] == 0);
		}
		images_check(images, 0);
		uint64_t submitted = UINT64_MAX;
		struct drm_syncobj_timeline_array query = { .handles = (uintptr_t)&timeline.handle,
			.points = (uintptr_t)&submitted, .count_handles = 1,
			.flags = DRM_SYNCOBJ_QUERY_FLAGS_LAST_SUBMITTED };
		OK(fd, DRM_IOCTL_SYNCOBJ_QUERY, &query); CHECK(submitted == batch);
		struct drm_syncobj_handle empty = { .handle = binary.handle,
			.flags = DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE };
		BAD(fd, DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &empty, EINVAL);
		errno = 0; int rc = ioctl(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit), saved_errno = errno;
		printf("RENDER_BATCH %u rc=%d errno=%d\n", batch, rc, saved_errno); CHECK(rc == 0);
		for (unsigned i = 0; i < 2; i++) {
			unsigned n = batch * 2 + i; memcpy(history[n], timestamps + PAGE + 64 + n * 32, 32);
			uint64_t *stamps = history[n];
			printf("RENDER_TIMESTAMPS %u TA=%" PRIu64 ",%" PRIu64 " FRAG=%" PRIu64 ",%" PRIu64 "\n", n, stamps[0], stamps[1], stamps[2], stamps[3]);
			CHECK(stamps[0] && stamps[1] > stamps[0] && stamps[2] && stamps[3] > stamps[2]);
			if (n) CHECK(stamps[0] > history[n-1][3]);
		}
		CHECK(memcmp(history, timestamps + PAGE + 64, (batch + 1) * 64) == 0);
		for (unsigned byte = 0; byte < PAGE * 3; byte++)
			if (byte < PAGE + 64 || byte >= PAGE + 64 + (batch + 1) * 64) CHECK(timestamps[byte] == 0xa5);
		images_check(images, 1);
		OK(fd, DRM_IOCTL_SYNCOBJ_WAIT, &wait); OK(fd, DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT, &twait);
	}
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EOPNOTSUPP); images_check(images, 1);
	CHECK(memcmp(history, timestamps + PAGE + 64, sizeof(history)) == 0);
	for (unsigned target = 0; target < 16; target++) CHECK(munmap(images[target], RENDER_OUTPUT_SIZE) == 0);
	CHECK(munmap(timestamps, PAGE * 3) == 0); CHECK(close(fd) == 0);
	printf("G17P_NATIVE_RENDER_BATCH_PASS 64 batches / 128 renders, distinct full images per command, 512 timestamps, barriers and batch fences; checks=%u\n", checks);
	return 0;
}
