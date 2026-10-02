/* SPDX-License-Identifier: MIT */
static int g17p_raw_ioctl(int fd, unsigned long request, void *argument)
{
    return ioctl(fd, request, argument);
}
static int g17p_wait_ioctl(int fd, unsigned long request, void *argument)
{
    if (request != DRM_IOCTL_ASAHI_SUBMIT) return g17p_raw_ioctl(fd,request,argument);
    struct drm_asahi_submit *user=argument, submit=*user;
    unsigned count=submit.in_sync_count+submit.out_sync_count;
    /* Malformed-count/pointer tests must reach the kernel unchanged. */
    if (submit.in_sync_count>4096 || submit.out_sync_count>4095 ||
        (count && submit.syncs<4096)) return g17p_raw_ioctl(fd,request,argument);
    struct drm_asahi_sync *syncs=calloc(count+1,sizeof(*syncs));
    if (!syncs) { errno=ENOMEM;return -1; }
    if (count) memcpy(syncs,(void *)(uintptr_t)submit.syncs,count*sizeof(*syncs));
    struct drm_syncobj_create object={0};
    if (g17p_raw_ioctl(fd,DRM_IOCTL_SYNCOBJ_CREATE,&object)) {free(syncs);return -1;}
    syncs[count]=(struct drm_asahi_sync){.handle=object.handle};
    submit.syncs=(uintptr_t)syncs;submit.out_sync_count++;
    int ret=g17p_raw_ioctl(fd,request,&submit),saved=errno;
    if (!ret) {
        struct timespec now;
        if (clock_gettime(CLOCK_MONOTONIC,&now)) {ret=-1;saved=errno;}
        else {
            struct drm_syncobj_wait wait={.handles=(uintptr_t)&object.handle,
                .count_handles=1,.timeout_nsec=(int64_t)now.tv_sec*1000000000+now.tv_nsec+15000000000LL,
                .flags=DRM_SYNCOBJ_WAIT_FLAGS_WAIT_ALL};
            ret=g17p_raw_ioctl(fd,DRM_IOCTL_SYNCOBJ_WAIT,&wait);saved=errno;
        }
    }
    struct drm_syncobj_destroy destroy={.handle=object.handle};
    g17p_raw_ioctl(fd,DRM_IOCTL_SYNCOBJ_DESTROY,&destroy);
    free(syncs);errno=saved;return ret;
}
#define ioctl(fd, request, argument) g17p_wait_ioctl(fd, request, argument)
