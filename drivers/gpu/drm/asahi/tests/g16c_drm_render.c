/* SPDX-License-Identifier: GPL-2.0-only */
/* Native Max render output/guard checks using our source-owned Mesa GEMs. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include <linux/sync_file.h>
static void *load(const char *folder,const char *name,size_t *size) {
 char path[512];CHECK(snprintf(path,sizeof(path),"%s/%s",folder,name)>0);
 FILE*f=fopen(path,"rb");CHECK(f!=NULL);CHECK(fseek(f,0,SEEK_END)==0);long n=ftell(f);CHECK(n>0);rewind(f);
 void*p=malloc(n);CHECK(p!=NULL);CHECK(fread(p,1,n,f)==(size_t)n);CHECK(fclose(f)==0);*size=n;return p;
}
static unsigned char *expected_image(const char *folder,const char *prefix,unsigned id,size_t size){
 char name[64],path[512];snprintf(name,sizeof(name),"%s-gem-%u.bin",prefix,id);
 CHECK(snprintf(path,sizeof(path),"%s/%s",folder,name)>0);
 if(access(path,F_OK)){CHECK(errno==ENOENT);return NULL;}
 size_t n;unsigned char*image=load(folder,name,&n);CHECK(n==size);return image;
}
static void save_phase(unsigned phase,unsigned id,const unsigned char*bytes,size_t size){
 char name[96];snprintf(name,sizeof(name),"/g16c-readback-%u-gem-%u.bin",phase,id);FILE*f=fopen(name,"wb");CHECK(f);
 CHECK(fwrite(bytes,1,size,f)==size);CHECK(!fclose(f));
 CHECK(size>=4);printf("G16C_PHASE_READBACK phase=%u id=%u bytes=%zu first=%02x%02x%02x%02x\n",phase,id,size,bytes[0],bytes[1],bytes[2],bytes[3]);
}
/* Explicit version4/6 compression regions: exact guards, opaque payload, plus
 * a separately checked CPU color oracle after the following load consumer.
 * These masks must never weaken primary color or ordinary fixture checking. */
static void check_input(unsigned id,const unsigned char*actual,const unsigned char*expected,const unsigned char*seed,const unsigned char*mask,size_t size){
 if(!mask){CHECK(!memcmp(actual,expected?expected:seed,size));return;}
 CHECK(expected);size_t payload=0,changed=0;
 for(size_t i=0;i<size;i++){
  CHECK(mask[i]<=1);
  if(mask[i]){payload++;changed+=actual[i]!=seed[i];}
  else {if(actual[i]!=expected[i])fprintf(stderr,"COMPRESSION_GUARD_MISMATCH id=%u offset=%zu actual=%02x expected=%02x\n",id,i,actual[i],expected[i]);CHECK(actual[i]==expected[i]);}
 }
 CHECK(payload&&payload<size&&changed);
 printf("G16C_COMPRESSION_REGIONS_PASS id=%u opaque_bytes=%zu exact_guard_bytes=%zu changed_from_seed=%zu\n",id,payload,size-payload,changed);
}
/* Exported old outputs stay alive and are read-only bindings in each new VM.
 * Native coherent caller-buffer ownership/cache validation includes the poison;
 * a stale GPU translation cannot hide behind reuse of a freed physical page. */
struct retired_render { unsigned char *output,*timestamps;size_t output_size;int output_fd,timestamp_fd; };
static struct retired_render retired[128];
static unsigned lifetime,lifetimes=1,draws_per_lifetime=2;
static int retired_map_fd=-1;
static int export_bo(int fd,uint32_t handle){
 struct drm_prime_handle p={.handle=handle,.flags=DRM_CLOEXEC|DRM_RDWR};OK(fd,DRM_IOCTL_PRIME_HANDLE_TO_FD,&p);return p.fd;
}
static void check_retired(void){
 static unsigned char poison[3*PAGE];memset(poison,0x3c,sizeof(poison));
 for(unsigned i=0;i<lifetime;i++){
  CHECK(!memcmp(retired[i].output,poison,retired[i].output_size));
  CHECK(!memcmp(retired[i].timestamps,poison,3*PAGE));
 }
}
static int run_lifetime(int argc,char**argv){
 setbuf(stdout,NULL);CHECK(argc>=2);int file_close=0,hold=0,check_inputs=0,reject_flags=0,expected_images=0,phase_pair=0,phase_triplet=0,expect_private_space=0,expect_alloc_retry=0;uint64_t kernel_base=0x7000000000ULL,kernel_size=0x20000000ULL;
 uint32_t pixel=0xff0000ff;
 for(int i=2;i<argc;i++){
  if(!strcmp(argv[i],"--file-close"))file_close=1;
  else if(!strcmp(argv[i],"--hold"))hold=1;
  else if(!strcmp(argv[i],"--expect-private-space"))expect_private_space=check_inputs=1;
  else if(!strcmp(argv[i],"--expect-allocation-retry"))expect_alloc_retry=check_inputs=1;
  else if(!strcmp(argv[i],"--check-inputs"))check_inputs=1;
  else if(!strcmp(argv[i],"--reject-flags"))reject_flags=1;
  else if(!strcmp(argv[i],"--expected-images"))expected_images=check_inputs=1;
  else if(!strcmp(argv[i],"--phase-pair"))phase_pair=expected_images=check_inputs=1;
  else if(!strcmp(argv[i],"--control-triplet")){phase_triplet=phase_pair=expected_images=check_inputs=1;draws_per_lifetime=3;}
  else if(!strncmp(argv[i],"--pixel=",8)){char*end;unsigned long n=strtoul(argv[i]+8,&end,0);CHECK(*end==0&&n<=UINT32_MAX);pixel=n;}
  else if(!strncmp(argv[i],"--kernel-size=",14)){char*end;errno=0;kernel_size=strtoull(argv[i]+14,&end,0);CHECK(!errno&&*end==0&&kernel_size>=0x20000000ULL&&!(kernel_size&(PAGE-1)));}
  else if(!strncmp(argv[i],"--kernel-base=",14)){char*end;kernel_base=strtoull(argv[i]+14,&end,0);CHECK(*end==0&&kernel_base&&!(kernel_base&(PAGE-1)));}
  else if(!strncmp(argv[i],"--lifetimes=",12)){}
  else CHECK(0);
 }
 size_t size;unsigned char*fixture=load(argv[1],"inputs.bin",&size);
 CHECK(!phase_pair||!hold);
 CHECK(!(expect_private_space&&expect_alloc_retry));
 CHECK(!(expect_private_space||expect_alloc_retry)||(lifetimes==1&&!file_close&&!hold&&!reject_flags));
 uint32_t h[8];CHECK(size>=32);memcpy(h,fixture,32);CHECK(h[0]==0x3242434d&&(h[1]==1||(h[1]==2&&expected_images)||((h[1]==3||h[1]==4||h[1]==6)&&phase_pair)||(h[1]==5&&phase_triplet))&&h[2]<128&&h[3]<4096);
 CHECK(!phase_triplet||h[1]==5);
 CHECK(size==32+h[2]*16+h[3]*sizeof(struct drm_asahi_gem_bind_op)+h[4]);
 int fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC);CHECK(fd>=0);
 if(lifetimes>1){const uint64_t bases[]={0x7000000000ULL,0x7100004000ULL,0x7200000000ULL};kernel_base=bases[lifetime%3];file_close=lifetime%2;}
 printf("G16C_RENDER_LIFETIME_BEGIN index=%u total=%u kernel_base=%#"PRIx64" file_close=%d\n",lifetime,lifetimes,kernel_base,file_close);
 CHECK(kernel_size<=(1ULL<<42)-kernel_base);
 struct drm_asahi_vm_create vm={.kernel_start=kernel_base,.kernel_end=kernel_base+kernel_size};OK(fd,DRM_IOCTL_ASAHI_VM_CREATE,&vm);
 struct drm_asahi_queue_create q={.vm_id=vm.vm_id,.usc_exec_base=EXEC};OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q);
 uint32_t handles[128]={0};unsigned char*output=NULL,*seed=NULL;size_t output_size=0;
 unsigned char*input_maps[128]={0},*input_seeds[128]={0},*input_expected[128]={0},*input_masks[128]={0},*output_expected=NULL;size_t input_sizes[128]={0};
 unsigned expected_mutable=0,opaque_bos=0;
 for(unsigned i=0;i<h[2];i++){
  uint32_t id,flags;uint64_t length;memcpy(&id,fixture+32+i*16,4);memcpy(&flags,fixture+36+i*16,4);memcpy(&length,fixture+40+i*16,8);CHECK(id<128&&!handles[id]);
  char name[64];snprintf(name,sizeof(name),"gem-%u.bin",id);size_t n;void*data=load(argv[1],name,&n);CHECK(n==length);
  if(lifetimes>1&&id==h[5])flags=DRM_ASAHI_GEM_WRITEBACK;
  handles[id]=bo_new(fd,n,flags,(flags&DRM_ASAHI_GEM_VM_PRIVATE)?vm.vm_id:0);
  unsigned char*map=bo_map(fd,handles[id],n);memcpy(map,data,n);
  unsigned char*expected=expected_images?expected_image(argv[1],"expected",id,n):NULL;
  unsigned char*mask=expected_image(argv[1],"opaque-writes",id,n);
  if(mask){CHECK((h[1]==4||h[1]==6)&&phase_pair&&id!=h[5]&&expected);input_masks[id]=mask;opaque_bos++;}
  if(expected)expected_mutable++;
  if(id==h[5]){output=map;seed=data;output_size=n;output_expected=expected;}
  else if(check_inputs){input_maps[id]=map;input_seeds[id]=data;input_sizes[id]=n;input_expected[id]=expected;}
  else{CHECK(munmap(map,n)==0);free(data);}
 }
 CHECK(output&&(uint64_t)h[6]+h[7]<=output_size&&h[7]>0&&!(h[7]%4));
 CHECK(!expected_images||output_expected);
 CHECK(h[1]!=4||opaque_bos==2);
 CHECK(h[1]!=6||opaque_bos==4);
 struct drm_asahi_gem_bind_op*ops=(void*)(fixture+32+h[2]*16);
 for(unsigned i=0;i<h[3];i++){CHECK(ops[i].handle<128&&handles[ops[i].handle]);ops[i].handle=handles[ops[i].handle];}
 struct drm_asahi_vm_bind fixture_bind={.vm_id=vm.vm_id,.num_binds=h[3],.stride=sizeof(*ops),.userptr=(uintptr_t)ops};OK(fd,DRM_IOCTL_ASAHI_VM_BIND,&fixture_bind);
 uint32_t imported_handles[256]={0};unsigned imported_count=0;
 for(unsigned i=0;i<lifetime;i++)for(unsigned kind=0;kind<2;kind++){
  struct drm_prime_handle import={.fd=kind?retired[i].timestamp_fd:retired[i].output_fd};OK(fd,DRM_IOCTL_PRIME_FD_TO_HANDLE,&import);
  imported_handles[imported_count++]=import.handle;
  bind(fd,vm.vm_id,import.handle,0x6002000000ULL+i*0x40000ULL+kind*0x10000ULL,
   kind?3*PAGE:retired[i].output_size,0,DRM_ASAHI_BIND_READ,0);
 }
 struct drm_asahi_submit submit={.queue_id=q.queue_id,.cmdbuf_size=h[4],.cmdbuf=(uintptr_t)(fixture+32+h[2]*16+h[3]*sizeof(*ops))};
 CHECK(h[4]>=sizeof(struct drm_asahi_cmd_render));
 struct drm_asahi_cmd_render*cmd=(void*)((unsigned char*)(uintptr_t)submit.cmdbuf+h[4]-sizeof(*cmd));
 uint32_t ts_bo=bo_new(fd,3*PAGE,DRM_ASAHI_GEM_WRITEBACK,0);
 unsigned char*ts=bo_map(fd,ts_bo,3*PAGE);memset(ts,0xa5,3*PAGE);
 struct drm_asahi_gem_bind_object obj={.op=DRM_ASAHI_BIND_OBJECT_OP_BIND,.flags=DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,
  .handle=ts_bo,.offset=PAGE,.range=PAGE};OK(fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&obj);
 struct drm_asahi_timestamp*refs[]={&cmd->ts_vtx.start,&cmd->ts_vtx.end,&cmd->ts_frag.start,&cmd->ts_frag.end};
 uint64_t history[3][4]={{0}};
 if(reject_flags){
  uint32_t flags=cmd->flags;cmd->flags=flags|(1<<4);BAD(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit,EINVAL);cmd->flags=flags;
  CHECK(!memcmp(output,seed,output_size));for(size_t i=0;i<3*PAGE;i++)CHECK(ts[i]==0xa5);
  for(unsigned id=0;id<128;id++)if(input_maps[id])CHECK(!memcmp(input_maps[id],input_seeds[id],input_sizes[id]));
  puts("G16C_RENDER_RESERVED_FLAG_PASS rejected_before_any_output_or_input_change=1");
 }
 if(expect_private_space||expect_alloc_retry){
  for(unsigned i=0;i<4;i++){refs[i]->handle=obj.object_handle;refs[i]->offset=64+i*8;}
  struct drm_syncobj_create done={0};OK(fd,DRM_IOCTL_SYNCOBJ_CREATE,&done);
  struct drm_asahi_sync sync={.handle=done.handle};submit.syncs=(uintptr_t)&sync;submit.out_sync_count=1;
  CHECK(!g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit));
  struct timespec now;CHECK(!clock_gettime(CLOCK_MONOTONIC,&now));
  struct drm_syncobj_wait wait={.handles=(uintptr_t)&done.handle,.count_handles=1,
   .timeout_nsec=(int64_t)now.tv_sec*1000000000+now.tv_nsec+15000000000LL,.flags=DRM_SYNCOBJ_WAIT_FLAGS_WAIT_ALL};
  OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);
  struct drm_syncobj_handle export={.handle=done.handle,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};OK(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&export);
  struct sync_file_info info={0};CHECK(!ioctl(export.fd,SYNC_IOC_FILE_INFO,&info)&&info.status==-(expect_alloc_retry?ENOMEM:ENOSPC));CHECK(!close(export.fd));
  CHECK(!memcmp(output,seed,output_size));unsigned bos=1;
  for(unsigned id=0;id<128;id++)if(input_maps[id]){CHECK(!memcmp(input_maps[id],input_seeds[id],input_sizes[id]));bos++;}
  for(size_t i=0;i<3*PAGE;i++)CHECK(ts[i]==0xa5);
  CHECK(bos==h[2]);struct drm_syncobj_destroy drop={.handle=done.handle};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&drop);
  if(expect_alloc_retry){
   printf("G16C_RENDER_INITIAL_ALLOCATION_PASS binary_fence=-12 all_BO_bytes_untouched=%u all_timestamp_bytes_untouched=%llu same_VM_queue_retry=1\n",bos,3*PAGE);
   submit.syncs=0;submit.out_sync_count=0;
  }else{
   printf("G16C_RENDER_PRIVATE_SPACE_PASS binary_fence=-28 all_BO_bytes_untouched=%u all_timestamp_bytes_untouched=%llu\n",bos,3*PAGE);
   goto cleanup_render;
  }
 }
 printf("G16C_NATIVE_RENDER_BEGIN output_bytes=%u guards=%zu\n",h[7],output_size-h[7]);
 for(unsigned iteration=0;iteration<draws_per_lifetime;iteration++){
  if(phase_pair&&iteration){
   char name[64],prefix[32];snprintf(name,sizeof(name),"phase%u-command.bin",iteration);snprintf(prefix,sizeof(prefix),"phase%u-expected",iteration);
   size_t n;unsigned char*next=load(argv[1],name,&n);CHECK(n==h[4]);memcpy((void*)(uintptr_t)submit.cmdbuf,next,n);free(next);
   free(output_expected);output_expected=expected_image(argv[1],prefix,h[5],output_size);CHECK(output_expected);expected_mutable=1;
   for(unsigned id=0;id<128;id++)if(input_maps[id]){
    free(input_expected[id]);input_expected[id]=expected_image(argv[1],prefix,id,input_sizes[id]);if(input_expected[id])expected_mutable++;
    char input_prefix[32];snprintf(input_prefix,sizeof(input_prefix),"phase%u-input",iteration);
    unsigned char*update=expected_image(argv[1],input_prefix,id,input_sizes[id]);
    if(update){CHECK(!input_masks[id]);memcpy(input_maps[id],update,input_sizes[id]);memcpy(input_seeds[id],update,input_sizes[id]);free(update);}
   }
  }
  for(unsigned i=0;i<4;i++){refs[i]->handle=obj.object_handle;refs[i]->offset=64+iteration*32+i*8;}
  if(!phase_pair||!iteration||phase_triplet){memcpy(output,seed,output_size);for(unsigned id=0;id<128;id++)if(input_expected[id])memcpy(input_maps[id],input_seeds[id],input_sizes[id]);}
  if(phase_triplet)printf("G16C_RENDER_CONTROL_BEGIN iteration=%u flags=%u reset_output_seed=1\n",iteration,cmd->flags);
  unsigned char*before=malloc(output_size);CHECK(before);memcpy(before,output,output_size);
  unsigned char*before_input[128]={0};for(unsigned id=0;id<128;id++)if(input_expected[id]){before_input[id]=malloc(input_sizes[id]);CHECK(before_input[id]);memcpy(before_input[id],input_maps[id],input_sizes[id]);}
  OK(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit);unsigned changed=0;
  if(phase_pair){save_phase(iteration,h[5],output,output_size);for(unsigned id=0;id<128;id++)if(input_expected[id])save_phase(iteration,id,input_maps[id],input_sizes[id]);}
  memcpy(history[iteration],ts+PAGE+64+iteration*32,32);uint64_t*stamps=history[iteration];
  printf("G16C_RENDER_TIMESTAMPS iteration=%u TA=%"PRIu64",%"PRIu64" FR=%"PRIu64",%"PRIu64"\n",iteration,stamps[0],stamps[1],stamps[2],stamps[3]);
  CHECK(stamps[0]&&stamps[1]>stamps[0]&&stamps[2]&&stamps[3]>stamps[2]);
  if(iteration)CHECK(stamps[0]>history[iteration-1][0]&&stamps[2]>history[iteration-1][2]);
  CHECK(!memcmp(history,ts+PAGE+64,(iteration+1)*32));
  for(size_t i=0;i<3*PAGE;i++)if(i<PAGE+64||i>=PAGE+64+(iteration+1)*32)CHECK(ts[i]==0xa5);
  for(size_t i=0;i<output_size;i++){unsigned char want=seed[i];if(i>=h[6]&&i<h[6]+h[7])want=(pixel>>(((i-h[6])%4)*8))&255;if(output_expected)want=output_expected[i];if(output[i]!=want)fprintf(stderr,"OUTPUT_MISMATCH offset=%zu actual=%02x expected=%02x\n",i,output[i],want);CHECK(output[i]==want);if(output[i]!=before[i])changed++;}
  unsigned changed_other=0,preserved=0;free(before);for(unsigned id=0;id<128;id++)if(before_input[id]){
   if(memcmp(before_input[id],input_maps[id],input_sizes[id]))changed_other++;
   if(h[1]==6&&iteration==1){
    unsigned char*keep=expected_image(argv[1],"phase1-preserve",id,1);
    if(keep){CHECK(*keep==1&&input_masks[id]);CHECK(!memcmp(before_input[id],input_maps[id],input_sizes[id]));free(keep);preserved++;printf("G16C_COMPRESSED_INPUT_PRESERVED id=%u bytes=%zu\n",id,input_sizes[id]);}
   }
   free(before_input[id]);
  }
  CHECK(h[1]!=6||iteration!=1||preserved==2);
  if(check_inputs){unsigned count=0;for(unsigned id=0;id<128;id++)if(input_maps[id]){check_input(id,input_maps[id],input_expected[id],input_seeds[id],input_masks[id],input_sizes[id]);count++;}if(expected_images)printf("G16C_RENDER_EXPECTATIONS_PASS iteration=%u checked_BOs=%u expected_mutable_BOs=%u\n",iteration,count+1,expected_mutable);else printf("G16C_RENDER_INPUTS_PASS iteration=%u unchanged_BOs=%u pixel=%#x\n",iteration,count,pixel);}
  check_retired();
  CHECK(changed||changed_other||(output_expected&&!memcmp(output_expected,seed,output_size)));printf("G16C_NATIVE_RENDER_OUTPUT iteration=%u changed_bytes=%u whole_output_and_guards=%zu\n",iteration,changed,output_size);
  if(phase_pair)printf("G16C_RENDER_PHASE_PASS iteration=%u preserve_prior_GPU_data=%u changed_other_BOs=%u\n",iteration,!phase_triplet&&iteration!=0,changed_other);
 }
 if(hold){
  CHECK(lifetimes==1);puts("G16C_RENDER_HOLD_READY");
  for(;;){int ch=getchar();CHECK(ch!=EOF);if(ch=='Q')break;if(ch!='R')continue;
   for(size_t i=0;i<output_size;i++){unsigned char want=seed[i];if(i>=h[6]&&i<h[6]+h[7])want=(pixel>>(((i-h[6])%4)*8))&255;if(output_expected)want=output_expected[i];CHECK(output[i]==want);}
   for(unsigned id=0;id<128;id++)if(input_maps[id])CHECK(!memcmp(input_maps[id],input_expected[id]?input_expected[id]:input_seeds[id],input_sizes[id]));
   memcpy(output,seed,output_size);for(unsigned id=0;id<128;id++)if(input_expected[id])memcpy(input_maps[id],input_seeds[id],input_sizes[id]);
   memset(ts+PAGE+96,0,32);OK(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit);
   uint64_t stamps[4];memcpy(stamps,ts+PAGE+96,32);CHECK(stamps[0]&&stamps[1]>stamps[0]&&stamps[2]&&stamps[3]>stamps[2]);
   CHECK(stamps[0]>history[1][0]&&stamps[2]>history[1][2]);memcpy(history[1],stamps,32);CHECK(!memcmp(history,ts+PAGE+64,64));
   for(size_t i=0;i<3*PAGE;i++)if(i<PAGE+64||i>=PAGE+128)CHECK(ts[i]==0xa5);
   for(size_t i=0;i<output_size;i++){unsigned char want=seed[i];if(i>=h[6]&&i<h[6]+h[7])want=(pixel>>(((i-h[6])%4)*8))&255;if(output_expected)want=output_expected[i];CHECK(output[i]==want);}
   for(unsigned id=0;id<128;id++)if(input_maps[id])CHECK(!memcmp(input_maps[id],input_expected[id]?input_expected[id]:input_seeds[id],input_sizes[id]));
   puts("G16C_RENDER_HOLD_OUTPUT_PASS");
  }
 }
 printf("G16C_NATIVE_RENDER_PASS draws=%u\n",draws_per_lifetime);
cleanup_render:
 free(output_expected);for(unsigned id=0;id<128;id++)if(input_maps[id]){CHECK(munmap(input_maps[id],input_sizes[id])==0);free(input_seeds[id]);free(input_expected[id]);free(input_masks[id]);}
 if(lifetimes>1){
  CHECK(output_size<=3*PAGE);int output_fd=export_bo(fd,handles[h[5]]),timestamp_fd=export_bo(fd,ts_bo);
  struct drm_prime_handle a={.fd=output_fd},b={.fd=timestamp_fd};
  OK(retired_map_fd,DRM_IOCTL_PRIME_FD_TO_HANDLE,&a);OK(retired_map_fd,DRM_IOCTL_PRIME_FD_TO_HANDLE,&b);
  unsigned char*held_output=bo_map(retired_map_fd,a.handle,output_size),*held_ts=bo_map(retired_map_fd,b.handle,3*PAGE);
  bo_close(retired_map_fd,a.handle);bo_close(retired_map_fd,b.handle);
  CHECK(munmap(output,output_size)==0);CHECK(munmap(ts,3*PAGE)==0);output=held_output;ts=held_ts;
  retired[lifetime]=(struct retired_render){.output=output,.timestamps=ts,.output_size=output_size,.output_fd=output_fd,.timestamp_fd=timestamp_fd};
 }
 if(file_close){
  CHECK(close(fd)==0);
  if(lifetimes==1){CHECK(munmap(output,output_size)==0);CHECK(munmap(ts,3*PAGE)==0);}
  else {memset(output,0x3c,output_size);memset(ts,0x3c,3*PAGE);}
  free(seed);free(fixture);puts("G16C_NATIVE_RENDER_FILE_CLOSE_PASS");return 0;
 }
 struct drm_asahi_queue_destroy qd={.queue_id=q.queue_id};OK(fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&qd);
 struct drm_asahi_gem_bind_object unobj={.op=DRM_ASAHI_BIND_OBJECT_OP_UNBIND,.object_handle=obj.object_handle};OK(fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&unobj);
 if(lifetimes==1)CHECK(munmap(ts,3*PAGE)==0);
 bo_close(fd,ts_bo);
 for(unsigned id=0;id<128;id++)if(handles[id])bo_close(fd,handles[id]);
 for(unsigned i=0;i<imported_count;i++)bo_close(fd,imported_handles[i]);
 if(lifetimes==1)CHECK(munmap(output,output_size)==0);
 free(seed);free(fixture);
 struct drm_asahi_vm_destroy vd={.vm_id=vm.vm_id};OK(fd,DRM_IOCTL_ASAHI_VM_DESTROY,&vd);puts("G16C_NATIVE_RENDER_CLOSE_PASS");CHECK(close(fd)==0);
 if(lifetimes>1){memset(output,0x3c,output_size);memset(ts,0x3c,3*PAGE);}return 0;
}

int main(int argc,char**argv){
 for(int i=2;i<argc;i++)if(!strncmp(argv[i],"--lifetimes=",12)){
  char*end;unsigned long n=strtoul(argv[i]+12,&end,0);CHECK(*end==0&&n>0&&n<=128);lifetimes=n;
 }
 if(lifetimes>1){retired_map_fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC);CHECK(retired_map_fd>=0);}
 for(lifetime=0;lifetime<lifetimes;lifetime++)CHECK(run_lifetime(argc,argv)==0);
 if(lifetimes>1){
  check_retired();
  for(unsigned i=0;i<lifetimes;i++){
   CHECK(munmap(retired[i].output,retired[i].output_size)==0);CHECK(munmap(retired[i].timestamps,3*PAGE)==0);
   CHECK(close(retired[i].output_fd)==0);CHECK(close(retired[i].timestamp_fd)==0);
  }
  CHECK(close(retired_map_fd)==0);
  printf("G16C_RENDER_LIFETIME_PASS lifetimes=%u draws=%u prior_outputs_and_timestamps_poisoned_and_cache_checked=1\n",lifetimes,lifetimes*draws_per_lifetime);
 }
 return 0;
}
