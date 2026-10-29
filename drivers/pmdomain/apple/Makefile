# SPDX-License-Identifier: GPL-2.0-only
obj-$(CONFIG_APPLE_PMGR_PWRSTATE)	+= pmgr-pwrstate.o
obj-$(CONFIG_APPLE_PMP_REPORT)	+= apple-pmp-report.o
apple-pmp-report-y := pmp-report.o
apple-pmp-report-$(CONFIG_CPU_FREQ) += pmp-cpu-vote.o
