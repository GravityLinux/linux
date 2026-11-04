# SPDX-License-Identifier: GPL-2.0-only
"""Primed Wi-Fi control channel, reconstructed from runtime host metadata.

The previous host driver must be stopped and its DMA allocations retained.
This uses the bare proxy; it neither starts a guest nor reads firmware code.
"""
import struct
import time
from m1n1.hw.dart import DART
import centauri_regs as R

SID = 2
COMPLETION_QUEUES = 234
SUBMISSION_QUEUES = 192
DEVICE_COMPLETION_INDICES = R.WIFI_DEVICE_COMPLETION_INDICES
DEVICE_SUBMISSION_INDICES = R.WIFI_DEVICE_SUBMISSION_INDICES
HOST_COMPLETION_INDICES = R.WIFI_HOST_COMPLETION_INDICES
HOST_SUBMISSION_INDICES = R.WIFI_HOST_SUBMISSION_INDICES
ADMIN_SUBMISSION_RING = 0x720
ADMIN_COMPLETION_RING = 0x820
ADMIN_ENTRIES = 16
ADMIN_ENTRY_SIZE = 16
ADMIN_NOTIFY = 0x1004
FRAME_STRIDE = 0x80
FRAME_AREA = 0x10000
ARENA_SIZE = 0x200000


class WiFiControl:
    """Attach a prepared context and separately owned control-frame arena.

    The allocator must own the supplied two-MiB stream-2 mapping. The first
    128 KiB reserve context space and sixteen separate frame slots; later regions may hold
    caller-owned queues. No page table or initialization state is changed.
    """

    def __init__(self, utils, driver, arena_iova, arena_physical, pages):
        self.u, self.p, self.driver = utils, utils.proxy, driver
        if driver.cfg(1, 0, 1, 0) != 0x1902106b:
            raise RuntimeError('Runtime Wi-Fi function required')
        if pages.u is not utils or (arena_physical, ARENA_SIZE) not in pages.pages:
            raise ValueError('Control arena is not owned by the allocator')
        self.bank = driver.runtime_bars[1][0]['physical']
        self.dart = DART.from_adt(utils, '/arm-io/dart-apcie0')
        self.pages = pages
        self.arena_iova, self.arena_physical = arena_iova, arena_physical
        window = utils.read(self.bank + R.WIFI_DMA_WINDOW_BASE_LOW, 32) | utils.read(self.bank + R.WIFI_DMA_WINDOW_BASE_HIGH, 32) << 32
        if arena_iova < window or arena_iova + ARENA_SIZE > window + utils.read(self.bank + R.WIFI_DMA_WINDOW_LENGTH, 32):
            raise ValueError('Control arena is outside the DMA window')
        if self.dart.iotranslate(SID, arena_iova, ARENA_SIZE) != [(arena_physical, ARENA_SIZE)]:
            raise RuntimeError('Control arena mapping differs from its owned allocation')
        self.iova = utils.read(self.bank + R.WIFI_IPC_CONTEXT_LOW, 32) | utils.read(self.bank + R.WIFI_IPC_CONTEXT_HIGH, 32) << 32
        ranges = self.dart.iotranslate(SID, self.iova, 0x4000)
        if len(ranges) != 1 or ranges[0][0] is None or ranges[0][1] != 0x4000:
            raise RuntimeError('Context is not one translated host page')
        self.physical = ranges[0][0]
        lower = utils.ba.phys_base & ~0xffffffff
        if not lower <= self.physical or self.physical + 0x4000 > lower + utils.ba.mem_size_actual:
            raise RuntimeError('Context is outside host DRAM')
        raw = self._read_context(0, 0x68)
        if struct.unpack_from('<Q', raw)[0] != 0x680300:
            raise RuntimeError('Unknown Wi-Fi context format')
        if struct.unpack_from('<2H', raw, 0x30) != (COMPLETION_QUEUES, SUBMISSION_QUEUES):
            raise RuntimeError('Unknown Wi-Fi queue counts')
        for field, offset in ((8, 0x68), (0x10, DEVICE_COMPLETION_INDICES),
                              (0x18, DEVICE_SUBMISSION_INDICES),
                              (0x20, HOST_COMPLETION_INDICES),
                              (0x28, HOST_SUBMISSION_INDICES),
                              (0x34, ADMIN_COMPLETION_RING), (0x3c, ADMIN_SUBMISSION_RING)):
            if struct.unpack_from('<Q', raw, field)[0] != self.iova + offset:
                raise RuntimeError('Wi-Fi context geometry differs from the observed profile')
        if struct.unpack_from('<I', raw, 0x44)[0] != 0x00100010:
            raise RuntimeError('Unknown Wi-Fi admin ring sizes')
        self._require_ready()

    def _read_context(self, offset, size):
        if offset < 0 or size <= 0 or offset + size > ADMIN_COMPLETION_RING + 0x100:
            raise ValueError('Read exceeds known control metadata')
        self.p.dc_ivac(self.physical + offset, size)
        return self.u.iface.readmem(self.physical + offset, size)

    def _index(self, offset):
        value = struct.unpack('<H', self._read_context(offset, 2))[0]
        if value >= ADMIN_ENTRIES:
            raise RuntimeError('Admin cursor is outside the ring')
        return value

    def _set_index(self, offset, value):
        self.p.dc_ivac(self.physical + offset, 2)
        self.u.write(self.physical + offset, value, 16)
        self.p.dc_cvac(self.physical + offset, 2)

    def _require_ready(self):
        if self.u.read(self.bank + R.WIFI_STARTUP_STATUS, 32) != 2 or self.u.read(self.bank + R.WIFI_IPC_STATE, 32) != 2:
            raise RuntimeError('Wi-Fi firmware and IPC must be ready')
        if self.dart.dart.regs.ERROR.val:
            raise RuntimeError('DART reports an outstanding error')

    def snapshot(self):
        return dict(firmware=self.u.read(self.bank + R.WIFI_STARTUP_STATUS, 32), ipc=self.u.read(self.bank + R.WIFI_IPC_STATE, 32),
                    device_completion=self._index(DEVICE_COMPLETION_INDICES),
                    device_submission=self._index(DEVICE_SUBMISSION_INDICES),
                    host_completion=self._index(HOST_COMPLETION_INDICES),
                    host_submission=self._index(HOST_SUBMISSION_INDICES),
                    dart_error=self.dart.dart.regs.ERROR.val)

    def submit(self, frame, timeout=2):
        """Transfer one fixed control record, returning transport completion.

        Notification always writes 1, independently of the producer index.
        Only one request may be outstanding. No packet payload is read.
        """
        if len(frame) not in (44, 52, 54, 62):
            raise ValueError('Control frames must use a supported basic or extended layout')
        self._require_ready()
        before = self.snapshot()
        slot = before['host_submission']
        if before['device_completion'] != slot or before['device_submission'] != slot:
            raise RuntimeError('A prior Wi-Fi control request remains outstanding')
        pending = (before['device_completion'] - before['host_completion']) & 15
        if pending > 1:
            raise RuntimeError('Unexpected unacknowledged completion count')
        if pending:
            address = self.physical + ADMIN_COMPLETION_RING + before['host_completion'] * ADMIN_ENTRY_SIZE
            self.u.iface.writemem(address, bytes(ADMIN_ENTRY_SIZE))
            self.p.dc_cvac(address, ADMIN_ENTRY_SIZE)
        frame_offset = FRAME_AREA + slot * FRAME_STRIDE
        self.u.iface.writemem(self.arena_physical + frame_offset, frame)
        self.p.dc_cvac(self.arena_physical + frame_offset, len(frame))
        entry = self.physical + ADMIN_SUBMISSION_RING + slot * ADMIN_ENTRY_SIZE
        self.u.iface.writemem(entry, struct.pack('<IQI', len(frame) << 8 | 1,
                                               self.arena_iova + frame_offset, slot))
        self.p.dc_cvac(entry, ADMIN_ENTRY_SIZE)
        next_producer = (slot + 1) & 15
        self._set_index(HOST_COMPLETION_INDICES, before['device_completion'])
        self._set_index(HOST_SUBMISSION_INDICES, next_producer)
        self.u.write(self.bank + ADMIN_NOTIFY, 1, 32)
        deadline = time.monotonic() + timeout
        while True:
            self._require_ready()
            state = self.snapshot()
            if state['device_completion'] == state['device_submission'] == next_producer:
                raw = self._read_context(ADMIN_COMPLETION_RING + slot * ADMIN_ENTRY_SIZE, ADMIN_ENTRY_SIZE)
                completion = struct.unpack('<4I', raw)
                if completion[:2] != (0x401, len(frame) << 16 | slot):
                    raise RuntimeError(f'Unexpected Wi-Fi completion: {completion!r}')
                return dict(slot=slot, bytes_completed=len(frame),
                            completion_words=list(completion), indices=state)
            if time.monotonic() >= deadline:
                raise TimeoutError(f'Wi-Fi control request is pending: {state!r}')
            time.sleep(0.001)


def completion_queue_config(queue, iova, entries):
    """Encode the basic completion-ring profile captured during Wi-Fi startup.

    Unresolved flags retain the observed values. This does not map memory
    or establish the higher-level result of the queue-registration request.
    """
    if not 0 <= queue < COMPLETION_QUEUES or not 0 < entries <= 0xffff:
        raise ValueError('Completion queue fields exceed the observed index/length widths')
    if not 0 < iova < 1 << 64:
        raise ValueError('Queue address must fit its unsigned wire field')
    return struct.pack('<11I', 2, queue << 16 | queue, iova & 0xffffffff, iova >> 32,
                       0xffff0000 | entries, 0xffff, 0, 2, 0, 0, 0)


def submission_queue_config(queue, iova, entries):
    """Encode the observed basic submission/completion pair with equal IDs.

    The second index in word 6 matches the completion queue in all six
    startup pairs. Other profiles and the extended admin records are not
    synthesized by this helper.
    """
    if not 0 <= queue < SUBMISSION_QUEUES or not 0 < entries <= 0xffff:
        raise ValueError('Submission queue fields exceed the observed index/length widths')
    if not 0 < iova < 1 << 64:
        raise ValueError('Queue address must fit its unsigned wire field')
    return struct.pack('<13I', 1, queue << 16 | queue, iova & 0xffffffff, iova >> 32,
                       0xffffffff, 0xffffffff, queue << 16 | entries, 0x80000,
                       0, 2, 0, 0, 0)


def make_wifi_context(iova):
    """Build the fresh header, indices and admin rings observed at publication.

    The auxiliary prefix is initially zero in reference-61. Its semantics
    remain unresolved; callers should zero their entire context allocation.
    """
    from centauri_ipc import make_context
    common = make_context(iova)
    data = bytearray(0x960)
    data[:0x68] = common[:0x68]
    for field, offset in ((0x18, DEVICE_SUBMISSION_INDICES),
                          (0x20, HOST_COMPLETION_INDICES),
                          (0x28, HOST_SUBMISSION_INDICES),
                          (0x34, ADMIN_COMPLETION_RING),
                          (0x3c, ADMIN_SUBMISSION_RING), (0x58, 0x920)):
        struct.pack_into('<Q', data, field, iova + offset)
    struct.pack_into('<2H', data, 0x30, COMPLETION_QUEUES, SUBMISSION_QUEUES)
    struct.pack_into('<I', data, 0x60, 0x00500000)
    struct.pack_into('<II', data, 0x68, 2, 2)
    return bytes(data)


CONTROL_HEADER = struct.Struct('<4IHHI')


def control_request(command, body=b'', transaction=1):
    """Encode the captured runtime header with its four-word prefix zeroed.

    Command 8 with an empty body is the first reference request. The prefix
    words' purposes and other command meanings are not inferred here. The
    transaction is echoed by replies and increments across reference requests.
    """
    length = CONTROL_HEADER.size + len(body)
    if not 0 <= command <= 0xffffffff or not 0 <= transaction <= 0xffff or length > 0xffff:
        raise ValueError('Control message fields exceed their wire widths')
    return CONTROL_HEADER.pack(0, 0, 0, 0, length, transaction, command) + body


def decode_control_header(prefix, received_size=None):
    """Decode only the fixed runtime header; leave the message body opaque."""
    if len(prefix) < CONTROL_HEADER.size:
        raise ValueError('Control message header is truncated')
    a, b, c, d, length, transaction, command = CONTROL_HEADER.unpack_from(prefix)
    if length < CONTROL_HEADER.size or (received_size is not None and length != received_size):
        raise ValueError('Control message length differs from its receive completion')
    return dict(prefix_words=(a, b, c, d), length=length, transaction=transaction, command=command)
