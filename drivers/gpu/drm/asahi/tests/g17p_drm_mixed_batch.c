/* SPDX-License-Identifier: MIT */
/* One mixed ioctl, distinct caller output for every command, both orders. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include "g17p_drm_render_batch_workload.h"
#define workload compute_workload
#define workloads compute_workloads
#include "g17p_drm_mixed_workload.h"
#undef workloads
#undef workload

static void render_check(unsigned char **images, int complete)
{
	for (unsigned target = 0; target < 16; target++) {
		float value = (target < 8 ? RENDER_TRIANGLES : RENDER_SECOND_TRIANGLES) * ((target % 8 + 1) / 8.0f);
		unsigned char expected[4]; memcpy(expected, &value, 4);
		for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++) {
			unsigned char want = complete ? 0 : 0xa5;
			if (complete && byte >= RENDER_PIXEL_0 && byte < RENDER_PIXEL_0 + 4) want = expected[byte - RENDER_PIXEL_0];
			if (complete && byte >= RENDER_PIXEL_1 && byte < RENDER_PIXEL_1 + 4) want = expected[byte - RENDER_PIXEL_1];
			CHECK(images[target][byte] == want);
		}
	}
}
static void compute_check(unsigned char **outputs, unsigned completed)
{
	for (unsigned n = 0; n < 2; n++) {
		for (unsigned byte = 0; byte < PAGE; byte++) {
			unsigned char want = 0xa5;
			if (n < completed && byte < 256) {
				float value = 2000.25f + (8 + n) * 129.0f + byte / 4;
				unsigned char bytes[4]; memcpy(bytes, &value, 4); want = bytes[byte % 4];
			}
			CHECK(outputs[n][byte] == want);
		}
	}
}
int main(int argc, char **argv)
{
	setbuf(stdout, NULL);
	int compute_first = argc == 2 && !strcmp(argv[1], "--compute-first");
	int compute_wave = argc == 2 && !strcmp(argv[1], "--compute-wave");
	CHECK(argc == 1 || compute_first || compute_wave);
	unsigned kinds[4];
	for (unsigned step = 0; step < 4; step++)
		kinds[step] = compute_wave ? (step == 1 || step == 2) : ((step & 1) ^ compute_first);

	int fd = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC); CHECK(fd >= 0);
	uint32_t vm = vm_new(fd);
	struct drm_asahi_queue_create q = { .vm_id = vm, .usc_exec_base = EXEC };
	OK(fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q);
	unsigned char *images[16] = {0}, *outputs[2] = {0};
	for (unsigned i = 0; i < sizeof(workloads) / sizeof(workloads[0]); i++) {
		const struct workload *w = &workloads[i];
		uint32_t bo = bo_new(fd, w->size, DRM_ASAHI_GEM_WRITEBACK, 0);
		void *map = bo_map(fd, bo, w->size); memcpy(map, w->data, w->size);
		if (w->writable) {
			for (unsigned j = 0; j < 16; j++) if (w->address == render_outputs[j]) images[j] = map;
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
		int output = -1;
		for (unsigned n = 0; n < 2; n++) if (w->address == batch_workloads[n].output) output = n;
		if (output >= 0) { outputs[output] = map; memset(map, 0xa5, PAGE); }
		else CHECK(munmap(map, w->size) == 0);
		bind(fd, vm, bo, w->address, w->size, 0, DRM_ASAHI_BIND_READ | (w->writable ? DRM_ASAHI_BIND_WRITE : 0), 0);
	}
	for (unsigned j = 0; j < 16; j++) { CHECK(images[j] != NULL); memset(images[j], 0xa5, RENDER_OUTPUT_SIZE); }
	CHECK(outputs[0] && outputs[1]);
	uint32_t ts_bo = bo_new(fd, PAGE * 3, DRM_ASAHI_GEM_WRITEBACK, 0);
	unsigned char *timestamps = bo_map(fd, ts_bo, PAGE * 3); memset(timestamps, 0xa5, PAGE * 3);
	struct drm_asahi_gem_bind_object object = { .op = DRM_ASAHI_BIND_OBJECT_OP_BIND,
		.flags = DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS, .handle = ts_bo, .offset = PAGE, .range = PAGE };
	OK(fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &object);

	struct compute_packet { struct drm_asahi_cmd_header header; struct drm_asahi_cmd_compute cmd; };
	union { uint64_t align; unsigned char bytes[2 * sizeof(render_command) + 3 * sizeof(struct compute_packet)]; } buffer;
	struct drm_asahi_cmd_header *headers[4];
	uint32_t *flags[4];
	struct drm_asahi_timestamp *stamps[4][4];
	unsigned counts[2] = {0}, position = 0;
	for (unsigned step = 0; step < 4; step++) {
		unsigned kind = kinds[step], n = counts[kind];
		if (kind) {
			struct compute_packet packet = { .header = { .cmd_type = DRM_ASAHI_CMD_COMPUTE, .size = sizeof(packet.cmd) },
				.cmd = { .cdm_ctrl_stream_base = batch_workloads[n].cdm,
					.cdm_ctrl_stream_end = batch_workloads[n].cdm + batch_workloads[n].cdm_size } };
			memcpy(buffer.bytes + position, &packet, sizeof(packet));
			struct compute_packet *at = (void *)(buffer.bytes + position);
			headers[step] = &at->header; flags[step] = &at->cmd.flags;
			stamps[step][0] = &at->cmd.ts.start; stamps[step][1] = &at->cmd.ts.end;
			position += sizeof(packet);
		} else {
			memcpy(buffer.bytes + position, n ? render_second_command : render_command, sizeof(render_command));
			struct drm_asahi_cmd_render *at = (void *)(buffer.bytes + position + sizeof(render_command) - sizeof(*at));
			headers[step] = (void *)((unsigned char *)at - sizeof(*headers[step])); flags[step] = &at->flags;
			stamps[step][0] = &at->ts_vtx.start; stamps[step][1] = &at->ts_vtx.end;
			stamps[step][2] = &at->ts_frag.start; stamps[step][3] = &at->ts_frag.end;
			position += sizeof(render_command);
		}
		headers[step]->vdm_barrier = counts[0]; headers[step]->cdm_barrier = counts[1]; counts[kind]++;
	}
	struct drm_syncobj_create binary = {0}, timeline = {0};
	OK(fd, DRM_IOCTL_SYNCOBJ_CREATE, &binary); OK(fd, DRM_IOCTL_SYNCOBJ_CREATE, &timeline);
	struct drm_asahi_sync syncs[] = {
		{ .sync_type = DRM_ASAHI_SYNC_SYNCOBJ, .handle = binary.handle },
		{ .sync_type = DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ, .handle = timeline.handle },
	};
	struct drm_asahi_submit submit = { .queue_id = q.queue_id, .cmdbuf = (uintptr_t)buffer.bytes,
		.cmdbuf_size = position, .syncs = (uintptr_t)syncs, .out_sync_count = 2 };
	struct drm_syncobj_array reset = { .handles = (uintptr_t)&binary.handle, .count_handles = 1 };
	struct drm_syncobj_wait wait = { .handles = (uintptr_t)&binary.handle, .count_handles = 1, .timeout_nsec = 0 };
	uint64_t point = 0;
	struct drm_syncobj_timeline_wait twait = { .handles = (uintptr_t)&timeline.handle,
		.points = (uintptr_t)&point, .count_handles = 1, .timeout_nsec = 0 };
	unsigned char expected[PAGE * 3]; memset(expected, 0xa5, sizeof(expected));
	unsigned batches = compute_first ? 2 : 1;
	uint64_t last = 0;
	printf("G17P_NATIVE_MIXED_BATCH_BEGIN %s, %u batches, every command has distinct outputs\n",
		compute_first ? "C/R/C/R" : compute_wave ? "R/C/C/R" : "R/C/R/C", batches);
	for (unsigned batch = 0; batch < batches; batch++) {
		for (unsigned target = 0; target < 16; target++) memset(images[target], 0xa5, RENDER_OUTPUT_SIZE);
		for (unsigned n = 0; n < 2; n++) memset(outputs[n], 0xa5, PAGE);
		for (unsigned step = 0; step < 4; step++) {
			unsigned count = kinds[step] ? 2 : 4;
			for (unsigned i = 0; i < count; i++) {
				unsigned offset = 64 + (batch * 4 + step) * 64 + i * 8;
				stamps[step][i]->handle = object.object_handle; stamps[step][i]->offset = offset;
				memset(timestamps + PAGE + offset, 0, 8); memset(expected + PAGE + offset, 0, 8);
			}
		}
		syncs[1].timeline_value = point = batch + 1;
		OK(fd, DRM_IOCTL_SYNCOBJ_RESET, &reset);
		/* Every trailing failure must leave all prior commands unpublished. */
		*flags[3] |= 1U << 31; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); *flags[3] &= ~(1U << 31);
		uint16_t barrier = headers[3]->vdm_barrier;
		headers[3]->vdm_barrier = 3; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL); headers[3]->vdm_barrier = barrier;
		stamps[3][0]->handle = 0xdeadbeef; BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, ENOENT); stamps[3][0]->handle = object.object_handle;
		/* Valid UAPI mapping, forbidden driver-private overlap: reject even
		 * when the first command is compute and only a later render uses it. */
		uint32_t collision = bo_new(fd, PAGE, DRM_ASAHI_GEM_WRITEBACK, 0);
		bind(fd, vm, collision, 0x10001b0000, PAGE, 0, DRM_ASAHI_BIND_READ | DRM_ASAHI_BIND_WRITE, 0);
		BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EINVAL);
		bind(fd, vm, 0, 0x10001b0000, PAGE, 0, DRM_ASAHI_BIND_UNBIND, 0); { struct drm_gem_close gc = { .handle = collision }; OK(fd, DRM_IOCTL_GEM_CLOSE, &gc); }
		render_check(images, 0); compute_check(outputs, 0); CHECK(memcmp(timestamps, expected, sizeof(expected)) == 0);
		uint64_t submitted = UINT64_MAX;
		struct drm_syncobj_timeline_array query = { .handles = (uintptr_t)&timeline.handle,
			.points = (uintptr_t)&submitted, .count_handles = 1, .flags = DRM_SYNCOBJ_QUERY_FLAGS_LAST_SUBMITTED };
		OK(fd, DRM_IOCTL_SYNCOBJ_QUERY, &query); CHECK(submitted == batch);
		struct drm_syncobj_handle empty = { .handle = binary.handle, .flags = DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE };
		BAD(fd, DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &empty, EINVAL);
		errno = 0; int result = ioctl(fd, DRM_IOCTL_ASAHI_SUBMIT, &submit), error = errno;
		printf("MIXED_BATCH %u rc=%d errno=%d\n", batch, result, error); CHECK(result == 0);
		for (unsigned step = 0; step < 4; step++) {
			unsigned count = kinds[step] ? 2 : 4, offset = PAGE + 64 + (batch * 4 + step) * 64;
			uint64_t values[4]; memcpy(values, timestamps + offset, count * 8);
			for (unsigned i = 0; i < count; i += 2) CHECK(values[i] && values[i + 1] > values[i]);
			CHECK(values[0] > last); last = values[count - 1];
			memcpy(expected + offset, values, count * 8);
			printf("MIXED_BATCH_COMMAND %u start=%" PRIu64 " end=%" PRIu64 "\n", batch * 4 + step, values[0], last);
		}
		CHECK(memcmp(timestamps, expected, sizeof(expected)) == 0);
		render_check(images, 1); compute_check(outputs, 2);
		OK(fd, DRM_IOCTL_SYNCOBJ_WAIT, &wait); OK(fd, DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT, &twait);
	}
	CHECK(close(fd) == 0);
	printf("G17P_NATIVE_MIXED_BATCH_PASS %u batches, %u renders / %u compute, every output and timestamp, cross-engine barriers, aggregate fences and rejected suffixes; checks=%u\n",
		batches, batches * 2, batches * 2, checks);
	return 0;
}
