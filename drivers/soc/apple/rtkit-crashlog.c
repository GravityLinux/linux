// SPDX-License-Identifier: GPL-2.0-only OR MIT
/*
 * Apple RTKit IPC library
 * Copyright (C) The Asahi Linux Contributors
 */
#include "rtkit-internal.h"

#define FOURCC(a, b, c, d) \
	(((u32)(a) << 24) | ((u32)(b) << 16) | ((u32)(c) << 8) | ((u32)(d)))

#define APPLE_RTKIT_CRASHLOG_HEADER FOURCC('C', 'L', 'H', 'E')
#define APPLE_RTKIT_CRASHLOG_STR FOURCC('C', 's', 't', 'r')
#define APPLE_RTKIT_CRASHLOG_VERSION FOURCC('C', 'v', 'e', 'r')
#define APPLE_RTKIT_CRASHLOG_MBOX FOURCC('C', 'm', 'b', 'x')
#define APPLE_RTKIT_CRASHLOG_TIME FOURCC('C', 't', 'i', 'm')
#define APPLE_RTKIT_CRASHLOG_REGS FOURCC('C', 'r', 'g', '8')
#define APPLE_RTKIT_CRASHLOG_STACK FOURCC('C', 'c', 's', 't')
#define APPLE_RTKIT_CRASHLOG_ASYNC_ERROR FOURCC('C', 'a', 's', 'C')
#define APPLE_RTKIT_CRASHLOG_DCP_MAP FOURCC('C', 'c', 'd', 'p')

/* For COMPILE_TEST on non-ARM64 architectures */
#ifndef PSR_MODE_EL0t
#define PSR_MODE_EL0t	0x00000000
#define PSR_MODE_EL1t	0x00000004
#define PSR_MODE_EL1h	0x00000005
#define PSR_MODE_EL2t	0x00000008
#define PSR_MODE_EL2h	0x00000009
#define PSR_MODE_MASK	0x0000000f
#endif

struct apple_rtkit_crashlog_header {
	u32 fourcc;
	u32 version;
	u32 size;
	u32 flags;
	u8 _unk[16];
};
static_assert(sizeof(struct apple_rtkit_crashlog_header) == 0x20);

struct apple_rtkit_crashlog_mbox_entry {
	u64 msg0;
	u64 msg1;
	u32 timestamp;
	u8 _unk[4];
};
static_assert(sizeof(struct apple_rtkit_crashlog_mbox_entry) == 0x18);

struct apple_rtkit_crashlog_regs {
	u32 unk_0;
	u32 unk_4;
	u64 regs[31];
	u64 sp;
	u64 pc;
	u64 psr;
	u64 cpacr;
	u64 fpsr;
	u64 fpcr;
	u64 unk[64];
	u64 far;
	u64 unk_X;
	u64 esr;
	u64 unk_Z;
} __packed;
static_assert(sizeof(struct apple_rtkit_crashlog_regs) == 0x350);

struct apple_rtkit_crashlog_dcp_map {
	u64 va;
	u64 dva;
	u32 flags;
} __packed;
static_assert(sizeof(struct apple_rtkit_crashlog_dcp_map) == 0x14);

struct apple_rtkit_crashlog_stack {
	u32 task;
	u32 unknown;
} __packed;
static_assert(sizeof(struct apple_rtkit_crashlog_stack) == 0x8);

struct apple_rtkit_crashlog_async_error {
	u64 l2c_err_sts;
	u64 l2c_err_adr;
	u64 l2c_err_inf;
	u64 lsu_err_sts;
	u64 fed_err_sts;
	u64 mmu_err_sts;
} __packed;
static_assert(sizeof(struct apple_rtkit_crashlog_async_error) == 0x30);

static void apple_rtkit_crashlog_dump_str(struct apple_rtkit *rtk, u8 *bfr,
					  size_t size)
{
	u32 idx;
	u8 *ptr, *end;

	memcpy(&idx, bfr, 4);

	ptr = bfr + 4;
	end = bfr + size;
	while (ptr < end) {
		u8 *newline = memchr(ptr, '\n', end - ptr);

		if (newline) {
			u8 tmp = *newline;
			*newline = '\0';
			dev_warn(rtk->dev, "RTKit: Message (id=%x): %s\n", idx,
				 ptr);
			*newline = tmp;
			ptr = newline + 1;
		} else {
			dev_warn(rtk->dev, "RTKit: Message (id=%x): %s", idx,
				 ptr);
			break;
		}
	}
}

static void apple_rtkit_crashlog_dump_version(struct apple_rtkit *rtk, u8 *bfr,
					      size_t size)
{
	dev_warn(rtk->dev, "RTKit: Version: %s", bfr + 16);
}

static void apple_rtkit_crashlog_dump_time(struct apple_rtkit *rtk, u8 *bfr,
					   size_t size)
{
	u64 crash_time;

	memcpy(&crash_time, bfr, 8);
	dev_warn(rtk->dev, "RTKit: Crash time: %lld", crash_time);
}

static void apple_rtkit_crashlog_dump_mailbox(struct apple_rtkit *rtk, u8 *bfr,
					      size_t size)
{
	u32 type, index, i;
	size_t n_messages;
	struct apple_rtkit_crashlog_mbox_entry entry;

	memcpy(&type, bfr + 16, 4);
	memcpy(&index, bfr + 24, 4);
	n_messages = (size - 28) / sizeof(entry);

	dev_warn(rtk->dev, "RTKit: Mailbox history (type = %d, index = %d)",
		 type, index);
	for (i = 0; i < n_messages; ++i) {
		memcpy(&entry, bfr + 28 + i * sizeof(entry), sizeof(entry));
		dev_warn(rtk->dev, "RTKit:  #%03d@%08x: %016llx %016llx", i,
			 entry.timestamp, entry.msg0, entry.msg1);
	}
}

static void apple_rtkit_crashlog_dump_regs(struct apple_rtkit *rtk, u8 *bfr,
					   size_t size)
{
	struct apple_rtkit_crashlog_regs *regs;
	const char *el;
	int i;

	if (size < sizeof(*regs)) {
		dev_warn(rtk->dev, "RTKit: Regs section too small: 0x%zx", size);
		return;
	}

	regs = (struct apple_rtkit_crashlog_regs *)bfr;

	switch (regs->psr & PSR_MODE_MASK) {
	case PSR_MODE_EL0t:
		el = "EL0t";
		break;
	case PSR_MODE_EL1t:
		el = "EL1t";
		break;
	case PSR_MODE_EL1h:
		el = "EL1h";
		break;
	case PSR_MODE_EL2t:
		el = "EL2t";
		break;
	case PSR_MODE_EL2h:
		el = "EL2h";
		break;
	default:
		el = "unknown";
		break;
	}

	dev_warn(rtk->dev, "RTKit: Exception dump:");
	dev_warn(rtk->dev, "  == Exception taken from %s ==", el);
	dev_warn(rtk->dev, "  PSR    = 0x%llx", regs->psr);
	dev_warn(rtk->dev, "  PC     = 0x%llx\n", regs->pc);
	dev_warn(rtk->dev, "  ESR    = 0x%llx\n", regs->esr);
	dev_warn(rtk->dev, "  FAR    = 0x%llx\n", regs->far);
	dev_warn(rtk->dev, "  SP     = 0x%llx\n", regs->sp);
	dev_warn(rtk->dev, "\n");

	for (i = 0; i < 31; i += 4) {
		if (i < 28)
			dev_warn(rtk->dev,
					 "  x%02d-x%02d = %016llx %016llx %016llx %016llx\n",
					 i, i + 3,
					 regs->regs[i], regs->regs[i + 1],
					 regs->regs[i + 2], regs->regs[i + 3]);
		else
			dev_warn(rtk->dev,
					 "  x%02d-x%02d = %016llx %016llx %016llx\n", i, i + 3,
					 regs->regs[i], regs->regs[i + 1], regs->regs[i + 2]);
	}

	dev_warn(rtk->dev, "\n");
}

static void apple_rtkit_crashlog_dump_dcp_map(struct apple_rtkit *rtk,
					       u8 *bfr, size_t size)
{
	struct apple_rtkit_crashlog_dcp_map entry;
	size_t offset;

	for (offset = 0; offset + sizeof(entry) <= size;
	     offset += sizeof(entry)) {
		memcpy(&entry, bfr + offset, sizeof(entry));
		dev_warn(rtk->dev,
			 "RTKit: DCP map: VA 0x%016llx -> DVA 0x%016llx flags 0x%x",
			 entry.va, entry.dva, entry.flags);
	}
}

static void apple_rtkit_crashlog_dump_stack(struct apple_rtkit *rtk, u8 *bfr,
					     size_t size)
{
	struct apple_rtkit_crashlog_stack stack;
	size_t offset;
	u64 address;

	if (size < sizeof(stack)) {
		dev_warn(rtk->dev, "RTKit: Stack section too small: 0x%zx",
			 size);
		return;
	}

	memcpy(&stack, bfr, sizeof(stack));
	dev_warn(rtk->dev,
		 "RTKit: Call stack (task %u, unknown 0x%x):",
		 stack.task, stack.unknown);

	for (offset = sizeof(stack); offset + sizeof(address) <= size;
	     offset += sizeof(address)) {
		memcpy(&address, bfr + offset, sizeof(address));
		if (!address)
			break;
		dev_warn(rtk->dev, "RTKit:   0x%016llx", address);
	}
}

static void apple_rtkit_crashlog_dump_async_error(struct apple_rtkit *rtk,
						   u8 *bfr, size_t size)
{
	struct apple_rtkit_crashlog_async_error error;

	if (size < sizeof(error)) {
		dev_warn(rtk->dev,
			 "RTKit: Async error section too small: 0x%zx", size);
		return;
	}

	memcpy(&error, bfr, sizeof(error));
	dev_warn(rtk->dev, "RTKit: Async error info:");
	dev_warn(rtk->dev, "RTKit:   L2C_ERR_STS = 0x%016llx",
		 error.l2c_err_sts);
	dev_warn(rtk->dev, "RTKit:   L2C_ERR_ADR = 0x%016llx",
		 error.l2c_err_adr);
	dev_warn(rtk->dev, "RTKit:   L2C_ERR_INF = 0x%016llx",
		 error.l2c_err_inf);
	dev_warn(rtk->dev, "RTKit:   LSU_ERR_STS = 0x%016llx",
		 error.lsu_err_sts);
	dev_warn(rtk->dev, "RTKit:   FED_ERR_STS = 0x%016llx",
		 error.fed_err_sts);
	dev_warn(rtk->dev, "RTKit:   MMU_ERR_STS = 0x%016llx",
		 error.mmu_err_sts);
}

void apple_rtkit_crashlog_dump(struct apple_rtkit *rtk, u8 *bfr, size_t size)
{
	size_t offset, payload_size;
	u32 section_fourcc, section_size;
	struct apple_rtkit_crashlog_header header;

	memcpy(&header, bfr, sizeof(header));
	if (header.fourcc != APPLE_RTKIT_CRASHLOG_HEADER) {
		dev_warn(rtk->dev, "RTKit: Expected crashlog header but got %x",
			 header.fourcc);
		return;
	}

	if (header.size > size) {
		dev_warn(rtk->dev, "RTKit: Crashlog size (%x) is too large",
			 header.size);
		return;
	}

	size = header.size;
	offset = sizeof(header);

	while (offset < size) {
		memcpy(&section_fourcc, bfr + offset, 4);
		memcpy(&section_size, bfr + offset + 12, 4);
		if (section_size < 16 || section_size > size - offset) {
			dev_warn(rtk->dev,
				 "RTKit: Invalid crashlog section size 0x%x at 0x%zx",
				 section_size, offset);
			return;
		}
		payload_size = section_size - 16;

		switch (section_fourcc) {
		case APPLE_RTKIT_CRASHLOG_HEADER:
			dev_dbg(rtk->dev, "RTKit: End of crashlog reached");
			return;
		case APPLE_RTKIT_CRASHLOG_STR:
			apple_rtkit_crashlog_dump_str(rtk, bfr + offset + 16,
						      payload_size);
			break;
		case APPLE_RTKIT_CRASHLOG_VERSION:
			apple_rtkit_crashlog_dump_version(
				rtk, bfr + offset + 16, payload_size);
			break;
		case APPLE_RTKIT_CRASHLOG_MBOX:
			apple_rtkit_crashlog_dump_mailbox(
				rtk, bfr + offset + 16, payload_size);
			break;
		case APPLE_RTKIT_CRASHLOG_TIME:
			apple_rtkit_crashlog_dump_time(rtk, bfr + offset + 16,
						       payload_size);
			break;
		case APPLE_RTKIT_CRASHLOG_REGS:
			apple_rtkit_crashlog_dump_regs(rtk, bfr + offset + 16,
						       payload_size);
			break;
		case APPLE_RTKIT_CRASHLOG_STACK:
			apple_rtkit_crashlog_dump_stack(rtk, bfr + offset + 16,
							payload_size);
			break;
		case APPLE_RTKIT_CRASHLOG_ASYNC_ERROR:
			apple_rtkit_crashlog_dump_async_error(
				rtk, bfr + offset + 16, payload_size);
			break;
		case APPLE_RTKIT_CRASHLOG_DCP_MAP:
			apple_rtkit_crashlog_dump_dcp_map(
				rtk, bfr + offset + 16, payload_size);
			break;
		default:
			dev_warn(rtk->dev,
				 "RTKit: Unknown crashlog section: %x",
				 section_fourcc);
		}

		offset += section_size;
	}

	dev_warn(rtk->dev,
		 "RTKit: End of crashlog reached but no footer present");
}
