/* SPDX-License-Identifier: MIT */
/* Raw asynchronous ioctls: two immutable-VM queues, disjoint outputs. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#undef ioctl
#include <linux/sync_file.h>
#include <poll.h>

static unsigned char *read_input(const char *folder, const char *name, size_t *size)
{
	char path[512]; CHECK(snprintf(path,sizeof(path),"%s/%s",folder,name)>0);
	FILE *f=fopen(path,"rb"); CHECK(f!=NULL);
	CHECK(!fseek(f,0,SEEK_END)); long n=ftell(f); CHECK(n>0);
	CHECK(!fseek(f,0,SEEK_SET)); unsigned char *b=malloc(n); CHECK(b!=NULL);
	CHECK(fread(b,1,n,f)==(size_t)n && !fclose(f)); *size=n; return b;
}
static uint32_t oracle(unsigned lane, unsigned steps)
{
	uint32_t value=0x5a110000U^lane;
	for(unsigned j=0;j<steps;j++) value=((value<<7)|(value>>25))+0x9e3779b9U+j;
	return value;
}
int main(int argc,char **argv)
{
	setbuf(stdout,NULL); CHECK(argc==2 || (argc==3 && (!strcmp(argv[2],"--preflight") || !strcmp(argv[2],"--serial") || !strcmp(argv[2],"--queued") || !strcmp(argv[2],"--warm") || !strcmp(argv[2],"--same-queue") || !strcmp(argv[2],"--same-queue-warm") || !strcmp(argv[2],"--same-queue-close"))));
    int serial=argc==3 && !strcmp(argv[2],"--serial");
    int queued=argc==3 && !strcmp(argv[2],"--queued");
    int same_queue=argc==3 && (!strcmp(argv[2],"--same-queue") || !strcmp(argv[2],"--same-queue-warm") || !strcmp(argv[2],"--same-queue-close"));
    int closing=argc==3 && !strcmp(argv[2],"--same-queue-close");
    int warm=argc==3 && (!strcmp(argv[2],"--warm") || !strcmp(argv[2],"--same-queue-warm") || closing);
    queued |= same_queue;
	size_t n; unsigned char *input=read_input(argv[1],"inputs.bin",&n);
	CHECK(n>=32); uint32_t h[8]; memcpy(h,input,32);
	CHECK(h[0]==0x384f4347 && (h[1]==1 || h[1]==2) && h[2]<128 && h[3]<4096 && h[4]==144);
	CHECK(n==32+h[2]*16+h[3]*sizeof(struct drm_asahi_gem_bind_op)+h[4]);
	unsigned char *seed[128]={0},*maps[128]={0},*expected[128]={0}; size_t sizes[128]={0}; uint32_t handles[128]={0};
	for(unsigned i=0;i<h[2];i++) {
		uint32_t id,flags;uint64_t length;memcpy(&id,input+32+i*16,4);memcpy(&flags,input+36+i*16,4);memcpy(&length,input+40+i*16,8);
		CHECK(id<128 && !seed[id] && length && !(length&(PAGE-1)) && (flags&~3U)==0);
		char name[64];snprintf(name,sizeof(name),"gem-%u.bin",id);seed[id]=read_input(argv[1],name,&sizes[id]);CHECK(sizes[id]==length);
	}
	for(unsigned job=0;job<2;job++) {
		unsigned id=h[5+job];CHECK(id<128 && sizes[id]==PAGE);
		char name[64];snprintf(name,sizeof(name),"expected-gem-%u.bin",id);expected[id]=read_input(argv[1],name,&n);CHECK(n==PAGE);
		for(unsigned i=0;i<PAGE;i++) {
			unsigned char want=seed[id][i];if(i<64)want=oracle(i/4,job && h[1]==1?1:h[7])>>(8*(i%4));CHECK(expected[id][i]==want);
		}
	}
	struct drm_asahi_gem_bind_op *ops=(void *)(input+32+h[2]*16);
	for(unsigned i=0;i<h[3];i++) CHECK(ops[i].handle<128 && seed[ops[i].handle] && ops[i].offset+ops[i].range<=sizes[ops[i].handle]);
	unsigned char *commands=input+32+h[2]*16+h[3]*sizeof(*ops);
	for(unsigned j=0;j<2;j++) {struct drm_asahi_cmd_header *ch=(void *)(commands+j*72);CHECK(ch->cmd_type==DRM_ASAHI_CMD_COMPUTE && ch->size==64 && ch->vdm_barrier==0xffff && ch->cdm_barrier==0xffff);}
	printf("G16C_COMPUTE_OVERLAP_INPUTS_PASS objects=%u binds=%u iterations=%u,%u CPU_oracle=1 raw_async=1\n",h[2],h[3],h[7],h[1]==1?1:h[7]);
	if(argc==3 && !serial && !queued && !warm)return 0;
	int fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC);CHECK(fd>=0);
	struct drm_asahi_vm_create vm={.kernel_start=0x7000000000ULL,.kernel_end=0x7020000000ULL};OK(fd,DRM_IOCTL_ASAHI_VM_CREATE,&vm);
	struct drm_asahi_queue_create q[2]={{.vm_id=vm.vm_id,.usc_exec_base=EXEC},{.vm_id=vm.vm_id,.usc_exec_base=EXEC}};
	for(unsigned j=0;j<2;j++) {
        if(same_queue && j)q[j]=q[0];
        else OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q[j]);
    }
    printf("OVERLAP_QUEUE_TOPOLOGY logical_queues=%u\n",same_queue?1U:2U);
	for(unsigned i=0;i<h[2];i++) {
		uint32_t id,flags;memcpy(&id,input+32+i*16,4);memcpy(&flags,input+36+i*16,4);
		handles[id]=bo_new(fd,sizes[id],flags,(flags&DRM_ASAHI_GEM_VM_PRIVATE)?vm.vm_id:0);
		maps[id]=bo_map(fd,handles[id],sizes[id]);memcpy(maps[id],seed[id],sizes[id]);
	}
	for(unsigned i=0;i<h[3];i++)ops[i].handle=handles[ops[i].handle];
	struct drm_asahi_vm_bind vb={.vm_id=vm.vm_id,.num_binds=h[3],.stride=sizeof(*ops),.userptr=(uintptr_t)ops};OK(fd,DRM_IOCTL_ASAHI_VM_BIND,&vb);
	uint32_t tsbo=bo_new(fd,PAGE*3,DRM_ASAHI_GEM_WRITEBACK,0);unsigned char *ts=bo_map(fd,tsbo,PAGE*3);memset(ts,0xa5,PAGE*3);
	struct drm_asahi_gem_bind_object obj={.op=DRM_ASAHI_BIND_OBJECT_OP_BIND,.flags=DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,.handle=tsbo,.offset=PAGE,.range=PAGE};OK(fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&obj);
	struct drm_syncobj_create done[2]={{0},{0}};struct drm_asahi_sync sync[2];struct drm_asahi_submit submits[2];
	for(unsigned j=0;j<2;j++) {
		OK(fd,DRM_IOCTL_SYNCOBJ_CREATE,&done[j]);sync[j]=(struct drm_asahi_sync){.handle=done[j].handle};
		struct drm_asahi_cmd_compute *cmd=(void *)(commands+j*72+8);
		cmd->ts.start=(struct drm_asahi_timestamp){.handle=obj.object_handle,.offset=64+j*32};
		cmd->ts.end=(struct drm_asahi_timestamp){.handle=obj.object_handle,.offset=72+j*32};
		submits[j]=(struct drm_asahi_submit){.queue_id=q[j].queue_id,.cmdbuf=(uintptr_t)(commands+j*72),.cmdbuf_size=72,.syncs=(uintptr_t)&sync[j],.out_sync_count=1};
	}
    unsigned phases=closing?2U:warm?3U:1U;
    for(unsigned phase=0;phase<phases;phase++) {
        int closing_now=closing && phase+1==phases;
        if(phase) {
            for(unsigned id=0;id<128;id++)if(seed[id])memcpy(maps[id],seed[id],sizes[id]);
            memset(ts,0xa5,PAGE*3);
            uint32_t resets[2]={done[0].handle,done[1].handle};
            struct drm_syncobj_array reset={.handles=(uintptr_t)resets,.count_handles=2};
            OK(fd,DRM_IOCTL_SYNCOBJ_RESET,&reset);
        }
        printf("OVERLAP_PHASE phase=%u warm=%d\n",phase,warm);
	puts("G16C_COMPUTE_OVERLAP_BEGIN");
	/* Both accepted before ANY completion wait. No g17p_drm_wait wrapper. */
	for(unsigned j=0;j<2;j++) {
        OK(fd,DRM_IOCTL_ASAHI_SUBMIT,&submits[j]);printf("OVERLAP_ACCEPTED job=%u queue=%u\n",j,q[j].queue_id);
        if(serial && !j) {
            struct timespec now;CHECK(!clock_gettime(CLOCK_MONOTONIC,&now));
            struct drm_syncobj_wait wait={.handles=(uintptr_t)&done[j].handle,.count_handles=1,.timeout_nsec=(int64_t)now.tv_sec*1000000000+now.tv_nsec+15000000000LL,.flags=DRM_SYNCOBJ_WAIT_FLAGS_WAIT_ALL};OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);
        }
    }
    int closed_fences[2]={-1,-1};
    if(closing_now) {
        for(unsigned j=0;j<2;j++) {
            struct drm_syncobj_handle export={.handle=done[j].handle,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
            OK(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&export);closed_fences[j]=export.fd;
        }
        struct pollfd pending={.fd=closed_fences[0],.events=POLLIN};
        CHECK(poll(&pending,1,0)==0);
        puts("CLOSE_WITH_TWO_ACCEPTED first_fence_still_pending=1");
        // Request native queue retirement while the two accepted jobs still
        // hold it. Surviving output mappings retain file references for readback.
        struct drm_asahi_queue_destroy destroy={.queue_id=q[0].queue_id};
        OK(fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&destroy);
        BAD(fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&destroy,ENOENT);
        struct drm_asahi_vm_destroy destroy_vm={.vm_id=vm.vm_id};
        BAD(fd,DRM_IOCTL_ASAHI_VM_DESTROY,&destroy_vm,EBUSY);
        puts("PENDING_QUEUE_DESTROY_PASS stale_queue=ENOENT VM_destroy=EBUSY");
        CHECK(!close(fd));fd=-1;
    }
    for(unsigned j=0;j<2;j++) {
        int fence_fd;
        if(closing_now) {
            fence_fd=closed_fences[j];struct pollfd ready={.fd=fence_fd,.events=POLLIN};
            CHECK(poll(&ready,1,15000)==1 && (ready.revents&POLLIN));
        } else {
            struct timespec now;CHECK(!clock_gettime(CLOCK_MONOTONIC,&now));
            struct drm_syncobj_wait wait={.handles=(uintptr_t)&done[j].handle,.count_handles=1,.timeout_nsec=(int64_t)now.tv_sec*1000000000+now.tv_nsec+15000000000LL,.flags=DRM_SYNCOBJ_WAIT_FLAGS_WAIT_ALL};OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);
            struct drm_syncobj_handle export={.handle=done[j].handle,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};OK(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&export);fence_fd=export.fd;
        }
        struct sync_file_info info={0};CHECK(!ioctl(fence_fd,SYNC_IOC_FILE_INFO,&info));CHECK(info.status==1);CHECK(!close(fence_fd));
        printf("OVERLAP_FENCE job=%u status=%d\n",j,info.status);
    }

	for(unsigned id=0;id<128;id++)if(seed[id]) {
		CHECK(!memcmp(maps[id],expected[id]?expected[id]:seed[id],sizes[id]));
		if(expected[id])printf("OVERLAP_OUTPUT gem=%u bytes=%zu words=16 full_CPU_and_guards=1\n",id,sizes[id]);
	}
	uint64_t times[2][2];for(unsigned j=0;j<2;j++){memcpy(times[j],ts+PAGE+64+j*32,16);CHECK(times[j][0] && times[j][1]>times[j][0]);printf("OVERLAP_TIMESTAMP job=%u start=%"PRIu64" end=%"PRIu64"\n",j,times[j][0],times[j][1]);}
	for(unsigned i=0;i<PAGE*3;i++)if(!(i>=PAGE+64 && i<PAGE+80) && !(i>=PAGE+96 && i<PAGE+112))CHECK(ts[i]==0xa5);
	if(serial || queued)CHECK(times[0][1]<times[1][0]);
    // Independent queues have no API ordering. Cold bindings serialize in
    // publication order, which may differ from the order of the two ioctls.
    else if(warm && phase==0)CHECK(times[0][1]<times[1][0] || times[1][1]<times[0][0]);
    else CHECK(times[1][0]<times[0][1] && times[0][0]<times[1][1]);
        printf("OVERLAP_PHASE_PASS phase=%u actual_overlap=%d full_outputs_and_guards=1\n",phase,
            times[1][0]<times[0][1] && times[0][0]<times[1][1]);
        if(closing && !closing_now)puts("CLOSE_WARMUP_PASS one_queue=1 first_pair_output_verified=1");
    }
	if(fd>=0)CHECK(!close(fd));
	printf("G16C_COMPUTE_%s_PASS logical_queues=%u, two commands, exact all BOs, timestamp guards, checks=%u\n",closing?"CLOSED_QUEUE_PIPELINE":same_queue?(warm?"ORDERED_QUEUE_REUSE":"ORDERED_QUEUE_PIPELINE"):serial?"SERIAL_CONTROL":queued?"PENDING_PAIR":warm?"WARM_REUSE":"OVERLAP",same_queue?1U:2U,checks);
	return 0;
}
