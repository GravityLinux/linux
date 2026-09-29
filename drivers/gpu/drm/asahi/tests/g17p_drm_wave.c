/* SPDX-License-Identifier: MIT */
/* 258 commands in bounded waves, distinct outputs and changing inputs. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include "g17p_drm_wave_workload.h"
#include "g17p_drm_sync.h"
#include "g17p_drm_timestamp.h"

struct command {
	struct drm_asahi_cmd_header attachment_header;
	struct drm_asahi_attachment attachment;
	struct drm_asahi_cmd_header header;
	struct drm_asahi_cmd_compute compute;
};
static void outputs_check(float **output, const int *epochs)
{
	for (unsigned graph = 0; graph < 64; graph++) {
		if (epochs[graph] >= 0)
			for (unsigned i = 0; i < 64; i++)
				CHECK(output[graph][i] == 2000.25f + graph * 129.0f + epochs[graph] * 8192.0f + i);
		for (unsigned i = epochs[graph] >= 0 ? 256 : 0; i < PAGE; i++)
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
	float *output[64] = {0}, *inputs[64] = {0};
	int epochs[64]; for (unsigned j = 0; j < 64; j++) epochs[j] = -1;
	CHECK(sizeof(batch_workloads) / sizeof(batch_workloads[0]) == 64);
	for (unsigned i = 0; i < sizeof(workloads) / sizeof(workloads[0]); i++) {
		const struct workload *w = &workloads[i];
		uint32_t bo = bo_new(fd, w->size, DRM_ASAHI_GEM_WRITEBACK, 0);
		void *map = bo_map(fd, bo, w->size);
		memcpy(map, w->data, w->size);
		int keep = 0;
		for (unsigned j = 0; j < 64; j++) if (w->address == batch_workloads[j].output) {
			output[j] = map; keep = 1; memset(map, 0xa5, PAGE);
		}
		for (unsigned j = 0; j < 64; j++) if (w->address == batch_workloads[j].input_a) { inputs[j] = map; keep = 1; }
		if (!keep) CHECK(munmap(map, w->size) == 0);
		bind(fd, vm, bo, w->address, w->size, 0,
		     DRM_ASAHI_BIND_READ | (w->writable ? DRM_ASAHI_BIND_WRITE : 0), 0);
	}
	for (unsigned i = 0; i < 64; i++) CHECK(output[i] && inputs[i]);

	struct command commands[64];
	struct timestamp_test timestamps = {0};
	struct sync_test sync = {0};
	struct drm_asahi_submit submit = { .queue_id = queue.queue_id, .cmdbuf = (uintptr_t)commands };
	puts("G17P_NATIVE_WAVE_BEGIN 64/64/64/64/2 commands, 258 unique inputs and status/timestamp pairs");
	for (unsigned batch = 0; batch < 5; batch++) {
		unsigned count = batch == 4 ? 2 : 64, first = batch * 64;
		submit.cmdbuf_size = count * sizeof(commands[0]); memset(commands, 0, sizeof(commands));
		for (unsigned j = 0; j < count; j++) {
			const struct batch_workload *w = &batch_workloads[j];
			for (unsigned i = 0; i < 64; i++) inputs[j][i] = 2000.0f + j * 128.0f + batch * 8192.0f + i;
			memset(output[j], 0xa5, PAGE); epochs[j] = -1;
			struct command *c = &commands[j];
			c->attachment_header = (struct drm_asahi_cmd_header){ .cmd_type = DRM_ASAHI_SET_COMPUTE_ATTACHMENTS,
				.size = sizeof(c->attachment), .vdm_barrier = DRM_ASAHI_BARRIER_NONE, .cdm_barrier = DRM_ASAHI_BARRIER_NONE };
			c->attachment.pointer = w->output; c->attachment.size = PAGE;
			c->header = (struct drm_asahi_cmd_header){ .cmd_type = DRM_ASAHI_CMD_COMPUTE, .size = sizeof(c->compute),
				.vdm_barrier = DRM_ASAHI_BARRIER_NONE, .cdm_barrier = j };
			c->compute.cdm_ctrl_stream_base = w->cdm; c->compute.cdm_ctrl_stream_end = w->cdm + w->cdm_size;
		}
		if (batch == 0) {
			commands[63].compute.flags = 1; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); commands[63].compute.flags = 0;
			commands[63].header.cdm_barrier = 64; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); commands[63].header.cdm_barrier = 63;
			outputs_check(output, epochs);
			timestamp_setup(fd, &timestamps, &commands[0].compute, &submit); sync_setup(fd, &sync, &submit);
			/* Rebind before assigning any references in this first full buffer. */
			timestamp_before(fd, &timestamps, &commands[16].compute, 16);
		}
		if (batch == 2) { timestamp_unbind(fd, &timestamps); timestamp_bind(fd, &timestamps); }
		for (unsigned j = 0; j < count; j++) if (first + j != 16)
			timestamp_before(fd, &timestamps, &commands[j].compute, first + j);
		if (batch == 4) {
			commands[2] = commands[0]; commands[2].header.cdm_barrier = 2;
			submit.cmdbuf_size = 3 * sizeof(commands[0]); BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EOPNOTSUPP);
			submit.cmdbuf_size = count * sizeof(commands[0]); outputs_check(output, epochs);
			CHECK(sync_point(fd, sync.timeline) == 4); CHECK(sync_stamp(fd, sync.binary) == sync.last_stamp);
		}
		sync_before(fd, &sync, batch, vm, (const unsigned char *)output[0]);
		OK(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit); sync_after(fd, &sync, batch);
		for (unsigned j = 0; j < count; j++) {
			unsigned n = first + j; epochs[j] = batch;
			memcpy(timestamps.saved[n], timestamps.map + PAGE + 64 + n * 16, 16);
			uint64_t start = timestamps.saved[n][0], end = timestamps.saved[n][1];
			CHECK(start && end > start); if (n) CHECK(start >= timestamps.saved[n-1][1]);
		}
		timestamp_check(&timestamps, first + count); outputs_check(output, epochs);
		printf("WAVE_BATCH %u PASS %u outputs, independent statuses/timestamps and aggregate fences\n", batch, count);
	}
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EOPNOTSUPP); outputs_check(output, epochs);
	timestamp_finish(fd, &timestamps, 258); sync_finish(fd, &sync, 5);
	CHECK(close(fd) == 0);
	printf("G17P_NATIVE_WAVE_PASS 258 commands, 16512 floats and guarded outputs, 516 timestamps, 5 aggregate fences, capacity rejection; checks=%u\n", checks);
	return 0;
}
