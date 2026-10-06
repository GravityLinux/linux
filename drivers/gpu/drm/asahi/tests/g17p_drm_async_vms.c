/* Candidate pressure variant: 48 independent ready queues, exact full outputs/guards. */
/* SPDX-License-Identifier: MIT */
/* Render, 258 retained computes in five buffers, then render again. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include "g17p_drm_render_workload.h"
#define workload compute_workload
#define workloads compute_workloads
#include "g17p_drm_retained_wave_workload.h"
#undef workloads
#undef workload
#include "g17p_drm_sync.h"
#include "g17p_drm_timestamp.h"

struct command {
	struct drm_asahi_cmd_header attachment_header;
	struct drm_asahi_attachment attachment;
	struct drm_asahi_cmd_header header;
	struct drm_asahi_cmd_compute compute;
};
static void outputs_check(float **output, const int *epochs)
{
	for (unsigned graph = 0; graph < 56; graph++) {
		if (epochs[graph] >= 0)
			for (unsigned i = 0; i < 64; i++)
				CHECK(output[graph][i] == 2000.25f + (graph+8) * 129.0f + epochs[graph] * 8192.0f + i);
		for (unsigned i = epochs[graph] >= 0 ? 256 : 0; i < PAGE; i++)
			CHECK(((unsigned char *)output[graph])[i] == 0xa5);
	}
}
static void render_check(unsigned char **images, int complete)
{
	for (unsigned target = 0; target < 8; target++) {
		float value = RENDER_TRIANGLES * ((target + 1) / 8.0f);
		unsigned char expected[4]; memcpy(expected, &value, 4);
		for (unsigned byte = 0; byte < RENDER_OUTPUT_SIZE; byte++) {
			unsigned char want = complete ? 0 : 0xa5;
			if (complete && byte >= RENDER_PIXEL_0 && byte < RENDER_PIXEL_0 + 4) want = expected[byte - RENDER_PIXEL_0];
			if (complete && byte >= RENDER_PIXEL_1 && byte < RENDER_PIXEL_1 + 4) want = expected[byte - RENDER_PIXEL_1];
			CHECK(images[target][byte] == want);
		}
	}
}

static int async_status(int fd, uint32_t handle)
{
    struct drm_syncobj_handle e={.handle=handle,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
    OK(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&e);
    struct sync_file_info info={0};OK(e.fd,SYNC_IOC_FILE_INFO,&info);
    CHECK(close(e.fd)==0);return info.status;
}
static void async_wait(int fd,uint32_t handle)
{
    struct drm_syncobj_wait wait={.handles=(uintptr_t)&handle,.count_handles=1,
        .timeout_nsec=now_ns()+15000000000ULL};
    OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);
}
static void async_compute(int fd,uint32_t queue,unsigned graph,uint32_t input,
    uint32_t output,uint32_t timeline,uint64_t point)
{
    struct command command={
        .attachment_header={.cmd_type=DRM_ASAHI_SET_COMPUTE_ATTACHMENTS,
            .size=sizeof(struct drm_asahi_attachment),.vdm_barrier=DRM_ASAHI_BARRIER_NONE,.cdm_barrier=DRM_ASAHI_BARRIER_NONE},
        .attachment={.pointer=batch_workloads[graph].output,.size=PAGE},
        .header={.cmd_type=DRM_ASAHI_CMD_COMPUTE,.size=sizeof(struct drm_asahi_cmd_compute),
            .vdm_barrier=DRM_ASAHI_BARRIER_NONE,.cdm_barrier=DRM_ASAHI_BARRIER_NONE},
        .compute={.cdm_ctrl_stream_base=batch_workloads[graph].cdm,
            .cdm_ctrl_stream_end=batch_workloads[graph].cdm+batch_workloads[graph].cdm_size},
    };
    struct drm_asahi_sync syncs[3];unsigned n=0;
    if(input)syncs[n++]=(struct drm_asahi_sync){.handle=input};
    syncs[n++]=(struct drm_asahi_sync){.handle=output};
    syncs[n++]=(struct drm_asahi_sync){.sync_type=DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ,
        .handle=timeline,.timeline_value=point};
    struct drm_asahi_submit submit={.queue_id=queue,.cmdbuf=(uintptr_t)&command,.cmdbuf_size=sizeof(command),
        .syncs=(uintptr_t)syncs,.in_sync_count=!!input,.out_sync_count=2};
    uint64_t before=now_ns();CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit)==0);
    printf("ASYNC_ACCEPT graph=%u latency_ns=%llu status=%d\n",graph,
        (unsigned long long)(now_ns()-before),async_status(fd,output));
    /* The worker must use its copied command stream after ioctl return. */
    memset(&command,0xa5,sizeof(command));memset(syncs,0xa5,sizeof(syncs));
}
static void async_check(float **outputs,uint64_t complete_mask)
{
    for(unsigned graph=0;graph<56;graph++){
        int complete=!!(complete_mask & (1ULL << graph));
        if(complete)for(unsigned i=0;i<64;i++)
            CHECK(outputs[graph][i]==2000.25f+(graph+8)*129.0f+i);
        for(unsigned i=complete?256:0;i<PAGE;i++)CHECK(((unsigned char *)outputs[graph])[i]==0xa5);
    }
}
/* Exact VM ownership: a pending VM_A job must protect A, not unrelated B. */
static void async_unrelated_vm_mutation(int fd, uint32_t pending_vm)
{
    uint32_t other=vm_new(fd),bo=bo_new(fd,PAGE,DRM_ASAHI_GEM_WRITEBACK,0);
    const uint64_t address=0x18000;
    bind(fd,other,bo,address,PAGE,0,DRM_ASAHI_BIND_READ|DRM_ASAHI_BIND_WRITE,0);
    bind(fd,pending_vm,0,batch_workloads[0].output,PAGE,0,DRM_ASAHI_BIND_UNBIND,0);
    bind(fd,other,0,address,PAGE,0,DRM_ASAHI_BIND_UNBIND,0);
    vm_destroy(fd,other,0);
    bo_close(fd,bo);
    printf("G17P_ASYNC_UNRELATED_VM_PASS pending snapshot retained; same/unrelated VM unbind succeeds\n");
}
static volatile sig_atomic_t wait_interrupted;
static void interrupt_wait(int signal_number){(void)signal_number;wait_interrupted=1;}

int main(int argc, char **argv)
{
    setbuf(stdout,NULL);
    int closing=argc==2 && !strcmp(argv[1],"--close");
    int backend=argc==2 && !strcmp(argv[1],"--backend");
    CHECK(argc==1 || closing || backend);
	int fd = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	uint32_t vm = vm_new(fd);
	struct drm_asahi_queue_create queue = { .vm_id = vm, .usc_exec_base = EXEC };
	OK(fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &queue);
	unsigned char *images[8] = {0};
	for (unsigned i=0; i<sizeof(workloads)/sizeof(workloads[0]); i++) {
		const struct workload *w=&workloads[i]; uint32_t bo=bo_new(fd,w->size,DRM_ASAHI_GEM_WRITEBACK,0);
		void *map=bo_map(fd,bo,w->size);memcpy(map,w->data,w->size);int keep=0;
		for(unsigned j=0;j<8;j++) if(w->address==render_outputs[j]) { images[j]=map;keep=1;memset(map,0xa5,RENDER_OUTPUT_SIZE); }
		if(!keep) CHECK(munmap(map,w->size)==0);
		bind(fd,vm,bo,w->address,w->size,0,DRM_ASAHI_BIND_READ|(w->writable?DRM_ASAHI_BIND_WRITE:0),0);
	}
	for(unsigned j=0;j<8;j++)CHECK(images[j]);
	float *output[56] = {0}, *inputs[56] = {0};
    uint32_t output_handles[56]={0};

	CHECK(sizeof(batch_workloads) / sizeof(batch_workloads[0]) == 56);
	for (unsigned i = 0; i < sizeof(compute_workloads) / sizeof(compute_workloads[0]); i++) {
		const struct compute_workload *w = &compute_workloads[i];
		uint32_t bo = bo_new(fd, w->size, DRM_ASAHI_GEM_WRITEBACK, 0);
		void *map = bo_map(fd, bo, w->size);
		memcpy(map, w->data, w->size);
		int keep = 0;
		for (unsigned j = 0; j < 56; j++) if (w->address == batch_workloads[j].output) {
			output[j] = map; output_handles[j]=bo; keep = 1; memset(map, 0xa5, PAGE);
		}
		for (unsigned j = 0; j < 56; j++) if (w->address == batch_workloads[j].input_a) { inputs[j] = map; keep = 1; }
		if (!keep) CHECK(munmap(map, w->size) == 0);
		bind(fd, vm, bo, w->address, w->size, 0,
		     DRM_ASAHI_BIND_READ | (w->writable ? DRM_ASAHI_BIND_WRITE : 0), 0);
	}
	for (unsigned i = 0; i < 56; i++) CHECK(output[i] && inputs[i]);


    for(unsigned graph=0;graph<56;graph++)for(unsigned i=0;i<64;i++)
        inputs[graph][i]=2000+(graph+8)*128+i;
    struct drm_asahi_submit draw={.queue_id=queue.queue_id,.cmdbuf=(uintptr_t)render_command,.cmdbuf_size=sizeof(render_command)};
    OK(fd,DRM_IOCTL_ASAHI_SUBMIT,&draw);render_check(images,1);
    CHECK(mount("debugfs","/sys/kernel/debug","debugfs",0,NULL)==0 || errno==EBUSY);
    uint32_t input=sync_new(fd,0),binary[4],timeline=sync_new(fd,0);
    for(unsigned i=0;i<4;i++)binary[i]=sync_new(fd,0);
    int sw=sync_import_pending(fd,input);
    async_compute(fd,queue.queue_id,0,input,binary[0],timeline,1);
    CHECK(async_status(fd,binary[0])==0);async_check(output,0);
    async_unrelated_vm_mutation(fd,vm);
    if(closing){
        struct drm_syncobj_handle exported={.handle=binary[0],.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
        OK(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&exported);
        CHECK(close(fd)==0);
        uint32_t increment=1;OK(sw,SW_INC,&increment);
        struct sync_file_info info={0};uint64_t deadline=now_ns()+15000000000ULL;
        do {info=(struct sync_file_info){0};OK(exported.fd,SYNC_IOC_FILE_INFO,&info);if(!info.status)usleep(1000);CHECK(now_ns()<deadline);}while(!info.status);
        CHECK(info.status==1);async_check(output,1);render_check(images,1);
        CHECK(close(exported.fd)==0);CHECK(close(sw)==0);
        printf("G17P_ASYNC_FILE_CLOSE_PASS accepted work completed after DRM file/handles/VM teardown; full output and guards preserved\n");
        return 0;
    }
    async_compute(fd,queue.queue_id,1,0,binary[1],timeline,2);
    CHECK(async_status(fd,binary[1])==0);async_check(output,0);
    struct drm_asahi_queue_create independent={.vm_id=vm,.usc_exec_base=EXEC};
    OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&independent);
    async_compute(fd,independent.queue_id,2,0,binary[2],timeline,3);
    async_wait(fd,binary[2]);CHECK(async_status(fd,binary[2])==1);
    CHECK(async_status(fd,binary[0])==0 && async_status(fd,binary[1])==0);
    async_check(output,4);render_check(images,1);
    uint32_t increment=1;OK(sw,SW_INC,&increment);
    async_wait(fd,binary[1]);
    /* GPU compute FIFO is ordered; independently completed submit fences
     * need not be notified in that order by separate completion workers. */
    int earlier_status=async_status(fd,binary[0]);
    CHECK((earlier_status==0 || earlier_status==1) && async_status(fd,binary[1])==1);
    async_check(output,7); /* Keep the GPU ordering oracle before waiting. */
    printf("G17P_ASYNC_OWN_FENCES earlier_status=%d after later CS completion\n",earlier_status);
    async_wait(fd,binary[0]);
    CHECK(async_status(fd,binary[0])==1 && async_status(fd,binary[1])==1);
    async_check(output,7);render_check(images,1);CHECK(close(sw)==0);
    uint64_t point=2;
    struct drm_syncobj_timeline_wait wait={.handles=(uintptr_t)&timeline,.points=(uintptr_t)&point,
        .count_handles=1,.timeout_nsec=now_ns()+15000000000ULL};
    OK(fd,DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT,&wait);
    sw=sync_import_pending(fd,input);
    async_compute(fd,queue.queue_id,3,input,binary[3],timeline,4);
    CHECK(async_status(fd,binary[3])==0);CHECK(close(sw)==0);
    async_wait(fd,binary[3]);CHECK(async_status(fd,binary[3])==-ENOENT);
    /* Failed dependencies retire the VM lease exactly once before signalling. */
    bind(fd,vm,0,batch_workloads[3].output,PAGE,0,DRM_ASAHI_BIND_UNBIND,0);
    bind(fd,vm,output_handles[3],batch_workloads[3].output,PAGE,0,
        DRM_ASAHI_BIND_READ|DRM_ASAHI_BIND_WRITE,0);
    printf("G17P_ASYNC_FAILED_VM_LEASE_PASS failed input releases exact VM guard\n");
    async_check(output,7);render_check(images,1);
    for(unsigned i=0;i<8;i++)memset(images[i],0xa5,RENDER_OUTPUT_SIZE);
    OK(fd,DRM_IOCTL_ASAHI_SUBMIT,&draw);render_check(images,1);async_check(output,7);
    printf("G17P_ASYNC_FRONTEND_PASS pending ioctls returned; same-queue order, independent-queue dependency progress, copied commands, binary/timeline fences, dependency error and recovery; complete images/guards; checks=%u\n",checks);
    if(backend){
        uint32_t burst_input=sync_new(fd,0),fences[48],timelines[48];
        int producer=sync_import_pending(fd,burst_input);
        for(unsigned i=0;i<48;i++){
            struct drm_asahi_queue_create q={.vm_id=vm,.usc_exec_base=EXEC};
            OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q);
            fences[i]=sync_new(fd,0);timelines[i]=sync_new(fd,0);
            async_compute(fd,q.queue_id,i+4,burst_input,fences[i],timelines[i],1);
            struct drm_asahi_queue_destroy destroy={.queue_id=q.queue_id};
            OK(fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&destroy);
        }
        /* An external producer can take arbitrarily long. Input time is not
         * GPU execution time; accepted fences stay pending beyond two seconds. */
        struct drm_asahi_gem_bind_op unmap={.addr=batch_workloads[4].output,.range=PAGE,.flags=DRM_ASAHI_BIND_UNBIND};
        struct drm_asahi_vm_bind unbind={.vm_id=vm,.num_binds=1,.stride=sizeof(unmap),.userptr=(uintptr_t)&unmap};
        OK(fd,DRM_IOCTL_ASAHI_VM_BIND,&unbind);
        /* Accepted jobs retain their original output mappings after unbind. */
        for(unsigned i=4;i<52;i++)bo_close(fd,output_handles[i]);
        struct sigaction action={.sa_handler=interrupt_wait},old_action;
        sigemptyset(&action.sa_mask);CHECK(sigaction(SIGUSR1,&action,&old_action)==0);
        pid_t interrupter=fork();CHECK(interrupter>=0);
        if(!interrupter){usleep(50000);kill(getppid(),SIGUSR1);_exit(0);}
        struct drm_syncobj_wait blocked={.handles=(uintptr_t)&fences[0],.count_handles=1,
            .timeout_nsec=now_ns()+15000000000ULL};
        errno=0;CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_SYNCOBJ_WAIT,&blocked)==-1 && errno==EINTR);
        int child_status;CHECK(waitpid(interrupter,&child_status,0)==interrupter);
        CHECK(WIFEXITED(child_status) && WEXITSTATUS(child_status)==0 && wait_interrupted);
        CHECK(sigaction(SIGUSR1,&old_action,NULL)==0);
        usleep(3000000);
        for(unsigned i=0;i<48;i++)CHECK(async_status(fd,fences[i])==0);
        async_check(output,7);render_check(images,1);
        uint32_t increase=1;OK(producer,SW_INC,&increase);
        for(unsigned i=0;i<48;i++){
            async_wait(fd,fences[i]);CHECK(async_status(fd,fences[i])==1);
            uint64_t one=1;
            struct drm_syncobj_timeline_wait done={.handles=(uintptr_t)&timelines[i],
                .points=(uintptr_t)&one,.count_handles=1,.timeout_nsec=now_ns()+15000000000ULL};
            OK(fd,DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT,&done);
        }
        async_check(output,((1ULL<<52)-1)&~8ULL);render_check(images,1);CHECK(close(producer)==0);
        printf("G17P_ASYNC_PRESSURE_PASS 48 accepted jobs on destroyed queues, imported input pending beyond two seconds, independent completion and timeline fences, every output/image/guard; checks=%u\n",checks);
    }
    CHECK(close(fd)==0);return 0;
}
