// SPDX-License-Identifier: GPL-2.0-only OR MIT
/* Copyright 2023 */

#include <linux/completion.h>
#include <linux/dma-mapping.h>

#include <drm/drm_fourcc.h>

#include "afk.h"
#include "dcp.h"

#define IBOOT_SET_SURFACE 1
#define IBOOT_ADDR_PLANAR 1
#define IBOOT_FMT_BGRA 1
#define IBOOT_FMT_RGBA 3
#define IBOOT_FMT_W30R 9
#define IBOOT_EOTF_GAMMA_SDR 1

struct iboot_plane {
	__le32 unk1;
	__le64 addr;
	__le32 tile_size;
	__le32 stride;
	__le32 unk2[4];
	__le32 addr_format;
	__le32 unk3;
} __packed;

struct iboot_layer {
	struct iboot_plane planes[3];
	__le32 unk;
	__le32 plane_cnt;
	__le32 width;
	__le32 height;
	__le32 surface_fmt;
	__le32 colorspace;
	__le32 eotf;
	u8 transform;
	u8 padding[3];
} __packed;

struct iboot_set_surface_cmd {
	struct iboot_layer layer;
	__le32 unk3;
	__le32 unk4;
} __packed;

struct iboot_cmd {
	__le32 op;
	__le32 len;
	__le32 unk1;
	__le32 unk2;
	struct iboot_set_surface_cmd payload;
} __packed;

static void disp_service_init(struct apple_epic_service *service, const char *name,
			const char *class, s64 unit)
{
	service->ep->dcp->iboot_service = service;
}


static const struct apple_epic_service_ops ibootep_ops[] = {
	{
		.name = "disp0-service",
		.init = disp_service_init,
	},
	{}
};

int ibootep_init(struct apple_dcp *dcp)
{
	dcp->ibootep = afk_init(dcp, DISP0_ENDPOINT, ibootep_ops);
	afk_start(dcp->ibootep);

	return 0;
}

int ibootep_set_surface(struct apple_dcp *dcp, dma_addr_t iova, u32 stride,
			u32 width, u32 height, u32 drm_format)
{
	struct iboot_cmd cmd = { 0 };
	dma_addr_t surface_iova = iova & DMA_BIT_MASK(36);
	u32 retcode = 0;
	u32 surface_fmt;
	int ret;

	if (!dcp->iboot_service)
		return -ENODEV;

	switch (drm_format) {
	case DRM_FORMAT_XRGB8888:
	case DRM_FORMAT_ARGB8888:
		/* The retained 26.6 pipeline is configured for the bootloader's
		 * 10-bit packed scanout. Keep the first Linux prototype on that
		 * proven path; the storage remains four bytes per pixel. */
		surface_fmt = IBOOT_FMT_W30R;
		break;
	case DRM_FORMAT_XBGR8888:
	case DRM_FORMAT_ABGR8888:
		surface_fmt = IBOOT_FMT_RGBA;
		break;
	case DRM_FORMAT_XRGB2101010:
		surface_fmt = IBOOT_FMT_W30R;
		break;
	default:
		return -EINVAL;
	}

	cmd.op = cpu_to_le32(IBOOT_SET_SURFACE);
	cmd.len = cpu_to_le32(sizeof(cmd));
	/* The compact service consumes the DART-relative address. The upper
	 * apple,dma-range remap bits belong to the AP DMA API, not the layer. */
	cmd.payload.layer.planes[0].addr = cpu_to_le64(surface_iova);
	cmd.payload.layer.planes[0].stride = cpu_to_le32(stride);
	cmd.payload.layer.planes[0].addr_format =
		cpu_to_le32(IBOOT_ADDR_PLANAR);
	cmd.payload.layer.plane_cnt = cpu_to_le32(1);
	cmd.payload.layer.width = cpu_to_le32(width);
	cmd.payload.layer.height = cpu_to_le32(height);
	cmd.payload.layer.surface_fmt = cpu_to_le32(surface_fmt);
	/* The compact interface uses the display colorimetry enum rather than
	 * IOMFB's colorspace enum.  Value 1 is the SDR BT.601/709 family. */
	cmd.payload.layer.colorspace = cpu_to_le32(2);
	cmd.payload.layer.eotf = cpu_to_le32(IBOOT_EOTF_GAMMA_SDR);

	ret = afk_send_command(dcp->iboot_service, EPIC_SUBTYPE_STD_SERVICE,
			       &cmd, sizeof(cmd), NULL, 0, &retcode);
	if (ret)
		return ret;
	if (retcode)
		return -EIO;

	return 0;
}
