/* SPDX-License-Identifier: GPL-2.0-only */
/* Unplug with an accepted render blocked on an external input fence.
 * The command/BO fixture is source-built; nothing is published to the GPU. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include <linux/sync_file.h>
#include <poll.h>
#include <sys/mount.h>

struct sw_fence_create { uint32_t value; char name[32]; int32_t fence; };
#define SW_CREATE _IOWR('W', 0, struct sw_fence_create)
static unsigned char *load(const char *dir, const char *name, size_t *size)
{
	char path[512]; CHECK(snprintf(path,sizeof(path),"%s/%s",dir,name)>0);
	FILE *f=fopen(path,"rb"); CHECK(f); CHECK(!fseek(f,0,SEEK_END));
	long n=ftell(f); CHECK(n>0); rewind(f);
	unsigned char *p=malloc(n); CHECK(p); CHECK(fread(p,1,n,f)==(size_t)n);
	CHECK(!fclose(f)); *size=n; return p;
}
static int fence_status(int fd)
{
	struct sync_file_info info={0}; CHECK(!ioctl(fd,SYNC_IOC_FILE_INFO,&info));
	return info.status;
}
int main(int argc,char **argv)
{
	setbuf(stdout,NULL); CHECK(argc==3);
	CHECK(!mount("debugfs","/sys/kernel/debug","debugfs",0,NULL)||errno==EBUSY);
	size_t n; unsigned char *fixture=load(argv[1],"inputs.bin",&n);
	uint32_t h[8]; CHECK(n>=sizeof(h)); memcpy(h,fixture,sizeof(h));
	CHECK(h[0]==0x3242434d && h[2]<128 && h[3]<4096);
	CHECK(n==32+h[2]*16+h[3]*sizeof(struct drm_asahi_gem_bind_op)+h[4]);
	int fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC); CHECK(fd>=0);
	struct drm_asahi_vm_create vm={.kernel_start=0x7000000000ULL,.kernel_end=0x7020000000ULL};
	OK(fd,DRM_IOCTL_ASAHI_VM_CREATE,&vm);
	struct drm_asahi_queue_create q={.vm_id=vm.vm_id,.usc_exec_base=EXEC};
	OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q);
	uint32_t handles[128]={0}; size_t sizes[128]={0};
	unsigned char *maps[128]={0},*seeds[128]={0};
	for(unsigned i=0;i<h[2];i++) {
		uint32_t id,flags; uint64_t size;
		memcpy(&id,fixture+32+i*16,4); memcpy(&flags,fixture+36+i*16,4);
		memcpy(&size,fixture+40+i*16,8); CHECK(id<128&&!handles[id]);
		char name[64]; snprintf(name,sizeof(name),"gem-%u.bin",id);
		seeds[id]=load(argv[1],name,&sizes[id]); CHECK(sizes[id]==size);
		handles[id]=bo_new(fd,size,flags,(flags&DRM_ASAHI_GEM_VM_PRIVATE)?vm.vm_id:0);
		maps[id]=bo_map(fd,handles[id],size); memcpy(maps[id],seeds[id],size);
	}
	struct drm_asahi_gem_bind_op *ops=(void *)(fixture+32+h[2]*16);
	for(unsigned i=0;i<h[3];i++){CHECK(ops[i].handle<128&&handles[ops[i].handle]);ops[i].handle=handles[ops[i].handle];}
	struct drm_asahi_vm_bind binding={.vm_id=vm.vm_id,.num_binds=h[3],.stride=sizeof(*ops),.userptr=(uintptr_t)ops};
	OK(fd,DRM_IOCTL_ASAHI_VM_BIND,&binding);
	struct drm_syncobj_create input={0},done={0};
	OK(fd,DRM_IOCTL_SYNCOBJ_CREATE,&input); OK(fd,DRM_IOCTL_SYNCOBJ_CREATE,&done);
	int sw=open("/sys/kernel/debug/sync/sw_sync",O_RDWR|O_CLOEXEC); CHECK(sw>=0);
	struct sw_fence_create pending={.value=1,.name="asahi-unplug-pending"};
	OK(sw,SW_CREATE,&pending);
	struct drm_syncobj_handle import={.handle=input.handle,.fd=pending.fence,
		.flags=DRM_SYNCOBJ_FD_TO_HANDLE_FLAGS_IMPORT_SYNC_FILE};
	OK(fd,DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE,&import); CHECK(!close(pending.fence));
	struct drm_asahi_sync syncs[2]={{.handle=input.handle},{.handle=done.handle}};
	struct drm_asahi_submit submit={.queue_id=q.queue_id,.cmdbuf_size=h[4],
		.cmdbuf=(uintptr_t)(fixture+32+h[2]*16+h[3]*sizeof(*ops)),
		.syncs=(uintptr_t)syncs,.in_sync_count=1,.out_sync_count=1};
	CHECK(!g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit));
	struct drm_syncobj_handle export={.handle=done.handle,
		.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
	OK(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&export); CHECK(fence_status(export.fd)==0);
	usleep(20000); CHECK(fence_status(export.fd)==0);
	for(unsigned id=0;id<128;id++) if(maps[id]) {
		CHECK(!memcmp(maps[id],seeds[id],sizes[id]));
		CHECK(!munmap(maps[id],sizes[id])); free(seeds[id]);
	}
	CHECK(!close(fd)); free(fixture);
	puts("UNPLUG_PENDING_ACCEPTED file_and_all_GEM_mappings_closed=1 input_unsignaled=1 output_fence=0");
	int control=open("/sys/bus/platform/drivers/asahi_neo/unbind",O_WRONLY|O_CLOEXEC);
	CHECK(control>=0); size_t len=strlen(argv[2]); CHECK(len&&len<64);
	CHECK(write(control,argv[2],len)==(ssize_t)len); CHECK(!close(control));
	struct pollfd pollfd={.fd=export.fd,.events=POLLIN};
	CHECK(poll(&pollfd,1,15000)==1); CHECK(pollfd.revents&POLLIN);
	CHECK(fence_status(export.fd)==-ENODEV);
	CHECK(!close(export.fd)); CHECK(!close(sw));
	printf("UNPLUG_PENDING_PASS output_fence=%d accepted_unpublished_job_retired=1 checks=%u\n",-ENODEV,checks);
	return 0;
}
