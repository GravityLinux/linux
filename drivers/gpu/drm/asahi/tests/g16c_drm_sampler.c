/* SPDX-License-Identifier: GPL-2.0-only */
/* Native sampler transport using output-qualified, wholly source-owned GEMs.
 * Native descriptor tables are normal caller DATA, never kernel shader edits. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main

struct object {uint32_t id,flags;uint64_t size,address;};
static struct object objects[128];
static unsigned char *maps[128],*seeds[128];
static unsigned object_count;
static void *load(const char *folder,const char *name,size_t *size)
{
	char path[512];CHECK(snprintf(path,sizeof(path),"%s/%s",folder,name)>0);
	FILE *f=fopen(path,"rb");CHECK(f!=NULL);CHECK(fseek(f,0,SEEK_END)==0);
	long n=ftell(f);CHECK(n>0);CHECK(fseek(f,0,SEEK_SET)==0);
	void *p=malloc(n);CHECK(p!=NULL);CHECK(fread(p,1,n,f)==(size_t)n);
	CHECK(fclose(f)==0);*size=n;return p;
}
static void *at(uint64_t address,uint64_t size)
{
	for(unsigned i=0;i<object_count;i++) {
		struct object *o=&objects[i];
		if(address>=o->address && address+size>=address && address+size<=o->address+o->size)
			return maps[i]+address-o->address;
	}
	CHECK(0);return NULL;
}
int main(int argc,char **argv)
{
	setbuf(stdout,NULL);CHECK(argc>=2 && argc<=3);
	unsigned raw_count=1;
	int raw=argc==3 && !strncmp(argv[2],"--raw",5);
	int wide=argc==3 && !strcmp(argv[2],"--wide");CHECK(argc==2 || raw || wide);
	if(raw && strcmp(argv[2],"--raw")) {
		CHECK(!strncmp(argv[2],"--raw=",6));char *end;unsigned long n=strtoul(argv[2]+6,&end,0);
		CHECK(!*end && n>0 && n<=32);raw_count=n;
	}
	size_t size;unsigned char *input=load(argv[1],"inputs.bin",&size);
	uint32_t header[8];uint64_t config[9];CHECK(size>=sizeof(header)+sizeof(config));
	memcpy(header,input,sizeof(header));memcpy(config,input+sizeof(header),sizeof(config));
	CHECK(header[0]==0x53434741 && header[1]==1 && header[2]<128);
	object_count=header[2];CHECK(size==104+object_count*sizeof(objects[0]));
	memcpy(objects,input+104,object_count*sizeof(objects[0]));
	int fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC);CHECK(fd>=0);
	struct drm_asahi_vm_create vm={.kernel_start=0x7000000000ULL,.kernel_end=0x7020000000ULL};
	OK(fd,DRM_IOCTL_ASAHI_VM_CREATE,&vm);
	uint32_t handles[128]={0};unsigned output_index=128;
	struct drm_asahi_gem_bind_op bindings[128]={0};
	for(unsigned i=0;i<object_count;i++) {
		struct object *o=&objects[i];char name[64];CHECK(o->id<128 && !handles[o->id]);
		snprintf(name,sizeof(name),"gem-%u.bin",o->id);seeds[i]=load(argv[1],name,&size);CHECK(size==o->size);
		handles[o->id]=bo_new(fd,o->size,o->flags,(o->flags&DRM_ASAHI_GEM_VM_PRIVATE)?vm.vm_id:0);
		maps[i]=bo_map(fd,handles[o->id],o->size);memcpy(maps[i],seeds[i],o->size);
		bindings[i]=(struct drm_asahi_gem_bind_op){.handle=handles[o->id],.range=o->size,
			.addr=o->address,.flags=RW};
		if(o->id==header[3])output_index=i;
	}
	CHECK(output_index<128);struct object *out=&objects[output_index];
	CHECK(header[4]+32<=out->size);
	struct drm_asahi_vm_bind bind={.vm_id=vm.vm_id,.num_binds=object_count,
		.stride=sizeof(bindings[0]),.userptr=(uintptr_t)bindings};OK(fd,DRM_IOCTL_ASAHI_VM_BIND,&bind);
	struct drm_asahi_queue_create q={.vm_id=vm.vm_id,.usc_exec_base=EXEC};OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q);
	struct {struct drm_asahi_cmd_header header;struct drm_asahi_cmd_compute compute;} cmd={
		.header={.cmd_type=DRM_ASAHI_CMD_COMPUTE,.size=sizeof(cmd.compute),
			.vdm_barrier=DRM_ASAHI_BARRIER_NONE,.cdm_barrier=DRM_ASAHI_BARRIER_NONE},
		.compute={.cdm_ctrl_stream_base=config[5],.cdm_ctrl_stream_end=config[5]+48}};
	struct drm_asahi_submit submit={.queue_id=q.queue_id,.cmdbuf=(uintptr_t)&cmd,.cmdbuf_size=sizeof(cmd)};
	/* Admission errors must leave the device usable and all caller DATA intact. */
	cmd.compute.sampler_heap=config[6];cmd.compute.sampler_count=0;
	BAD(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit,EINVAL);
	cmd.compute.sampler_heap=0;cmd.compute.sampler_count=1;
	BAD(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit,EINVAL);
	cmd.compute.sampler_heap=config[6]+1;
	BAD(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit,EINVAL);
	cmd.compute.sampler_heap=config[6];cmd.compute.sampler_count=UINT32_MAX;
	BAD(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit,EINVAL);
	cmd.compute.sampler_heap=config[7]+(4<<20)-8;cmd.compute.sampler_count=2;
	BAD(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit,EINVAL);
	printf("G16C_SAMPLER_ADMISSION_PASS negatives=5\n");
	uint32_t counts[]={1,17,513,65535,65537,500000,500000,500000,1};
	unsigned char *expected=malloc(out->size);CHECK(expected!=NULL);
	for(unsigned trial=0;trial<(raw?1:wide?9:3);trial++) {
		uint32_t count=raw?raw_count:counts[trial];uint64_t heap=config[trial==6?7:6];
		memcpy(maps[output_index],seeds[output_index],out->size);
		uint32_t indices[2]={0,1};memcpy(at(config[3],8),indices,8);
		uint64_t arguments[3]={1,28,count+27};memcpy(at(config[4],24),arguments,24);
		if(!raw) {
			CHECK((28ULL+count)*32<=config[8]);
			unsigned char *table=at(config[2],config[8]);memset(table,0,config[8]);
			memcpy(table,at(config[1],28*32),28*32);
			unsigned char *logical=at(heap,(uint64_t)count*8);
			for(unsigned i=0;i<count;i++)memcpy(table+(28ULL+i)*32,logical+i*8,8);
			memcpy(at(config[0],8),&config[2],8);
		}
		cmd.compute.sampler_heap=heap;cmd.compute.sampler_count=count;
		memcpy(expected,maps[output_index],out->size);
		/* Snapshot every input AFTER caller table construction. GPU/kernel must
		 * preserve all inputs, including the root slot, table and shader BO. */
		for(unsigned i=0;i<object_count;i++)if(i!=output_index)memcpy(seeds[i],maps[i],objects[i].size);
		OK(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit);
		float rgba[8];memcpy(rgba,maps[output_index]+header[4],sizeof(rgba));
		printf("G16C_SAMPLER_OBSERVATION trial=%u raw=%d count=%u heap=%#llx RGBA=%g,%g,%g,%g;%g,%g,%g,%g\n",
			trial,raw,count,(unsigned long long)heap,rgba[0],rgba[1],rgba[2],rgba[3],rgba[4],rgba[5],rgba[6],rgba[7]);
		if(raw)memcpy(expected+header[4],rgba,sizeof(rgba));
		else {float want[8]={1,1,1,1,1,1,1,1};
			if(count>1 && trial!=6)want[4]=want[5]=want[6]=0.5f;
			memcpy(expected+header[4],want,sizeof(want));}
		CHECK(memcmp(expected,maps[output_index],out->size)==0);
		for(unsigned i=0;i<object_count;i++)if(i!=output_index)CHECK(memcmp(seeds[i],maps[i],objects[i].size)==0);
		printf("G16C_SAMPLER_%s trial=%u count=%u output_guards=%llu all_input_BOs_unchanged=%u\n",
			raw?"RAW_OBSERVATION":"OUTPUT_PASS",trial,count,(unsigned long long)out->size-32,object_count-1);
		usleep(50000);
	}
	struct drm_asahi_queue_destroy destroy={.queue_id=q.queue_id};OK(fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&destroy);
	for(unsigned i=0;i<object_count;i++)bo_close(fd,handles[objects[i].id]);
	vm_destroy(fd,vm.vm_id,0);
	for(unsigned i=0;i<object_count;i++){CHECK(munmap(maps[i],objects[i].size)==0);free(seeds[i]);}
	CHECK(close(fd)==0);free(expected);free(input);
	printf("G16C_SAMPLER_CLOSE_PASS raw=%d\n",raw);return 0;
}
