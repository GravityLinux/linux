/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __MT7932_REGS_H
#define __MT7932_REGS_H

/* All offsets below are PCI BAR0 offsets, not SoC physical addresses. */
#define MT7932_BAR_SIZE			SZ_1M
#define MT7932_CHIP_ID			0x7932
#define MT7932_HW_REV			0xb1000
#define MT7932_HW_REV_MASK		GENMASK(7, 0)
#define MT7932_REMAP_WINDOW_SIZE		SZ_64K
#define MT7932_WF_REMAP_PHYS		0x70000000
#define MT7932_WF_REMAP_BAR		0x70000
#define MT7932_CONN_REMAP_PHYS		0x18050000
#define MT7932_CONN_REMAP_ALIAS		0x7c050000
#define MT7932_CONN_REMAP_BAR		0x90000

#define MT7932_PCIE_REMAP(_n)		(0xfe24c + 4 * (_n))
#define MT7932_PCIE_REMAP_TARGET		GENMASK(31, 16)
/* Low halves are observed remap controls, not decoded address bits. */
#define MT7932_PCIE_REMAP_WF_CONTROL	0x1846
#define MT7932_PCIE_REMAP_CONN_CONTROL	0x1848
#define MT7932_PCIE_REMAP_BOOT		0x18451807
#define MT7932_PCIE_REMAP_RUNTIME		0x1845184f
#define MT7932_PCIE_REMAP_SETTLE_US	2

#define MT7932_SECURE_READY		0x40060
#define MT7932_SECURE_READY_BIT		BIT(0)
#define MT7932_SECURE_COMPLETE		0x40260
#define MT7932_SECURE_COMPLETE_BIT	BIT(0)

/* Observed shared-buffer publication sequence. Detailed buffer ownership,
 * enable-bit meanings and descriptor formats remain unresolved. Keep the
 * native order; do not describe these as ordinary WFDMA rings.
 */
#define MT7932_HOST_DMA_CONTROL0		0x93c28
#define MT7932_HOST_DMA_CONTROL0_INIT	BIT(0)
#define MT7932_HOST_DMA_CONTROL1		0x93a38
#define MT7932_HOST_DMA_CONTROL1_INIT	BIT(1)
#define MT7932_HOST_DMA_ADDR0		0x93a3c
#define MT7932_HOST_DMA_ADDR1		0x93a30
#define MT7932_HOST_DMA_ADDR2		0x93a54

/* PCI configuration space, unlike the BAR0 offsets above. */
#define MT7932_PCIE_FW_STATE		0x48c
#define MT7932_PCIE_FW_STATE_MASK		GENMASK(19, 16)
#define MT7932_PCIE_FW_STATE_READY	2

#endif
