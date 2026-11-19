/* SPDX-License-Identifier: GPL-2.0-only */
/* Two live immutable VM roots, identical virtual mappings, independent BOs. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#undef ioctl
#include <linux/sync_file.h>

static unsigned char *input_file(const char *dir, const char *name, size_t *size)
{
    char path[512];CHECK(snprintf(path,sizeof(path),"%s/%s",dir,name)>0);
    FILE *f=fopen(path,"rb");CHECK(f!=NULL);CHECK(!fseek(f,0,SEEK_END));
    long n=ftell(f);CHECK(n>0);CHECK(!fseek(f,0,SEEK_SET));
    unsigned char *p=malloc((size_t)n);CHECK(p!=NULL);
    CHECK(fread(p,1,(size_t)n,f)==(size_t)n && !fclose(f));*size=(size_t)n;return p;
}
static uint32_t rotate(uint32_t v,unsigned n){return (v<<n)|(v>>(32-n));}
static uint32_t private_hash(unsigned lane,unsigned words,unsigned selector)
{
    uint32_t scratch[4096],v=0x5a110000u^lane^selector;
    CHECK(words==256 || words==4096);
    for(unsigned j=0;j<words;j++){v=rotate(v,7)+0x9e3779b9u+j;scratch[j]=v^((j+1)*0x045d9f3bu);}
    for(unsigned j=0;j<words;j++){unsigned i=(j*13+selector+lane)&(words-1);scratch[i]=rotate(scratch[i]^scratch[(i+17)&(words-1)],5)+j+selector;}
    v=0x31415926u^lane;for(unsigned j=0;j<words;j++)v=rotate(v,3)^scratch[(j*29+selector)&(words-1)];return v;
}
struct root_case {
    int fd;
    unsigned char *timestamps;
    struct drm_asahi_gem_bind_object timestamp_object;
    struct drm_asahi_vm_create vm;
    struct drm_asahi_queue_create queue;
    uint32_t handles[128];unsigned char *maps[128];
    struct drm_syncobj_create done;
    struct drm_asahi_sync sync;
    struct drm_asahi_submit submit;
    unsigned char command[72];
};
static void wait_root(int fd,struct root_case *r,unsigned index)
{
    struct timespec now;CHECK(!clock_gettime(CLOCK_MONOTONIC,&now));
    struct drm_syncobj_wait wait={.handles=(uintptr_t)&r->done.handle,.count_handles=1,
        .timeout_nsec=(int64_t)now.tv_sec*1000000000+now.tv_nsec+15000000000LL,
        .flags=DRM_SYNCOBJ_WAIT_FLAGS_WAIT_ALL};OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);
    struct drm_syncobj_handle export={.handle=r->done.handle,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
    OK(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&export);
    struct sync_file_info info={0};CHECK(!ioctl(export.fd,SYNC_IOC_FILE_INFO,&info));CHECK(info.status==1);CHECK(!close(export.fd));
    printf("CROSS_VM_FENCE root=%u status=1\n",index);
}
static void drain_mark(const char *event,unsigned index)
{
    int fd=open("/dev/kmsg",O_WRONLY|O_CLOEXEC);CHECK(fd>=0);char text[128];
    int n=snprintf(text,sizeof(text),"<6>DRAIN_MARK event=%s index=%u\n",event,index);CHECK(n>0 && (size_t)n<sizeof(text));
    CHECK(write(fd,text,(size_t)n)==n);CHECK(!close(fd));
}
int main(int argc,char **argv)
{
    setbuf(stdout,NULL);CHECK(argc>=2);int serial=0,overlap=0,relocate=0,separate_files=0,drain_close=0,drain_arrival=0,return_after=0;unsigned rounds=3,jobs=8;
    for(int i=2;i<argc;i++){
        if(!strcmp(argv[i],"--serial"))serial=1;
        else if(!strcmp(argv[i],"--require-overlap"))overlap=1;
        else if(!strcmp(argv[i],"--relocate"))relocate=1;
        else if(!strcmp(argv[i],"--separate-files"))separate_files=1;
        else if(!strcmp(argv[i],"--drain-close"))drain_close=1;
        else if(!strcmp(argv[i],"--drain-arrival"))drain_arrival=1;
        else if(!strcmp(argv[i],"--return-after-arrival"))return_after=1;
        else if(!strncmp(argv[i],"--jobs=",7)){jobs=(unsigned)strtoul(argv[i]+7,NULL,10);CHECK(jobs>=4 && jobs<=32);}
        else if(!strncmp(argv[i],"--rounds=",9)){rounds=(unsigned)strtoul(argv[i]+9,NULL,10);CHECK(rounds>0);}
        else CHECK(0);
    }
    if(return_after)CHECK(drain_arrival);
    CHECK(!(serial && overlap));if(drain_close || drain_arrival){CHECK(separate_files && !overlap && !serial && !(drain_close && drain_arrival));rounds=0;}
    size_t n;unsigned char *input=input_file(argv[1],"inputs.bin",&n);CHECK(n>=32);
    uint32_t h[8];memcpy(h,input,32);int pair=h[0]==0x384f4347;
    CHECK((pair && (h[1]==1 || h[1]==2) && h[4]==144) ||
          (h[0]==0x39504347 && (h[1]==1 || h[1]==2) && h[4]==72));
    CHECK(h[2]<128 && h[3]<4096 && n==32+h[2]*16+h[3]*sizeof(struct drm_asahi_gem_bind_op)+h[4]);
    unsigned char *seed[128]={0},*expected[128]={0};size_t sizes[128]={0};uint32_t flags[128]={0};
    for(unsigned i=0;i<h[2];i++){
        uint32_t id;uint64_t length;memcpy(&id,input+32+i*16,4);CHECK(id<128 && !seed[id]);
        memcpy(&flags[id],input+36+i*16,4);memcpy(&length,input+40+i*16,8);
        CHECK(length && !(length&(PAGE-1)) && !(flags[id]&~3u));
        char name[64];snprintf(name,sizeof(name),"gem-%u.bin",id);seed[id]=input_file(argv[1],name,&sizes[id]);CHECK(sizes[id]==length);
    }
    if(drain_close || drain_arrival)CHECK(pair);
    unsigned outputs[2]={h[5],pair?h[6]:h[5]};
    for(unsigned j=0;j<2;j++){
        unsigned id=outputs[j];CHECK(id<128 && sizes[id]==PAGE);
        if(!expected[id]){char name[64];snprintf(name,sizeof(name),"expected-gem-%u.bin",id);expected[id]=input_file(argv[1],name,&n);CHECK(n==PAGE);}
        CHECK(memcmp(seed[id],expected[id],PAGE)!=0);
        if(!pair && h[1]==1){
            CHECK(h[6]==256 || h[6]==4096);
            for(unsigned i=0;i<16;i++){uint32_t value;memcpy(&value,expected[id]+i*4,4);CHECK(value==private_hash(i,h[6],h[7]));}
        }
    }
    const struct drm_asahi_gem_bind_op *original=(void *)(input+32+h[2]*16);
    for(unsigned i=0;i<h[3];i++)CHECK(original[i].handle<128 && seed[original[i].handle] && original[i].offset+original[i].range<=sizes[original[i].handle]);
    const unsigned char *commands=input+32+h[2]*16+h[3]*sizeof(*original);
    struct root_case roots[2]={0};int fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC);CHECK(fd>=0);
    for(unsigned j=0;j<2;j++){
        struct root_case *r=&roots[j];r->fd=j && separate_files?open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC):fd;CHECK(r->fd>=0);int fd=r->fd;r->vm.kernel_start=j && relocate?0x7100000000ULL:0x7000000000ULL;r->vm.kernel_end=r->vm.kernel_start+0x20000000;
        OK(fd,DRM_IOCTL_ASAHI_VM_CREATE,&r->vm);r->queue=(struct drm_asahi_queue_create){.vm_id=r->vm.vm_id,.usc_exec_base=EXEC};OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&r->queue);
        for(unsigned id=0;id<128;id++)if(seed[id]){r->handles[id]=bo_new(fd,sizes[id],flags[id],flags[id]&DRM_ASAHI_GEM_VM_PRIVATE?r->vm.vm_id:0);r->maps[id]=bo_map(fd,r->handles[id],sizes[id]);memcpy(r->maps[id],seed[id],sizes[id]);}
        struct drm_asahi_gem_bind_op *ops=malloc(h[3]*sizeof(*ops));CHECK(ops!=NULL);memcpy(ops,original,h[3]*sizeof(*ops));
        for(unsigned i=0;i<h[3];i++)ops[i].handle=r->handles[ops[i].handle];
        struct drm_asahi_vm_bind bind={.vm_id=r->vm.vm_id,.num_binds=h[3],.stride=sizeof(*ops),.userptr=(uintptr_t)ops};OK(fd,DRM_IOCTL_ASAHI_VM_BIND,&bind);free(ops);
        memcpy(r->command,commands+(pair?j*72:0),72);struct drm_asahi_cmd_header *ch=(void *)r->command;
        CHECK(ch->cmd_type==DRM_ASAHI_CMD_COMPUTE && ch->size==64);
        if(pair)CHECK(ch->vdm_barrier==0xffff && ch->cdm_barrier==0xffff);
        OK(fd,DRM_IOCTL_SYNCOBJ_CREATE,&r->done);r->sync=(struct drm_asahi_sync){.handle=r->done.handle};
        r->submit=(struct drm_asahi_submit){.queue_id=r->queue.queue_id,.cmdbuf=(uintptr_t)r->command,.cmdbuf_size=72,.syncs=(uintptr_t)&r->sync,.out_sync_count=1};
    }
    CHECK(separate_files?(roots[0].fd!=roots[1].fd):(roots[0].vm.vm_id!=roots[1].vm.vm_id && roots[0].queue.queue_id!=roots[1].queue.queue_id));
    for(unsigned j=0;j<2;j++){
        struct root_case *r=&roots[j];
        if(j==0 || separate_files){
            uint32_t tsbo=bo_new(r->fd,PAGE*3,DRM_ASAHI_GEM_WRITEBACK,0);r->timestamps=bo_map(r->fd,tsbo,PAGE*3);
            r->timestamp_object=(struct drm_asahi_gem_bind_object){.op=DRM_ASAHI_BIND_OBJECT_OP_BIND,.flags=DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,.handle=tsbo,.offset=PAGE,.range=PAGE};OK(r->fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&r->timestamp_object);
        } else {r->timestamps=roots[0].timestamps;r->timestamp_object=roots[0].timestamp_object;}
        struct drm_asahi_cmd_compute *c=(void *)(r->command+8);c->ts.start=(struct drm_asahi_timestamp){.handle=r->timestamp_object.object_handle,.offset=64+j*32};c->ts.end=(struct drm_asahi_timestamp){.handle=r->timestamp_object.object_handle,.offset=72+j*32};
    }
    printf("CROSS_VM_INPUTS_PASS roots=2 objects_per_root=%u private_words=%u cloned_physical_BOs=1 relocated=%d files=%u\n",h[2],pair?0:h[6],relocate,separate_files?2:1);

    CHECK(!pair && !serial && !overlap && !drain_close && !drain_arrival && !return_after);
    enum { QUEUES=130 };
    struct root_case *r=&roots[0];
    struct drm_asahi_queue_create q[QUEUES]={{0}};q[0]=r->queue;
    for(unsigned i=1;i<QUEUES;i++) {
        q[i]=(struct drm_asahi_queue_create){.vm_id=r->vm.vm_id,.usc_exec_base=EXEC};
        OK(r->fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q[i]);
    }
    puts("GENERATION_CAPACITY_ACCEPTED API_queues=130 one_live_VM=1 no_queue_destroy=1");
    uint64_t previous=0;
    for(unsigned round=0;round<rounds;round++) {
        for(unsigned i=0;i<QUEUES;i++) {
            for(unsigned id=0;id<128;id++)if(seed[id])memcpy(r->maps[id],seed[id],sizes[id]);
            memset(r->timestamps,0xa5,PAGE*3);
            struct drm_syncobj_array reset={.handles=(uintptr_t)&r->done.handle,.count_handles=1};
            OK(r->fd,DRM_IOCTL_SYNCOBJ_RESET,&reset);
            r->submit.queue_id=q[i].queue_id;
            OK(r->fd,DRM_IOCTL_ASAHI_SUBMIT,&r->submit);wait_root(r->fd,r,i);
            for(unsigned id=0;id<128;id++)if(seed[id]) {
                const unsigned char *want=id==outputs[0]?expected[id]:seed[id];
                CHECK(!memcmp(r->maps[id],want,sizes[id]));
            }
            uint64_t times[2];memcpy(times,r->timestamps+PAGE+64,16);
            CHECK(times[0]>previous && times[1]>times[0]);previous=times[1];
            for(unsigned j=0;j<PAGE*3;j++)if(j<PAGE+64 || j>=PAGE+80)CHECK(r->timestamps[j]==0xa5);
            printf("GENERATION_CAPACITY_OUTPUT round=%u API_queue=%u GPU_start=%"PRIu64" GPU_end=%"PRIu64" BOs_and_guards=1\n",round,i,times[0],times[1]);
        }
        drain_mark("GENERATION_ROUND",round);
    }
    for(unsigned i=0;i<QUEUES;i++){
        struct drm_asahi_queue_destroy destroy={.queue_id=q[i].queue_id};OK(r->fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&destroy);
    }
    for(unsigned j=0;j<2;j++)for(unsigned id=0;id<128;id++)if(seed[id])CHECK(!munmap(roots[j].maps[id],sizes[id]));
    for(unsigned j=0;j<2;j++)if(j==0 || separate_files)CHECK(!munmap(roots[j].timestamps,PAGE*3));
    if(separate_files)CHECK(!close(roots[1].fd));
    CHECK(!close(fd));
    printf("G16C_GENERATION_CAPACITY_PASS API_queues=%u rounds=%u commands=%u checks=%u\n",QUEUES,rounds,QUEUES*rounds,checks);
    return 0;
}
