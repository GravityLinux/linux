/* SPDX-License-Identifier: MIT */
/* Dynamic private arrays: independent C oracle, exact caller BOs and guards. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#undef ioctl
#include <linux/sync_file.h>

static unsigned char *read_input(const char *folder,const char *name,size_t *size)
{
    char path[512];CHECK(snprintf(path,sizeof(path),"%s/%s",folder,name)>0);
    FILE *f=fopen(path,"rb");CHECK(f!=NULL && !fseek(f,0,SEEK_END));
    long n=ftell(f);CHECK(n>0 && !fseek(f,0,SEEK_SET));
    unsigned char *b=malloc(n);CHECK(b!=NULL);
    CHECK(fread(b,1,n,f)==(size_t)n && !fclose(f));*size=n;return b;
}
static uint32_t rotate(uint32_t v,unsigned n){return (v<<n)|(v>>(32-n));}
static uint32_t oracle(unsigned lane,unsigned count,unsigned selector)
{
    uint32_t words[4096],value=0x5a110000U^lane^selector;
    for(unsigned j=0;j<count;j++) {
        value=rotate(value,7)+0x9e3779b9U+j;
        words[j]=value^((j+1)*0x045d9f3bU);
    }
    for(unsigned step=0;step<count;step++) {
        unsigned i=(step*13+selector+lane)&(count-1);
        words[i]=rotate(words[i]^words[(i+17)&(count-1)],5)+step+selector;
    }
    value=0x31415926U^lane;
    for(unsigned j=0;j<count;j++)value=rotate(value,3)^words[(j*29+selector)&(count-1)];
    return value;
}
int main(int argc,char **argv)
{
    setbuf(stdout,NULL);unsigned repeat=2;int preflight=0,expected_iteration=-1,expected_errno=0;uint64_t kernel_size=0x20000000ULL;CHECK(argc>=2);
    for(int arg=2;arg<argc;arg++) {
        if(!strcmp(argv[arg],"--preflight"))preflight=1;
        else if(!strncmp(argv[arg],"--expect-error=",15)) {
            char extra;CHECK(sscanf(argv[arg]+15,"%d:%d%c",&expected_iteration,&expected_errno,&extra)==2);
            CHECK(expected_iteration>=0 && (expected_errno==ENOMEM || expected_errno==ENOSPC));
        }
        else if(!strncmp(argv[arg],"--kernel-size=",14)) {
            char *end;errno=0;kernel_size=strtoull(argv[arg]+14,&end,0);
            CHECK(!errno && *end==0 && kernel_size>=0x20000000ULL && !(kernel_size&(PAGE-1)) && kernel_size<=(1ULL<<42)-0x7000000000ULL);
        }
        else {
            CHECK(!strncmp(argv[arg],"--repeat=",9));char *end;errno=0;
            unsigned long count=strtoul(argv[arg]+9,&end,0);CHECK(!errno && *end==0 && count>0 && count<=256);repeat=count;
        }
    }
    CHECK(expected_iteration<0 || (unsigned)expected_iteration<repeat);
    size_t n;unsigned char *input=read_input(argv[1],"inputs.bin",&n);
    uint32_t h[8];CHECK(n>=sizeof(h));memcpy(h,input,sizeof(h));
    CHECK(h[0]==0x39504347 && (h[1]==1 || h[1]==2) && h[2]<128 && h[3]<4096 && h[4]==72);
    int pressure=h[1]==2;
    CHECK(h[5]<128 && (h[6]==256 || h[6]==4096));
    CHECK(pressure ? h[6]==4096 && (h[7]==256 || h[7]==16384) : h[7]==9);
    CHECK(n==32+h[2]*16+h[3]*sizeof(struct drm_asahi_gem_bind_op)+72);
    unsigned char *seed[128]={0},*maps[128]={0};size_t sizes[128]={0};uint32_t handles[128]={0};
    for(unsigned i=0;i<h[2];i++) {
        uint32_t id,flags;uint64_t length;memcpy(&id,input+32+i*16,4);memcpy(&flags,input+36+i*16,4);memcpy(&length,input+40+i*16,8);
        CHECK(id<128 && !seed[id] && length && !(length&(PAGE-1)) && !(flags&~3U));
        char name[64];snprintf(name,sizeof(name),"gem-%u.bin",id);seed[id]=read_input(argv[1],name,&sizes[id]);CHECK(sizes[id]==length);
    }
    CHECK(sizes[h[5]]==PAGE);char name[64];snprintf(name,sizeof(name),"expected-gem-%u.bin",h[5]);
    unsigned char *expected=read_input(argv[1],name,&n);CHECK(n==PAGE);
    uint32_t words[16]={0};unsigned output_offset=pressure?128:0;
    if(pressure) {
        for(unsigned i=0;i<h[7];i++)words[i&15]^=oracle(i,h[6],9);
    } else for(unsigned i=0;i<16;i++)words[i]=oracle(i,h[6],9);
    for(unsigned i=0;i<PAGE;i++) {
        unsigned char want=seed[h[5]][i];
        if(i>=output_offset && i<output_offset+64)want=words[(i-output_offset)/4]>>(8*(i%4));
        CHECK(expected[i]==want);
    }
    struct drm_asahi_gem_bind_op *ops=(void *)(input+32+h[2]*16);
    for(unsigned i=0;i<h[3];i++)CHECK(ops[i].handle<128 && seed[ops[i].handle] && ops[i].offset+ops[i].range<=sizes[ops[i].handle]);
    unsigned char *commands=input+32+h[2]*16+h[3]*sizeof(*ops);
    struct drm_asahi_cmd_header *ch=(void *)commands;CHECK(ch->cmd_type==DRM_ASAHI_CMD_COMPUTE && ch->size==64);
    printf("G16C_PRIVATE_INPUTS_PASS words=%u threads=%u objects=%u binds=%u independent_C_oracle=1\n",h[6],pressure?h[7]:16,h[2],h[3]);
    if(preflight)return 0;
    int fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC);CHECK(fd>=0);
    struct drm_asahi_vm_create vm={.kernel_start=0x7000000000ULL,.kernel_end=0x7000000000ULL+kernel_size};OK(fd,DRM_IOCTL_ASAHI_VM_CREATE,&vm);
    printf("G16C_PRIVATE_VM start=%"PRIx64" end=%"PRIx64" bytes=%"PRIu64"\n",(uint64_t)vm.kernel_start,(uint64_t)vm.kernel_end,kernel_size);
    struct drm_asahi_queue_create q={.vm_id=vm.vm_id,.usc_exec_base=EXEC};OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q);
    for(unsigned i=0;i<h[2];i++) {
        uint32_t id,flags;memcpy(&id,input+32+i*16,4);memcpy(&flags,input+36+i*16,4);
        handles[id]=bo_new(fd,sizes[id],flags,(flags&DRM_ASAHI_GEM_VM_PRIVATE)?vm.vm_id:0);
        maps[id]=bo_map(fd,handles[id],sizes[id]);memcpy(maps[id],seed[id],sizes[id]);
    }
    for(unsigned i=0;i<h[3];i++)ops[i].handle=handles[ops[i].handle];
    struct drm_asahi_vm_bind vb={.vm_id=vm.vm_id,.num_binds=h[3],.stride=sizeof(*ops),.userptr=(uintptr_t)ops};OK(fd,DRM_IOCTL_ASAHI_VM_BIND,&vb);
    uint32_t tsbo=bo_new(fd,PAGE*3,DRM_ASAHI_GEM_WRITEBACK,0);unsigned char *ts=bo_map(fd,tsbo,PAGE*3);memset(ts,0xa5,PAGE*3);
    struct drm_asahi_gem_bind_object obj={.op=DRM_ASAHI_BIND_OBJECT_OP_BIND,.flags=DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,.handle=tsbo,.offset=PAGE,.range=PAGE};OK(fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&obj);
    uint64_t previous=0;
    for(unsigned iteration=0;iteration<repeat;iteration++) {
        memcpy(maps[h[5]],seed[h[5]],PAGE);
        unsigned char before_ts[PAGE*3];memcpy(before_ts,ts,sizeof(before_ts));
        struct drm_asahi_cmd_compute *cmd=(void *)(commands+8);
        cmd->ts.start=(struct drm_asahi_timestamp){.handle=obj.object_handle,.offset=64+iteration*32};
        cmd->ts.end=(struct drm_asahi_timestamp){.handle=obj.object_handle,.offset=72+iteration*32};
        struct drm_syncobj_create done={0};OK(fd,DRM_IOCTL_SYNCOBJ_CREATE,&done);
        struct drm_asahi_sync sync={.handle=done.handle};
        struct drm_asahi_submit submit={.queue_id=q.queue_id,.cmdbuf=(uintptr_t)commands,.cmdbuf_size=72,.syncs=(uintptr_t)&sync,.out_sync_count=1};
        printf("G16C_PRIVATE_BEGIN words=%u iteration=%u\n",h[6],iteration);OK(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit);
        struct timespec now;CHECK(!clock_gettime(CLOCK_MONOTONIC,&now));
        struct drm_syncobj_wait wait={.handles=(uintptr_t)&done.handle,.count_handles=1,.timeout_nsec=(int64_t)now.tv_sec*1000000000+now.tv_nsec+15000000000LL,.flags=DRM_SYNCOBJ_WAIT_FLAGS_WAIT_ALL};OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);
        struct drm_syncobj_handle export={.handle=done.handle,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};OK(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&export);
        int rejected=(int)iteration==expected_iteration;
        struct sync_file_info info={0};CHECK(!ioctl(export.fd,SYNC_IOC_FILE_INFO,&info));
        CHECK(info.status==(rejected?-expected_errno:1));CHECK(!close(export.fd));
        if(rejected) {
            for(unsigned id=0;id<128;id++)if(seed[id])CHECK(!memcmp(maps[id],seed[id],sizes[id]));
            CHECK(!memcmp(ts,before_ts,sizeof(before_ts)));
            printf("G16C_PRIVATE_REJECT iteration=%u errno=%d all_BO_bytes_untouched=1 all_timestamps_untouched=1 binary_fence=%d\n",iteration,expected_errno,info.status);
            struct drm_syncobj_destroy drop={.handle=done.handle};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&drop);
            continue;
        }
        for(unsigned id=0;id<128;id++)if(seed[id])CHECK(!memcmp(maps[id],id==h[5]?expected:seed[id],sizes[id]));
        CHECK(memcmp(maps[h[5]],seed[h[5]],PAGE));
        uint64_t times[2];memcpy(times,ts+PAGE+64+iteration*32,16);CHECK(times[0]>previous && times[1]>times[0]);previous=times[1];
        for(unsigned i=0;i<PAGE*3;i++)if(!(i>=PAGE+64+iteration*32 && i<PAGE+80+iteration*32))CHECK(ts[i]==before_ts[i]);
        printf("G16C_PRIVATE_OUTPUT words=%u iteration=%u all_BO_bytes_exact=1 output_guard_bytes=16320 fence=1 start=%"PRIu64" end=%"PRIu64"\n",h[6],iteration,times[0],times[1]);
        struct drm_syncobj_destroy drop={.handle=done.handle};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&drop);
    }
    CHECK(!close(fd));printf("G16C_PRIVATE_PASS words=%u threads=%u submissions=%u checks=%u rejected=%u\n",h[6],pressure?h[7]:16,repeat,checks,expected_iteration>=0);return 0;
}
