/* SPDX-License-Identifier: MIT */
/* Real UAPI: three independently owned compute queues during repeated long renders. */
#define main memory_test_main
#include "g17p_drm_memory.c"
#undef main
#include "g17p_drm_retained_wave_workload.h"
#define workload render_workload
#define workloads render_workloads
#include "g17p_drm_render_batch_workload.h"
#undef workloads
#undef workload
#include "g17p_drm_sync.h"
#include <linux/sync_file.h>
#ifndef CLIENTS
#define CLIENTS 3
#endif
#define JOBS 56
struct command {struct drm_asahi_cmd_header header;struct drm_asahi_cmd_compute compute;};
struct client {uint32_t vm,queue,stamp,object,fence;float *input[JOBS],*output[JOBS];volatile uint64_t *times;struct command commands[JOBS];};

static void finish(int fd,uint32_t fence)
{
 struct drm_syncobj_wait wait={.handles=(uintptr_t)&fence,.count_handles=1,.timeout_nsec=now_ns()+15000000000ULL};
 OK(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);
 struct drm_syncobj_handle exported={.handle=fence,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
 OK(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&exported);
 struct sync_file_info info={0};OK(exported.fd,SYNC_IOC_FILE_INFO,&info);CHECK(info.status==1);CHECK(close(exported.fd)==0);
 struct drm_syncobj_destroy d={.handle=fence};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&d);
}
int main(int argc,char **argv)
{
 setbuf(stdout,NULL);unsigned rounds=argc==2?(unsigned)atoi(argv[1]):25;CHECK(rounds>0&&rounds<=4096);
 CHECK(mount("debugfs","/sys/kernel/debug","debugfs",0,NULL)==0 || errno==EBUSY);
 int fd=open("/dev/dri/renderD128",O_RDWR|O_CLOEXEC);CHECK(fd>=0);
 struct client clients[CLIENTS]={0};
 uint32_t render_vm=vm_new(fd);struct drm_asahi_queue_create rq={.vm_id=render_vm,.usc_exec_base=EXEC};OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&rq);
 unsigned char *render_maps[8]={0};
 for(unsigned i=0;i<sizeof(render_workloads)/sizeof(render_workloads[0]);i++) {
  const struct render_workload *w=&render_workloads[i];uint32_t bo=bo_new(fd,w->size,DRM_ASAHI_GEM_WRITEBACK,0);
  void *map=bo_map(fd,bo,w->size);memcpy(map,w->data,w->size);int keep=0;
  for(unsigned j=0;j<8;j++) if(w->address==render_outputs[j]) {render_maps[j]=map;keep=1;memset(map,0xa5,RENDER_OUTPUT_SIZE);}
  if(!keep) CHECK(munmap(map,w->size)==0);
  bind(fd,render_vm,bo,w->address,w->size,0,DRM_ASAHI_BIND_READ|(w->writable?DRM_ASAHI_BIND_WRITE:0),0);
 }
 for(unsigned j=0;j<8;j++) CHECK(render_maps[j]);
 uint32_t render_fence=0;
 uint32_t render_stamp=bo_new(fd,PAGE*3,0,0);
 volatile uint64_t *render_times=bo_map(fd,render_stamp,PAGE*3);
 struct drm_asahi_gem_bind_object ro={.op=DRM_ASAHI_BIND_OBJECT_OP_BIND,.flags=DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,.handle=render_stamp,.offset=PAGE,.range=PAGE};
 OK(fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&ro);

 CHECK(sizeof(batch_workloads)/sizeof(batch_workloads[0])==JOBS);
 for(unsigned n=0;n<CLIENTS;n++) {
  struct client *c=&clients[n];
  struct drm_asahi_vm_create v={.kernel_start=KSTART+n*0x40000000ULL,.kernel_end=KSTART+n*0x40000000ULL+0x20000000};
  OK(fd,DRM_IOCTL_ASAHI_VM_CREATE,&v);c->vm=v.vm_id;
  struct drm_asahi_queue_create q={.vm_id=c->vm,.usc_exec_base=EXEC};OK(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&q);c->queue=q.queue_id;
  unsigned executable=0;
  for(unsigned i=0;i<sizeof(render_workloads)/sizeof(render_workloads[0]);i++) {
   const struct render_workload *w=&render_workloads[i];if(w->address!=EXEC) continue;
   uint32_t bo=bo_new(fd,w->size,DRM_ASAHI_GEM_WRITEBACK,0);void *map=bo_map(fd,bo,w->size);
   memcpy(map,w->data,w->size);CHECK(munmap(map,w->size)==0);
   bind(fd,c->vm,bo,w->address,w->size,0,DRM_ASAHI_BIND_READ,0);executable++;
  }
  CHECK(executable==1);
  for(unsigned i=0;i<sizeof(workloads)/sizeof(workloads[0]);i++) {
   const struct workload *w=&workloads[i];uint32_t bo=bo_new(fd,w->size,DRM_ASAHI_GEM_WRITEBACK,0);
   void *map=bo_map(fd,bo,w->size);memcpy(map,w->data,w->size);int keep=0;
   for(unsigned j=0;j<JOBS;j++) {
    if(w->address==batch_workloads[j].input_a) {c->input[j]=map;keep=1;}
    if(w->address==batch_workloads[j].output) {c->output[j]=map;keep=1;}
   }
   if(!keep) CHECK(munmap(map,w->size)==0);
   bind(fd,c->vm,bo,w->address,w->size,0,DRM_ASAHI_BIND_READ|(w->writable?DRM_ASAHI_BIND_WRITE:0),0);
  }
  c->stamp=bo_new(fd,PAGE*3,DRM_ASAHI_GEM_WRITEBACK,0);c->times=bo_map(fd,c->stamp,PAGE*3);
  struct drm_asahi_gem_bind_object object={.op=DRM_ASAHI_BIND_OBJECT_OP_BIND,.flags=DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,.handle=c->stamp,.offset=PAGE,.range=PAGE};
  OK(fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&object);c->object=object.object_handle;
  for(unsigned j=0;j<JOBS;j++) {
   CHECK(c->input[j]&&c->output[j]);const struct batch_workload *w=&batch_workloads[j];
   c->commands[j]=(struct command){.header={.cmd_type=DRM_ASAHI_CMD_COMPUTE,.size=sizeof(struct drm_asahi_cmd_compute),.vdm_barrier=DRM_ASAHI_BARRIER_NONE,.cdm_barrier=j},
    .compute={.cdm_ctrl_stream_base=w->cdm,.cdm_ctrl_stream_end=w->cdm+w->cdm_size,.ts.start={.handle=c->object,.offset=64+j*16},.ts.end={.handle=c->object,.offset=72+j*16}}};
  }
 }
 for(unsigned round=0;round<rounds;round++) {
  uint32_t input_gate=sync_new(fd,0);int producer=sync_import_pending(fd,input_gate);
  for(unsigned n=0;n<CLIENTS;n++) {
   struct client *c=&clients[n];memset((void *)c->times,0xa5,PAGE*3);
   for(unsigned j=0;j<JOBS;j++) {for(unsigned i=0;i<64;i++) c->input[j][i]=2000.0f+(j+8)*128+(round*CLIENTS+n)*8192.0f+i;memset(c->output[j],0xa5,PAGE);}
   c->fence=sync_new(fd,0);struct drm_asahi_sync syncs[2]={{.handle=input_gate},{.handle=c->fence}};
   struct drm_asahi_submit submit={.queue_id=c->queue,.cmdbuf=(uintptr_t)c->commands,.cmdbuf_size=sizeof(c->commands),.syncs=(uintptr_t)syncs,.in_sync_count=1,.out_sync_count=1};
   CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit)==0);
  }
  {
   // Repeated long render starts before the next independent compute wave.
   render_fence=sync_new(fd,0);struct drm_asahi_sync sync={.handle=render_fence};
   unsigned char draw[sizeof(render_command)];memcpy(draw,render_command,sizeof(draw));
   struct drm_asahi_cmd_render *r=(void *)(draw+sizeof(draw)-sizeof(*r));
   struct drm_asahi_cmd_header *h=(void *)((unsigned char *)r-sizeof(*h));
   h->vdm_barrier=h->cdm_barrier=DRM_ASAHI_BARRIER_NONE;
   memset((void *)render_times,0xa5,PAGE*3);
   for(unsigned j=0;j<8;j++) memset(render_maps[j],0xa5,RENDER_OUTPUT_SIZE);
   r->ts_vtx.start=(struct drm_asahi_timestamp){.handle=ro.object_handle,.offset=0};
   r->ts_vtx.end=(struct drm_asahi_timestamp){.handle=ro.object_handle,.offset=8};
   r->ts_frag.start=(struct drm_asahi_timestamp){.handle=ro.object_handle,.offset=16};
   r->ts_frag.end=(struct drm_asahi_timestamp){.handle=ro.object_handle,.offset=24};
   struct drm_asahi_submit submit={.queue_id=rq.queue_id,.cmdbuf=(uintptr_t)draw,.cmdbuf_size=sizeof(draw),.syncs=(uintptr_t)&sync,.out_sync_count=1};
   CHECK(g17p_raw_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit)==0);
   uint64_t deadline=now_ns()+5000000000ULL;
   while(render_times[PAGE/8]==0xa5a5a5a5a5a5a5a5ULL || render_times[PAGE/8]==0) {CHECK(now_ns()<deadline);usleep(100);}
  }
  uint32_t increment=1;OK(producer,SW_INC,&increment);CHECK(close(producer)==0);
  // All three queues are accepted before waiting for any one of them.
  for(unsigned n=CLIENTS;n--;) {
   struct client *c=&clients[n];finish(fd,c->fence);uint64_t prior=0;
   for(unsigned j=0;j<JOBS;j++) {
    for(unsigned i=0;i<64;i++) {
     float a=2000.0f+(j+8)*128+(round*CLIENTS+n)*8192.0f+i;
     CHECK(c->input[j][i]==a);CHECK(c->output[j][i]==a+0.25f+(j+8));
    }
    for(unsigned i=256;i<PAGE;i++) CHECK(((unsigned char *)c->output[j])[i]==0xa5);
    uint64_t start=c->times[(PAGE+64)/8+j*2],end=c->times[(PAGE+64)/8+j*2+1];
    CHECK(start&&start!=0xa5a5a5a5a5a5a5a5ULL&&end>start&&start>=prior);prior=end;
   }
   for(unsigned i=0;i<PAGE*3;i++) if(i<PAGE+64||i>=PAGE+64+JOBS*16) CHECK(((unsigned char *)c->times)[i]==0xa5);
  }
 finish(fd,render_fence);
 for(unsigned j=0;j<8;j++) {
  float expected=RENDER_TRIANGLES*((j+1)/8.0f);unsigned char bytes[4];memcpy(bytes,&expected,4);
  for(unsigned i=0;i<RENDER_OUTPUT_SIZE;i++) {
   unsigned char want=0;
   if(i>=RENDER_PIXEL_0&&i<RENDER_PIXEL_0+4) want=bytes[i-RENDER_PIXEL_0];
   if(i>=RENDER_PIXEL_1&&i<RENDER_PIXEL_1+4) want=bytes[i-RENDER_PIXEL_1];
   if(render_maps[j][i]!=want) {
    printf("RENDER_IMAGE_FAIL round=%u target=%u byte=%u actual=%02x expected=%02x\n",round,j,i,render_maps[j][i],want);
    for(unsigned target=0;target<8;target++) {
     char path[96];snprintf(path,sizeof(path),"/tmp/overlap-render-%u.raw",target);
     int dump=open(path,O_WRONLY|O_CREAT|O_TRUNC,0600);CHECK(dump>=0);
     CHECK(write(dump,render_maps[target],RENDER_OUTPUT_SIZE)==RENDER_OUTPUT_SIZE);CHECK(close(dump)==0);
    }
   }
   CHECK(render_maps[j][i]==want);
  }
 }

  uint64_t cs0=clients[0].times[(PAGE+64)/8],cs1=clients[0].times[(PAGE+72)/8];
  uint64_t rs=render_times[PAGE/8+2],re=render_times[PAGE/8+3];
  int overlap=cs0<re && cs1>rs;
  printf("COMPUTE_RENDER_WAVE_TIMESTAMPS round=%u R=%llu/%llu CS=%llu/%llu overlap=%d\n",round,(unsigned long long)rs,(unsigned long long)re,(unsigned long long)cs0,(unsigned long long)cs1,overlap);
  CHECK(rs && re>rs);if(round>0) CHECK(overlap);
  for(unsigned i=0;i<PAGE*3;i++) if(i<PAGE || i>=PAGE+32) CHECK(((volatile unsigned char *)render_times)[i]==0xa5);
  struct drm_syncobj_destroy gate_destroy={.handle=input_gate};OK(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&gate_destroy);
  printf("COMPUTE_QUEUES_ROUND_PASS %u threeVMs distinct private apertures, %u fulloutputs and owned timestamps/fences\n",round,CLIENTS*JOBS);
 }
 puts("COMPUTE_QUEUES_FIRST_RENDER_PASS ownfence eight full images");
 CHECK(close(fd)==0);
 printf("COMPUTE_QUEUES_UAPI_PASS commands=%u queues=%u checks=%u\n",rounds*CLIENTS*JOBS,CLIENTS,checks);
 return 0;
}
