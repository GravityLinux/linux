# SPDX-License-Identifier: GPL-2.0-only
"""For LLMs: own PF2 context/queue publication after a quiescing reset.

Only host protocol structures are initialized. Firmware is already running.
The caller supplies stopped old DMA, reserved typed buffers and a live mapping.
"""
import struct
import time
from centauri_ipc import make_beta_context

HANDSHAKE = 0x107c
GLOBAL_DEVICE_INDICES = 0x78
GLOBAL_HOST_INDICES = 0xa6
GLOBAL_SUBMISSION_RING = 0xd4
GLOBAL_COMPLETION_RING = 0x1d4
GLOBAL_ENTRIES = 16
CONTROL_FRAME_AREA = 0x1000
CONTROL_FRAME_STRIDE = 0x80
CONTROL_VECTOR_POINTERS = 0x10
CONTROL_RING_POINTERS = 0x34
EXTERNAL_CONTROL_TX = 1
# The tested mode marks completion in the submitted inline slot (tag bit 26).
# The alternative separate-CQ mode is not yet functional.
QUEUE_IN_PLACE_COMPLETIONS = 1 << 22
# These encode the three capacity geometries actually tested on hardware.
CAPACITY_MODES = {260: 0x00410001, 2592: 0x30510001, 1024: 0x40100001,
                  724: 0x00b50001, 256: 0x40040001, 268: 0x00430001}


def inline_capacity_mode(capacity, *, exponent=None):
    """Construct a controlled capacity encoding for hardware equivalence tests.

    Byte 0 is Create Transfer Ring; byte 1 reserves no extra header words.
    The footer payload mantissa in bits 23:16 counts 32-bit words, scaled
    by the exponent in bits 31:28. Capacity is mantissa * 4 * 2**exponent. The baseline uses the separately verified modes above;
    alternative encodings must be probed before replacing those defaults.
    """
    if type(capacity) is not int or not 0 < capacity <= 4096:
        raise ValueError('Capacity outside the bounded probe range')
    choices = range(5) if exponent is None else (exponent,)
    for shift in choices:
        if type(shift) is not int or not 0 <= shift <= 4:
            raise ValueError('Invalid capacity exponent')
        unit = 1 << (2 + shift)
        mantissa, remainder = divmod(capacity, unit)
        if not remainder and 0 < mantissa <= 255:
            return shift << 28 | mantissa << 16 | 1
    raise ValueError('Capacity has no exact inline encoding')


def core_host_queue_layout(context, *, acl_mtu=None, slots=2, arena_bytes=0x8000,
                           ring_start_offset=0x4000):
    """Generate host ring placement without retaining reference addresses.

    Bootstrap command/event rings suffice to read the controller's ACL MTU.
    Adding ACL rings preserves those initial ring addresses. All rings fit in
    the caller's zeroed arena. The default leaves a separate context page;
    a smaller ring offset is a controlled packing experiment.
    """
    if type(slots) is not int or not 2 <= slots <= 128 or type(arena_bytes) is not int or arena_bytes < 0x4000 or arena_bytes & 0x3fff:
        raise ValueError('Invalid host arena geometry')
    if type(context) is not int or context < 0 or context & 0x3fff or context + arena_bytes > 1 << 36:
        raise ValueError('Invalid aligned host arena address')
    if type(ring_start_offset) is not int or not 0x1000 <= ring_start_offset < arena_bytes or ring_start_offset & 63:
        raise ValueError('Invalid controlled ring start offset')
    # HCI command: opcode(2), parameter length(1), parameters(<=255).
    # HCI event: code(1), parameter length(1), parameters(<=255).
    capacities = [(1, (3 + 255 + 3) & -4), (7, (2 + 255 + 3) & -4)]
    if acl_mtu is not None:
        if type(acl_mtu) is not int or not 0 < acl_mtu <= 4092:
            raise ValueError('ACL MTU outside the bounded host allocation')
        acl_capacity = (4 + acl_mtu + 3) & -4
        capacities += [(2, acl_capacity), (8, acl_capacity)]
    queues, offset = [], ring_start_offset
    for number, capacity in capacities:
        offset = (offset + 63) & -64
        end = offset + slots * (16 + capacity)
        if end > arena_bytes:
            raise ValueError('Host rings exceed the reserved arena')
        queues.append(dict(number=number, ring_iova=context + offset,
                           entries=slots, capacity=capacity))
        offset = end
    return queues


class BluetoothControl:
    def __init__(self, transport, *, frame_area=CONTROL_FRAME_AREA,
                 frame_stride=CONTROL_FRAME_STRIDE, frame_slots=GLOBAL_ENTRIES):
        self.transport = transport
        self.context = transport.context
        self.memory = transport.memory
        self.bank = transport.bank
        self.producer = 0
        self.completion_entry_bytes = 32
        if (type(frame_area) is not int or frame_area < 0x800 or frame_area & 63
                or type(frame_stride) is not int or frame_stride < 52 or frame_stride & 63
                or type(frame_slots) is not int or not 1 <= frame_slots <= GLOBAL_ENTRIES
                or frame_area + frame_stride * frame_slots > 0x4000):
            raise ValueError('Invalid controlled frame storage geometry')
        self.frame_area, self.frame_stride, self.frame_slots = frame_area, frame_stride, frame_slots
        self.frame_start = self.context + frame_area
        self.frame_end = self.frame_start + frame_stride * frame_slots
        for queue in getattr(transport, 'state', {}).get('bluetooth_queues', ()):
            self.check_frame_separation(queue['ring_iova'], queue['entries'] * (16 + queue['capacity']))

    def check_frame_separation(self, address, length):
        if address < self.frame_end and self.frame_start < address + length:
            raise ValueError('Control frame storage overlaps a live ring/vector')

    def wait_state(self, wanted, timeout=2):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            state = self.memory.read(self.bank, 32)
            if state == wanted:
                return
            if self.memory.read(self.bank + 0x18, 32) != 2:
                raise RuntimeError('Bluetooth firmware left ready during handshake')
            time.sleep(.005)
        raise TimeoutError(f'Bluetooth IPC did not reach state {wanted}, last={state}')

    def zero(self, address, length):
        for offset in range(0, length, 4096):
            self.transport.write(address + offset, bytes(min(4096, length - offset)))

    def publish_context(self, dma_base, dma_end, *, completion_entry_bytes=16):
        if self.memory.read(self.bank, 32) != 0 or self.memory.read(self.bank + 0x18, 32) != 2:
            raise RuntimeError('Publishing requires reset IPC with running firmware')
        if not 0 <= dma_base <= self.context or not self.context + 0x4000 <= dma_end < 1 << 36:
            raise ValueError('Context outside DMA window')
        self.zero(self.context, 0x4000)
        header = make_beta_context(self.context, completion_entry_bytes=completion_entry_bytes)
        self.completion_entry_bytes = completion_entry_bytes
        self.transport.write(self.context, header)
        self.memory.write(self.bank + HANDSHAKE, 1, 32)
        self.wait_state(1)
        for offset, value in ((0xc, dma_base & 0xffffffff), (0x10, dma_base >> 32),
                              (0x14, dma_end - dma_base), (4, self.context & 0xffffffff),
                              (8, self.context >> 32)):
            self.memory.write(self.bank + offset, value, 32)
        self.memory.write(self.bank + HANDSHAKE, 2, 32)
        self.wait_state(2)
        self.transport.ready()
        self.producer = 0

    def submit(self, frame):
        if len(frame) != 52:
            raise ValueError('Only typed queue registration is supported')
        t, slot = self.transport, self.producer
        device_cq, device_sq, host_cq, host_sq = struct.unpack('<4Q', t.read(self.context + CONTROL_VECTOR_POINTERS, 32))
        completion_ring, submission_ring = struct.unpack('<2Q', t.read(self.context + CONTROL_RING_POINTERS, 16))
        vectors_in_context = all(self.context <= pointer <= self.context + 0x4000 - 2
                                 for pointer in (device_cq, device_sq, host_cq, host_sq))
        rings_in_context = (self.context <= completion_ring <= self.context + 0x4000 - GLOBAL_ENTRIES * self.completion_entry_bytes
                            and self.context <= submission_ring <= self.context + 0x4000 - GLOBAL_ENTRIES * 16)
        if not vectors_in_context or not rings_in_context:
            raise ValueError('Control vectors/ring outside the owned context')
        for pointer in (device_cq, device_sq, host_cq, host_sq):
            self.check_frame_separation(pointer, 22 * 2)
        self.check_frame_separation(completion_ring, GLOBAL_ENTRIES * self.completion_entry_bytes)
        self.check_frame_separation(submission_ring, GLOBAL_ENTRIES * 16)
        def device_indices():
            return (struct.unpack('<H', t.read(device_cq, 2))[0],
                    struct.unpack('<H', t.read(device_sq, 2))[0])
        indices = device_indices()
        if indices != (slot, slot):
            raise RuntimeError('Previous global control operation incomplete')
        # Reuse is safe only after the exact prior submission/completion pair
        # above has drained; a single-frame experiment cannot overwrite DMA.
        frame_iova = self.frame_start + (slot % self.frame_slots) * self.frame_stride
        t.write(frame_iova, frame)
        t.write(submission_ring + slot * 16,
                struct.pack('<IQI', len(frame) << 8 | EXTERNAL_CONTROL_TX, frame_iova, slot))
        next_slot = (slot + 1) % GLOBAL_ENTRIES
        t.write(host_cq, struct.pack('<H', indices[0]))
        t.write(host_sq, struct.pack('<H', next_slot))
        self.memory.write(self.bank + 0x1000, next_slot, 32)
        end = time.monotonic() + 2
        while time.monotonic() < end:
            t.ready()
            indices = device_indices()
            if indices == (next_slot, next_slot):
                break
            time.sleep(.005)
        else:
            raise TimeoutError(f'Queue setup did not complete: {indices}')
        header = struct.unpack('<4I', t.read(completion_ring + slot * self.completion_entry_bytes, 16))
        if header[0] != 0x401 or header[1] != len(frame) << 16 | slot:
            raise RuntimeError(f'Unknown queue registration completion: {[hex(word) for word in header]}')
        self.producer = next_slot

    def register(self, queue, *, registration_flags=QUEUE_IN_PLACE_COMPLETIONS):
        number, entries, capacity, ring = (queue[k] for k in ('number', 'entries', 'capacity', 'ring_iova'))
        if capacity not in CAPACITY_MODES or not 1 <= number < 22 or not 2 <= entries <= 128:
            raise ValueError('Unsupported typed queue geometry')
        self.check_frame_separation(ring, entries * (16 + capacity))
        self.zero(ring, entries * (16 + capacity))
        # Same host/device cursor index; inline address sentinel; no separate
        # completion queue (0xffff). Bit 22 selects the working in-place mode.
        words = (CAPACITY_MODES[capacity], number << 16 | number, ring & 0xffffffff,
                 ring >> 32, 0xffffffff, 0xffffffff, 0xffff0000 | entries,
                 registration_flags | number, 0, 0, 0, 0, 0)
        self.submit(struct.pack('<13I', *words))
        print('BT_OWN_QUEUE_REGISTERED', dict(number=number, capacity=capacity, entries=entries), flush=True)
