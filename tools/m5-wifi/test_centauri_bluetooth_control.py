# SPDX-License-Identifier: GPL-2.0-only
"""Control serialization must follow relocated, bounded host pointers."""
import struct
import unittest
from types import SimpleNamespace

from centauri_bluetooth_control import BluetoothControl, core_host_queue_layout, inline_capacity_mode


class HostContext:
    def __init__(self, frame_area=0x1000, frame_slots=16):
        self.frame_area, self.frame_slots = frame_area, frame_slots
        self.frame_pointers = []
        self.context = 0x4000000
        self.bank = 0xb82000000
        self.data = bytearray(0x4000)
        self.writes = []
        # Deliberately nonadjacent vectors and relocated control rings.
        self.vectors = tuple(self.context + offset for offset in (0x800, 0x840, 0x880, 0x8c0))
        self.completion = self.context + 0x400
        self.submission = self.context + 0x700
        struct.pack_into('<4Q', self.data, 0x10, *self.vectors)
        struct.pack_into('<2Q', self.data, 0x34, self.completion, self.submission)
        self.memory = SimpleNamespace(write=self.doorbell)

    def read(self, address, length):
        offset = address - self.context
        assert 0 <= offset <= len(self.data) - length
        return bytes(self.data[offset:offset + length])

    def write(self, address, data):
        offset = address - self.context
        assert 0 <= offset <= len(self.data) - len(data)
        self.writes.append((address, bytes(data)))
        self.data[offset:offset + len(data)] = data

    def ready(self):
        pass

    def doorbell(self, address, producer, width):
        assert address == self.bank + 0x1000 and width == 32
        slot = (producer - 1) % 16
        flags, pointer, tag = struct.unpack('<IQI', self.read(self.submission + slot * 16, 16))
        assert flags & 255 == 1 and tag == slot
        assert pointer == self.context + self.frame_area + (slot % self.frame_slots) * 0x80
        self.frame_pointers.append(pointer)
        for vector in self.vectors[:2]:
            struct.pack_into('<H', self.data, vector - self.context, producer)
        struct.pack_into('<4I', self.data, self.completion - self.context + slot * 32,
                         0x401, (flags >> 8) << 16 | slot, 0, 0)


class ControlPointerTests(unittest.TestCase):
    def test_separate_vectors_and_relocated_rings(self):
        host = HostContext()
        control = BluetoothControl(host)
        frame = struct.pack('<13I', 1, 0x10001, *([0] * 11))
        control.submit(frame)
        self.assertEqual(control.producer, 1)
        self.assertEqual(struct.unpack('<H', host.read(host.vectors[3], 2))[0], 1)
        self.assertEqual(host.read(host.context + 0x1000, 52), frame)
        self.assertEqual(host.read(host.context + 0xd4, 16), bytes(16))

    def test_one_frame_is_reused_across_control_ring_wrap(self):
        host = HostContext(frame_area=0xf80, frame_slots=1)
        control = BluetoothControl(host, frame_area=0xf80, frame_slots=1)
        for sequence in range(20):
            frame = struct.pack('<13I', 1, sequence, *([0] * 11))
            control.submit(frame)
            self.assertEqual(host.read(host.context + 0xf80, 52), frame)
        self.assertEqual(set(host.frame_pointers), {host.context + 0xf80})
        # An outstanding device index must stop reuse before any write.
        struct.pack_into('<H', host.data, host.vectors[0] - host.context, 3)
        count = len(host.writes)
        with self.assertRaises(RuntimeError):
            control.submit(bytes(52))
        self.assertEqual(len(host.writes), count)

    def test_frame_storage_must_not_alias_declared_data_rings(self):
        host = HostContext()
        host.state = {'bluetooth_queues': core_host_queue_layout(host.context,
                      acl_mtu=2586, arena_bytes=0x4000, ring_start_offset=0x1000)}
        with self.assertRaises(ValueError):
            BluetoothControl(host)
        BluetoothControl(host, frame_area=0xf80, frame_slots=1)
        self.assertEqual(host.writes, [])

    def test_invalid_pointers_stop_before_writing(self):
        for field, bad in ((0x10, 0x4004000), (0x34, 0x4003f00), (0x3c, 0x4003fff)):
            with self.subTest(field=field):
                host = HostContext()
                struct.pack_into('<Q', host.data, field, bad)
                with self.assertRaises(ValueError):
                    BluetoothControl(host).submit(bytes(52))
                self.assertEqual(host.writes, [])


class GeneratedLayoutTests(unittest.TestCase):
    def test_bootstrap_addresses_survive_adding_acl(self):
        base = 0x10000000
        partial = core_host_queue_layout(base)
        full = core_host_queue_layout(base, acl_mtu=2586)
        self.assertEqual(partial, full[:2])
        self.assertEqual({q['number'] for q in full}, {1, 2, 7, 8})
        for queue, following in zip(full, full[1:]):
            end = queue['ring_iova'] + queue['entries'] * (16 + queue['capacity'])
            self.assertLessEqual(end, following['ring_iova'])
        self.assertLessEqual(full[-1]['ring_iova'] + 2 * (16 + full[-1]['capacity']), base + 0x8000)
        with self.assertRaises(ValueError):
            core_host_queue_layout(base, acl_mtu=2586, slots=128)

    def test_full_advertised_mtu_fits_one_dart_page(self):
        base = 0x10000000
        kwargs = dict(arena_bytes=0x4000, ring_start_offset=0x1000)
        partial = core_host_queue_layout(base, **kwargs)
        full = core_host_queue_layout(base, acl_mtu=2586, **kwargs)
        self.assertEqual(partial, full[:2])
        self.assertEqual([q['capacity'] for q in full], [260, 260, 2592, 2592])
        for queue, following in zip(full, full[1:]):
            self.assertLessEqual(queue['ring_iova'] + queue['entries'] * (16 + queue['capacity']), following['ring_iova'])
        self.assertLessEqual(full[-1]['ring_iova'] + 2 * (16 + full[-1]['capacity']), base + 0x4000)
        with self.assertRaises(ValueError):
            core_host_queue_layout(base, acl_mtu=2586, slots=3, **kwargs)

    def test_capacity_equivalence_is_exact(self):
        for exponent in (2, 3):
            mode = inline_capacity_mode(2592, exponent=exponent)
            self.assertEqual(((mode >> 16) & 255) << (2 + (mode >> 28)), 2592)
        for capacity in (True, 0, 2593, 4097):
            with self.assertRaises(ValueError):
                inline_capacity_mode(capacity)


if __name__ == '__main__':
    unittest.main()
