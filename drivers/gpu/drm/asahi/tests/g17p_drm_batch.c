/* SPDX-License-Identifier: MIT */
/* Four synchronous eight-command batches, with independently owned outputs. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include "g17p_drm_batch_workload.h"
#include "g17p_drm_sync.h"
#include "g17p_drm_timestamp.h"

struct command {
	struct drm_asahi_cmd_header attachment_header;
	struct drm_asahi_attachment attachment;
	struct drm_asahi_cmd_header header;
	struct drm_asahi_cmd_compute compute;
};
static void outputs_check(float **output, unsigned completed)
{
	for (unsigned graph = 0; graph < 32; graph++) {
		if (graph < completed)
			for (unsigned i = 0; i < 64; i++)
				CHECK(output[graph][i] == 2000.25f + graph * 129.0f + i);
		for (unsigned i = graph < completed ? 256 : 0; i < PAGE; i++)
			CHECK(((unsigned char *)output[graph])[i] == 0xa5);
	}
}
int main(void)
{
	setbuf(stdout, NULL);
	int fd = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	uint32_t vm = vm_new(fd);
	struct drm_asahi_queue_create queue = { .vm_id = vm, .usc_exec_base = EXEC };
	OK(fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &queue);
	float *output[32] = {0};
	CHECK(sizeof(batch_workloads) / sizeof(batch_workloads[0]) == 32);
	for (unsigned i = 0; i < sizeof(workloads) / sizeof(workloads[0]); i++) {
		const struct workload *w = &workloads[i];
		uint32_t bo = bo_new(fd, w->size, DRM_ASAHI_GEM_WRITEBACK, 0);
		void *map = bo_map(fd, bo, w->size);
		memcpy(map, w->data, w->size);
		int keep = 0;
		for (unsigned j = 0; j < 32; j++) if (w->address == batch_workloads[j].output) {
			output[j] = map; keep = 1; memset(map, 0xa5, PAGE);
		}
		if (!keep) CHECK(munmap(map, w->size) == 0);
		bind(fd, vm, bo, w->address, w->size, 0,
		     DRM_ASAHI_BIND_READ | (w->writable ? DRM_ASAHI_BIND_WRITE : 0), 0);
	}
	for (unsigned i = 0; i < 32; i++) CHECK(output[i] != NULL);
	struct command commands[8];
	struct timestamp_test timestamps = {0};
	struct sync_test sync = {0};
	struct drm_asahi_submit submit = { .queue_id = queue.queue_id,
		.cmdbuf = (uintptr_t)commands, .cmdbuf_size = sizeof(commands) };
	for (unsigned batch = 0; batch < 4; batch++) {
		memset(commands, 0, sizeof(commands));
		for (unsigned j = 0; j < 8; j++) {
			const struct batch_workload *w = &batch_workloads[batch * 8 + j];
			struct command *c = &commands[j];
			c->attachment_header = (struct drm_asahi_cmd_header){
				.cmd_type = DRM_ASAHI_SET_COMPUTE_ATTACHMENTS, .size = sizeof(c->attachment),
				.vdm_barrier = DRM_ASAHI_BARRIER_NONE, .cdm_barrier = DRM_ASAHI_BARRIER_NONE };
			c->attachment.pointer = w->output; c->attachment.size = PAGE;
			c->header = (struct drm_asahi_cmd_header){ .cmd_type = DRM_ASAHI_CMD_COMPUTE,
				.size = sizeof(c->compute), .vdm_barrier = DRM_ASAHI_BARRIER_NONE,
				.cdm_barrier = j }; /* previous command, or previous batch for zero */
			c->compute.cdm_ctrl_stream_base = w->cdm;
			c->compute.cdm_ctrl_stream_end = w->cdm + w->cdm_size;
		}
		if (batch == 0) {
			commands[7].compute.flags = 1;
			BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL);
			commands[7].compute.flags = 0; commands[7].header.cdm_barrier = 8;
			BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL);
			commands[7].header.cdm_barrier = 7;
			outputs_check(output, 0);
			timestamp_setup(fd, &timestamps, &commands[0].compute, &submit);
			sync_setup(fd, &sync, &submit);
		}
		for (unsigned j = 0; j < 8; j++)
			timestamp_before(fd, &timestamps, &commands[j].compute, batch * 8 + j);
		if (batch == 3) {
			struct command too_many[9];
			memcpy(too_many, commands, sizeof(commands)); too_many[8] = commands[0]; too_many[8].header.cdm_barrier = 9;
			struct drm_asahi_submit rejected = submit;
			rejected.cmdbuf = (uintptr_t)too_many; rejected.cmdbuf_size = sizeof(too_many);
			BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &rejected, EINVAL);
			outputs_check(output, 24);
			CHECK(sync_stamp(fd, sync.binary) == sync.last_stamp);
			CHECK(sync_point(fd, sync.timeline) == 3);
		}
		sync_before(fd, &sync, batch, vm, (const unsigned char *)output[0]);
		OK(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit);
		sync_after(fd, &sync, batch);
		for (unsigned j = 0; j < 8; j++) {
			unsigned n = batch * 8 + j;
			memcpy(timestamps.saved[n], timestamps.map + PAGE + 64 + n * 16, 16);
			uint64_t start = timestamps.saved[n][0], end = timestamps.saved[n][1];
			printf("TIMESTAMP %u start=%" PRIu64 " end=%" PRIu64 "\n", n, start, end);
			CHECK(start != 0 && end > start);
			if (n) CHECK(start >= timestamps.saved[n - 1][1]);
		}
		timestamp_check(&timestamps, (batch + 1) * 8);
		outputs_check(output, (batch + 1) * 8);
		printf("BATCH %u PASS 8 unique outputs; completion fence after all timestamps\n", batch);
	}
	sync.entries[3].timeline_value = 5;
	commands[7].compute.flags = 1;
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL);
	CHECK(sync_stamp(fd, sync.binary) == sync.last_stamp);
	CHECK(sync_point(fd, sync.timeline) == 4);
	outputs_check(output, 32);
	timestamp_finish(fd, &timestamps, 32);
	for (unsigned j = 0; j < 32; j++) CHECK(munmap(output[j], PAGE) == 0);
	CHECK(close(fd) == 0);
	printf("G17P_NATIVE_BATCH_PASS 4 batches, 32 independent outputs / 2048 floats, command barriers, timestamps and batch fences; checks=%u\n", checks);
	return 0;
}
