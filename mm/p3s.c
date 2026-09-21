// SPDX-License-Identifier: GPL-2.0
#include <linux/init.h>
#include <linux/kstrtox.h>
#include <linux/mm_types.h>
#include <linux/export.h>
#include <linux/mm.h>
#include <linux/p3s/mm.h>

enum p3s_4k_mode p3s_4k_mode __ro_after_init = P3S_4K_MODE_OFF;
EXPORT_SYMBOL(p3s_4k_mode);

static int __init parse_p3s_4k(char *str)
{
	bool enabled;

	if (!str)
		return -EINVAL;

	if (!kstrtobool(str, &enabled)) {
		p3s_4k_mode = enabled ? P3S_4K_MODE_ON : P3S_4K_MODE_OFF;
		return 0;
	}

	if (!strcmp(str, "alternate")) {
		p3s_4k_mode = P3S_4K_MODE_ALTERNATE;
		return 0;
	}

	return -EINVAL;
}
early_param("p3s_4k", parse_p3s_4k);
