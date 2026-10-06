/* SPDX-License-Identifier: MIT */
/* Logical unbind/rebind with accepted work retaining old mapping/TS leases. */
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

struct cs_command {
    struct drm_asahi_cmd_header header;
    struct drm_asahi_cmd_compute compute;
};
struct allocation { uint32_t bo, object; unsigned char *map; };

static struct allocation allocation_new(int fd, int timestamp)
{
    struct allocation a = {.bo = bo_new(fd, PAGE * 3, DRM_ASAHI_GEM_WRITEBACK, 0)};
    a.map = bo_map(fd, a.bo, PAGE * 3);
    memset(a.map, 0xa5, PAGE * 3);
    if (timestamp) {
        struct drm_asahi_gem_bind_object b = {.op = DRM_ASAHI_BIND_OBJECT_OP_BIND,
            .flags = DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,
            .handle = a.bo, .offset = PAGE, .range = PAGE};
        OK(fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &b);
        a.object = b.object_handle;
    }
    return a;
}
static void object_unbind(int fd, uint32_t handle)
{
    struct drm_asahi_gem_bind_object b = {.op = DRM_ASAHI_BIND_OBJECT_OP_UNBIND,
        .object_handle = handle};
    OK(fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &b);
}
static int status(int fd, uint32_t fence)
{
    struct drm_syncobj_handle e = {.handle = fence,
        .flags = DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE};
    OK(fd, DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &e);
    struct sync_file_info info = {0};
    OK(e.fd, SYNC_IOC_FILE_INFO, &info);
    CHECK(close(e.fd) == 0);
    return info.status;
}
static void wait_success(int fd, uint32_t fence)
{
    struct drm_syncobj_wait w = {.handles = (uintptr_t)&fence, .count_handles = 1,
        .timeout_nsec = now_ns() + 15000000000ULL};
    OK(fd, DRM_IOCTL_SYNCOBJ_WAIT, &w);
    int result = status(fd, fence);
    printf("UNBIND_FENCE handle=%u status=%d\n", fence, result);
    CHECK(result == 1);
}
static uint32_t queue_new(int fd, uint32_t vm)
{
    struct drm_asahi_queue_create q = {.vm_id = vm, .usc_exec_base = EXEC};
    OK(fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q);
    return q.queue_id;
}
static struct cs_command command(unsigned graph, uint32_t object)
{
    const struct batch_workload *w = &batch_workloads[graph];
    return (struct cs_command){
        .header = {.cmd_type = DRM_ASAHI_CMD_COMPUTE,
            .size = sizeof(struct drm_asahi_cmd_compute),
            .vdm_barrier = DRM_ASAHI_BARRIER_NONE, .cdm_barrier = DRM_ASAHI_BARRIER_NONE},
        .compute = {.cdm_ctrl_stream_base = w->cdm, .cdm_ctrl_stream_end = w->cdm + w->cdm_size,
            .ts.start = {.handle = object, .offset = object ? 64 : 0},
            .ts.end = {.handle = object, .offset = object ? 72 : 0}},
    };
}
static void submit(int fd, uint32_t queue, unsigned graph, uint32_t object,
    uint32_t input, uint32_t output, uint32_t timeline, unsigned point)
{
    struct cs_command c = command(graph, object);
    struct drm_asahi_sync s[3]; unsigned n = 0;
    if (input) s[n++] = (struct drm_asahi_sync){.handle = input};
    s[n++] = (struct drm_asahi_sync){.handle = output};
    s[n++] = (struct drm_asahi_sync){.sync_type = DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ,
        .handle = timeline, .timeline_value = point};
    struct drm_asahi_submit b = {.queue_id = queue, .cmdbuf = (uintptr_t)&c,
        .cmdbuf_size = sizeof(c), .syncs = (uintptr_t)s,
        .in_sync_count = !!input, .out_sync_count = 2};
    CHECK(g17p_raw_ioctl(fd, DRM_IOCTL_ASAHI_SUBMIT, &b) == 0);
    memset(&c, 0xa5, sizeof(c)); memset(s, 0xa5, sizeof(s));
}
static void output_check(const struct allocation *a, float expected, int done)
{
    if (done) for (unsigned i = 0; i < 64; i++)
        CHECK(((volatile float *)(a->map + PAGE))[i] == expected + i);
    for (unsigned i = 0; i < PAGE * 3; i++) {
        if (done && i >= PAGE && i < PAGE + 256) continue;
        CHECK(((volatile unsigned char *)a->map)[i] == 0xa5);
    }
}
static void timestamp_check(const struct allocation *a, int done)
{
    volatile uint64_t *s = (void *)(a->map + PAGE + 64);
    if (done) { CHECK(s[0] != 0 && s[0] != 0xa5a5a5a5a5a5a5a5ULL); CHECK(s[1] > s[0]); }
    for (unsigned i = 0; i < PAGE * 3; i++) {
        if (done && i >= PAGE + 64 && i < PAGE + 80) continue;
        CHECK(((volatile unsigned char *)a->map)[i] == 0xa5);
    }
}
static void timeline_check(int fd, uint32_t handle, uint64_t point)
{
    struct drm_syncobj_timeline_wait w = {.handles = (uintptr_t)&handle,
        .points = (uintptr_t)&point, .count_handles = 1, .timeout_nsec = now_ns() + 15000000000ULL};
    OK(fd, DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT, &w);
}

#ifndef G17P_UNBIND_LIBRARY
int main(int argc, char **argv)
{
    setbuf(stdout, NULL);
    unsigned rounds = argc >= 2 ? strtoul(argv[1], NULL, 0) : 8;
    int input_error = argc == 3 && !strcmp(argv[2], "--input-error");
    CHECK(argc <= 3 && (argc != 3 || input_error));
    CHECK(rounds > 0 && rounds <= 4096);
    int fd = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC); CHECK(fd >= 0);
    CHECK(mount("debugfs", "/sys/kernel/debug", "debugfs", 0, NULL) == 0 || errno == EBUSY);
    uint32_t vm = vm_new(fd), old_queue = queue_new(fd, vm), new_queue = queue_new(fd, vm);
    unsigned executable = 0;
    for (unsigned i = 0; i < sizeof(render_workloads) / sizeof(render_workloads[0]); i++) {
        const struct render_workload *w = &render_workloads[i];
        if (w->address != EXEC) continue;
        uint32_t bo = bo_new(fd, w->size, DRM_ASAHI_GEM_WRITEBACK, 0);
        void *map = bo_map(fd, bo, w->size); memcpy(map, w->data, w->size);
        CHECK(munmap(map, w->size) == 0);
        bind(fd, vm, bo, w->address, w->size, 0, DRM_ASAHI_BIND_READ, 0);
        executable++;
    }
    CHECK(executable == 1);
    float *inputs[2][2] = {{0}}, *independent = NULL;
    for (unsigned i = 0; i < sizeof(workloads) / sizeof(workloads[0]); i++) {
        const struct workload *w = &workloads[i];
        if (w->address == batch_workloads[0].output) continue;
        uint32_t bo = bo_new(fd, w->size, DRM_ASAHI_GEM_WRITEBACK, 0);
        void *map = bo_map(fd, bo, w->size); memcpy(map, w->data, w->size); int keep = 0;
        for (unsigned j = 0; j < 2; j++) {
            if (w->address == batch_workloads[j].input_a) { inputs[j][0] = map; keep = 1; }
            if (w->address == batch_workloads[j].input_b) { inputs[j][1] = map; keep = 1; }
        }
        if (w->address == batch_workloads[1].output) { independent = map; keep = 1; }
        if (!keep) CHECK(munmap(map, w->size) == 0);
        bind(fd, vm, bo, w->address, w->size, 0,
            DRM_ASAHI_BIND_READ | (w->writable ? DRM_ASAHI_BIND_WRITE : 0), 0);
    }
    CHECK(inputs[0][0] && inputs[0][1] && inputs[1][0] && inputs[1][1] && independent);
    uint32_t gate = sync_new(fd, 0), timeline = sync_new(fd, 0);
    for (unsigned j = 0; j < 2; j++) for (unsigned i = 0; i < 64; i++) {
        inputs[j][0][i] = 5000.0f + j * 512.0f + i; inputs[j][1][i] = 0.25f + j;
    }
    memset(independent, 0xa5, PAGE);
    uint32_t warm = sync_new(fd, 0);
    submit(fd, new_queue, 1, 0, 0, warm, timeline, 1); wait_success(fd, warm);
    for (unsigned i = 0; i < 64; i++) CHECK(independent[i] == 5513.25f + i);
    for (unsigned i = 256; i < PAGE; i++) CHECK(((unsigned char *)independent)[i] == 0xa5);
    puts("UNBIND_FIXTURE_PREFLIGHT_PASS unchanged source compute, before any unbind");
    for (unsigned round = 0; round < rounds; round++) {
        float base = 1000.0f + round * 4096.0f;
        for (unsigned j = 0; j < 2; j++) for (unsigned i = 0; i < 64; i++) {
            inputs[j][0][i] = base + j * 512.0f + i; inputs[j][1][i] = 0.25f + j;
        }
        memset(independent, 0xa5, PAGE);
        struct allocation old = allocation_new(fd, 0), fresh = allocation_new(fd, 0);
        struct allocation old_ts = allocation_new(fd, 1), fresh_ts = allocation_new(fd, 1);
        bind(fd, vm, old.bo, batch_workloads[0].output, PAGE, PAGE, RW, 0);
        const uint64_t alias = 0x18000;
        bind(fd, vm, old.bo, alias, PAGE * 3, 0, RW, 0);
        uint32_t f_old = sync_new(fd, 0), f_new = sync_new(fd, 0), f_other = sync_new(fd, 0);
        int sw = sync_import_pending(fd, gate);
        submit(fd, old_queue, 0, old_ts.object, gate, f_old, timeline, (round + 1) * 3 + 1);
        CHECK(status(fd, f_old) == 0);
        /* Both an unrelated range and a range captured by accepted work. */
        bind(fd, vm, 0, alias + PAGE, PAGE, 0, DRM_ASAHI_BIND_UNBIND, 0);
        bind(fd, vm, fresh.bo, alias + PAGE, PAGE, PAGE, RW, 0);
        bind(fd, vm, 0, batch_workloads[0].output, PAGE, 0, DRM_ASAHI_BIND_UNBIND, 0);
        object_unbind(fd, old_ts.object);
        struct cs_command removed = command(0, old_ts.object);
        struct drm_asahi_submit bad = {.queue_id = new_queue, .cmdbuf = (uintptr_t)&removed,
            .cmdbuf_size = sizeof(removed)};
        errno = 0; CHECK(g17p_raw_ioctl(fd, DRM_IOCTL_ASAHI_SUBMIT, &bad) == -1 && errno == ENOENT);
        removed = command(0, fresh_ts.object);
        removed.compute.cdm_ctrl_stream_base = batch_workloads[0].output;
        removed.compute.cdm_ctrl_stream_end = batch_workloads[0].output + 32;
        errno = 0; CHECK(g17p_raw_ioctl(fd, DRM_IOCTL_ASAHI_SUBMIT, &bad) == -1 && errno == EINVAL);
        bind(fd, vm, fresh.bo, batch_workloads[0].output, PAGE, PAGE, RW, 0);
        bo_close(fd, old.bo); bo_close(fd, old_ts.bo);
        submit(fd, new_queue, 1, 0, 0, f_other, timeline, (round + 1) * 3 + 2);
        wait_success(fd, f_other);
        for (unsigned i = 0; i < 64; i++) CHECK(independent[i] == base + 513.25f + i);
        for (unsigned i = 256; i < PAGE; i++) CHECK(((unsigned char *)independent)[i] == 0xa5);
        submit(fd, new_queue, 0, fresh_ts.object, 0, f_new, timeline, (round + 1) * 3 + 3);
        wait_success(fd, f_new);
        CHECK(status(fd, f_old) == 0);
        output_check(&old, base + 0.25f, 0); timestamp_check(&old_ts, 0);
        output_check(&fresh, base + 0.25f, 1); timestamp_check(&fresh_ts, 1);
        if (input_error) {
            CHECK(close(sw) == 0); sw = -1;
            struct drm_syncobj_wait w = {.handles = (uintptr_t)&f_old, .count_handles = 1,
                .timeout_nsec = now_ns() + 15000000000ULL};
            OK(fd, DRM_IOCTL_SYNCOBJ_WAIT, &w);
            CHECK(status(fd, f_old) == -ENOENT);
        } else {
            uint32_t inc = 1; OK(sw, SW_INC, &inc); wait_success(fd, f_old);
        }
        timeline_check(fd, timeline, (round + 1) * 3 + 3);
        if (sw >= 0) CHECK(close(sw) == 0);
        output_check(&old, base + 0.25f, !input_error); timestamp_check(&old_ts, !input_error);
        output_check(&fresh, base + 0.25f, 1); timestamp_check(&fresh_ts, 1);
        bind(fd, vm, 0, batch_workloads[0].output, PAGE, 0, DRM_ASAHI_BIND_UNBIND, 0);
        bind(fd, vm, 0, alias, PAGE * 3, 0, DRM_ASAHI_BIND_UNBIND, 0);
        object_unbind(fd, fresh_ts.object);
        bo_close(fd, fresh.bo); bo_close(fd, fresh_ts.bo);
        CHECK(munmap(old.map, PAGE * 3) == 0 && munmap(fresh.map, PAGE * 3) == 0);
        CHECK(munmap(old_ts.map, PAGE * 3) == 0 && munmap(fresh_ts.map, PAGE * 3) == 0);
        uint32_t fences[] = {f_old, f_new, f_other};
        for (unsigned i = 0; i < 3; i++) {
            struct drm_syncobj_destroy d = {.handle = fences[i]}; OK(fd, DRM_IOCTL_SYNCOBJ_DESTROY, &d);
        }
        printf("UNBIND_PENDING_ROUND %u old/new outputs, TS, guards, binary/timeline, partial range, aliases, close, independent progress PASS\n", round);
    }
    CHECK(close(fd) == 0);
    printf("G17P_UNBIND_PENDING_PASS rounds=%u submissions=%u checks=%u input_error=%d\n", rounds, rounds * 3, checks, input_error);
    return 0;
}

#endif
