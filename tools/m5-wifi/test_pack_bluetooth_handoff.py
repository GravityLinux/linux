# SPDX-License-Identifier: GPL-2.0-only
"""Reject stale/foreign resources and unsupported ring extents at encoding."""
import copy
import unittest
from pack_bluetooth_handoff import pack, HEADER, QUEUE, CORE_QUEUES


class BluetoothMetadataTests(unittest.TestCase):
    def setUp(self):
        self.state = dict(system_serial='TESTDEVICE', handoff_nonce='5a'*32,
                          bluetooth_context_iova=0x10000000,
                          bluetooth_bar0_physical=0xb82000000, dart_base=0x488000000,
                          handoff_nonce_physical=0x10000100000,
                          dram_base=0x10000000000, dram_size=0x400000000,
                          streams={'3': {'mappings': [dict(iova=0x10000000, physical=0x10000200000, length=0x4000)]}},
                          bluetooth_queues=[dict(number=n, entries=2, capacity=c,
                                                ring_iova=0x10000000+o)
                                            for n, (c, o) in CORE_QUEUES.items()])

    def test_only_named_metadata(self):
        encoded = pack(self.state)
        self.assertEqual(len(encoded), 224)
        self.assertEqual(HEADER.unpack_from(encoded)[:6], (b'CENB0002', 224, 1, 4, 10, 0x10000000))
        self.assertEqual([QUEUE.unpack_from(encoded, HEADER.size+24+i*16)[0] for i in range(4)], [1, 2, 7, 8])

    def test_accepts_other_serials_without_changing_geometry(self):
        for serial in ('OTHER', 'ANOTHER-DEVICE', 'X' * 32):
            state = copy.deepcopy(self.state)
            state['system_serial'] = serial
            encoded = pack(state)
            with self.subTest(serial=serial):
                self.assertEqual(len(encoded), 224)
                self.assertEqual(HEADER.unpack_from(encoded)[4], len(serial))
                self.assertEqual(encoded[HEADER.size:], pack(self.state)[HEADER.size:])

    def test_refuse_malformed_identity_and_unsupported_geometry(self):
        for field, value in [('system_serial', ''), ('system_serial', 'X' * 33), ('dart_base', 0), ('handoff_nonce_physical', 0), ('bluetooth_context_iova', 1)]:
            state = copy.deepcopy(self.state); state[field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                pack(state)
        for field, value in [('entries', 3), ('capacity', 2586), ('ring_iova', 0x10000000), ('number', 9)]:
            state = copy.deepcopy(self.state); state['bluetooth_queues'][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                pack(state)


if __name__ == '__main__':
    unittest.main()
