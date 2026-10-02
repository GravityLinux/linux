/* SPDX-License-Identifier: MIT */
/* Run with the source owned command-fetch diagnostic armed at ordinal zero. */
#define G17P_RENDER_VM_HELPERS
#include "g17p_drm_render_vm.c"
#include "g17p_drm_sync.h"

int main(void)
{
	setbuf(stdout, NULL);
	struct render_owner owners[2] = {{0}};
	for (unsigned i = 0; i < 2; i++) create_render_owner(&owners[i], i);
	struct render_owner *o = &owners[0];
	struct drm_asahi_cmd_render *command = (void *)(o->command + sizeof(o->command) - sizeof(*command));
	struct drm_asahi_timestamp *refs[] = { &command->ts_vtx.start, &command->ts_vtx.end,
		&command->ts_frag.start, &command->ts_frag.end };
	for (unsigned i = 0; i < 4; i++)
		*refs[i] = (struct drm_asahi_timestamp){ .handle = o->object, .offset = 64 + i * 8 };
	struct drm_asahi_sync syncs[] = {
		{ .sync_type = DRM_ASAHI_SYNC_SYNCOBJ, .handle = o->binary },
		{ .sync_type = DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ, .handle = o->timeline, .timeline_value = 1 },
	};
	struct drm_asahi_submit submit = { .queue_id = o->queue, .cmdbuf = (uintptr_t)o->command,
		.cmdbuf_size = sizeof(o->command), .syncs = (uintptr_t)syncs, .out_sync_count = 2 };
    CHECK(mount("debugfs","/sys/kernel/debug","debugfs",0,NULL)==0 || errno==EBUSY);
    uint32_t input=sync_new(owners[1].fd,0);
    int producer=sync_import_pending(owners[1].fd,input);
    struct drm_asahi_sync blocked_syncs[]={
        {.handle=input},{.handle=owners[1].binary},
        {.sync_type=DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ,.handle=owners[1].timeline,.timeline_value=1},
    };
    struct drm_asahi_submit blocked={.queue_id=owners[1].queue,
        .cmdbuf=(uintptr_t)owners[1].command,.cmdbuf_size=sizeof(owners[1].command),
        .syncs=(uintptr_t)blocked_syncs,.in_sync_count=1,.out_sync_count=2};
    CHECK(g17p_raw_ioctl(owners[1].fd,DRM_IOCTL_ASAHI_SUBMIT,&blocked)==0);
	puts("G17P_OWNED_COMMAND_FETCH_FAULT_BEGIN");
	OK(o->fd, DRM_IOCTL_ASAHI_SUBMIT, &submit);
	struct drm_syncobj_handle exported = { .handle = o->binary,
		.flags = DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE };
	OK(o->fd, DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &exported);
	struct sync_file_info info = {0};
	CHECK(ioctl(exported.fd, SYNC_IOC_FILE_INFO, &info) == 0);
	CHECK(info.status < 0);
	printf("G17P_FAULT_FENCE status=%d\n", info.status);
    struct drm_syncobj_wait failed_wait={.handles=(uintptr_t)&owners[1].binary,
        .count_handles=1,.timeout_nsec=now_ns()+15000000000ULL};
    OK(owners[1].fd,DRM_IOCTL_SYNCOBJ_WAIT,&failed_wait);
    struct drm_syncobj_handle failed_export={.handle=owners[1].binary,
        .flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
    OK(owners[1].fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&failed_export);
    struct sync_file_info failed_info={0};OK(failed_export.fd,SYNC_IOC_FILE_INFO,&failed_info);
    CHECK(failed_info.status==-EIO);CHECK(close(failed_export.fd)==0);
    CHECK(close(producer)==0);
    puts("G17P_ASYNC_FATAL_FANOUT_PASS pending imported input failed without its producer signaling; inactive work stayed untouched");
	check_render_owner(&owners[1], 1);
	unsigned char before[PAGE * 3]; memcpy(before, o->timestamps, sizeof(before));
    BAD(o->fd, DRM_IOCTL_ASAHI_SUBMIT, &submit, EIO);
	CHECK(memcmp(before, o->timestamps, sizeof(before)) == 0);
	/* The query returned num_fences=1. Reset the count for another summary
	 * query rather than requesting details through a null userspace pointer. */
	info = (struct sync_file_info){0};
	CHECK(ioctl(exported.fd, SYNC_IOC_FILE_INFO, &info) == 0 && info.status < 0);
	check_render_owner(&owners[1], 1);
	CHECK(close(exported.fd) == 0);
	for (unsigned i = 0; i < 2; i++) CHECK(close(owners[i].fd) == 0);
	puts("G17P_OWNED_COMMAND_FETCH_FAULT_PASS errored aggregate fence, later EIO admission, inactive images/guards/timestamps preserved");
	return 0;
}
