/* SPDX-License-Identifier: MIT */
/* UAPI regression: retire B, publish 255 A jobs, then reuse B across wrap. */
#define G17P_RENDER_VM_HELPERS
#include "g17p_drm_render_vm.c"
#include "g17p_drm_sync.h"

static void submit_checked(struct render_owner *o, unsigned owner)
{
 unsigned generation = o->last + 1;
 CHECK(generation < 2048);
 struct drm_asahi_cmd_render *command = (void *)(o->command + sizeof(o->command) - sizeof(*command));
 struct drm_asahi_timestamp *refs[] = { &command->ts_vtx.start, &command->ts_vtx.end, &command->ts_frag.start, &command->ts_frag.end };
 for (unsigned i=0;i<4;i++) *refs[i]=(struct drm_asahi_timestamp){.handle=o->object,.offset=64+generation*32+i*8};
 struct drm_syncobj_array reset={.handles=(uintptr_t)&o->binary,.count_handles=1};
 OK(o->fd,DRM_IOCTL_SYNCOBJ_RESET,&reset);
 for(unsigned i=0;i<8;i++) memset(o->images[i],0xa5,RENDER_OUTPUT_SIZE);
 struct drm_asahi_sync syncs[]={
  {.sync_type=DRM_ASAHI_SYNC_SYNCOBJ,.handle=o->binary},
  {.sync_type=DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ,.handle=o->timeline,.timeline_value=generation+1},
 };
 struct drm_asahi_submit submit={.queue_id=o->queue,.cmdbuf=(uintptr_t)o->command,.cmdbuf_size=sizeof(o->command),.syncs=(uintptr_t)syncs,.out_sync_count=2};
 CHECK(g17p_raw_ioctl(o->fd,DRM_IOCTL_ASAHI_SUBMIT,&submit)==0);
 struct drm_syncobj_wait wait={.handles=(uintptr_t)&o->binary,.count_handles=1,.timeout_nsec=now_ns()+15000000000ULL};
 OK(o->fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);
 struct drm_syncobj_handle exported={.handle=o->binary,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
 OK(o->fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&exported);
 struct sync_file_info info={0};CHECK(ioctl(exported.fd,SYNC_IOC_FILE_INFO,&info)==0);CHECK(close(exported.fd)==0);
 if(info.status!=1) printf("DORMANT_POOL_FENCE_ERROR owner=%u generation=%u status=%d\n",owner,generation,info.status);
 CHECK(info.status==1);
 uint64_t point=generation+1;
 struct drm_syncobj_timeline_wait twait={.handles=(uintptr_t)&o->timeline,.points=(uintptr_t)&point,.count_handles=1,.timeout_nsec=0};
 OK(o->fd,DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT,&twait);
 memcpy(o->saved[generation],o->timestamps+PAGE+64+generation*32,32);
 uint64_t *ts=o->saved[generation];CHECK(ts[0] && ts[1]>ts[0] && ts[2]>=ts[1] && ts[3]>ts[2]);
 if(generation) CHECK(ts[0]>o->saved[generation-1][3]);
 o->last=generation;check_render_owner(o,owner);
}
int main(void)
{
 setbuf(stdout,NULL);tiny_render=1;render_timestamp_range=PAGE*7;
 struct render_owner owners[2]={{0}};for(unsigned i=0;i<2;i++)create_render_owner(&owners[i],i);
 submit_checked(&owners[1],1);
 for(unsigned wave=0;wave<3;wave++){
  for(unsigned i=0;i<255;i++)submit_checked(&owners[0],0);
  check_render_owner(&owners[1],1);
  printf("DORMANT_POOL_RESUME wave=%u preceding_other_jobs=255\n",wave);
  submit_checked(&owners[1],1);check_render_owner(&owners[0],0);
 }
 for(unsigned i=0;i<2;i++){check_render_owner(&owners[i],i);CHECK(close(owners[i].fd)==0);}
 puts("G17P_DORMANT_POOL_WRAP_PASS 769 checked renders,three dormant outer wraps,all images/guards/private timestamps/binary+timeline fences");
 return 0;
}
