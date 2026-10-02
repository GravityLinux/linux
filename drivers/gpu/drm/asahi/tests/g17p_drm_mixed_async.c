/* SPDX-License-Identifier: MIT */
/* UAPI pipeline measurement: two disjoint render outputs, raw async submits. */
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
    const unsigned char *draws[]={render_command,render_second_command};
    for(unsigned j=0;j<16;j++) memset(images[j],0xa5,RENDER_OUTPUT_SIZE);
    for(unsigned j=0;j<2;j++) {
        union {uint64_t align;unsigned char bytes[sizeof(render_command)];} warm_stream;
        memcpy(warm_stream.bytes,draws[j],sizeof(render_command));
        struct drm_asahi_cmd_render *r=(void *)(warm_stream.bytes+sizeof(render_command)-sizeof(*r));
        struct drm_asahi_cmd_header *h=(void *)((unsigned char *)r-sizeof(*h));
        h->vdm_barrier=h->cdm_barrier=DRM_ASAHI_BARRIER_NONE;
        memset(&r->ts_vtx,0,sizeof(r->ts_vtx));memset(&r->ts_frag,0,sizeof(r->ts_frag));
        uint32_t fence=sync_new(fd,0);struct drm_asahi_sync sync={.handle=fence};
        struct drm_asahi_submit submit={.queue_id=queue,.cmdbuf=(uintptr_t)warm_stream.bytes,
            .cmdbuf_size=sizeof(render_command),.syncs=(uintptr_t)&sync,.out_sync_count=1};
        CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit)==0);wait_output(fd,fence);
        struct drm_syncobj_destroy destroy={.handle=fence};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&destroy);
        printf("G17P_RENDER_AFTER_COMPUTE_WARM_RENDER_PASS stage=%u ownfence\n",j);
    }
    images_check();
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
        CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit)==0);wait_output(fd,fence);
        for(unsigned i=0;i<64;i++) CHECK(((volatile float *)warm_output)[i]==2000.25f+8*129.0f+epoch*8192.0f+i);
        for(unsigned i=256;i<PAGE;i++) CHECK(((volatile unsigned char *)warm_output)[i]==0xa5);
        struct drm_syncobj_destroy destroy={.handle=fence};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&destroy);
    }
    memcpy(warm_input_snapshot,warm_input,PAGE);
    warm_check();images_check();
    printf("G17P_RENDER_AFTER_COMPUTE_WARMUP_PASS samefileVM two complete renders then two64float computes/guards; programs unchanged\n");
}
struct cs_command {
    struct drm_asahi_cmd_header header;
    struct drm_asahi_cmd_compute compute;
};
static struct cs_command compute_command(unsigned index,uint32_t object,unsigned offset,
    uint16_t render_dependency,uint16_t compute_dependency)
{
    const struct batch_workload *w=&batch_workloads[index];
    return (struct cs_command){
        .header={.cmd_type=DRM_ASAHI_CMD_COMPUTE,.size=sizeof(struct drm_asahi_cmd_compute),
            .vdm_barrier=render_dependency,.cdm_barrier=compute_dependency},
        .compute={.cdm_ctrl_stream_base=w->cdm,.cdm_ctrl_stream_end=w->cdm+w->cdm_size,
            .ts.start={.handle=object,.offset=offset},.ts.end={.handle=object,.offset=offset+8}},
    };
}
static void draw_command(unsigned char *dst,unsigned index,uint32_t object,unsigned offset,
    uint16_t render_dependency,uint16_t compute_dependency)
{
    memcpy(dst,index?render_second_command:render_command,sizeof(render_command));
    struct drm_asahi_cmd_render *r=(void *)(dst+sizeof(render_command)-sizeof(*r));
    struct drm_asahi_cmd_header *h=(void *)((unsigned char *)r-sizeof(*h));
    h->vdm_barrier=render_dependency;h->cdm_barrier=compute_dependency;
    r->flags |= DRM_ASAHI_RENDER_VDM_BARRIER_FRAGMENT;
    struct drm_asahi_timestamp *refs[]={&r->ts_vtx.start,&r->ts_vtx.end,&r->ts_frag.start,&r->ts_frag.end};
    for(unsigned i=0;i<4;i++) *refs[i]=(struct drm_asahi_timestamp){.handle=object,.offset=offset+i*8};
}
static uint32_t submit_commands(int fd,uint32_t queue,void *bytes,size_t size,uint32_t input)
{
    uint32_t output=sync_new(fd,0);
    struct drm_asahi_sync syncs[]={{.handle=input},{.handle=output}};
    struct drm_asahi_submit submit={.queue_id=queue,.cmdbuf=(uintptr_t)bytes,.cmdbuf_size=size,
        .syncs=(uintptr_t)(input?syncs:syncs+1),.in_sync_count=!!input,.out_sync_count=1};
    CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit)==0);
    return output;
}
static int fence_status(int fd,uint32_t handle)
{
    struct drm_syncobj_handle exported={.handle=handle,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
    OK(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&exported);
    struct sync_file_info info={0};OK(exported.fd,SYNC_IOC_FILE_INFO,&info);
    CHECK(close(exported.fd)==0);return info.status;
}
static void cs_reset(unsigned n,unsigned epoch)
{
    CHECK(cs_a[n]&&cs_b[n]&&cs_out[n]);
    for(unsigned i=0;i<64;i++) {cs_a[n][i]=1000.0f+epoch*4096+i;cs_b[n][i]=0.25f+n;}
    memset(cs_out[n],0xa5,PAGE);
}
static void cs_check(unsigned n,unsigned epoch)
{
    for(unsigned i=0;i<64;i++) {
        CHECK(((volatile float *)cs_a[n])[i]==1000.0f+epoch*4096+i);
        CHECK(((volatile float *)cs_b[n])[i]==0.25f+n);
        CHECK(((volatile float *)cs_out[n])[i]==1000.25f+epoch*4096+n+i);
    }
    for(unsigned i=256;i<PAGE;i++) CHECK(((volatile unsigned char *)cs_out[n])[i]==0xa5);
}
static void pending(int fd,uint32_t fence) {CHECK(fence_status(fd,fence)==0);}
static void stamp_ready(volatile uint64_t *stamp)
{
    uint64_t deadline=now_ns()+10000000000ULL;
    while(*stamp==0xa5a5a5a5a5a5a5a5ULL || *stamp==0) {
        CHECK(now_ns()<deadline);usleep(1000);
    }
}
static void close_sync(int fd,uint32_t fence) {
    struct drm_syncobj_destroy d={.handle=fence};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&d);
}
int main(int argc, char **argv)
{
    setbuf(stdout, NULL);
    int stress=argc==2 && !strcmp(argv[1],"--stress"); CHECK(argc==1 || stress);
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
    warm_render_compute(fd,vm,queues[0]);

    CHECK(mount("debugfs","/sys/kernel/debug","debugfs",0,NULL)==0 || errno==EBUSY);
    const uint16_t none=DRM_ASAHI_BARRIER_NONE;
    const uint32_t obj=object.object_handle;
    volatile uint64_t *ts=(void *)(timestamps+PAGE);
    union { uint64_t align; unsigned char bytes[sizeof(render_command)*2+sizeof(struct cs_command)]; } stream;
    /* One input-blocked C and one mixed [R,C,R], same public queue. The
     * middle C depends on the blocked previous C. Both R must nevertheless
     * retire, and an independent C on another queue must run before release. */
    for(unsigned i=0;i<3;i++) cs_reset(i,0);
    for(unsigned i=0;i<16;i++) memset(images[i],0xa5,RENDER_OUTPUT_SIZE);
    uint32_t input=sync_new(fd,0);int producer=sync_import_pending(fd,input);
    struct cs_command opening=compute_command(0,obj,64,none,none);
    uint32_t blocked=submit_commands(fd,queues[0],&opening,sizeof(opening),input);
    draw_command(stream.bytes,0,obj,96,none,none);
    struct cs_command middle=compute_command(2,obj,160,none,0);
    memcpy(stream.bytes+sizeof(render_command),&middle,sizeof(middle));
    draw_command(stream.bytes+sizeof(render_command)+sizeof(middle),1,obj,128,1,none);
    uint32_t mixed=submit_commands(fd,queues[0],stream.bytes,sizeof(stream.bytes),0);
    struct cs_command unrelated=compute_command(1,obj,176,none,none);
    uint32_t independent=submit_commands(fd,queues[1],&unrelated,sizeof(unrelated),0);
    wait_output(fd,independent);cs_check(1,0);
    stamp_ready(&ts[(128+24)/8]);images_check();
    pending(fd,blocked);pending(fd,mixed);
    for(unsigned i=0;i<PAGE;i++) CHECK(((volatile unsigned char *)cs_out[0])[i]==0xa5 && ((volatile unsigned char *)cs_out[2])[i]==0xa5);
    CHECK(ts[160/8]==0xa5a5a5a5a5a5a5a5ULL);
    CHECK(ts[128/8]>=ts[(96+8)/8]);
    printf("G17P_MIXED_HOL_PASS two renders and unrelated compute completed while priorcompute and middlecompute remain blocked; both aggregate fences pending\n");
    uint32_t increment=1;OK(producer,SW_INC,&increment);CHECK(close(producer)==0);
    wait_output(fd,blocked);wait_output(fd,mixed);cs_check(0,0);cs_check(2,0);images_check();
    CHECK(ts[160/8]>=ts[(64+8)/8]);
    for(unsigned i=0;i<4;i++) close_sync(fd,((uint32_t[]){input,blocked,mixed,independent})[i]);
    /* Same queue, separate submissions: independent CS must overlap long FR.
     * A second CS genuinely depends on that FR; this must not block an
     * unrelated queue's CS behind a firmware wait on one shared engine. */
    unsigned overlaps=0, count=stress?4096:8;
    for(unsigned run=1;run<=count;run++) {
        unsigned epoch=1+((run-1)%511);
        memset(timestamps+PAGE+256,0xa5,96);
        cs_reset(1,epoch);cs_reset(2,epoch);
        draw_command(stream.bytes,0,obj,256,none,none);
        uint32_t render=submit_commands(fd,queues[0],stream.bytes,sizeof(render_command),0);
        usleep(10000); /* Aim inside live work; timestamps decide overlap. */
        struct cs_command dependent=compute_command(2,obj,304,0,none);
        uint32_t dep=submit_commands(fd,queues[0],&dependent,sizeof(dependent),0);
        struct cs_command free=compute_command(1,obj,288,none,none);
        uint32_t other=submit_commands(fd,queues[1],&free,sizeof(free),0);
        wait_output(fd,other);cs_check(1,epoch);
        wait_output(fd,render);wait_output(fd,dep);cs_check(2,epoch);images_check();
        CHECK(ts[304/8]>=ts[(256+24)/8]);
        int overlap=ts[(288+8)/8]>ts[256/8] && ts[288/8]<ts[(256+24)/8];overlaps+=overlap;
        printf("G17P_MIXED_OVERLAP epoch=%u overlap=%d R=%" PRIu64 "/%" PRIu64
            " TAend=%" PRIu64 " FRstart=%" PRIu64 " independentCS=%" PRIu64 "/%" PRIu64 " dependentCS=%" PRIu64 "/%" PRIu64 "\n",
            run,overlap,ts[256/8],ts[(256+24)/8],ts[(256+8)/8],ts[(256+16)/8],ts[288/8],ts[(288+8)/8],ts[304/8],ts[(304+8)/8]);
        close_sync(fd,render);close_sync(fd,dep);close_sync(fd,other);
    }
    CHECK(overlaps>0);
    /* Failed accepted input affects only its genuine dependents. The mixed
     * job still produces both independent images before the error is injected,
     * and a later samequeue compute can pass a skipped predecessor. */
    for(unsigned i=0;i<3;i++) cs_reset(i,11);
    memset(timestamps+PAGE+512,0xa5,112);
    for(unsigned i=0;i<16;i++) memset(images[i],0xa5,RENDER_OUTPUT_SIZE);
    input=sync_new(fd,0);producer=sync_import_pending(fd,input);
    opening=compute_command(0,obj,512,none,none);
    blocked=submit_commands(fd,queues[0],&opening,sizeof(opening),input);
    draw_command(stream.bytes,0,obj,544,none,none);
    middle=compute_command(2,obj,608,none,0);
    memcpy(stream.bytes+sizeof(render_command),&middle,sizeof(middle));
    draw_command(stream.bytes+sizeof(render_command)+sizeof(middle),1,obj,576,1,none);
    mixed=submit_commands(fd,queues[0],stream.bytes,sizeof(stream.bytes),0);
    stamp_ready(&ts[(576+24)/8]);images_check();pending(fd,mixed);pending(fd,blocked);
    CHECK(close(producer)==0);
    for(unsigned i=0;i<2;i++) {
        uint32_t fence=i?mixed:blocked;
        struct drm_syncobj_wait wait={.handles=(uintptr_t)&fence,.count_handles=1,
            .timeout_nsec=now_ns()+15000000000ULL};
        OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);CHECK(fence_status(fd,fence)==-ENOENT);
    }
    for(unsigned i=0;i<PAGE;i++) CHECK(((volatile unsigned char *)cs_out[0])[i]==0xa5 && ((volatile unsigned char *)cs_out[2])[i]==0xa5);
    middle=compute_command(2,obj,624,none,none);
    independent=submit_commands(fd,queues[0],&middle,sizeof(middle),0);
    wait_output(fd,independent);cs_check(2,11);images_check();
    for(unsigned i=0;i<4;i++) close_sync(fd,((uint32_t[]){input,blocked,mixed,independent})[i]);
    printf("G17P_MIXED_ERROR_PASS only dependent CS failed; both independent images completed before error; later samequeue CS succeeds\n");
    /* Exact in-buffer C->R dependency, followed by R->C. */
    cs_reset(0,9);cs_reset(2,9);
    opening=compute_command(0,obj,384,none,none);
    memcpy(stream.bytes,&opening,sizeof(opening));
    draw_command(stream.bytes+sizeof(opening),1,obj,400,none,1);
    middle=compute_command(2,obj,432,1,none);
    memcpy(stream.bytes+sizeof(opening)+sizeof(render_command),&middle,sizeof(middle));
    uint32_t chain=submit_commands(fd,queues[0],stream.bytes,sizeof(render_command)+2*sizeof(middle),0);
    wait_output(fd,chain);cs_check(0,9);cs_check(2,9);images_check();
    CHECK(ts[400/8]>=ts[(384+8)/8]);CHECK(ts[432/8]>=ts[(400+24)/8]);
    close_sync(fd,chain);
    for(unsigned i=0;i<PAGE;i++) CHECK(timestamps[i]==0xa5 && timestamps[PAGE*2+i]==0xa5);
    printf("G17P_MIXED_ASYNC_PASS blocked priorCS, mixedRC R bypass, independent crossqueueCS, realRCS/CSR dependencies, %u/%u live overlaps, completeimages, CSoutputs/guards, ownedfences; checks=%u\n",overlaps,count,checks);
    /* Rebind compute ASIDs2/3 between two VMs while an independent render
     * keeps ASID1 live. Identical GPU VAs use distinct physical input/output
     * BOs: checking both VMs catches stale-root and overbroad invalidation. */
    float *views[2][3][3];
    for(unsigned j=0;j<3;j++) {views[0][j][0]=cs_a[j];views[0][j][1]=cs_b[j];views[0][j][2]=cs_out[j];}
    uint32_t other_vm=vm_new(fd);
    struct drm_asahi_queue_create newq={.vm_id=other_vm,.usc_exec_base=EXEC};
    OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&newq);
    for(unsigned i=0;i<sizeof(compute_workloads)/sizeof(compute_workloads[0]);i++) {
        const struct compute_workload *w=&compute_workloads[i];
        uint32_t object_bo=bo_new(fd,w->size,DRM_ASAHI_GEM_WRITEBACK,0);
        void *map=bo_map(fd,object_bo,w->size);memcpy(map,w->data,w->size);int keep=0;
        for(unsigned j=0;j<3;j++) {
            if(w->address==batch_workloads[j].input_a) {views[1][j][0]=map;keep=1;}
            if(w->address==batch_workloads[j].input_b) {views[1][j][1]=map;keep=1;}
            if(w->address==batch_workloads[j].output) {views[1][j][2]=map;keep=1;}
        }
        if(!keep) CHECK(munmap(map,w->size)==0);
        bind(fd,other_vm,object_bo,w->address,w->size,0,DRM_ASAHI_BIND_READ|(w->writable?DRM_ASAHI_BIND_WRITE:0),0);
    }
    /* Compute streams share this executable BO with the render fixture.
     * Copy its original bytes unchanged into the independently rooted VM. */
    unsigned executable=0;
    for(unsigned i=0;i<sizeof(workloads)/sizeof(workloads[0]);i++) {
        const struct workload *w=&workloads[i];
        if(w->address!=EXEC) continue;
        uint32_t object_bo=bo_new(fd,w->size,DRM_ASAHI_GEM_WRITEBACK,0);
        void *map=bo_map(fd,object_bo,w->size);memcpy(map,w->data,w->size);
        CHECK(munmap(map,w->size)==0);
        bind(fd,other_vm,object_bo,w->address,w->size,0,DRM_ASAHI_BIND_READ,0);
        executable++;
    }
    CHECK(executable==1);
    unsigned cross_overlap=0;unsigned char other_output[PAGE];
    for(unsigned run=0;run<16;run++) {
        unsigned view=(run+1)%2;
        for(unsigned j=0;j<3;j++) {cs_a[j]=views[view][j][0];cs_b[j]=views[view][j][1];cs_out[j]=views[view][j][2];}
        memcpy(other_output,views[1-view][1][2],PAGE);cs_reset(1,64+run);
        memset(timestamps+PAGE+704,0xa5,48);
        draw_command(stream.bytes,0,obj,704,none,none);
        uint32_t r=submit_commands(fd,queues[0],stream.bytes,sizeof(render_command),0);
        usleep(10000);
        struct cs_command cs=compute_command(1,obj,736,none,none);
        uint32_t c=submit_commands(fd,view?newq.queue_id:queues[1],&cs,sizeof(cs),0);
        wait_output(fd,c);cs_check(1,64+run);wait_output(fd,r);images_check();
        CHECK(memcmp(other_output,views[1-view][1][2],PAGE)==0);
        int overlap=ts[(736+8)/8]>ts[704/8] && ts[736/8]<ts[(704+24)/8];cross_overlap+=overlap;
        printf("G17P_MIXED_CROSS_VM run=%u computeVM=%u overlap=%d R=%" PRIu64 "/%" PRIu64
            "/%" PRIu64 "/%" PRIu64 " CS=%" PRIu64 "/%" PRIu64 "\n",
            run,view,overlap,ts[704/8],ts[(704+8)/8],ts[(704+16)/8],ts[(704+24)/8],ts[736/8],ts[(736+8)/8]);
        close_sync(fd,r);close_sync(fd,c);
    }
    CHECK(cross_overlap>0);
    printf("G17P_MIXED_CROSS_VM_PASS 16 compute-root handoffs with live independent render, %u overlaps, bothVM sameVA distinctBO outputs and guards verified\n",cross_overlap);
    /* Accept thousands of jobs, then fail newest-to-oldest. Their exact
     * output fences must fail without publishing any command; unrelated work
     * must finish while the input fences remain blocked. This exercises the
     * accepted-history final-owner drop on recovery, queue and file teardown. */
    struct rlimit limits;CHECK(getrlimit(RLIMIT_NOFILE,&limits)==0);
    limits.rlim_cur=limits.rlim_max;CHECK(setrlimit(RLIMIT_NOFILE,&limits)==0);
    CHECK(limits.rlim_cur>4200);
    uint32_t failure_outputs[4096];int failure_producers[4096];
    for(unsigned mode=0;mode<3;mode++) {
        unsigned failure_count=mode==0?4096:512;
        struct drm_asahi_queue_create fq={.vm_id=vm,.usc_exec_base=EXEC};
        OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&fq);
        cs_reset(0,96);cs_reset(1,97);
        memset(timestamps+PAGE+800,0xa5,32);
        struct cs_command failed_command=compute_command(0,obj,800,none,none);
        for(unsigned n=0;n<failure_count;n++) {
            uint32_t failure_input=sync_new(fd,0);
            failure_producers[n]=sync_import_pending(fd,failure_input);
            failure_outputs[n]=submit_commands(fd,fq.queue_id,&failed_command,sizeof(failed_command),failure_input);
            close_sync(fd,failure_input);
        }
        struct cs_command live_command=compute_command(1,obj,816,none,none);
        uint32_t live_output=submit_commands(fd,queues[1],&live_command,sizeof(live_command),0);
        wait_output(fd,live_output);cs_check(1,97);close_sync(fd,live_output);
        for(unsigned n=failure_count;n--;) {
            pending(fd,failure_outputs[n]);CHECK(close(failure_producers[n])==0);
            struct drm_syncobj_wait wait={.handles=(uintptr_t)&failure_outputs[n],.count_handles=1,
                .timeout_nsec=now_ns()+15000000000ULL};
            OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);CHECK(fence_status(fd,failure_outputs[n])==-ENOENT);
            close_sync(fd,failure_outputs[n]);
        }
        for(unsigned i=0;i<PAGE;i++) CHECK(((volatile unsigned char *)cs_out[0])[i]==0xa5);
        CHECK(ts[800/8]==0xa5a5a5a5a5a5a5a5ULL && ts[808/8]==0xa5a5a5a5a5a5a5a5ULL);
        if(mode==0) {
            uint32_t recovered=submit_commands(fd,fq.queue_id,&failed_command,sizeof(failed_command),0);
            wait_output(fd,recovered);cs_check(0,96);close_sync(fd,recovered);
        }
        if(mode!=2) {
            struct drm_asahi_queue_destroy qd={.queue_id=fq.queue_id};
            OK(fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&qd);
        }
        printf("G17P_MIXED_FAILURE_CHAIN_PASS mode=%u count=%u reverse failed fences, no rejected writes, unrelated CS, teardown/recovery\n",mode,failure_count);
    }
    CHECK(close(fd)==0);puts("G17P_MIXED_FILE_TEARDOWN_PASS");return 0;
}
