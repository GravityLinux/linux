/* SPDX-License-Identifier: MIT */
/* Firmware dependencies retain real output/fence/timestamp ownership through
 * render backlog and mixed commands. All GPU programs are existing authored
 * integration fixtures; each long-prefix render has distinct output backing. */
#define G17P_CPU_LIBRARY
#include "g17p_drm_cpu_parallel.c"
#undef G17P_CPU_LIBRARY
#define PREFIX 32

static void dep_render(unsigned char *bytes,int second,uint32_t object,unsigned offset,
    uint16_t vdm,uint16_t cdm)
{
    memcpy(bytes,second?render_second_command:render_command,sizeof(render_command));
    struct drm_asahi_cmd_render *r=(void *)(bytes+sizeof(render_command)-sizeof(*r));
    struct drm_asahi_cmd_header *h=(void *)((unsigned char *)r-sizeof(*h));
    h->vdm_barrier=vdm;h->cdm_barrier=cdm;
    struct drm_asahi_timestamp *ts[]={&r->ts_vtx.start,&r->ts_vtx.end,&r->ts_frag.start,&r->ts_frag.end};
    for(unsigned i=0;i<4;i++)*ts[i]=(struct drm_asahi_timestamp){.handle=object,.offset=offset+i*8};
}
static void dep_submit(int fd,uint32_t queue,void *bytes,size_t size,uint32_t fence)
{
    struct drm_asahi_sync out={.handle=fence};
    struct drm_asahi_submit s={.queue_id=queue,.cmdbuf=(uintptr_t)bytes,.cmdbuf_size=size,.syncs=(uintptr_t)&out,.out_sync_count=1};
    CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&s)==0);
}
static void dep_image(unsigned char *image,unsigned target,unsigned triangles)
{
    float value=triangles*((target%8+1)/8.0f);unsigned char pixel[4];memcpy(pixel,&value,4);
    for(unsigned byte=0;byte<RENDER_OUTPUT_SIZE;byte++){
        unsigned char want=0;
        if(byte>=RENDER_PIXEL_0&&byte<RENDER_PIXEL_0+4)want=pixel[byte-RENDER_PIXEL_0];
        if(byte>=RENDER_PIXEL_1&&byte<RENDER_PIXEL_1+4)want=pixel[byte-RENDER_PIXEL_1];
        CHECK(image[byte]==want);
    }
}
int main(int argc,char **argv)
{
    setbuf(stdout,NULL);CHECK(argc>=2&&argc<=4);
    int rr=!strcmp(argv[1],"rrcc"),error=!strcmp(argv[1],"error");
    CHECK(rr||error||!strcmp(argv[1],"rcrc"));
    unsigned rounds=argc>=3?(unsigned)strtoul(argv[2],NULL,0):1;CHECK(rounds&&rounds<=64);
    int split=argc==4&&!strcmp(argv[3],"--split");CHECK(argc<4||split);
    cpu_render=1;int fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC);CHECK(fd>=0);
    struct cpu_client slow={0},fast={0};cpu_create(fd,&slow,0);cpu_create(fd,&fast,1);
    if(error){
        /* Only the authored VDM encoder draw count changes; GPU programs stay exact. */
        for(unsigned i=0;i<sizeof(render_workloads)/sizeof(render_workloads[0]);i++){
            const struct render_workload *w=&render_workloads[i];if(w->address!=0x1000018000ULL)continue;
            bind(fd,slow.vm,0,w->address,w->size,0,DRM_ASAHI_BIND_UNBIND,0);
            uint32_t bo=bo_new(fd,w->size,DRM_ASAHI_GEM_WRITEBACK,0);unsigned char *map=bo_map(fd,bo,w->size);memcpy(map,w->data,w->size);
            uint32_t vertices=400017*3;memcpy(map+0x68,&vertices,4);
            bind(fd,slow.vm,bo,w->address,w->size,0,DRM_ASAHI_BIND_READ,0);CHECK(munmap(map,w->size)==0);
        }
    }
    uint32_t timeline=sync_new(fd,0);
    submit(fd,fast.queue,2,fast.ts.object,0,fast.fence,timeline,1);wait_success(fd,fast.fence);cpu_check(&fast,1,2);
    uint32_t fences[PREFIX],tail_fences[3];
    for(unsigned i=0;i<PREFIX;i++)fences[i]=sync_new(fd,0);
    for(unsigned i=0;i<3;i++)tail_fences[i]=sync_new(fd,0);
    for(unsigned round=0;round<rounds;round++){
        memset(slow.ts.map,0xa5,PAGE*3);memset(fast.ts.map,0xa5,PAGE*3);memset(fast.output[2],0xa5,PAGE);
        for(unsigned j=0;j<16;j++)memset(slow.images[j],0xa5,RENDER_OUTPUT_SIZE);
        for(unsigned j=0;j<2;j++)memset(slow.output[j],0xa5,PAGE);
        unsigned count=error?1:PREFIX;unsigned char *old[PREFIX][8];
        for(unsigned n=0;n<count;n++){
            for(unsigned j=0;j<8;j++){
                bind(fd,slow.vm,0,render_outputs[j],RENDER_OUTPUT_SIZE,0,DRM_ASAHI_BIND_UNBIND,0);
                uint32_t bo=bo_new(fd,RENDER_OUTPUT_SIZE,DRM_ASAHI_GEM_WRITEBACK,0);
                old[n][j]=bo_map(fd,bo,RENDER_OUTPUT_SIZE);memset(old[n][j],0xa5,RENDER_OUTPUT_SIZE);
                bind(fd,slow.vm,bo,render_outputs[j],RENDER_OUTPUT_SIZE,0,RW,0);
                slow.images[j]=old[n][j];
            }
            union{uint64_t align;unsigned char bytes[sizeof(render_command)];} r;
            dep_render(r.bytes,0,slow.ts.object,64+n*32,DRM_ASAHI_BARRIER_NONE,DRM_ASAHI_BARRIER_NONE);
            dep_submit(fd,slow.queue,r.bytes,sizeof(r.bytes),fences[n]);
        }
        union{uint64_t align;unsigned char bytes[sizeof(render_command)+2*sizeof(struct cs_command)];} tail;
        unsigned offset=0,used[3],sizes[3],kind[3]={rr?0:1,rr?1:0,1},cs=0;
        unsigned steps=error?1:3;if(error)kind[0]=1;
        uint16_t history[2]={0,0};
        for(unsigned i=0;i<steps;i++){
            used[i]=offset;unsigned ts=64+count*32+i*32;
            if(!kind[i]){
                dep_render(tail.bytes+offset,1,slow.ts.object,ts,split?0:history[0],split?0:history[1]);
                sizes[i]=sizeof(render_command);
            }else{
                struct cs_command c=command(cs++,slow.ts.object);
                c.header.vdm_barrier=split?0:history[0];c.header.cdm_barrier=split?0:history[1];
                c.compute.ts.start.offset=ts;c.compute.ts.end.offset=ts+8;
                memcpy(tail.bytes+offset,&c,sizeof(c));sizes[i]=sizeof(c);
            }
            offset+=sizes[i];history[kind[i]]++;
        }
        if(split)for(unsigned i=0;i<steps;i++)dep_submit(fd,slow.queue,tail.bytes+used[i],sizes[i],tail_fences[i]);
        else dep_submit(fd,slow.queue,tail.bytes,offset,tail_fences[0]);
        submit(fd,fast.queue,2,fast.ts.object,0,fast.fence,timeline,round+2);wait_success(fd,fast.fence);cpu_check(&fast,1,2);
        int pending=status(fd,fences[count-1]);printf("GPU_DEPENDENCY_INDEPENDENT round=%u pair=%s producer_status=%d split=%d\n",round,argv[1],pending,split);
        if(error){
            struct drm_syncobj_wait w={.handles=(uintptr_t)&fences[0],.count_handles=1,.timeout_nsec=now_ns()+15000000000ULL};
            OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&w);printf("GPU_DEPENDENCY_ERROR_PRODUCER status=%d\n",status(fd,fences[0]));CHECK(status(fd,fences[0])==-ENOMEM);
            uint32_t consumer=tail_fences[0];w.handles=(uintptr_t)&consumer;w.timeout_nsec=now_ns()+15000000000ULL;
            OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&w);printf("GPU_DEPENDENCY_ERROR_CONSUMER status=%d\n",status(fd,consumer));CHECK(status(fd,consumer)==-ENOMEM);
            for(unsigned byte=0;byte<PAGE;byte++)CHECK(((unsigned char *)slow.output[0])[byte]==0xa5);
            for(unsigned byte=64+count*32;byte<64+count*32+16;byte++)CHECK(slow.ts.map[PAGE+byte]==0xa5);
        }else{
            CHECK(pending==0);
            for(unsigned n=0;n<count;n++)wait_success(fd,fences[n]);
            for(unsigned i=0;i<(split?steps:1);i++)wait_success(fd,tail_fences[i]);
            for(unsigned n=0;n<count;n++){
                for(unsigned j=0;j<8;j++)dep_image(old[n][j],j,RENDER_TRIANGLES);
                volatile uint64_t *t=(void *)(slow.ts.map+PAGE+64+n*32);
                CHECK(t[0]&&t[1]>t[0]&&t[2]>=t[1]&&t[3]>t[2]);
            }
            for(unsigned j=8;j<16;j++)dep_image(slow.images[j],j,RENDER_SECOND_TRIANGLES);
            for(unsigned g=0;g<2;g++){
                for(unsigned byte=0;byte<PAGE;byte++){
                    unsigned char want=0xa5;
                    if(byte<256){float v=1000.25f+g*513.0f+byte/4;unsigned char b[4];memcpy(b,&v,4);want=b[byte%4];}
                    CHECK(((unsigned char *)slow.output[g])[byte]==want);
                }
            }
            volatile uint64_t *last=(void *)(slow.ts.map+PAGE+64+(count-1)*32);
            volatile uint64_t *ft=(void *)(fast.ts.map+PAGE+64);CHECK(ft[1]<last[3]);
            uint64_t previous=last[3];
            for(unsigned i=0;i<steps;i++){
                volatile uint64_t *t=(void *)(slow.ts.map+PAGE+64+count*32+i*32);
                unsigned nr=kind[i]?2:4;CHECK(t[0]>previous);for(unsigned j=0;j<nr;j+=2)CHECK(t[j+1]>t[j]);previous=t[nr-1];
            }
            for(unsigned byte=0;byte<PAGE*3;byte++){
                int changed=byte>=PAGE+64&&byte<PAGE+64+count*32;
                for(unsigned i=0;i<steps;i++)changed|=byte>=PAGE+64+count*32+i*32&&byte<PAGE+64+count*32+i*32+(kind[i]?16:32);
                if(!changed)CHECK(slow.ts.map[byte]==0xa5);
            }
            printf("GPU_DEPENDENCY_ROUND_PASS pair=%s split=%d round=%u distinct_images=%u owned_fences=%u GPU_independent_before_producer=1\n",argv[1],split,round,count*8+8,count+(split?steps:1));
        }
    }
    CHECK(close(fd)==0);printf("G17P_GPU_DEPENDENCIES_PASS pair=%s rounds=%u split=%d checks=%u\n",argv[1],rounds,split,checks);return 0;
}
