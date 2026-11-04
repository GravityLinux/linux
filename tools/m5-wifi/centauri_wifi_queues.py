# SPDX-License-Identifier: GPL-2.0-only
"""Owned basic Wi-Fi queues, initially validated with numeric command 8.

Management headers are decoded; body reads require an explicit query or
bounded observer. Firmware must already be ready; no guest runs during queue
operation.
"""
import struct
import time
from dataclasses import dataclass

from centauri_queues import DESCRIPTOR
from centauri_wifi import (completion_queue_config, submission_queue_config,
                           control_request, decode_control_header,
                           DEVICE_COMPLETION_INDICES, DEVICE_SUBMISSION_INDICES,
                           HOST_COMPLETION_INDICES, HOST_SUBMISSION_INDICES, ADMIN_NOTIFY)

COMPLETION = struct.Struct('<4I')
POOL_SIZE = 0x800000
RING_REGION_SIZE = 0x8000
RING_AREA = 0x20000


@dataclass(frozen=True)
class Profile:
    submission_entries: int
    completion_entries: int
    capacity: int
    receive_target: int


PROFILES = {
    6: Profile(128, 1024, 8192, 32),
    5: Profile(16, 16, 2048, 8),
    2: Profile(154, 154, 4096, 153),
    3: Profile(96, 96, 4096, 95),
    4: Profile(512, 512, 2560, 511),
    1: Profile(128, 128, 8192, 0),
}
SETUP_ORDER = ((2, 6), (2, 5), (1, 6), (1, 5), (2, 1), (1, 1),
               (2, 2), (1, 2), (2, 3), (1, 3), (2, 4), (1, 4))


class ManagementCommandError(RuntimeError):
    """Firmware returned nonzero status for an explicit asynchronous request."""

    def __init__(self, header):
        self.header = dict(header)
        super().__init__(f"Management command {header['command']:#x} rejected "
                         f"with prefix word 3={header['prefix_words'][3]:#x}")


def decode_completion(raw):
    """Decode the observed fixed completion, retaining unknown trailing words."""
    if len(raw) != COMPLETION.size:
        raise ValueError('A basic completion occupies sixteen bytes')
    flags, transfer, a, b = COMPLETION.unpack(raw)
    if flags & 0xffff != 0x401:
        raise ValueError('Unknown basic completion format')
    return dict(source=flags >> 16, bytes_completed=transfer >> 16,
                cookie=transfer & 0xffff, extra_words=(a, b))


class WiFiBasicQueues:
    """Configure a fresh context, or continue an initialized owned prototype."""

    def __init__(self, control, pool_iova, pool_physical):
        self._bind(control, pool_iova, pool_physical)

    def _bind(self, control, pool_iova, pool_physical):
        self.control, self.u, self.p = control, control.u, control.p
        self.pool_iova, self.pool_physical = pool_iova, pool_physical
        if (pool_physical, POOL_SIZE) not in control.pages.pages:
            raise ValueError('Receive pool is not owned by the control allocator')
        if control.dart.iotranslate(2, pool_iova, POOL_SIZE) != [(pool_physical, POOL_SIZE)]:
            raise RuntimeError('Receive pool mapping differs from its owned allocation')
        window = self.u.read(control.bank + 0x18, 32) | self.u.read(control.bank + 0x1c, 32) << 32
        if pool_iova < window or pool_iova + POOL_SIZE > window + self.u.read(control.bank + 0x20, 32):
            raise ValueError('Receive pool is outside the published DMA window')
        self.rings, self.queues = {}, {}
        offset = RING_AREA
        for kind, number in SETUP_ORDER:
            profile = PROFILES[number]
            count = profile.completion_entries if kind == 2 else profile.submission_entries
            self.rings[kind, number] = dict(iova=control.arena_iova + offset,
                physical=control.arena_physical + offset, entries=count)
            offset += RING_REGION_SIZE
        if offset > 0x200000:
            raise ValueError('Basic rings exceed the control arena')
        offset = 0
        for number, profile in PROFILES.items():
            self.queues[number] = dict(profile=profile, producer=0, cursor=0,
                pool_iova=pool_iova + offset, pool_physical=pool_physical + offset)
            offset += profile.submission_entries * profile.capacity
            offset = (offset + 0x3fff) & ~0x3fff
        if offset > POOL_SIZE:
            raise ValueError('Basic buffers exceed the owned receive pool')
        self.received_headers = []
        self.completion_counts = {number: 0 for number in PROFILES}
        self.configured = False
        self.next_transaction = 1
        self.read_body_for = None
        self.receive_observers = {}
        self.control_event_observers = {}

    def observe_control_event(self, command, max_bytes, callback):
        """Deliver selected asynchronous management bodies before recycling.

        Event commands have bit 31 set. Only an explicitly selected command's
        bounded completed body is copied; no embedded pointer is followed.
        The callback receives its decoded header and body prefix. None removes
        the observer. As with receive observers, failures retain the credit.
        """
        if not 0x80000000 <= command <= 0xffffffff:
            raise ValueError('Expected an asynchronous control command')
        if callback is None:
            self.control_event_observers.pop(command, None)
            return
        if not callable(callback) or not 0 < max_bytes <= PROFILES[2].capacity - 24:
            raise ValueError('Event observer needs a callback and bounded body size')
        self.control_event_observers[command] = (max_bytes, callback)

    def observe_receive(self, number, max_bytes, callback):
        """Observe a bounded prefix before an opaque receive credit is recycled.

        Only queues 3..6 are exposed here. Management replies use ``query``.
        The callback receives completion metadata and at most ``max_bytes``
        copied from the completed owned buffer; embedded pointers are never
        followed. No extra receive payload is read unless explicitly enabled.
        Passing None removes the observer. Callback failures leave the current
        completion unconsumed, so callers can remove the observer and retry.
        """
        if number not in (3, 4, 5, 6):
            raise ValueError('Only opaque receive queues can be observed')
        if callback is None:
            self.receive_observers.pop(number, None)
            return
        if not callable(callback) or not 0 < max_bytes <= PROFILES[number].capacity:
            raise ValueError('Observer needs a callback and a bounded prefix size')
        self.receive_observers[number] = (max_bytes, callback)

    @classmethod
    def from_initialized(cls, control, pool_iova, pool_physical, states, next_transaction):
        """Continue matching owned queues without resetting or reconfiguring them.

        The prior prototype must use SETUP_ORDER and these exact allocations.
        Supplied cursors are checked against host-owned context indices.
        """
        queues = cls.__new__(cls)
        queues._bind(control, pool_iova, pool_physical)
        if not 0 <= next_transaction <= 0xffff:
            raise ValueError('Next transaction must fit the management header')
        queues.next_transaction = next_transaction
        if set(states) != set(PROFILES):
            raise ValueError('Initialized state must cover all basic queues')
        for number, state in states.items():
            q = queues.queues[number]
            profile = q['profile']
            producer, cursor = state['producer'], state['cursor']
            if not 0 <= producer < profile.submission_entries or not 0 <= cursor < profile.completion_entries:
                raise ValueError('Initialized cursor is outside its ring')
            if queues._index(HOST_SUBMISSION_INDICES + number * 2, profile.submission_entries) != producer:
                raise RuntimeError('Host submission cursor differs from the supplied state')
            if queues._index(HOST_COMPLETION_INDICES + number * 2, profile.completion_entries) != cursor:
                raise RuntimeError('Host completion cursor differs from the supplied state')
            q.update(producer=producer, cursor=cursor)
        queues.configured = True
        return queues

    def _index(self, offset, limit):
        address = self.control.physical + offset
        self.p.dc_ivac(address, 2)
        value = self.u.read(address, 16)
        if not 0 <= value < limit:
            raise RuntimeError('Wi-Fi queue cursor is outside its ring')
        return value

    def _set_index(self, offset, value):
        address = self.control.physical + offset
        self.p.dc_ivac(address, 2)
        self.u.write(address, value, 16)
        self.p.dc_cvac(address, 2)

    def _notify(self):
        self.u.write(self.control.bank + ADMIN_NOTIFY, 1, 32)

    def configure(self):
        state = self.control.snapshot()
        if any(state[field] for field in ('device_completion', 'device_submission',
                                         'host_completion', 'host_submission')):
            raise RuntimeError('Configure basic queues only on a fresh owned context')
        for kind, number in SETUP_ORDER:
            ring = self.rings[kind, number]
            self.p.memset8(ring['physical'], 0, RING_REGION_SIZE)
            self.p.dc_cvac(ring['physical'], RING_REGION_SIZE)
            builder = completion_queue_config if kind == 2 else submission_queue_config
            self.control.submit(builder(number, ring['iova'], ring['entries']))
            if kind == 1 and number in (6, 5):
                self.post_receive(number, PROFILES[number].receive_target)
        for number in (3, 4, 2):
            self.post_receive(number, PROFILES[number].receive_target)
        self._set_index(HOST_COMPLETION_INDICES, self.control.snapshot()['device_completion'])
        self.configured = True

    def post_receive(self, number, count):
        q, profile = self.queues[number], PROFILES[number]
        if not profile.receive_target:
            raise ValueError('This basic queue transmits management requests')
        consumed = self._index(DEVICE_SUBMISSION_INDICES + number * 2, profile.submission_entries)
        free = (consumed - q['producer'] - 1) % profile.submission_entries
        if not 0 <= count <= free:
            raise RuntimeError('Receive posting would overwrite an active descriptor')
        remaining = count
        while remaining:
            first = q['producer']
            n = min(remaining, profile.submission_entries - first)
            records = b''.join(DESCRIPTOR.pack(profile.capacity << 8 | 1,
                q['pool_iova'] + slot * profile.capacity, slot) for slot in range(first, first + n))
            address = self.rings[1, number]['physical'] + first * DESCRIPTOR.size
            self.u.iface.writemem(address, records)
            self.p.dc_cvac(address, len(records))
            q['producer'] = (first + n) % profile.submission_entries
            remaining -= n
        self._set_index(HOST_SUBMISSION_INDICES + number * 2, q['producer'])
        self._notify()

    def poll(self):
        self.control._require_ready()
        added = []
        changed = False
        for number in (6, 5, 1, 2, 3, 4):
            q, profile = self.queues[number], PROFILES[number]
            ring = self.rings[2, number]
            producer = self._index(DEVICE_COMPLETION_INDICES + number * 2, ring['entries'])
            while q['cursor'] != producer:
                address = ring['physical'] + q['cursor'] * COMPLETION.size
                self.p.dc_ivac(address, COMPLETION.size)
                completion = decode_completion(self.u.iface.readmem(address, COMPLETION.size))
                cookie, length = completion['cookie'], completion['bytes_completed']
                if completion['source'] != number or cookie >= profile.submission_entries or length > profile.capacity:
                    raise RuntimeError('Completion differs from the owned basic queue profile')
                if number in self.receive_observers:
                    limit, callback = self.receive_observers[number]
                    size = min(length, limit)
                    payload = q['pool_physical'] + cookie * profile.capacity
                    if size:
                        self.p.dc_ivac(payload, size)
                    prefix = self.u.iface.readmem(payload, size) if size else b''
                    callback(dict(queue=number, **completion), prefix)
                if number == 2 and length >= 24:
                    payload = q['pool_physical'] + cookie * profile.capacity
                    self.p.dc_ivac(payload, 24)
                    header = decode_control_header(self.u.iface.readmem(payload, 24), received_size=length)
                    if self.read_body_for == (header['transaction'], header['command']):
                        header['body'] = b''
                        if length > 24:
                            self.p.dc_ivac(payload + 24, length - 24)
                            header['body'] = self.u.iface.readmem(payload + 24, length - 24)
                    if header['command'] in self.control_event_observers:
                        limit, callback = self.control_event_observers[header['command']]
                        size = min(length - 24, limit)
                        if size:
                            self.p.dc_ivac(payload + 24, size)
                        body = self.u.iface.readmem(payload + 24, size) if size else b''
                        callback({k: v for k, v in header.items() if k != 'body'}, body)
                    self.received_headers.append(header)
                    added.append(header)
                self.u.iface.writemem(address, bytes(COMPLETION.size))
                self.p.dc_cvac(address, COMPLETION.size)
                q['cursor'] = (q['cursor'] + 1) % ring['entries']
                self.completion_counts[number] += 1
                changed = True
            self._set_index(HOST_COMPLETION_INDICES + number * 2, q['cursor'])
            if profile.receive_target:
                consumed = self._index(DEVICE_SUBMISSION_INDICES + number * 2, profile.submission_entries)
                outstanding = (q['producer'] - consumed) % profile.submission_entries
                if outstanding < profile.receive_target:
                    self.post_receive(number, profile.receive_target - outstanding)
        if changed:
            self._notify()
        return added

    def send_control(self, command, body=b''):
        if not self.configured:
            raise RuntimeError('Initialize the basic queues before sending a request')
        self.control._require_ready()
        q, profile = self.queues[1], PROFILES[1]
        slot = q['producer']
        consumed = self._index(DEVICE_SUBMISSION_INDICES + 2, profile.submission_entries)
        if (slot + 1) % profile.submission_entries == consumed:
            raise RuntimeError('Management transmit ring is full')
        transaction = self.next_transaction
        packet = control_request(command, body, transaction=transaction)
        if len(packet) > profile.capacity:
            raise ValueError('Request exceeds its owned transmit buffer')
        address = q['pool_physical'] + slot * profile.capacity
        self.u.iface.writemem(address, packet)
        self.p.dc_cvac(address, len(packet))
        descriptor = self.rings[1, 1]['physical'] + slot * DESCRIPTOR.size
        self.u.iface.writemem(descriptor, DESCRIPTOR.pack(len(packet) << 8 | 1,
            q['pool_iova'] + slot * profile.capacity, slot))
        self.p.dc_cvac(descriptor, DESCRIPTOR.size)
        q['producer'] = (slot + 1) % profile.submission_entries
        self._set_index(HOST_SUBMISSION_INDICES + 2, q['producer'])
        self._notify()
        self.next_transaction = (transaction + 1) & 0xffff
        return transaction

    def query(self, command, body=b'', timeout=5, read_body=False, reply_command=None):
        """Match command and transaction, optionally copying the runtime body.

        A distinct reply command is explicit: scan 0x1000f acknowledges with
        0x4001000f. No command bits are silently masked during matching.
        Explicit body reads cover only the completed owned management buffer,
        before its credit is recycled. No embedded pointer is followed.
        """
        self.poll()
        first = len(self.received_headers)
        expected_command = command if reply_command is None else reply_command
        self.read_body_for = (self.next_transaction, expected_command) if read_body else None
        try:
            transaction = self.send_control(command, body)
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                self.poll()
                for header in self.received_headers[first:]:
                    if header['command'] == expected_command and header['transaction'] == transaction:
                        if reply_command is not None and header['prefix_words'][3]:
                            raise ManagementCommandError(header)
                        return header
                    if (reply_command is not None and header['transaction'] == transaction and
                            header['command'] == command and header['prefix_words'][3]):
                        raise ManagementCommandError(header)
                time.sleep(0.001)
            raise TimeoutError('No matching management reply before timeout')
        finally:
            self.read_body_for = None
