/* SPDX-License-Identifier: GPL-2.0-only */
/* Exact logical timestamp spans, page-rounded aliases and accepted-work leases. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include <sys/mount.h>

struct sw_create { uint32_t value; char name[32]; int32_t fence; };
#define SW_CREATE_RANGE _IOWR('W',0,struct sw_create)
#define SW_INC_RANGE _IOW('W',1,uint32_t)
struct scene {
 int fd;uint32_t vm,queue,h[8],handles[128];unsigned nrefs;
 unsigned char *fixture,*maps[128],*seeds[128];size_t sizes[128];
 struct drm_asahi_timestamp *refs[4];struct drm_asahi_submit submit;
};
struct stamps {
 unsigned n;uint32_t bo[4],object[4];unsigned char *map[4];
 uint64_t offset,size,at[4],value[4];unsigned char seed;
};
static unsigned char *read_blob(const char *dir,const char *name,size_t *size){
 char path[512];CHECK(snprintf(path,sizeof(path),"%s/%s",dir,name)>0);
 FILE*f=fopen(path,"rb");CHECK(f);CHECK(!fseek(f,0,SEEK_END));long n=ftell(f);CHECK(n>0);rewind(f);
 unsigned char*b=malloc(n);CHECK(b);CHECK(fread(b,1,n,f)==(size_t)n);CHECK(!fclose(f));*size=n;return b;
}
static void scene_open(struct scene*r,const char*dir){
 size_t n;r->fixture=read_blob(dir,"inputs.bin",&n);CHECK(n>=32);memcpy(r->h,r->fixture,32);
 CHECK(r->h[1]==1&&r->h[2]<128&&r->h[3]<4096);
 int render=r->h[0]==0x3242434d;CHECK(render||r->h[0]==0x36434741);r->nrefs=render?4:2;
 CHECK(n==32+r->h[2]*16+r->h[3]*sizeof(struct drm_asahi_gem_bind_op)+r->h[4]);
 r->fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC);CHECK(r->fd>=0);
 struct drm_asahi_vm_create vm={.kernel_start=0x7100004000ULL,.kernel_end=0x7120004000ULL};OK(r->fd,DRM_IOCTL_ASAHI_VM_CREATE,&vm);r->vm=vm.vm_id;
 struct drm_asahi_queue_create q={.vm_id=r->vm,.usc_exec_base=EXEC};OK(r->fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q);r->queue=q.queue_id;
 for(unsigned i=0;i<r->h[2];i++){
  uint32_t id,flags;uint64_t size;memcpy(&id,r->fixture+32+i*16,4);memcpy(&flags,r->fixture+36+i*16,4);memcpy(&size,r->fixture+40+i*16,8);
  CHECK(id<128&&!r->handles[id]);char name[64];snprintf(name,sizeof(name),"gem-%u.bin",id);r->seeds[id]=read_blob(dir,name,&n);CHECK(n==size);r->sizes[id]=n;
  r->handles[id]=bo_new(r->fd,n,flags,(flags&DRM_ASAHI_GEM_VM_PRIVATE)?r->vm:0);r->maps[id]=bo_map(r->fd,r->handles[id],n);memcpy(r->maps[id],r->seeds[id],n);
 }
 struct drm_asahi_gem_bind_op*ops=(void*)(r->fixture+32+r->h[2]*16);
 for(unsigned i=0;i<r->h[3];i++){CHECK(ops[i].handle<128&&r->handles[ops[i].handle]);ops[i].handle=r->handles[ops[i].handle];}
 struct drm_asahi_vm_bind bind={.vm_id=r->vm,.num_binds=r->h[3],.stride=sizeof(*ops),.userptr=(uintptr_t)ops};OK(r->fd,DRM_IOCTL_ASAHI_VM_BIND,&bind);
 r->submit=(struct drm_asahi_submit){.queue_id=r->queue,.cmdbuf_size=r->h[4],.cmdbuf=(uintptr_t)(r->fixture+32+r->h[2]*16+r->h[3]*sizeof(*ops))};
 if(render){struct drm_asahi_cmd_render*c=(void*)(uintptr_t)(r->submit.cmdbuf+r->h[4]-sizeof(*c));r->refs[0]=&c->ts_vtx.start;r->refs[1]=&c->ts_vtx.end;r->refs[2]=&c->ts_frag.start;r->refs[3]=&c->ts_frag.end;}
 else{struct drm_asahi_cmd_compute*c=(void*)(uintptr_t)(r->submit.cmdbuf+8);r->refs[0]=&c->ts.start;r->refs[1]=&c->ts.end;}
 CHECK(r->h[5]<128&&r->maps[r->h[5]]&&(uint64_t)r->h[6]+r->h[7]<=r->sizes[r->h[5]]);
}
static void output_reset(struct scene*r){memcpy(r->maps[r->h[5]],r->seeds[r->h[5]],r->sizes[r->h[5]]);}
static void scene_check(struct scene*r,int submitted){
 unsigned changed=0;
 for(unsigned id=0;id<128;id++)if(r->maps[id])for(size_t at=0;at<r->sizes[id];at++){
  unsigned char want=r->seeds[id][at];
  if(submitted&&id==r->h[5]&&at>=r->h[6]&&at<r->h[6]+r->h[7])want=((r->nrefs==4?0xff0000ffU:0xdeadbeefU)>>((at-r->h[6])%4*8))&255;
  CHECK(r->maps[id][at]==want);changed+=r->maps[id][at]!=r->seeds[id][at];
 }
 if(submitted)CHECK(changed>0);
}
static void stamp_make(struct scene*r,struct stamps*t,uint64_t offset,uint64_t size,unsigned char seed,int edge){
 t->n=r->nrefs;t->offset=offset;t->size=size;t->seed=seed;
 for(unsigned i=0;i<t->n;i++){
  t->bo[i]=bo_new(r->fd,3*PAGE,DRM_ASAHI_GEM_WRITEBACK,0);t->map[i]=bo_map(r->fd,t->bo[i],3*PAGE);memset(t->map[i],seed,3*PAGE);
  struct drm_asahi_gem_bind_object obj={.op=DRM_ASAHI_BIND_OBJECT_OP_BIND,.flags=DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,.handle=t->bo[i],.offset=offset,.range=size};
  OK(r->fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&obj);t->object[i]=obj.object_handle;t->at[i]=offset+((edge&&i%2)?size-8:0);
 }
}
static void stamp_select(struct scene*r,struct stamps*t){for(unsigned i=0;i<t->n;i++)*r->refs[i]=(struct drm_asahi_timestamp){.handle=t->object[i],.offset=t->at[i]-t->offset};}
static void stamp_seed_check(struct stamps*t){for(unsigned i=0;i<t->n;i++)for(size_t at=0;at<3*PAGE;at++)CHECK(t->map[i][at]==t->seed);}
static void stamp_check(struct stamps*t){
 for(unsigned i=0;i<t->n;i++){
  uint64_t value;memcpy(&value,t->map[i]+t->at[i],8);CHECK(value&&value!=0xa5a5a5a5a5a5a5a5ULL&&value!=0x3c3c3c3c3c3c3c3cULL&&value!=0x5a5a5a5a5a5a5a5aULL);
  if(t->value[i])CHECK(t->value[i]==value);else t->value[i]=value;
  for(size_t at=0;at<3*PAGE;at++)if(at<t->at[i]||at>=t->at[i]+8)CHECK(t->map[i][at]==t->seed);
 }
 CHECK(t->value[1]>t->value[0]);if(t->n==4)CHECK(t->value[3]>t->value[2]);
}
static void stamp_unbind(struct scene*r,struct stamps*t){
 for(unsigned i=0;i<t->n;i++){
  struct drm_asahi_gem_bind_object un={.op=DRM_ASAHI_BIND_OBJECT_OP_UNBIND,.object_handle=t->object[i]};OK(r->fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&un);t->object[i]=0;
  bo_close(r->fd,t->bo[i]);t->bo[i]=0;
 }
}
static void stamp_finish(struct scene*r,struct stamps*t){if(t->object[0])stamp_unbind(r,t);for(unsigned i=0;i<t->n;i++)CHECK(!munmap(t->map[i],3*PAGE));}
static uint64_t time_ns(void){struct timespec t;CHECK(!clock_gettime(CLOCK_MONOTONIC,&t));return(uint64_t)t.tv_sec*1000000000+t.tv_nsec;}
static uint32_t new_sync(int fd){struct drm_syncobj_create s={0};OK(fd,DRM_IOCTL_SYNCOBJ_CREATE,&s);return s.handle;}
static void invalid_ranges(struct scene*r,struct stamps*t){
 const uint64_t bad[][2]={{0,0},{1,8},{8,1},{3*PAGE,8},{3*PAGE-8,16},{UINT64_MAX-7,16}};
 for(unsigned i=0;i<sizeof(bad)/sizeof(*bad);i++){
  struct drm_asahi_gem_bind_object obj={.op=DRM_ASAHI_BIND_OBJECT_OP_BIND,.flags=DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,.handle=t->bo[0],.offset=bad[i][0],.range=bad[i][1]};BAD(r->fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&obj,EINVAL);
 }
 struct drm_asahi_timestamp saved=*r->refs[0];r->refs[0]->offset=8;BAD(r->fd,DRM_IOCTL_ASAHI_SUBMIT,&r->submit,EINVAL);
 r->refs[0]->offset=1;BAD(r->fd,DRM_IOCTL_ASAHI_SUBMIT,&r->submit,EINVAL);
 *r->refs[0]=saved;scene_check(r,0);stamp_seed_check(t);
 puts("G16C_TIMESTAMP_LOGICAL_REJECT_PASS bind_rejects=6 submit_rejects=2 all_seeds_intact=1");
}
static void pending_rebind(struct scene*r){
 struct stamps old={0},fresh={0},reuse={0};stamp_make(r,&old,PAGE-8,16,0xa5,1);stamp_select(r,&old);output_reset(r);
 CHECK(!mount("debugfs","/sys/kernel/debug","debugfs",0,NULL)||errno==EBUSY);
 uint32_t input=new_sync(r->fd),done=new_sync(r->fd);int sw=open("/sys/kernel/debug/sync/sw_sync",O_RDWR|O_CLOEXEC);CHECK(sw>=0);
 struct sw_create create={.value=1,.name="timestamp-range"};OK(sw,SW_CREATE_RANGE,&create);
 struct drm_syncobj_handle import={.handle=input,.fd=create.fence,.flags=DRM_SYNCOBJ_FD_TO_HANDLE_FLAGS_IMPORT_SYNC_FILE};OK(r->fd,DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE,&import);CHECK(!close(create.fence));
 struct drm_asahi_sync syncs[2]={{.handle=input},{.handle=done}};r->submit.syncs=(uintptr_t)syncs;r->submit.in_sync_count=r->submit.out_sync_count=1;
 CHECK(!g17p_raw_ioctl(r->fd,DRM_IOCTL_ASAHI_SUBMIT,&r->submit));
 struct drm_syncobj_wait wait={.handles=(uintptr_t)&done,.count_handles=1,.flags=DRM_SYNCOBJ_WAIT_FLAGS_WAIT_ALL,.timeout_nsec=time_ns()};BAD(r->fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait,ETIME);
 stamp_unbind(r,&old);stamp_make(r,&fresh,8,8,0x3c,0);usleep(20000);scene_check(r,0);stamp_seed_check(&old);stamp_seed_check(&fresh);
 uint32_t increment=1;OK(sw,SW_INC_RANGE,&increment);wait.timeout_nsec=time_ns()+15000000000ULL;OK(r->fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);
 scene_check(r,1);stamp_check(&old);stamp_seed_check(&fresh);
 r->submit.syncs=0;r->submit.in_sync_count=r->submit.out_sync_count=0;
 output_reset(r);stamp_select(r,&fresh);OK(r->fd,DRM_IOCTL_ASAHI_SUBMIT,&r->submit);scene_check(r,1);stamp_check(&fresh);stamp_check(&old);
 CHECK(fresh.value[0]>old.value[0]);
 stamp_make(r,&reuse,3*PAGE-8,8,0x5a,0);stamp_select(r,&reuse);output_reset(r);OK(r->fd,DRM_IOCTL_ASAHI_SUBMIT,&r->submit);
 scene_check(r,1);stamp_check(&reuse);stamp_check(&old);stamp_check(&fresh);CHECK(reuse.value[0]>fresh.value[0]);
 stamp_finish(r,&old);stamp_finish(r,&fresh);stamp_finish(r,&reuse);CHECK(!close(sw));
 struct drm_syncobj_destroy drop={.handle=input};OK(r->fd,DRM_IOCTL_SYNCOBJ_DESTROY,&drop);drop.handle=done;OK(r->fd,DRM_IOCTL_SYNCOBJ_DESTROY,&drop);
 printf("G16C_TIMESTAMP_PENDING_REBIND_PASS refs=%u jobs=3 input_blocked_verified=1 old_tokens_and_GEM_handles_closed=1 replacements_untouched_until_selected=1 old_words_and_guards_preserved=1\n",r->nrefs);
}
int main(int argc,char**argv){
 setbuf(stdout,NULL);CHECK(argc==2||(argc==3&&!strcmp(argv[2],"--file-close")));struct scene r={0};scene_open(&r,argv[1]);
 const uint64_t cases[][2]={{8,8},{PAGE-8,16},{PAGE+8,PAGE+16},{3*PAGE-8,8}};
 for(unsigned i=0;i<sizeof(cases)/sizeof(*cases);i++){
  struct stamps t={0};stamp_make(&r,&t,cases[i][0],cases[i][1],0xa5,i==1||i==2);stamp_select(&r,&t);output_reset(&r);
  if(!i)invalid_ranges(&r,&t);
  OK(r.fd,DRM_IOCTL_ASAHI_SUBMIT,&r.submit);scene_check(&r,1);stamp_check(&t);
  printf("G16C_TIMESTAMP_RANGE_PASS case=%u offset=%"PRIu64" range=%"PRIu64" refs=%u complete_BO_guards=%llu\n",i,t.offset,t.size,t.n,(unsigned long long)(t.n*(3*PAGE-8)));stamp_finish(&r,&t);
 }
 pending_rebind(&r);
 if(argc==2){struct drm_asahi_queue_destroy q={.queue_id=r.queue};OK(r.fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&q);for(unsigned id=0;id<128;id++)if(r.handles[id])bo_close(r.fd,r.handles[id]);vm_destroy(r.fd,r.vm,0);}
 CHECK(!close(r.fd));scene_check(&r,1);
 for(unsigned id=0;id<128;id++)if(r.maps[id]){CHECK(!munmap(r.maps[id],r.sizes[id]));free(r.seeds[id]);}
 free(r.fixture);
 printf("G16C_TIMESTAMP_RANGES_PASS engine=%s jobs=7 file_close=%u all_caller_BOs_and_timestamp_guards=1\n",r.nrefs==4?"render":"compute",argc==3);return 0;
}
