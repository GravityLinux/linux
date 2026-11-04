#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Linux-local Python TAP prototype for a retained Centauri Wi-Fi connection.

Only use with fresh, private handoff metadata from the same device boot.
The loader reserves all live DMA allocations/page tables. This process uses
/dev/mem O_SYNC (normal uncached mappings for reserved RAM on arm64) and
explicit ordering barriers. It never takes over PCIe or the DART domain.
Firmware and association must already be live; this is not cold startup.
"""
import argparse
import ctypes
import fcntl
import json
import mmap
import os
from pathlib import Path
import select
import resource
import struct
import time
from types import SimpleNamespace

from centauri_ethernet import EthernetQueues
import centauri_regs as R
from centauri_regs import (WIFI_DEVICE_COMPLETION_INDICES,
                           WIFI_DEVICE_SUBMISSION_INDICES,
                           WIFI_HOST_COMPLETION_INDICES,
                           WIFI_HOST_SUBMISSION_INDICES)


# Gravity/Asahi DART8110 register layout; error status only, no fault pointer.
DART_ERROR_STATUS = 0x100


class RetainedTranslations:
    """Translate only SID 2 metadata; never walk or read mapped page contents."""

    def __init__(self, mappings):
        self.mappings = sorted(mappings, key=lambda item: item['iova'])
        end = 0
        for item in self.mappings:
            if item['iova'] < end or item['length'] <= 0:
                raise ValueError('Overlapping or empty retained translations')
            end = item['iova'] + item['length']

    def iotranslate(self, sid, iova, length):
        if sid != 2 or length <= 0:
            raise ValueError('Only Wi-Fi SID 2 is supported')
        regions = []
        end = iova + length
        for item in self.mappings:
            start, stop = item['iova'], item['iova'] + item['length']
            if start <= iova < stop:
                size = min(stop, end) - iova
                regions.append((item['physical'] + iova - start, size))
                iova += size
                if iova == end:
                    return regions
            elif start > iova:
                break
        raise ValueError('Unmapped Wi-Fi DMA extent')


class LinuxMemory:
    """On-demand uncached physical mappings; all extents must be retained RAM.

    Do not reserve these ranges with no-map: arm64 then chooses Device memory
    instead of Normal-NC, which would prohibit unaligned Ethernet copies.
    The FDT reservation map keeps Linux from allocating or touching them.
    """

    def __init__(self, state, library):
        self.state = state
        soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
        resource.setrlimit(resource.RLIMIT_NOFILE, (max(soft, min(hard, 16384)), hard))
        self.fd = os.open('/dev/mem', os.O_RDWR | os.O_SYNC | os.O_CLOEXEC)
        self.maps = {}
        self.lib = ctypes.CDLL(str(library))
        self.lib.centauri_fence.argtypes = []
        self.lib.centauri_fence.restype = None
        self.lib.centauri_read32.argtypes = [ctypes.c_void_p]
        self.lib.centauri_read32.restype = ctypes.c_uint32
        self.lib.centauri_write32.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
        self.lib.centauri_write32.restype = None
        self.ba = SimpleNamespace(phys_base=state['dram_base'],
                                  mem_size_actual=state['dram_size'])
        self.iface, self.p = self, self
        self.allowed = list(state['reserved_dram'])
        self.allowed.extend(((state['bar0_physical'], 0x10000),
                             (state['dart_base'], 0x4000)))

    def _mapping(self, address, length):
        if length <= 0 or not any(base <= address and address + length <= base + size
                                  for base, size in self.allowed):
            raise ValueError('Access outside the retained DMA/MMIO allocations')
        page = mmap.PAGESIZE
        base, offset = address & -page, address & (page - 1)
        size = (offset + length + page - 1) & -page
        key = base, size
        if key not in self.maps:
            self.maps[key] = mmap.mmap(self.fd, size, mmap.MAP_SHARED,
                                       mmap.PROT_READ | mmap.PROT_WRITE, offset=base)
        return self.maps[key], offset

    def readmem(self, address, length):
        mapping, offset = self._mapping(address, length)
        self.lib.centauri_fence()
        result = mapping[offset:offset + length]
        self.lib.centauri_fence()
        return result

    def writemem(self, address, data):
        mapping, offset = self._mapping(address, len(data))
        self.lib.centauri_fence()
        mapping[offset:offset + len(data)] = data
        self.lib.centauri_fence()

    def dc_ivac(self, address, length):
        # O_SYNC arm64 mappings of reserved system RAM are Normal-NC.
        self.lib.centauri_fence()

    dc_cvac = dc_ivac

    def read(self, address, width):
        if width != 32 or address & 3:
            raise ValueError('Only aligned 32-bit MMIO is supported')
        mapping, offset = self._mapping(address, 4)
        pointer = ctypes.addressof(ctypes.c_char.from_buffer(mapping, offset))
        return self.lib.centauri_read32(pointer)

    def write(self, address, value, width):
        if width != 32 or address & 3 or not 0 <= value < 1 << 32:
            raise ValueError('Only aligned 32-bit MMIO is supported')
        mapping, offset = self._mapping(address, 4)
        pointer = ctypes.addressof(ctypes.c_char.from_buffer(mapping, offset))
        self.lib.centauri_write32(pointer, value)


class RetainedControl:
    def __init__(self, state, memory):
        self.u, self.p, self.state = memory, memory, state
        self.bank = state['bar0_physical']
        self.iova, self.physical = state['context_iova'], state['context_physical']
        self.arena_iova = state['owned_tx_arena_iova']
        self.arena_physical = state['owned_tx_arena_physical']
        self.dart = RetainedTranslations(state['streams']['2']['mappings'])
        if memory.readmem(state['handoff_nonce_physical'], 32).hex() != state['handoff_nonce']:
            raise RuntimeError('Stale or mismatched boot handoff metadata')
        context = memory.read(self.bank + R.WIFI_IPC_CONTEXT_LOW, 32) | memory.read(self.bank + R.WIFI_IPC_CONTEXT_HIGH, 32) << 32
        if context != self.iova:
            raise RuntimeError('The retained IPC context has changed')
        self._require_ready()

    def _require_ready(self):
        if self.u.read(self.bank + R.WIFI_STARTUP_STATUS, 32) != 2 or self.u.read(self.bank + R.WIFI_IPC_STATE, 32) != 2:
            raise RuntimeError('Firmware and IPC must remain ready')
        if self.u.read(self.state['dart_base'] + DART_ERROR_STATUS, 32):
            raise RuntimeError('DART reports an outstanding error')

    def snapshot(self):
        fields = dict(device_completion=WIFI_DEVICE_COMPLETION_INDICES,
                      device_submission=WIFI_DEVICE_SUBMISSION_INDICES,
                      host_completion=WIFI_HOST_COMPLETION_INDICES,
                      host_submission=WIFI_HOST_SUBMISSION_INDICES)
        return dict(firmware=self.u.read(self.bank + R.WIFI_STARTUP_STATUS, 32),
                    ipc=self.u.read(self.bank + R.WIFI_IPC_STATE, 32),
                    dart_error=self.u.read(self.state['dart_base'] + DART_ERROR_STATUS, 32),
                    **{name: struct.unpack('<H', self.u.readmem(self.physical + offset, 2))[0]
                       for name, offset in fields.items()})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('handoff', type=Path)
    parser.add_argument('--library', type=Path, default=Path(__file__).with_name('centauri_native.so'))
    parser.add_argument('--interface', default='centauri0')
    args = parser.parse_args()
    state = json.loads(args.handoff.read_text())
    compatible = Path('/proc/device-tree/compatible').read_bytes().split(b'\0')
    if os.uname().machine != 'aarch64' or b'apple,t8142' not in compatible:
        raise RuntimeError('This handoff requires a T8142 device')
    memory = LinuxMemory(state, args.library.resolve())
    control = RetainedControl(state, memory)
    configs = {(item['kind'], item['queue']): tuple(item['words']) for item in state['queue_configs']}
    net = EthernetQueues(control, state['context_iova'], configs, bytes.fromhex(state['station_mac']))
    net.poll_tx()
    net.poll_rx(want_data=False)
    net.enable_recycling()
    tap = os.open('/dev/net/tun', os.O_RDWR | os.O_NONBLOCK | os.O_CLOEXEC)
    # Linux UAPI: TUNSETIFF, IFF_TAP | IFF_NO_PI. No offload headers.
    name = args.interface.encode()
    if not 0 < len(name) < 16:
        raise ValueError('Invalid TAP interface name')
    fcntl.ioctl(tap, 0x400454ca, struct.pack('16sH22x', name, 0x1002))
    print('CENTAURI_LINUX_TAP_READY', args.interface, net.snapshot(), flush=True)
    counts, last = dict(tx=0, rx=0), time.monotonic()
    while True:
        for _ in range(8):
            if not select.select([tap], [], [], 0)[0]:
                break
            net.send(os.read(tap, 2048))
            counts['tx'] += 1
        for frame in net.poll_rx():
            if os.write(tap, frame) != len(frame):
                raise RuntimeError('Short TAP frame write')
            counts['rx'] += 1
        if time.monotonic() - last >= 10:
            print('CENTAURI_LINUX_TAP_PROGRESS', counts, net.snapshot(), flush=True)
            last = time.monotonic()
        time.sleep(0.001)


if __name__ == '__main__':
    main()
