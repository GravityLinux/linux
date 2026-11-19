// SPDX-License-Identifier: GPL-2.0-only OR MIT
#include <assert.h>
#include <errno.h>
#include <limits.h>
#include <stdbool.h>
#include <stdio.h>

#include "../../../../drivers/hwmon/macsmc-hwmon-thermal.h"

int main(void)
{
	struct macsmc_thermal_state s = {0};
	int temp = 123;

	assert(macsmc_thermal_valid(112156));
	assert(!macsmc_thermal_valid(LONG_MAX));
	assert(!macsmc_thermal_valid(LONG_MIN));
	assert(!macsmc_thermal_valid(150001));
	assert(!macsmc_thermal_valid(-40001));
	assert(macsmc_thermal_update(&s, 93000, 0, 95000, &temp) == 0 && temp == 93000);
	for (int i = 0; i < 2; i++) {
		assert(macsmc_thermal_update(&s, INT_MIN, -EIO, 95000, &temp) == -EIO);
		assert(temp == 93000 && !s.fault);
	}
	assert(macsmc_thermal_update(&s, INT_MIN, -EIO, 95000, &temp) == 0);
	assert(s.fault && temp == 95000);
	assert(macsmc_thermal_update(&s, 80000, 0, 95000, &temp) == 0 && temp == 95000);
	assert(macsmc_thermal_update(&s, 80000, 0, 95000, &temp) == 0 && temp == 95000);
	/* An intervening failed key resets the complete-sample recovery count. */
	assert(macsmc_thermal_update(&s, INT_MIN, -EIO, 95000, &temp) == 0 && temp == 95000);
	for (int i = 0; i < 2; i++)
		assert(macsmc_thermal_update(&s, 80000, 0, 95000, &temp) == 0 && temp == 95000);
	assert(macsmc_thermal_update(&s, 80000, 0, 95000, &temp) == 0);
	assert(!s.fault && temp == 80000);
	/* Sensor failure must not lower a hotter last-known control temperature. */
	assert(macsmc_thermal_update(&s, 105000, 0, 95000, &temp) == 0);
	for (int i = 0; i < 3; i++)
		macsmc_thermal_update(&s, INT_MIN, -ERANGE, 95000, &temp);
	assert(s.fault && temp == 105000);
	puts("PASS: thermal range, retain-on-error, bounded fault clamp and recovery");
	return 0;
}
