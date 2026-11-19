// SPDX-License-Identifier: GPL-2.0-only OR MIT
/*
 * Apple SoC PMGR device power state driver
 *
 * Copyright The Asahi Linux Contributors
 */

#include <linux/bitops.h>
#include <linux/bitfield.h>
#include <linux/err.h>
#include <linux/io.h>
#include <linux/iopoll.h>
#include <linux/of.h>
#include <linux/of_address.h>
#include <linux/platform_device.h>
#include <linux/pm_domain.h>
#include <linux/regmap.h>
#include <linux/mfd/syscon.h>
#include <linux/reset-controller.h>
#include <linux/module.h>
#include <linux/soc/apple/pmgr.h>

#define APPLE_PMGR_RESET        BIT(31)
#define APPLE_PMGR_AUTO_ENABLE  BIT(28)
#define APPLE_PMGR_PS_AUTO      GENMASK(27, 24)
#define APPLE_PMGR_PS_MIN       GENMASK(19, 16)
#define APPLE_PMGR_PS_RESET     BIT(12)
#define APPLE_PMGR_BUSY         BIT(11)
#define APPLE_PMGR_DEV_DISABLE  BIT(10)
#define APPLE_PMGR_WAS_CLKGATED BIT(9)
#define APPLE_PMGR_WAS_PWRGATED BIT(8)
#define APPLE_PMGR_PS_ACTUAL    GENMASK(7, 4)
#define APPLE_PMGR_PS_TARGET    GENMASK(3, 0)

#define APPLE_PMGR_FLAGS        (APPLE_PMGR_WAS_CLKGATED | APPLE_PMGR_WAS_PWRGATED)

#define APPLE_PMGR_PS_ACTIVE    0xf
#define APPLE_PMGR_PS_CLKGATE   0x4
#define APPLE_PMGR_PS_PWRGATE   0x0

#define APPLE_PMGR_PS_SET_TIMEOUT 100
#define APPLE_PMGR_RESET_TIME 1

struct apple_pmgr_ps {
	struct device *dev;
	struct generic_pm_domain genpd;
	struct reset_controller_dev rcdev;
	struct regmap *regmap;
	struct regmap *gate_map;
	u32 gate_offset;
	struct regmap *auto_map;
	u32 auto_offset, auto_mask, auto_saved;
	bool auto_saved_valid;
	u32 offset;
	u32 min_state;
	bool force_disable;
	bool force_reset;
	bool externally_clocked;
	bool no_auto_enable;
};

/* T8132's ADT supplies a per-domain gate sequencer independently of the
 * power-state aperture. Only described domains use it. The display lifecycle
 * is a consumer of these domain operations, not the owner of this register.
 */
static int apple_pmgr_parse_gate(struct apple_pmgr_ps *ps, struct device_node *node)
{
	int max_register;

	if (!of_property_present(node, "apple,power-gate"))
		return 0;
	if (!of_device_is_compatible(node, "apple,t8132-pmgr-pwrstate"))
		return -EOPNOTSUPP;
	ps->gate_map = syscon_regmap_lookup_by_phandle_args(node,
		"apple,power-gate", 1, &ps->gate_offset);
	if (IS_ERR(ps->gate_map))
		return PTR_ERR(ps->gate_map);
	max_register = regmap_get_max_register(ps->gate_map);
	if (max_register < 0)
		return max_register;
	if ((ps->gate_offset & 3) || max_register < 0x24 ||
	    ps->gate_offset > max_register - 0x24)
		return -EINVAL;
	return 0;
}

static int apple_pmgr_parse_auto_control(struct apple_pmgr_ps *ps,
					 struct device_node *node)
{
	u32 args[2];
	int max_register;

	if (!of_property_present(node, "apple,auto-pm-control"))
		return 0;
	ps->auto_map = syscon_regmap_lookup_by_phandle_args(node,
		"apple,auto-pm-control", 2, args);
	if (IS_ERR(ps->auto_map))
		return PTR_ERR(ps->auto_map);
	ps->auto_offset = args[0];
	ps->auto_mask = args[1];
	max_register = regmap_get_max_register(ps->auto_map);
	if (max_register < 0)
		return max_register;
	if ((ps->auto_offset & 3) || !ps->auto_mask ||
	    ps->auto_offset > max_register)
		return -EINVAL;
	return 0;
}

static int apple_pmgr_take_control(struct apple_pmgr_ps *ps)
{
	int ret;

	if (!ps->auto_map)
		return 0;
	if (!ps->auto_saved_valid) {
		ret = regmap_read(ps->auto_map, ps->auto_offset, &ps->auto_saved);
		if (ret)
			return ret;
		ps->auto_saved_valid = true;
	}
	return regmap_update_bits(ps->auto_map, ps->auto_offset,
				  ps->auto_mask, 0);
}

static int apple_pmgr_release_control(struct apple_pmgr_ps *ps)
{
	int ret;

	if (!ps->auto_map || !ps->auto_saved_valid)
		return 0;
	ret = regmap_update_bits(ps->auto_map, ps->auto_offset,
				 ps->auto_mask, ps->auto_saved);
	if (!ret)
		ps->auto_saved_valid = false;
	return ret;
}

static int apple_pmgr_gate_set(struct apple_pmgr_ps *ps, bool on)
{
	u32 gate, state;
	int ret;

	if (!ps->gate_map)
		return 0;
	/* Full writes are required even if the readback is unchanged. */
	ret = regmap_write(ps->gate_map, ps->gate_offset, on ? 0 : 3);
	if (!ret)
		ret = regmap_write(ps->gate_map, ps->gate_offset + 0x24, on ? 1 : 0);
	if (!ret)
		ret = regmap_read(ps->gate_map, ps->gate_offset, &gate);
	if (!ret)
		ret = regmap_read(ps->gate_map, ps->gate_offset + 0x24, &state);
	if (!ret && (gate != (on ? 0 : 3) ||
		    state != (on ? 1 : 0)))
		ret = -EIO;
	dev_dbg(ps->dev, "power-gate offset=%#x on=%u error=%d\n",
		ps->gate_offset, on, ret);
	return ret;
}

#define genpd_to_apple_pmgr_ps(_genpd) container_of(_genpd, struct apple_pmgr_ps, genpd)
#define rcdev_to_apple_pmgr_ps(_rcdev) container_of(_rcdev, struct apple_pmgr_ps, rcdev)

static int apple_pmgr_ps_set(struct generic_pm_domain *genpd, u32 pstate, bool auto_enable)
{
	int ret;
	struct apple_pmgr_ps *ps = genpd_to_apple_pmgr_ps(genpd);
	u32 reg, cur;

	if (ps->no_auto_enable)
		auto_enable = false;

	ret = regmap_read(ps->regmap, ps->offset, &reg);
	if (ret < 0)
		return ret;

	ret = apple_pmgr_take_control(ps);
	if (ret)
		return ret;

	/* Resets are synchronous, and only work if the device is powered and clocked. */
	if (reg & APPLE_PMGR_RESET && pstate != APPLE_PMGR_PS_ACTIVE)
		dev_err(ps->dev, "PS %s: powering off with RESET active\n",
			genpd->name);

	if (pstate != APPLE_PMGR_PS_ACTIVE && (ps->force_disable || ps->force_reset)) {
		u32 reg_pre = reg & ~(APPLE_PMGR_AUTO_ENABLE | APPLE_PMGR_FLAGS);

		if (ps->force_disable)
			reg_pre |= APPLE_PMGR_DEV_DISABLE;
		if (ps->force_reset)
			reg_pre |= APPLE_PMGR_PS_RESET;

		regmap_write(ps->regmap, ps->offset, reg_pre);

		ret = regmap_read_poll_timeout_atomic(
			ps->regmap, ps->offset, cur,
			(cur & (APPLE_PMGR_DEV_DISABLE | APPLE_PMGR_PS_RESET)) ==
			(reg_pre & (APPLE_PMGR_DEV_DISABLE | APPLE_PMGR_PS_RESET)), 1,
			APPLE_PMGR_PS_SET_TIMEOUT);

		if (ret < 0)
			dev_err(ps->dev, "PS %s: Failed to set reset/disable bits (now: 0x%x)\n",
				genpd->name, reg);
	}

	if (ps->gate_map && pstate == APPLE_PMGR_PS_PWRGATE) {
		/* Leave AUTO at the existing target before arming real gating. */
		reg &= ~(APPLE_PMGR_AUTO_ENABLE | APPLE_PMGR_FLAGS);
		ret = regmap_write(ps->regmap, ps->offset, reg);
		if (ret)
			return ret;
		ret = regmap_read_poll_timeout_atomic(ps->regmap, ps->offset, cur,
			!(cur & APPLE_PMGR_AUTO_ENABLE), 1, APPLE_PMGR_PS_SET_TIMEOUT);
		if (ret)
			return ret;
		ret = apple_pmgr_gate_set(ps, false);
		if (ret)
			return ret;
	} else if (ps->gate_map && pstate == APPLE_PMGR_PS_ACTIVE) {
		/* Restore running policy before target-f/AUTO and before RUN. */
		ret = apple_pmgr_gate_set(ps, true);
		if (ret)
			return ret;
	}

	reg &= ~(APPLE_PMGR_DEV_DISABLE | APPLE_PMGR_PS_RESET |
		 APPLE_PMGR_AUTO_ENABLE | APPLE_PMGR_FLAGS | APPLE_PMGR_PS_TARGET);
	reg |= FIELD_PREP(APPLE_PMGR_PS_TARGET, pstate);
	if (ps->gate_map && pstate == APPLE_PMGR_PS_PWRGATE)
		reg |= APPLE_PMGR_FLAGS;

	dev_dbg(ps->dev, "PS %s: pwrstate = 0x%x: 0x%x\n", genpd->name, pstate, reg);

	regmap_write(ps->regmap, ps->offset, reg);

	if (ps->externally_clocked && pstate == APPLE_PMGR_PS_ACTIVE) {
		/*
		 * If this clock domain requires an external clock, then
		 * consider the "clock gated" state to be good enough.
		 */
		ret = regmap_read_poll_timeout_atomic(
			ps->regmap, ps->offset, cur,
			FIELD_GET(APPLE_PMGR_PS_ACTUAL, cur) >= APPLE_PMGR_PS_CLKGATE, 1,
			APPLE_PMGR_PS_SET_TIMEOUT);
	} else {
		ret = regmap_read_poll_timeout_atomic(
			ps->regmap, ps->offset, cur,
			FIELD_GET(APPLE_PMGR_PS_ACTUAL, cur) == pstate, 1,
			APPLE_PMGR_PS_SET_TIMEOUT);
	}

	if (ret < 0)
		dev_err(ps->dev, "PS %s: Failed to reach power state 0x%x (now: 0x%x)\n",
			genpd->name, pstate, reg);

	if (auto_enable) {
		/* Not all devices implement this; this is a no-op where not implemented. */
		reg |= APPLE_PMGR_AUTO_ENABLE;
		regmap_write(ps->regmap, ps->offset, reg);
	}
	if (!ret && pstate == APPLE_PMGR_PS_ACTIVE)
		ret = apple_pmgr_release_control(ps);

	return ret;
}

static bool apple_pmgr_ps_is_active(u32 reg)
{
	/*
	 * We consider domains as active if they are actually on, or if they have auto-PM
	 * enabled and the intended target is on.
	 */
	return (FIELD_GET(APPLE_PMGR_PS_ACTUAL, reg) == APPLE_PMGR_PS_ACTIVE ||
		(FIELD_GET(APPLE_PMGR_PS_TARGET, reg) == APPLE_PMGR_PS_ACTIVE &&
		 reg & APPLE_PMGR_AUTO_ENABLE));
}

static int apple_pmgr_ps_power_on(struct generic_pm_domain *genpd)
{
	return apple_pmgr_ps_set(genpd, APPLE_PMGR_PS_ACTIVE, true);
}

static int apple_pmgr_ps_power_off(struct generic_pm_domain *genpd)
{
	return apple_pmgr_ps_set(genpd, APPLE_PMGR_PS_PWRGATE, false);
}

int apple_pmgr_read_state(struct device_node *node, u32 *value)
{
	struct regmap *map;
	u32 offset;

	if (!of_device_is_compatible(node, "apple,t8132-pmgr-pwrstate") ||
	    of_property_read_u32(node, "reg", &offset))
		return -EINVAL;
	map = syscon_node_to_regmap(node->parent);
	if (IS_ERR(map))
		return PTR_ERR(map);
	return regmap_read(map, offset, value);
}

EXPORT_SYMBOL_GPL(apple_pmgr_read_state);

static int apple_pmgr_reset_assert(struct reset_controller_dev *rcdev, unsigned long id)
{
	struct apple_pmgr_ps *ps = rcdev_to_apple_pmgr_ps(rcdev);
	unsigned long flags;

	spin_lock_irqsave(&ps->genpd.slock, flags);

	if (ps->genpd.status == GENPD_STATE_OFF)
		dev_err(ps->dev, "PS 0x%x: asserting RESET while powered down\n", ps->offset);

	dev_dbg(ps->dev, "PS 0x%x: assert reset\n", ps->offset);
	/* Quiesce device before asserting reset */
	regmap_update_bits(ps->regmap, ps->offset, APPLE_PMGR_FLAGS | APPLE_PMGR_DEV_DISABLE,
			   APPLE_PMGR_DEV_DISABLE);
	regmap_update_bits(ps->regmap, ps->offset, APPLE_PMGR_FLAGS | APPLE_PMGR_RESET,
			   APPLE_PMGR_RESET);

	spin_unlock_irqrestore(&ps->genpd.slock, flags);

	return 0;
}

static int apple_pmgr_reset_deassert(struct reset_controller_dev *rcdev, unsigned long id)
{
	struct apple_pmgr_ps *ps = rcdev_to_apple_pmgr_ps(rcdev);
	unsigned long flags;

	spin_lock_irqsave(&ps->genpd.slock, flags);

	dev_dbg(ps->dev, "PS 0x%x: deassert reset\n", ps->offset);
	regmap_update_bits(ps->regmap, ps->offset, APPLE_PMGR_FLAGS | APPLE_PMGR_RESET, 0);
	regmap_update_bits(ps->regmap, ps->offset, APPLE_PMGR_FLAGS | APPLE_PMGR_DEV_DISABLE, 0);

	if (ps->genpd.status == GENPD_STATE_OFF)
		dev_err(ps->dev, "PS 0x%x: RESET was deasserted while powered down\n", ps->offset);

	spin_unlock_irqrestore(&ps->genpd.slock, flags);

	return 0;
}

static int apple_pmgr_reset_reset(struct reset_controller_dev *rcdev, unsigned long id)
{
	int ret;

	ret = apple_pmgr_reset_assert(rcdev, id);
	if (ret)
		return ret;

	usleep_range(APPLE_PMGR_RESET_TIME, 2 * APPLE_PMGR_RESET_TIME);

	return apple_pmgr_reset_deassert(rcdev, id);
}

static int apple_pmgr_reset_status(struct reset_controller_dev *rcdev, unsigned long id)
{
	struct apple_pmgr_ps *ps = rcdev_to_apple_pmgr_ps(rcdev);
	u32 reg = 0;

	regmap_read(ps->regmap, ps->offset, &reg);

	return !!(reg & APPLE_PMGR_RESET);
}

static const struct reset_control_ops apple_pmgr_reset_ops = {
	.assert		= apple_pmgr_reset_assert,
	.deassert	= apple_pmgr_reset_deassert,
	.reset		= apple_pmgr_reset_reset,
	.status		= apple_pmgr_reset_status,
};

static int apple_pmgr_reset_xlate(struct reset_controller_dev *rcdev,
				  const struct of_phandle_args *reset_spec)
{
	return 0;
}

static int apple_pmgr_ps_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct device_node *node = dev->of_node;
	struct apple_pmgr_ps *ps;
	struct regmap *regmap;
	struct of_phandle_iterator it;
	int ret;
	const char *name;
	bool active;
	bool preserve_clkgated = false;
	u32 boot_state;

	regmap = syscon_node_to_regmap(node->parent);
	if (IS_ERR(regmap))
		return PTR_ERR(regmap);

	ps = devm_kzalloc(dev, sizeof(*ps), GFP_KERNEL);
	if (!ps)
		return -ENOMEM;

	ps->dev = dev;
	ps->regmap = regmap;
	ret = apple_pmgr_parse_gate(ps, node);
	if (ret)
		return dev_err_probe(dev, ret, "invalid power-gate description\n");
	ret = apple_pmgr_parse_auto_control(ps, node);
	if (ret)
		return dev_err_probe(dev, ret, "invalid automatic-PM control\n");

	ret = of_property_read_string(node, "label", &name);
	if (ret < 0) {
		dev_err(dev, "missing label property\n");
		return ret;
	}

	ret = of_property_read_u32(node, "reg", &ps->offset);
	if (ret < 0) {
		dev_err(dev, "missing reg property\n");
		return ret;
	}

	ps->genpd.flags |= GENPD_FLAG_IRQ_SAFE;
	ps->genpd.name = name;
	ps->genpd.power_on = apple_pmgr_ps_power_on;
	ps->genpd.power_off = apple_pmgr_ps_power_off;

	ret = of_property_read_u32(node, "apple,min-state", &ps->min_state);
	if (ret == 0 && ps->min_state <= APPLE_PMGR_PS_ACTIVE)
		regmap_update_bits(regmap, ps->offset, APPLE_PMGR_FLAGS | APPLE_PMGR_PS_MIN,
				   FIELD_PREP(APPLE_PMGR_PS_MIN, ps->min_state));

	if (of_property_read_bool(node, "apple,force-disable"))
		ps->force_disable = true;

	if (of_property_read_bool(node, "apple,force-reset"))
		ps->force_reset = true;

	if (of_property_read_bool(node, "apple,externally-clocked"))
		ps->externally_clocked = true;
	ps->no_auto_enable = of_property_read_bool(node, "apple,no-auto-enable");

	ret = regmap_read(regmap, ps->offset, &boot_state);
	if (ret < 0)
		return ret;

	active = apple_pmgr_ps_is_active(boot_state);
	if (of_property_read_bool(node, "apple,always-on")) {
		ps->genpd.flags |= GENPD_FLAG_ALWAYS_ON;
		/*
		 * Firmware can leave a critical domain clock-gated rather than
		 * fully active. Keep that usable, non-off state without requesting
		 * ACTIVE or changing its automatic-PM policy. ALWAYS_ON still
		 * prevents genpd from powering it off. Do not extend this to
		 * unknown intermediate states, reset or disabled domains.
		 */
		if (!active &&
		    FIELD_GET(APPLE_PMGR_PS_TARGET, boot_state) == APPLE_PMGR_PS_CLKGATE &&
		    FIELD_GET(APPLE_PMGR_PS_ACTUAL, boot_state) == APPLE_PMGR_PS_CLKGATE &&
		    !(boot_state & (APPLE_PMGR_RESET | APPLE_PMGR_DEV_DISABLE))) {
			preserve_clkgated = true;
			active = true;
			dev_info(dev, "preserving boot clock-gated always-on domain %s (PS %#x)\n",
				 name, boot_state);
		}
		if (!active) {
			dev_warn(dev, "always-on domain %s is not on at boot\n", name);
			/* Turn it on so pm_genpd_init does not fail */
			active = apple_pmgr_ps_power_on(&ps->genpd) == 0;
		}
	} else if (active) {
		ps->genpd.flags |= GENPD_FLAG_DEFER_OFF;
	}

	/*
	 * Let a consumer keep its domain powered across system sleep by marking its device as
	 * being on an awake path. Nothing changes for devices that don't. Whether the
	 * bootloader left a domain on says nothing about whether a driver started later needs
	 * it across suspend, e.g. a USB4 host router that is only powered while a link is up.
	 */
	ps->genpd.flags |= GENPD_FLAG_ACTIVE_WAKEUP;

	/* Turn on auto-PM if the domain is already on */
	if (active && !preserve_clkgated && !ps->auto_map)
		regmap_update_bits(regmap, ps->offset, APPLE_PMGR_FLAGS | APPLE_PMGR_AUTO_ENABLE,
				   ps->no_auto_enable ? 0 : APPLE_PMGR_AUTO_ENABLE);

	ret = pm_genpd_init(&ps->genpd, NULL, !active);
	if (ret < 0) {
		dev_err(dev, "pm_genpd_init failed\n");
		return ret;
	}

	ret = of_genpd_add_provider_simple(node, &ps->genpd);
	if (ret < 0) {
		dev_err(dev, "of_genpd_add_provider_simple failed\n");
		return ret;
	}

	of_for_each_phandle(&it, ret, node, "power-domains", "#power-domain-cells", -1) {
		struct of_phandle_args parent, child;

		parent.np = it.node;
		parent.args_count = of_phandle_iterator_args(&it, parent.args, MAX_PHANDLE_ARGS);
		child.np = node;
		child.args_count = 0;
		ret = of_genpd_add_subdomain(&parent, &child);

		if (ret == -EPROBE_DEFER) {
			of_node_put(parent.np);
			goto err_remove;
		} else if (ret < 0) {
			dev_err(dev, "failed to add to parent domain: %d (%s -> %s)\n",
				ret, it.node->name, node->name);
			of_node_put(parent.np);
			goto err_remove;
		}
	}

	/*
	 * Do not participate in regular PM; parent power domains are handled via the
	 * genpd hierarchy.
	 */
	pm_genpd_remove_device(dev);

	ps->rcdev.owner = THIS_MODULE;
	ps->rcdev.nr_resets = 1;
	ps->rcdev.ops = &apple_pmgr_reset_ops;
	ps->rcdev.of_node = dev->of_node;
	ps->rcdev.of_reset_n_cells = 0;
	ps->rcdev.of_xlate = apple_pmgr_reset_xlate;

	ret = devm_reset_controller_register(dev, &ps->rcdev);
	if (ret < 0)
		goto err_remove;

	return 0;
err_remove:
	of_genpd_del_provider(node);
	pm_genpd_remove(&ps->genpd);
	return ret;
}

static const struct of_device_id apple_pmgr_ps_of_match[] = {
	{ .compatible = "apple,t8103-pmgr-pwrstate" },
	{ .compatible = "apple,pmgr-pwrstate" },
	{}
};

MODULE_DEVICE_TABLE(of, apple_pmgr_ps_of_match);

static struct platform_driver apple_pmgr_ps_driver = {
	.probe = apple_pmgr_ps_probe,
	.driver = {
		.name = "apple-pmgr-pwrstate",
		.of_match_table = apple_pmgr_ps_of_match,
	},
};

MODULE_AUTHOR("Hector Martin <marcan@marcan.st>");
MODULE_DESCRIPTION("PMGR power state driver for Apple SoCs");

module_platform_driver(apple_pmgr_ps_driver);
