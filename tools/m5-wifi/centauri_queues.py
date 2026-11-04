# SPDX-License-Identifier: GPL-2.0-only
"""Owned data queues reconstructed from reference-39 and bare target-40.

The setup words retain observed values whose individual bit meanings remain
unresolved. Ring geometry, index offsets, receive credits and the runtime
greeting have been exercised without a running guest.
"""
import struct
from dataclasses import dataclass


ARENA_SIZE = 0x200000
DEVICE_QUEUE_INDICES = 0x7a
HOST_QUEUE_INDICES = 0x96
QUEUE_DOORBELLS = 0x1000
DESCRIPTOR = struct.Struct('<IQI')
INLINE_CAPACITY = 0x400
INLINE_STRIDE = DESCRIPTOR.size + INLINE_CAPACITY


@dataclass(frozen=True)
class Profile:
    offset: int
    entries: int
    stride: int
    capacity: int
    mode: int
    receive_target: int = 0
    pool: int | None = None


PROFILES = {
    8: Profile(0x00000, 4, 16, 0x1000, 0, 3, 0x18000),
    2: Profile(0x04000, 16, 16, 0x8000, 0, 15, 0x20000),
    4: Profile(0x08000, 16, INLINE_STRIDE, INLINE_CAPACITY, 0x40100000, 7),
    3: Profile(0x10000, 16, INLINE_STRIDE, INLINE_CAPACITY, 0x40100000),
}


class DataArena:
    """Allocate mappings before control transfers, then keep them fixed."""

    def __init__(self, ipc, iova=None):
        from m1n1.hw.dart8110 import R_TLB_OP

        before = ipc.snapshot()
        if before['index_pairs'] != [[0, 0], [0, 0]]:
            raise RuntimeError('Allocate the arena before starting control transfers')
        candidates = [iova] if iova is not None else (
            list(range(0x07000000, 0x07e00000, ARENA_SIZE)) +
            list(range(0x01000000, 0x07000000, ARENA_SIZE)))
        selected = None
        for candidate in candidates:
            if candidate < 0 or candidate & 0x3fff or candidate + ARENA_SIZE > 1 << 36:
                raise ValueError('Arena must fit in aligned DART address space')
            if ipc.dart.iotranslate(ipc.sid, candidate, ARENA_SIZE) == [(None, ARENA_SIZE)]:
                selected = candidate
                break
        if selected is None:
            raise RuntimeError('No unused IOVA extent for the owned arena')
        physical = ipc.pages.memalign(0x4000, ARENA_SIZE)
        # Independent223/224 qualify these shared service queues through PF0
        # only, including full radio startup and a Bluetooth link. Peer domains
        # own their own resources; no cross-function IOVA aliases are needed.
        if not ipc.regs.TCR[ipc.sid].reg.TRANSLATE_ENABLE or not ipc.regs.TTBR[ipc.sid].reg.VALID:
            raise RuntimeError('Preserving data DMA requires a valid translated domain')
        ipc.dart.iomap_at(ipc.sid, selected, physical, ARENA_SIZE)
        for address in ipc.dart.dart.pt_cache:
            ipc.u.proxy.dc_cvac(address, 0x4000)
        ipc.regs.TLB_OP.val = R_TLB_OP(STREAM=ipc.sid, OP=1).value
        base = ipc.u.adt['/arm-io/dart-apcie0'].get_reg(0)[0]
        ipc.driver.wait32(base + 0x80, 1 << 31, 0, 'Data arena invalidation', timeout=1)
        if ipc.dart.iotranslate(ipc.sid, selected, ARENA_SIZE) != [(physical, ARENA_SIZE)]:
            raise RuntimeError('Data arena does not map to its owned allocation')
        self._bind(ipc, selected, physical)

    @classmethod
    def from_owned_memory(cls, ipc, iova, physical, states=None):
        """Continue an existing prototype's owned allocation without remapping."""
        if (physical, ARENA_SIZE) not in ipc.pages.pages:
            raise ValueError('Arena is not an allocation owned by this IPC instance')
        if ipc.dart.iotranslate(ipc.sid, iova, ARENA_SIZE) != [(physical, ARENA_SIZE)]:
            raise RuntimeError('Existing arena mappings differ from the owned allocation')
        arena = cls.__new__(cls)
        arena._bind(ipc, iova, physical, states)
        return arena

    def _bind(self, ipc, iova, physical, states=None):
        self.ipc, self.u, self.iova, self.physical = ipc, ipc.u, iova, physical
        self.queues = {number: DataQueue(self, number, profile)
                       for number, profile in PROFILES.items()}
        if states:
            for number, state in states.items():
                queue = self.queues[number]
                if not 0 <= state['producer'] < queue.profile.entries or not 0 <= state['cursor'] < queue.profile.entries:
                    raise ValueError('Existing queue cursor is outside its ring')
                queue.producer, queue.cursor = state['producer'], state['cursor']

    def replenish(self):
        for queue in self.queues.values():
            target = queue.profile.receive_target
            if target:
                outstanding = (queue.producer - queue.cursor) % queue.profile.entries
                if outstanding < target:
                    queue.post_receive(target - outstanding)


class DataQueue:
    def __init__(self, arena, number, profile):
        self.arena, self.number, self.profile = arena, number, profile
        self.u, self.p = arena.u, arena.u.proxy
        self.physical, self.iova = arena.physical + profile.offset, arena.iova + profile.offset
        self.cursor = self.producer = 0

    def configure(self):
        profile = self.profile
        frame = [profile.mode | 1, self.number << 16 | self.number,
                 self.iova & 0xffffffff, self.iova >> 32,
                 0xffffffff, 0xffffffff, 0xffff0000 | profile.entries,
                 0x400000 | self.number, 0, 0, 0, 0, 0]
        return self.arena.ipc.submit(struct.pack('<13I', *frame))

    def consumer(self):
        address = self.arena.ipc.physical + DEVICE_QUEUE_INDICES + self.number * 2
        self.p.dc_ivac(address, 2)
        value = self.u.read(address, 16)
        if value >= self.profile.entries:
            raise RuntimeError('Device consumer is outside its queue')
        return value

    def _kick(self):
        ipc = self.arena.ipc
        address = ipc.physical + HOST_QUEUE_INDICES + self.number * 2
        self.u.write(address, self.producer, 16)
        self.p.dc_cvac(address, 2)
        self.u.write(ipc.bank + QUEUE_DOORBELLS + self.number * 4, self.producer, 32)

    def post_receive(self, count):
        profile = self.profile
        if not profile.receive_target:
            raise ValueError('This profile is an inline transmit queue')
        outstanding = (self.producer - self.cursor) % profile.entries
        if not 0 <= count <= profile.entries - 1 - outstanding:
            raise RuntimeError('Receive posting would overwrite an outstanding entry')
        for _ in range(count):
            slot = self.producer
            address = self.physical + slot * profile.stride
            if profile.pool is None:
                pointer, flags = 0xffffffffffffffff, 0
            else:
                pointer = self.arena.iova + profile.pool + slot * profile.capacity
                flags = 1
            self.u.iface.writemem(address, DESCRIPTOR.pack(profile.capacity << 8 | flags, pointer, slot))
            self.p.dc_cvac(address, DESCRIPTOR.size)
            self.producer = (slot + 1) % profile.entries
        self._kick()

    def send_inline(self, payload):
        profile = self.profile
        if profile.receive_target or profile.pool is not None or len(payload) > profile.capacity:
            raise ValueError('Payload does not fit this inline transmit profile')
        slot = self.producer
        if (slot + 1) % profile.entries == self.consumer():
            raise RuntimeError('Inline transmit ring is full')
        address = self.physical + slot * profile.stride
        record = DESCRIPTOR.pack(len(payload) << 8 | 2, 0xffffffffffffffff, slot) + payload
        self.u.iface.writemem(address, record)
        self.p.dc_cvac(address, len(record))
        self.producer = (slot + 1) % profile.entries
        self._kick()

    def receive(self):
        """Read completed owned packets; leave queue-8 and large queue-2 data opaque."""
        profile = self.profile
        if not profile.receive_target:
            raise ValueError('Use consumer() for an inline transmit queue')
        consumer = self.consumer()
        packets = []
        while self.cursor != consumer:
            slot = self.cursor
            address = self.physical + slot * profile.stride
            self.p.dc_ivac(address, profile.stride)
            descriptor, pointer, cookie = DESCRIPTOR.unpack(self.u.iface.readmem(address, DESCRIPTOR.size))
            length = descriptor >> 8
            if length > profile.capacity:
                raise RuntimeError('Received byte count exceeds its owned buffer')
            payload = None
            if profile.pool is None:
                if pointer != 0xffffffffffffffff:
                    raise RuntimeError('Inline descriptor unexpectedly points outside its record')
                payload = self.u.iface.readmem(address + DESCRIPTOR.size, length)
            else:
                expected = self.arena.iova + profile.pool + slot * profile.capacity
                if pointer != expected:
                    raise RuntimeError('External descriptor changed its owned buffer address')
                if self.number == 2 and length <= 0x400:
                    physical = self.arena.physical + profile.pool + slot * profile.capacity
                    self.p.dc_ivac(physical, length)
                    payload = self.u.iface.readmem(physical, length)
            packets.append(dict(queue=self.number, slot=slot, size=length,
                                format=descriptor & 0xff, cookie=cookie, payload=payload))
            self.cursor = (slot + 1) % profile.entries
        return packets


def encode_runtime_message(kind, sequence, body=b''):
    length = 8 + len(body)
    if not 0 <= kind <= 0xff or not 0 <= sequence <= 0xffffffff or length > 0xffffff:
        raise ValueError('Runtime message fields exceed their wire widths')
    return struct.pack('<II', kind << 24 | length, sequence) + body


def decode_runtime_message(packet):
    if len(packet) < 8:
        raise ValueError('Runtime packet is shorter than its header')
    word, sequence = struct.unpack_from('<II', packet)
    if word & 0xffffff != len(packet):
        raise ValueError('Runtime packet byte count differs from its header')
    return dict(kind=word >> 24, sequence=sequence, body=packet[8:])
