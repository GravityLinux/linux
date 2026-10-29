// SPDX-License-Identifier: GPL-2.0
/*
 * Apple Silicon DWC3 Glue driver
 * Copyright (C) The Asahi Linux Contributors
 *
 * Based on:
 *  - dwc3-qcom.c Copyright (c) 2018, The Linux Foundation. All rights reserved.
 *  - dwc3-of-simple.c Copyright (c) 2015 Texas Instruments Incorporated - https://www.ti.com
 */

#include <linux/of.h>
#include <linux/bitfield.h>
#include <linux/delay.h>
#include <linux/io.h>
#include <linux/usb.h>
#include <linux/usb/hcd.h>
#include <linux/usb/ch11.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/platform_device.h>
#include <linux/reset.h>
#include <linux/soc/apple/tunable.h>

#include "glue.h"

/* USB3 hub SetPortFeature(LINK_STATE): port in the low byte of wIndex,
 * unshifted link-state selector in the high byte.
 */
#define DWC3_APPLE_SS_ROOT_PORT 1
#define DWC3_APPLE_LINK_STATE_SELECTOR GENMASK(15, 8)
#define DWC3_APPLE_HUB_REQUEST_TIMEOUT_MS 1000
#define DWC3_APPLE_SS_DISABLE_POLL_COUNT 100
#define DWC3_APPLE_SS_DISABLE_POLL_MIN_US 1000
#define DWC3_APPLE_SS_DISABLE_POLL_MAX_US 2000

/*
 * This platform requires a very specific sequence of operations to bring up dwc3 and its USB3 PHY:
 *
 * 1) The PHY itself has to be brought up; for this we need to know the mode (USB3,
 *    USB3+DisplayPort, USB4, etc) and the lane orientation. This happens through typec_mux_set.
 * 2) DWC3 has to be brought up but we must not touch the gadget area or start xhci yet.
 * 3) The PHY bring-up has to be finalized and dwc3's PIPE interface has to be switched to the
 *    USB3 PHY, this is done inside phy_set_mode.
 * 4) We can now initialize xhci or gadget mode.
 *
 * We can switch 1 and 2 but 3 has to happen after (1 and 2) and 4 has to happen after 3.
 *
 * And then to bring this all down again:
 *
 * 1) DWC3 has to exit host or gadget mode and must no longer touch those registers
 * 2) The PHY has to switch dwc3's PIPE interface back to the dummy backend
 * 3) The PHY itself can be shut down, this happens from typec_mux_set
 *
 * We also can't transition the PHY from one mode to another while dwc3 is up and running (this is
 * slightly wrong, some transitions are possible, others aren't but because we have no documentation
 * for this I'd rather play it safe).
 *
 * After both the PHY and dwc3 are initialized we will only ever see a single "new device connected"
 * event. If we just keep them running only the first device plugged in will ever work. XHCI's port
 * status register actually does show the correct state but no interrupt ever comes in. In gadget
 * mode we don't even get a USBDisconnected event and everything looks like there's still something
 * connected on the other end.
 * This can be partially explained because the USB2 D+/D- lines are connected through a stateful
 * eUSB2 repeater which in turn is controlled by a variant of the TI TPS6598x USB PD chip which
 * resets the repeater out-of-band everytime the CC lines are (dis)connected. This then requires a
 * PHY reset to make sure the PHY and the eUSB2 repeater state are synchronized again.
 *
 * And to make this all extra fun: If we get the order of some of this wrong either the port is just
 * broken until a phy+dwc3 reset, or it's broken until a full SoC reset (likely because we can't
 * reset some parts of the PHY), or some watchdog kicks in after a few seconds and forces a full SoC
 * reset (mostly seen this with USB4/Thunderbolt but there's clearly some watchdog that hates
 * invalid states).
 *
 * Hence there's really no good way to keep dwc3 fully up and running after we disconnect a cable
 * because then we can't shut down the PHY anymore. And if we kept the PHY running in whatever mode
 * it was until the next cable is connected we'd need to tear it all down and bring it back up again
 * anyway to detect and use the next device.
 *
 * Instead, we just shut down everything when a cable is disconnected and transition to
 * DWC3_APPLE_NO_CABLE.
 * During initial probe we don't have any information about the connected cable and can't bring up
 * the PHY properly and thus also can't fully bring up dwc3. Instead, we just keep everything off
 * and defer the first dwc3 probe until we get the first cable connected event. Until then we stay
 * in DWC3_APPLE_PROBE_PENDING.
 * Once a cable is connected we then keep track of the controller mode here by transitioning to
 * DWC3_APPLE_HOST or DWC3_APPLE_DEVICE.
 */
enum dwc3_apple_state {
	DWC3_APPLE_PROBE_PENDING, /* Before first cable connection, dwc3_core_probe not called */
	DWC3_APPLE_NO_CABLE, /* No cable connected, dwc3 suspended after dwc3_core_exit */
	DWC3_APPLE_HOST, /* Cable connected, dwc3 in host mode */
	DWC3_APPLE_DEVICE, /* Cable connected, dwc3 in device mode */
};

/**
 * struct dwc3_apple - Apple-specific DWC3 USB controller
 * @dwc: Core DWC3 structure
 * @dev: Pointer to the device structure
 * @mmio_resource: Resource to be passed to dwc3_core_probe
 * @apple_regs: Apple-specific DWC3 registers
 * @reset: Reset control
 * @role_sw: USB role switch
 * @lock: Mutex for synchronizing access
 * @state: Current state of the controller, see documentation for the enum for details
 */
struct dwc3_apple;

struct dwc3_apple_host_port {
	struct dwc3_apple *controller;
	struct usb_role_switch *sw;
	enum usb_role role;
	u32 index;
};

struct dwc3_apple {
	struct dwc3 dwc;

	struct device *dev;
	struct resource *mmio_resource;
	void __iomem *apple_regs;

	struct reset_control *reset;
	struct apple_tunable *host_tunable;
	void __iomem *cfg_regs;
	struct apple_tunable *cfg_host_tunable;
	struct usb_role_switch *role_sw;
	bool ss_port_disabled;
	struct dwc3_apple_host_port host_ports[2];

	struct mutex lock;

	enum dwc3_apple_state state;
};

#define to_dwc3_apple(d) container_of((d), struct dwc3_apple, dwc)

/*
 * Apple Silicon dwc3 vendor-specific registers
 *
 * These registers were identified by tracing XNU's memory access patterns and correlating them with
 * debug output over serial to determine their names. We don't exactly know what these do but
 * without these USB3 devices sometimes don't work.
 */
#define APPLE_DWC3_REGS_START 0xcd00
#define APPLE_DWC3_REGS_END 0xcdff

#define APPLE_DWC3_CIO_LFPS_OFFSET 0xcd38
#define APPLE_DWC3_CIO_LFPS_OFFSET_VALUE 0xf800f80

#define APPLE_DWC3_CIO_BW_NGT_OFFSET 0xcd3c
#define APPLE_DWC3_CIO_BW_NGT_OFFSET_VALUE 0xfc00fc0

#define APPLE_DWC3_CIO_LINK_TIMER 0xcd40
#define APPLE_DWC3_CIO_PENDING_HP_TIMER GENMASK(23, 16)
#define APPLE_DWC3_CIO_PENDING_HP_TIMER_VALUE 0x14
#define APPLE_DWC3_CIO_PM_LC_TIMER GENMASK(15, 8)
#define APPLE_DWC3_CIO_PM_LC_TIMER_VALUE 0xa
#define APPLE_DWC3_CIO_PM_ENTRY_TIMER GENMASK(7, 0)
#define APPLE_DWC3_CIO_PM_ENTRY_TIMER_VALUE 0x10

static inline void dwc3_apple_writel(struct dwc3_apple *appledwc, u32 offset, u32 value)
{
	writel(value, appledwc->apple_regs + offset - APPLE_DWC3_REGS_START);
}

static inline u32 dwc3_apple_readl(struct dwc3_apple *appledwc, u32 offset)
{
	return readl(appledwc->apple_regs + offset - APPLE_DWC3_REGS_START);
}

static inline void dwc3_apple_mask(struct dwc3_apple *appledwc, u32 offset, u32 mask, u32 value)
{
	u32 reg;

	reg = dwc3_apple_readl(appledwc, offset);
	reg &= ~mask;
	reg |= value;
	dwc3_apple_writel(appledwc, offset, reg);
}

static void dwc3_apple_setup_cio(struct dwc3_apple *appledwc)
{
	dwc3_apple_writel(appledwc, APPLE_DWC3_CIO_LFPS_OFFSET, APPLE_DWC3_CIO_LFPS_OFFSET_VALUE);
	dwc3_apple_writel(appledwc, APPLE_DWC3_CIO_BW_NGT_OFFSET,
			  APPLE_DWC3_CIO_BW_NGT_OFFSET_VALUE);
	dwc3_apple_mask(appledwc, APPLE_DWC3_CIO_LINK_TIMER, APPLE_DWC3_CIO_PENDING_HP_TIMER,
			FIELD_PREP(APPLE_DWC3_CIO_PENDING_HP_TIMER,
				   APPLE_DWC3_CIO_PENDING_HP_TIMER_VALUE));
	dwc3_apple_mask(appledwc, APPLE_DWC3_CIO_LINK_TIMER, APPLE_DWC3_CIO_PM_LC_TIMER,
			FIELD_PREP(APPLE_DWC3_CIO_PM_LC_TIMER, APPLE_DWC3_CIO_PM_LC_TIMER_VALUE));
	dwc3_apple_mask(appledwc, APPLE_DWC3_CIO_LINK_TIMER, APPLE_DWC3_CIO_PM_ENTRY_TIMER,
			FIELD_PREP(APPLE_DWC3_CIO_PM_ENTRY_TIMER,
				   APPLE_DWC3_CIO_PM_ENTRY_TIMER_VALUE));
}

static void dwc3_apple_set_ptrcap(struct dwc3_apple *appledwc, u32 mode)
{
	guard(spinlock_irqsave)(&appledwc->dwc.lock);
	dwc3_set_prtcap(&appledwc->dwc, mode, false);
}

static int dwc3_apple_core_probe(struct dwc3_apple *appledwc)
{
	struct dwc3_probe_data probe_data = {};
	int ret;

	lockdep_assert_held(&appledwc->lock);
	WARN_ON_ONCE(appledwc->state != DWC3_APPLE_PROBE_PENDING);

	appledwc->dwc.dev = appledwc->dev;
	probe_data.dwc = &appledwc->dwc;
	probe_data.res = appledwc->mmio_resource;
	probe_data.ignore_clocks_and_resets = true;
	probe_data.skip_core_init_mode = true;
	probe_data.properties = DWC3_DEFAULT_PROPERTIES;

	ret = dwc3_core_probe(&probe_data);
	if (ret)
		return ret;

	/*
	 * Started interrupt IN transfers stop all IN traffic on this core
	 * (seen on T8132, reported on M3); run interrupt endpoints as bulk.
	 */
	appledwc->dwc.int_as_bulk = 1;

	appledwc->state = DWC3_APPLE_NO_CABLE;
	return 0;
}

static int dwc3_apple_core_init(struct dwc3_apple *appledwc)
{
	int ret;

	lockdep_assert_held(&appledwc->lock);

	switch (appledwc->state) {
	case DWC3_APPLE_PROBE_PENDING:
		ret = dwc3_apple_core_probe(appledwc);
		if (ret)
			dev_err(appledwc->dev, "Failed to probe DWC3 Core, err=%d\n", ret);
		break;
	case DWC3_APPLE_NO_CABLE:
		ret = dwc3_core_init(&appledwc->dwc);
		if (ret)
			dev_err(appledwc->dev, "Failed to initialize DWC3 Core, err=%d\n", ret);
		break;
	default:
		/* Unreachable unless there's a bug in this driver */
		WARN_ON_ONCE(1);
		ret = -EINVAL;
		break;
	}

	return ret;
}

/* Private Neo bring-up: current DT has no suspend-clock provider, so
 * the generic core leaves the reset PWRDNSCALE value (1560) untouched.
 * Native startup programs 13; live A/B shows warm-reset completion changing
 * from 3.30s to 27.6ms. Preserve other GCTL fields and normal autosuspend.
 * Replace this captured board setting with the real suspend clock when its
 * rate/source is established; do not infer a clock rate from this alone.
 */
#define DWC3_APPLE_NEO_NATIVE_PWRDNSCALE 13

static void dwc3_apple_setup_suspend_scale(struct dwc3_apple *appledwc)
{
	void __iomem *gctl;
	u32 reg;

	if (!of_machine_is_compatible("apple,j700") || appledwc->dwc.susp_clk)
		return;

	/* The core mapping starts at the global registers, not the xHCI area. */
	gctl = appledwc->dwc.regs + DWC3_GCTL - DWC3_GLOBALS_REGS_START;
	reg = readl(gctl);
	reg &= ~DWC3_GCTL_PWRDNSCALE_MASK;
	reg |= FIELD_PREP(DWC3_GCTL_PWRDNSCALE_MASK,
			  DWC3_APPLE_NEO_NATIVE_PWRDNSCALE);
	writel(reg, gctl);
}

static int dwc3_apple_init(struct dwc3_apple *appledwc, enum dwc3_apple_state state)
{
	int ret, ret_reset;

	lockdep_assert_held(&appledwc->lock);

	/*
	 * The USB2 PHY on this platform must be configured for host or device mode while it is
	 * still powered off and before dwc3 tries to access it. Otherwise, the new configuration
	 * will sometimes only take affect after the *next* time dwc3 is brought up which causes
	 * the connected device to just not work.
	 * The USB3 PHY must be configured later after dwc3 has already been initialized.
	 */
	switch (state) {
	case DWC3_APPLE_HOST:
		phy_set_mode(appledwc->dwc.usb2_generic_phy[0], PHY_MODE_USB_HOST);
		break;
	case DWC3_APPLE_DEVICE:
		phy_set_mode(appledwc->dwc.usb2_generic_phy[0], PHY_MODE_USB_DEVICE);
		break;
	default:
		/* Unreachable unless there's a bug in this driver */
		return -EINVAL;
	}

	ret = reset_control_deassert(appledwc->reset);
	if (ret) {
		dev_err(appledwc->dev, "Failed to deassert reset, err=%d\n", ret);
		return ret;
	}

	ret = dwc3_apple_core_init(appledwc);
	if (ret)
		goto reset_assert;

	dwc3_apple_setup_suspend_scale(appledwc);

	/* Factory host configuration is needed after every controller reset,
	 * including USB2-only startup. Values are supplied by the board ADT.
	 */
	apple_tunable_apply(appledwc->dwc.regs, appledwc->host_tunable);
	apple_tunable_apply(appledwc->cfg_regs, appledwc->cfg_host_tunable);

	/*
	 * Now that the core is initialized and already went through dwc3_core_soft_reset we can
	 * configure some unknown Apple-specific settings and then bring up xhci or gadget mode.
	 */
	dwc3_apple_setup_cio(appledwc);

	switch (state) {
	case DWC3_APPLE_HOST:
		appledwc->dwc.dr_mode = USB_DR_MODE_HOST;
		dwc3_apple_set_ptrcap(appledwc, DWC3_GCTL_PRTCAP_HOST);
		/*
		 * This platform requires SUSPHY to be enabled here already in order to properly
		 * configure the PHY and switch dwc3's PIPE interface to USB3 PHY. The USB2 PHY
		 * has already been configured to the correct mode earlier.
		 */
		dwc3_enable_susphy(&appledwc->dwc, true);
		phy_set_mode(appledwc->dwc.usb3_generic_phy[0], PHY_MODE_USB_HOST);
		appledwc->ss_port_disabled = false;
		ret = dwc3_host_init(&appledwc->dwc);
		if (ret) {
			dev_err(appledwc->dev, "Failed to initialize host, ret=%d\n", ret);
			goto core_exit;
		}

		break;
	case DWC3_APPLE_DEVICE:
		appledwc->dwc.dr_mode = USB_DR_MODE_PERIPHERAL;
		dwc3_apple_set_ptrcap(appledwc, DWC3_GCTL_PRTCAP_DEVICE);
		/*
		 * This platform requires SUSPHY to be enabled here already in order to properly
		 * configure the PHY and switch dwc3's PIPE interface to USB3 PHY. The USB2 PHY
		 * has already been configured to the correct mode earlier.
		 */
		dwc3_enable_susphy(&appledwc->dwc, true);
		phy_set_mode(appledwc->dwc.usb3_generic_phy[0], PHY_MODE_USB_DEVICE);
		ret = dwc3_gadget_init(&appledwc->dwc);
		if (ret) {
			dev_err(appledwc->dev, "Failed to initialize gadget, ret=%d\n", ret);
			goto core_exit;
		}
		break;
	default:
		/* Unreachable unless there's a bug in this driver */
		WARN_ON_ONCE(1);
		ret = -EINVAL;
		goto core_exit;
	}

	appledwc->state = state;
	return 0;

core_exit:
	dwc3_core_exit(&appledwc->dwc);
reset_assert:
	ret_reset = reset_control_assert(appledwc->reset);
	if (ret_reset)
		dev_warn(appledwc->dev, "Failed to assert reset, err=%d\n", ret_reset);

	return ret;
}

static int dwc3_apple_exit(struct dwc3_apple *appledwc)
{
	int ret = 0;

	lockdep_assert_held(&appledwc->lock);

	switch (appledwc->state) {
	case DWC3_APPLE_PROBE_PENDING:
	case DWC3_APPLE_NO_CABLE:
		/* Nothing to do if we're already off */
		return 0;
	case DWC3_APPLE_DEVICE:
		dwc3_gadget_exit(&appledwc->dwc);
		break;
	case DWC3_APPLE_HOST:
		dwc3_host_exit(&appledwc->dwc);
		break;
	}

	/*
	 * This platform requires SUSPHY to be enabled in order to properly power down the PHY
	 * and switch dwc3's PIPE interface back to a dummy PHY (i.e. no USB3 support and USB2 via
	 * a different PHY connected through ULPI).
	 */
	dwc3_enable_susphy(&appledwc->dwc, true);
	dwc3_core_exit(&appledwc->dwc);
	appledwc->state = DWC3_APPLE_NO_CABLE;

	ret = reset_control_assert(appledwc->reset);
	if (ret) {
		dev_err(appledwc->dev, "Failed to assert reset, err=%d\n", ret);
		return ret;
	}

	return 0;
}

static int dwc3_usb_role_switch_set(struct usb_role_switch *sw, enum usb_role role)
{
	struct dwc3_apple *appledwc = usb_role_switch_get_drvdata(sw);
	int ret;

	guard(mutex)(&appledwc->lock);

	/*
	 * Skip role switches if appledwc is already in the desired state. The
	 * USB-C port controller on M2 and M1/M2 Pro/Max/Ultra devices issues
	 * additional interrupts which results in usb_role_switch_set_role()
	 * calls with the current role.
	 * Ignore those calls here to ensure the USB-C port controller and
	 * appledwc are in a consistent state.
	 * This matches the behaviour in __dwc3_set_mode().
	 * Do no handle USB_ROLE_NONE for DWC3_APPLE_NO_CABLE and
	 * DWC3_APPLE_PROBE_PENDING since that is no-op anyway.
	 */
	if (appledwc->state == DWC3_APPLE_HOST && role == USB_ROLE_HOST)
		return 0;
	if (appledwc->state == DWC3_APPLE_DEVICE && role == USB_ROLE_DEVICE)
		return 0;

	/*
	 * We need to tear all of dwc3 down and re-initialize it every time a cable is
	 * connected or disconnected or when the mode changes. See the documentation for enum
	 * dwc3_apple_state for details.
	 */
	ret = dwc3_apple_exit(appledwc);
	if (ret)
		return ret;

	switch (role) {
	case USB_ROLE_NONE:
		/* Nothing to do if no cable is connected */
		return 0;
	case USB_ROLE_HOST:
		return dwc3_apple_init(appledwc, DWC3_APPLE_HOST);
	case USB_ROLE_DEVICE:
		return dwc3_apple_init(appledwc, DWC3_APPLE_DEVICE);
	default:
		dev_err(appledwc->dev, "Invalid target role: %d\n", role);
		return -EINVAL;
	}
}

static enum usb_role dwc3_usb_role_switch_get(struct usb_role_switch *sw)
{
	struct dwc3_apple *appledwc = usb_role_switch_get_drvdata(sw);

	guard(mutex)(&appledwc->lock);

	switch (appledwc->state) {
	case DWC3_APPLE_HOST:
		return USB_ROLE_HOST;
	case DWC3_APPLE_DEVICE:
		return USB_ROLE_DEVICE;
	case DWC3_APPLE_NO_CABLE:
	case DWC3_APPLE_PROBE_PENDING:
		return USB_ROLE_NONE;
	default:
		/* Unreachable unless there's a bug in this driver */
		dev_err(appledwc->dev, "Invalid internal state: %d\n", appledwc->state);
		return USB_ROLE_NONE;
	}
}

static int dwc3_apple_setup_role_switch(struct dwc3_apple *appledwc)
{
	struct usb_role_switch_desc dwc3_role_switch = { NULL };

	dwc3_role_switch.fwnode = dev_fwnode(appledwc->dev);
	dwc3_role_switch.set = dwc3_usb_role_switch_set;
	dwc3_role_switch.get = dwc3_usb_role_switch_get;
	dwc3_role_switch.driver_data = appledwc;
	appledwc->role_sw = usb_role_switch_register(appledwc->dev, &dwc3_role_switch);
	if (IS_ERR(appledwc->role_sw))
		return PTR_ERR(appledwc->role_sw);

	return 0;
}

/* Both Neo Type-C connectors feed one internal high-speed hub and DWC3.
 * A port owns its own role request; stop the host only after the last user.
 */
/* The back connector owns the single SuperSpeed root port. Stop its link
 * before Type-C removes the PHY, without resetting the live front HS hub.
 * Use hub requests so xHCI owns PORTSC and its change-bit bookkeeping.
 */
static int dwc3_apple_ss_link(struct dwc3_apple *appledwc, bool enable)
{
	struct usb_hcd *hcd;
	struct usb_device *root;
	struct usb_interface *intf;
	struct usb_port_status status;
	u16 link = enable ? USB_SS_PORT_LS_RX_DETECT : USB_SS_PORT_LS_SS_DISABLED;
	int ret;

	if (!appledwc->dwc.xhci)
		return -ENODEV;
	hcd = platform_get_drvdata(appledwc->dwc.xhci);
	if (!hcd || !hcd->shared_hcd)
		return -ENODEV;
	root = hcd->shared_hcd->self.root_hub;
	if (!root || root->maxchild != 1)
		return -EOPNOTSUPP;
	intf = usb_ifnum_to_if(root, 0);
	if (!intf)
		return -ENODEV;
	ret = usb_autopm_get_interface(intf);
	if (ret)
		return ret;
	ret = usb_control_msg_send(root, 0, USB_REQ_SET_FEATURE, USB_RT_PORT,
				   USB_PORT_FEAT_LINK_STATE, DWC3_APPLE_SS_ROOT_PORT |
				   FIELD_PREP(DWC3_APPLE_LINK_STATE_SELECTOR,
					      FIELD_GET(USB_PORT_STAT_LINK_STATE, link)),
				   NULL, 0, DWC3_APPLE_HUB_REQUEST_TIMEOUT_MS, GFP_KERNEL);
	if (ret)
		goto out;
	if (!enable) {
		ret = -ETIMEDOUT;
		for (unsigned int i = 0; i < DWC3_APPLE_SS_DISABLE_POLL_COUNT; i++) {
			int err = usb_control_msg_recv(root, 0, USB_REQ_GET_STATUS,
					USB_DIR_IN | USB_RT_PORT, HUB_PORT_STATUS,
					DWC3_APPLE_SS_ROOT_PORT, &status,
					offsetof(struct usb_port_status, dwExtPortStatus),
					DWC3_APPLE_HUB_REQUEST_TIMEOUT_MS, GFP_KERNEL);

			if (err) {
				ret = err;
				break;
			}
			if ((le16_to_cpu(status.wPortStatus) & USB_PORT_STAT_LINK_STATE) == link) {
				ret = 0;
				break;
			}
			usleep_range(DWC3_APPLE_SS_DISABLE_POLL_MIN_US,
				     DWC3_APPLE_SS_DISABLE_POLL_MAX_US);
		}
	}
	if (!ret)
		appledwc->ss_port_disabled = !enable;
out:
	usb_autopm_put_interface(intf);
	dev_info(appledwc->dev, "Neo SS link %s result%d\n",
		 enable ? "RxDetect" : "disabled", ret);
	return ret;
}

static int dwc3_apple_host_port_set(struct usb_role_switch *sw, enum usb_role role)
{
	struct dwc3_apple_host_port *port = usb_role_switch_get_drvdata(sw);
	struct dwc3_apple *appledwc = port->controller;
	bool active = role == USB_ROLE_HOST;
	int ret = 0;

	if (role != USB_ROLE_NONE && role != USB_ROLE_HOST)
		return -EOPNOTSUPP;

	guard(mutex)(&appledwc->lock);
	if (port->role == role)
		return 0;

	for (unsigned int i = 0; i < ARRAY_SIZE(appledwc->host_ports); i++)
		if (&appledwc->host_ports[i] != port &&
		    appledwc->host_ports[i].role == USB_ROLE_HOST)
			active = true;

	if (!port->index && active && appledwc->state == DWC3_APPLE_HOST) {
		if (role == USB_ROLE_NONE)
			ret = dwc3_apple_ss_link(appledwc, false);
		else if (appledwc->ss_port_disabled)
			ret = dwc3_apple_ss_link(appledwc, true);
		if (ret)
			return ret;
	}

	if (active && appledwc->state != DWC3_APPLE_HOST)
		ret = dwc3_apple_init(appledwc, DWC3_APPLE_HOST);
	else if (!active)
		ret = dwc3_apple_exit(appledwc);

	if (!ret)
		port->role = role;
	dev_info(appledwc->dev, "shared host port%u role%u active%d state%u result%d\n",
		 port->index, role, active, appledwc->state, ret);
	return ret;
}

static void dwc3_apple_host_port_unregister(void *data)
{
	struct dwc3_apple_host_port *port = data;

	if (port->sw) {
		usb_role_switch_unregister(port->sw);
		port->sw = NULL;
	}
}

static int dwc3_apple_setup_host_ports(struct dwc3_apple *appledwc)
{
	struct device *dev = appledwc->dev;
	unsigned long seen = 0;
	int ret;

	device_for_each_child_node_scoped(dev, child) {
		struct usb_role_switch_desc desc = {};
		struct dwc3_apple_host_port *port;
		u32 index;

		if (!fwnode_property_present(child, "usb-role-switch"))
			continue;
		ret = fwnode_property_read_u32(child, "reg", &index);
		if (ret || index >= ARRAY_SIZE(appledwc->host_ports) || test_bit(index, &seen))
			return -EINVAL;
		set_bit(index, &seen);
		port = &appledwc->host_ports[index];
		port->controller = appledwc;
		port->index = index;
		/* Private diagnostic: cycle each host request without cable changes. */
		desc.allow_userspace_control = true;
		desc.fwnode = child;
		desc.set = dwc3_apple_host_port_set;
		desc.driver_data = port;
		desc.name = devm_kasprintf(dev, GFP_KERNEL, "%s-port%u", dev_name(dev), index);
		if (!desc.name)
			return -ENOMEM;
		port->sw = usb_role_switch_register(dev, &desc);
		if (IS_ERR(port->sw)) {
			ret = PTR_ERR(port->sw);
			port->sw = NULL;
			return ret;
		}
		ret = devm_add_action_or_reset(dev, dwc3_apple_host_port_unregister, port);
		if (ret)
			return ret;
	}

	return seen == GENMASK(ARRAY_SIZE(appledwc->host_ports) - 1, 0) ? 0 : -EINVAL;
}

static int dwc3_apple_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct dwc3_apple *appledwc;
	enum usb_dr_mode dr_mode;
	int ret;

	appledwc = devm_kzalloc(&pdev->dev, sizeof(*appledwc), GFP_KERNEL);
	if (!appledwc)
		return -ENOMEM;

	appledwc->dev = &pdev->dev;
	mutex_init(&appledwc->lock);
	dr_mode = usb_get_dr_mode(dev);
	appledwc->dwc.dr_mode = dr_mode;

	appledwc->reset = devm_reset_control_get_exclusive(dev, NULL);
	if (IS_ERR(appledwc->reset))
		return dev_err_probe(&pdev->dev, PTR_ERR(appledwc->reset),
				     "Failed to get reset control\n");

	ret = reset_control_assert(appledwc->reset);
	if (ret) {
		dev_err(&pdev->dev, "Failed to assert reset, err=%d\n", ret);
		return ret;
	}

	appledwc->mmio_resource = platform_get_resource_byname(pdev, IORESOURCE_MEM, "dwc3-core");
	if (!appledwc->mmio_resource) {
		dev_err(dev, "Failed to get DWC3 MMIO\n");
		return -EINVAL;
	}

	if (of_find_property(dev->of_node, "apple,tunable-host", NULL)) {
		struct resource globals = *appledwc->mmio_resource;

		if (resource_size(&globals) <= DWC3_GLOBALS_REGS_START)
			return -EINVAL;
		globals.start += DWC3_GLOBALS_REGS_START;
		appledwc->host_tunable = devm_apple_tunable_parse(dev, dev->of_node,
							"apple,tunable-host", &globals);
		if (IS_ERR(appledwc->host_tunable))
			return dev_err_probe(dev, PTR_ERR(appledwc->host_tunable),
					     "Invalid host tunables\n");
	}

	if (of_find_property(dev->of_node, "apple,tunable-cfg-host", NULL)) {
		struct resource *cfg;

		cfg = platform_get_resource_byname(pdev, IORESOURCE_MEM, "dwc3-cfg");
		if (!cfg)
			return -EINVAL;
		appledwc->cfg_host_tunable = devm_apple_tunable_parse(dev, dev->of_node,
						"apple,tunable-cfg-host", cfg);
		if (IS_ERR(appledwc->cfg_host_tunable))
			return dev_err_probe(dev, PTR_ERR(appledwc->cfg_host_tunable),
					     "Invalid host configuration tunables\n");
		appledwc->cfg_regs = devm_ioremap_resource(dev, cfg);
		if (IS_ERR(appledwc->cfg_regs))
			return PTR_ERR(appledwc->cfg_regs);
	}

	appledwc->apple_regs = devm_platform_ioremap_resource_byname(pdev, "dwc3-apple");
	if (IS_ERR(appledwc->apple_regs))
		return dev_err_probe(dev, PTR_ERR(appledwc->apple_regs),
				     "Failed to map Apple-specific MMIO\n");

	/*
	 * On this platform, DWC3 can only be brought up after parts of the PHY have been
	 * initialized with knowledge of the target mode and cable orientation from typec_set_mux.
	 * Since this has not happened here we cannot setup DWC3 yet and instead defer this until
	 * the first cable is connected. See the documentation for enum dwc3_apple_state for
	 * details.
	 */
	appledwc->state = DWC3_APPLE_PROBE_PENDING;
	platform_set_drvdata(pdev, &appledwc->dwc);
	if (device_property_read_bool(dev, "apple,shared-host-ports")) {
		if (dr_mode != USB_DR_MODE_HOST)
			return -EINVAL;
		appledwc->dwc.current_dr_role = DWC3_GCTL_PRTCAP_HOST;
		return dwc3_apple_setup_host_ports(appledwc);
	}
	if (dr_mode == USB_DR_MODE_HOST) {
		/*
		 * Host-only instances have no cable event to start the core. Mark
		 * the role before core probe so dwc3_core_soft_reset() does not try
		 * to reset the unused gadget side of the controller.
		 */
		appledwc->dwc.current_dr_role = DWC3_GCTL_PRTCAP_HOST;
		guard(mutex)(&appledwc->lock);
		ret = dwc3_apple_init(appledwc, DWC3_APPLE_HOST);
		if (ret)
			return dev_err_probe(dev, ret, "Failed to start fixed host controller\n");
		return 0;
	}

	ret = dwc3_apple_setup_role_switch(appledwc);
	if (ret)
		return dev_err_probe(&pdev->dev, ret, "Failed to setup role switch\n");

	return 0;
}

static void dwc3_apple_remove(struct platform_device *pdev)
{
	struct dwc3 *dwc = platform_get_drvdata(pdev);
	struct dwc3_apple *appledwc = to_dwc3_apple(dwc);

	guard(mutex)(&appledwc->lock);

	if (appledwc->role_sw)
		usb_role_switch_unregister(appledwc->role_sw);
	for (unsigned int i = 0; i < ARRAY_SIZE(appledwc->host_ports); i++)
		dwc3_apple_host_port_unregister(&appledwc->host_ports[i]);

	/*
	 * If we're still in DWC3_APPLE_PROBE_PENDING we never got any cable connected event and
	 * dwc3_core_probe was never called and there's hence no need to call dwc3_core_remove.
	 * dwc3_apple_exit can be called unconditionally because it checks the state itself.
	 */
	dwc3_apple_exit(appledwc);
	if (appledwc->state != DWC3_APPLE_PROBE_PENDING)
		dwc3_core_remove(&appledwc->dwc);
}

static const struct of_device_id dwc3_apple_of_match[] = {
	{ .compatible = "apple,t8103-dwc3" },
	{}
};
MODULE_DEVICE_TABLE(of, dwc3_apple_of_match);

static struct platform_driver dwc3_apple_driver = {
	.probe		= dwc3_apple_probe,
	.remove		= dwc3_apple_remove,
	.driver		= {
		.name	= "dwc3-apple",
		.of_match_table	= dwc3_apple_of_match,
	},
};

module_platform_driver(dwc3_apple_driver);

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Sven Peter <sven@kernel.org>");
MODULE_DESCRIPTION("DesignWare DWC3 Apple Silicon Glue Driver");
