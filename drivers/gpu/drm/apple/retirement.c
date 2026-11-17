// SPDX-License-Identifier: GPL-2.0-only OR MIT
/* Opt-in T8132 display-engine lifecycle. Keep all providers bound. */
#include <linux/debugfs.h>
#include <drm/drm_modeset_lock.h>
#include <linux/iopoll.h>
#include <linux/module.h>
#include <linux/mfd/syscon.h>
#include <linux/regmap.h>
#include <linux/of.h>
#include <linux/of_graph.h>
#include <linux/of_platform.h>
#include <linux/pm_runtime.h>
#include <linux/seq_file.h>
#include <linux/soc/apple/pmgr.h>
#include <linux/soc/apple/dart.h>
#include <linux/soc/apple/pmp-report.h>
#include <linux/soc/apple/rtkit.h>
#include <linux/uaccess.h>

#include "afk.h"
#include "av.h"
#include "iomfb_v26_6.h"
#include "dcp.h"
#include "retirement.h"
#include "trace.h"

#define DCP_CPU_CONTROL 0x44
#define DCP_CPU_RUN BIT(4)
#define DCP_IDLE_GRACE msecs_to_jiffies(70000)

static bool retire_unused;
module_param(retire_unused, bool, 0444);
MODULE_PARM_DESC(retire_unused, "Experimental T8132 unused display-engine retirement (default off)");

bool dcp_retirement_policy_enabled(void)
{
	return retire_unused && of_machine_is_compatible("apple,t8132");
}

enum dcp_idle_state { DCP_IDLE_BOOTING, DCP_IDLE_READY, DCP_IDLE_RETIRING,
	DCP_IDLE_RETIRED, DCP_IDLE_WAKING, DCP_IDLE_FAILED };
static const char * const idle_names[] = {
	"booting", "ready", "retiring", "retired", "waking", "failed",
};

struct dcp_retirement {
	struct apple_dcp *dcp;
	struct delayed_work work;
	struct dentry *debugfs;
	struct device_node *cpu_node, *fe_node, *sys_node, *report_node;
	struct regmap *shared;
	u32 shared_request_bit, shared_power_bit;
	struct device *report_owner;
	spinlock_t rpc_lock;
	unsigned int rpc_count;
	bool rpc_blocked, ever_used, stopping, pm_held, piodma_held, av_was_open;
	bool primary, logical_off, manual_disable, resume_retired, hpd_parked;
	enum dcp_idle_state state;
	const char *stage;
	int error;
	u32 saved_run;
	unsigned int retire_count, restart_count;
};

static void idle_stage(struct dcp_retirement *idle, const char *stage)
{
	idle->stage = stage;
	trace_dcp_retirement_stage(idle->dcp, stage);
}

bool dcp_retirement_is_primary(struct apple_dcp *dcp)
{
	return dcp->retirement && dcp->retirement->primary;
}

bool dcp_retirement_transport_required(struct apple_dcp *dcp)
{
	return dcp->retirement != NULL;
}

bool dcp_retirement_defer_poweroff(struct apple_dcp *dcp)
{
	if (!dcp_retirement_is_primary(dcp) ||
	    !READ_ONCE(dcp->retirement->manual_disable))
		return false;
	WRITE_ONCE(dcp->retirement->manual_disable, false);
	WRITE_ONCE(dcp->retirement->logical_off, true);
	dcp->valid_mode = false;
	dev_info(dcp->dev, "CRTC disabled; retaining firmware until explicit retirement\n");
	return true;
}

bool dcp_retirement_logically_off(struct apple_dcp *dcp)
{
	return dcp_retirement_is_primary(dcp) &&
		READ_ONCE(dcp->retirement->logical_off);
}

bool dcp_retirement_retained_display(struct apple_dcp *dcp)
{
	return dcp_retirement_is_primary(dcp) && dcp->retirement->retire_count;
}

bool dcp_retirement_display_was_used(struct apple_dcp *dcp)
{
	return dcp->retirement && READ_ONCE(dcp->retirement->ever_used);
}

int dcp_retirement_rpc_get(struct apple_dcp *dcp)
{
	struct dcp_retirement *idle = dcp->retirement;
	unsigned long flags;
	int ret = 0;

	if (!idle)
		return 0;
	spin_lock_irqsave(&idle->rpc_lock, flags);
	if (idle->rpc_blocked)
		ret = -EHOSTDOWN;
	else
		idle->rpc_count++;
	spin_unlock_irqrestore(&idle->rpc_lock, flags);
	return ret;
}

void dcp_retirement_rpc_put(struct apple_dcp *dcp)
{
	struct dcp_retirement *idle = dcp->retirement;
	unsigned long flags;

	if (!idle)
		return;
	spin_lock_irqsave(&idle->rpc_lock, flags);
	WARN_ON(!idle->rpc_count);
	idle->rpc_count--;
	spin_unlock_irqrestore(&idle->rpc_lock, flags);
}

static int idle_block_rpc(struct dcp_retirement *idle)
{
	unsigned long flags;
	int ret = 0;

	spin_lock_irqsave(&idle->rpc_lock, flags);
	if (idle->rpc_count)
		ret = -EBUSY;
	else
		idle->rpc_blocked = true;
	spin_unlock_irqrestore(&idle->rpc_lock, flags);
	return ret;
}

static void idle_unblock_rpc(struct dcp_retirement *idle)
{
	unsigned long flags;

	spin_lock_irqsave(&idle->rpc_lock, flags);
	idle->rpc_blocked = false;
	spin_unlock_irqrestore(&idle->rpc_lock, flags);
}

void dcp_retirement_note_use(struct apple_dcp *dcp)
{
	if (dcp->retirement)
		WRITE_ONCE(dcp->retirement->ever_used, true);
}

bool dcp_retirement_failed(struct apple_dcp *dcp)
{
	return dcp->retirement &&
		READ_ONCE(dcp->retirement->state) == DCP_IDLE_FAILED;
}

bool dcp_retirement_is_retired(struct apple_dcp *dcp)
{
	return dcp->retirement &&
		READ_ONCE(dcp->retirement->state) == DCP_IDLE_RETIRED;
}

static int idle_failure(struct dcp_retirement *idle, int ret)
{
	idle->error = ret;
	idle->state = DCP_IDLE_FAILED;
	/* Keep outstanding callers' buffers alive; reject all new requests. */
	spin_lock_irq(&idle->rpc_lock);
	idle->rpc_blocked = true;
	spin_unlock_irq(&idle->rpc_lock);
	dev_err(idle->dcp->dev, "retirement failed at %s: %d (no retry)\n",
		idle->stage, ret);
	return ret;
}

static bool idle_eligible(struct apple_dcp *dcp)
{
	struct dcp_retirement *idle = dcp->retirement;

	return idle && !idle->stopping && !dcp->typec_port &&
		(idle->primary ? (READ_ONCE(idle->logical_off) && dcp->crtc &&
		 !dcp->crtc->base.state->active) :
		 (!READ_ONCE(idle->ever_used) && !dcp->phy)) &&
		!dcp->crashed && dcp->active &&
		!dcp->valid_mode && !dcp->during_modeset &&
		(idle->primary || !READ_ONCE(dcp->connector->connected)) &&
		!dcp->ch_cmd.depth && !dcp->ch_oobcmd.depth &&
		!dcp->ch_cb.depth && !dcp->ch_oobcb.depth &&
		!dcp->ch_async.depth && !dcp->ch_oobasync.depth;
}

static unsigned int idle_endpoints(struct apple_dcp *dcp,
				  struct apple_dcp_afkep **eps)
{
	struct apple_dcp_afkep *all[] = {
		dcp->systemep, dcp->ibootep, dcp->dptxep, dcp->avep,
		dcp->avauxep, dcp->dpavctrlep, dcp->dpdevep, dcp->dpsacep,
		dcp->remoteallocep, dcp->epicep2c, dcp->expertep,
		dcp->epicep25, dcp->hdcpep, dcp->dcpavservep,
	};
	unsigned int i, n = 0;

	for (i = 0; i < ARRAY_SIZE(all); i++)
		if (!IS_ERR_OR_NULL(all[i]))
			eps[n++] = all[i];
	return n;
}

static int idle_drain_services(struct apple_dcp *dcp)
{
	struct apple_dcp_afkep *eps[16];
	unsigned int i, j, n = idle_endpoints(dcp, eps);
	unsigned long flags;
	bool busy;

	for (i = 0; i < n; i++) {
		flush_workqueue(eps[i]->wq);
		spin_lock_irqsave(&eps[i]->lock, flags);
		busy = !list_empty(&eps[i]->compact_cmds);
		spin_unlock_irqrestore(&eps[i]->lock, flags);
		if (busy)
			return -EBUSY;
		for (j = 0; j < eps[i]->num_channels; j++) {
			struct apple_epic_service *srv = &eps[i]->services[j];

			spin_lock_irqsave(&srv->lock, flags);
			busy = !bitmap_empty(srv->cmd_map, MAX_PENDING_CMDS);
			spin_unlock_irqrestore(&srv->lock, flags);
			if (busy)
				return -EBUSY;
		}
	}
	return 0;
}

static int idle_prepare_endpoints(struct apple_dcp *dcp)
{
	struct apple_dcp_afkep *eps[16], *tmp;
	unsigned int i, j, n = idle_endpoints(dcp, eps);
	int ret;

	/* Native retirement walks every started transport in descending order. */
	for (i = 0; i < n; i++)
		for (j = i + 1; j < n; j++)
			if (eps[i]->endpoint < eps[j]->endpoint) {
				tmp = eps[i]; eps[i] = eps[j]; eps[j] = tmp;
			}
	for (i = 0; i < n; i++) {
		ret = afk_retained_idle(eps[i]);
		if (ret)
			return ret;
	}
	return 0;
}

static int idle_resume_endpoints(struct apple_dcp *dcp)
{
	struct apple_dcp_afkep *eps[16], *tmp;
	unsigned int i, j, n = idle_endpoints(dcp, eps);
	int ret;

	/* The retained reconnect walks the existing transports upwards. It
	 * does not allocate new rings or restart the RTKit endpoints.
	 */
	for (i = 0; i < n; i++)
		for (j = i + 1; j < n; j++)
			if (eps[i]->endpoint > eps[j]->endpoint) {
				tmp = eps[i]; eps[i] = eps[j]; eps[j] = tmp;
			}
	for (i = 0; i < n; i++) {
		ret = afk_retained_resume(eps[i]);
		if (ret)
			return ret;
		flush_workqueue(eps[i]->wq);
	}
	return 0;
}

static int idle_check_domains(struct dcp_retirement *idle)
{
	u32 cpu, fe, sys;
	int ret;

	ret = apple_pmgr_read_state(idle->cpu_node, &cpu);
	if (!ret)
		ret = apple_pmgr_read_state(idle->fe_node, &fe);
	if (!ret)
		ret = apple_pmgr_read_state(idle->sys_node, &sys);
	if (ret)
		return ret;
	/* These registers remain accessible with the display island off. */
	return ((cpu | fe | sys) & 0xff) ? -EBUSY : 0;
}

/* Shared display requests are serialized by the display-engine pool lock.
 * Preserve other engines' votes; the notification is the request population.
 */
static int idle_shared_lifecycle(struct dcp_retirement *idle, bool on)
{
	const u32 offsets[] = { 0xc028, 0x4000, 0x9c14, 0xc020 };
	unsigned int enable, request, power, value[4];
	int i, ret;

	if (on) {
		ret = regmap_update_bits(idle->shared, 0xc024, 3, 3);
		if (ret)
			return ret;
	}
	ret = regmap_read(idle->shared, 0xc028, &enable);
	if (!ret)
		ret = regmap_read(idle->shared, 0x4000, &request);
	if (!ret)
		ret = regmap_read(idle->shared, 0xc020, &power);
	if (ret)
		return ret;
	value[0] = on ? enable | BIT(idle->shared_request_bit) :
		enable & ~BIT(idle->shared_request_bit);
	value[1] = on ? request | BIT(idle->shared_request_bit) :
		request & ~BIT(idle->shared_request_bit);
	value[2] = hweight32(value[1] & 7);
	value[3] = on ? power | BIT(idle->shared_power_bit) :
		power & ~BIT(idle->shared_power_bit);
	for (i = 0; i < ARRAY_SIZE(offsets); i++) {
		ret = regmap_write(idle->shared, offsets[i], value[i]);
		if (ret)
			return ret;
	}
	return 0;
}

static int idle_cpu_off(struct dcp_retirement *idle)
{
	struct apple_dcp *dcp = idle->dcp;
	int ret;

	if (idle->report_owner) {
		idle_stage(idle, "PMP-vote-off");
		ret = pm_runtime_put_sync_suspend(idle->report_owner);
		if (ret < 0)
			return ret;
	}
	idle_stage(idle, "shared-request-off");
	ret = idle_shared_lifecycle(idle, false);
	if (ret)
		return ret;
	idle_stage(idle, "CPU-off");
	/* The domain provider owns automatic control and the gate sequencer. */
	ret = pm_runtime_put_sync_suspend(dcp->cpu_pm_dev);
	if (ret < 0)
		return ret;
	dcp->cpu_pm_held = false;
	return 0;
}

static int idle_retire_locked(struct apple_dcp *dcp)
{
	struct dcp_retirement *idle = dcp->retirement;
	u32 ctrl;
	int ret;

	if (!idle)
		return -EOPNOTSUPP;
	if (idle->state == DCP_IDLE_RETIRED)
		return 0;
	if (idle->state != DCP_IDLE_READY || !idle_eligible(dcp))
		return -EBUSY;
	if (!idle->report_owner && idle->report_node) {
#if IS_REACHABLE(CONFIG_APPLE_PMP_REPORT)
		idle->report_owner = devm_apple_pmp_report_idle_owner(dcp->dev,
								 idle->report_node);
		if (IS_ERR(idle->report_owner)) {
			ret = PTR_ERR(idle->report_owner);
			idle->report_owner = NULL;
			return ret;
		}
#else
		if (of_property_read_bool(idle->report_node, "apple,always-on"))
			return -EOPNOTSUPP;
#endif
	}
	idle->error = 0;
	idle_stage(idle, "quiesce-services");
	ret = dcp_av_retire(dcp, &idle->av_was_open);
	if (ret)
		goto abort_quiesce;
	idle_stage(idle, idle->primary ? "display-state-off/A500" : "engine-power/A500");
	ret = iomfb_retained_display_off_v26_6_0(dcp, idle->primary && idle->ever_used);
	if (ret)
		return idle_failure(idle, ret);
	idle_stage(idle, "block-RPC");
	ret = idle_block_rpc(idle);
	if (ret)
		goto abort_quiesce;
	idle_stage(idle, "drain-services");
	ret = idle_drain_services(dcp);
	if (ret)
		goto abort_quiesce;
	idle_stage(idle, "check-unclaimed");
	if (!idle_eligible(dcp)) {
		ret = -EBUSY;
		goto abort_quiesce;
	}
	idle_stage(idle, "AFK-control2");
	ret = idle_prepare_endpoints(dcp);
	if (ret)
		return idle_failure(idle, ret);
	idle_stage(idle, "shared-request-on");
	ret = idle_shared_lifecycle(idle, true);
	if (ret)
		return idle_failure(idle, ret);
	if (dcp->hdmi_hpd_irq) {
		disable_irq(dcp->hdmi_hpd_irq);
		idle->hpd_parked = true;
	}
	idle->state = DCP_IDLE_RETIRING;
	idle_stage(idle, "AP-201");
	ret = apple_rtkit_idle_ap(dcp->rtk);
	if (ret)
		return idle_failure(idle, ret);
	idle_stage(idle, "IOP-201");
	ret = apple_rtkit_idle_iop(dcp->rtk);
	if (ret)
		return idle_failure(idle, ret);
	idle_stage(idle, "park-mailbox");
	ret = apple_rtkit_park(dcp->rtk);
	if (ret)
		return idle_failure(idle, ret);
	ret = idle_drain_services(dcp);
	if (ret)
		return idle_failure(idle, ret);
	idle_stage(idle, "clear-RUN");
	ctrl = readl(dcp->coproc_reg + DCP_CPU_CONTROL);
	idle->saved_run = ctrl;
	writel(ctrl & ~DCP_CPU_RUN, dcp->coproc_reg + DCP_CPU_CONTROL);
	/* Complete the stopped-core store before withdrawing its DMA paths. */
	if (readl(dcp->coproc_reg + DCP_CPU_CONTROL) & DCP_CPU_RUN)
		return idle_failure(idle, -EIO);
	idle_stage(idle, "DART-streams-off");
	ret = apple_dart_retained_streams(dcp->dev, false);
	if (!ret)
		ret = apple_dart_retained_streams(&dcp->piodma->dev, false);
	if (ret)
		return idle_failure(idle, ret);
	ret = idle_cpu_off(idle);
	if (ret)
		return idle_failure(idle, ret);
	idle_stage(idle, "PIODMA-DART-idle");
	idle->piodma_held = false;
	ret = pm_runtime_put_sync_suspend(&dcp->piodma->dev);
	if (ret < 0)
		return idle_failure(idle, ret);
	idle_stage(idle, "FE/SYS-off");
	idle->pm_held = false;
	ret = pm_runtime_put_sync_suspend(dcp->dev);
	if (ret < 0)
		return idle_failure(idle, ret);
	ret = read_poll_timeout(idle_check_domains, ret, !ret, 1000, 1000000, false, idle);
	if (ret)
		return idle_failure(idle, ret);
	idle->state = DCP_IDLE_RETIRED;
	idle_stage(idle, "off");
	idle->retire_count++;
	dev_info(dcp->dev, "retirement: powered off engine (%u)\n", idle->retire_count);
	return 0;
abort_quiesce:
	idle->error = ret;
	dev_info(dcp->dev, "retirement deferred at %s: %d\n", idle->stage, ret);
	idle_unblock_rpc(idle);
	if (dcp_av_restart(dcp, idle->av_was_open))
		return idle_failure(idle, -EIO);
	return ret;
}

static int idle_restart_locked(struct apple_dcp *dcp, bool system_resume)
{
	struct dcp_retirement *idle = dcp->retirement;
	int ret;

	if (!idle || idle->state == DCP_IDLE_READY)
		return 0;
	if (idle->state != DCP_IDLE_RETIRED)
		return -EHOSTDOWN;
	if (!system_resume && dcp_typec_pool_sleeping())
		return -EBUSY;
	idle->state = DCP_IDLE_WAKING;
	idle_stage(idle, "SYS/FE-on");
	ret = pm_runtime_resume_and_get(dcp->dev);
	if (ret < 0)
		return idle_failure(idle, ret);
	idle->pm_held = true;
	idle_stage(idle, "CPU-on");
	ret = pm_runtime_resume_and_get(dcp->cpu_pm_dev);
	if (ret < 0)
		return idle_failure(idle, ret);
	dcp->cpu_pm_held = true;
	if (idle->report_owner) {
		idle_stage(idle, "PMP-vote-on");
		ret = pm_runtime_resume_and_get(idle->report_owner);
		if (ret < 0)
			return idle_failure(idle, ret);
	}
	ret = idle_shared_lifecycle(idle, true);
	if (ret)
		return idle_failure(idle, ret);
	idle_stage(idle, "PIODMA-DART-restore");
	ret = pm_runtime_resume_and_get(&dcp->piodma->dev);
	if (ret < 0)
		return idle_failure(idle, ret);
	idle->piodma_held = true;
	ret = apple_dart_retained_streams(dcp->dev, true);
	if (!ret)
		ret = apple_dart_retained_streams(&dcp->piodma->dev, true);
	if (ret)
		return idle_failure(idle, ret);
	idle_stage(idle, "rearm-management/inbox");
	ret = apple_rtkit_rearm_retained(dcp->rtk);
	if (ret)
		return idle_failure(idle, ret);
	idle_stage(idle, "RUN");
	writel(idle->saved_run | DCP_CPU_RUN, dcp->coproc_reg + DCP_CPU_CONTROL);
	if (!(readl(dcp->coproc_reg + DCP_CPU_CONTROL) & DCP_CPU_RUN))
		return idle_failure(idle, -EIO);
	idle_stage(idle, "firmware-Hello/epmap/ON");
	ret = apple_rtkit_boot_retained(dcp->rtk);
	if (ret)
		return idle_failure(idle, ret);
	/* Core startup is fresh, but firmware memory and application rings survive. */
	idle_stage(idle, "retained-transports-resume");
	ret = idle_resume_endpoints(dcp);
	if (ret)
		return idle_failure(idle, ret);
	idle_unblock_rpc(idle);
	idle_stage(idle, "retained-DPTX-service");
	if (!dcp->active || !dcp->dptxport[0].service)
		return idle_failure(idle, -ENODEV);
	ret = dptxport_validate_connection(dcp->dptxport[0].service, 0,
					 dcp->dptx_phy, dcp->dptx_die);
	if (ret)
		return idle_failure(idle, ret);
	ret = dcp_av_restart(dcp, idle->av_was_open);
	if (ret)
		return idle_failure(idle, ret);
	idle->state = DCP_IDLE_READY;
	idle_stage(idle, "ready");
	idle->restart_count++;
	if (!idle->primary)
		mod_delayed_work(system_unbound_wq, &idle->work, DCP_IDLE_GRACE);
	if (idle->primary) {
		idle->logical_off = false;
		dcp->hotplug_keepalive = false;
	}
	if (idle->hpd_parked) {
		enable_irq(dcp->hdmi_hpd_irq);
		idle->hpd_parked = false;
	}
	dev_info(dcp->dev, "retirement: ready, retained services answered (%u)\n",
		 idle->restart_count);
	return 0;
}

int dcp_retirement_restart_locked(struct apple_dcp *dcp)
{
	return idle_restart_locked(dcp, false);
}

static void idle_work(struct work_struct *work)
{
	struct dcp_retirement *idle = container_of(to_delayed_work(work),
						 struct dcp_retirement, work);
	int ret;

	dcp_typec_pool_lock();
	ret = idle_retire_locked(idle->dcp);
	if (ret == -EBUSY && !idle->stopping && !idle->ever_used &&
	    idle->state == DCP_IDLE_READY)
		mod_delayed_work(system_unbound_wq, &idle->work, DCP_IDLE_GRACE);
	dcp_typec_pool_unlock();
}

void dcp_retirement_ready_locked(struct apple_dcp *dcp)
{
	if (!dcp->retirement || dcp->retirement->state != DCP_IDLE_BOOTING)
		return;
	dcp->retirement->state = DCP_IDLE_READY;
	dcp->retirement->stage = "ready";
	if (!dcp->retirement->primary)
		mod_delayed_work(system_unbound_wq, &dcp->retirement->work, DCP_IDLE_GRACE);
}

void dcp_retirement_unplug_locked(struct apple_dcp *dcp)
{
	/* A previously claimed engine is intentionally ineligible for now. */
	if (dcp->retirement && !dcp->retirement->primary &&
	    !dcp->retirement->stopping)
		mod_delayed_work(system_unbound_wq, &dcp->retirement->work, DCP_IDLE_GRACE);
}

int dcp_retirement_suspend(struct apple_dcp *dcp)
{
	struct dcp_retirement *idle = dcp->retirement;
	int ret;

	/* Keep the existing after-real-use workaround until it is qualified. */
	if (!idle || READ_ONCE(idle->ever_used) || dcp->typec_port)
		return 0;
	if (idle->primary && dcp->crtc && !dcp->crtc->base.state->active &&
	    !READ_ONCE(dcp->connector->connected))
		WRITE_ONCE(idle->logical_off, true);
	dcp_typec_pool_lock();
	ret = idle_retire_locked(dcp);
	if (!ret)
		idle->resume_retired = true;
	dcp_typec_pool_unlock();
	return ret ? ret : 1;
}

int dcp_retirement_resume(struct apple_dcp *dcp)
{
	struct dcp_retirement *idle = dcp->retirement;
	int ret;

	if (!idle || !idle->resume_retired)
		return 0;
	dcp_typec_pool_lock();
	/* Pool claims stay blocked until PM_POST_SUSPEND, after device resume. */
	ret = idle_restart_locked(dcp, true);
	if (!ret)
		idle->resume_retired = false;
	dcp_typec_pool_unlock();
	return ret ? ret : 1;
}

int dcp_retirement_runtime_suspend(struct device *dev)
{
	struct apple_dcp *dcp = dev_get_drvdata(dev);

	int ret;

	if (!dcp->retirement || dcp->retirement->state != DCP_IDLE_RETIRING)
		return -EBUSY;
	if (!dcp->fe_pm_dev || !dcp->fe_pm_held)
		return 0;
	ret = pm_runtime_put_sync_suspend(dcp->fe_pm_dev);
	if (ret < 0)
		return ret;
	dcp->fe_pm_held = false;
	return 0;
}

int dcp_retirement_runtime_resume(struct device *dev)
{
	struct apple_dcp *dcp = dev_get_drvdata(dev);
	int ret;

	if (!dcp->fe_pm_dev || dcp->fe_pm_held)
		return 0;
	ret = pm_runtime_resume_and_get(dcp->fe_pm_dev);
	if (ret < 0)
		return ret;
	dcp->fe_pm_held = true;
	return 0;
}

int dcp_retirement_mapping_context(struct device *dev)
{
	if (!dcp_retirement_policy_enabled())
		return 0;
	/*
	 * The DRM allocation context does not issue DMA. DART map/unmap
	 * operations take their own temporary RPM references; the real DCP owns
	 * the persistent hardware vote, including its PIODMA child. Drop only
	 * the allocation context's redundant initial DL_FLAG_RPM_ACTIVE vote.
	 */
	return pm_runtime_set_suspended(dev);
}

static int idle_show(struct seq_file *m, void *data)
{
	struct apple_dcp *dcp = m->private;
	struct dcp_retirement *idle = dcp->retirement;
	u32 cpu = 0, fe = 0, sys = 0, ap, iop;

	dcp_typec_pool_lock();
	apple_rtkit_power_state(dcp->rtk, &ap, &iop);
	seq_printf(m, "RTKit_AP=%#x IOP=%#x\n", ap, iop);
	apple_pmgr_read_state(idle->cpu_node, &cpu);
	apple_pmgr_read_state(idle->fe_node, &fe);
	apple_pmgr_read_state(idle->sys_node, &sys);
	seq_printf(m, "state=%s stage=%s error=%d used=%u rpc=%u blocked=%u retire=%u restart=%u\n",
		idle_names[idle->state], idle->stage, idle->error, idle->ever_used,
		idle->rpc_count, idle->rpc_blocked, idle->retire_count, idle->restart_count);
	seq_printf(m, "CPU=%#x FE=%#x SYS=%#x saved_RUN=%#x\n", cpu, fe, sys, idle->saved_run);
	seq_printf(m, "runtime_suspended=%u usage=%d RTKit_running=%u RTKit_crashed=%u\n",
		pm_runtime_suspended(dcp->dev), atomic_read(&dcp->dev->power.usage_count),
		apple_rtkit_is_running(dcp->rtk), apple_rtkit_is_crashed(dcp->rtk));
	{
		struct device_node *ep, *audio, *pd;
		u32 value;

		ep = of_graph_get_endpoint_by_regs(dcp->dev->of_node, 0, 0);
		audio = of_graph_get_remote_port_parent(ep);
		of_node_put(ep);
		pd = of_parse_phandle(audio, "power-domains", 0);
		if (pd && !apple_pmgr_read_state(pd, &value))
			seq_printf(m, "DPA=%#x\n", value);
		of_node_put(pd);
		of_node_put(audio);
	}
#if IS_REACHABLE(CONFIG_APPLE_PMP_REPORT)
	{
		struct device_node *node;
		struct platform_device *provider;
		u64 target, actual, status;

		node = of_find_compatible_node(NULL, NULL, "apple,t8132-pmp-v2-report");
		provider = node ? of_find_device_by_node(node) : NULL;
		of_node_put(node);
		if (provider) {
			if (!apple_pmp_report_snapshot(&provider->dev, &target, &actual, &status))
				seq_printf(m, "PMP_target=%#llx actual=%#llx status=%#llx\n",
					 target, actual, status);
			put_device(&provider->dev);
		}
	}
#endif
	if ((fe & 0xf0) == 0xf0) {
		u32 fw, scanout;

		seq_printf(m, "CPU_CONTROL=%#x\n", readl(dcp->coproc_reg + DCP_CPU_CONTROL));
		if (!apple_dart_retained_state(dcp->dev, &fw) &&
		    !apple_dart_retained_state(&dcp->piodma->dev, &scanout))
			seq_printf(m, "DART_enabled_fw=%#x scanout=%#x\n", fw, scanout);
	}
	dcp_typec_pool_unlock();
	return 0;
}
DEFINE_SHOW_ATTRIBUTE(idle);

static ssize_t idle_control_write(struct file *file, const char __user *buf,
				  size_t len, loff_t *pos)
{
	struct apple_dcp *dcp = file->private_data;
	char command[16];
	int ret;

	if (!len || len >= sizeof(command))
		return -EINVAL;
	if (copy_from_user(command, buf, len))
		return -EFAULT;
	command[len] = 0;
	if (dcp_retirement_is_primary(dcp))
		drm_modeset_lock_all(dcp->crtc->base.dev);
	dcp_typec_pool_lock();
	if (sysfs_streq(command, "prepare")) {
		/* Explicit primary-output test only: arm the next DRM disable.
		 * Ordinary DPMS and hot-unplug retain the existing keepalive path.
		 */
		if (!dcp_retirement_is_primary(dcp) ||
		    dcp->retirement->state != DCP_IDLE_READY)
			ret = -EINVAL;
		else {
			WRITE_ONCE(dcp->retirement->manual_disable, true);
			ret = 0;
		}
	} else if (sysfs_streq(command, "retire")) {
		/* An unused disconnected main output is already logically off.
		 * Do not require a modeset solely to exercise its firmware cycle.
		 */
		if (dcp_retirement_is_primary(dcp) &&
		    !dcp_retirement_display_was_used(dcp) && dcp->crtc &&
		    !dcp->crtc->base.state->active && dcp->connector &&
		    !READ_ONCE(dcp->connector->connected))
			WRITE_ONCE(dcp->retirement->logical_off, true);
		ret = idle_retire_locked(dcp);
	} else if (sysfs_streq(command, "restart"))
		ret = dcp_retirement_restart_locked(dcp);
	else
		ret = -EINVAL;
	dcp_typec_pool_unlock();
	if (dcp_retirement_is_primary(dcp))
		drm_modeset_unlock_all(dcp->crtc->base.dev);
	return ret ? ret : len;
}
static const struct file_operations idle_control_fops = {
	.owner = THIS_MODULE, .open = simple_open, .write = idle_control_write,
	.llseek = noop_llseek,
};

int dcp_retirement_init(struct apple_dcp *dcp)
{
	struct dcp_retirement *idle;
	int ret;

	if (!dcp_retirement_policy_enabled() || !dcp->cpu_pm_dev ||
	    dcp->fw_compat != DCP_FIRMWARE_V_26_6 ||
	    (!of_device_is_compatible(dcp->dev->of_node, "apple,t8132-dcpext") &&
	     !((of_machine_is_compatible("apple,j773g") ||
	       of_machine_is_compatible("apple,j713")) &&
	       of_device_is_compatible(dcp->dev->of_node, "apple,t8132-dcp"))))
		return 0;
	idle = devm_kzalloc(dcp->dev, sizeof(*idle), GFP_KERNEL);
	if (!idle)
		return -ENOMEM;
	idle->dcp = dcp;
	idle->primary = (of_machine_is_compatible("apple,j773g") ||
	       of_machine_is_compatible("apple,j713")) &&
		of_device_is_compatible(dcp->dev->of_node, "apple,t8132-dcp");
	idle_stage(idle, "booting");
	spin_lock_init(&idle->rpc_lock);
	INIT_DELAYED_WORK(&idle->work, idle_work);
	idle->cpu_node = of_parse_phandle(dcp->dev->of_node, "power-domains", 1);
	idle->fe_node = of_parse_phandle(dcp->dev->of_node, "power-domains", 0);
	idle->sys_node = of_parse_phandle(idle->fe_node, "power-domains", 0);
	idle->report_node = of_parse_phandle(dcp->dev->of_node, "apple,retirement-report", 0);
	if (!idle->cpu_node || !idle->fe_node || !idle->sys_node) {
		ret = -EINVAL;
		goto put_nodes;
	}
	{
		struct device_node *parent = of_parse_phandle(idle->cpu_node, "power-domains", 0);

		ret = parent == idle->fe_node ? 0 : -EINVAL;
		of_node_put(parent);
		if (ret)
			goto put_nodes;
	}
	{
		u32 bits[2];

		ret = of_property_read_u32_array(dcp->dev->of_node,
						"apple,retirement-shared-bits", bits, 2);
		if (ret || bits[0] > 2 || bits[1] > 31) {
			ret = -EINVAL;
			goto put_nodes;
		}
		idle->shared_request_bit = bits[0];
		idle->shared_power_bit = bits[1];
		idle->shared = syscon_regmap_lookup_by_phandle(dcp->dev->of_node,
						"apple,retirement-shared-control");
		if (IS_ERR(idle->shared)) {
			ret = PTR_ERR(idle->shared);
			goto put_nodes;
		}
	}
	pm_runtime_get_noresume(dcp->dev);
	ret = pm_runtime_set_active(dcp->dev);
	if (ret) {
		pm_runtime_put_noidle(dcp->dev);
		goto put_nodes;
	}
	pm_runtime_enable(dcp->dev);
	idle->pm_held = true;
	/* Keep both firmware and PIODMA/scanout DART contexts live while the
	 * coprocessor can DMA. Restore them before CPU/RUN on a later wake.
	 */
	pm_runtime_get_noresume(&dcp->piodma->dev);
	ret = pm_runtime_set_active(&dcp->piodma->dev);
	if (ret) {
		pm_runtime_put_noidle(&dcp->piodma->dev);
		pm_runtime_disable(dcp->dev);
		pm_runtime_put_noidle(dcp->dev);
		goto put_nodes;
	}
	pm_runtime_enable(&dcp->piodma->dev);
	idle->piodma_held = true;
	dcp->retirement = idle;
	idle->debugfs = debugfs_create_dir(dev_name(dcp->dev), NULL);
	debugfs_create_file("retirement_state", 0400, idle->debugfs, dcp, &idle_fops);
	debugfs_create_file("retirement", 0200, idle->debugfs, dcp, &idle_control_fops);
	return 0;
put_nodes:
	of_node_put(idle->report_node);
	of_node_put(idle->sys_node);
	of_node_put(idle->fe_node);
	of_node_put(idle->cpu_node);
	return ret;
}

void dcp_retirement_stop(struct apple_dcp *dcp)
{
	struct dcp_retirement *idle = dcp->retirement;

	if (!idle)
		return;
	WRITE_ONCE(idle->stopping, true);
	debugfs_remove_recursive(idle->debugfs);
	cancel_delayed_work_sync(&idle->work);
	pm_runtime_disable(&dcp->piodma->dev);
	if (idle->piodma_held)
		pm_runtime_put_noidle(&dcp->piodma->dev);
	/* Keep an idle engine idle on removal; never attempt an untested recovery. */
	of_node_put(idle->report_node);
	of_node_put(idle->sys_node);
	of_node_put(idle->fe_node);
	of_node_put(idle->cpu_node);
}

void dcp_retirement_fini(struct apple_dcp *dcp)
{
	if (!dcp->retirement)
		return;
	pm_runtime_disable(dcp->dev);
	if (dcp->retirement->pm_held)
		pm_runtime_put_noidle(dcp->dev);
	dcp->retirement = NULL;
}
