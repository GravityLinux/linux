/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef NEO_SECURE_H
#define NEO_SECURE_H
#include <linux/device.h>
struct neo_secure;
struct neo_secure *neo_secure_get(struct device *dev);
int neo_secure_wait(struct neo_secure *secure);
#endif
