# SPDX-License-Identifier: GPL-2.0-only
"""Bounded published global-control frames; never firmware or nested payloads."""
import struct
import sys
from pathlib import Path

# run_guest.py loads observers with runpy, without adding their directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from trace_centauri_bootstrap import PageTableReader
from m1n1.hv import TraceMode
from m1n1.hw.dart import DART
from m1n1.utils import irange


class ControlFrameCapture:
    def __init__(self, hv, capture):
        self.hv, self.capture = hv, capture
        self.seen = set()
        self.observations = 0
        self.dart = None

    def read_dma(self, sid, iova, length):
        if not 0 < length <= 0x3b0 or iova < 0 or iova + length > 1 << 36:
            raise ValueError('Control metadata extent outside capture bounds')
        ranges = self.dart.iotranslate(sid, iova, length)
        lower = self.hv.u.ba.phys_base & ~0xffffffff
        upper = lower + self.hv.u.ba.mem_size_actual
        if sum(n for _, n in ranges) != length or any(
                p is None or n <= 0 or p < lower or p + n > upper for p, n in ranges):
            raise ValueError('Published control metadata outside mapped DRAM')
        return b''.join(self.hv.iface.readmem(p, n) for p, n in ranges)

    def write(self, event):
        if not event.flags.WRITE or event.flags.WIDTH != 2 or self.observations >= 16:
            return
        self.observations += 1
        ecam = self.hv.adt['/arm-io/apcie'].get_reg(0)[0]
        if self.hv.u.read(ecam + 0x100000, 32) != 0x1901106b:
            return
        if self.hv.u.read(0xb82028050, 32) != 2:
            return
        if self.dart is None:
            self.dart = DART.from_adt(self.hv.u, '/arm-io/dart-apcie0')
            lower = self.hv.u.ba.phys_base & ~0xffffffff
            self.dart.dart.iface = PageTableReader(
                self.hv.iface, lower, self.hv.u.ba.mem_size_actual)
        port = self.hv.adt['/arm-io/apcie'].get_reg(8)[0]
        rid_words = [self.hv.u.read(port + 0x3000 + 4 * i, 32) for i in range(16)]
        sids = {w >> 16 & 0xff for w in rid_words if w & 0x80000000 and w & 0xffff == 0x100}
        if len(sids) != 1:
            return
        sid = sids.pop()
        address = self.hv.u.read(0xb82028054, 32) | self.hv.u.read(0xb82028058, 32) << 32
        self.dart.invalidate_cache()
        data = self.read_dma(sid, address, 0x3b0)
        if struct.unpack_from('<4Q', data) != (0x680300, address + 0x68,
                                               address + 0x78, address + 0x7a):
            return
        self.capture.emit('global_control_ring', doorbell=hex(event.data),
            context_iova=hex(address), sid=sid,
            pair_a=list(struct.unpack_from('<2H', data, 0x78)),
            pair_b=list(struct.unpack_from('<2H', data, 0x94)),
            ring_a=[list(struct.unpack_from('<4I', data, 0xb0 + 16 * i)) for i in range(16)],
            ring_b=[list(struct.unpack_from('<4I', data, 0x1b0 + 16 * i)) for i in range(16)],
            policy='published global control ring metadata only')
        for i in range(16):
            descriptor, pointer, cookie = struct.unpack_from('<IQI', data, 0xb0 + i * 16)
            length = descriptor >> 8
            key = (i, descriptor, pointer, cookie)
            if key in self.seen or descriptor & 0xff != 1 or length not in (4, 52):
                continue
            self.seen.add(key)
            frame = self.read_dma(sid, pointer, length)
            self.capture.emit('global_control_frame', slot=i, cookie=cookie,
                descriptor=hex(descriptor), iova=hex(pointer), size=length,
                words=[hex(v) for v in struct.unpack('<' + 'I' * (length // 4), frame)],
                policy='exact-size published control frame; queue prefix capture declared separately')
            if length == 52:
                words = struct.unpack('<13I', frame)
                # This specific initial command publishes a fresh host queue
                # allocation. Capture only its 64-byte control prefix, not
                # ring payloads, further pointers, or device firmware memory.
                if words[0] & 0xffff == 1:
                    queue_iova = words[2] | words[3] << 32
                    prefix = self.read_dma(sid, queue_iova, 64)
                    self.capture.emit('global_queue_control_prefix', slot=i,
                        iova=hex(queue_iova), size=64,
                        words=[hex(v) for v in struct.unpack('<16I', prefix)],
                        policy='fresh host queue control prefix only; no nested pointers followed')


if 'hv' in globals():
    control_frame_capture = ControlFrameCapture(hv, centauri_trace)
    hv.add_tracer(irange(0xb82029000, 4), 'centauri/global-control-frames',
                  TraceMode.WSYNC, None, control_frame_capture.write)
    centauri_trace.emit('global_control_frame_capture', maximum_observations=16,
                        allowed_frame_sizes=[4, 52], queue_prefix_bytes=64,
                        policy='published host control and queue metadata only')
