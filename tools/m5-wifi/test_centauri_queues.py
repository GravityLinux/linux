# SPDX-License-Identifier: GPL-2.0-only
"""Check queue ownership, record spacing and receive-buffer bounds."""
import struct
import unittest
from types import SimpleNamespace as NS

from centauri_queues import (DataArena, DataQueue, PROFILES, DESCRIPTOR, INLINE_STRIDE,
                              encode_runtime_message, decode_runtime_message)


class QueueTests(unittest.TestCase):
    def test_control_arena_needs_no_peer_dma_domains(self):
        pages = NS(pages=[])
        def allocate(alignment, size):
            pages.pages.append((0x10000000, size))
            return 0x10000000
        pages.memalign = allocate
        mappings = {}
        def translate(sid, address, size):
            self.assertEqual(sid, 1)
            return [(mappings.get(address), size)]
        def map_at(sid, address, physical, size):
            self.assertEqual(sid, 1)
            mappings[address] = physical
        ipc = NS(sid=1, pages=pages, snapshot=lambda: dict(index_pairs=[[0, 0], [0, 0]]),
                 dart=NS(iotranslate=translate, iomap_at=map_at, dart=NS(pt_cache={})),
                 regs=NS(TCR={1: NS(reg=NS(TRANSLATE_ENABLE=True))},
                         TTBR={1: NS(reg=NS(VALID=True))}, TLB_OP=NS(val=0)),
                 u=NS(proxy=NS(), adt={'/arm-io/dart-apcie0': NS(get_reg=lambda n: (0x1000, 0x1000))}),
                 driver=NS(wait32=lambda *a, **kw: None))
        arena = DataArena(ipc, iova=0x7000000)
        self.assertEqual(arena.physical, 0x10000000)
        restored = DataArena.from_owned_memory(ipc, arena.iova, arena.physical)
        self.assertEqual(restored.iova, arena.iova)

    def setUp(self):
        self.writes, self.reads = [], []
        self.consumer = 0
        self.header = bytes(16)
        def readmem(address, size):
            self.reads.append((address, size))
            return self.header if size == 16 else bytes(size)
        u = NS(iface=NS(writemem=lambda *a: self.writes.append(a), readmem=readmem),
               proxy=NS(dc_cvac=lambda *a: None, dc_ivac=lambda *a: None),
               read=lambda *a: self.consumer, write=lambda *a: self.writes.append(a))
        self.arena = NS(u=u, iova=0x1000000, physical=0x10000000,
                        ipc=NS(physical=0x20000000, bank=0x30000000))

    def test_inline_last_entry_requires_the_second_page_and_does_not_overlap(self):
        q = DataQueue(self.arena, 3, PROFILES[3])
        q.producer, self.consumer = 15, 14
        q.send_inline(bytes(1024))
        address, record = self.writes[0]
        self.assertEqual(address, self.arena.physical + 0x10000 + 15 * INLINE_STRIDE)
        self.assertEqual(len(record), 1040)
        self.assertGreater(address + len(record), self.arena.physical + 0x14000)
        self.assertLessEqual(address + len(record), self.arena.physical + 0x18000)
        self.assertEqual(q.producer, 0)
        self.assertEqual(self.writes[-1], (0x3000100c, 0, 32))

    def test_receive_credit_overflow_is_rejected_before_writing_memory(self):
        q = DataQueue(self.arena, 2, PROFILES[2])
        with self.assertRaises(RuntimeError):
            q.post_receive(16)
        self.assertEqual(self.writes, [])

    def test_external_pointer_change_is_rejected_before_reading_payload(self):
        q = DataQueue(self.arena, 2, PROFILES[2])
        self.consumer = 1
        self.header = DESCRIPTOR.pack(0x1f01, 0xdead0000, 0x4000000)
        with self.assertRaisesRegex(RuntimeError, 'owned buffer'):
            q.receive()
        self.assertEqual(self.reads, [(q.physical, 16)])

    def test_queue8_payload_remains_unread_when_buffers_complete(self):
        q = DataQueue(self.arena, 8, PROFILES[8])
        self.consumer = 1
        pointer = self.arena.iova + PROFILES[8].pool
        self.header = DESCRIPTOR.pack(0xfdc01, pointer, 0x4000000)
        packet, = q.receive()
        self.assertEqual(packet['size'], 4060)
        self.assertIsNone(packet['payload'])
        self.assertEqual(self.reads, [(q.physical, 16)])

    def test_runtime_packet_length_and_header_bounds_are_checked(self):
        for packet in (bytes(7), struct.pack('<II', 9 << 24 | 10, 1)):
            with self.assertRaises(ValueError):
                decode_runtime_message(packet)
        with self.assertRaises(ValueError):
            encode_runtime_message(256, 0)


if __name__ == '__main__':
    unittest.main()
