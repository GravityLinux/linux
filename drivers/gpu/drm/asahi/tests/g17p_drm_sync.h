/* SPDX-License-Identifier: MIT */
/* Optional --sync checks for g17p_drm_compute.c. CONFIG_SW_SYNC is needed
 * only for testing imported, delayed/error fences; GPU work stays native. */
#include <linux/sync_file.h>
#include <signal.h>
#include <sys/mount.h>
#include <sys/wait.h>
#include <time.h>

struct sw_fence_create { uint32_t value; char name[32]; int32_t fence; };
#define SW_CREATE _IOWR('W', 0, struct sw_fence_create)
#define SW_INC _IOW('W', 1, uint32_t)
struct sync_test {
	uint32_t input, seed, binary, timeline;
	struct drm_asahi_sync entries[4];
	int sw;
	pid_t child;
	uint64_t started, last_stamp;
};
static volatile sig_atomic_t sync_interrupted;
static void sync_interrupt(int sig) { (void)sig; sync_interrupted = 1; }
static uint64_t now_ns(void)
{
	struct timespec ts;
	CHECK(clock_gettime(CLOCK_MONOTONIC, &ts) == 0);
	return (uint64_t)ts.tv_sec * 1000000000 + ts.tv_nsec;
}
static uint32_t sync_new(int fd, uint32_t flags)
{
	struct drm_syncobj_create c = { .flags = flags };
	OK(fd, DRM_IOCTL_SYNCOBJ_CREATE, &c);
	return c.handle;
}
static uint64_t sync_point(int fd, uint32_t handle)
{
	uint64_t point = UINT64_MAX;
	struct drm_syncobj_timeline_array q = { .handles = (uintptr_t)&handle,
		.points = (uintptr_t)&point, .count_handles = 1,
		.flags = DRM_SYNCOBJ_QUERY_FLAGS_LAST_SUBMITTED };
	OK(fd, DRM_IOCTL_SYNCOBJ_QUERY, &q);
	return point;
}
static void sync_empty(int fd, struct sync_test *t)
{
	struct drm_syncobj_handle e = { .handle = t->binary,
		.flags = DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE };
	BAD(fd, DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &e, EINVAL);
	CHECK(sync_point(fd, t->timeline) == 0);
}
static uint64_t sync_stamp(int fd, uint32_t handle)
{
	struct drm_syncobj_handle e = { .handle = handle,
		.flags = DRM_SYNCOBJ_HANDLE_TO_FD_FLAGS_EXPORT_SYNC_FILE };
	OK(fd, DRM_IOCTL_SYNCOBJ_HANDLE_TO_FD, &e);
	struct sync_fence_info fence = {0};
	struct sync_file_info info = { .num_fences = 1, .sync_fence_info = (uintptr_t)&fence };
	OK(e.fd, SYNC_IOC_FILE_INFO, &info);
	CHECK(info.status == 1 && info.num_fences == 1 && fence.status == 1);
	CHECK(!strcmp(fence.driver_name, "asahi") && !strcmp(fence.obj_name, "neo-execution"));
	CHECK(fence.timestamp_ns != 0);
	CHECK(close(e.fd) == 0);
	return fence.timestamp_ns;
}
static int sync_import_pending(int fd, uint32_t handle)
{
	int sw = open("/sys/kernel/debug/sync/sw_sync", O_RDWR | O_CLOEXEC);
	CHECK(sw >= 0);
	struct sw_fence_create c = { .value = 1, .name = "neo-dependency" };
	OK(sw, SW_CREATE, &c);
	struct drm_syncobj_handle import = { .handle = handle, .fd = c.fence,
		.flags = DRM_SYNCOBJ_FD_TO_HANDLE_FLAGS_IMPORT_SYNC_FILE };
	OK(fd, DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE, &import);
	CHECK(close(c.fence) == 0);
	return sw;
}
static void sync_join(pid_t child)
{
	int status;
	CHECK(waitpid(child, &status, 0) == child);
	CHECK(WIFEXITED(status) && WEXITSTATUS(status) == 0);
}
static void sync_setup(int fd, struct sync_test *t, struct drm_asahi_submit *submit)
{
	CHECK(mount("debugfs", "/sys/kernel/debug", "debugfs", 0, NULL) == 0 || errno == EBUSY);
	t->input = sync_new(fd, 0);
	t->seed = sync_new(fd, DRM_SYNCOBJ_CREATE_SIGNALED);
	t->binary = sync_new(fd, 0);
	t->timeline = sync_new(fd, 0);
	t->entries[0] = (struct drm_asahi_sync){ .handle = t->input };
	t->entries[1] = (struct drm_asahi_sync){ .handle = t->seed };
	t->entries[2] = (struct drm_asahi_sync){ .handle = t->binary };
	t->entries[3] = (struct drm_asahi_sync){ .sync_type = DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ,
		.handle = t->timeline, .timeline_value = 1 };
	submit->syncs = (uintptr_t)t->entries;
	submit->in_sync_count = 2; submit->out_sync_count = 2;
	/* Missing input fence, malformed type/value/handle/count and bad pointer
	 * must never initialize the GPU or install an output fence. */
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, EINVAL);
	t->entries[0].handle = t->seed;
	t->entries[2].sync_type = 2;
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, EINVAL);
	t->entries[2].sync_type = 0; t->entries[2].timeline_value = 1;
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, EINVAL);
	t->entries[2].timeline_value = 0; t->entries[2].handle = UINT32_MAX;
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, ENOENT);
	t->entries[2].handle = t->binary; submit->syncs = 1;
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, EFAULT);
	submit->syncs = (uintptr_t)t->entries; submit->in_sync_count = UINT32_MAX;
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, EINVAL);
	submit->in_sync_count = 2; t->entries[0].handle = t->input;
	sync_empty(fd, t);

	/* sw_sync signals outstanding fences with -ENOENT when its timeline
	 * closes. The imported error must be propagated before publication. */
	t->sw = sync_import_pending(fd, t->input);
	CHECK(close(t->sw) == 0);
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, ENOENT);
	sync_empty(fd, t);
	/* A timeline chain's own status hides its contained fence's error. */
	t->sw = sync_import_pending(fd, t->input);
	uint32_t failed_timeline = sync_new(fd, 0);
	struct drm_syncobj_transfer transfer = { .src_handle = t->input,
		.dst_handle = failed_timeline, .dst_point = 5 };
	OK(fd, DRM_IOCTL_SYNCOBJ_TRANSFER, &transfer);
	/* Transfer merges an already-signaled fence into a successful stub,
	 * so inject the error only after the pending fence is transferred. */
	CHECK(close(t->sw) == 0);
	t->entries[0] = (struct drm_asahi_sync){ .sync_type = DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ,
		.handle = failed_timeline, .timeline_value = 5 };
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, ENOENT);
	sync_empty(fd, t);
	struct drm_syncobj_destroy destroy = { .handle = failed_timeline };
	OK(fd, DRM_IOCTL_SYNCOBJ_DESTROY, &destroy);
	t->entries[0] = (struct drm_asahi_sync){ .handle = t->input };
	t->sw = sync_import_pending(fd, t->input);
	uint64_t before = now_ns();
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, ETIMEDOUT);
	CHECK(now_ns() - before >= 1500000000);
	sync_empty(fd, t);
	CHECK(close(t->sw) == 0);

	t->sw = sync_import_pending(fd, t->input);
	struct sigaction action = { .sa_handler = sync_interrupt };
	CHECK(sigemptyset(&action.sa_mask) == 0);
	CHECK(sigaction(SIGUSR1, &action, NULL) == 0);
	pid_t parent = getpid(), child = fork();
	CHECK(child >= 0);
	if (child == 0) { usleep(150000); CHECK(kill(parent, SIGUSR1) == 0); _exit(0); }
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, EINTR);
	CHECK(sync_interrupted == 1);
	sync_join(child);
	sync_empty(fd, t);
	CHECK(close(t->sw) == 0);
	printf("G17P_SYNC_REJECTION_PASS malformed, missing, errored, timed-out and interrupted inputs; outputs untouched\n");
}
static void sync_before(int fd, struct sync_test *t, unsigned n, uint32_t vm,
			const unsigned char *output)
{
	t->entries[3].timeline_value = n + 1;
	if (n != 0) {
		t->entries[0].handle = t->binary;
		t->entries[1] = (struct drm_asahi_sync){ .sync_type = DRM_ASAHI_SYNC_TIMELINE_SYNCOBJ,
			.handle = t->timeline, .timeline_value = n > 1 ? n - 1 : n };
		return;
	}
	t->sw = sync_import_pending(fd, t->input);
	t->started = now_ns();
	t->child = fork(); CHECK(t->child >= 0);
	if (t->child == 0) {
		usleep(150000);
		for (unsigned i = 0; i < PAGE; i++) CHECK(output[i] == 0xa5);
		/* Uses the same file's state lock. This must work while SUBMIT
		 * waits for our dependency. Queue IDs are not reused. */
		struct drm_asahi_queue_create q = { .vm_id = vm, .usc_exec_base = EXEC };
		OK(fd, DRM_IOCTL_ASAHI_QUEUE_CREATE, &q);
		struct drm_asahi_queue_destroy d = { .queue_id = q.queue_id };
		OK(fd, DRM_IOCTL_ASAHI_QUEUE_DESTROY, &d);
		uint32_t increment = 1;
		OK(t->sw, SW_INC, &increment);
		_exit(0);
	}
}
static void sync_after(int fd, struct sync_test *t, unsigned n)
{
	if (n == 0) {
		CHECK(now_ns() - t->started >= 100000000);
		sync_join(t->child);
		CHECK(close(t->sw) == 0);
	}
	struct drm_syncobj_wait w = { .handles = (uintptr_t)&t->binary, .count_handles = 1 };
	OK(fd, DRM_IOCTL_SYNCOBJ_WAIT, &w);
	uint64_t point = n + 1;
	struct drm_syncobj_timeline_wait tw = { .handles = (uintptr_t)&t->timeline,
		.points = (uintptr_t)&point, .count_handles = 1 };
	OK(fd, DRM_IOCTL_SYNCOBJ_TIMELINE_WAIT, &tw);
	CHECK(sync_point(fd, t->timeline) == point);
	uint64_t stamp = sync_stamp(fd, t->binary);
	CHECK(stamp > t->last_stamp);
	t->last_stamp = stamp;
}
static inline void sync_finish(int fd, struct sync_test *t)
{
	CHECK(sync_stamp(fd, t->binary) == t->last_stamp);
	CHECK(sync_point(fd, t->timeline) == 32);
	uint32_t handles[] = { t->input, t->seed, t->binary, t->timeline };
	for (unsigned i = 0; i < 4; i++) {
		struct drm_syncobj_destroy d = { .handle = handles[i] };
		OK(fd, DRM_IOCTL_SYNCOBJ_DESTROY, &d);
	}
	printf("G17P_SYNC_COMPLETION_PASS 32 binary and timeline fences, imported wait, same-file progress, rejected work preserves fences\n");
}
