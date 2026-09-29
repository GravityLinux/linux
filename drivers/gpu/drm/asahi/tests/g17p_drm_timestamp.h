/* SPDX-License-Identifier: MIT */
/* Firmware writes into an offset GEM object range, with live object rebind. */
struct timestamp_test {
	uint32_t bo, object;
	unsigned char *map;
	uint64_t saved[258][2];
};
static void timestamp_bind(int fd, struct timestamp_test *t)
{
	struct drm_asahi_gem_bind_object obj = { .op = DRM_ASAHI_BIND_OBJECT_OP_BIND,
		.flags = DRM_ASAHI_BIND_OBJECT_USAGE_TIMESTAMPS,
		.handle = t->bo, .offset = PAGE, .range = PAGE };
	OK(fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &obj);
	t->object = obj.object_handle;
}
static void timestamp_unbind(int fd, struct timestamp_test *t)
{
	struct drm_asahi_gem_bind_object obj = { .op = DRM_ASAHI_BIND_OBJECT_OP_UNBIND,
		.object_handle = t->object };
	OK(fd, DRM_IOCTL_ASAHI_GEM_BIND_OBJECT, &obj);
}
static void timestamp_setup(int fd, struct timestamp_test *t,
		struct drm_asahi_cmd_compute *cmd, struct drm_asahi_submit *submit)
{
	t->bo = bo_new(fd, PAGE * 3, DRM_ASAHI_GEM_WRITEBACK, 0);
	t->map = bo_map(fd, t->bo, PAGE * 3);
	memset(t->map, 0xa5, PAGE * 3);
	timestamp_bind(fd, t);
	cmd->ts.start.handle = UINT32_MAX;
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, ENOENT);
	cmd->ts.start.handle = t->object; cmd->ts.start.offset = 1;
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, EINVAL);
	cmd->ts.start.offset = PAGE;
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, EINVAL);
	cmd->ts.start.handle = 0; cmd->ts.start.offset = 8;
	BAD(fd, DRM_IOCTL_ASAHI_SUBMIT, submit, EINVAL);
	cmd->ts.start.handle = t->object; cmd->ts.start.offset = 64;
	cmd->ts.end.handle = t->object; cmd->ts.end.offset = 72;
	for (unsigned i = 0; i < PAGE * 3; i++) CHECK(t->map[i] == 0xa5);
}
static void timestamp_before(int fd, struct timestamp_test *t,
		struct drm_asahi_cmd_compute *cmd, unsigned n)
{
	if (n == 16) {
		timestamp_unbind(fd, t);
		timestamp_bind(fd, t);
	}
	cmd->ts.start.handle = cmd->ts.end.handle = t->object;
	cmd->ts.start.offset = 64 + n * 16;
	cmd->ts.end.offset = 72 + n * 16;
}
static void timestamp_check(struct timestamp_test *t, unsigned count)
{
	for (unsigned i = 0; i < PAGE * 3; i++) {
		if (i >= PAGE + 64 && i < PAGE + 64 + count * 16) continue;
		CHECK(t->map[i] == 0xa5);
	}
	for (unsigned i = 0; i < count; i++)
		CHECK(!memcmp(t->map + PAGE + 64 + i * 16, t->saved[i], 16));
}
static inline void timestamp_after(struct timestamp_test *t, unsigned n)
{
	memcpy(t->saved[n], t->map + PAGE + 64 + n * 16, 16);
	uint64_t start = t->saved[n][0], end = t->saved[n][1];
	printf("TIMESTAMP %u start=%" PRIu64 " end=%" PRIu64 "\n", n, start, end);
	CHECK(start != 0 && end > start);
	if (n) CHECK(start >= t->saved[n - 1][1]);
	timestamp_check(t, n + 1);
}
static void timestamp_finish(int fd, struct timestamp_test *t, unsigned count)
{
	timestamp_check(t, count);
	timestamp_unbind(fd, t);
	CHECK(munmap(t->map, 3 * PAGE) == 0);
	bo_close(fd, t->bo);
	printf("G17P_TIMESTAMP_PASS %u ordered start/end pairs, live object rebind, offset and guard bytes intact\n", count);
}
