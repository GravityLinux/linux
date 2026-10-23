/* SPDX-License-Identifier: MIT */
/* One native DRM submission with our Mesa-generated constant32 resources.
 * No firmware objects, Apple shader bytes or reference UAT pages are inputs. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#define G17P_TIMESTAMP_CAPACITY 2
#include "g17p_drm_timestamp.h"
#include "g17p_drm_sync.h"

/* A close request must survive until CPU/input-blocked accepted work retires,
 * including a queue which has not constructed ANY firmware objects yet. */
static void submit_then_destroy(int fd, struct drm_asahi_submit *original,
    const unsigned char *output, const unsigned char *seed, size_t size)
{
	uint32_t input=sync_new(fd,0),done=sync_new(fd,0);
	int sw=sync_import_pending(fd,input);
	struct drm_asahi_sync syncs[2]={{.handle=input},{.handle=done}};
	struct drm_asahi_submit submit=*original;
	submit.syncs=(uintptr_t)syncs;submit.in_sync_count=1;submit.out_sync_count=1;
	CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit)==0);
	struct drm_asahi_queue_destroy destroy={.queue_id=submit.queue_id};
	OK(fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&destroy);
	BAD(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit,ENOENT);
	usleep(20000); CHECK(memcmp(output,seed,size)==0);
	struct drm_syncobj_wait wait={.handles=(uintptr_t)&done,.count_handles=1,
		.timeout_nsec=now_ns(),.flags=DRM_SYNCOBJ_WAIT_FLAGS_WAIT_ALL};
	BAD(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait,ETIME);
	uint32_t increment=1;OK(sw,SW_INC,&increment);
	wait.timeout_nsec=now_ns()+15000000000ULL;
	OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);
	CHECK(close(sw)==0);
	struct drm_syncobj_destroy drop={.handle=input};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&drop);
	drop.handle=done;OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&drop);
	printf("G16C_DESTROYED_ACCEPTED_QUEUE_PASS queue=%u\n",submit.queue_id);
}

static void *read_file(const char *folder, const char *name, size_t *size)
{
	char path[512]; CHECK(snprintf(path,sizeof(path),"%s/%s",folder,name)>0);
	FILE *f=fopen(path,"rb"); CHECK(f!=NULL);
	CHECK(fseek(f,0,SEEK_END)==0); long n=ftell(f); CHECK(n>0);
	CHECK(fseek(f,0,SEEK_SET)==0); void *p=malloc(n); CHECK(p!=NULL);
	CHECK(fread(p,1,n,f)==(size_t)n); CHECK(fclose(f)==0); *size=n; return p;
}
int main(int argc, char **argv)
{
	setbuf(stdout,NULL);
	const char *folder=argc>1?argv[1]:"/g16c-fixture";
	int use_timestamps=0,file_close=0,destroy_accepted=0,hold=0;
	unsigned replacements=0;
	unsigned queue_count=1;
	uint64_t kernel_base=0x7000000000ULL;
	for(int i=2;i<argc;i++) {
		if(!strcmp(argv[i],"--timestamps")) use_timestamps=1;
		else if(!strcmp(argv[i],"--hold")) hold=1;
		else if(!strcmp(argv[i],"--file-close")) file_close=1;
		else if(!strcmp(argv[i],"--queues=2")) queue_count=2;
		else if(!strcmp(argv[i],"--destroy-accepted")) destroy_accepted=1;
		else if(!strncmp(argv[i],"--replace=",10)) {
			char *end;unsigned long n=strtoul(argv[i]+10,&end,0);
			CHECK(*end==0 && n>0 && n<=4096);replacements=n;
		}
		else if(!strncmp(argv[i],"--kernel-base=",14)) {
			char *end; errno=0; kernel_base=strtoull(argv[i]+14,&end,0);
			CHECK(!errno && *end==0 && kernel_base && !(kernel_base&(PAGE-1)));
		}
		else CHECK(0);
	}
	struct timestamp_test timestamps={0};
	CHECK(!use_timestamps || (!replacements && !destroy_accepted));
	CHECK(!hold || (!use_timestamps && !replacements && !destroy_accepted && queue_count==1));
	if(destroy_accepted) CHECK(mount("debugfs","/sys/kernel/debug","debugfs",0,NULL)==0 || errno==EBUSY);
	size_t size; unsigned char *fixture=read_file(folder,"inputs.bin",&size);
	CHECK(size>=32); uint32_t header[8]; memcpy(header,fixture,32);
	CHECK(header[0]==0x36434741 && header[1]==1);
	unsigned count=header[2], binds=header[3], command_size=header[4];
	CHECK(count<128 && binds<4096 && command_size==72);
	CHECK(size==32+count*16+binds*sizeof(struct drm_asahi_gem_bind_op)+command_size);
	int fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC); CHECK(fd>=0);
	CHECK(kernel_base+0x20000000ULL>kernel_base);
	struct drm_asahi_vm_create vm={.kernel_start=kernel_base,.kernel_end=kernel_base+0x20000000ULL};
	OK(fd,DRM_IOCTL_ASAHI_VM_CREATE,&vm);
	struct drm_asahi_queue_create q={.vm_id=vm.vm_id,.usc_exec_base=EXEC};
	OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q);
	struct drm_asahi_queue_create q2={.vm_id=vm.vm_id,.usc_exec_base=EXEC};
	if(queue_count==2) OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q2);
	uint32_t handles[128]={0}; unsigned char *output=NULL,*seed=NULL; size_t output_size=0;
	for(unsigned i=0;i<count;i++) {
		uint32_t id,flags;uint64_t length;
		memcpy(&id,fixture+32+i*16,4);memcpy(&flags,fixture+36+i*16,4);memcpy(&length,fixture+40+i*16,8);
		CHECK(id<128 && !handles[id]);char name[64];snprintf(name,sizeof(name),"gem-%u.bin",id);
		size_t n;void *data=read_file(folder,name,&n);CHECK(n==length);
		handles[id]=bo_new(fd,n,flags,(flags&DRM_ASAHI_GEM_VM_PRIVATE)?vm.vm_id:0);
		unsigned char *map=bo_map(fd,handles[id],n);memcpy(map,data,n);
		if(id==header[5]) {output=map;seed=data;output_size=n;}
		else {CHECK(munmap(map,n)==0);free(data);}
	}
	CHECK(output && header[6]+header[7]<=output_size && header[7]==65536);
	struct drm_asahi_gem_bind_op *ops=(void *)(fixture+32+count*16);
	for(unsigned i=0;i<binds;i++) {CHECK(ops[i].handle<128 && handles[ops[i].handle]);ops[i].handle=handles[ops[i].handle];}
	struct drm_asahi_vm_bind bind_args={.vm_id=vm.vm_id,.num_binds=binds,.stride=sizeof(*ops),.userptr=(uintptr_t)ops};
	OK(fd,DRM_IOCTL_ASAHI_VM_BIND,&bind_args);
	struct drm_asahi_submit submit={.queue_id=q.queue_id,.cmdbuf_size=command_size,
		.cmdbuf=(uintptr_t)(fixture+32+count*16+binds*sizeof(*ops))};
	struct drm_asahi_cmd_compute *command=(void *)((unsigned char *)(uintptr_t)submit.cmdbuf+sizeof(struct drm_asahi_cmd_header));
	if(use_timestamps) timestamp_setup(fd,&timestamps,command,&submit);
	printf("G16C_NATIVE_COMPUTE_BEGIN vm=%u queue=%u output_bytes=%zu\n",vm.vm_id,q.queue_id,output_size);
	unsigned iterations=replacements?replacements*2:2;
	for(unsigned iteration=0;iteration<iterations;iteration++) {
	if(!q.queue_id) OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q);
	int survivor=queue_count==2 && iteration%2;
	submit.queue_id=survivor?q2.queue_id:q.queue_id;
	memcpy(output,seed,output_size);
	if(use_timestamps) timestamp_before(fd,&timestamps,command,iteration);
	if(destroy_accepted && !survivor) {
		submit_then_destroy(fd,&submit,output,seed,output_size);q.queue_id=0;
	} else OK(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit);
	unsigned changed=0;
	for(size_t i=0;i<output_size;i++) {
		unsigned char want=seed[i];
		if(i>=header[6] && i<header[6]+header[7]) want=(0xdeadbeefU>>((i-header[6])%4*8))&255;
		if(output[i]!=seed[i]) changed++;
		CHECK(output[i]==want);
	}
	CHECK(changed>0);
	if(use_timestamps) timestamp_after(&timestamps,iteration);
	printf("G16C_NATIVE_COMPUTE_OUTPUT iteration=%u words=16384 changed_bytes=%u whole_output_and_guards_checked=%zu\n",iteration,changed,output_size);
	if(replacements && !destroy_accepted && !survivor) {
		struct drm_asahi_queue_destroy destroy={.queue_id=q.queue_id};
		OK(fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&destroy);q.queue_id=0;
	}
	usleep(50000);
	}
    if(hold) {
        puts("G16C_COMPUTE_HOLD_READY");
        for(;;) {
            int ch=getchar();CHECK(ch!=EOF);
            if(ch=='Q')break;
            if(ch!='R')continue;
            for(size_t i=0;i<output_size;i++) {
                unsigned char want=seed[i];
                if(i>=header[6]&&i<header[6]+header[7])want=(0xdeadbeefU>>((i-header[6])%4*8))&255;
                CHECK(output[i]==want);
            }
            memcpy(output,seed,output_size);OK(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit);
            for(size_t i=0;i<output_size;i++) {
                unsigned char want=seed[i];
                if(i>=header[6]&&i<header[6]+header[7])want=(0xdeadbeefU>>((i-header[6])%4*8))&255;
                CHECK(output[i]==want);
            }
            puts("G16C_COMPUTE_HOLD_OUTPUT_PASS");
        }
    }
	if(use_timestamps) timestamp_finish(fd,&timestamps,2);
	printf("G16C_NATIVE_COMPUTE_PASS submissions=%u user_timestamps=%u\n",iterations,use_timestamps);
	if(!file_close) {
		struct drm_asahi_queue_destroy qd={.queue_id=q.queue_id};
		if(q.queue_id) OK(fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&qd);
		if(queue_count==2) {qd.queue_id=q2.queue_id;OK(fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&qd);}
		for(unsigned id=0;id<128;id++) if(handles[id]) bo_close(fd,handles[id]);
		vm_destroy(fd,vm.vm_id,0);
		printf("G16C_NATIVE_CONTEXT_RELEASE_PASS vm=%u\n",vm.vm_id);
	}
	CHECK(close(fd)==0);CHECK(munmap(output,output_size)==0);
	if(file_close) printf("G16C_NATIVE_FILE_CLOSE_PASS; kernel release receipt must also be checked\n");
	free(seed);free(fixture);return 0;
}
