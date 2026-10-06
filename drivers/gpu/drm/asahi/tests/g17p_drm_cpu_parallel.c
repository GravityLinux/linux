/* SPDX-License-Identifier: MIT */
/* Independent UAPI work completes during an off-lock CPU preparation/cache pause. */
#define G17P_UNBIND_LIBRARY
#include "g17p_drm_unbind_pending.c"
#undef G17P_UNBIND_LIBRARY
static int cpu_render;
struct cpu_client {
    uint32_t vm, queue, fence, stamp;
    float *input[3][2], *output[3];
    struct allocation ts;
    unsigned char *images[16];
    uint32_t image_bos[16];
};
static void cpu_create(int fd, struct cpu_client *c, unsigned index)
{
    struct drm_asahi_vm_create v = {.kernel_start=KSTART + index * 0x40000000ULL,
        .kernel_end=KSTART + index * 0x40000000ULL + 0x20000000};
    OK(fd, DRM_IOCTL_ASAHI_VM_CREATE, &v); c->vm=v.vm_id;
    c->queue=queue_new(fd,c->vm);
    for (unsigned i=0;i<sizeof(render_workloads)/sizeof(render_workloads[0]);i++) {
        const struct render_workload *w=&render_workloads[i]; if(!cpu_render && w->address!=EXEC) continue;
        uint32_t bo=bo_new(fd,w->size,DRM_ASAHI_GEM_WRITEBACK,0);
        void *map=bo_map(fd,bo,w->size);memcpy(map,w->data,w->size);int keep=0;
        if(cpu_render) for(unsigned j=0;j<16;j++) if(w->address==render_outputs[j]) {
            c->images[j]=map;c->image_bos[j]=bo;memset(map,0xa5,RENDER_OUTPUT_SIZE);keep=1;
        }
        if(!keep)CHECK(munmap(map,w->size)==0);
        bind(fd,c->vm,bo,w->address,w->size,0,DRM_ASAHI_BIND_READ|(w->writable?DRM_ASAHI_BIND_WRITE:0),0);
    }
    for(unsigned i=0;i<sizeof(workloads)/sizeof(workloads[0]);i++) {
        const struct workload *w=&workloads[i];uint32_t bo=bo_new(fd,w->size,DRM_ASAHI_GEM_WRITEBACK,0);
        void *map=bo_map(fd,bo,w->size);memcpy(map,w->data,w->size);int keep=0;
        for(unsigned j=0;j<3;j++) {
            if(w->address==batch_workloads[j].input_a) {c->input[j][0]=map;keep=1;}
            if(w->address==batch_workloads[j].input_b) {c->input[j][1]=map;keep=1;}
            if(w->address==batch_workloads[j].output) {c->output[j]=map;keep=1;}
        }
        if(!keep)CHECK(munmap(map,w->size)==0);
        bind(fd,c->vm,bo,w->address,w->size,0,DRM_ASAHI_BIND_READ|(w->writable?DRM_ASAHI_BIND_WRITE:0),0);
    }
    for(unsigned j=0;j<3;j++) {
        CHECK(c->input[j][0]&&c->input[j][1]&&c->output[j]);
        for(unsigned i=0;i<64;i++) {c->input[j][0][i]=1000.0f+index*10000.0f+j*512.0f+i;c->input[j][1][i]=0.25f+j;}
        memset(c->output[j],0xa5,PAGE);
    }
    c->ts=allocation_new(fd,1);c->fence=sync_new(fd,0);
}
static void cpu_check(struct cpu_client *c,unsigned index,unsigned graph)
{
    for(unsigned i=0;i<64;i++){float want=1000.25f+index*10000.0f+graph*513.0f+i;if(c->output[graph][i]!=want)printf("CPU_OUTPUT_MISMATCH owner=%u graph=%u index=%u actual=%a expected=%a\n",index,graph,i,c->output[graph][i],want);CHECK(c->output[graph][i]==want);}
    for(unsigned i=256;i<PAGE;i++)CHECK(((unsigned char *)c->output[graph])[i]==0xa5);
    timestamp_check(&c->ts,1);
}
static void cpu_render_submit(int fd,struct cpu_client *c)
{
    union {uint64_t alignment;unsigned char bytes[sizeof(render_command)];} draw;
    memcpy(draw.bytes,render_command,sizeof(draw.bytes));
    struct drm_asahi_cmd_render *r=(void *)(draw.bytes+sizeof(draw.bytes)-sizeof(*r));
    struct drm_asahi_cmd_header *h=(void *)((unsigned char *)r-sizeof(*h));h->vdm_barrier=h->cdm_barrier=DRM_ASAHI_BARRIER_NONE;
    struct drm_asahi_timestamp *t[]={&r->ts_vtx.start,&r->ts_vtx.end,&r->ts_frag.start,&r->ts_frag.end};
    for(unsigned j=0;j<4;j++) *t[j]=(struct drm_asahi_timestamp){.handle=c->ts.object,.offset=64+j*8};
    struct drm_asahi_sync out={.handle=c->fence};
    struct drm_asahi_submit s={.queue_id=c->queue,.cmdbuf=(uintptr_t)draw.bytes,.cmdbuf_size=sizeof(draw.bytes),.syncs=(uintptr_t)&out,.out_sync_count=1};
    CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&s)==0);memset(draw.bytes,0xa5,sizeof(draw.bytes));
}
static void cpu_render_check(struct cpu_client *c)
{
    for(unsigned target=0;target<16;target++) {
        float value=RENDER_TRIANGLES*((target%8+1)/8.0f);unsigned char bytes[4];memcpy(bytes,&value,4);
        for(unsigned byte=0;byte<RENDER_OUTPUT_SIZE;byte++) {
            unsigned char want=target<8?0:0xa5;
            if(target<8&&byte>=RENDER_PIXEL_0&&byte<RENDER_PIXEL_0+4)want=bytes[byte-RENDER_PIXEL_0];
            if(target<8&&byte>=RENDER_PIXEL_1&&byte<RENDER_PIXEL_1+4)want=bytes[byte-RENDER_PIXEL_1];
            CHECK(c->images[target][byte]==want);
        }
    }
    volatile uint64_t *ts=(void *)(c->ts.map+PAGE+64);
    CHECK(ts[0]&&ts[0]!=0xa5a5a5a5a5a5a5a5ULL&&ts[1]>ts[0]&&ts[2]>=ts[1]&&ts[3]>ts[2]);
    for(unsigned i=0;i<PAGE*3;i++)if(i<PAGE+64||i>=PAGE+96)CHECK(c->ts.map[i]==0xa5);
}
#ifndef G17P_CPU_LIBRARY
int main(int argc,char **argv)
{
    setbuf(stdout,NULL);CHECK(argc==2);
    cpu_render=!strncmp(argv[1],"render-",7);
    int prepare=!strcmp(argv[1],"prepare")||!strcmp(argv[1],"render-prepare");CHECK(prepare||!strcmp(argv[1],"cache")||!strcmp(argv[1],"render-cache"));
    int fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC);CHECK(fd>=0);
    struct cpu_client warm={0},slow={0},fast={0};
    cpu_create(fd,&warm,0);
    if(cpu_render && prepare) {
        slow=warm;slow.queue=queue_new(fd,warm.vm);slow.ts=allocation_new(fd,1);slow.fence=sync_new(fd,0);
    } else cpu_create(fd,&slow,1);
    cpu_create(fd,&fast,2);
    CHECK(warm.queue==1&&slow.queue==2&&fast.queue==3);
    uint32_t timeline=sync_new(fd,0);
    if(prepare) {
        if(cpu_render)cpu_render_submit(fd,&warm);else submit(fd,warm.queue,0,warm.ts.object,0,warm.fence,timeline,1);
        wait_success(fd,warm.fence);if(cpu_render)cpu_render_check(&warm);else cpu_check(&warm,0,0);
        if(cpu_render) for(unsigned j=0;j<8;j++) {
            bind(fd,slow.vm,0,render_outputs[j],RENDER_OUTPUT_SIZE,0,DRM_ASAHI_BIND_UNBIND,0);
            slow.image_bos[j]=bo_new(fd,RENDER_OUTPUT_SIZE,DRM_ASAHI_GEM_WRITEBACK,0);
            slow.images[j]=bo_map(fd,slow.image_bos[j],RENDER_OUTPUT_SIZE);memset(slow.images[j],0xa5,RENDER_OUTPUT_SIZE);
            bind(fd,slow.vm,slow.image_bos[j],render_outputs[j],RENDER_OUTPUT_SIZE,0,RW,0);
        }
    } else {
        /* The first independent compute uses grid 32; its CPU retirement
         * is paused by the explicit boot-only diagnostic, after GPU completion. */
        slow=warm;
    }
    uint64_t began=now_ns();
    if(cpu_render)cpu_render_submit(fd,&slow);else submit(fd,slow.queue,1,slow.ts.object,0,slow.fence,timeline,2);
    usleep(200000);
    CHECK(status(fd,slow.fence)==0);
    submit(fd,fast.queue,2,fast.ts.object,0,fast.fence,timeline,3);
    wait_success(fd,fast.fence);
    uint64_t elapsed=now_ns()-began;
    CHECK(elapsed<1500000000ULL);CHECK(status(fd,slow.fence)==0);
    cpu_check(&fast,2,2);
    printf("CPU_PARALLEL_INDEPENDENT_PASS phase=%s latency_ns=%" PRIu64 " paused_fence_pending=1 full_output_ts_guards=1\n",argv[1],elapsed);
    wait_success(fd,slow.fence);if(cpu_render)cpu_render_check(&slow);else cpu_check(&slow,prepare?1:0,1);
    CHECK(close(fd)==0);
    printf("G17P_CPU_PARALLEL_PASS phase=%s checks=%u\n",argv[1],checks);
    return 0;
}

#endif
