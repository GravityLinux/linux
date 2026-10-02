/* SPDX-License-Identifier: MIT */
/* Bounded pool-zero growth refusal and independently backed pool-one work. */
#define G17P_RENDER_VM_HELPERS
#include "g17p_drm_render_vm.c"
#include "g17p_drm_limit_recovery_workload.h"

static void execute_limit(struct render_owner *o, unsigned generation, int expected_status)
{
	struct drm_asahi_cmd_render *cmd = (void *)(o->command + sizeof(o->command) - sizeof(*cmd));
	struct drm_asahi_timestamp *refs[] = { &cmd->ts_vtx.start, &cmd->ts_vtx.end, &cmd->ts_frag.start, &cmd->ts_frag.end };
	for (unsigned i = 0; i < 4; i++)
		*refs[i] = (struct drm_asahi_timestamp){ .handle = o->object, .offset = 64 + generation * 32 + i * 8 };
	struct drm_syncobj_array reset = { .handles = (uintptr_t)&o->binary, .count_handles = 1 };
	OK(o->fd, DRM_IOCTL_SYNCOBJ_RESET, &reset);
	struct drm_asahi_sync syncs[] = {
		{ .sync_type = DRM_ASAHI_SYNC_SYNCOBJ, .handle = o->binary },
		{ .sync_type = DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ, .handle = o->timeline, .timeline_value = generation + 1 },
	};
	struct drm_asahi_submit submit = { .queue_id = o->queue, .cmdbuf = (uintptr_t)o->command,
		.cmdbuf_size = sizeof(o->command), .syncs = (uintptr_t)syncs, .out_sync_count = 2 };
	errno = 0;
	int result = ioctl(o->fd, DRM_IOCTL_ASAHI_SUBMIT, &submit), error = errno;
	printf("LIMIT_SUBMIT expected=%d rc=%d errno=%d\n", expected_status, result, error);
	CHECK(result == 0);
	struct drm_syncobj_handle exported = { .handle = o->binary,
		.flags = DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE };
	OK(o->fd, DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &exported);
	struct sync_file_info info = {0}; CHECK(ioctl(exported.fd, SYNC_IOC_FILE_INFO, &info) == 0);
	printf("LIMIT_FENCE expected=%d status=%d\n", expected_status, info.status);
	CHECK(info.status == expected_status); CHECK(close(exported.fd) == 0);
	struct drm_syncobj_wait wait = { .handles = (uintptr_t)&o->binary, .count_handles = 1, .timeout_nsec = 0 };
	uint64_t point = generation + 1;
	struct drm_syncobj_timeline_wait twait = { .handles = (uintptr_t)&o->timeline, .points = (uintptr_t)&point,
		.count_handles = 1, .timeout_nsec = 0 };
	OK(o->fd, DRM_IOCTL_SYNCOBJ_WAIT, &wait); OK(o->fd, DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT, &twait);
}

int main(void)
{
	setbuf(stdout, NULL);
	struct render_owner owners[2] = {{0}};
	for (unsigned i = 0; i < 2; i++) create_render_owner(&owners[i], i);
	puts("G17P_OWNED_POOL_LIMIT_BEGIN pool0=8 pool1=unbounded independent alternating pairs");
	execute_limit(&owners[0], 0, -ENOMEM);
	check_render_owner(&owners[1], 1);
	for (unsigned byte = 0; byte < PAGE; byte++) CHECK(owners[0].sentinel[byte] == 0x61);
	for (unsigned byte = 0; byte < PAGE * 3; byte++)
		if (byte < PAGE + 64 || byte >= PAGE + 96) CHECK(owners[0].timestamps[byte] == 0xa5);
	unsigned char *snapshots[8];
	for (unsigned target = 0; target < 8; target++) {
		snapshots[target] = malloc(RENDER_OUTPUT_SIZE); CHECK(snapshots[target]);
		memcpy(snapshots[target], owners[0].images[target], RENDER_OUTPUT_SIZE);
	}
	unsigned char timestamps[PAGE * 3]; memcpy(timestamps, owners[0].timestamps, sizeof(timestamps));
	execute_limit(&owners[1], 0, 1);
	owners[1].last = 0; memcpy(owners[1].saved[0], owners[1].timestamps + PAGE + 64, 32);
	uint64_t *ts = owners[1].saved[0]; CHECK(ts[0] && ts[1] > ts[0] && ts[2] >= ts[1] && ts[3] > ts[2]);
	check_render_owner(&owners[1], 1);
	for (unsigned target = 0; target < 8; target++) {
		for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++)
			CHECK(owners[0].images[target][byte] == snapshots[target][byte]);
	}
	CHECK(memcmp(timestamps, owners[0].timestamps, sizeof(timestamps)) == 0);
	for (unsigned byte = 0; byte < PAGE; byte++) CHECK(owners[0].sentinel[byte] == 0x61);
	/* Replace only authored draw data and output GEMs on the failed queue.
	 * Earlier mappings stay alive so complete old backing is still checked. */
	struct render_owner *o = &owners[0];
	struct drm_asahi_cmd_render *cmd = (void *)(o->command + sizeof(o->command) - sizeof(*cmd));
	unsigned found = 0;
	for (unsigned i = 0; i < sizeof(recovery_workloads) / sizeof(recovery_workloads[0]); i++) {
		const struct recovery_workload *w = &recovery_workloads[i];
		if (w->address != cmd->vdm_ctrl_stream_base) continue;
		bind(o->fd, o->vm, 0, w->address, w->size, 0, DRM_ASAHI_BIND_UNBIND, 0);
		uint32_t bo = bo_new(o->fd, w->size, DRM_ASAHI_GEM_WRITEBACK, 0);
		void *map = bo_map(o->fd, bo, w->size); memcpy(map, w->data, w->size);
		bind(o->fd, o->vm, bo, w->address, w->size, 0, DRM_ASAHI_BIND_READ, 0);
		found++;
	}
	CHECK(found == 1);
	unsigned char *old[8];
	for (unsigned target = 0; target < 8; target++) {
		old[target] = o->images[target];
		bind(o->fd, o->vm, 0, render_outputs[target], RENDER_OUTPUT_SIZE, 0, DRM_ASAHI_BIND_UNBIND, 0);
		uint32_t bo = bo_new(o->fd, RENDER_OUTPUT_SIZE, DRM_ASAHI_GEM_WRITEBACK, 0);
		o->images[target] = bo_map(o->fd, bo, RENDER_OUTPUT_SIZE); memset(o->images[target], 0xa5, RENDER_OUTPUT_SIZE);
		bind(o->fd, o->vm, bo, render_outputs[target], RENDER_OUTPUT_SIZE, 0, RW, 0);
	}
	execute_limit(o, 1, 1);
	uint64_t *recovered = (void *)(o->timestamps + PAGE + 96);
	CHECK(recovered[0] && recovered[1] > recovered[0] && recovered[2] >= recovered[1] && recovered[3] > recovered[2]);
	for (unsigned target = 0; target < 8; target++) {
		float value = RECOVERY_RENDER_TRIANGLES * ((target + 1) / 8.0f);
		unsigned char pixel[4]; memcpy(pixel, &value, 4);
		for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++) {
			unsigned char want = 0;
			if (byte >= RENDER_PIXEL_0 && byte < RENDER_PIXEL_0 + 4) want = pixel[byte - RENDER_PIXEL_0];
			if (byte >= RENDER_PIXEL_1 && byte < RENDER_PIXEL_1 + 4) want = pixel[byte - RENDER_PIXEL_1];
			CHECK(o->images[target][byte] == want);
			CHECK(old[target][byte] == snapshots[target][byte]);
		}
		free(snapshots[target]);
	}
	for (unsigned byte = 0; byte < PAGE * 3; byte++)
		if (byte < PAGE + 96 || byte >= PAGE + 128) CHECK(o->timestamps[byte] == timestamps[byte]);
	for (unsigned byte = 0; byte < PAGE; byte++) CHECK(o->sentinel[byte] == 0x61);
	check_render_owner(&owners[1], 1);
	for (unsigned i = 0; i < 2; i++) CHECK(close(owners[i].fd) == 0);
	printf("G17P_OWNED_POOL_LIMIT_PASS own ENOMEM fence, inactive caller untouched, other pool and same-queue fresh GEM recovery have complete images/timestamps/successful fences, failed owner's old backing preserved; checks=%u\n", checks);
	return 0;
}
