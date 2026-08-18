// SPDX-License-Identifier: GPL-2.0-only OR MIT
/* Copyright 2021 Alyssa Rosenzweig */
/* Based on meson driver which is
 * Copyright (C) 2016 BayLibre, SAS
 * Author: Neil Armstrong <narmstrong@baylibre.com>
 * Copyright (C) 2015 Amlogic, Inc. All rights reserved.
 * Copyright (C) 2014 Endless Mobile
 */

#include <linux/aperture.h>
#include <linux/component.h>
#include <linux/delay.h>
#include <linux/dma-direct.h>
#include <linux/dma-mapping.h>
#include <linux/jiffies.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/of_address.h>
#include <linux/of_device.h>
#include <linux/of_graph.h>
#include <linux/of_platform.h>
#include <linux/of_reserved_mem.h>
#include <linux/set_memory.h>
#include <linux/scatterlist.h>

#include <asm/cacheflush.h>
#include <asm/tlbflush.h>

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_blend.h>
#include <drm/clients/drm_client_setup.h>
#include <drm/drm_crtc.h>
#include <drm/drm_drv.h>
#include <drm/drm_fb_helper.h>
#include <drm/drm_fbdev_dma.h>
#include <drm/drm_fb_dma_helper.h>
#include <drm/drm_gem_dma_helper.h>
#include <drm/drm_gem_framebuffer_helper.h>
#include <drm/drm_simple_kms_helper.h>
#include <drm/drm_mode.h>
#include <drm/drm_modeset_helper.h>
#include <drm/drm_module.h>
#include <drm/drm_of.h>
#include <drm/drm_print.h>
#include <drm/drm_probe_helper.h>
#include <drm/drm_vblank.h>
#include <drm/drm_fixed.h>

#include "dcp.h"
#include "plane.h"

#define DRIVER_NAME     "apple"
#define DRIVER_DESC     "Apple display controller DRM driver"

#define MAX_COPROCESSORS 3

struct apple_drm_private {
	struct drm_device drm;
	struct resource scanout_pool;
	bool scanout_pool_attached;
};

struct apple_drm_gem_object {
	struct drm_gem_dma_object dma;
	struct sg_table *scanout_sgt;
	dma_addr_t pool_dma_addr;
	bool pool_fixed_iova;
};

static void apple_drm_gem_restore_direct_map(struct apple_drm_gem_object *obj)
{
	struct scatterlist *sg;
	unsigned int i;

	if (!obj->scanout_sgt)
		return;

	for_each_sg(obj->scanout_sgt->sgl, sg,
		    obj->scanout_sgt->orig_nents, i) {
		struct page *page = sg_page(sg);
		unsigned int nr_pages = PAGE_ALIGN(sg->offset + sg->length) >> PAGE_SHIFT;
		unsigned int j;

		for (j = 0; j < nr_pages; j++)
			set_direct_map_default_noflush(page + j);
	}

	flush_tlb_all();
	sg_free_table(obj->scanout_sgt);
	kfree(obj->scanout_sgt);
	obj->scanout_sgt = NULL;
}

static void apple_drm_gem_free(struct drm_gem_object *gem_obj)
{
	struct apple_drm_gem_object *obj =
		container_of(to_drm_gem_dma_obj(gem_obj),
			     struct apple_drm_gem_object, dma);

	if (obj->pool_fixed_iova) {
		obj->dma.dma_addr = obj->pool_dma_addr;
		obj->pool_fixed_iova = false;
	}

	apple_drm_gem_restore_direct_map(obj);
	drm_gem_dma_free(&obj->dma);
}

static void apple_drm_gem_print_info(struct drm_printer *p,
				     unsigned int indent,
				     const struct drm_gem_object *gem_obj)
{
	const struct apple_drm_gem_object *obj =
		container_of(to_drm_gem_dma_obj(gem_obj),
			     struct apple_drm_gem_object, dma);
	phys_addr_t run_start = 0, run_end = 0;
	struct scatterlist *sg;
	unsigned int i;

	drm_gem_dma_object_print_info(p, indent, gem_obj);
	if (!obj->scanout_sgt)
		return;

	drm_printf_indent(p, indent, "scanout physical backing:\n");
	for_each_sg(obj->scanout_sgt->sgl, sg,
		    obj->scanout_sgt->orig_nents, i) {
		phys_addr_t start = sg_phys(sg);
		phys_addr_t end = start + sg->length;

		if (!run_end) {
			run_start = start;
			run_end = end;
			continue;
		}
		if (start == run_end) {
			run_end = end;
			continue;
		}

		drm_printf_indent(p, indent + 1, "%pa..%pa (%pa bytes)\n",
				  &run_start, &run_end,
				  &(phys_addr_t){ run_end - run_start });
		run_start = start;
		run_end = end;
	}
	if (run_end)
		drm_printf_indent(p, indent + 1, "%pa..%pa (%pa bytes)\n",
				  &run_start, &run_end,
				  &(phys_addr_t){ run_end - run_start });
}

static const struct drm_gem_object_funcs apple_drm_gem_funcs = {
	.free = apple_drm_gem_free,
	.print_info = apple_drm_gem_print_info,
	.get_sg_table = drm_gem_dma_object_get_sg_table,
	.vmap = drm_gem_dma_object_vmap,
	.mmap = drm_gem_dma_object_mmap,
	.vm_ops = &drm_gem_dma_vm_ops,
};

static struct drm_gem_object *
apple_drm_gem_create_object(struct drm_device *drm, size_t size)
{
	struct apple_drm_gem_object *obj;

	obj = kzalloc_obj(*obj);
	if (!obj)
		return ERR_PTR(-ENOMEM);

	obj->dma.base.funcs = &apple_drm_gem_funcs;
	return &obj->dma.base;
}

static int apple_drm_gem_remove_direct_map(struct drm_gem_dma_object *dma_obj)
{
	struct apple_drm_gem_object *obj =
		container_of(dma_obj, struct apple_drm_gem_object, dma);
	struct scatterlist *sg;
	unsigned int i;
	int ret = 0;

	if (!can_set_direct_map())
		return -EOPNOTSUPP;

	obj->scanout_sgt = drm_gem_dma_get_sg_table(dma_obj);
	if (IS_ERR(obj->scanout_sgt)) {
		ret = PTR_ERR(obj->scanout_sgt);
		obj->scanout_sgt = NULL;
		return ret;
	}

	for_each_sg(obj->scanout_sgt->sgl, sg,
		    obj->scanout_sgt->orig_nents, i) {
		struct page *page = sg_page(sg);
		unsigned int nr_pages = PAGE_ALIGN(sg->offset + sg->length) >> PAGE_SHIFT;
		unsigned int j;

		for (j = 0; j < nr_pages; j++) {
			unsigned long start = (unsigned long)page_address(page + j);

			/* dma_alloc_wc() cleans the allocator's WB alias, but a clean
			 * can leave an AMCC cache-directory entry behind.  DCP scanout
			 * is realtime, so retire the entry before removing that alias. */
			dcache_clean_inval_poc(start, start + PAGE_SIZE);
			ret = set_direct_map_invalid_noflush(page + j);
			if (ret)
				goto err_restore;
		}
	}

	flush_tlb_all();
	return 0;

err_restore:
	apple_drm_gem_restore_direct_map(obj);
	return ret;
}

static int apple_drm_gem_map_nomap_pool(struct drm_gem_dma_object *dma_obj)
{
	struct apple_drm_gem_object *obj =
		container_of(dma_obj, struct apple_drm_gem_object, dma);
	struct apple_drm_private *apple =
		container_of(dma_obj->base.dev, struct apple_drm_private, drm);
	struct device *dev = dma_obj->base.dev->dev;
	dma_addr_t pool_dma = dma_obj->dma_addr;
	phys_addr_t phys = dma_to_phys(dev, pool_dma);
	dma_addr_t iova;
	u64 iova_base;
	resource_size_t size = dma_obj->base.size;

	if (!apple->scanout_pool_attached ||
	    phys < apple->scanout_pool.start ||
	    size > resource_size(&apple->scanout_pool) ||
	    phys - apple->scanout_pool.start >
		resource_size(&apple->scanout_pool) - size) {
		dev_err(dev,
			"scanout allocation %pa+%pa is outside no-map pool %pr\n",
			&phys, &size, &apple->scanout_pool);
		return -ERANGE;
	}

	if (of_property_read_u64(dev->of_node, "apple,scanout-iova-base",
				 &iova_base))
		return -EINVAL;

	/* m1n1 installs this entire mapping before Linux and leaves the locked
	 * realtime DART hierarchy untouched. Selecting a buffer is therefore an
	 * offset calculation, not a Linux page-table update. */
	iova = iova_base + (phys - apple->scanout_pool.start);

	obj->pool_dma_addr = pool_dma;
	obj->pool_fixed_iova = true;
	dma_obj->dma_addr = iova;
	dev_info(dev, "pre-mapped no-map scanout buffer: phys %pa -> iova %pad (%zu bytes)\n",
		 &phys, &iova, dma_obj->base.size);
	return 0;
}

DEFINE_DRM_GEM_DMA_FOPS(apple_fops);

#define DART_PAGE_SIZE 16384

static struct drm_gem_dma_object *
apple_drm_gem_create_scanout(struct drm_device *drm, size_t size)
{
	struct drm_gem_dma_object *dma_obj;
	int ret;

	dma_obj = drm_gem_dma_create(drm, round_up(size, DART_PAGE_SIZE));
	if (IS_ERR(dma_obj))
		return dma_obj;

	if (of_property_read_bool(drm->dev->of_node,
				  "apple,scanout-memory-is-nomap"))
		ret = apple_drm_gem_map_nomap_pool(dma_obj);
	else
		ret = apple_drm_gem_remove_direct_map(dma_obj);
	if (ret) {
		drm_gem_object_put(&dma_obj->base);
		return ERR_PTR(ret);
	}

	return dma_obj;
}

static int apple_drm_gem_dumb_create(struct drm_file *file_priv,
                            struct drm_device *drm,
                            struct drm_mode_create_dumb *args)
{
	struct drm_gem_dma_object *dma_obj;
	int ret;

        args->pitch = ALIGN(DIV_ROUND_UP(args->width * args->bpp, 8), 64);
        args->size = round_up(args->pitch * args->height, DART_PAGE_SIZE);

	dma_obj = apple_drm_gem_create_scanout(drm, args->size);
	if (IS_ERR(dma_obj))
		return PTR_ERR(dma_obj);

	ret = drm_gem_handle_create(file_priv, &dma_obj->base, &args->handle);

	drm_gem_object_put(&dma_obj->base);
	return ret;
}

static const struct drm_driver apple_drm_driver = {
	DRM_GEM_DMA_DRIVER_OPS_VMAP_WITH_DUMB_CREATE(apple_drm_gem_dumb_create),
	DRM_FBDEV_DMA_DRIVER_OPS,
	.gem_create_object	= apple_drm_gem_create_object,
	.name			= DRIVER_NAME,
	.desc			= DRIVER_DESC,
	.major			= 1,
	.minor			= 0,
	.driver_features	= DRIVER_MODESET | DRIVER_GEM | DRIVER_ATOMIC | DRIVER_SYNCOBJ | DRIVER_SYNCOBJ_TIMELINE,
	.fops			= &apple_fops,
};

static enum drm_connector_status
apple_connector_detect(struct drm_connector *connector, bool force)
{
	struct apple_connector *apple_connector = to_apple_connector(connector);

	return apple_connector->connected ? connector_status_connected :
						  connector_status_disconnected;
}

static void apple_connector_oob_hotplug(struct drm_connector *connector,
					enum drm_connector_status status)
{
	struct apple_connector *apple_connector = to_apple_connector(connector);

	printk("#### oob_hotplug status:0x%x ####\n", (u32)status);

	if (status == connector_status_connected)
		dcp_dptx_connect_oob(apple_connector->dcp, 0);
	else if (status == connector_status_disconnected)
		dcp_dptx_disconnect_oob(apple_connector->dcp, 0);
	else
		dev_err(&apple_connector->dcp->dev, "unexpected connector status"
			":0x%x in oob_hotplug event\n", (u32)status);
}

static void apple_crtc_atomic_enable(struct drm_crtc *crtc,
				     struct drm_atomic_state *state)
{
	struct drm_crtc_state *crtc_state;
	crtc_state = drm_atomic_get_new_crtc_state(state, crtc);

	if (crtc_state->active_changed && crtc_state->active) {
		struct apple_crtc *apple_crtc = to_apple_crtc(crtc);
		dcp_poweron(apple_crtc->dcp);
		/* Force the CTM to be set on first swap */
		crtc_state->color_mgmt_changed = true;
	}

	if (crtc_state->active)
		dcp_crtc_atomic_modeset(crtc, state);
}

static void apple_crtc_atomic_disable(struct drm_crtc *crtc,
				      struct drm_atomic_state *state)
{
	struct drm_crtc_state *crtc_state;
	crtc_state = drm_atomic_get_new_crtc_state(state, crtc);

	if (crtc_state->active_changed && !crtc_state->active) {
		struct apple_crtc *apple_crtc = to_apple_crtc(crtc);
		dcp_poweroff(apple_crtc->dcp);
	}

	if (crtc->state->event && !crtc->state->active) {
		spin_lock_irq(&crtc->dev->event_lock);
		drm_crtc_send_vblank_event(crtc, crtc->state->event);
		spin_unlock_irq(&crtc->dev->event_lock);

		crtc->state->event = NULL;
	}
}

static void apple_crtc_atomic_begin(struct drm_crtc *crtc,
				    struct drm_atomic_state *state)
{
	struct apple_crtc *apple_crtc = to_apple_crtc(crtc);
	unsigned long flags;

	if (crtc->state->event) {
		spin_lock_irqsave(&crtc->dev->event_lock, flags);
		apple_crtc->event = crtc->state->event;
		spin_unlock_irqrestore(&crtc->dev->event_lock, flags);
		crtc->state->event = NULL;
	}
}

static void apple_crtc_cleanup(struct drm_crtc *crtc)
{
	drm_crtc_cleanup(crtc);
	kfree(to_apple_crtc(crtc));
}

static int apple_crtc_parse_crc_source(const char *source, bool *enabled)
{
	int ret = 0;

	if (!source) {
		*enabled = false;
	} else if (strcmp(source, "auto") == 0) {
		*enabled = true;
	} else {
		*enabled = false;
		ret = -EINVAL;
	}

	return ret;
}

static int apple_crtc_set_crc_source(struct drm_crtc *crtc, const char *source)
{
	bool enabled = false;

	int ret = apple_crtc_parse_crc_source(source, &enabled);

	if (!ret)
		dcp_set_crc(crtc, enabled);

	return ret;
}

static int apple_crtc_verify_crc_source(struct drm_crtc *crtc,
					const char *source,
					size_t *values_cnt)
{
	bool enabled;

	if (apple_crtc_parse_crc_source(source, &enabled) < 0) {
		pr_warn("dcp: Invalid CRC source name %s\n", source);
		return -EINVAL;
	}

	*values_cnt = 1;

	return 0;
}

static const char * const apple_crtc_crc_sources[] = {"auto"};

static const char *const * apple_crtc_get_crc_sources(struct drm_crtc *crtc,
						      size_t *count)
{
	*count = ARRAY_SIZE(apple_crtc_crc_sources);
	return apple_crtc_crc_sources;
}

static const struct drm_crtc_funcs apple_crtc_funcs = {
	.atomic_destroy_state	= drm_atomic_helper_crtc_destroy_state,
	.atomic_duplicate_state = drm_atomic_helper_crtc_duplicate_state,
	.destroy		= apple_crtc_cleanup,
	.page_flip		= drm_atomic_helper_page_flip,
	.reset			= drm_atomic_helper_crtc_reset,
	.set_config             = drm_atomic_helper_set_config,
	.set_crc_source		= apple_crtc_set_crc_source,
	.verify_crc_source	= apple_crtc_verify_crc_source,
	.get_crc_sources	= apple_crtc_get_crc_sources,

};

static const struct drm_mode_config_funcs apple_mode_config_funcs = {
	.atomic_check		= drm_atomic_helper_check,
	.atomic_commit		= drm_atomic_helper_commit,
	.fb_create		= drm_gem_fb_create,
};

static const struct drm_mode_config_helper_funcs apple_mode_config_helpers = {
	.atomic_commit_tail	= drm_atomic_helper_commit_tail_rpm,
};

static void appledrm_connector_cleanup(struct drm_connector *connector)
{
	drm_connector_cleanup(connector);
	kfree(to_apple_connector(connector));
}

static const struct drm_connector_funcs apple_connector_funcs = {
	.fill_modes		= drm_helper_probe_single_connector_modes,
	.destroy		= appledrm_connector_cleanup,
	.reset			= drm_atomic_helper_connector_reset,
	.atomic_duplicate_state	= drm_atomic_helper_connector_duplicate_state,
	.atomic_destroy_state	= drm_atomic_helper_connector_destroy_state,
	.detect			= apple_connector_detect,
	.debugfs_init		= apple_connector_debugfs_init,
	.oob_hotplug_event	= apple_connector_oob_hotplug,
};

static const struct drm_connector_helper_funcs apple_connector_helper_funcs = {
	.get_modes		= dcp_get_modes,
	.mode_valid		= dcp_mode_valid,
};

static const struct drm_crtc_helper_funcs apple_crtc_helper_funcs = {
	.atomic_begin		= apple_crtc_atomic_begin,
	.atomic_check		= dcp_crtc_atomic_check,
	.atomic_flush		= dcp_flush,
	.atomic_enable		= apple_crtc_atomic_enable,
	.atomic_disable		= apple_crtc_atomic_disable,
	.mode_fixup		= dcp_crtc_mode_fixup,
};

static int apple_probe_per_dcp(struct device *dev,
			       struct drm_device *drm,
			       struct platform_device *dcp,
			       int num, bool dcp_ext)
{
	struct apple_crtc *crtc;
	struct apple_connector *connector;
	struct apple_encoder *enc;
	struct drm_plane *primary_plane = NULL;
	struct drm_plane *planes[DCP_MAX_PLANES];
	unsigned long *iomfb_surfaces = dcp_get_iomfb_surfaces(dcp);
	u32 surface_order[DCP_MAX_PLANES];
	DECLARE_BITMAP(ordered_surfaces, DCP_MAX_PLANES);
	int surface_count;
	int order_count;
	int ret;
	u32 primary_surface;
	u32 surf;
	int zpos = 0;
	bool supports_l10r = !dcp_fw_compat_is_12_x(dcp);
	enum drm_plane_type plane_type;

	surface_count = bitmap_weight(iomfb_surfaces, DCP_MAX_PLANES);
	order_count = of_property_count_u32_elems(dcp->dev.of_node,
						  "apple,iomfb-surface-order");
	if (order_count >= 0) {
		if (order_count != surface_count) {
			dev_err(dev, "iomfb surface order has %d entries, expected %d\n",
				order_count, surface_count);
			return -EINVAL;
		}
		ret = of_property_read_u32_array(dcp->dev.of_node,
						 "apple,iomfb-surface-order",
						 surface_order, surface_count);
		if (ret)
			return ret;
	} else {
		for_each_set_bit(surf, iomfb_surfaces, DCP_MAX_PLANES)
			surface_order[zpos++] = surf;
	}
	primary_surface = surface_order[0];
	of_property_read_u32(dcp->dev.of_node, "apple,iomfb-primary-surface",
			     &primary_surface);
	if (primary_surface >= DCP_MAX_PLANES ||
	    !test_bit(primary_surface, iomfb_surfaces)) {
		dev_err(dev, "invalid primary iomfb surface %u\n",
			primary_surface);
		return -EINVAL;
	}

	bitmap_zero(ordered_surfaces, DCP_MAX_PLANES);
	for (zpos = 0; zpos < surface_count; zpos++) {
		surf = surface_order[zpos];
		if (surf >= DCP_MAX_PLANES ||
		    !test_bit(surf, iomfb_surfaces) ||
		    test_and_set_bit(surf, ordered_surfaces)) {
			dev_err(dev, "invalid iomfb surface %u at zpos %d\n",
				surf, zpos);
			return -EINVAL;
		}

		if (surf == primary_surface)
			plane_type = DRM_PLANE_TYPE_PRIMARY;
		else
			plane_type = DRM_PLANE_TYPE_OVERLAY;
		planes[zpos] = apple_plane_init(drm, 1U << num, surf,
						supports_l10r,
						plane_type);
		if (IS_ERR(planes[zpos]))
			return PTR_ERR(planes[zpos]);

		ret = drm_plane_create_zpos_immutable_property(planes[zpos], zpos);
		if (ret)
			return ret;
		if (plane_type == DRM_PLANE_TYPE_PRIMARY)
			primary_plane = planes[zpos];
	}

	crtc = kzalloc(sizeof(*crtc), GFP_KERNEL);
	ret = drm_crtc_init_with_planes(drm, &crtc->base, primary_plane,
					NULL,
					&apple_crtc_funcs, NULL);
	if (ret)
		return ret;

	drm_crtc_helper_add(&crtc->base, &apple_crtc_helper_funcs);
	drm_crtc_enable_color_mgmt(&crtc->base, 0, true, 0);

	enc = drmm_simple_encoder_alloc(drm, struct apple_encoder, base,
					DRM_MODE_ENCODER_TMDS);
	if (IS_ERR(enc))
                return PTR_ERR(enc);
	enc->base.possible_crtcs = drm_crtc_mask(&crtc->base);

	connector = kzalloc(sizeof(*connector), GFP_KERNEL);
	mutex_init(&connector->chunk_lock);
	drm_connector_helper_add(&connector->base,
				 &apple_connector_helper_funcs);

	// HACK:
	if (dcp_ext)
		connector->base.fwnode = fwnode_handle_get(dcp->dev.fwnode);

	ret = drm_connector_init(drm, &connector->base, &apple_connector_funcs,
				 dcp_get_connector_type(dcp));
	if (ret)
		return ret;

	connector->base.polled = DRM_CONNECTOR_POLL_HPD;
	connector->connected = false;
	connector->dcp = dcp;

	INIT_WORK(&connector->hotplug_wq, dcp_hotplug);

	crtc->dcp = dcp;
	dcp_link(dcp, crtc, connector);

	return drm_connector_attach_encoder(&connector->base, &enc->base);
}

static int apple_get_fb_resource(struct device *dev, const char *name,
				 struct resource *fb_r)
{
	int idx, ret = -ENODEV;
	struct device_node *node;

	idx = of_property_match_string(dev->of_node, "memory-region-names", name);

	node = of_parse_phandle(dev->of_node, "memory-region", idx);
	if (!node) {
		dev_err(dev, "reserved-memory node '%s' not found\n", name);
		return -ENODEV;
	}

	if (!of_device_is_available(node)) {
		dev_err(dev, "reserved-memory node '%s' is unavailable\n", name);
		goto err;
	}

	if (!of_device_is_compatible(node, "framebuffer")) {
		dev_err(dev, "reserved-memory node '%s' is incompatible\n",
			node->full_name);
		goto err;
	}

	ret = of_address_to_resource(node, 0, fb_r);

err:
	of_node_put(node);
	return ret;
}

static int apple_get_scanout_pool_resource(struct device *dev,
					    struct resource *pool_r)
{
	struct device_node *node;
	int idx, ret = -ENODEV;

	idx = of_property_match_string(dev->of_node, "memory-region-names",
				       "scanout-pool");
	if (idx < 0)
		return idx;

	node = of_parse_phandle(dev->of_node, "memory-region", idx);
	if (!node)
		return -ENODEV;

	if (!of_device_is_available(node) ||
	    !of_device_is_compatible(node, "shared-dma-pool") ||
	    !of_property_read_bool(node, "no-map")) {
		dev_err(dev, "scanout-pool must be an available no-map shared-dma-pool\n");
		goto out_put;
	}

	ret = of_address_to_resource(node, 0, pool_r);

out_put:
	of_node_put(node);
	return ret;
}

static const struct of_device_id apple_dcp_id_tbl[] = {
	{ .compatible = "apple,dcp" },
	{ .compatible = "apple,dcpext" },
	{},
};

static int apple_drm_init_dcp(struct device *dev)
{
	struct apple_drm_private *apple = dev_get_drvdata(dev);
	struct platform_device *dcp[MAX_COPROCESSORS];
	struct device_node *np;
	u64 timeout;
	int i, ret, num_dcp = 0;

	for_each_matching_node(np, apple_dcp_id_tbl) {
		bool dcp_ext;
		if (!of_device_is_available(np)) {
			of_node_put(np);
			continue;
		}
		dcp_ext = of_device_is_compatible(np, "apple,dcpext") ||
		          of_property_present(np, "phys");

		dcp[num_dcp] = of_find_device_by_node(np);
		of_node_put(np);
		if (!dcp[num_dcp])
			continue;

		device_link_add(dev, &dcp[num_dcp]->dev, DL_FLAG_AUTOREMOVE_SUPPLIER);

		ret = apple_probe_per_dcp(dev, &apple->drm, dcp[num_dcp],
					  num_dcp, dcp_ext);
		if (ret)
			continue;

		/* apple_probe_per_dcp() has permanently allocated a CRTC and its
		 * planes.  Count it even if firmware startup fails so later DCPs get
		 * the CRTC index used by their possible_crtcs masks, and retain the
		 * matching platform device in the readiness array. */
		num_dcp++;
		ret = dcp_start(dcp[num_dcp - 1]);
		if (ret)
			continue;
	}

	if (num_dcp < 1)
		return -ENODEV;

	/*
	 * Starting DPTX might take some time.
	 */
	timeout = get_jiffies_64() + msecs_to_jiffies(3000);

	for (i = 0; i < num_dcp; ++i) {
		u64 jiffies = get_jiffies_64();
		u64 wait = time_after_eq64(jiffies, timeout) ?
				   0 :
				   timeout - jiffies;
		ret = dcp_wait_ready(dcp[i], wait);
		/* There is nothing we can do if a dcp/dcpext does not boot
		 * (successfully). Ignoring it should not do any harm now.
		 * Needs to reevaluated when adding dcpext support.
		 */
		if (ret)
			dev_warn(dev, "DCP[%d] not ready: %d\n", i, ret);
	}
	/* HACK: Wait for dcp* to settle before a modeset */
	msleep(100);

	return 0;
}

static int apple_drm_init(struct device *dev)
{
	struct apple_drm_private *apple;
	struct resource fb_r;
	resource_size_t fb_size;
	bool use_nomap_pool;
	int ret;

	ret = dma_set_mask_and_coherent(dev, DMA_BIT_MASK(42));
	if (ret)
		return ret;

	ret = apple_get_fb_resource(dev, "framebuffer", &fb_r);
	if (ret)
		return ret;

	apple = devm_drm_dev_alloc(dev, &apple_drm_driver,
				   struct apple_drm_private, drm);
	if (IS_ERR(apple))
		return PTR_ERR(apple);

	dev_set_drvdata(dev, apple);
	use_nomap_pool = of_property_read_bool(dev->of_node,
					       "apple,scanout-memory-is-nomap");
	if (use_nomap_pool) {
		ret = apple_get_scanout_pool_resource(dev, &apple->scanout_pool);
		if (ret) {
			dev_err(dev, "invalid scanout-pool reserved memory: %d\n", ret);
			return ret;
		}

		ret = of_reserved_mem_device_init_by_name(dev, dev->of_node,
							  "scanout-pool");
		if (ret) {
			dev_err(dev, "failed to attach scanout-pool: %d\n", ret);
			return ret;
		}
		apple->scanout_pool_attached = true;
		dev_info(dev, "using no-map scanout pool %pr\n",
			 &apple->scanout_pool);
	}

	ret = component_bind_all(dev, apple);
	if (ret)
		goto err_release_rmem;

	ret = drmm_mode_config_init(&apple->drm);
	if (ret)
		goto err_unbind;

	/* DCP requires source buffers to be at least 32x32 pixels. */
	apple->drm.mode_config.min_width = 32;
	apple->drm.mode_config.min_height = 32;

	/*
	 * TODO: this is the max framebuffer size not the maximal supported
	 * output resolution. DCP reports the maximal framebuffer size take it
	 * from there.
	 * Hardcode it for now to the M1 Max DCP reported 'MaxSrcBufferWidth'
	 * and 'MaxSrcBufferHeight' of 16384.
	 */
	apple->drm.mode_config.max_width = 16384;
	apple->drm.mode_config.max_height = 16384;

	apple->drm.mode_config.funcs = &apple_mode_config_funcs;
	apple->drm.mode_config.helper_private = &apple_mode_config_helpers;

	ret = apple_drm_init_dcp(dev);
	if (ret)
		goto err_unbind;

	drm_mode_config_reset(&apple->drm);

	fb_size = fb_r.end - fb_r.start + 1;
	ret = aperture_remove_conflicting_devices(fb_r.start, fb_size,
						  apple_drm_driver.name);
	if (ret) {
		dev_err(dev, "Failed remove fb: %d\n", ret);
		goto err_unbind;
	}

	ret = drm_dev_register(&apple->drm, 0);
	if (ret)
		goto err_unbind;

	drm_client_setup_with_fourcc(&apple->drm, DRM_FORMAT_XRGB8888);

	return 0;

err_unbind:
	component_unbind_all(dev, NULL);
err_release_rmem:
	if (apple->scanout_pool_attached) {
		of_reserved_mem_device_release(dev);
		apple->scanout_pool_attached = false;
	}
	return ret;
}

static void apple_drm_uninit(struct device *dev)
{
	struct apple_drm_private *apple = dev_get_drvdata(dev);

	drm_dev_unregister(&apple->drm);
	drm_atomic_helper_shutdown(&apple->drm);

	component_unbind_all(dev, NULL);
	if (apple->scanout_pool_attached) {
		of_reserved_mem_device_release(dev);
		apple->scanout_pool_attached = false;
	}

	dev_set_drvdata(dev, NULL);
}

static int apple_drm_bind(struct device *dev)
{
	return apple_drm_init(dev);
}

static void apple_drm_unbind(struct device *dev)
{
	apple_drm_uninit(dev);
}

const struct component_master_ops apple_drm_ops = {
	.bind	= apple_drm_bind,
	.unbind	= apple_drm_unbind,
};

static int add_dcp_components(struct device *dev,
			      struct component_match **matchptr)
{
	struct device_node *np, *endpoint, *port;
	int num = 0;

	for_each_matching_node(np, apple_dcp_id_tbl) {
		if (of_device_is_available(np)) {
			drm_of_component_match_add(dev, matchptr,
						   component_compare_of, np);
			num++;
			for_each_endpoint_of_node(np, endpoint) {
				port = of_graph_get_remote_port_parent(endpoint);
				if (!port)
					continue;

#if !IS_ENABLED(CONFIG_DRM_APPLE_AUDIO)
				if (of_device_is_compatible(port, "apple,dpaudio")) {
					of_node_put(port);
					continue;
				}
#endif

				/*
				 * The ATC phy driver is not part of the component
				 * collection for the Apple display-subsystem so
				 * ignore it here.
				 */
				if (of_device_is_compatible(port, "apple,t8103-atcphy")) {
					of_node_put(port);
					continue;
				}

				if (of_device_is_available(port))
					drm_of_component_match_add(dev, matchptr,
							   component_compare_of,
							   port);
				of_node_put(port);
			}
		}
		of_node_put(np);
	}

	return num;
}

static int apple_platform_probe(struct platform_device *pdev)
{
	struct device *mdev = &pdev->dev;
	struct component_match *match = NULL;
	int num_dcp;

	/* add DCP components, handle less than 1 as probe error */
	num_dcp = add_dcp_components(mdev, &match);
	if (num_dcp < 1)
		return -ENODEV;

	return component_master_add_with_match(mdev, &apple_drm_ops, match);
}

static void apple_platform_remove(struct platform_device *pdev)
{
	component_master_del(&pdev->dev, &apple_drm_ops);
}

static const struct of_device_id of_match[] = {
	{ .compatible = "apple,display-subsystem" },
	{}
};
MODULE_DEVICE_TABLE(of, of_match);

#ifdef CONFIG_PM_SLEEP
static int apple_platform_suspend(struct device *dev)
{
	struct apple_drm_private *apple = dev_get_drvdata(dev);

	if (apple)
		return drm_mode_config_helper_suspend(&apple->drm);

	return 0;
}

static int apple_platform_resume(struct device *dev)
{
	struct apple_drm_private *apple = dev_get_drvdata(dev);

	if (apple)
		drm_mode_config_helper_resume(&apple->drm);

	return 0;
}

static const struct dev_pm_ops apple_platform_pm_ops = {
	.suspend	= apple_platform_suspend,
	.resume		= apple_platform_resume,
};
#endif

static struct platform_driver apple_platform_driver = {
	.driver	= {
		.name = "apple-drm",
		.of_match_table	= of_match,
#ifdef CONFIG_PM_SLEEP
		.pm = &apple_platform_pm_ops,
#endif
	},
	.probe		= apple_platform_probe,
	.remove		= apple_platform_remove,
};



static int __init appledrm_register(void)
{
	if (drm_firmware_drivers_only())
		return -ENODEV;

#if IS_ENABLED(CONFIG_DRM_APPLE_AUDIO)
	dcp_audio_register();
#endif
	dcp_register();
	platform_driver_register(&apple_platform_driver);

	return 0;
}

static void __exit appledrm_unregister(void)
{
#if IS_ENABLED(CONFIG_DRM_APPLE_AUDIO)
	dcp_audio_unregister();
#endif
	dcp_unregister();
	platform_driver_unregister(&apple_platform_driver);
}

module_init(appledrm_register);
module_exit(appledrm_unregister);

MODULE_AUTHOR("Asahi Linux contributors");
MODULE_DESCRIPTION(DRIVER_DESC);
MODULE_LICENSE("Dual MIT/GPL");
