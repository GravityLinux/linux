// SPDX-License-Identifier: GPL-2.0-only OR MIT
/* One permanently wired display engine per Type-C output. */
#include <linux/debugfs.h>
#include <linux/list.h>
#include <linux/module.h>
#include <linux/of_platform.h>
#include <linux/seq_file.h>
#include <linux/suspend.h>
#include <linux/usb/typec_dp.h>
#include "dcp.h"
#include "retirement.h"

struct apple_fixed_dp {
	struct device *dev;
	struct list_head node;
	struct platform_device *engine;
	struct apple_dcp *dcp;
	struct phy *phy;
	struct mux_control *xbar;
	struct typec_mux_dev *mux;
	struct dentry *debugfs;
	spinlock_t lock;
	struct work_struct work;
	bool hpd, irq, active_hpd;
	u32 sequence, target, source;
	unsigned long mode, active_mode;
	int error;
};

static DEFINE_MUTEX(display_lifecycle_lock);
static LIST_HEAD(fixed_ports);
static bool display_sleeping;

void dcp_lifecycle_lock(void) { mutex_lock(&display_lifecycle_lock); }
void dcp_lifecycle_unlock(void) { mutex_unlock(&display_lifecycle_lock); }
bool dcp_lifecycle_sleeping(void)
{
	lockdep_assert_held(&display_lifecycle_lock);
	return display_sleeping;
}
bool dcp_fixed_dp_hpd(struct apple_dcp *dcp)
{
	return dcp->fixed_dp && READ_ONCE(dcp->fixed_dp->hpd);
}

static void fixed_dp_work(struct work_struct *work)
{
	struct apple_fixed_dp *port = container_of(work, struct apple_fixed_dp, work);
	struct apple_dcp *dcp = port->dcp;
	bool hpd, irq;
	u32 sequence;
	unsigned long mode;
	int ret;

	dcp_lifecycle_lock();
	if (display_sleeping || !dcp->typec_routing_ready)
		goto unlock;
	for (;;) {
		spin_lock_irq(&port->lock);
		hpd = port->hpd;
		irq = port->irq;
		port->irq = false;
		sequence = port->sequence;
		mode = port->mode;
		spin_unlock_irq(&port->lock);
		ret = 0;
		if (!hpd) {
			if (dcp->dptxport[0].connected)
				ret = dcp_dptx_disconnect_oob(port->engine, 0);
			if (!ret && port->active_hpd) {
				port->active_hpd = false;
				dcp_retirement_unplug_locked(dcp);
			}
		} else {
			/* The provider and this permanent route stay registered while off. */
			ret = dcp_retirement_restart_locked(dcp);
			if (!ret && dcp->dptxport[0].connected && port->active_mode != mode)
				ret = dcp_dptx_disconnect_oob(port->engine, 0);
			if (!ret) {
				port->active_hpd = true;
				port->active_mode = mode;
				if (!dcp->dptxport[0].connected)
					ret = dcp_dptx_connect_oob(port->engine, 0);
				else if (irq)
					ret = dptxport_hpd_irq(dcp->dptxport[0].service);
			}
		}
		port->error = ret;
		dev_info(port->dev, "fixed DP ATC%u engine=%s HPD=%u IRQ=%u mode=%#lx ret=%d\n",
			 port->target, dev_name(dcp->dev), hpd, irq, mode, ret);
		spin_lock_irq(&port->lock);
		if (sequence == port->sequence) {
			spin_unlock_irq(&port->lock);
			break;
		}
		spin_unlock_irq(&port->lock);
	}
unlock:
	dcp_lifecycle_unlock();
}

static int fixed_dp_set(struct typec_mux_dev *mux, struct typec_mux_state *state)
{
	struct apple_fixed_dp *port = typec_mux_get_drvdata(mux);
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
	port->irq |= irq;
	port->mode = state->mode;
	port->sequence++;
	spin_unlock_irq(&port->lock);
	queue_work(system_unbound_wq, &port->work);
	return 0;
}

void dcp_typec_set_ready(struct apple_dcp *dcp, bool ready)
{
	dcp_lifecycle_lock();
	dcp->typec_routing_ready = ready;
	if (ready)
		dcp_retirement_ready_locked(dcp);
	if (dcp->fixed_dp)
		queue_work(system_unbound_wq, &dcp->fixed_dp->work);
	dcp_lifecycle_unlock();
}

static int fixed_dp_show(struct seq_file *m, void *data)
{
	struct apple_fixed_dp *port = m->private;
	dcp_lifecycle_lock();
	seq_printf(m, "engine=%s atc=%u source=%u static=1 ready=%u hpd=%u irq=%u mode=%#lx connected=%u last_lanes=%u last_rate_Mbps=%u error=%d\n",
		dev_name(port->dcp->dev), port->target, port->source,
		port->dcp->typec_routing_ready, READ_ONCE(port->hpd),
		READ_ONCE(port->irq), READ_ONCE(port->mode),
		port->dcp->dptxport[0].connected,
		port->dcp->dptxport[0].lane_count,
		port->dcp->dptxport[0].phy_ops.dp.link_rate, port->error);
	dcp_lifecycle_unlock();
	return 0;
}
DEFINE_SHOW_ATTRIBUTE(fixed_dp);

static void fixed_dp_stop(void *data)
{
	struct apple_fixed_dp *port = data;
	typec_mux_unregister(port->mux);
	dcp_lifecycle_lock();
	list_del(&port->node);
	port->dcp->fixed_dp = NULL;
	dcp_lifecycle_unlock();
	cancel_work_sync(&port->work);
	debugfs_remove_recursive(port->debugfs);
	mux_control_deselect(port->xbar);
	phy_power_off(port->phy);
	phy_exit(port->phy);
	port->dcp->phy = NULL;
	put_device(&port->engine->dev);
}

static int fixed_dp_probe(struct platform_device *pdev)
{
	struct device *dev = &pdev->dev;
	struct apple_fixed_dp *port;
	struct device_node *np;
	struct typec_mux_desc desc = {};
	char name[32];
	int ret;

	port = devm_kzalloc(dev, sizeof(*port), GFP_KERNEL);
	if (!port)
		return -ENOMEM;
	port->dev = dev;
	spin_lock_init(&port->lock);
	INIT_WORK(&port->work, fixed_dp_work);
	if (of_property_read_u32(dev->of_node, "apple,atc-index", &port->target) ||
	    of_property_read_u32(dev->of_node, "mux-index", &port->source))
		return -EINVAL;
	port->phy = devm_phy_get(dev, "dp-phy");
	if (IS_ERR(port->phy))
		return dev_err_probe(dev, PTR_ERR(port->phy), "Missing fixed DP PHY\n");
	port->xbar = devm_mux_control_get(dev, "dp-xbar");
	if (IS_ERR(port->xbar))
		return dev_err_probe(dev, PTR_ERR(port->xbar), "Missing fixed crossbar\n");
	np = of_parse_phandle(dev->of_node, "apple,display-engine", 0);
	port->engine = of_find_device_by_node(np);
	of_node_put(np);
	if (!port->engine)
		return -EPROBE_DEFER;
	port->dcp = platform_get_drvdata(port->engine);
	if (!port->dcp) { ret = -EPROBE_DEFER; goto put_engine; }
	if (!device_link_add(dev, &port->engine->dev, DL_FLAG_AUTOREMOVE_CONSUMER)) {
		ret = -ENOMEM;
		goto put_engine;
	}
	ret = phy_init(port->phy);
	if (ret) goto put_engine;
	ret = phy_power_on(port->phy);
	if (ret) goto exit_phy;
	/* Program once, not on HPD. No engine or port is ever reassigned. */
	ret = mux_control_select(port->xbar, port->source);
	if (ret) goto power_off;
	dcp_lifecycle_lock();
	if (port->dcp->fixed_dp || port->dcp->phy) {
		ret = -EBUSY;
		dcp_lifecycle_unlock();
		goto deselect;
	}
	port->dcp->fixed_dp = port;
	port->dcp->phy = port->phy;
	port->dcp->dptx_phy = port->target;
	list_add_tail(&port->node, &fixed_ports);
	dcp_lifecycle_unlock();
	desc.fwnode = dev_fwnode(dev);
	desc.set = fixed_dp_set;
	desc.drvdata = port;
	port->mux = typec_mux_register(dev, &desc);
	if (IS_ERR(port->mux)) {
		ret = PTR_ERR(port->mux);
		dcp_lifecycle_lock();
		list_del(&port->node);
		port->dcp->fixed_dp = NULL;
		port->dcp->phy = NULL;
		dcp_lifecycle_unlock();
		goto deselect;
	}
	snprintf(name, sizeof(name), "fixed-dp-atc%u", port->target);
	port->debugfs = debugfs_create_dir(name, NULL);
	debugfs_create_file("state", 0400, port->debugfs, port, &fixed_dp_fops);
	dev_info(dev, "Fixed %s source %u to ATC%u\n", dev_name(port->dcp->dev), port->source, port->target);
	queue_work(system_unbound_wq, &port->work);
	return devm_add_action_or_reset(dev, fixed_dp_stop, port);
deselect:
	mux_control_deselect(port->xbar);
power_off:
	phy_power_off(port->phy);
exit_phy:
	phy_exit(port->phy);
put_engine:
	put_device(&port->engine->dev);
	return ret;
}

static const struct of_device_id fixed_dp_match[] = {
	{ .compatible = "apple,t8132-fixed-dp" }, {}
};
MODULE_DEVICE_TABLE(of, fixed_dp_match);
static struct platform_driver fixed_dp_driver = {
	.probe = fixed_dp_probe,
	.driver = { .name = "apple-fixed-dp", .of_match_table = fixed_dp_match },
};

static int fixed_dp_pm_notify(struct notifier_block *nb, unsigned long action, void *data)
{
	struct apple_fixed_dp *port;
	if (action != PM_SUSPEND_PREPARE && action != PM_POST_SUSPEND)
		return NOTIFY_DONE;
	dcp_lifecycle_lock();
	display_sleeping = action == PM_SUSPEND_PREPARE;
	if (!display_sleeping)
		list_for_each_entry(port, &fixed_ports, node)
			queue_work(system_unbound_wq, &port->work);
	dcp_lifecycle_unlock();
	return NOTIFY_OK;
}
static struct notifier_block fixed_dp_pm_notifier = { .notifier_call = fixed_dp_pm_notify };
int dcp_typec_register(void)
{
	int ret = register_pm_notifier(&fixed_dp_pm_notifier);
	if (ret) return ret;
	ret = platform_driver_register(&fixed_dp_driver);
	if (ret) unregister_pm_notifier(&fixed_dp_pm_notifier);
	return ret;
}
void dcp_typec_unregister(void)
{
	platform_driver_unregister(&fixed_dp_driver);
	unregister_pm_notifier(&fixed_dp_pm_notifier);
}
