// SPDX-License-Identifier: GPL-2.0-only OR MIT
/* Direct DisplayPort routing: one ATC output, one free external DCP. */
#include <linux/list.h>
#include <linux/module.h>
#include <linux/of.h>
#include <linux/of_platform.h>
#include <linux/suspend.h>
#include <linux/usb/typec_dp.h>
#include <linux/workqueue.h>

#include "dcp.h"
#include "retirement.h"
#include "dcp-internal.h"

#define APPLE_TYPEC_DCPS 3

struct apple_typec_port {
	struct device *dev;
	struct list_head node;
	struct phy *phy;
	struct mux_control *xbar;
	struct typec_mux_dev *mux;
	struct platform_device *engines[APPLE_TYPEC_DCPS];
	struct apple_dcp *dcp;
	unsigned int num_engines;
	struct phy *previous_phy;
	u32 previous_target;
	u32 target;
	spinlock_t lock;
	bool hpd;
	bool irq;
	u32 sequence;
	unsigned long mode;
	unsigned long active_mode;
	struct work_struct work;
};

/* Serialize pool ownership and keep firmware callbacks outside the PD lock. */
static DEFINE_MUTEX(apple_typec_lock);
static LIST_HEAD(apple_typec_ports);
static bool apple_typec_sleeping;

void dcp_typec_pool_lock(void)
{
	mutex_lock(&apple_typec_lock);
}

void dcp_typec_pool_unlock(void)
{
	mutex_unlock(&apple_typec_lock);
}

bool dcp_typec_pool_sleeping(void)
{
	lockdep_assert_held(&apple_typec_lock);
	return apple_typec_sleeping;
}

static void apple_typec_retry_ports(void)
{
	struct apple_typec_port *port;

	lockdep_assert_held(&apple_typec_lock);
	list_for_each_entry(port, &apple_typec_ports, node)
		queue_work(system_unbound_wq, &port->work);
}

static int apple_typec_release(struct apple_typec_port *port)
{
	struct apple_dcp *dcp = port->dcp;
	int ret;

	lockdep_assert_held(&apple_typec_lock);
	if (!dcp)
		return 0;

	ret = dcp_dptx_disconnect_oob(to_platform_device(dcp->dev), 0);
	if (ret)
		return ret;

	/* release_display has acknowledged before the source can be reassigned. */
	dcp->phy = port->previous_phy;
	dcp->dptx_phy = port->previous_target;
	dcp->typec_port = NULL;
	port->dcp = NULL;
	port->active_mode = 0;
	ret = mux_control_deselect(port->xbar);
	if (ret)
		dev_warn(port->dev, "Failed to disconnect display crossbar: %d\n", ret);
	phy_power_off(port->phy);
	phy_exit(port->phy);
	dev_info(port->dev, "Released %s from ATC%u\n", dev_name(dcp->dev), port->target);
	if (dcp->phy && dcp->typec_routing_ready) {
		ret = dcp_dptx_connect_oob(to_platform_device(dcp->dev), 0);
		if (ret) {
			dcp->typec_routing_ready = false;
			dev_warn(dcp->dev, "Failed to restore fixed display output: %d\n", ret);
		}
	}
	port->previous_phy = NULL;
	dcp_retirement_unplug_locked(dcp);
	apple_typec_retry_ports();
	return 0;
}

static int apple_typec_claim(struct apple_typec_port *port)
{
	struct apple_dcp *dcp = NULL;
	int ret, i;

	lockdep_assert_held(&apple_typec_lock);
	if (apple_typec_sleeping)
		return -EBUSY;
	for (i = 0; i < port->num_engines; i++) {
		struct apple_dcp *candidate = platform_get_drvdata(port->engines[i]);

		if (candidate && candidate->typec_routing_ready && !candidate->crashed &&
		    !dcp_retirement_failed(candidate) &&
		    !candidate->typec_port &&
		    (!candidate->phy || !READ_ONCE(candidate->connector->connected))) {
			dcp = candidate;
			break;
		}
	}
	if (!dcp)
		return -EBUSY;

	ret = dcp_retirement_restart_locked(dcp);
	if (ret)
		return ret;

	ret = phy_init(port->phy);
	if (ret)
		return ret;
	ret = phy_power_on(port->phy);
	if (ret)
		goto exit_phy;
	/* ADT UFP endpoint plus stream bit; never derived from a DRM CRTC ID. */
	ret = mux_control_try_select(port->xbar, dcp->typec_xbar_source);
	if (ret)
		goto power_off;

	port->previous_phy = dcp->phy;
	port->previous_target = dcp->dptx_phy;
	if (port->previous_phy) {
		ret = dcp_dptx_disconnect_oob(to_platform_device(dcp->dev), 0);
		if (ret) {
			int err = mux_control_deselect(port->xbar);

			if (err)
				dev_warn(port->dev, "Crossbar rollback failed: %d\n", err);
			port->previous_phy = NULL;
			goto power_off;
		}
	}
	dcp->phy = port->phy;
	dcp->dptx_phy = port->target;
	dcp->typec_port = port;
	port->dcp = dcp;
	dcp_retirement_note_use(dcp);
	dev_info(port->dev, "Assigned %s source %u to ATC%u\n",
		 dev_name(dcp->dev), dcp->typec_xbar_source, port->target);
	return 0;

power_off:
	phy_power_off(port->phy);
exit_phy:
	phy_exit(port->phy);
	return ret;
}

static void apple_typec_work(struct work_struct *work)
{
	struct apple_typec_port *port = container_of(work, struct apple_typec_port, work);
	bool hpd, irq;
	u32 sequence;
	unsigned long mode;
	int ret;

	mutex_lock(&apple_typec_lock);
	/* Keep latched HPD/mode events until all suppliers have resumed. */
	if (apple_typec_sleeping) {
		mutex_unlock(&apple_typec_lock);
		return;
	}
	for (;;) {
		spin_lock_irq(&port->lock);
		hpd = port->hpd;
		irq = port->irq;
		port->irq = false;
		sequence = port->sequence;
		mode = port->mode;
		spin_unlock_irq(&port->lock);

		ret = 0;
		/* PD can change C/E (four lanes) to D (DP plus USB3) with HPD high. */
		if (hpd && port->dcp && port->active_mode != mode)
			ret = apple_typec_release(port);
		if (ret)
			goto check_sequence;
		if (hpd && port->dcp && port->previous_phy) {
			int i;

			for (i = 0; i < port->num_engines; i++) {
				struct apple_dcp *free = platform_get_drvdata(port->engines[i]);

				if (free && free->typec_routing_ready && !free->crashed &&
				    !dcp_retirement_failed(free) &&
				    !free->phy && !free->typec_port &&
				    !dcp_retirement_restart_locked(free)) {
					ret = apple_typec_release(port);
					break;
				}
			}
		}
		if (ret)
			goto check_sequence;
		if (!hpd) {
			ret = apple_typec_release(port);
		} else {
			if (!port->dcp)
				ret = apple_typec_claim(port);
			if (!ret)
				port->active_mode = mode;
			if (!ret && !port->dcp->dptxport[0].connected)
				ret = dcp_dptx_connect_oob(to_platform_device(port->dcp->dev), 0);
			else if (!ret && irq)
				ret = dptxport_hpd_irq(port->dcp->dptxport[0].service);
		}
		if (ret && ret != -EBUSY)
			dev_warn(port->dev, "Display HPD=%u IRQ=%u failed: %d\n", hpd, irq, ret);

check_sequence:
		spin_lock_irq(&port->lock);
		if (sequence == port->sequence) {
			spin_unlock_irq(&port->lock);
			break;
		}
		spin_unlock_irq(&port->lock);
	}
	mutex_unlock(&apple_typec_lock);
}

static int apple_typec_set(struct typec_mux_dev *mux, struct typec_mux_state *state)
{
	struct apple_typec_port *port = typec_mux_get_drvdata(mux);
	const struct typec_displayport_data *dp = state->data;
	bool hpd = false, irq = false;

	if (state->alt && state->alt->svid == USB_TYPEC_DP_SID && dp &&
	    (state->mode == TYPEC_DP_STATE_C || state->mode == TYPEC_DP_STATE_D ||
	     state->mode == TYPEC_DP_STATE_E)) {
		hpd = !!(dp->status & DP_STATUS_HPD_STATE);
		irq = hpd && !!(dp->status & DP_STATUS_IRQ_HPD);
	}
	spin_lock_irq(&port->lock);
	port->hpd = hpd;
	port->mode = state->mode;
	port->irq |= irq;
	port->sequence++;
	spin_unlock_irq(&port->lock);
	queue_work(system_unbound_wq, &port->work);
	return 0;
}

void dcp_typec_set_ready(struct apple_dcp *dcp, bool ready)
{
	if (!of_property_present(dcp->dev->of_node, "apple,dp-xbar-source"))
		return;

	mutex_lock(&apple_typec_lock);
	dcp->typec_routing_ready = ready;
	if (ready)
		dcp_retirement_ready_locked(dcp);
	if (!ready && dcp->typec_port)
		apple_typec_release(dcp->typec_port);
	apple_typec_retry_ports();
	mutex_unlock(&apple_typec_lock);
}

static void apple_typec_put_engines(void *data)
{
	struct apple_typec_port *port = data;
	int i;

	for (i = 0; i < APPLE_TYPEC_DCPS; i++) {
		if (port->engines[i])
			put_device(&port->engines[i]->dev);
	}
}

static void apple_typec_stop(void *data)
{
	struct apple_typec_port *port = data;

	typec_mux_unregister(port->mux);
	/* Prevent readiness notifications from requeuing work after cancellation. */
	mutex_lock(&apple_typec_lock);
	list_del(&port->node);
	mutex_unlock(&apple_typec_lock);
	cancel_work_sync(&port->work);
	mutex_lock(&apple_typec_lock);
	if (apple_typec_release(port)) {
		/* Firmware failed to release: quarantine the engine on driver removal. */
		struct apple_dcp *dcp = port->dcp;
		int ret;

		dcp->typec_routing_ready = false;
		dcp->phy = port->previous_phy;
		dcp->dptx_phy = port->previous_target;
		dcp->typec_port = NULL;
		port->dcp = NULL;
		ret = mux_control_deselect(port->xbar);
		if (ret)
			dev_warn(port->dev, "Crossbar removal failed: %d\n", ret);
		phy_power_off(port->phy);
		phy_exit(port->phy);
	}
	mutex_unlock(&apple_typec_lock);
}

static int apple_typec_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct apple_typec_port *port;
	struct typec_mux_desc desc = {};
	struct device_node *np;
	int i, ret;

	port = devm_kzalloc(dev, sizeof(*port), GFP_KERNEL);
	if (!port)
		return -ENOMEM;
	port->dev = dev;
	spin_lock_init(&port->lock);
	INIT_WORK(&port->work, apple_typec_work);
	ret = of_property_read_u32(dev->of_node, "apple,atc-index", &port->target);
	if (ret || port->target > 3)
		return -EINVAL;
	port->phy = devm_phy_get(dev, "dp-phy");
	if (IS_ERR(port->phy))
		return dev_err_probe(dev, PTR_ERR(port->phy), "Failed to get DP PHY\n");
	port->xbar = devm_mux_control_get(dev, "dp-xbar");
	if (IS_ERR(port->xbar))
		return dev_err_probe(dev, PTR_ERR(port->xbar), "Failed to get display crossbar\n");

	ret = devm_add_action_or_reset(dev, apple_typec_put_engines, port);
	if (ret)
		return ret;
	ret = of_count_phandle_with_args(dev->of_node, "apple,display-engines", NULL);
	if (ret < 1 || ret > APPLE_TYPEC_DCPS)
		return -EINVAL;
	port->num_engines = ret;
	for (i = 0; i < port->num_engines; i++) {
		np = of_parse_phandle(dev->of_node, "apple,display-engines", i);
		port->engines[i] = of_find_device_by_node(np);
		of_node_put(np);
		if (!port->engines[i] || !platform_get_drvdata(port->engines[i]))
			return -EPROBE_DEFER;
		if (!of_property_present(port->engines[i]->dev.of_node, "apple,dp-xbar-source"))
			return -EINVAL;
		if (!device_link_add(dev, &port->engines[i]->dev, DL_FLAG_AUTOREMOVE_CONSUMER))
			return -ENOMEM;
	}

	desc.fwnode = dev_fwnode(dev);
	desc.set = apple_typec_set;
	desc.drvdata = port;
	mutex_lock(&apple_typec_lock);
	list_add_tail(&port->node, &apple_typec_ports);
	mutex_unlock(&apple_typec_lock);
	port->mux = typec_mux_register(dev, &desc);
	if (IS_ERR(port->mux)) {
		mutex_lock(&apple_typec_lock);
		list_del(&port->node);
		mutex_unlock(&apple_typec_lock);
		cancel_work_sync(&port->work);
		return PTR_ERR(port->mux);
	}
	return devm_add_action_or_reset(dev, apple_typec_stop, port);
}

static const struct of_device_id apple_typec_match[] = {
	{ .compatible = "apple,t8132-dp-altmode" },
	{}
};
MODULE_DEVICE_TABLE(of, apple_typec_match);

static struct platform_driver apple_typec_driver = {
	.probe = apple_typec_probe,
	.driver = {
		.name = "apple-dp-altmode",
		.of_match_table = apple_typec_match,
	},
};

static int apple_typec_pm_notify(struct notifier_block *nb,
				 unsigned long action, void *data)
{
	if (!dcp_retirement_policy_enabled())
		return NOTIFY_DONE;
	if (action != PM_SUSPEND_PREPARE && action != PM_POST_SUSPEND)
		return NOTIFY_DONE;
	mutex_lock(&apple_typec_lock);
	apple_typec_sleeping = action == PM_SUSPEND_PREPARE;
	if (!apple_typec_sleeping)
		apple_typec_retry_ports();
	mutex_unlock(&apple_typec_lock);
	return NOTIFY_OK;
}

static struct notifier_block apple_typec_pm_notifier = {
	.notifier_call = apple_typec_pm_notify,
};

int dcp_typec_register(void)
{
	int ret;

	ret = register_pm_notifier(&apple_typec_pm_notifier);
	if (ret)
		return ret;
	ret = platform_driver_register(&apple_typec_driver);
	if (ret)
		unregister_pm_notifier(&apple_typec_pm_notifier);
	return ret;
}

void dcp_typec_unregister(void)
{
	unregister_pm_notifier(&apple_typec_pm_notifier);
	platform_driver_unregister(&apple_typec_driver);
}
