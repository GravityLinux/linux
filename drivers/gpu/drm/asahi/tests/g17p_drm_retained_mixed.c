/* SPDX-License-Identifier: MIT */
/* One render, 258 ordinary retained computes, then another render. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include "g17p_drm_render_workload.h"
#define workload compute_workload
#define workloads compute_workloads
#include "g17p_drm_mixed_workload.h"
#undef workloads
#undef workload

static void render_check(unsigned char **images, int complete)
{
	for (unsigned target = 0; target < 8; target++) {
		float value = RENDER_TRIANGLES * ((target + 1) / 8.0f);
		unsigned char expected[4]; memcpy(expected, &value, 4);
		for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++) {
			unsigned char want = complete ? 0 : 0xa5;
			if (complete && byte >= RENDER_PIXEL_0 && byte < RENDER_PIXEL_0 + 4) want = expected[byte - RENDER_PIXEL_0];
			if (complete && byte >= RENDER_PIXEL_1 && byte < RENDER_PIXEL_1 + 4) want = expected[byte - RENDER_PIXEL_1];
			CHECK(images[target][byte] == want);
		}
	}
}
static void compute_check(unsigned char **outputs, const int *epochs)
{
	for (unsigned n = 0; n < 2; n++) {
		for (unsigned byte = 0; byte < PAGE; byte++) {
			unsigned char want = 0xa5;
			if (epochs[n] >= 0 && byte < 256) {
				float value = 2000.25f + (8 + n) * 129.0f + epochs[n] * 256.0f + byte / 4;
				unsigned char bytes[4]; memcpy(bytes, &value, 4); want = bytes[byte % 4];
			}
			CHECK(outputs[n][byte] == want);
		}
	}
}
int main(void)
{
	setbuf(stdout, NULL);
	const unsigned steps = 260;
	int fd = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC); CHECK(fd >= 0);
	uint32_t vm = vm_new(fd);
	struct drm_asahi_queue_create q = { .vm_id = vm, .usc_exec_base = EXEC };
	OK(fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q);
	unsigned char *images[8] = {0}, *outputs[2] = {0};
	float *inputs[2] = {0}; int epochs[2] = {-1,-1};
	for (unsigned i = 0; i < sizeof(workloads) / sizeof(workloads[0]); i++) {
		const struct workload *w = &workloads[i];
		uint32_t bo = bo_new(fd, w->size, DRM_ASAHI_GEM_WRITEBACK, 0);
		void *map = bo_map(fd, bo, w->size); memcpy(map, w->data, w->size);
		if (w->writable) {
			for (unsigned j = 0; j < 8; j++) if (w->address == render_outputs[j]) images[j] = map;
		} else CHECK(munmap(map, w->size) == 0);
		bind(fd, vm, bo, w->address, w->size, 0, DRM_ASAHI_BIND_READ | (w->writable ? DRM_ASAHI_BIND_WRITE : 0), 0);
	}
	for (unsigned i = 0; i < sizeof(compute_workloads) / sizeof(compute_workloads[0]); i++) {
		const struct compute_workload *w = &compute_workloads[i];
		/* Every compute allocation must be disjoint from every render BO. */
		for (unsigned j = 0; j < sizeof(workloads) / sizeof(workloads[0]); j++)
			CHECK(w->address + w->size <= workloads[j].address || workloads[j].address + workloads[j].size <= w->address);
		uint32_t bo = bo_new(fd, w->size, DRM_ASAHI_GEM_WRITEBACK, 0);
		unsigned char *map = bo_map(fd, bo, w->size); memcpy(map, w->data, w->size);
		int output = -1, input = -1;
		for (unsigned n = 0; n < 2; n++) if (w->address == batch_workloads[n].input_a) input = n;
		for (unsigned n = 0; n < 2; n++) if (w->address == batch_workloads[n].output) output = n;
		if (output >= 0) { outputs[output] = map; memset(map, 0xa5, PAGE); }
		else if (input >= 0) inputs[input] = (float *)map;
		else CHECK(munmap(map, w->size) == 0);
		bind(fd, vm, bo, w->address, w->size, 0, DRM_ASAHI_BIND_READ | (w->writable ? DRM_ASAHI_BIND_WRITE : 0), 0);
	}
	for (unsigned j = 0; j < 8; j++) { CHECK(images[j] != NULL); memset(images[j], 0xa5, RENDER_OUTPUT_SIZE); }
	CHECK(outputs[0] && outputs[1] && inputs[0] && inputs[1]);
	uint32_t ts_bo = bo_new(fd, PAGE * 3, DRM_ASAHI_GEM_WRITEBACK, 0);
	unsigned char *timestamps = bo_map(fd, ts_bo, PAGE * 3); memset(timestamps, 0xa5, PAGE * 3);
	struct drm_asahi_gem_bind_object object = { .op = DRM_ASAHI_BIND_OBJECT_OP_BIND,
		.flags = DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS, .handle = ts_bo, .offset = PAGE, .range = PAGE };
	OK(fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &object);
	union { uint64_t align; unsigned char bytes[sizeof(render_command)]; } render;
	memcpy(render.bytes, render_command, sizeof(render_command));
	struct drm_asahi_cmd_render *rcmd = (void *)(render.bytes + sizeof(render_command) - sizeof(*rcmd));
	struct { struct drm_asahi_cmd_header header; struct drm_asahi_cmd_compute cmd; } compute = {
		.header = { .cmd_type = DRM_ASAHI_CMD_COMPUTE, .size = sizeof(struct drm_asahi_cmd_compute),
			.vdm_barrier = DRM_ASAHI_BARRIER_NONE, .cdm_barrier = DRM_ASAHI_BARRIER_NONE },
	};
	struct drm_syncobj_create binary = {0}, timeline = {0};
	OK(fd, DRM_IOCTL_SYNCOBJ_CREATE, &binary); OK(fd, DRM_IOCTL_SYNCOBJ_CREATE, &timeline);
	struct drm_asahi_sync syncs[] = {
		{ .sync_type = DRM_ASAHI_SYNC_SYNCOBJ, .handle = binary.handle },
		{ .sync_type = DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ, .handle = timeline.handle },
	};
	struct drm_syncobj_array reset = { .handles = (uintptr_t)&binary.handle, .count_handles = 1 };
	struct drm_syncobj_wait wait = { .handles = (uintptr_t)&binary.handle, .count_handles = 1, .timeout_nsec = 0 };
	uint64_t point = 0;
	struct drm_syncobj_timeline_wait twait = { .handles = (uintptr_t)&timeline.handle,
		.points = (uintptr_t)&point, .count_handles = 1, .timeout_nsec = 0 };
	unsigned char expected[PAGE * 3]; memset(expected, 0xa5, sizeof(expected));
	uint64_t last = 0;
	puts("G17P_NATIVE_RETAINED_MIXED_BEGIN R/Cx258/R, retained queue one and command-local storage");
	for (unsigned step = 0; step < steps; step++) {
		int is_compute = step > 0 && step < 259;
		unsigned count = is_compute ? 2 : 4;
		struct drm_asahi_timestamp *refs[4];
		if (is_compute) {
			unsigned n = (step - 1) % 2;
			for (unsigned i = 0; i < 64; i++) inputs[n][i] = 2000.0f + (8+n)*128.0f + step*256.0f + i;
			memset(outputs[n], 0xa5, PAGE); epochs[n] = -1;
			compute.cmd.cdm_ctrl_stream_base = batch_workloads[n].cdm;
			compute.cmd.cdm_ctrl_stream_end = batch_workloads[n].cdm + batch_workloads[n].cdm_size;
			refs[0] = &compute.cmd.ts.start; refs[1] = &compute.cmd.ts.end;
		} else {
			for (unsigned target = 0; target < 8; target++) memset(images[target], 0xa5, RENDER_OUTPUT_SIZE);
			render_check(images, 0);
			refs[0] = &rcmd->ts_vtx.start; refs[1] = &rcmd->ts_vtx.end;
			refs[2] = &rcmd->ts_frag.start; refs[3] = &rcmd->ts_frag.end;
		}
		for (unsigned i = 0; i < count; i++) { refs[i]->handle = object.object_handle; refs[i]->offset = 64 + step * 32 + i * 8; }
		memset(timestamps + PAGE + 64 + step * 32, 0, count * 8);
		syncs[1].timeline_value = point = step + 1;
		OK(fd, DRM_IOCTL_SYNCOBJ_RESET, &reset);
		struct drm_asahi_submit submit = { .queue_id = q.queue_id,
			.cmdbuf = is_compute ? (uintptr_t)&compute : (uintptr_t)render.bytes,
			.cmdbuf_size = is_compute ? sizeof(compute) : sizeof(render_command),
			.syncs = (uintptr_t)syncs, .out_sync_count = 2 };
		if (step == 257) {
			/* Only two source command placements remain: reject before prefix. */
			typeof(compute) rejected[3] = {compute,compute,compute};
			struct drm_asahi_submit bad = submit; bad.cmdbuf = (uintptr_t)rejected; bad.cmdbuf_size = sizeof(rejected);
			BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &bad, EOPNOTSUPP);
			compute_check(outputs, epochs);
		}
		errno = 0; int result = ioctl(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit), error = errno;
		printf("MIXED_STEP %u %s rc=%d errno=%d\n", step, is_compute ? "compute" : "render", result, error); CHECK(result == 0);
		uint64_t stamps[4]; memcpy(stamps, timestamps + PAGE + 64 + step * 32, count * 8);
		for (unsigned i = 0; i < count; i += 2) CHECK(stamps[i] && stamps[i + 1] > stamps[i]);
		CHECK(stamps[0] > last); last = stamps[count - 1];
		memcpy(expected + PAGE + 64 + step * 32, stamps, count * 8);
		CHECK(memcmp(timestamps, expected, sizeof(expected)) == 0);
		if (is_compute) epochs[(step-1)%2] = step;
		render_check(images, 1); compute_check(outputs, epochs);
		OK(fd, DRM_IOCTL_SYNCOBJ_WAIT, &wait); OK(fd, DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT, &twait);
		printf("MIXED_OUTPUT_PASS step=%u start=%" PRIu64 " end=%" PRIu64 "\n", step, stamps[0], last);
	}
	struct drm_asahi_submit exhausted = { .queue_id=q.queue_id,.cmdbuf=(uintptr_t)&compute,.cmdbuf_size=sizeof(compute) };
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &exhausted, EOPNOTSUPP);
	render_check(images,1);compute_check(outputs,epochs);CHECK(memcmp(timestamps,expected,sizeof(expected))==0);
	CHECK(close(fd) == 0);
	printf("G17P_NATIVE_RETAINED_MIXED_PASS 2 renders, 258 fresh compute results, 524 timestamps, 260 aggregate fence pairs, exhausted admission; checks=%u\n",checks);
	return 0;
}
