// SPDX-License-Identifier: GPL-2.0-only
/* Linux arm64 userspace prototype: ordered MMIO and uncached DMA mappings. */
#include <stdint.h>

void centauri_fence(void)
{
	__asm__ volatile("dsb sy" ::: "memory");
}

uint32_t centauri_read32(const volatile uint32_t *address)
{
	centauri_fence();
	uint32_t value = *address;
	centauri_fence();
	return value;
}

void centauri_write32(volatile uint32_t *address, uint32_t value)
{
	centauri_fence();
	*address = value;
	centauri_fence();
}
