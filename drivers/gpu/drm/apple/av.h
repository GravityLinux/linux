#ifndef __AV_H__
#define __AV_H__

#include "parser.h"

//int avep_audiosrv_startlink(struct apple_dcp *dcp, struct dcp_sound_cookie *cookie);
//int avep_audiosrv_stoplink(struct apple_dcp *dcp);

#if IS_ENABLED(CONFIG_DRM_APPLE_AUDIO)
int dcp_av_retire(struct apple_dcp *dcp, bool *was_open);
int dcp_av_restart(struct apple_dcp *dcp, bool was_open);
void av_service_connect(struct apple_dcp *dcp);
void av_service_disconnect(struct apple_dcp *dcp);
void av_service_unplug(struct apple_dcp *dcp);
int dcp_audiosrv_close_for_power_transition(struct device *dev);
int dcp_audiosrv_open_after_power_transition(struct device *dev);
#else
static inline int dcp_av_retire(struct apple_dcp *dcp, bool *was_open) { return 0; }
static inline int dcp_av_restart(struct apple_dcp *dcp, bool was_open) { return 0; }
static inline void av_service_connect(struct apple_dcp *dcp) { }
static inline void av_service_disconnect(struct apple_dcp *dcp) { }
static inline void av_service_unplug(struct apple_dcp *dcp) { }
static inline int dcp_audiosrv_close_for_power_transition(struct device *dev)
{
	return 0;
}
static inline int dcp_audiosrv_open_after_power_transition(struct device *dev)
{
	return 0;
}
#endif

#endif /* __AV_H__ */
