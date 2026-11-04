#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Python Centauri Bluetooth queue experiments and Linux VHCI transport.

For LLMs: experimental runtime-derived protocol. No firmware code is read.
Uses an owned or retained SID3 context; setup lives in the control module.
VHCI only connects our Python hardware driver to Linux's normal HCI stack.
"""
import argparse
from collections import deque
import json
import os
from pathlib import Path
import select
import struct
import time

from centauri_linux import LinuxMemory

BT_IPC_STATE = 0
BT_CONTEXT_LOW = 4
BT_CONTEXT_HIGH = 8
BT_FIRMWARE_STATE = 0x18
BT_QUEUE_DOORBELLS = 0x1000
BT_DEVICE_INDEX_VECTOR = 0x18
BT_HOST_INDEX_VECTOR = 0x28
BT_QUEUE_COUNTS = 0x30
DART_ERROR_STATUS = 0x100
INLINE_HEADER = struct.Struct('<IQI')
INLINE_POINTER = (1 << 64) - 1
INLINE_TRANSMIT = 2
HCI_COMMAND, HCI_ACL, HCI_EVENT = 1, 2, 4
HCI_EVENT_CONNECTION_COMPLETE = 3
HCI_LINK_TYPE_ACL = 1
N1_LINK_TYPE_ACL_F0 = 0xf0
N1_LINK_TYPE_ACL_F1 = 0xf1  # Both observed as ACL; role relationship remains under test.
COMMAND_QUEUE, ACL_TX_QUEUE, EVENT_QUEUE, ACL_RX_QUEUE = 1, 2, 7, 8
# Standard HCI commands and individually scoped runtime-derived vendor operations.
HCI_OP_CREATE_CONNECTION = 0x0405
HCI_OP_CANCEL_CREATE_CONNECTION = 0x0408
HCI_OP_DISCONNECT = 0x0406
HCI_OP_RESET = 0x0c03
HCI_OP_WRITE_SCAN_ENABLE = 0x0c1a
HCI_OP_READ_SCAN_ENABLE = 0x0c19
HCI_OP_CONTROLLER_TO_HOST_FLOW_CONTROL = 0x0c31
HCI_OP_HOST_BUFFER_SIZE = 0x0c33
HCI_OP_WRITE_SIMPLE_PAIRING_MODE = 0x0c56
HCI_OP_WRITE_LE_HOST_SUPPORT = 0x0c6d
HCI_OP_WRITE_SECURE_CONNECTIONS_HOST_SUPPORT = 0x0c7a
HCI_OP_READ_BUFFER_SIZE = 0x1005
HCI_OP_READ_BD_ADDR = 0x1009
HCI_OP_VENDOR_CLASSIC_CREATE_INHIBIT = 0xff77
HCI_OP_VENDOR_FACTORY_ADDRESS = 0xfc06
HCI_OP_VENDOR_RADIO_INITIALIZE = 0xff0b
HCI_OP_VENDOR_STARTUP_MASK_PROBE = HCI_OP_VENDOR_RADIO_INITIALIZE  # Legacy probe alias.
HCI_ACL_HEADER_BYTES = 4
AUXILIARY_RX_QUEUES = {9: (9, 724), 10: (17, 256), 11: (9, 268),
                       12: (128, 1024), 13: (17, 260)}
# Confirmed posted inline RX geometry; body semantics remain unknown.


def linux_hci_event(event, *, translate_vendor_acl=False):
    """Translate only the observed successful N1 ACL connection code.

    Other vendor values are not interpreted as bitfields. Failed connection
    events do not define usable link fields and are forwarded unchanged.
    """
    if len(event) != 13 or event[0] != HCI_EVENT_CONNECTION_COMPLETE or event[2]:
        return event
    link_type = event[11]
    if not translate_vendor_acl or link_type in (0, 1, 2):
        return event
    if link_type not in (N1_LINK_TYPE_ACL_F0, N1_LINK_TYPE_ACL_F1):
        raise ValueError(f'Unrecognized controller connection type {link_type:#x}')
    return event[:11] + bytes([HCI_LINK_TYPE_ACL]) + event[12:]


class ClassicCreateSetup:
    """Withhold Reset Complete until the owned FF77=0 setup completes.

    No unrelated HCI event or data packet is consumed by this state machine.
    Only the response to its own vendor command is hidden from the kernel.
    """
    def __init__(self):
        self.reset_reply = None
        self.sent = False
        self.started = None

    @property
    def pending(self):
        return self.reset_reply is not None

    def submit(self, transport):
        if not self.pending:
            return
        if time.monotonic() - self.started > 3:
            raise TimeoutError('Classic Create Connection setup did not complete')
        if self.sent:
            return
        previous = transport.vendor_probe_opcode
        transport.vendor_probe_opcode = HCI_OP_VENDOR_CLASSIC_CREATE_INHIBIT
        try:
            command = struct.pack('<HBB', HCI_OP_VENDOR_CLASSIC_CREATE_INHIBIT, 1, 0)
            self.sent = transport.send(HCI_COMMAND, command)
        finally:
            transport.vendor_probe_opcode = previous

    def receive(self, kind, packet):
        if kind != HCI_EVENT or len(packet) < 6:
            return [(kind, packet)]
        if (packet[0] == 0x0f and self.pending
                and struct.unpack_from('<H', packet, 4)[0] == HCI_OP_VENDOR_CLASSIC_CREATE_INHIBIT):
            if packet[2]:
                raise RuntimeError(f'Classic setup status {packet[2]:#x}')
            return []
        if packet[0] != 0x0e:
            return [(kind, packet)]
        opcode, = struct.unpack_from('<H', packet, 3)
        if opcode == HCI_OP_RESET and packet[5] == 0:
            if self.pending:
                raise RuntimeError('Overlapping controller resets')
            self.reset_reply, self.started = packet, time.monotonic()
            self.sent = False
            return []
        if opcode == HCI_OP_VENDOR_CLASSIC_CREATE_INHIBIT and self.pending:
            if not self.sent or packet[5]:
                raise RuntimeError(f'Classic setup completion status {packet[5]:#x}')
            reply, self.reset_reply = self.reset_reply, None
            self.sent, self.started = False, None
            print('BT_CLASSIC_CREATE_SETUP_OK', flush=True)
            return [(HCI_EVENT, reply)]
        return [(kind, packet)]


class BluetoothQueues:
    def __init__(self, memory, state, *, tag_base=0, accept_role=None, vendor_probe_opcode=None, bootstrap_only=False):
        if not 0 <= tag_base <= 0xffffffff - 128 or accept_role not in (None, 0, 1):
            raise ValueError('Invalid controlled probe value')
        self.tag_base, self.accept_role = tag_base, accept_role
        self.vendor_probe_opcode = vendor_probe_opcode
        self.header_shapes = {}
        self.probe_acl_start_pb = None
        self.acl_pb_changes = 0
        self.memory, self.state = memory, state
        self.bank = state['bluetooth_bar0_physical']
        self.context = state['bluetooth_context_iova']
        self.maps = sorted(state['streams']['3']['mappings'], key=lambda m: m['iova'])
        if hasattr(memory, 'validate_current_owner'):
            memory.validate_current_owner(state)
        elif memory.readmem(state['handoff_nonce_physical'], 32).hex() != state['handoff_nonce']:
            raise RuntimeError('Current boot nonce mismatch')
        self.ready()
        current = memory.read(self.bank + BT_CONTEXT_LOW, 32) | memory.read(self.bank + BT_CONTEXT_HIGH, 32) << 32
        if current != self.context:
            raise RuntimeError('Bluetooth context changed')
        counts, = struct.unpack('<I', self.read(self.context + BT_QUEUE_COUNTS, 4))
        if counts >> 16 != 22:
            raise ValueError('Unknown Bluetooth queue vector extent')
        self.device_indices, = struct.unpack('<Q', self.read(self.context + BT_DEVICE_INDEX_VECTOR, 8))
        self.host_indices, = struct.unpack('<Q', self.read(self.context + BT_HOST_INDEX_VECTOR, 8))
        self.queues = {}
        for record in state['bluetooth_queues']:
            number = record['number']
            auxiliary = number in AUXILIARY_RX_QUEUES and record.get('receive_only', False)
            if number not in (COMMAND_QUEUE, ACL_TX_QUEUE, EVENT_QUEUE, ACL_RX_QUEUE) and not auxiliary:
                continue
            capacity, entries = record['capacity'], record['entries']
            if not 2 <= entries <= 128 or not 0 < capacity <= 4096:
                raise ValueError('Unexpected queue geometry')
            if auxiliary:
                max_entries, known_capacity = AUXILIARY_RX_QUEUES[number]
                if capacity != known_capacity or entries > max_entries:
                    raise ValueError('Unknown auxiliary receive geometry')
            q = dict(number=number, entries=entries, capacity=capacity,
                     ring=record['ring_iova'], stride=INLINE_HEADER.size + capacity)
            q['cursor'] = self.index(self.device_indices, number)
            producer = self.index(self.host_indices, number)
            if not 0 <= producer < entries or not 0 <= q['cursor'] < entries:
                raise ValueError('Queue indices out of range')
            if number in (COMMAND_QUEUE, ACL_TX_QUEUE) and producer != q['cursor']:
                raise RuntimeError('Old transmitter still active')
            # Check only posted RX metadata; do not inspect pre-handoff bodies.
            if (number in (EVENT_QUEUE, ACL_RX_QUEUE) or auxiliary) and producer != q['cursor']:
                descriptor = INLINE_HEADER.unpack(self.read(q['ring'] + q['cursor'] * q['stride'], 16))
                if descriptor[1] != INLINE_POINTER or descriptor[0] >> 8 != capacity:
                    raise ValueError(f'Posted RX header does not confirm capacity for queue {number}')
            self.queues[number] = q
        required = {COMMAND_QUEUE, EVENT_QUEUE} if bootstrap_only else {COMMAND_QUEUE, ACL_TX_QUEUE, EVENT_QUEUE, ACL_RX_QUEUE}
        if not required.issubset(self.queues) or set(self.queues) - ({1, 2, 7, 8} | set(AUXILIARY_RX_QUEUES)):
            raise ValueError('Missing HCI queue pair')
        self.counters = dict(command_tx=0, acl_tx=0, event_rx=0, acl_rx=0, skipped_vendor=0, auxiliary_rx=0)
        self.auxiliary_counts = {n: 0 for n in AUXILIARY_RX_QUEUES if n in self.queues}
        # Drop old completed packets unread, fill free credits only.
        for number in (EVENT_QUEUE, ACL_RX_QUEUE, *AUXILIARY_RX_QUEUES):
            if number not in self.queues:
                continue
            q = self.queues[number]
            producer = self.index(self.host_indices, number)
            while self.next(q, producer) != q['cursor']:
                self.post_rx(q, producer)
                producer = self.next(q, producer)
            self.publish(q, producer)

    def ready(self):
        ipc = self.memory.read(self.bank + BT_IPC_STATE, 32)
        firmware = self.memory.read(self.bank + BT_FIRMWARE_STATE, 32)
        dart_error = self.memory.read(self.state['dart_base'] + DART_ERROR_STATUS, 32)
        if ipc != 2 or firmware != 2 or dart_error:
            raise RuntimeError(f'Bluetooth not ready: IPC={ipc} firmware={firmware} DART={dart_error:#x}')

    def chunks(self, iova, length):
        end = iova + length
        if length <= 0 or length > 4096:
            raise ValueError('Invalid typed DMA access')
        for m in self.maps:
            stop = m['iova'] + m['length']
            if m['iova'] <= iova < stop:
                count = min(end, stop) - iova
                yield m['physical'] + iova - m['iova'], count
                iova += count
                if iova == end:
                    return
        raise ValueError('Unmapped Bluetooth DMA extent')

    def read(self, iova, length):
        return b''.join(self.memory.readmem(p, n) for p, n in self.chunks(iova, length))

    def write(self, iova, data):
        offset = 0
        for physical, count in self.chunks(iova, len(data)):
            self.memory.writemem(physical, data[offset:offset + count])
            offset += count

    def index(self, vector, number):
        return struct.unpack('<H', self.read(vector + number * 2, 2))[0]

    @staticmethod
    def next(q, index):
        return (index + 1) % q['entries']

    def publish(self, q, producer):
        self.write(self.host_indices + q['number'] * 2, struct.pack('<H', producer))
        self.memory.write(self.bank + BT_QUEUE_DOORBELLS + 4 * q['number'], producer, 32)

    def post_rx(self, q, slot):
        self.write(q['ring'] + slot * q['stride'],
                   INLINE_HEADER.pack(q['capacity'] << 8, INLINE_POINTER, self.tag_base + slot))

    def send(self, packet_type, body, *, descriptor_flags=INLINE_TRANSMIT, cookie=None):
        self.ready()
        pb_changed = False
        if packet_type == HCI_COMMAND:
            if len(body) < 3 or body[2] + 3 != len(body):
                raise ValueError('Invalid HCI command extent')
            opcode, = struct.unpack_from('<H', body)
            if opcode >> 10 == 0x3f and opcode != self.vendor_probe_opcode:
                raise ValueError('Vendor commands require a specific owned-data probe opcode')
            if opcode == 0x0409:
                original = body[-1]
                if self.accept_role is not None:
                    body = body[:-1] + bytes([self.accept_role])
                print('BT_ACCEPT_ROLE', dict(requested=original, submitted=body[-1]), flush=True)
            number, counter = COMMAND_QUEUE, 'command_tx'
        elif packet_type == HCI_ACL:
            if len(body) < 4 or struct.unpack_from('<H', body, 2)[0] + 4 != len(body):
                raise ValueError('Invalid ACL extent')
            if self.probe_acl_start_pb is not None:
                handle, = struct.unpack_from('<H', body)
                old_pb = handle >> 12 & 3
                if old_pb in (0, 2) and old_pb != self.probe_acl_start_pb:
                    body = struct.pack('<H', (handle & ~0x3000) | self.probe_acl_start_pb << 12) + body[2:]
                    pb_changed = True
            number, counter = ACL_TX_QUEUE, 'acl_tx'
        else:
            raise ValueError('Unsupported HCI packet type')
        q = self.queues[number]
        if len(body) > q['capacity']:
            raise ValueError('Payload exceeds queue capacity')
        producer = self.index(self.host_indices, number)
        consumer = self.index(self.device_indices, number)
        if not 0 <= producer < q['entries'] or not 0 <= consumer < q['entries']:
            raise RuntimeError('TX cursor out of bounds')
        if self.next(q, producer) == consumer:
            return False
        header = INLINE_HEADER.pack(len(body) << 8 | descriptor_flags,
                                    INLINE_POINTER, self.tag_base + producer if cookie is None else cookie)
        self.write(q['ring'] + producer * q['stride'], header + body)
        self.publish(q, self.next(q, producer))
        self.counters[counter] += 1
        self.acl_pb_changes += int(pb_changed)
        if packet_type == HCI_COMMAND:
            print('BT_COMMAND', dict(opcode=hex(opcode), size=len(body)), flush=True)
        return True

    def poll(self, budget=64):
        self.ready()
        result = []
        streams = [(EVENT_QUEUE, HCI_EVENT, 'event_rx'), (ACL_RX_QUEUE, HCI_ACL, 'acl_rx')]
        streams.extend((n, 0, 'auxiliary_rx') for n in AUXILIARY_RX_QUEUES)
        for number, packet_type, counter in streams:
            if number not in self.queues:
                continue
            q = self.queues[number]
            consumer = self.index(self.device_indices, number)
            producer = self.index(self.host_indices, number)
            cursor = q['cursor']
            if not 0 <= consumer < q['entries'] or not 0 <= producer < q['entries']:
                raise RuntimeError('RX cursor out of bounds')
            received = 0
            while cursor != consumer and received < budget:
                address = q['ring'] + cursor * q['stride']
                flags, pointer, cookie = INLINE_HEADER.unpack(self.read(address, 16))
                shape = (number, flags & 255, cookie == cursor, cookie - cursor)
                if shape not in self.header_shapes:
                    print('BT_RX_HEADER_SHAPE', dict(queue=number, low_flags=flags & 255, tag_matches_slot=cookie == cursor, tag_minus_slot=cookie - cursor), flush=True)
                self.header_shapes[shape] = self.header_shapes.get(shape, 0) + 1
                length = flags >> 8
                if pointer != INLINE_POINTER or not (0 if packet_type == 0 else 2) <= length <= q['capacity']:
                    raise RuntimeError('Invalid completed inline header')
                prefix = self.read(address + 16, min(length, 4)) if packet_type else b''
                if packet_type == 0:
                    # Maintain confirmed inline RX credits, without examining
                    # any unknown auxiliary body or forwarding it as HCI.
                    self.counters[counter] += 1
                    self.auxiliary_counts[number] += 1
                elif packet_type == HCI_EVENT and prefix[0] == 0xff:
                    # Only the already-read two-byte event discriminator and
                    # extent are observed; the remaining vendor body stays unread.
                    shape = ('vendor', length, tuple(prefix[2:]))
                    if shape not in self.header_shapes and len(self.header_shapes) < 64:
                        print('BT_VENDOR_EVENT_HEADER', dict(length=length, discriminator=list(prefix[2:])), flush=True)
                    if shape in self.header_shapes or len(self.header_shapes) < 64:
                        self.header_shapes[shape] = self.header_shapes.get(shape, 0) + 1
                    self.counters['skipped_vendor'] += 1
                else:
                    if packet_type == HCI_EVENT:
                        valid = prefix[1] + 2 == length
                    else:
                        valid = length >= 4 and struct.unpack_from('<H', prefix, 2)[0] + 4 == length
                    if not valid:
                        raise RuntimeError('Standard HCI frame extent mismatch')
                    packet = self.read(address + 16, length)
                    if packet_type == HCI_EVENT:
                        details = dict(event=hex(packet[0]), size=length)
                        if packet[0] == 0x0f and length == 6:
                            details.update(opcode=hex(struct.unpack_from('<H', packet, 4)[0]), status=packet[2], command_credits=packet[3])
                        if packet[0] == 0x0e and length >= 6:
                            details.update(opcode=hex(struct.unpack_from('<H', packet, 3)[0]), status=packet[5])
                        if packet[0] == 3 and length == 13:
                            details.update(status=packet[2], handle=struct.unpack_from('<H', packet, 3)[0], link_type=packet[11], encryption=packet[12])
                        elif packet[0] == 0x12 and length == 10:
                            details.update(status=packet[2], role=packet[9])
                        elif packet[0] == 5 and length == 6:
                            details.update(status=packet[2], handle=struct.unpack_from('<H', packet, 3)[0], reason=packet[5])
                        print('BT_EVENT', details, flush=True)
                    result.append((packet_type, packet))
                    self.counters[counter] += 1
                if self.next(q, producer) == consumer:
                    raise RuntimeError('No free RX slot after completion')
                self.post_rx(q, producer)
                producer = self.next(q, producer)
                cursor = self.next(q, cursor)
                received += 1
            if cursor != q['cursor']:
                self.publish(q, producer)
                q['cursor'] = cursor
        return result

    def snapshot(self):
        return dict(self.counters, probe_acl_start_pb=self.probe_acl_start_pb, acl_pb_changes=self.acl_pb_changes, auxiliary_by_queue=dict(self.auxiliary_counts),
                    all_host_indices=list(struct.unpack('<22H', self.read(self.host_indices, 44))),
                    all_device_indices=list(struct.unpack('<22H', self.read(self.device_indices, 44))), firmware=self.memory.read(self.bank + BT_FIRMWARE_STATE, 32),
                    ipc=self.memory.read(self.bank + BT_IPC_STATE, 32),
                    dart_error=self.memory.read(self.state['dart_base'] + DART_ERROR_STATUS, 32), queues={n: dict(host=self.index(self.host_indices, n),
                    device=self.index(self.device_indices, n), cursor=q['cursor'],
                    capacity=q['capacity'], stride=q['stride']) for n, q in self.queues.items()})


def probe(transport, rounds=1):
    for opcode in (0x1001, 0x1009, 0x1002, 0x1003, 0x1005) * rounds:
        if not transport.send(HCI_COMMAND, struct.pack('<HB', opcode, 0)):
            raise RuntimeError('Command queue unexpectedly full')
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            events = transport.poll()
            found = next((body for kind, body in events if kind == HCI_EVENT and body[0] == 0x0e
                          and len(body) >= 6 and struct.unpack_from('<H', body, 3)[0] == opcode), None)
            if found:
                if found[5] != 0:
                    raise RuntimeError(f'Standard query {opcode:#x} failed: {found[5]}')
                # Public standard controller identity, not firmware/program data.
                if opcode == 0x1009 and len(found) == 12:
                    print('BT_CONTROLLER_ADDRESS', ':'.join(f'{x:02X}' for x in found[6:][::-1]), flush=True)
                break
            time.sleep(0.005)
        else:
            raise TimeoutError(f'No completion for {opcode:#x}')
    print('BT_STANDARD_PROBES_OK', transport.snapshot(), flush=True)



def command_complete(transport, opcode, parameters=b'', timeout=3):
    """Exchange one generated HCI command outside an active kernel HCI owner."""
    if len(parameters) > 255:
        raise ValueError('HCI command parameters exceed the standard extent')
    if not transport.send(HCI_COMMAND, struct.pack('<HB', opcode, len(parameters)) + parameters):
        raise RuntimeError('HCI command queue is full')
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for kind, packet in transport.poll():
            if kind == HCI_EVENT and packet[0] == 0x0e and len(packet) >= 6 and struct.unpack_from('<H', packet, 3)[0] == opcode:
                if packet[5]:
                    raise RuntimeError(f'HCI {opcode:#x} status {packet[5]:#x}')
                return packet[6:]
        time.sleep(.005)
    raise TimeoutError(f'No command complete for {opcode:#x}')


def probe_radio_initialize(transport):
    """Probe FF0B with no HCI parameters, outside a kernel HCI owner.

    A function-reset trial passed page scan and outgoing creation with this
    empty command. Zero the inactive slot so undeclared bytes are deterministic.
    This does not claim whole-chip firmware initialization.
    """
    if transport.vendor_probe_opcode is not None:
        raise RuntimeError('Another vendor experiment is active')
    queue = transport.queues[COMMAND_QUEUE]
    producer = transport.index(transport.host_indices, COMMAND_QUEUE)
    consumer = transport.index(transport.device_indices, COMMAND_QUEUE)
    if producer != consumer or not 0 <= producer < queue['entries']:
        raise RuntimeError('Radio initialization requires a drained command queue')
    transport.write(queue['ring'] + producer * queue['stride'] + INLINE_HEADER.size,
                    bytes(queue['capacity']))
    transport.vendor_probe_opcode = HCI_OP_VENDOR_RADIO_INITIALIZE
    try:
        reply = command_complete(transport, HCI_OP_VENDOR_RADIO_INITIALIZE)
    finally:
        transport.vendor_probe_opcode = None
    print('BT_RADIO_INITIALIZE_PROBE', dict(parameter_bytes=0, reply_bytes=len(reply)), flush=True)
    return len(reply)


def probe_ff0b_mask(transport, mask):
    """Controlled 128-bit mask hypothesis; the opcode's meaning is unproven.

    The runtime startup message is uniformly all ones. Generate the chosen
    logical mask rather than retaining/replaying a captured parameter blob.
    This is a probe only and is never sent by default initialization.
    """
    if not isinstance(mask, int) or not 0 <= mask < 1 << 128:
        raise ValueError('Mask probe must fit the observed 16-byte parameter extent')
    previous = transport.vendor_probe_opcode
    if previous is not None:
        raise RuntimeError('Another vendor experiment is active')
    transport.vendor_probe_opcode = HCI_OP_VENDOR_STARTUP_MASK_PROBE
    try:
        reply = command_complete(transport, HCI_OP_VENDOR_STARTUP_MASK_PROBE, mask.to_bytes(16, 'little'))
    finally:
        transport.vendor_probe_opcode = previous
    print('BT_FF0B_MASK_PROBE', dict(set_bits=mask.bit_count(), reply_bytes=len(reply)), flush=True)
    return len(reply)


def probe_host_buffers(transport, *, include_acl_header=True, disable_flow_control=False):
    """Generate the standard host-buffer announcement outside a kernel owner.

    The reference announces Read Buffer Size's ACL MTU plus the four-byte HCI
    ACL header. This is an explicit N1 experiment, not a changed definition of
    the standard MTU field. No reference packet body is replayed. Flow control
    stays disabled because this transport does not send Host Completed Packets.
    """
    raw = command_complete(transport, HCI_OP_READ_BUFFER_SIZE)
    if len(raw) != 7:
        raise ValueError('Unknown standard Read Buffer Size return extent')
    acl_data_mtu, controller_sco_mtu, controller_acl_packets, controller_sco_packets = struct.unpack('<HBHH', raw)
    host_acl_mtu = acl_data_mtu + (HCI_ACL_HEADER_BYTES if include_acl_header else 0)
    if not 0 < host_acl_mtu <= transport.queues[ACL_RX_QUEUE]['capacity']:
        raise ValueError('Host buffer announcement exceeds the retained ACL RX slot')
    # Full byte-sized SCO length, one buffer of each kind: named standard
    # parameter fields observed in the reference, not an opaque byte string.
    host_sco_mtu, host_acl_packets, host_sco_packets = 255, 1, 1
    parameters = struct.pack('<HBHH', host_acl_mtu, host_sco_mtu,
                             host_acl_packets, host_sco_packets)
    command_complete(transport, HCI_OP_HOST_BUFFER_SIZE, parameters)
    if disable_flow_control:
        command_complete(transport, HCI_OP_CONTROLLER_TO_HOST_FLOW_CONTROL, bytes([0]))
    fields = dict(controller_acl_mtu=acl_data_mtu,
                  controller_sco_mtu=controller_sco_mtu,
                  controller_acl_packets=controller_acl_packets,
                  controller_sco_packets=controller_sco_packets,
                  host_acl_mtu=host_acl_mtu, host_sco_mtu=host_sco_mtu,
                  host_acl_packets=host_acl_packets, host_sco_packets=host_sco_packets,
                  explicit_flow_control_disable=disable_flow_control)
    print('BT_STANDARD_HOST_BUFFER_PROBE', fields, flush=True)
    return fields


def probe_host_support(transport, *, simple_pairing=True, le_host=True,
                       simultaneous_le_host=True, secure_connections=True):
    """Generate named standard host-support fields for controlled bring-up.

    This does not infer which fields are necessary. Isolate the booleans on
    hardware before turning a successful compound experiment into a default.
    """
    fields = dict(simple_pairing=bool(simple_pairing), le_host=bool(le_host),
                  simultaneous_le_host=bool(simultaneous_le_host),
                  secure_connections=bool(secure_connections))
    command_complete(transport, HCI_OP_WRITE_SIMPLE_PAIRING_MODE,
                     bytes([fields['simple_pairing']]))
    command_complete(transport, HCI_OP_WRITE_LE_HOST_SUPPORT,
                     bytes([fields['le_host'], fields['simultaneous_le_host']]))
    command_complete(transport, HCI_OP_WRITE_SECURE_CONNECTIONS_HOST_SUPPORT,
                     bytes([fields['secure_connections']]))
    print('BT_STANDARD_HOST_SUPPORT_PROBE', fields, flush=True)
    return fields


def probe_create_connection(transport, peer, *, packet_types=0xcc18,
                            page_scan_repetition=2, page_scan_mode=0, clock_offset=0,
                            allow_role_switch=1, timeout=8, read_role=False):
    """Bounded standard Classic page probe, without a kernel HCI owner.

    Only command status and the selected peer's connection metadata are
    retained. Any successful temporary connection is explicitly disconnected.
    """
    address = bytes.fromhex(peer.replace(':', ''))
    if len(address) != 6 or not 0 <= packet_types <= 65535 or page_scan_repetition not in (0, 1, 2) or page_scan_mode not in (0, 1, 2, 3) or not 0 <= clock_offset <= 65535 or allow_role_switch not in (0, 1) or not 0 < timeout <= 30:
        raise ValueError('Invalid controlled connection field')
    params = struct.pack('<6sHBBHB', address[::-1], packet_types,
                         page_scan_repetition, page_scan_mode, clock_offset, allow_role_switch)
    result = dict(peer=peer, packet_types=packet_types, page_scan_repetition=page_scan_repetition,
                  page_scan_mode=page_scan_mode, clock_offset=clock_offset, allow_role_switch=allow_role_switch,
                  command_status=None, connection_complete=None)
    if not transport.send(HCI_COMMAND, struct.pack('<HB', HCI_OP_CREATE_CONNECTION, len(params)) + params):
        raise RuntimeError('No command credit for connection probe')
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        for kind, event in transport.poll():
            if kind != HCI_EVENT:
                continue
            if len(event) == 6 and event[0] == 0x0f and struct.unpack_from('<H', event, 4)[0] == HCI_OP_CREATE_CONNECTION:
                result['command_status'] = event[2]
                if event[2]:
                    return result
            if len(event) == 13 and event[0] == HCI_EVENT_CONNECTION_COMPLETE and event[5:11] == address[::-1]:
                handle, = struct.unpack_from('<H', event, 3)
                result['connection_complete'] = dict(status=event[2], handle=handle,
                    controller_link_type=event[11], encryption=event[12])
                if not event[2]:
                    if read_role:
                        try:
                            role = command_complete(transport, 0x0809, struct.pack('<H', handle))
                            role_handle, role_value = struct.unpack('<HB', role)
                            result['role_discovery'] = dict(handle=role_handle, role=role_value)
                        except (RuntimeError, TimeoutError, struct.error) as error:
                            result['role_discovery_error'] = str(error)
                    payload = struct.pack('<HBHB', HCI_OP_DISCONNECT, 3, handle, 0x13)
                    if not transport.send(HCI_COMMAND, payload):
                        raise RuntimeError('No command credit to close probe connection')
                    drain_end = time.monotonic() + 3
                    while time.monotonic() < drain_end:
                        received = transport.poll()
                        if any(k == HCI_EVENT and len(e) == 6 and e[0] == 5 and struct.unpack_from('<H', e, 3)[0] == handle for k, e in received):
                            result['disconnected'] = True
                            return result
                        time.sleep(.005)
                return result
        time.sleep(.005)
    result['timed_out'] = True
    if result['command_status'] == 0:
        command_complete(transport, HCI_OP_CANCEL_CREATE_CONNECTION, address[::-1])
        result['cancelled'] = True
    return result


def probe_fe44_zero_record(transport):
    """Probe only a generated zero 24-byte reference-startup command.

    No captured FE44 body is retained or replayed; its field roles are unknown.
    Never print or interpret any vendor return body.
    """
    previous = transport.vendor_probe_opcode
    transport.vendor_probe_opcode = 0xfe44
    try:
        reply = command_complete(transport, 0xfe44, bytes(24))
    finally:
        transport.vendor_probe_opcode = previous
    print('BT_FE44_ZERO_RECORD_PROBE', dict(parameter_bytes=24, reply_bytes=len(reply)), flush=True)


def probe_factory_address(transport, factory_address):
    """Test FC06 with the authorized device's own address and standard readback.

    The vendor opcode is from runtime control traffic. Unlike standard readback,
    its address parameter uses display/network order. Its six-byte value is
    generated from typed device identity; no vendor command body is replayed.
    """
    address = bytes.fromhex(factory_address.replace(':', ''))
    if len(address) != 6 or address in (bytes(6), bytes([255]) * 6):
        raise ValueError('Invalid own controller address')
    before = command_complete(transport, HCI_OP_READ_BD_ADDR)
    transport.vendor_probe_opcode = HCI_OP_VENDOR_FACTORY_ADDRESS
    try:
        command_complete(transport, HCI_OP_VENDOR_FACTORY_ADDRESS, address)
    finally:
        transport.vendor_probe_opcode = None
    after = command_complete(transport, HCI_OP_READ_BD_ADDR)
    if after != address[::-1]:
        raise RuntimeError('FC06 did not program the selected address')
    print('BT_FACTORY_ADDRESS_PROBE_OK', dict(before_zero=before == bytes(6), matches_factory=True), flush=True)
    return after

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('handoff', type=Path)
    parser.add_argument('--library', type=Path, default=Path(__file__).with_name('centauri_native.so'))
    parser.add_argument('--probe-only', action='store_true')
    parser.add_argument('--probe-control', type=Path,
                        help='Owned JSON probe file: acl_start_pb=null, 0 or 2; never alters continuation fragments')
    parser.add_argument('--probe-rounds', type=int, choices=range(1, 101), default=1)
    parser.add_argument('--probe-tag-base', type=lambda value: int(value, 0), default=0,
                        help='Controlled TX/RX header tag variation, without changing ring indices')
    parser.add_argument('--probe-accept-role', type=int, choices=(0, 1),
                        help='Controlled standard Accept Connection Request role variation')
    parser.add_argument('--no-classic-create-setup', action='store_true',
                        help='Probe only: leave standard outgoing creation inhibited after reset')
    parser.add_argument('--probe-startup-mask', type=lambda value: int(value, 0),
                        help='Before VHCI registration, probe FF0B with a generated 128-bit value')
    parser.add_argument('--probe-radio-initialize', action='store_true',
                        help='Before VHCI registration, probe FF0B without HCI parameters')
    parser.add_argument('--translate-vendor-acl-link-type', '--probe-link-type-low-nibble',
                        dest='translate_vendor_acl', action='store_true',
                        help='Translate observed N1 ACL codes 0xf0/0xf1 to Linux ACL type 1')
    args = parser.parse_args()
    if args.probe_radio_initialize and args.probe_startup_mask is not None:
        parser.error('Choose only one startup-command experiment')
    state = json.loads(args.handoff.read_text())
    if os.uname().machine != 'aarch64':
        raise RuntimeError('This transport requires an arm64 device')
    # LinuxMemory's physical guard includes the selected PF2 BAR resource.
    state = dict(state, bar0_physical=state['bluetooth_bar0_physical'])
    memory = LinuxMemory(state, args.library.resolve())
    transport = BluetoothQueues(memory, state, tag_base=args.probe_tag_base, accept_role=args.probe_accept_role)
    print('BT_PYTHON_OWNED', transport.snapshot(), flush=True)
    if args.probe_radio_initialize:
        probe_radio_initialize(transport)
    if args.probe_startup_mask is not None:
        probe_ff0b_mask(transport, args.probe_startup_mask)
    if args.probe_only:
        probe(transport, args.probe_rounds)
        return
    fd = os.open('/dev/vhci', os.O_RDWR | os.O_NONBLOCK | os.O_CLOEXEC)
    os.write(fd, b'\xff\x00')  # Linux VHCI UAPI: create a primary HCI controller.
    pending, last = deque(), time.monotonic()
    reset_setup = None if args.no_classic_create_setup else ClassicCreateSetup()
    probe_checked, probe_value = 0, None
    while True:
        if args.probe_control and time.monotonic() - probe_checked >= .25:
            probe_checked = time.monotonic()
            if args.probe_control.exists():
                with args.probe_control.open('rb') as source:
                    raw = source.read(257)
                if len(raw) > 256:
                    raise ValueError('Oversized owned probe control')
                config = json.loads(raw)
                if set(config) != {'acl_start_pb'} or config['acl_start_pb'] not in (None, 0, 2):
                    raise ValueError('Unknown owned probe control')
                value = config['acl_start_pb']
                if value != probe_value:
                    print('BT_CONTROLLED_ACL_PB_PROBE', dict(previous=probe_value, submitted=value), flush=True)
                    transport.probe_acl_start_pb = probe_value = value
        for _ in range(8):
            if not select.select([fd], [], [], 0)[0]:
                break
            packet = os.read(fd, 4096)
            if packet[0] == 0xff:
                print('BT_VHCI_CREATED', dict(index=struct.unpack_from('<H', packet, 2)[0]), flush=True)
            else:
                pending.append(packet)
        if reset_setup and reset_setup.pending:
            reset_setup.submit(transport)
        else:
            while pending:
                if not transport.send(pending[0][0], pending[0][1:]):
                    break
                pending.popleft()
        for received_kind, received_body in transport.poll():
            forwarded = reset_setup.receive(received_kind, received_body) if reset_setup else [(received_kind, received_body)]
            for packet_type, body in forwarded:
                if packet_type == HCI_EVENT:
                    if len(body) == 13 and body[0] == HCI_EVENT_CONNECTION_COMPLETE and not body[2] and body[11] not in (0, 1, 2):
                        print('BT_VENDOR_CONNECTION_TYPE', dict(raw=body[11], applied=args.translate_vendor_acl), flush=True)
                    body = linux_hci_event(body, translate_vendor_acl=args.translate_vendor_acl)
                packet = bytes([packet_type]) + body
                if os.write(fd, packet) != len(packet):
                    raise RuntimeError('Short VHCI packet write')
        if time.monotonic() - last >= 10:
            print('BT_PYTHON_PROGRESS', transport.snapshot(), flush=True)
            last = time.monotonic()
        time.sleep(0.005)


if __name__ == '__main__':
    main()
