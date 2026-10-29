// SPDX-License-Identifier: GPL-2.0-only OR MIT
#include <drm.h>
#include <drm_mode.h>
#include <drm_fourcc.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

static void fail(const char *what)
{
	fprintf(stderr, "DCP_FAIL %s: %s\n", what, strerror(errno)); exit(1);
}
struct buffer { uint32_t handle, fb; uint64_t size; void *map; };
static struct buffer create(int fd, unsigned width, unsigned height, uint32_t color)
{
	struct drm_mode_create_dumb c = { .width = width, .height = height, .bpp = 32 };
	if (ioctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &c)) fail("CREATE_DUMB");
	struct drm_mode_fb_cmd2 fb = { .width = width, .height = height,
		.pixel_format = DRM_FORMAT_XRGB8888, .handles = {c.handle}, .pitches = {c.pitch} };
	if (ioctl(fd, DRM_IOCTL_MODE_ADDFB2, &fb)) fail("ADDFB2");
	struct drm_mode_map_dumb m = { .handle = c.handle };
	if (ioctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &m)) fail("MAP_DUMB");
	void *map = mmap(NULL, c.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, m.offset);
	if (map == MAP_FAILED) fail("mmap");
	for (unsigned y = 0; y < height; y++) {
		uint32_t *row = (void *)((char *)map + y * c.pitch);
		for (unsigned x = 0; x < width; x++)
			row[x] = ((x / 128 + y / 128) % 2) ? color : 0xffffffff;
	}
	__atomic_thread_fence(__ATOMIC_SEQ_CST);
	printf("DCP_BUFFER fb=%u handle=%u pitch=%u bytes=%llu color=%08x\n",
	       fb.fb_id, c.handle, c.pitch, (unsigned long long)c.size, color);
	return (struct buffer){ c.handle, fb.fb_id, c.size, map };
}
int main(int argc, char **argv)
{
	setvbuf(stdout, NULL, _IOLBF, 0);
	const char *path = argc > 1 ? argv[1] : "/dev/dri/card0";
	int fd = open(path, O_RDWR | O_CLOEXEC);
	if (fd < 0) fail("open card");
	char name[64] = {0}; struct drm_version ver = { .name = name, .name_len = sizeof(name) - 1 };
	if (ioctl(fd, DRM_IOCTL_VERSION, &ver)) fail("VERSION");
	printf("DCP_DRIVER %s\n", name);
	if (strcmp(name, "apple")) { fprintf(stderr, "DCP_FAIL unexpected driver\n"); return 1; }
	if (ioctl(fd, DRM_IOCTL_SET_MASTER, NULL)) fail("SET_MASTER");
	struct drm_mode_card_res res = {0};
	if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res)) fail("GETRESOURCES counts");
	uint32_t *crtcs = calloc(res.count_crtcs, sizeof(uint32_t));
	uint32_t *conns = calloc(res.count_connectors, sizeof(uint32_t));
	if (!crtcs || !conns || !res.count_crtcs || !res.count_connectors) {
		fprintf(stderr, "DCP_FAIL no connectors/CRTCs\n"); return 2;
	}
	res.crtc_id_ptr = (uintptr_t)crtcs; res.connector_id_ptr = (uintptr_t)conns;
	res.count_fbs = 0; res.count_encoders = 0;
	if (ioctl(fd, DRM_IOCTL_MODE_GETRESOURCES, &res)) fail("GETRESOURCES ids");
	uint32_t connector = 0, crtc = 0; struct drm_mode_modeinfo mode = {0};
	for (unsigned i = 0; i < res.count_connectors; i++) {
		struct drm_mode_get_connector c = { .connector_id = conns[i] };
		if (ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, &c)) fail("GETCONNECTOR counts");
		printf("DCP_CONNECTOR id=%u type=%u connection=%u modes=%u\n",
		       c.connector_id, c.connector_type, c.connection, c.count_modes);
		if (c.connection != 1 || !c.count_modes) continue;
		struct drm_mode_modeinfo *modes = calloc(c.count_modes, sizeof(*modes));
		uint32_t *encoders = calloc(c.count_encoders ? c.count_encoders : 1, sizeof(uint32_t));
		if (!modes || !encoders) fail("calloc");
		c.modes_ptr = (uintptr_t)modes; c.encoders_ptr = (uintptr_t)encoders;
		c.count_props = 0;
		if (ioctl(fd, DRM_IOCTL_MODE_GETCONNECTOR, &c)) fail("GETCONNECTOR modes");
		if (c.connection != 1 || !c.count_modes) { free(modes); free(encoders); continue; }
		mode = modes[0];
		for (unsigned j = 0; j < c.count_modes; j++)
			if (modes[j].type & DRM_MODE_TYPE_PREFERRED) { mode = modes[j]; break; }
		uint32_t eid = c.encoder_id ? c.encoder_id : (c.count_encoders ? encoders[0] : 0);
		struct drm_mode_get_encoder e = { .encoder_id = eid };
		if (!eid || ioctl(fd, DRM_IOCTL_MODE_GETENCODER, &e)) fail("GETENCODER");
		for (unsigned j = 0; j < res.count_crtcs && j < 32; j++)
			if (e.possible_crtcs & (1u << j)) { crtc = crtcs[j]; break; }
		connector = c.connector_id; free(modes); free(encoders); break;
	}
	if (!connector || !crtc) { fprintf(stderr, "DCP_FAIL no connected usable output\n"); return 2; }
	printf("DCP_MODE connector=%u crtc=%u mode=%s %ux%u\n",
	       connector, crtc, mode.name, mode.hdisplay, mode.vdisplay);
	struct buffer bufs[2] = { create(fd, mode.hdisplay, mode.vdisplay, 0xffff0000),
		create(fd, mode.hdisplay, mode.vdisplay, 0xff0000ff) };
	struct drm_mode_crtc c = { .set_connectors_ptr = (uintptr_t)&connector,
		.count_connectors = 1, .crtc_id = crtc, .fb_id = bufs[0].fb,
		.mode_valid = 1, .mode = mode };
	if (ioctl(fd, DRM_IOCTL_MODE_SETCRTC, &c)) fail("SETCRTC");
	printf("DCP_MODESET_PASS alternating red/white and blue/white checkerboards\n");
	for (unsigned frame = 1; frame <= 12; frame++) {
		usleep(400000);
		struct drm_mode_crtc_page_flip f = { .crtc_id = crtc, .fb_id = bufs[frame % 2].fb,
			.flags = DRM_MODE_PAGE_FLIP_EVENT, .user_data = frame };
		if (ioctl(fd, DRM_IOCTL_MODE_PAGE_FLIP, &f)) fail("PAGE_FLIP");
		struct pollfd p = { .fd = fd, .events = POLLIN };
		if (poll(&p, 1, 3000) != 1) { fprintf(stderr, "DCP_FAIL flip event timeout\n"); return 1; }
		char events[4096]; ssize_t n = read(fd, events, sizeof(events)); int found = 0;
		if (n <= 0) fail("read flip event");
		for (ssize_t off = 0; off + (ssize_t)sizeof(struct drm_event) <= n;) {
			struct drm_event *e = (void *)(events + off);
			if (e->length < sizeof(*e) || (size_t)off + e->length > (size_t)n) return 1;
			if (e->type == DRM_EVENT_FLIP_COMPLETE && e->length >= sizeof(struct drm_event_vblank)) {
				struct drm_event_vblank *v = (void *)e;
				if (v->user_data == frame) found = 1;
			}
			off += e->length;
		}
		if (!found) { fprintf(stderr, "DCP_FAIL unmatched flip event\n"); return 1; }
		printf("DCP_FLIP_PASS frame=%u fb=%u\n", frame, f.fb_id);
	}
	printf("DCP_PAGE_FLIP_TEST_PASS flips=12; holding final checkerboard for visual confirmation\n");
	for (;;) pause();
}
