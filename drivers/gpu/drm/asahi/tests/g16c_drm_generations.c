/* SPDX-License-Identifier: MIT */
/* Live-VM BO generations and accepted old snapshots, with source-owned Mesa DATA. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include <sys/mount.h>
struct sw_fence_create { uint32_t value; char name[32]; int32_t fence; };
#define SW_CREATE _IOWR('W', 0, struct sw_fence_create)
#define SW_INC _IOW('W', 1, uint32_t)
static uint64_t now_ns(void){struct timespec t;CHECK(!clock_gettime(CLOCK_MONOTONIC,&t));return (uint64_t)t.tv_sec*1000000000+t.tv_nsec;}
static uint32_t sync_new(int fd,uint32_t flags){struct drm_syncobj_create c={.flags=flags};OK(fd,DRM_IOCTL_SYNCOBJ_CREATE,&c);return c.handle;}
static int sync_import_pending(int fd,uint32_t handle){
 int sw=open("/sys/kernel/debug/sync/sw_sync",O_RDWR|O_CLOEXEC);CHECK(sw>=0);
 struct sw_fence_create c={.value=1,.name="g16c-old-snapshot"};OK(sw,SW_CREATE,&c);
 struct drm_syncobj_handle import={.handle=handle,.fd=c.fence,.flags=DRM_SYNCOBJ_FD_TO_HANDLE_FLAGS_IMPORT_SYNC_FILE};
 OK(fd,DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE,&import);CHECK(!close(c.fence));return sw;
}

struct image { uint32_t handle; unsigned char *allocation, *output; size_t size, prefix; int poisoned; };
static unsigned char *seed;
static size_t output_size;
static uint32_t output_offset, output_bytes;
static int render;
static void *load(const char *dir, const char *name, size_t *size)
{
 char path[512];CHECK(snprintf(path,sizeof(path),"%s/%s",dir,name)>0);
 FILE *f=fopen(path,"rb");CHECK(f);CHECK(!fseek(f,0,SEEK_END));long n=ftell(f);CHECK(n>0);rewind(f);
 void *p=malloc(n);CHECK(p);CHECK(fread(p,1,n,f)==(size_t)n);CHECK(!fclose(f));*size=n;return p;
}
static void reset(struct image *im)
{
 memset(im->allocation,0x5a,im->size);memcpy(im->output,seed,output_size);
}
static void result(struct image *im, int completed)
{
 for(size_t i=0;i<im->size;i++) {
  unsigned char want=0x5a;
  if(i>=im->prefix && i<im->prefix+output_size) {
   size_t j=i-im->prefix;want=seed[j];
   if(completed && j>=output_offset && j<output_offset+output_bytes) {
    unsigned b=(j-output_offset)%4;
    want=render?(b==0||b==3?255:0):(0xdeadbeefU>>(8*b))&255;
   }
  }
  CHECK(im->allocation[i]==want);
 }
}
static void old_guards(struct image *images,unsigned count)
{
 for(unsigned n=0;n<count;n++)if(images[n].poisoned)
  for(size_t i=0;i<images[n].size;i++)CHECK(images[n].allocation[i]==0x3c);
}
static void replace_output(int fd,uint32_t vm,struct image *images,unsigned index,
 struct drm_asahi_gem_bind_op *ops,const uint64_t *offsets,unsigned bindings,int poison)
{
 struct image *old=&images[index-1],*next=&images[index];
 next->size=output_size+2*PAGE;next->prefix=PAGE;
 next->handle=bo_new(fd,next->size,DRM_ASAHI_GEM_VM_PRIVATE|DRM_ASAHI_GEM_WRITEBACK,vm);
 next->allocation=bo_map(fd,next->handle,next->size);next->output=next->allocation+PAGE;reset(next);
 unsigned changed=0;
 for(unsigned i=0;i<bindings;i++)if(ops[i].handle==old->handle) {
  bind(fd,vm,0,ops[i].addr,ops[i].range,0,DRM_ASAHI_BIND_UNBIND,0);
  ops[i].handle=next->handle;ops[i].offset=offsets[i]+PAGE;
  struct drm_asahi_vm_bind b={.vm_id=vm,.num_binds=1,.stride=sizeof(*ops),.userptr=(uintptr_t)&ops[i]};
  OK(fd,DRM_IOCTL_ASAHI_VM_BIND,&b);changed++;
 }
 CHECK(changed);
 /* Keep old physical pages alive and visible as coherent read-only guards. */
 bind(fd,vm,old->handle,0x6003000000ULL+(index-1)*0x40000ULL,old->size,0,DRM_ASAHI_BIND_READ,0);
 if(poison){memset(old->allocation,0x3c,old->size);old->poisoned=1;}
}
static void submit_checked(int fd,struct drm_asahi_submit *submit,struct image *images,unsigned n)
{
 reset(&images[n]);OK(fd,DRM_IOCTL_ASAHI_SUBMIT,submit);result(&images[n],1);old_guards(images,n);
 printf("G16C_GENERATION_OUTPUT kind=%s generation=%u logical_bytes=%zu allocation_bytes=%zu old_guards=%u\n",render?"render":"compute",n,output_size,images[n].size,n);
}
int main(int argc,char **argv)
{
 setbuf(stdout,NULL);CHECK(argc>=2);unsigned generations=3;int deferred=0,destroy_old=0;
 for(int i=2;i<argc;i++) {
  if(!strncmp(argv[i],"--generations=",14)){char *end;unsigned long n=strtoul(argv[i]+14,&end,0);CHECK(!*end&&n>=2&&n<=256);generations=n;}
  else if(!strcmp(argv[i],"--deferred-old"))deferred=1;
  else if(!strcmp(argv[i],"--destroy-deferred-old")){deferred=1;destroy_old=1;}else CHECK(0);
 }
 size_t size;unsigned char *fixture=load(argv[1],"inputs.bin",&size);uint32_t h[8];CHECK(size>=32);memcpy(h,fixture,32);
 CHECK((h[0]==0x36434741||h[0]==0x3242434d)&&h[1]==1&&h[2]<128&&h[3]<4096);
 render=h[0]==0x3242434d;CHECK(size==32+h[2]*16+h[3]*sizeof(struct drm_asahi_gem_bind_op)+h[4]);
 int fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC);CHECK(fd>=0);
 struct drm_asahi_vm_create vm={.kernel_start=0x7000000000,.kernel_end=0x7020000000};OK(fd,DRM_IOCTL_ASAHI_VM_CREATE,&vm);
 struct drm_asahi_queue_create q[2]={{.vm_id=vm.vm_id,.usc_exec_base=EXEC},{.vm_id=vm.vm_id,.usc_exec_base=EXEC}};
 OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q[0]);OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q[1]);
 struct image *images=calloc(generations,sizeof(*images));CHECK(images);uint32_t handles[128]={0};
 for(unsigned i=0;i<h[2];i++) {
  uint32_t id,flags;uint64_t length;memcpy(&id,fixture+32+i*16,4);memcpy(&flags,fixture+36+i*16,4);memcpy(&length,fixture+40+i*16,8);CHECK(id<128&&!handles[id]);
  char name[64];snprintf(name,sizeof(name),"gem-%u.bin",id);size_t bytes;void *data=load(argv[1],name,&bytes);CHECK(bytes==length);
  handles[id]=bo_new(fd,bytes,flags,(flags&DRM_ASAHI_GEM_VM_PRIVATE)?vm.vm_id:0);unsigned char *map=bo_map(fd,handles[id],bytes);memcpy(map,data,bytes);
  if(id==h[5]){seed=data;output_size=bytes;images[0]=(struct image){.handle=handles[id],.allocation=map,.output=map,.size=bytes};}
  else {CHECK(!munmap(map,bytes));free(data);}
 }
 output_offset=h[6];output_bytes=h[7];CHECK(seed&&output_offset+output_bytes<=output_size&&output_bytes>0);
 struct drm_asahi_gem_bind_op *ops=(void*)(fixture+32+h[2]*16);uint64_t *offsets=calloc(h[3],sizeof(*offsets));CHECK(offsets);
 for(unsigned i=0;i<h[3];i++){CHECK(ops[i].handle<128&&handles[ops[i].handle]);ops[i].handle=handles[ops[i].handle];offsets[i]=ops[i].offset;}
 struct drm_asahi_vm_bind b={.vm_id=vm.vm_id,.num_binds=h[3],.stride=sizeof(*ops),.userptr=(uintptr_t)ops};OK(fd,DRM_IOCTL_ASAHI_VM_BIND,&b);
 struct drm_asahi_submit submit={.queue_id=q[0].queue_id,.cmdbuf_size=h[4],.cmdbuf=(uintptr_t)(fixture+32+h[2]*16+h[3]*sizeof(*ops))};
 printf("G16C_GENERATIONS_BEGIN kind=%s generations=%u deferred=%d vm=%u queues=%u,%u\n",render?"render":"compute",generations,deferred,vm.vm_id,q[0].queue_id,q[1].queue_id);
 submit_checked(fd,&submit,images,0);
 unsigned first=1;
 if(deferred) {
  CHECK(mount("debugfs","/sys/kernel/debug","debugfs",0,NULL)==0||errno==EBUSY);
  uint32_t input=sync_new(fd,0),done=sync_new(fd,0);int sw=sync_import_pending(fd,input);
  struct drm_asahi_sync syncs[2]={{.handle=input},{.handle=done}};struct drm_asahi_submit pending=submit;
  pending.syncs=(uintptr_t)syncs;pending.in_sync_count=1;pending.out_sync_count=1;reset(&images[0]);
  CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&pending)==0);
  if(destroy_old){
   struct drm_asahi_queue_destroy d={.queue_id=q[0].queue_id};OK(fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&d);
   BAD(fd,DRM_IOCTL_ASAHI_SUBMIT,&pending,ENOENT);
  }
  replace_output(fd,vm.vm_id,images,1,ops,offsets,h[3],0);submit.queue_id=q[1].queue_id;submit_checked(fd,&submit,images,1);
  result(&images[0],0);struct drm_syncobj_wait wait={.handles=(uintptr_t)&done,.count_handles=1,.timeout_nsec=now_ns(),.flags=DRM_SYNCOBJ_WAIT_FLAGS_WAIT_ALL};
  BAD(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait,ETIME);
  uint32_t inc=1;OK(sw,SW_INC,&inc);wait.timeout_nsec=now_ns()+15000000000ULL;OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);
  result(&images[0],1);result(&images[1],1);memset(images[0].allocation,0x3c,images[0].size);images[0].poisoned=1;
  submit_checked(fd,&submit,images,1);CHECK(!close(sw));
  if(destroy_old){OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q[0]);puts("G16C_DESTROYED_OLD_SNAPSHOT_QUEUE_PASS accepted_A_survived_rebind_and_logical_destroy=1");}
  struct drm_syncobj_destroy drop={.handle=input};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&drop);drop.handle=done;OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&drop);
  puts("G16C_ACCEPTED_OLD_SNAPSHOT_PASS B_completed_before_input_A_then_A_wrote_only_old_pages_then_B_resumed=1");first=2;
 }
 for(unsigned n=first;n<generations;n++) {
  replace_output(fd,vm.vm_id,images,n,ops,offsets,h[3],1);submit.queue_id=q[n%2].queue_id;submit_checked(fd,&submit,images,n);
 }
 for(unsigned i=0;i<2;i++){struct drm_asahi_queue_destroy d={.queue_id=q[i].queue_id};OK(fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&d);}
 for(unsigned i=0;i<h[2];i++){uint32_t id;memcpy(&id,fixture+32+i*16,4);bo_close(fd,handles[id]);}
 for(unsigned n=1;n<generations;n++)bo_close(fd,images[n].handle);
 struct drm_asahi_vm_destroy vd={.vm_id=vm.vm_id};OK(fd,DRM_IOCTL_ASAHI_VM_DESTROY,&vd);
 old_guards(images,generations);result(&images[generations-1],1);
 for(unsigned n=0;n<generations;n++)CHECK(!munmap(images[n].allocation,images[n].size));
 CHECK(!close(fd));free(images);free(offsets);free(fixture);free(seed);
 printf("G16C_GENERATIONS_PASS kind=%s generations=%u deferred_old=%d one_VM=1 explicit_close=1\n",render?"render":"compute",generations,deferred);return 0;
}
