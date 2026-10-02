/* SPDX-License-Identifier: MIT */
/* Source shader-store leaf loss, successful retirement, fresh-backing recovery. */
#define G17P_RENDER_VM_HELPERS
#include "g17p_drm_render_vm.c"

static void execute(struct render_owner *o, unsigned generation)
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
	OK(o->fd, DRM_IOCTL_ASAHI_SUBMIT, &submit);
	struct drm_syncobj_handle exported = { .handle = o->binary,
		.flags = DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE };
	OK(o->fd, DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &exported);
	struct sync_file_info info = {0}; CHECK(ioctl(exported.fd, SYNC_IOC_FILE_INFO, &info) == 0);
	printf("SOFT_FAULT_FENCE generation=%u status=%d\n", generation, info.status);
	CHECK(info.status == 1); CHECK(close(exported.fd) == 0);
	struct drm_syncobj_wait wait = { .handles = (uintptr_t)&o->binary, .count_handles = 1, .timeout_nsec = 0 };
	uint64_t point = generation + 1;
	struct drm_syncobj_timeline_wait twait = { .handles = (uintptr_t)&o->timeline, .points = (uintptr_t)&point,
		.count_handles = 1, .timeout_nsec = 0 };
	OK(o->fd, DRM_IOCTL_SYNCOBJ_WAIT, &wait); OK(o->fd, DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT, &twait);
	o->last = generation;
	memcpy(o->saved[generation], o->timestamps + PAGE + 64 + generation * 32, 32);
	uint64_t *ts = o->saved[generation]; CHECK(ts[0] && ts[1] > ts[0] && ts[2] >= ts[1] && ts[3] > ts[2]);
}
int main(void)
{
	setbuf(stdout, NULL);
	struct render_owner owners[2] = {{0}};
	for (unsigned i = 0; i < 2; i++) create_render_owner(&owners[i], i);
	struct render_owner *o = &owners[0];
	/* Source soft-store test starts discarded target zero at zero. Its
	 * authored program would otherwise write nonzero values to both pixels. */
	memset(o->images[0], 0, RENDER_OUTPUT_SIZE);
	puts("G17P_OWNED_SHADER_STORE_FAULT_BEGIN");
	execute(o, 0);
	for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++) CHECK(o->images[0][byte] == 0);
	for (unsigned target = 1; target < 8; target++) {
		float value = RENDER_TRIANGLES * ((target + 1) / 8.0f);
		unsigned char pixel[4]; memcpy(pixel, &value, 4);
		for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++) {
			unsigned char want = 0;
			if (byte >= RENDER_PIXEL_0 && byte < RENDER_PIXEL_0 + 4) want = pixel[byte - RENDER_PIXEL_0];
			if (byte >= RENDER_PIXEL_1 && byte < RENDER_PIXEL_1 + 4) want = pixel[byte - RENDER_PIXEL_1];
			CHECK(o->images[target][byte] == want);
		}
	}
	for (unsigned byte = 0; byte < PAGE; byte++) CHECK(o->sentinel[byte] == 0x61);
	for (unsigned byte = 0; byte < PAGE * 3; byte++)
		if (byte < PAGE + 64 || byte >= PAGE + 96) CHECK(o->timestamps[byte] == 0xa5);
	CHECK(memcmp(o->saved[0], o->timestamps + PAGE + 64, 32) == 0);
	check_render_owner(&owners[1], 1);
	unsigned char *old[8], *snapshots[8];
	for (unsigned target = 0; target < 8; target++) {
		old[target] = o->images[target]; snapshots[target] = malloc(RENDER_OUTPUT_SIZE); CHECK(snapshots[target]);
		memcpy(snapshots[target], old[target], RENDER_OUTPUT_SIZE);
		/* Unbind restores the quiescent owned leaf before removing exactly
		 * this caller span. Old userspace mappings keep the old GEM alive. */
		bind(o->fd, o->vm, 0, render_outputs[target], RENDER_OUTPUT_SIZE, 0, DRM_ASAHI_BIND_UNBIND, 0);
		uint32_t bo = bo_new(o->fd, RENDER_OUTPUT_SIZE, DRM_ASAHI_GEM_WRITEBACK, 0);
		o->images[target] = bo_map(o->fd, bo, RENDER_OUTPUT_SIZE); memset(o->images[target], 0xa5, RENDER_OUTPUT_SIZE);
		bind(o->fd, o->vm, bo, render_outputs[target], RENDER_OUTPUT_SIZE, 0, RW, 0);
	}
	execute(o, 1); check_render_owner(o, 0); check_render_owner(&owners[1], 1);
	execute(&owners[1], 0); check_render_owner(o, 0); check_render_owner(&owners[1], 1);
	for (unsigned target = 0; target < 8; target++) {
		for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++) CHECK(old[target][byte] == snapshots[target][byte]);
		free(snapshots[target]);
	}
	for (unsigned i = 0; i < 2; i++) CHECK(close(owners[i].fd) == 0);
	printf("G17P_OWNED_SHADER_STORE_FAULT_PASS discarded target, seven correct images, successful fences/timestamps, same queue fresh GEM recovery, old backing and second VM preserved; checks=%u\n", checks);
	return 0;
}
