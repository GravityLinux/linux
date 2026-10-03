/* SPDX-License-Identifier: MIT */
/* Fresh render-first UAPI session: first compute while a render fence is pending. */
#include <sys/resource.h>
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include "g17p_drm_render_batch_workload.h"
#define workload compute_workload
#define workloads compute_workloads
#include "g17p_drm_retained_wave_workload.h"
#undef workloads
#undef workload
#include "g17p_drm_sync.h"
#ifndef DRM_ASAHI_RENDER_VDM_BARRIER_FRAGMENT
#define DRM_ASAHI_RENDER_VDM_BARRIER_FRAGMENT (1U << 3)
#endif

static unsigned char *images[16];
static void images_check(void)
{
    for (unsigned target = 0; target < 16; target++) {
        float value = (target < 8 ? RENDER_TRIANGLES : RENDER_SECOND_TRIANGLES)
            * ((target % 8 + 1) / 8.0f);
        unsigned char expected[4]; memcpy(expected, &value, 4);
        for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++) {
            unsigned char want = 0;
            if (byte >= RENDER_PIXEL_0 && byte < RENDER_PIXEL_0 + 4)
                want = expected[byte - RENDER_PIXEL_0];
            if (byte >= RENDER_PIXEL_1 && byte < RENDER_PIXEL_1 + 4)
                want = expected[byte - RENDER_PIXEL_1];
            CHECK(images[target][byte] == want);
        }
    }
}
static void wait_output(int fd, uint32_t handle)
{
    struct drm_syncobj_wait wait = {.handles=(uintptr_t)&handle,
        .count_handles=1, .timeout_nsec=now_ns()+15000000000ULL};
    OK(fd, DRM_IOCTL_SYNCOBJ_WAIT, &wait);
    struct drm_syncobj_handle exported = {.handle=handle,
        .flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
    OK(fd, DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &exported);
    struct sync_file_info info = {0}; OK(exported.fd, SYNC_IOC_FILE_INFO, &info);
    CHECK(info.status == 1); CHECK(close(exported.fd) == 0);
}
static int fence_status(int fd,uint32_t handle);
static uint32_t first_object;
static unsigned char *first_timestamps;
static float *warm_output, *warm_input;
static float *cs_a[3], *cs_b[3], *cs_out[3];
static unsigned char warm_input_snapshot[PAGE];
static void warm_check(void)
{
    for(unsigned i=0;i<PAGE;i++) CHECK(((volatile unsigned char *)warm_input)[i]==warm_input_snapshot[i]);
    for(unsigned i=0;i<64;i++) CHECK(((volatile float *)warm_output)[i]==2000.25f+8*129.0f+8192.0f+i);
    for(unsigned i=256;i<PAGE;i++) CHECK(((volatile unsigned char *)warm_output)[i]==0xa5);
}
static void warm_render_compute(int fd,uint32_t vm,uint32_t queue)
{
    const struct batch_workload *w=&batch_workloads[0];
    for(unsigned i=0;i<sizeof(compute_workloads)/sizeof(compute_workloads[0]);i++) {
        const struct compute_workload *b=&compute_workloads[i];
        uint32_t bo=bo_new(fd,b->size,DRM_ASAHI_GEM_WRITEBACK,0);
        void *map=bo_map(fd,bo,b->size);memcpy(map,b->data,b->size);int keep=0;
        if(b->address==w->output) {warm_output=map;keep=1;}
        if(b->address==w->input_a) {warm_input=map;keep=1;}
        for(unsigned j=0;j<3;j++) {
            if(b->address==batch_workloads[j].input_a) {cs_a[j]=map;keep=1;}
            if(b->address==batch_workloads[j].input_b) {cs_b[j]=map;keep=1;}
            if(b->address==batch_workloads[j].output) {cs_out[j]=map;keep=1;}
        }
        if(!keep) CHECK(munmap(map,b->size)==0);
        bind(fd,vm,bo,b->address,b->size,0,DRM_ASAHI_BIND_READ|(b->writable?DRM_ASAHI_BIND_WRITE:0),0);
    }
    CHECK(warm_input&&warm_output);
    const unsigned char *draws[]={render_second_command,render_command};
    for(unsigned j=0;j<16;j++) memset(images[j],0xa5,RENDER_OUTPUT_SIZE);
    uint32_t late_render=0;
    for(unsigned j=0;j<2;j++) {
        union {uint64_t align;unsigned char bytes[sizeof(render_command)];} warm_stream;
        memcpy(warm_stream.bytes,draws[j],sizeof(render_command));
        struct drm_asahi_cmd_render *r=(void *)(warm_stream.bytes+sizeof(render_command)-sizeof(*r));
        struct drm_asahi_cmd_header *h=(void *)((unsigned char *)r-sizeof(*h));
        h->vdm_barrier=h->cdm_barrier=DRM_ASAHI_BARRIER_NONE;
        memset(&r->ts_vtx,0,sizeof(r->ts_vtx));memset(&r->ts_frag,0,sizeof(r->ts_frag));
        if(j==1) {
            r->ts_vtx.start=(struct drm_asahi_timestamp){.handle=first_object,.offset=32};
            r->ts_vtx.end=(struct drm_asahi_timestamp){.handle=first_object,.offset=40};
            r->ts_frag.start=(struct drm_asahi_timestamp){.handle=first_object,.offset=48};
            r->ts_frag.end=(struct drm_asahi_timestamp){.handle=first_object,.offset=56};
        }
        uint32_t fence=sync_new(fd,0);struct drm_asahi_sync sync={.handle=fence};
        struct drm_asahi_submit submit={.queue_id=queue,.cmdbuf=(uintptr_t)warm_stream.bytes,
            .cmdbuf_size=sizeof(render_command),.syncs=(uintptr_t)&sync,.out_sync_count=1};
        CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit)==0);
        if(j==1) {late_render=fence;continue;}
        wait_output(fd,fence);
        struct drm_syncobj_destroy destroy={.handle=fence};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&destroy);
        printf("G17P_RENDER_AFTER_COMPUTE_WARM_RENDER_PASS stage=%u ownfence\n",j);
    }
    CHECK(late_render && fence_status(fd,late_render)==0);
    struct {
        struct drm_asahi_cmd_header attachment_header;
        struct drm_asahi_attachment attachment;
        struct drm_asahi_cmd_header header;
        struct drm_asahi_cmd_compute compute;
    } commands={
        .attachment_header={.cmd_type=DRM_ASAHI_SET_COMPUTE_ATTACHMENTS,.size=sizeof(struct drm_asahi_attachment),
            .vdm_barrier=DRM_ASAHI_BARRIER_NONE,.cdm_barrier=DRM_ASAHI_BARRIER_NONE},
        .attachment={.pointer=w->output,.size=PAGE},
        .header={.cmd_type=DRM_ASAHI_CMD_COMPUTE,.size=sizeof(struct drm_asahi_cmd_compute),
            .vdm_barrier=DRM_ASAHI_BARRIER_NONE,.cdm_barrier=DRM_ASAHI_BARRIER_NONE},
        .compute={.cdm_ctrl_stream_base=w->cdm,.cdm_ctrl_stream_end=w->cdm+w->cdm_size},
    };
    for(unsigned epoch=0;epoch<2;epoch++) {
        for(unsigned i=0;i<64;i++) warm_input[i]=2000.0f+8*128.0f+epoch*8192.0f+i;
        memset(warm_output,0xa5,PAGE);
        uint32_t fence=sync_new(fd,0);struct drm_asahi_sync sync={.handle=fence};
        struct drm_asahi_submit submit={.queue_id=queue,.cmdbuf=(uintptr_t)&commands,
            .cmdbuf_size=sizeof(commands),.syncs=(uintptr_t)&sync,.out_sync_count=1};
        if(epoch==0) {
            commands.compute.ts.start=(struct drm_asahi_timestamp){.handle=first_object,.offset=0};
            commands.compute.ts.end=(struct drm_asahi_timestamp){.handle=first_object,.offset=8};
            CHECK(fence_status(fd,late_render)==0);
        }
        CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit)==0);wait_output(fd,fence);
        if(epoch==0) {
            wait_output(fd,late_render);images_check();
            volatile uint64_t *t=(void *)(first_timestamps+PAGE);
            printf("FIRST_COMPUTE_LIVE_RENDER_TIMESTAMPS R=%llu/%llu CS=%llu/%llu overlap=%d\n",
                (unsigned long long)t[4],(unsigned long long)t[7],(unsigned long long)t[0],(unsigned long long)t[1],t[0]<t[7]&&t[1]>t[4]);
            CHECK(t[0]&&t[1]>t[0]&&t[7]>t[4]);
            CHECK(t[0]<t[7] && t[1]>t[4]);
            struct drm_syncobj_destroy d={.handle=late_render};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&d);
            memset(&commands.compute.ts,0,sizeof(commands.compute.ts));
        }
        for(unsigned i=0;i<64;i++) CHECK(((volatile float *)warm_output)[i]==2000.25f+8*129.0f+epoch*8192.0f+i);
        for(unsigned i=256;i<PAGE;i++) CHECK(((volatile unsigned char *)warm_output)[i]==0xa5);
        struct drm_syncobj_destroy destroy={.handle=fence};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&destroy);
    }
    memcpy(warm_input_snapshot,warm_input,PAGE);
    warm_check();images_check();
    printf("G17P_RENDER_AFTER_COMPUTE_WARMUP_PASS samefileVM first compute submitted during pending render; complete images and two64float computes/guards; programs unchanged\n");
}
static int fence_status(int fd,uint32_t handle)
{
    struct drm_syncobj_handle exported={.handle=handle,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
    OK(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&exported);
    struct sync_file_info info={0};OK(exported.fd,SYNC_IOC_FILE_INFO,&info);
    CHECK(close(exported.fd)==0);return info.status;
}
int main(void)
{
    setbuf(stdout, NULL);
    int fd = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC); CHECK(fd >= 0);
    uint32_t vm = vm_new(fd), queues[2];
    for (unsigned i=0; i<2; i++) {
        struct drm_asahi_queue_create q = {.vm_id=vm,.usc_exec_base=EXEC};
        OK(fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q); queues[i]=q.queue_id;
    }
    for (unsigned i=0; i<sizeof(workloads)/sizeof(workloads[0]); i++) {
        const struct workload *w=&workloads[i];
        uint32_t bo=bo_new(fd,w->size,DRM_ASAHI_GEM_WRITEBACK,0);
        void *map=bo_map(fd,bo,w->size); memcpy(map,w->data,w->size);
        int keep=0;
        for (unsigned j=0; j<16; j++) if(w->address==render_outputs[j]) {
            images[j]=map; keep=1;
        }
        if(!keep) CHECK(munmap(map,w->size)==0);
        bind(fd,vm,bo,w->address,w->size,0,
            DRM_ASAHI_BIND_READ|(w->writable?DRM_ASAHI_BIND_WRITE:0),0);
    }
    for(unsigned j=0;j<16;j++) CHECK(images[j]);
    uint32_t bo=bo_new(fd,PAGE*3,DRM_ASAHI_GEM_WRITEBACK,0);
    unsigned char *timestamps=bo_map(fd,bo,PAGE*3);
    memset(timestamps,0xa5,PAGE*3);
    struct drm_asahi_gem_bind_object object={.op=DRM_ASAHI_BIND_OBJECT_OP_BIND,
        .flags=DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,
        .handle=bo,.offset=PAGE,.range=PAGE};
    OK(fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&object);
    first_object=object.object_handle;first_timestamps=timestamps;
    warm_render_compute(fd,vm,queues[0]);
    puts("FIRST_COMPUTE_LIVE_RENDER_PASS owned fences, complete images/compute/guards");
    for(unsigned i=0;i<PAGE*3;i++)
        if(i<PAGE || (i>=PAGE+16 && i<PAGE+32) || i>=PAGE+64)
            CHECK(timestamps[i]==0xa5);
    CHECK(close(fd)==0);return 0;
}
