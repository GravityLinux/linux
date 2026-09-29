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
static void check_images(unsigned char **outputs)
{
	for (unsigned target = 0; target < 8; target++) {
		float value = RENDER_TRIANGLES * ((target + 1)/8.0f);
		unsigned char expected[4]; memcpy(expected, &value, 4);
		for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++) {
			unsigned char want = 0;
			if (byte >= RENDER_PIXEL_0 && byte < RENDER_PIXEL_0 + 4) want = expected[byte - RENDER_PIXEL_0];
			if (byte >= RENDER_PIXEL_1 && byte < RENDER_PIXEL_1 + 4) want = expected[byte - RENDER_PIXEL_1];
			CHECK(outputs[target][byte] == want);
		}
	}
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
	unsigned char *retired[31][8] = {{0}};
	uint32_t output_handles[8] = {0};
	for (size_t i = 0; i < sizeof(workloads) / sizeof(workloads[0]); i++) {
		const struct workload *w = &workloads[i];
		uint32_t bo = bo_new(fd, w->size, DRM_ASAHI_GEM_WRITEBACK, 0);
		void *map = bo_map(fd, bo, w->size);
		memcpy(map, w->data, w->size);
		if (w->writable) {
			memset(map, 0xa5, w->size);
			for (unsigned target = 0; target < 8; target++)
				if (w->address == render_outputs[target]) {
					outputs[target] = map; output_handles[target] = bo;
				}
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
	printf("G17P_NATIVE_RENDER_BEGIN vm=%u queue=%u buffers=%zu triangles=%u\n", vm, q.queue_id, sizeof(workloads)/sizeof(workloads[0]), RENDER_TRIANGLES);
	uint32_t flags = cmd->flags; cmd->flags |= 1U << 31;
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); cmd->flags = flags;
	cmd->width_px = 0; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); cmd->width_px = 128;
	cmd->vertex_helper.binary = 4; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); cmd->vertex_helper.binary = 0;
	cmd->bg.usc = 0; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); cmd->bg.usc = 0x1e8240;
	cmd->ts_vtx.start.offset = 1; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); cmd->ts_vtx.start.offset = 64;
	cmd->ts_vtx.start.handle = UINT32_MAX; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, ENOENT); cmd->ts_vtx.start.handle = object.object_handle;
	memcpy(buffer.bytes + sizeof(render_command), buffer.bytes, sizeof(render_command));
	struct drm_asahi_cmd_render *last = (void *)(buffer.bytes + sizeof(buffer.bytes) - sizeof(*last));
	last->flags |= 1U << 31;
	submit.cmdbuf_size *= 2; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); submit.cmdbuf_size /= 2;
	const unsigned draws = 32;
	uint64_t history[32][4] = {{0}};
	struct drm_syncobj_array reset = { .handles = (uintptr_t)&binary.handle, .count_handles = 1 };
	struct drm_syncobj_wait wait = { .handles = (uintptr_t)&binary.handle, .count_handles = 1, .timeout_nsec = 0 };
	uint64_t point = 0;
	struct drm_syncobj_timeline_wait twait = { .handles = (uintptr_t)&timeline.handle,
		.points = (uintptr_t)&point, .count_handles = 1, .timeout_nsec = 0 };
	for (unsigned draw = 0; draw < draws; draw++) {
		if (draw) {
			for (unsigned target = 0; target < 8; target++) {
				retired[draw - 1][target] = outputs[target];
				bind(fd, vm, 0, render_outputs[target], RENDER_OUTPUT_SIZE, 0,
					DRM_ASAHI_BIND_UNBIND, 0);
				bo_close(fd, output_handles[target]);
				output_handles[target] = bo_new(fd, RENDER_OUTPUT_SIZE + PAGE * 2, DRM_ASAHI_GEM_WRITEBACK, 0);
				unsigned char *allocation = bo_map(fd, output_handles[target], RENDER_OUTPUT_SIZE + PAGE * 2);
				memset(allocation, 0x5a, RENDER_OUTPUT_SIZE + PAGE * 2);
				outputs[target] = allocation + PAGE;
				/* Same GPU VA, new physical owner and nonzero GEM offset. */
				bind(fd, vm, output_handles[target], render_outputs[target], RENDER_OUTPUT_SIZE,
					PAGE, DRM_ASAHI_BIND_READ | DRM_ASAHI_BIND_WRITE, 0);
			}
		}
		/* Fresh poison detects cursor-only completion and stale output. */
		for (unsigned target = 0; target < 8; target++) memset(outputs[target], 0xa5, RENDER_OUTPUT_SIZE);
		unsigned offset = 64 + draw * 32;
		memset(timestamps + PAGE + offset, 0, 32);
		for (unsigned i = 0; i < 4; i++) refs[i]->offset = offset + i * 8;
		syncs[1].timeline_value = point = draw + 1;
		OK(fd, DRM_IOCTL_SYNCOBJ_RESET, &reset);
		check_unpublished(outputs);
		if (draw == 1) {
			/* A late private-range collision must leave the prior root and
			 * publication intact; removing it permits the same draw. */
			uint32_t collision = bo_new(fd, PAGE, DRM_ASAHI_GEM_WRITEBACK, 0);
			bind(fd, vm, collision, 0x1002000000ULL, PAGE, 0, DRM_ASAHI_BIND_READ, 0);
			BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL);
			check_unpublished(outputs); check_images(retired[0]);
			bind(fd, vm, 0, 0x1002000000ULL, PAGE, 0, DRM_ASAHI_BIND_UNBIND, 0);
			bo_close(fd, collision);
		}
		errno = 0; int rc = ioctl(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit), saved_errno = errno;
		printf("RENDER DRAW %u rc=%d errno=%d\n", draw, rc, saved_errno);
		if (rc != 0) for (unsigned target = 0; target < 8; target++) {
			float value; memcpy(&value, outputs[target] + RENDER_PIXEL_0, 4);
			printf("FAILED OUTPUT %u pixel=%g\n", target, value);
		}
		CHECK(rc == 0);
		memcpy(history[draw], timestamps + PAGE + offset, 32);
		uint64_t *stamps = history[draw];
		printf("RENDER_TIMESTAMPS %u TA=%" PRIu64 ",%" PRIu64 " FRAG=%" PRIu64 ",%" PRIu64 "\n", draw, stamps[0], stamps[1], stamps[2], stamps[3]);
		CHECK(stamps[0] && stamps[1] > stamps[0] && stamps[2] && stamps[3] > stamps[2]);
		if (draw) CHECK(stamps[0] > history[draw-1][0] && stamps[2] > history[draw-1][2]);
		CHECK(memcmp(history, timestamps + PAGE + 64, (draw + 1) * 32) == 0);
		for (unsigned byte = 0; byte < PAGE * 3; byte++)
			if (byte < PAGE + 64 || byte >= PAGE + offset + 32) CHECK(timestamps[byte] == 0xa5);
		check_images(outputs);
		/* Every older allocation stays mapped in the CPU and must remain
		 * intact. Stale GPU translations cannot hide behind VA reuse. */
		for (unsigned old = 0; old < draw; old++) check_images(retired[old]);
		for (unsigned generation = 1; generation <= draw; generation++) {
			unsigned char **images = generation == draw ? outputs : retired[generation];
			for (unsigned target = 0; target < 8; target++)
				for (unsigned byte = 0; byte < PAGE; byte++) {
					CHECK(images[target][(int)byte - (int)PAGE] == 0x5a);
					CHECK(images[target][RENDER_OUTPUT_SIZE + byte] == 0x5a);
				}
		}
		OK(fd, DRM_IOCTL_SYNCOBJ_WAIT, &wait);
		OK(fd, DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT, &twait);
	}
	/* Finite storage admission must reject the next command atomically. */
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EOPNOTSUPP);
	check_images(outputs);
	CHECK(memcmp(history, timestamps + PAGE + 64, sizeof(history)) == 0);
	for (unsigned target = 0; target < 8; target++) {
		CHECK(munmap(retired[0][target], RENDER_OUTPUT_SIZE) == 0);
		for (unsigned generation = 1; generation < draws; generation++) {
			unsigned char **images = generation == draws - 1 ? outputs : retired[generation];
			CHECK(munmap(images[target] - PAGE, RENDER_OUTPUT_SIZE + PAGE * 2) == 0);
		}
	}
	CHECK(munmap(timestamps, PAGE * 3) == 0);
	CHECK(close(fd) == 0);
	printf("G17P_NATIVE_RENDER_PASS 32 renders, 256 independent exact full 128x128 R32F images, retained old images and guards, 128 timestamps and binary/timeline fences; checks=%u\n", checks);
	return 0;
}
