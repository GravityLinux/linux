# SPDX-License-Identifier: GPL-2.0-only
"""Adopt registered Ethernet queues after the reference guest has exited.

Firmware, association, rings and receive buffers remain reference-prepared.
Transmit packets use the control allocator's owned arena. Existing mappings,
queue registrations and IPC state are preserved. Receive recycling can be
enabled after the former owner's pending completions have been drained.
"""
import json
import struct
import time

from centauri_regs import (WIFI_DMA_WINDOW_BASE_LOW, WIFI_DMA_WINDOW_BASE_HIGH,
                           WIFI_DMA_WINDOW_LENGTH,
                           WIFI_DEVICE_COMPLETION_INDICES as DEVICE_COMPLETION_INDICES,
                           WIFI_DEVICE_SUBMISSION_INDICES as DEVICE_SUBMISSION_INDICES,
                           WIFI_HOST_COMPLETION_INDICES as HOST_COMPLETION_INDICES,
                           WIFI_HOST_SUBMISSION_INDICES as HOST_SUBMISSION_INDICES)

RX_DESCRIPTOR = struct.Struct('<IQI')
TX_DESCRIPTOR = struct.Struct('<IQI4I')
COMPLETION = struct.Struct('<8I')
DATA_FORMAT = 3
RX_COMPLETION_FORMAT = 0x403
TX_COMPLETION_FORMAT = 0x502
TX_NOTIFY = 0x1030
RX_NOTIFY = 0x1050
RX_PADDING = 2
PACKET_CAPACITY = 2048
TX_AREA = 0x20000
TX_SLOT_BYTES = 0x1000
TX_SLOTS = 64
OBSERVED_TX_METADATA = (0x20000, 0, 0, 0)


def tx_descriptor(length, iova, cookie):
    """Encode the observed data descriptor; metadata word 0 remains unassigned."""
    if not 14 <= length <= PACKET_CAPACITY or not 0 < iova < 1 << 64 or not 0 <= cookie < 2048:
        raise ValueError('Unsupported data transmit fields')
    return TX_DESCRIPTOR.pack(length << 8 | DATA_FORMAT, iova, cookie, *OBSERVED_TX_METADATA)


def decode_completion(raw):
    """Retain all extended metadata and distinguish transmit from receive."""
    if len(raw) != COMPLETION.size:
        raise ValueError('Extended completions occupy 32 bytes')
    words = COMPLETION.unpack(raw)
    kind = words[0] & 0xffff
    if kind not in (RX_COMPLETION_FORMAT, TX_COMPLETION_FORMAT):
        raise ValueError('Unknown data completion format')
    return dict(source=words[0] >> 16, cookie=words[1] & 0xffff,
                bytes_received=words[1] >> 16 if kind == RX_COMPLETION_FORMAT else None,
                format=kind, words=words)


def receive_buffers(raw):
    """Index retained allocations by cookie, including a recycled ring.

    Cookie 1 was completed after reposting in slot 2047 on target-77.
    Retired slots can duplicate a record; a cookie must still identify one
    immutable allocation. Unused receive slots are zero.
    """
    if len(raw) != 2048 * RX_DESCRIPTOR.size:
        raise ValueError('Receive submission ring must have 2048 entries')
    records = {}
    for offset in range(0, len(raw), RX_DESCRIPTOR.size):
        record = raw[offset:offset + RX_DESCRIPTOR.size]
        flags, pointer, tag = RX_DESCRIPTOR.unpack(record)
        if not any(record):
            continue
        cookie = tag & 0xffff
        if flags & 255 != DATA_FORMAT or not RX_PADDING <= flags >> 8 <= PACKET_CAPACITY or not pointer or cookie >= 2048:
            raise ValueError('Unknown receive buffer descriptor')
        if cookie in records and records[cookie] != record:
            raise ValueError('One receive cookie identifies different allocations')
        records[cookie] = record
    return records


def latest_queue_registry(path):
    """Load only queue metadata after the last observed context publication.

    Teardown or unknown admin records invalidate this bounded prototype's
    snapshot. Do not attach a stale context after a reference-driver restart.
    """
    context, configs = None, {}
    invalid = False
    with open(path) as source:
        for line in source:
            event = json.loads(line)
            if event['kind'] == 'wifi_initial_context_metadata':
                context, configs, invalid = int(event['iova'], 0), {}, False
            elif event['kind'] == 'wifi_admin_control_frame' and context is not None:
                words = tuple(int(value, 0) for value in event.get('words', ()))
                if event['length'] not in (44, 52, 54, 62) or not words or words[0] & 0xffff not in (1, 2):
                    invalid = True
                    continue
                configs[words[0] & 0xffff, words[1] & 0xffff] = words
    if context is None or invalid:
        raise ValueError('A complete current queue configuration snapshot is required')
    return context, configs


class EthernetQueues:
    """Bare-proxy packet I/O using retained registered queues and owned TX data.

    The previous queue owner must have fully exited. The supplied context and
    configuration snapshot must describe the current publication. No firmware
    payloads or arbitrary diagnostic pointers are read.
    """

    def __init__(self, control, context, configs, station_mac):
        if context != control.iova:
            raise RuntimeError('Captured queues belong to a different IPC context')
        if len(station_mac) != 6 or station_mac[0] & 1:
            raise ValueError('A configured unicast station MAC is required')
        self.control, self.u, self.p = control, control.u, control.p
        self.context, self.configs, self.mac = context, configs, station_mac
        self.window = self.u.read(control.bank + WIFI_DMA_WINDOW_BASE_LOW, 32) | self.u.read(control.bank + WIFI_DMA_WINDOW_BASE_HIGH, 32) << 32
        self.window_end = self.window + self.u.read(control.bank + WIFI_DMA_WINDOW_LENGTH, 32)
        self.rx_cqs, self.rx_descriptors, self.rx_rings = {}, {}, {}
        for (kind, number), words in configs.items():
            if kind == 2 and words[4] >> 16 in (12, 13) and words[5] == 0xb:
                if words[4] & 0xffff != 4096 or words[1] >> 16 >= 234:
                    raise ValueError('Unknown receive completion geometry')
                self.rx_cqs[number] = dict(ring=words[2] | words[3] << 32,
                    entries=words[4] & 0xffff, cursor_index=words[1] >> 16,
                    source=words[4] >> 16, ethernet=words[4] >> 16 == 12 and words[11] == 0)
        if not self.rx_cqs:
            raise ValueError('No recognized receive completion profile')
        for number in (12, 13):
            words = configs[1, number]
            if words[6] & 0xffff != 2048 or words[7] != 0x228000b:
                raise ValueError('Unknown receive submission profile')
            self.rx_descriptors[number] = receive_buffers(self.read(words[2] | words[3] << 32, 2048 * RX_DESCRIPTOR.size))
            self.rx_rings[number] = words[2] | words[3] << 32
        candidates = [(number, words) for (kind, number), words in configs.items()
                      if kind == 1 and words[0] == 0x40001 and words[7] == 0x80012 and words[13:] == (0, 0)]
        if len(candidates) != 1:
            raise ValueError('A single observed default transmit queue is required')
        self.tx_queue, words = candidates[0]
        if self.tx_queue >= 192 or words[6] & 0xffff != 2048:
            raise ValueError('Unknown transmit ring depth')
        self.tx_ring = words[2] | words[3] << 32
        words = configs[2, words[6] >> 16]
        if words[0] != 0x40002 or words[4] != 0xffff2000 or words[5] != 0x12:
            raise ValueError('Unknown transmit completion profile')
        self.tx_completion = words[2] | words[3] << 32
        self.tx_cursor_index = words[1] >> 16
        if self.tx_cursor_index >= 234:
            raise ValueError('Transmit completion index exceeds the context')
        self.rx_seen, self.tx_seen = 0, []
        self.recycle_enabled = False
        self.rx_recycled = {12: 0, 13: 0}
        self.rx_reposted_inflight = {12: {}, 13: {}}
        self.rx_repost_completions = []
        control._require_ready()

    def enable_recycling(self):
        """Recycle future completions after draining the old owner's backlog.

        Buffer cookies identify retained allocations independently of the
        submission slot. Their original pointer and capacity remain fixed.
        """
        self.control._require_ready()
        self.recycle_enabled = True

    def _repost_rx(self, source, cookies):
        if not cookies:
            return
        producer = self.index(HOST_SUBMISSION_INDICES, source)
        consumed = self.index(DEVICE_SUBMISSION_INDICES, source)
        if producer >= 2048 or consumed >= 2048:
            raise RuntimeError('Receive submission cursor exceeds its ring')
        free = (consumed - producer - 1) % 2048
        if len(cookies) > free or len(set(cookies)) != len(cookies):
            raise RuntimeError('Receive recycling exceeds freed descriptors')
        for cookie in cookies:
            if cookie in self.rx_reposted_inflight[source]:
                raise RuntimeError('Receive buffer is already posted')
            record = self.rx_descriptors[source][cookie]
            self.write(self.rx_rings[source] + producer * RX_DESCRIPTOR.size, record)
            self.rx_reposted_inflight[source][cookie] = producer
            producer = (producer + 1) % 2048
        self.set_index(HOST_SUBMISSION_INDICES, source, producer)
        self.rx_recycled[source] += len(cookies)

    def _regions(self, iova, size):
        if not 0 < size <= 0x10000 or iova < self.window or iova + size > self.window_end:
            raise ValueError('DMA extent exceeds the published host window')
        regions = self.control.dart.iotranslate(2, iova, size)
        lower = self.u.ba.phys_base & ~0xffffffff
        if sum(n for _, n in regions) != size or any(pa is None or pa < lower or pa + n > lower + self.u.ba.mem_size_actual for pa, n in regions):
            raise ValueError('Packet or metadata mapping is outside host DRAM')
        return regions

    def read(self, iova, size):
        parts = []
        for physical, length in self._regions(iova, size):
            self.p.dc_ivac(physical, length)
            parts.append(self.u.iface.readmem(physical, length))
        return b''.join(parts)

    def write(self, iova, data):
        offset = 0
        for physical, length in self._regions(iova, len(data)):
            self.p.dc_ivac(physical, length)
            self.u.iface.writemem(physical, data[offset:offset + length])
            self.p.dc_cvac(physical, length)
            offset += length

    def index(self, array, number):
        return struct.unpack('<H', self.read(self.context + array + number * 2, 2))[0]

    def set_index(self, array, number, value):
        self.write(self.context + array + number * 2, struct.pack('<H', value))

    def poll_tx(self):
        producer = self.index(DEVICE_COMPLETION_INDICES, self.tx_cursor_index)
        cursor = self.index(HOST_COMPLETION_INDICES, self.tx_cursor_index)
        if producer >= 8192 or cursor >= 8192:
            raise RuntimeError('Transmit completion cursor exceeds its ring')
        added = []
        while cursor != producer:
            item = decode_completion(self.read(self.tx_completion + cursor * COMPLETION.size, COMPLETION.size))
            if item['format'] != TX_COMPLETION_FORMAT:
                raise RuntimeError('Unexpected format on the transmit completion queue')
            self.tx_seen.append(item)
            added.append(item)
            cursor = (cursor + 1) % 8192
        self.set_index(HOST_COMPLETION_INDICES, self.tx_cursor_index, cursor)
        return added

    def poll_rx(self, want_data=True):
        """Copy completed Ethernet frames, or acknowledge metadata only.

        Receive completions include two padding bytes in their byte count.
        Other receive classes remain opaque. Recycling preserves the original
        buffer cookie when posting it into the next available submission slot.
        """
        self.control._require_ready()
        frames = []
        recycled = {12: [], 13: []}
        for info in self.rx_cqs.values():
            cursor = self.index(HOST_COMPLETION_INDICES, info['cursor_index'])
            producer = self.index(DEVICE_COMPLETION_INDICES, info['cursor_index'])
            if cursor >= info['entries'] or producer >= info['entries']:
                raise RuntimeError('Receive completion cursor exceeds its ring')
            changed = cursor != producer
            while cursor != producer:
                batch = min((producer - cursor) % info['entries'], info['entries'] - cursor, 64)
                raw = self.read(info['ring'] + cursor * COMPLETION.size, batch * COMPLETION.size)
                for offset in range(0, len(raw), COMPLETION.size):
                    item = decode_completion(raw[offset:offset + COMPLETION.size])
                    cookie, length = item['cookie'], item['bytes_received']
                    if item['format'] != RX_COMPLETION_FORMAT or item['source'] != info['source'] or cookie >= 2048 or not RX_PADDING <= length <= PACKET_CAPACITY:
                        raise RuntimeError('Completion differs from the receive profile')
                    record = self.rx_descriptors[info['source']].get(cookie)
                    if record is None:
                        raise RuntimeError('Completion cookie has no retained allocation')
                    flags, pointer, tag = RX_DESCRIPTOR.unpack(record)
                    if flags & 255 != DATA_FORMAT or flags >> 8 < length or tag & 0xffff != cookie:
                        raise RuntimeError('Completion differs from its posted receive buffer')
                    self.rx_seen += 1
                    previous_slot = self.rx_reposted_inflight[info['source']].pop(cookie, None)
                    if previous_slot is not None and len(self.rx_repost_completions) < 16:
                        self.rx_repost_completions.append(dict(source=info['source'], cookie=cookie,
                                                               submission_slot=previous_slot))
                    if want_data and info['ethernet'] and length >= RX_PADDING + 14:
                        frame = self.read(pointer + RX_PADDING, length - RX_PADDING)
                        if frame[:6] == self.mac or frame[0] & 1:
                            frames.append(frame)
                    if self.recycle_enabled:
                        recycled[info['source']].append(cookie)
                cursor = (cursor + batch) % info['entries']
            if changed:
                self.set_index(HOST_COMPLETION_INDICES, info['cursor_index'], cursor)
        if self.recycle_enabled:
            for source, cookies in recycled.items():
                self._repost_rx(source, cookies)
            if any(recycled.values()):
                self.u.write(self.control.bank + RX_NOTIFY, 1, 32)
        return frames

    def send(self, frame, timeout=5):
        """Send one owned Ethernet packet and wait for its data completion."""
        self.control._require_ready()
        self.poll_tx()
        slot = self.index(HOST_SUBMISSION_INDICES, self.tx_queue)
        consumed = self.index(DEVICE_SUBMISSION_INDICES, self.tx_queue)
        if slot != consumed or slot >= 2048:
            raise RuntimeError('A previous transmit descriptor is still outstanding')
        offset = TX_AREA + slot % TX_SLOTS * TX_SLOT_BYTES
        descriptor = tx_descriptor(len(frame), self.control.arena_iova + offset, slot)
        address = self.control.arena_physical + offset
        self.u.iface.writemem(address, frame)
        self.p.dc_cvac(address, len(frame))
        self.write(self.tx_ring + slot * TX_DESCRIPTOR.size, descriptor)
        self.set_index(HOST_SUBMISSION_INDICES, self.tx_queue, (slot + 1) % 2048)
        self.u.write(self.control.bank + TX_NOTIFY, 1, 32)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for item in self.poll_tx():
                if item['source'] == self.tx_queue and item['cookie'] == slot:
                    return item
            time.sleep(0.005)
        raise TimeoutError('No matching data transmit completion')

    def snapshot(self):
        return dict(state=self.control.snapshot(), tx_queue=self.tx_queue,
                    tx_cursor_index=self.tx_cursor_index,
                    tx_producer=self.index(HOST_SUBMISSION_INDICES, self.tx_queue),
                    tx_consumer=self.index(DEVICE_SUBMISSION_INDICES, self.tx_queue),
                    rx12_posted=self.index(HOST_SUBMISSION_INDICES, 12),
                    rx12_consumed=self.index(DEVICE_SUBMISSION_INDICES, 12),
                    rx13_posted=self.index(HOST_SUBMISSION_INDICES, 13),
                    rx13_consumed=self.index(DEVICE_SUBMISSION_INDICES, 13), rx_seen=self.rx_seen,
                    recycle_enabled=self.recycle_enabled, rx_recycled=dict(self.rx_recycled))
