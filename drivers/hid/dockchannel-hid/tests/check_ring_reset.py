#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Compile the real reset/start functions against captured wire expectations.

J616c macOS 26.6.2: 40 02 01 00 00 ..., assert reset, hold, 40 02 01
00 01 ...; then 40 02 01 02 00 ..., release reset, 40 02 01 02 01 ... .
No firmware payload or Apple instructions are part of this test.
"""
from pathlib import Path
import os
import subprocess
import tempfile

source = (Path(__file__).resolve().parents[1] / "dockchannel-hid.c").read_text()


def function(name):
    begin = source.index("static int " + name + "(")
    first = source.index("{", begin)
    depth = 1
    end = first + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[begin:end]


shim = r'''
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <errno.h>
typedef uint8_t u8;
#define dev_info(...) ((void)0)
#define dev_err(...) ((void)0)
#define dev_warn(...) ((void)0)
#define CMD_RESET_INTERFACE 0x40
struct gpio_desc { int unused; } pin;
struct dockchannel_hid { bool use_ring; void *dev; } transport;
struct dchid_iface {
 struct dockchannel_hid *dchid;
 int index, gpio_id;
 bool starting;
 const char *name;
 struct gpio_desc *reset_gpio;
} iface;
static char order[64];
static int used, ncmd, fail_cmd, gpio_error, nlevel, levels[8], holds;
static u8 packets[8][16];
static size_t lengths[8];
static void event(char kind) { order[used++] = kind; }
static int dchid_comm_cmd(struct dockchannel_hid *d, void *data, size_t len) {
 (void)d; event('P'); assert(ncmd < 8 && len <= 16);
 lengths[ncmd] = len; memcpy(packets[ncmd++], data, len);
 return ncmd == fail_cmd ? -EIO : 1; /* A positive reply length is success. */
}
static int gpiod_set_value_cansleep(struct gpio_desc *g, int value) {
 assert(g == &pin); event('G'); levels[nlevel++] = value; return gpio_error;
}
static void msleep(unsigned ms) { assert(ms == 50); event('D'); holds++; }
static int dchid_register_ring(struct dockchannel_hid *d) {
 if (d->use_ring)
  event('R');
 return 0;
}
static int dchid_get_firmware(struct dchid_iface *i, void **fw, size_t *size) {
 static u8 blob[4]; (void)i; *fw = blob; *size = sizeof(blob); return 0;
}
static int dchid_request_gpio(struct dchid_iface *i) { (void)i; return 0; }
static int dchid_send_firmware(struct dchid_iface *i, void *fw, size_t size) {
 (void)i; assert(fw && size == 4); event('F'); return 1;
}
'''

checks = r'''
static void reset(bool ring, bool gpio) {
 memset(order, 0, sizeof(order)); memset(packets, 0, sizeof(packets));
 used = ncmd = fail_cmd = gpio_error = nlevel = holds = 0;
 transport.use_ring = ring;
 iface = (struct dchid_iface) { .dchid = &transport, .index = 1,
  .name = "multi-touch", .reset_gpio = gpio ? &pin : NULL };
}
static const u8 captured[4][9] = {
 {0x40,2,1,0,0,0,0,0,0}, {0x40,2,1,0,1,0,0,0,0},
 {0x40,2,1,2,0,0,0,0,0}, {0x40,2,1,2,1,0,0,0,0}
};
int main(void) {
 reset(true, true);
 assert(dchid_start_interface(&iface) == 0 && iface.starting);
 assert(!strcmp(order, "RFPGDPPGP") && ncmd == 4);
 assert(nlevel == 2 && levels[0] == 1 && levels[1] == 0 && holds == 1);
 for (int i = 0; i < 4; i++)
  assert(lengths[i] == 9 && !memcmp(packets[i], captured[i], 9));
 reset(true, false);
 assert(dchid_start_interface(&iface) == 0 && !strcmp(order, "RFPPPP"));
 assert(nlevel == 0 && holds == 0);
 reset(false, false);
 assert(dchid_start_interface(&iface) == 0 && !strcmp(order, "FPP"));
 assert(lengths[0] == 4 && !memcmp(packets[0], (u8[]){0x40,1,1,0}, 4));
 assert(lengths[1] == 4 && !memcmp(packets[1], (u8[]){0x40,1,1,2}, 4));
 reset(true, true); fail_cmd = 1;
 assert(dchid_start_interface(&iface) == -EIO && !iface.starting);
 assert(!strcmp(order, "RFP"));
 reset(true, true); fail_cmd = 2;
 assert(dchid_start_interface(&iface) == -EIO && !iface.starting);
 assert(!strcmp(order, "RFPGDP"));
 reset(true, true); gpio_error = -EIO;
 assert(dchid_start_interface(&iface) == -EIO && !iface.starting);
 assert(!strcmp(order, "RFPG"));
 reset(true, true);
 assert(dchid_reset_interface(&iface, 1) == -EINVAL && used == 0);
 puts("PASS: captured reset bytes/order, positive replies, legacy/no-GPIO paths, error stops");
}
'''

with tempfile.TemporaryDirectory(prefix="dchid-reset-") as directory:
    root = Path(directory)
    unit = root / "reset.c"
    unit.write_text(shim + function("dchid_reset_interface") +
                    function("dchid_start_interface") + checks)
    binary = root / "reset"
    subprocess.run([os.environ.get("CC", "cc"), "-std=gnu11", "-Wall", "-Wextra",
                    "-Werror", str(unit), "-o", str(binary)], check=True)
    subprocess.run([str(binary)], check=True)
