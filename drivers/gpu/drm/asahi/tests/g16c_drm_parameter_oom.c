// SPDX-License-Identifier: GPL-2.0-only
// Native owned OOM7/error9, failed dependency and same-queue recovery.
#define _POSIX_C_SOURCE 200809L
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>
#include "asahi_drm.h"
#include <linux/sync_file.h>
#include <time.h>
#include "g17p_drm_wait.h"

struct object { uint32_t id,flags; uint64_t size; };
static void checked_ioctl(int fd,unsigned long request,void *arg) {
    int status=ioctl(fd,request,arg);
    if (status<0) {
        fprintf(stderr,"ioctl %#lx returned %d (errno %d: %s)\n",request,status,errno,strerror(errno));
        exit(1);
    }
}
static void read_exact(FILE *f,void *data,size_t size) {
    if (fread(data,1,size,f)!=size) { fputs("short fixture\n",stderr); exit(1); }
}
static FILE *input(const char *dir,const char *name) {
    char path[4096];
    int n=snprintf(path,sizeof(path),"%s/%s",dir,name);
    assert(n>0 && (size_t)n<sizeof(path));
    FILE *f=fopen(path,"rb"); if (!f) { perror(path); exit(1); } return f;
}
static void set_timestamps(void *commands,size_t size,uint32_t object_handle,unsigned generation) {
    size_t cursor=0; unsigned render=0,compute=0,index=0;
    while(cursor<size) {
        assert(size-cursor>=sizeof(struct drm_asahi_cmd_header));
        struct drm_asahi_cmd_header *header=(void *)((uint8_t *)commands+cursor);
        cursor+=sizeof(*header); assert(header->size<=size-cursor);
        if(header->cmd_type==DRM_ASAHI_CMD_RENDER || header->cmd_type==DRM_ASAHI_CMD_COMPUTE) {
            assert(header->vdm_barrier==0);
            if(generation==3)header->vdm_barrier=UINT16_MAX; /* Explicit independent retry after failed render. */
        } else assert(header->vdm_barrier==UINT16_MAX && header->cdm_barrier==UINT16_MAX);
        struct drm_asahi_timestamp *stamps[4]; unsigned count=0;
        if(header->cmd_type==DRM_ASAHI_CMD_RENDER) {
            assert(header->size==sizeof(struct drm_asahi_cmd_render) && !render && compute==1);
            struct drm_asahi_cmd_render *cmd=(void *)((uint8_t *)commands+cursor);
            assert(cmd->width_px==1 && cmd->height_px==1 && cmd->samples==1 && cmd->layers==1);
            assert(header->cdm_barrier==1);
            stamps[0]=&cmd->ts_vtx.start; stamps[1]=&cmd->ts_vtx.end;
            stamps[2]=&cmd->ts_frag.start; stamps[3]=&cmd->ts_frag.end;
            count=4; render++;
        } else if(header->cmd_type==DRM_ASAHI_CMD_COMPUTE) {
            assert(header->size==sizeof(struct drm_asahi_cmd_compute) && !render && !compute);

            struct drm_asahi_cmd_compute *cmd=(void *)((uint8_t *)commands+cursor);
            stamps[0]=&cmd->ts.start; stamps[1]=&cmd->ts.end;
            count=2; compute++;
        }
        for(unsigned i=0;i<count;i++,index++) {
            stamps[i]->handle=object_handle; stamps[i]->offset=24+index*32;
        }
        cursor+=header->size;
    }
    assert(cursor==size && render==1 && compute==1 && index==6);
}
static int fence_status(int fd,uint32_t handle) {
    struct drm_syncobj_handle export={.handle=handle,.flags=DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
    checked_ioctl(fd,DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD,&export);
    struct sync_file_info info={0};checked_ioctl(export.fd,SYNC_IOC_FILE_INFO,&info);
    assert(close(export.fd)==0);return info.status;
}
int main(int argc,char **argv) {
    assert(argc==3 || (argc==4 && (!strcmp(argv[3],"--file-close") || !strcmp(argv[3],"--preflight"))));
    int file_close=argc==4;uint64_t origin=UINT64_C(0x7000000000);
    _Static_assert(sizeof(struct object)==16,"fixture layout");
    FILE *f=input(argv[1],"inputs.bin"); uint32_t h[8]; read_exact(f,h,sizeof(h));
    assert(h[0]==0x3242434d && h[1]==2 && h[2]<=128 && h[3]<8192 && h[4]<65536);
    struct object *objects=calloc(h[2],sizeof(*objects));
    struct drm_asahi_gem_bind_op *binds=calloc(h[3],sizeof(*binds));
    void *commands=malloc(h[4]); assert(objects && binds && commands);
    read_exact(f,objects,h[2]*sizeof(*objects)); read_exact(f,binds,h[3]*sizeof(*binds));
    read_exact(f,commands,h[4]); assert(fgetc(f)==EOF); fclose(f);
    assert(h[5]==200000 && h[6]==0 && h[7]==0);
    if(argc==4 && !strcmp(argv[3],"--preflight")) {
        for(unsigned phase=1;phase<=3;phase++)set_timestamps(commands,h[4],0x1234,phase);
        free(objects);free(binds);free(commands);puts("G16C_OOM_READER_PREFLIGHT_PASS three phases, hardware barriers and synthetic attachments");return 0;
    }
    int fd=open("/dev/dri/renderD128",O_RDWR); if(fd<0) { perror("render node"); return 1; }
    struct drm_asahi_params_global params={0};
    struct drm_asahi_get_params get={.pointer=(uintptr_t)&params,.size=sizeof(params)};
    checked_ioctl(fd,DRM_IOCTL_ASAHI_GET_PARAMS,&get);
    assert(params.chip_id==0x6041 && params.gpu_generation==16 && params.gpu_variant=='C');
    assert(params.max_commands_per_submission>=2);
    struct drm_asahi_vm_create vm={.kernel_start=origin,.kernel_end=origin+0x20000000};
    checked_ioctl(fd,DRM_IOCTL_ASAHI_VM_CREATE,&vm);
    uint32_t handles[128]={0}; uint8_t *maps[128]={0},*seeds[128]={0};
    for(unsigned i=0;i<h[2];i++) {
        assert(objects[i].id<128 && objects[i].size>0 && objects[i].size<=0x1000000);
        struct drm_asahi_gem_create create={.size=objects[i].size,.flags=objects[i].flags,
            .vm_id=(objects[i].flags&DRM_ASAHI_GEM_VM_PRIVATE)?vm.vm_id:0};
        checked_ioctl(fd,DRM_IOCTL_ASAHI_GEM_CREATE,&create); handles[objects[i].id]=create.handle;
        struct drm_asahi_gem_mmap_offset offset={.handle=create.handle};
        checked_ioctl(fd,DRM_IOCTL_ASAHI_GEM_MMAP_OFFSET,&offset);
        maps[i]=mmap(NULL,objects[i].size,PROT_READ|PROT_WRITE,MAP_SHARED,fd,offset.offset);
        assert(maps[i]!=MAP_FAILED); char name[64]; snprintf(name,sizeof(name),"gem-%u.bin",objects[i].id);
        f=input(argv[1],name); read_exact(f,maps[i],objects[i].size); assert(fgetc(f)==EOF); fclose(f);
        if(objects[i].id==12 || objects[i].id==10 || objects[i].id==26) {
            seeds[i]=malloc(objects[i].size); assert(seeds[i]); memcpy(seeds[i],maps[i],objects[i].size);
        }
    }
    for(unsigned i=0;i<h[3];i++) { assert(binds[i].handle<128 && handles[binds[i].handle]); binds[i].handle=handles[binds[i].handle]; }
    struct drm_asahi_vm_bind bind={.vm_id=vm.vm_id,.num_binds=h[3],.stride=sizeof(*binds),.userptr=(uintptr_t)binds};
    checked_ioctl(fd,DRM_IOCTL_ASAHI_VM_BIND,&bind);
    struct drm_asahi_queue_create queue={.vm_id=vm.vm_id,.priority=1,.usc_exec_base=UINT64_C(1)<<40};
    checked_ioctl(fd,DRM_IOCTL_ASAHI_QUEUE_CREATE,&queue);
    printf("READY: native OOM7/error9 then failed dependency and independent same-queue recovery instances=%u\n",h[5]); fflush(stdout);
    struct drm_syncobj_create sync_create={0}; checked_ioctl(fd,DRM_IOCTL_SYNCOBJ_CREATE,&sync_create);
    uint8_t *timestamp_maps[3]={0};
    uint32_t timestamp_handles[3]={0};
    for(unsigned generation=1;generation<=3;generation++) {
        struct drm_asahi_gem_create stamp_create={.size=0x4000};
        checked_ioctl(fd,DRM_IOCTL_ASAHI_GEM_CREATE,&stamp_create);
        timestamp_handles[generation-1]=stamp_create.handle;
        struct drm_asahi_gem_mmap_offset stamp_offset={.handle=stamp_create.handle};
        checked_ioctl(fd,DRM_IOCTL_ASAHI_GEM_MMAP_OFFSET,&stamp_offset);
        uint8_t *stamp_map=mmap(NULL,0x4000,PROT_READ|PROT_WRITE,MAP_SHARED,fd,stamp_offset.offset);
        assert(stamp_map!=MAP_FAILED); timestamp_maps[generation-1]=stamp_map;
        memset(stamp_map,0xa5,0x4000);
        /* Keep a user alias so subsequent submits upload/read back old bank
         * guards. Late firmware DMA becomes visible to this C client. */
        struct drm_asahi_gem_bind_op stamp_alias={.flags=DRM_ASAHI_BIND_READ|DRM_ASAHI_BIND_WRITE,
            .handle=stamp_create.handle,.range=0x4000,
            .addr=UINT64_C(0x6fe0000000)+(generation-1)*0x4000};
        struct drm_asahi_vm_bind stamp_vm_bind={.vm_id=vm.vm_id,.num_binds=1,
            .stride=sizeof(stamp_alias),.userptr=(uintptr_t)&stamp_alias};
        checked_ioctl(fd,DRM_IOCTL_ASAHI_VM_BIND,&stamp_vm_bind);
        struct drm_asahi_gem_bind_object stamp_bind={.op=DRM_ASAHI_BIND_OBJECT_OP_BIND,
            .flags=DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,.handle=stamp_create.handle,
            .offset=0,.range=0x4000};
        checked_ioctl(fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&stamp_bind);
        assert(stamp_bind.object_handle);
        set_timestamps(commands,h[4],stamp_bind.object_handle,generation);
        if(generation>=2) {
            unsigned uniform=0;while(uniform<h[2] && objects[uniform].id!=14)uniform++;assert(uniform<h[2]);
            f=input(argv[2],"gem-14.bin");read_exact(f,maps[uniform],objects[uniform].size);assert(fgetc(f)==EOF);fclose(f);
        }
        struct drm_asahi_get_time before={0};
        checked_ioctl(fd,DRM_IOCTL_ASAHI_GET_TIME,&before);
        for(unsigned i=0;i<h[2];i++) if(seeds[i]) memcpy(maps[i],seeds[i],objects[i].size);
        struct drm_syncobj_create binary={0};checked_ioctl(fd,DRM_IOCTL_SYNCOBJ_CREATE,&binary);
        struct drm_asahi_sync sync[2]={{.sync_type=DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ,.handle=sync_create.handle,.timeline_value=generation},
            {.sync_type=DRM_ASAHI_SYNC_SYNCOBJ,.handle=binary.handle}};
        struct drm_asahi_submit submit={.queue_id=queue.queue_id,.cmdbuf=(uintptr_t)commands,
            .cmdbuf_size=h[4],.syncs=(uintptr_t)&sync,.out_sync_count=2};
        checked_ioctl(fd,DRM_IOCTL_ASAHI_SUBMIT,&submit);
        uint64_t point=generation;
        struct drm_syncobj_timeline_wait wait={.handles=(uintptr_t)&sync_create.handle,.points=(uintptr_t)&point,
            .count_handles=1,.timeout_nsec=0};
        checked_ioctl(fd,DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT,&wait);
        int state=fence_status(fd,binary.handle);
        assert(state==(generation==3?1:-ENOMEM));
        struct drm_syncobj_destroy binary_destroy={.handle=binary.handle};checked_ioctl(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&binary_destroy);
        unsigned checked=0;
        for(unsigned i=0;i<h[2];i++) if(seeds[i]) {
            char name[64];snprintf(name,sizeof(name),"expected-gem-%u.bin",objects[i].id);
            uint8_t *expected=malloc(objects[i].size);assert(expected);
            f=input(generation==1?argv[1]:argv[2],name);read_exact(f,expected,objects[i].size);assert(fgetc(f)==EOF);fclose(f);
            size_t written=objects[i].id==12?16:objects[i].id==10?4:8;
            assert(!memcmp(maps[i]+written,seeds[i]+written,objects[i].size-written));
            const uint8_t *wanted=generation==2?seeds[i]:expected;
            int exact=generation>=2 || objects[i].id==12;
            if(exact && memcmp(maps[i],wanted,objects[i].size)) {
                for(size_t at=0;at<objects[i].size;at++)if(maps[i][at]!=wanted[at]) {
                    fprintf(stderr,"GEM %u mismatch byte %zu: got %02x expected %02x\n",objects[i].id,at,maps[i][at],wanted[at]);break;
                }
                exit(1);
            }
            if(generation==3 || (generation==1 && objects[i].id==12))assert(memcmp(maps[i],seeds[i],objects[i].size));
            char actual_name[128];
            snprintf(actual_name,sizeof(actual_name),"/oom-epoch%u-gem%u.bin",generation,objects[i].id);
            FILE *actual=fopen(actual_name,"wb");assert(actual);
            assert(fwrite(maps[i],1,objects[i].size,actual)==objects[i].size);assert(fclose(actual)==0);
            free(expected);
            checked++;
        }
        assert(checked==3);
        struct drm_asahi_get_time after={0};
        checked_ioctl(fd,DRM_IOCTL_ASAHI_GET_TIME,&after);
        uint64_t values[6];
        for(unsigned i=0;i<6;i++) {
            memcpy(&values[i],stamp_map+24+i*32,8);
            if(generation==2)assert(values[i]==UINT64_C(0xa5a5a5a5a5a5a5a5));
            else if(generation==3 || i<2) {
                assert(values[i]!=UINT64_C(0xa5a5a5a5a5a5a5a5));
                assert(before.gpu_timestamp<=values[i] && values[i]<=after.gpu_timestamp);
            }
        }
        if(generation!=2)assert(values[0]<=values[1]);
        if(generation==3) {assert(values[2]<=values[3] && values[4]<=values[5]);assert(values[1]<=values[2]);}
        for(unsigned byte=0;byte<0x4000;byte++) {
            int written=byte>=24 && byte<192 && (byte-24)%32<8;
            if(!written) assert(stamp_map[byte]==0xa5);
        }
        char actual_stamp[128];snprintf(actual_stamp,sizeof(actual_stamp),"/oom-epoch%u-timestamps.bin",generation);
        FILE *actual=fopen(actual_stamp,"wb");assert(actual);
        assert(fwrite(stamp_map,1,0x4000,actual)==0x4000);assert(fclose(actual)==0);
        for(unsigned old=0;old+1<generation;old++)
            for(unsigned byte=0;byte<0x4000;byte++) assert(timestamp_maps[old][byte]==0x3c);
        struct drm_asahi_gem_bind_object unbind={.op=DRM_ASAHI_BIND_OBJECT_OP_UNBIND,
            .object_handle=stamp_bind.object_handle};
        checked_ioctl(fd,DRM_IOCTL_ASAHI_GEM_BIND_OBJECT,&unbind);
        memset(stamp_map,0x3c,0x4000);
        printf("G16C_OOM_PHASE_PASS phase=%u fence_status=%d actual_OOM=%d dependency_rejected=%d independent_recovery=%d guards=294884 prior_timestamp_poison=1\n",generation,state,generation==1,generation==2,generation==3);
        fflush(stdout);
    }
    struct drm_asahi_queue_destroy qd={.queue_id=queue.queue_id};
    if(!file_close)checked_ioctl(fd,DRM_IOCTL_ASAHI_QUEUE_DESTROY,&qd);
    for(unsigned i=0;i<h[2];i++) {
        struct drm_gem_close drop={.handle=handles[objects[i].id]};
        checked_ioctl(fd,DRM_IOCTL_GEM_CLOSE,&drop);
        assert(munmap(maps[i],objects[i].size)==0);free(seeds[i]);
    }
    for(unsigned i=0;i<3;i++) {
        struct drm_gem_close drop={.handle=timestamp_handles[i]};
        checked_ioctl(fd,DRM_IOCTL_GEM_CLOSE,&drop);
        assert(munmap(timestamp_maps[i],0x4000)==0);
    }
    struct drm_asahi_vm_destroy vd={.vm_id=vm.vm_id};
    if(!file_close)checked_ioctl(fd,DRM_IOCTL_ASAHI_VM_DESTROY,&vd);
    struct drm_syncobj_destroy sd={.handle=sync_create.handle};
    checked_ioctl(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&sd);
    assert(close(fd)==0);free(objects);free(binds);free(commands);
    printf("G16C_NATIVE_PARAMETER_OOM_CLOSE_PASS file_close=%d origin=%#" PRIx64 "\n",file_close,origin);return 0;
}
