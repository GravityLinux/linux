/* SPDX-License-Identifier: GPL-2.0-only OR MIT */
#ifndef MACSMC_HWMON_THERMAL_H
#define MACSMC_HWMON_THERMAL_H

#include <linux/types.h>

/* Protected by the thermal zone lock, including reads through thermal sysfs. */
struct macsmc_thermal_state {
	unsigned int failures;
	unsigned int recovery;
	int last;
	bool fault;
};

static inline bool macsmc_thermal_valid(long temperature)
{
	return temperature >= -40000 && temperature <= 150000;
}

/*
 * Failed reads must not remove an existing cooling request. After three
 * failures, report a conservative control temperature at the highest passive
 * trip until three complete samples succeed. Individual hwmon readings are
 * never replaced by this control value.
 */
static inline int macsmc_thermal_update(struct macsmc_thermal_state *state,
					int reading, int error, int failsafe,
					int *temperature)
{
	if (error) {
		state->recovery = 0;
		if (state->failures < 3)
			state->failures++;
		if (state->failures == 3)
			state->fault = true;
		if (!state->fault)
			return error;
	} else {
		state->failures = 0;
		state->last = reading;
		if (state->fault && ++state->recovery == 3) {
			state->fault = false;
			state->recovery = 0;
		}
	}

	*temperature = state->last;
	if (state->fault && *temperature < failsafe)
		*temperature = failsafe;
	return 0;
}

#endif
