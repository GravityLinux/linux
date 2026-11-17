/* SPDX-License-Identifier: GPL-2.0-only OR MIT */
#ifndef __APPLE_DCP_RETIREMENT_H__
#define __APPLE_DCP_RETIREMENT_H__
#include <linux/types.h>
struct apple_dcp;
struct device;
bool dcp_retirement_policy_enabled(void);
bool dcp_retirement_transport_required(struct apple_dcp *dcp);
bool dcp_retirement_is_primary(struct apple_dcp *dcp);
bool dcp_retirement_defer_poweroff(struct apple_dcp *dcp);
bool dcp_retirement_retained_display(struct apple_dcp *dcp);
bool dcp_retirement_display_was_used(struct apple_dcp *dcp);
bool dcp_retirement_logically_off(struct apple_dcp *dcp);

int dcp_retirement_init(struct apple_dcp *dcp);
void dcp_retirement_stop(struct apple_dcp *dcp);
void dcp_retirement_fini(struct apple_dcp *dcp);
void dcp_retirement_ready_locked(struct apple_dcp *dcp);
void dcp_retirement_unplug_locked(struct apple_dcp *dcp);
void dcp_retirement_note_use(struct apple_dcp *dcp);
bool dcp_retirement_failed(struct apple_dcp *dcp);
bool dcp_retirement_is_retired(struct apple_dcp *dcp);
int dcp_retirement_restart_locked(struct apple_dcp *dcp);
int dcp_retirement_suspend(struct apple_dcp *dcp);
int dcp_retirement_resume(struct apple_dcp *dcp);
int dcp_retirement_runtime_suspend(struct device *dev);
int dcp_retirement_runtime_resume(struct device *dev);
int dcp_retirement_mapping_context(struct device *dev);
int dcp_retirement_rpc_get(struct apple_dcp *dcp);
void dcp_retirement_rpc_put(struct apple_dcp *dcp);

/* Existing pool lock also serializes lifecycle transitions; never used in IRQ. */
void dcp_typec_pool_lock(void);
void dcp_typec_pool_unlock(void);
bool dcp_typec_pool_sleeping(void);
#endif
