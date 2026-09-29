/* SPDX-License-Identifier: MIT */
/* Two DRM files, colliding DVAs, and source-default synchronous VM handoff. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include "g17p_drm_workload.h"

struct command {
	struct drm_asahi_cmd_header attachment_header;
	struct drm_asahi_attachment attachment;
	struct drm_asahi_cmd_header header;
	struct drm_asahi_cmd_compute compute;
};
struct owner {
	int fd;
	uint32_t vm, queue, binary, timeline, object;
	float *a, *b, *output;
	unsigned char *allocation, *timestamps, *sentinel;
	uint64_t saved[16][2];
	int last;
};
static void *make_buffer(struct owner *owner, uint64_t address, size_t size,
	const void *data, int writable)
{
	uint32_t bo = bo_new(owner->fd, size, DRM_ASAHI_GEM_WRITEBACK, 0);
	void *map = bo_map(owner->fd, bo, size);
	if (data) memcpy(map, data, size); else memset(map, 0, size);
	bind(owner->fd, owner->vm, bo, address, size, 0, DRM_ASAHI_BIND_READ | (writable ? DRM_ASAHI_BIND_WRITE : 0), 0);
	return map;
}
static void create_owner(struct owner *o, unsigned index)
{
	o->last = -1; o->fd = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC); CHECK(o->fd >= 0);
	o->vm = vm_new(o->fd);
	struct drm_asahi_queue_create q = { .vm_id = o->vm, .usc_exec_base = EXEC };
	OK(o->fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q); o->queue = q.queue_id;
	for (unsigned i = 0; i < sizeof(workloads)/sizeof(workloads[0]); i++) {
		const struct workload *w = &workloads[i];
		void *map = make_buffer(o, w->address, w->size, w->data, w->writable);
		CHECK(munmap(map, w->size) == 0);
	}
	o->a = make_buffer(o, COMPUTE_INPUT_A, PAGE, NULL, 1);
	o->b = make_buffer(o, COMPUTE_INPUT_B, PAGE, NULL, 1);
	uint32_t bo = bo_new(o->fd, PAGE * 3, DRM_ASAHI_GEM_WRITEBACK, 0);
	o->allocation = bo_map(o->fd, bo, PAGE * 3); memset(o->allocation, 0x5a, PAGE * 3);
	o->output = (void *)(o->allocation + PAGE); memset(o->output, 0xa5, PAGE);
	bind(o->fd, o->vm, bo, COMPUTE_OUTPUT, PAGE, PAGE, RW, 0);
	o->sentinel = make_buffer(o, EXEC + 0x70000000 + index * PAGE, PAGE, NULL, 1);
	memset(o->sentinel, 0x61 + index, PAGE);
	uint32_t ts = bo_new(o->fd, PAGE * 3, DRM_ASAHI_GEM_WRITEBACK, 0);
	o->timestamps = bo_map(o->fd, ts, PAGE * 3); memset(o->timestamps, 0xa5, PAGE * 3);
	struct drm_asahi_gem_bind_object object = { .op = DRM_ASAHI_BIND_OBJECT_OP_BIND,
		.flags = DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS, .handle = ts, .offset = PAGE, .range = PAGE };
	OK(o->fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &object); o->object = object.object_handle;
	struct drm_syncobj_create binary = {0}, timeline = {0};
	OK(o->fd, DRM_IOCTL_SYNCOBJ_CREATE, &binary); OK(o->fd, DRM_IOCTL_SYNCOBJ_CREATE, &timeline);
	o->binary = binary.handle; o->timeline = timeline.handle;
}
static void check_owner(struct owner *o, unsigned index)
{
	unsigned used = o->last < 0 ? 0 : 256;
	if (used) for (unsigned i = 0; i < 64; i++) CHECK(o->output[i] == 1000.5f + index * 10000.0f + o->last * 100.25f + i);
	for (unsigned i = used; i < PAGE; i++) CHECK(((unsigned char *)o->output)[i] == 0xa5);
	for (unsigned i = 0; i < PAGE; i++) {
		CHECK(o->allocation[i] == 0x5a); CHECK(o->allocation[PAGE * 2 + i] == 0x5a);
		CHECK(o->sentinel[i] == 0x61 + index);
	}
	for (unsigned i = 0; i < PAGE * 3; i++) {
		if (i >= PAGE + 64 && i < PAGE + 64 + (o->last + 1) * 16) continue;
		CHECK(o->timestamps[i] == 0xa5);
	}
	CHECK(memcmp(o->saved, o->timestamps + PAGE + 64, (o->last + 1) * 16) == 0);
}
int main(void)
{
	setbuf(stdout, NULL);
	struct owner owners[2] = {{0}};
	for (unsigned i = 0; i < 2; i++) create_owner(&owners[i], i);
	uint64_t previous = 0;
	for (unsigned n = 0; n < 32; n++) {
		unsigned index = n % 2, generation = n / 2; struct owner *o = &owners[index];
		for (unsigned i = 0; i < 64; i++) { o->a[i] = 1000.0f + index * 10000.0f + generation * 100.0f + i; o->b[i] = 0.5f + generation * 0.25f; }
		memset(o->output, 0xa5, PAGE);
		struct command command = {
			.attachment_header = { .cmd_type = DRM_ASAHI_SET_COMPUTE_ATTACHMENTS, .size = sizeof(struct drm_asahi_attachment), .vdm_barrier = DRM_ASAHI_BARRIER_NONE, .cdm_barrier = DRM_ASAHI_BARRIER_NONE },
			.attachment = { .pointer = COMPUTE_OUTPUT, .size = PAGE },
			.header = { .cmd_type = DRM_ASAHI_CMD_COMPUTE, .size = sizeof(struct drm_asahi_cmd_compute), .vdm_barrier = DRM_ASAHI_BARRIER_NONE, .cdm_barrier = DRM_ASAHI_BARRIER_NONE },
			.compute = { .cdm_ctrl_stream_base = COMPUTE_CDM, .cdm_ctrl_stream_end = COMPUTE_CDM + COMPUTE_CDM_SIZE },
		};
		command.compute.ts.start = (struct drm_asahi_timestamp){ .handle = o->object, .offset = 64 + generation * 16 };
		command.compute.ts.end = (struct drm_asahi_timestamp){ .handle = o->object, .offset = 72 + generation * 16 };
		struct drm_asahi_sync syncs[] = {
			{ .sync_type = DRM_ASAHI_SYNC_SYNCOBJ, .handle = o->binary },
			{ .sync_type = DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ, .handle = o->timeline, .timeline_value = generation + 1 },
		};
		struct drm_asahi_submit submit = { .queue_id = o->queue, .cmdbuf = (uintptr_t)&command, .cmdbuf_size = sizeof(command), .syncs = (uintptr_t)syncs, .out_sync_count = 2 };
		struct drm_syncobj_array reset = { .handles = (uintptr_t)&o->binary, .count_handles = 1 };
		OK(o->fd, DRM_IOCTL_SYNCOBJ_RESET, &reset);
		if (n == 1) {
			command.compute.flags = 1;
			BAD(o->fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); command.compute.flags = 0;
			check_owner(&owners[0], 0); check_owner(&owners[1], 1);
		}
		errno = 0; int rc = ioctl(o->fd, DRM_IOCTL_ASAHI_SUBMIT, &submit), saved_errno = errno;
		printf("VM_SUBMIT %u owner=%u generation=%u rc=%d errno=%d\n", n, index, generation, rc, saved_errno); CHECK(rc == 0);
		o->last = generation;
		memcpy(o->saved[generation], o->timestamps + PAGE + 64 + generation * 16, 16);
		uint64_t start = o->saved[generation][0], end = o->saved[generation][1];
		CHECK(start > previous && end > start); previous = end;
		printf("VM_TIMESTAMP %u owner=%u start=%" PRIu64 " end=%" PRIu64 "\n", n, index, start, end);
		for (unsigned owner = 0; owner < 2; owner++) check_owner(&owners[owner], owner);
		struct drm_syncobj_wait wait = { .handles = (uintptr_t)&o->binary, .count_handles = 1, .timeout_nsec = 0 };
		uint64_t point = generation + 1;
		struct drm_syncobj_timeline_wait twait = { .handles = (uintptr_t)&o->timeline, .points = (uintptr_t)&point, .count_handles = 1, .timeout_nsec = 0 };
		OK(o->fd, DRM_IOCTL_SYNCOBJ_WAIT, &wait); OK(o->fd, DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT, &twait);
		if (n == 31) {
			submit.queue_id = owners[0].queue;
			command.compute.ts.start.handle = command.compute.ts.end.handle = owners[0].object;
			syncs[0].handle = owners[0].binary; syncs[1].handle = owners[0].timeline;
			syncs[1].timeline_value = 17;
			command.compute.flags = 1;
			BAD(owners[0].fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL);
		}
	}
	for (unsigned i = 0; i < 2; i++) {
		struct owner *o = &owners[i]; check_owner(o, i);
		CHECK(munmap(o->a, PAGE) == 0); CHECK(munmap(o->b, PAGE) == 0);
		CHECK(munmap(o->allocation, PAGE * 3) == 0); CHECK(munmap(o->sentinel, PAGE) == 0);
		CHECK(munmap(o->timestamps, PAGE * 3) == 0); CHECK(close(o->fd) == 0);
	}
	printf("G17P_NATIVE_COMPUTE_VM_PASS 32 alternating submissions from two files, colliding caller DVAs, exact independent outputs, inactive images/guards/sentinels preserved, timestamps and fences; checks=%u\n", checks);
	return 0;
}
