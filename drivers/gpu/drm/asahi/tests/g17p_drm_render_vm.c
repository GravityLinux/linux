/* SPDX-License-Identifier: MIT */
/* Two files with identical DVAs and independently backed pressure renders. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include <linux/sync_file.h>
#include "g17p_drm_render_workload.h"
#include "g17p_drm_second_render_workload.h"

static size_t render_timestamp_range = PAGE;
static int tiny_render;

struct render_owner {
	int fd, last;
	uint32_t vm, queue, binary, timeline, object;
	unsigned char *images[8], *timestamps, *sentinel;
	uint64_t saved[2048][4];
	struct { uint64_t address, size; } spans[64];
	unsigned span_count;
	unsigned char command[sizeof(render_command)];
};
static void create_render_owner(struct render_owner *o, unsigned index)
{
	o->last = -1;
	o->fd = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC); CHECK(o->fd >= 0);
	o->vm = vm_new(o->fd);
	struct drm_asahi_queue_create q = { .vm_id = o->vm, .usc_exec_base = EXEC };
	OK(o->fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q); o->queue = q.queue_id;
	unsigned count = index ? sizeof(second_workloads)/sizeof(second_workloads[0]) : sizeof(workloads)/sizeof(workloads[0]);
	for (unsigned i = 0; i < count; i++) {
		uint64_t address = index ? second_workloads[i].address : workloads[i].address;
		size_t size = index ? second_workloads[i].size : workloads[i].size;
		const void *data = index ? second_workloads[i].data : workloads[i].data;
		int writable = index ? second_workloads[i].writable : workloads[i].writable;
		uint32_t bo = bo_new(o->fd, size, DRM_ASAHI_GEM_WRITEBACK, 0);
		void *map = bo_map(o->fd, bo, size); memcpy(map, data, size);
		/* Bounded diagnostic: change only our authored encoder's draw count. */
		if (tiny_render && address == 0x1000018000ULL) {
			uint32_t vertices = 3; memcpy((unsigned char *)map + 0x68, &vertices, 4);
		}
		for (unsigned target = 0; target < 8; target++) {
			CHECK(render_outputs[target] == second_render_outputs[target]);
			if (address == render_outputs[target]) o->images[target] = map;
		}
		bind(o->fd, o->vm, bo, address, size, 0, DRM_ASAHI_BIND_READ | (writable ? DRM_ASAHI_BIND_WRITE : 0), 0);
		CHECK(o->span_count < 64);
		o->spans[o->span_count++] = (typeof(o->spans[0])){address, size};
	}
	for (unsigned i = 0; i < 8; i++) { CHECK(o->images[i]); memset(o->images[i], 0xa5, RENDER_OUTPUT_SIZE); }
	uint32_t ts = bo_new(o->fd, render_timestamp_range + PAGE * 2, DRM_ASAHI_GEM_WRITEBACK, 0);
	o->timestamps = bo_map(o->fd, ts, render_timestamp_range + PAGE * 2);
	memset(o->timestamps, 0xa5, render_timestamp_range + PAGE * 2);
	struct drm_asahi_gem_bind_object object = { .op = DRM_ASAHI_BIND_OBJECT_OP_BIND,
		.flags = DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS, .handle = ts, .offset = PAGE, .range = render_timestamp_range };
	OK(o->fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &object); o->object = object.object_handle;
	uint32_t sentinel = bo_new(o->fd, PAGE, DRM_ASAHI_GEM_WRITEBACK, 0);
	o->sentinel = bo_map(o->fd, sentinel, PAGE); memset(o->sentinel, 0x61 + index, PAGE);
	bind(o->fd, o->vm, sentinel, EXEC + 0x70000000, PAGE, 0, RW, 0);
	CHECK(o->span_count < 64);
	o->spans[o->span_count++] = (typeof(o->spans[0])){EXEC + 0x70000000, PAGE};
	CHECK(sizeof(render_command) == sizeof(second_render_command));
	memcpy(o->command, index ? second_render_command : render_command, sizeof(o->command));
	struct drm_syncobj_create binary = {0}, timeline = {0};
	OK(o->fd, DRM_IOCTL_SYNCOBJ_CREATE, &binary); OK(o->fd, DRM_IOCTL_SYNCOBJ_CREATE, &timeline);
	o->binary = binary.handle; o->timeline = timeline.handle;
}
static void check_render_owner(struct render_owner *o, unsigned index)
{
	CHECK(RENDER_OUTPUT_SIZE == SECOND_RENDER_OUTPUT_SIZE);
	CHECK(RENDER_PIXEL_0 == SECOND_RENDER_PIXEL_0 && RENDER_PIXEL_1 == SECOND_RENDER_PIXEL_1);
	for (unsigned target = 0; target < 8; target++) {
		float value = (tiny_render ? 1 : index ? SECOND_RENDER_TRIANGLES : RENDER_TRIANGLES) * ((target + 1)/8.0f);
		unsigned char pixel[4]; memcpy(pixel, &value, 4);
		for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++) {
			unsigned char want = o->last < 0 ? 0xa5 : 0;
			if (o->last >= 0 && byte >= RENDER_PIXEL_0 && byte < RENDER_PIXEL_0 + 4) want = pixel[byte - RENDER_PIXEL_0];
			if (o->last >= 0 && byte >= RENDER_PIXEL_1 && byte < RENDER_PIXEL_1 + 4) want = pixel[byte - RENDER_PIXEL_1];
			if (o->images[target][byte] != want) {
				printf("RENDER_VM_IMAGE_MISMATCH owner=%u generation=%d target=%u byte=%u actual=%02x expected=%02x pixels=%g/%g expected_pixel=%g\n",
					index, o->last, target, byte, o->images[target][byte], want,
					(double)*(float *)(o->images[target] + RENDER_PIXEL_0),
					(double)*(float *)(o->images[target] + RENDER_PIXEL_1), (double)value);
				printf("RENDER_VM_IMAGE_LINE");
				for (unsigned off = byte & ~63U; off < (byte & ~63U) + 64; off++)
					printf(" %02x", o->images[target][off]);
				puts("");
#ifdef G17P_RENDER_VM_DIAGNOSTIC
				puts("RENDER_VM_IMAGE_STOP press return after capturing caller state");
				CHECK(getchar() == '\n');
#endif
			}
			CHECK(o->images[target][byte] == want);
		}
	}
	for (unsigned byte = 0; byte < PAGE; byte++) CHECK(o->sentinel[byte] == 0x61 + index);
	for (unsigned byte = 0; byte < render_timestamp_range + PAGE * 2; byte++) {
		if (byte >= PAGE + 64 && byte < PAGE + 64 + (o->last + 1) * 32) continue;
		CHECK(o->timestamps[byte] == 0xa5);
	}
	CHECK(memcmp(o->saved, o->timestamps + PAGE + 64, (o->last + 1) * 32) == 0);
}
#ifndef G17P_RENDER_VM_HELPERS
int main(int argc, char **argv)
{
	setbuf(stdout, NULL);
	int cleanup = argc == 2 && !strcmp(argv[1], "--cleanup");
	int extended = argc == 2 && !strcmp(argv[1], "--extended");
	int pair_series = argc == 2 && !strcmp(argv[1], "--pair-series");
	int inspect = argc == 2 && !strcmp(argv[1], "--inspect");
	int single_series = argc == 2 && !strcmp(argv[1], "--single-series");
	int stress = argc == 2 && !strcmp(argv[1], "--stress");
	int trace = argc == 2 && !strcmp(argv[1], "--trace");
	int single_owner = argc == 2 && !strcmp(argv[1], "--single-owner");
	tiny_render = argc == 2 && !strcmp(argv[1], "--tiny");
	CHECK(argc == 1 || cleanup || extended || pair_series || inspect || single_series || stress || trace || single_owner || tiny_render);
	unsigned count = stress ? 4096 : extended || inspect ? 255 : pair_series || single_series ? 128 : 32;
	if (stress) render_timestamp_range = PAGE * 5;
	struct render_owner owners[2] = {{0}};
	for (unsigned i = 0; i < 2; i++) create_render_owner(&owners[i], i);
	uint64_t previous = 0;
	uint64_t total_checks = 0;
	unsigned checkpoint = 0;
	printf("G17P_NATIVE_RENDER_VM_BEGIN %u alternating renders, two files, identical DVAs, triangles=%u/%u\n", count, RENDER_TRIANGLES, SECOND_RENDER_TRIANGLES);
	if (single_owner || tiny_render) printf("RENDER_VM_DIAGNOSTIC single_owner=%d tiny=%d\n", single_owner, tiny_render);
	for (unsigned n = 0; n < count; n++) {
		if (inspect && (n == 34 || n == 35 || n == 36 || n >= 68)) {
			printf("RENDER_VM_INSPECT before ordinal=%u; press return to submit\n", n);
			CHECK(getchar() == '\n');
		}
		unsigned index = single_owner ? 0 : n % 2, generation = single_owner ? n : n / 2; struct render_owner *o = &owners[index];
		struct drm_asahi_cmd_render *command = (void *)(o->command + sizeof(o->command) - sizeof(*command));
		struct drm_asahi_timestamp *refs[] = { &command->ts_vtx.start, &command->ts_vtx.end, &command->ts_frag.start, &command->ts_frag.end };
		for (unsigned i = 0; i < 4; i++) *refs[i] = (struct drm_asahi_timestamp){ .handle = o->object, .offset = 64 + generation * 32 + i * 8 };
		struct drm_asahi_sync syncs[] = {
			{ .sync_type = DRM_ASAHI_SYNC_SYNCOBJ, .handle = o->binary },
			{ .sync_type = DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ, .handle = o->timeline, .timeline_value = generation + 1 },
		};
		struct drm_asahi_submit submit = { .queue_id = o->queue, .cmdbuf = (uintptr_t)o->command, .cmdbuf_size = sizeof(o->command), .syncs = (uintptr_t)syncs, .out_sync_count = 2 };
		struct drm_syncobj_array reset = { .handles = (uintptr_t)&o->binary, .count_handles = 1 };
		OK(o->fd, DRM_IOCTL_SYNCOBJ_RESET, &reset);
		if (n == 1) {
			uint32_t flags = command->flags; command->flags |= 1U << 31;
			BAD(o->fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); command->flags = flags;
			check_render_owner(&owners[0], 0); check_render_owner(&owners[1], 1);
		}
		for (unsigned i = 0; i < 8; i++) memset(o->images[i], 0xa5, RENDER_OUTPUT_SIZE);
		if (trace) {
			printf("RENDER_VM_TRACE before submission=%u owner=%u; press return\n", n, index);
			CHECK(getchar() == '\n');
		}
		errno = 0; int result = ioctl(o->fd, DRM_IOCTL_ASAHI_SUBMIT, &submit), error = errno;
		printf("RENDER_VM_SUBMIT %u owner=%u generation=%u rc=%d errno=%d\n", n, index, generation, result, error); CHECK(result == 0);
		if (trace) {
			printf("RENDER_VM_TRACE after submission=%u; press return\n", n);
			CHECK(getchar() == '\n');
		}
		o->last = generation;
		memcpy(o->saved[generation], o->timestamps + PAGE + 64 + generation * 32, 32);
		uint64_t *ts = o->saved[generation];
		int timestamps_ok = ts[0] > previous && ts[1] > ts[0] && ts[2] >= ts[1] && ts[3] > ts[2];
		if (!timestamps_ok || (n >= 168 && n <= 174))
			printf("RENDER_VM_TIMESTAMPS %u previous=%llu values=%llu/%llu/%llu/%llu\n", n,
				(unsigned long long)previous, (unsigned long long)ts[0], (unsigned long long)ts[1],
				(unsigned long long)ts[2], (unsigned long long)ts[3]);
		struct drm_syncobj_wait wait = { .handles = (uintptr_t)&o->binary, .count_handles = 1, .timeout_nsec = 0 };
		uint64_t point = generation + 1;
		struct drm_syncobj_timeline_wait twait = { .handles = (uintptr_t)&o->timeline, .points = (uintptr_t)&point, .count_handles = 1, .timeout_nsec = 0 };
		OK(o->fd, DRM_IOCTL_SYNCOBJ_WAIT, &wait); OK(o->fd, DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT, &twait);
		struct drm_syncobj_handle exported = { .handle = o->binary, .flags = DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE };
		OK(o->fd, DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &exported);
		struct sync_file_info info = {0}; CHECK(ioctl(exported.fd, SYNC_IOC_FILE_INFO, &info) == 0);
		if (info.status != 1) printf("RENDER_VM_FENCE %u status=%d\n", n, info.status);
		CHECK(info.status == 1); CHECK(close(exported.fd) == 0);
		CHECK(timestamps_ok); previous = ts[3];
		for (unsigned i = 0; i < 2; i++) check_render_owner(&owners[i], i);
		total_checks += (unsigned)(checks - checkpoint); checkpoint = checks;
	}
	if (cleanup) for (unsigned i = 0; i < 2; i++) {
		struct render_owner *o = &owners[i];
		struct drm_asahi_queue_destroy qd = { .queue_id = o->queue };
		OK(o->fd, DRM_IOCTL_ASAHI_QUEUE_DESTROY, &qd);
		for (unsigned span = 0; span < o->span_count; span++)
			bind(o->fd, o->vm, 0, o->spans[span].address, o->spans[span].size,
				0, DRM_ASAHI_BIND_UNBIND, 0);
		vm_destroy(o->fd, o->vm, 0);
		check_render_owner(&owners[0], 0); check_render_owner(&owners[1], 1);
	}
	for (unsigned i = 0; i < 2; i++) { check_render_owner(&owners[i], i); CHECK(close(owners[i].fd) == 0); }
	if (cleanup) puts("G17P_RENDER_VM_CLEANUP_PASS exact caller spans unbound from active and inactive roots; both output images preserved");
	total_checks += (unsigned)(checks - checkpoint);
	printf("G17P_NATIVE_RENDER_VM_PASS %u alternating renders, colliding DVAs, both complete images and inactive guards/sentinels, timestamps and successful fences; checks=%llu\n", count, (unsigned long long)total_checks);
	return 0;
}
#endif
