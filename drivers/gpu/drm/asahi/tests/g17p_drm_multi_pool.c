/* SPDX-License-Identifier: MIT */
/* Independently addressed caller outputs, raw UAPI submissions and own fences. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include "g17p_drm_sync.h"
#include "g17p_drm_multi_pool_workload.h"

#define JOBS (sizeof(pool_jobs)/sizeof(pool_jobs[0]))
static unsigned char *images[JOBS][8];
static void check_images(unsigned job)
{
    for(unsigned target=0;target<8;target++) {
        float expected=pool_jobs[job].triangles*((target+1)/8.0f);
        unsigned char bytes[4]; memcpy(bytes,&expected,4);
        for(unsigned at=0;at<RENDER_OUTPUT_SIZE;at++) {
            unsigned char value=0;
            if(at>=RENDER_PIXEL_0 && at<RENDER_PIXEL_0+4) value=bytes[at-RENDER_PIXEL_0];
            if(at>=RENDER_PIXEL_1 && at<RENDER_PIXEL_1+4) value=bytes[at-RENDER_PIXEL_1];
            if(images[job][target][at]!=value) {
                fprintf(stderr,"POOL_IMAGE_FAIL job=%u target=%u byte=%u expected=%u actual=%u\n",
                    job,target,at,value,images[job][target][at]); exit(1);
            }
        }
    }
}
static void wait_fence(int fd,uint32_t fence)
{
    struct drm_syncobj_wait w={.handles=(uintptr_t)&fence,.count_handles=1,
        .timeout_nsec=now_ns()+30000000000ULL}; OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&w);
    struct drm_syncobj_handle h={.handle=fence,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
    OK(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&h);
    struct sync_file_info info={0}; OK(h.fd,SYNC_IOC_FILE_INFO,&info);
    CHECK(info.status==1); CHECK(close(h.fd)==0);
}
static uint32_t submit(int fd,uint32_t queue,unsigned job,uint32_t timestamp)
{
    union { uint64_t align; unsigned char bytes[2048]; } stream;
    const struct pool_job *p=&pool_jobs[job]; CHECK(p->size<=sizeof(stream));
    memcpy(stream.bytes,p->command,p->size);
    struct drm_asahi_cmd_render *r=(void *)(stream.bytes+p->size-sizeof(*r));
    struct drm_asahi_cmd_header *h=(void *)((unsigned char *)r-sizeof(*h));
    h->vdm_barrier=h->cdm_barrier=DRM_ASAHI_BARRIER_NONE;
    memset(&r->ts_vtx,0,sizeof(r->ts_vtx));memset(&r->ts_frag,0,sizeof(r->ts_frag));
    if(timestamp) {
        struct drm_asahi_timestamp *refs[]={&r->ts_vtx.start,&r->ts_vtx.end,&r->ts_frag.start,&r->ts_frag.end};
        for(unsigned k=0;k<4;k++) *refs[k]=(struct drm_asahi_timestamp){.handle=timestamp,.offset=job*64+k*8};
    }
    uint32_t fence=sync_new(fd,0);struct drm_asahi_sync sync={.handle=fence};
    struct drm_asahi_submit s={.queue_id=queue,.cmdbuf=(uintptr_t)stream.bytes,
        .cmdbuf_size=p->size,.syncs=(uintptr_t)&sync,.out_sync_count=1};
    CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&s)==0);return fence;
}
int main(int argc,char **argv)
{
    setbuf(stdout,NULL);
    unsigned rounds=1;int require_overlap=0;
    if(argc>1) rounds=strtoul(argv[1],NULL,0);
    if(argc>2) require_overlap=!strcmp(argv[2],"--require-overlap");
    CHECK(argc<=3 && rounds>0 && rounds<=4096);
    int fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC);CHECK(fd>=0);
    uint32_t vm=vm_new(fd),queues[JOBS];
    for(unsigned j=0;j<JOBS;j++) {
        struct drm_asahi_queue_create q={.vm_id=vm,.usc_exec_base=EXEC};
        OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q);queues[j]=q.queue_id;
    }
    for(unsigned i=0;i<sizeof(workloads)/sizeof(workloads[0]);i++) {
        const struct workload *w=&workloads[i];uint32_t bo=bo_new(fd,w->size,DRM_ASAHI_GEM_WRITEBACK,0);
        void *map=bo_map(fd,bo,w->size);memcpy(map,w->data,w->size);int keep=0;
        for(unsigned j=0;j<JOBS;j++) for(unsigned k=0;k<8;k++) if(w->address==pool_jobs[j].outputs[k]) {
            CHECK(!images[j][k]);images[j][k]=map;keep=1;
        }
        if(!keep) CHECK(munmap(map,w->size)==0);
        bind(fd,vm,bo,w->address,w->size,0,DRM_ASAHI_BIND_READ|(w->writable?DRM_ASAHI_BIND_WRITE:0),0);
    }
    for(unsigned j=0;j<JOBS;j++) for(unsigned k=0;k<8;k++) CHECK(images[j][k]);
    uint32_t bo=bo_new(fd,PAGE*3,DRM_ASAHI_GEM_WRITEBACK,0);
    unsigned char *timestamps=bo_map(fd,bo,PAGE*3);memset(timestamps,0xa5,PAGE*3);
    struct drm_asahi_gem_bind_object object={.op=DRM_ASAHI_BIND_OBJECT_OP_BIND,
        .flags=DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,.handle=bo,.offset=PAGE,.range=PAGE};
    OK(fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&object);
    printf("MULTI_POOL_BEGIN jobs=%zu rounds=%u independent_outputs=%zu\n",JOBS,rounds,JOBS*8);
    for(unsigned j=0;j<JOBS;j++) {
        for(unsigned k=0;k<8;k++) memset(images[j][k],0xa5,RENDER_OUTPUT_SIZE);
        uint32_t fence=submit(fd,queues[j],j,0);wait_fence(fd,fence);check_images(j);
        struct drm_syncobj_destroy d={.handle=fence};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&d);
        printf("MULTI_POOL_SERIAL_PASS job=%u triangles=%u images=8 own_fence\n",j,pool_jobs[j].triangles);
    }
    unsigned third_overlaps=0,maximum_depth=0,maximum_inflight=0;
    for(unsigned round=0;round<rounds;round++) {
        uint32_t fences[JOBS];memset(timestamps+PAGE,0,PAGE);
        for(unsigned j=0;j<JOBS;j++) for(unsigned k=0;k<8;k++) memset(images[j][k],0xa5,RENDER_OUTPUT_SIZE);
        for(unsigned j=0;j<JOBS;j++) fences[j]=submit(fd,queues[j],j,object.object_handle);
        for(unsigned j=JOBS;j-->0;) {wait_fence(fd,fences[j]);check_images(j);}
        unsigned peak=0;
        for(unsigned j=0;j<JOBS;j++) {
            uint64_t start=*(uint64_t *)(timestamps+PAGE+j*64);unsigned live=0;
            for(unsigned k=0;k<JOBS;k++) {
                uint64_t *t=(void *)(timestamps+PAGE+k*64);
                live+=t[0]<=start && start<t[3];
            }
            if(live>peak) peak=live;
        }
        if(peak>maximum_inflight) maximum_inflight=peak;
        if(peak>=3) third_overlaps++;
        unsigned depth=0;uint64_t first_end=*(uint64_t *)(timestamps+PAGE+24);
        for(unsigned j=0;j<JOBS;j++) {
            uint64_t *t=(void *)(timestamps+PAGE+j*64);
            CHECK(t[0]>0 && t[0]<t[1] && t[2]>0 && t[2]<t[3]);
            if(t[1]<first_end) depth++;
            if(round<2 || round+1==rounds) printf("MULTI_POOL_TS round=%u job=%u ta=%llu,%llu fr=%llu,%llu\n",
                round,j,(unsigned long long)t[0],(unsigned long long)t[1],(unsigned long long)t[2],(unsigned long long)t[3]);
            for(unsigned b=32;b<64;b++) CHECK(timestamps[PAGE+j*64+b]==0);
            struct drm_syncobj_destroy d={.handle=fences[j]};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&d);
        }
        for(unsigned b=JOBS*64;b<PAGE;b++) CHECK(timestamps[PAGE+b]==0);
        for(unsigned b=0;b<PAGE;b++) CHECK(timestamps[b]==0xa5 && timestamps[PAGE*2+b]==0xa5);
        if(depth>maximum_depth) maximum_depth=depth;
        if(round<2 || (round+1)%64==0 || round+1==rounds) printf("MULTI_POOL_ROUND_PASS round=%u depth=%u peak_inflight=%u full_images=%zu own_fences=%zu guards\n",round,depth,peak,JOBS*8,JOBS);
    }
    printf("MULTI_POOL_PASS jobs=%zu rounds=%u overlap_rounds=%u maximum_depth=%u peak_inflight=%u images=%zu\n",
        JOBS,rounds,third_overlaps,maximum_depth,maximum_inflight,JOBS*8*(rounds+1));
    if(require_overlap) CHECK(third_overlaps==rounds && maximum_inflight>=3);
    CHECK(close(fd)==0);return 0;
}
