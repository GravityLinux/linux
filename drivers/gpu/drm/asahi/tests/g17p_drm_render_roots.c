/* SPDX-License-Identifier: MIT */
/* Same DVAs in different live VMs; full outputs, inactive guards, fences, timestamps. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include "g17p_drm_render_batch_workload.h"
#include "g17p_drm_sync.h"
struct client {int fd; uint32_t vm,queue,object,fence; uint32_t image_bos[16]; unsigned char *images[16];volatile uint64_t *times;};
static void finish(struct client *c)
{
 struct drm_syncobj_wait w={.handles=(uintptr_t)&c->fence,.count_handles=1,.timeout_nsec=now_ns()+15000000000ULL};OK(c->fd,DRM_IOCTL_SYNCOBJ_WAIT,&w);
 struct drm_syncobj_handle e={.handle=c->fence,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};OK(c->fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&e);
 struct sync_file_info info={0};OK(e.fd,SYNC_IOC_FILE_INFO,&info);CHECK(info.status==1);CHECK(close(e.fd)==0);
 struct drm_syncobj_destroy d={.handle=c->fence};OK(c->fd,DRM_IOCTL_SYNCOBJ_DESTROY,&d);
}
int main(int argc,char **argv)
{
 setbuf(stdout,NULL);unsigned rounds=argc>1?(unsigned)atoi(argv[1]):32;int strict=1,geometry=0,snapshots=0;for(int i=2;i<argc;i++){if(!strcmp(argv[i],"--allow-serial"))strict=0;else if(!strcmp(argv[i],"--geometry"))geometry=1;else if(!strcmp(argv[i],"--same-vm"))snapshots=1;else CHECK(0);}CHECK(rounds && rounds<=4096);
 struct client clients[2]={0};
 for(unsigned n=0;n<2;n++) {
  struct client *c=&clients[n];c->fd=snapshots && n?clients[0].fd:open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC);CHECK(c->fd>=0);c->vm=snapshots && n?clients[0].vm:vm_new(c->fd);
  struct drm_asahi_queue_create q={.vm_id=c->vm,.usc_exec_base=EXEC};OK(c->fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q);c->queue=q.queue_id;
  if(snapshots && n) memcpy(c->images,clients[0].images,sizeof(c->images));
  else for(unsigned i=0;i<sizeof(workloads)/sizeof(workloads[0]);i++) {
   const struct workload *w=&workloads[i];uint32_t bo=bo_new(c->fd,w->size,DRM_ASAHI_GEM_WRITEBACK,0);
   void *map=bo_map(c->fd,bo,w->size);memcpy(map,w->data,w->size);int keep=0;
   for(unsigned j=0;j<16;j++) if(w->address==render_outputs[j]) {c->images[j]=map;c->image_bos[j]=bo;keep=1;}
   if(!keep) CHECK(munmap(map,w->size)==0);
   bind(c->fd,c->vm,bo,w->address,w->size,0,DRM_ASAHI_BIND_READ|(w->writable?DRM_ASAHI_BIND_WRITE:0),0);
  }
  for(unsigned j=0;j<16;j++) CHECK(c->images[j]);
  uint32_t bo=bo_new(c->fd,PAGE*3,DRM_ASAHI_GEM_WRITEBACK,0);c->times=bo_map(c->fd,bo,PAGE*3);
  struct drm_asahi_gem_bind_object o={.op=DRM_ASAHI_BIND_OBJECT_OP_BIND,.flags=DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,.handle=bo,.offset=PAGE,.range=PAGE};OK(c->fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&o);c->object=o.object_handle;
 }
 unsigned overlaps=0;
 for(unsigned round=0;round<rounds;round++) {
  unsigned order[2]={snapshots?0:round%2,snapshots?1:1-round%2};
  for(unsigned n=0;n<2;n++) {
   memset((void *)clients[n].times,0xa5,PAGE*3);
   for(unsigned j=0;j<16;j++) memset(clients[n].images[j],0xa5,RENDER_OUTPUT_SIZE);
  }
  if(snapshots) for(unsigned j=8;j<16;j++) bind(clients[0].fd,clients[0].vm,0,render_outputs[j],RENDER_OUTPUT_SIZE,0,DRM_ASAHI_BIND_UNBIND,0);
  for(unsigned k=0;k<2;k++) {
   unsigned n=order[k];struct client *c=&clients[n];c->fence=sync_new(c->fd,0);
   union {uint64_t align;unsigned char bytes[sizeof(render_command)];} draw;
   memcpy(draw.bytes,n?render_second_command:render_command,sizeof(draw.bytes));
   struct drm_asahi_cmd_render *r=(void *)(draw.bytes+sizeof(draw.bytes)-sizeof(*r));struct drm_asahi_cmd_header *h=(void *)((unsigned char *)r-sizeof(*h));h->vdm_barrier=h->cdm_barrier=DRM_ASAHI_BARRIER_NONE;
   if(geometry && n) {r->width_px=127;r->utile_height_px=16;}
   if(snapshots && n) {
    // Admit the second render's original output bindings after the first
    // ioctl captured its snapshot. Both streams remain byte-for-byte unchanged.
    for(unsigned j=8;j<16;j++) bind(c->fd,c->vm,clients[0].image_bos[j],render_outputs[j],RENDER_OUTPUT_SIZE,0,DRM_ASAHI_BIND_READ|DRM_ASAHI_BIND_WRITE,0);
   }
   struct drm_asahi_timestamp *t[]={&r->ts_vtx.start,&r->ts_vtx.end,&r->ts_frag.start,&r->ts_frag.end};
   for(unsigned j=0;j<4;j++) *t[j]=(struct drm_asahi_timestamp){.handle=c->object,.offset=j*8};
   struct drm_asahi_sync out={.handle=c->fence};struct drm_asahi_submit s={.queue_id=c->queue,.cmdbuf=(uintptr_t)draw.bytes,.cmdbuf_size=sizeof(draw.bytes),.syncs=(uintptr_t)&out,.out_sync_count=1};CHECK(g17p_raw_ioctl(c->fd,DRM_IOCTL_ASAHI_SUBMIT,&s)==0);memset(draw.bytes,0xa5,sizeof(draw.bytes));
  }
  finish(&clients[order[1]]);finish(&clients[order[0]]);
  for(unsigned n=0;n<2;n++) {
   struct client *c=&clients[n];
   for(unsigned target=0;target<16;target++) {
    unsigned char expected[4];float value=((snapshots?target/8:n)?RENDER_SECOND_TRIANGLES:RENDER_TRIANGLES)*((target%8+1)/8.0f);memcpy(expected,&value,4);
    for(unsigned byte=0;byte<RENDER_OUTPUT_SIZE;byte++) {
     unsigned char want=!snapshots && target/8!=n?0xa5:0;
     if((snapshots || target/8==n) && byte>=RENDER_PIXEL_0 && byte<RENDER_PIXEL_0+4) want=expected[byte-RENDER_PIXEL_0];
     if((snapshots || target/8==n) && byte>=RENDER_PIXEL_1 && byte<RENDER_PIXEL_1+4) want=expected[byte-RENDER_PIXEL_1];
     if(c->images[target][byte]!=want) printf("ROOT_IMAGE_FAIL round=%u vm=%u target=%u byte=%u actual=%02x expected=%02x\n",round,n,target,byte,c->images[target][byte],want);
     CHECK(c->images[target][byte]==want);
    }
   }
   uint64_t a=c->times[PAGE/8],b=c->times[PAGE/8+1],d=c->times[PAGE/8+2],e=c->times[PAGE/8+3];CHECK(a && a!=0xa5a5a5a5a5a5a5a5ULL && b>a && d && d!=0xa5a5a5a5a5a5a5a5ULL && e>d);
   for(unsigned byte=0;byte<PAGE*3;byte++) if(byte<PAGE || byte>=PAGE+32) CHECK(((volatile unsigned char *)c->times)[byte]==0xa5);
  }
  uint64_t *a=(void *)(clients[order[0]].times+PAGE/8),*b=(void *)(clients[order[1]].times+PAGE/8);int overlap=b[0]<a[3] && b[3]>a[0];overlaps+=overlap;
  printf("RENDER_ROOTS_WAVE round=%u first_vm=%u overlap=%d A=%llu/%llu/%llu/%llu B=%llu/%llu/%llu/%llu\n",round,order[0],overlap,(unsigned long long)a[0],(unsigned long long)a[1],(unsigned long long)a[2],(unsigned long long)a[3],(unsigned long long)b[0],(unsigned long long)b[1],(unsigned long long)b[2],(unsigned long long)b[3]);
  if(strict && round>=15) CHECK(overlap);
 }
 printf("RENDER_ROOTS_UAPI_PASS rounds=%u overlaps=%u images=%u checks=%u snapshots=%d geometry=%d\n",rounds,overlaps,rounds*32,checks,snapshots,geometry);if(strict) CHECK(overlaps>0);
 for(unsigned n=0;n<(snapshots?1U:2U);n++) CHECK(close(clients[n].fd)==0);
 return 0;
}
