# SPDX-License-Identifier: GPL-2.0-only
"""Descriptor vectors and context restart observed in reference-73."""
import json
import pathlib
import struct
import tempfile
import unittest

from centauri_ethernet import (tx_descriptor, decode_completion, latest_queue_registry,
                               receive_buffers, RX_DESCRIPTOR)


class EthernetWireTests(unittest.TestCase):
    def test_recycled_receive_cookie_is_independent_of_submission_slot(self):
        ring = bytearray(2048 * RX_DESCRIPTOR.size)
        first = RX_DESCRIPTOR.pack(2048 << 8 | 3, 0xf004000, 1)
        second = RX_DESCRIPTOR.pack(2048 << 8 | 3, 0xf008000, 2)
        # Target-77 completes cookie 1 from slot 2047, and cookie 2 from
        # slot 0. Retired slots may retain identical descriptor copies.
        ring[-16:] = first
        ring[:16] = second
        ring[16:32] = first
        self.assertEqual(receive_buffers(ring), {1: first, 2: second})
        ring[16:32] = RX_DESCRIPTOR.pack(2048 << 8 | 3, 0xf00c000, 1)
        with self.assertRaises(ValueError):
            receive_buffers(ring)

    def test_transmit_descriptor_matches_published_ipv6_frame(self):
        words = (0x4e03, 0x9258800, 0, 10, 0x20000, 0, 0, 0)
        self.assertEqual(tx_descriptor(78, 0x9258800, 10), struct.pack('<8I', *words))

    def test_receive_completion_has_source_length_cookie_and_extended_metadata(self):
        words = (0xc0403, 0xc801a1, 0, 0x201c2080, 0x395f883, 1, 0, 0x30a87)
        decoded = decode_completion(struct.pack('<8I', *words))
        self.assertEqual((decoded['source'], decoded['cookie'], decoded['bytes_received']), (12, 417, 200))
        self.assertEqual(decoded['words'], words)
        with self.assertRaises(ValueError):
            decode_completion(struct.pack('<8I', *words)[:16])

    def test_transmit_completion_does_not_assign_a_receive_byte_count(self):
        words = (0x1c0502, 15, 0x600, 0x613f3000, 0xfa18aaee, 4, 0x2f40f1fa, 0)
        decoded = decode_completion(struct.pack('<8I', *words))
        self.assertEqual((decoded['source'], decoded['cookie']), (28, 15))
        self.assertIsNone(decoded['bytes_received'])

    def test_registry_discards_queues_from_the_previous_publication(self):
        old = [0x40001, 0x190019, 0x8a14000, 0, 0xffffffff, 0xffffffff,
               0x4c0800, 0x80012, 0, 3, 0, 0, 0, 0, 0]
        new = [0x40001, 0x200020, 0x8b4c000, 0, 0xffffffff, 0xffffffff,
               0x500800, 0x80012, 0, 3, 0, 0, 0, 0, 0]
        events = [dict(kind='wifi_initial_context_metadata', iova='0x8440000'),
                  dict(kind='wifi_admin_control_frame', length=62, words=list(map(hex, old))),
                  dict(kind='wifi_initial_context_metadata', iova='0x7cc8000'),
                  dict(kind='wifi_admin_control_frame', length=62, words=list(map(hex, new)))]
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'metadata.jsonl'
            path.write_text(''.join(json.dumps(event) + '\n' for event in events))
            context, configs = latest_queue_registry(path)
        self.assertEqual(context, 0x7cc8000)
        self.assertEqual(configs, {(1, 32): tuple(new)})


if __name__ == '__main__':
    unittest.main()
