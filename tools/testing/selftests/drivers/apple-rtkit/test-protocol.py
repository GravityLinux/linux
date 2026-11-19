#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Host protocol tests using the production RTKit handlers and type definitions.

Only kernel services (mailbox, allocation, completions and logging) are stubbed.
Checks default/quirked message bytes, map ordering, boot gating and setup failure.
No device access or installed kernel headers are required; CC defaults to cc.
"""
from pathlib import Path
import os
import re
import shlex
import subprocess
import tempfile

root = next(p for p in Path(__file__).resolve().parents
            if (p / 'drivers/soc/apple/rtkit.c').exists())
source = (root / 'drivers/soc/apple/rtkit.c').read_text()
public = (root / 'include/linux/soc/apple/rtkit.h').read_text()
private = (root / 'drivers/soc/apple/rtkit-internal.h').read_text()


def item(text, pattern, semicolon=False):
    match = re.search(pattern, text, re.M)
    if match is None:
        raise RuntimeError(f'missing production item: {pattern}')
    start = match.start()
    brace = text.index('{', match.end())
    depth, end = 1, brace + 1
    while depth:
        depth += (text[end] == '{') - (text[end] == '}')
        end += 1
    return text[start:end + int(semicolon)] + '\n'


preamble = r'''
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
typedef uint8_t u8;
typedef uint32_t u32;
typedef uint64_t u64;
typedef uint64_t dma_addr_t;
#define __iomem
#define __free(x)
#define GFP_KERNEL 0
#define BIT_ULL(n) (UINT64_C(1) << (n))
#define GENMASK_ULL(h, l) ((UINT64_MAX >> (63 - (h))) & (UINT64_MAX << (l)))
#define FIELD_GET(mask, val) (((val) & (mask)) >> __builtin_ctzll(mask))
#define FIELD_PREP(mask, val) (((u64)(val) << __builtin_ctzll(mask)) & (mask))
#define DECLARE_BITMAP(name, bits) unsigned long name[(bits) / 64]
#define for_each_set_bit(i, bits, n) \
    for ((i) = 0; (i) < (n); (i)++) if ((bits)[(i) / 64] & BIT_ULL((i) % 64))
#define dev_dbg(...) ((void)0)
#define dev_warn(...) ((void)0)
#define dev_err(...) ((void)0)
#define APPLE_RTKIT_APP_ENDPOINT_START 0x20
#define APPLE_RTKIT_MAX_ENDPOINTS 0x100
struct completion { bool done; int id; };
static void complete_all(struct completion *c) { c->done = true; }
static void set_bit(int bit, unsigned long *bits) { bits[bit / 64] |= BIT_ULL(bit % 64); }
struct apple_rtkit;
'''
constants = source[source.index('enum {'):source.index('struct apple_rtkit_rx_work')]
types = ''.join([
    item(public, r'^enum apple_rtkit_quirks', True),
    item(public, r'^struct apple_rtkit_shmem', True),
    item(public, r'^struct apple_rtkit_ops', True),
    item(private, r'^struct apple_rtkit(?= \{)', True),
])
stubs = r'''
enum { START, ACK, SEND, WAIT, POWER };
struct event { int kind, ep; u64 msg; };
static struct event events[64];
static size_t nr;
static void event(int kind, int ep, u64 msg) {
    assert(nr < sizeof(events) / sizeof(*events));
    events[nr++] = (struct event){ kind, ep, msg };
}
static int apple_rtkit_start_ep(struct apple_rtkit *r, u8 ep) {
    event(START, ep, 0); return 0;
}
static int apple_rtkit_management_send(struct apple_rtkit *r, u8 type, u64 msg) {
    event(ACK, type, msg); return 0;
}
static int apple_rtkit_send_message(struct apple_rtkit *r, u8 ep, u64 msg,
                                    void *completion, bool atomic) {
    event(SEND, ep, msg); return 0;
}
static int apple_rtkit_wait_for_completion(struct completion *c) {
    event(WAIT, c->id, 0); return c->done ? 0 : -ETIMEDOUT;
}
static int apple_rtkit_set_ap_power_state(struct apple_rtkit *r, unsigned int state) {
    event(POWER, 0, state); return 0;
}
static u8 storage[65536];
static void *dma_alloc_coherent(struct device *dev, size_t size,
                                dma_addr_t *addr, int flags) {
    assert(size <= sizeof(storage)); *addr = 0x12345000; return storage;
}
static void *kzalloc(size_t size, int flags) { return calloc(1, size); }
static void apple_rtkit_memcpy(struct apple_rtkit *r, void *dst,
                               struct apple_rtkit_shmem *src, size_t off, size_t size) {
    memcpy(dst, (char *)src->buffer + off, size);
}
static void apple_rtkit_crashlog_dump(struct apple_rtkit *r, u8 *buf, size_t size) {}
'''
functions = ''.join(item(source, rf'^(?:static )?(?:bool|void|int) {name}\(')
                    for name in ['apple_rtkit_has_quirk', 'apple_rtkit_is_running',
                                 'apple_rtkit_management_rx_epmap',
                                 'apple_rtkit_common_rx_get_buffer',
                                 'apple_rtkit_crashlog_rx', 'apple_rtkit_boot'])
tests = r'''
static void expect(size_t i, int kind, int ep, u64 msg) {
    assert(i < nr && events[i].kind == kind && events[i].ep == ep && events[i].msg == msg);
}
static int setup(void *cookie, struct apple_rtkit_shmem *buf) {
    if (cookie) return -ENOMEM;
    buf->buffer = storage;
    buf->is_mapped = buf->iova != 0;
    if (!buf->is_mapped) buf->iova = 0x12345000;
    return 0;
}
static void check_map(u32 quirks) {
    struct apple_rtkit_ops ops = { .quirks = quirks };
    struct apple_rtkit r = { .ops = &ops };
    bool early = quirks & APPLE_RTKIT_QUIRK_EARLY_CRASHLOG;
    nr = 0;
    /* Crashlog, endpoint 7 and oslog in the first chunk; application EP32 in the last. */
    apple_rtkit_management_rx_epmap(&r, BIT_ULL(1) | BIT_ULL(7) | BIT_ULL(8));
    assert(nr == (early ? 2 : 1));
    if (early) expect(0, START, 1, 0);
    expect(early ? 1 : 0, ACK, 8, 1);
    apple_rtkit_management_rx_epmap(&r, BIT_ULL(51) | BIT_ULL(32) | 1);
    expect(early ? 2 : 1, ACK, 8, BIT_ULL(51) | BIT_ULL(32));
    if (!early) expect(2, START, 1, 0);
    expect(3, START, 7, 0);
    expect(4, START, 8, 0);
    assert(nr == 5 && r.epmap_completion.done && r.boot_result == 0);
    assert(r.endpoints[0] & BIT_ULL(32));
}
static void check_buffers(u32 quirks) {
    struct apple_rtkit_ops ops = { .quirks = quirks, .shmem_setup = setup };
    struct apple_rtkit r = { .ops = &ops };
    struct apple_rtkit_shmem b = {0};
    bool early = quirks & APPLE_RTKIT_QUIRK_EARLY_CRASHLOG;
    u64 request = BIT_ULL(52) | (UINT64_C(2) << 44) | 0x12345000;
    nr = 0;
    apple_rtkit_crashlog_rx(&r, request);
    assert(r.crashlog_completion.done && r.crashlog_buffer.is_mapped);
    assert(r.crashlog_buffer.size == 8192 && r.crashlog_buffer.iova == 0x12345000);
    assert(nr == (early ? 1 : 0));
    if (early) expect(0, SEND, 1, request);
    /* A mapped non-crash buffer is never echoed, even with the crash quirk. */
    nr = 0;
    assert(apple_rtkit_common_rx_get_buffer(&r, &b, 4, request) == 0 && nr == 0);
    /* Allocated oslog: exact reply bytes, including the default size bits. */
    request = BIT_ULL(56) | (UINT64_C(0x6000) << 36);
    assert(apple_rtkit_common_rx_get_buffer(&r, &b, 8, request) == 0);
    assert(b.size == 0x6000 && !b.is_mapped);
    u64 reply = BIT_ULL(56) | 0x12345;
    if (!(quirks & APPLE_RTKIT_QUIRK_OSLOG_NO_SIZE)) reply |= UINT64_C(0x6000) << 36;
    expect(0, SEND, 8, reply);
    /* Endpoint 7 keeps its distinct reply type; neither quirk changes it. */
    nr = 0;
    assert(apple_rtkit_common_rx_get_buffer(&r, &b, 7, BIT_ULL(52) | (UINT64_C(2) << 44)) == 0);
    expect(0, SEND, 7, (UINT64_C(5) << 52) | (UINT64_C(2) << 44) | 0x12345000);
    /* Failed mapping does not ACK or complete a crash-buffer exchange. */
    nr = 0;
    r.cookie = &r;
    memset(&r.crashlog_buffer, 0, sizeof(r.crashlog_buffer));
    r.crashlog_completion.done = false;
    apple_rtkit_crashlog_rx(&r, BIT_ULL(52) | (UINT64_C(2) << 44) | 0x12345000);
    assert(!r.crashlog_completion.done && !r.crashlog_buffer.size && nr == 0);
    /* The default DMA allocator follows the same oslog wire format. */
    ops.shmem_setup = NULL;
    assert(apple_rtkit_common_rx_get_buffer(&r, &b, 8, request) == 0);
    expect(0, SEND, 8, reply);
}
static void check_boot(u32 quirks) {
    struct apple_rtkit_ops ops = { .quirks = quirks };
    struct apple_rtkit r = { .ops = &ops,
        .epmap_completion = { true, 1 }, .iop_pwr_ack_completion = { true, 2 },
        .crashlog_completion = { false, 3 } };
    bool early = quirks & APPLE_RTKIT_QUIRK_EARLY_CRASHLOG;
    nr = 0;
    int ret = apple_rtkit_boot(&r);
    expect(0, WAIT, 1, 0); expect(1, WAIT, 2, 0);
    if (early) {
        assert(ret == -ETIMEDOUT && nr == 3);
        expect(2, WAIT, 3, 0);
        nr = 0; r.crashlog_completion.done = true;
        assert(apple_rtkit_boot(&r) == 0 && nr == 4);
        expect(2, WAIT, 3, 0); expect(3, POWER, 0, 0x20);
    } else {
        assert(ret == 0 && nr == 3); expect(2, POWER, 0, 0x20);
    }
    /* An earlier boot error must not request AP power. */
    nr = 0; r.boot_result = -EPROTO;
    assert(apple_rtkit_boot(&r) == -EPROTO && nr == 1);
}
int main(void) {
    const u32 flags[] = {0, APPLE_RTKIT_QUIRK_EARLY_CRASHLOG,
        APPLE_RTKIT_QUIRK_OSLOG_NO_SIZE,
        APPLE_RTKIT_QUIRK_EARLY_CRASHLOG | APPLE_RTKIT_QUIRK_OSLOG_NO_SIZE};
    for (size_t i = 0; i < sizeof(flags)/sizeof(*flags); i++) {
        check_map(flags[i]); check_buffers(flags[i]); check_boot(flags[i]);
    }
    puts("PASS: default and opt-in RTKit ordering, replies, boot gating and failure paths");
}
'''
with tempfile.TemporaryDirectory(prefix='apple-rtkit-protocol-') as temp:
    path = Path(temp)
    harness = path / 'protocol.c'
    harness.write_text(preamble + constants + types + stubs + functions + tests)
    subprocess.run(shlex.split(os.environ.get('CC', 'cc')) + [
        '-std=gnu11', '-Wall', '-Wextra', '-Werror', '-Wno-unused-parameter',
        str(harness), '-o', str(path / 'protocol')], check=True)
    subprocess.run([str(path / 'protocol')], check=True)
