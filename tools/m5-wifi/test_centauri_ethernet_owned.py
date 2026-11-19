# SPDX-License-Identifier: GPL-2.0-only
"""Extended queue vectors from bounded native185 metadata."""
import struct
import unittest
from centauri_ethernet_owned import extended_completion_config, extended_submission_config


class OwnedEthernetWireTests(unittest.TestCase):
    def test_receive_completion_logical_number_differs_from_cursor(self):
        frame=extended_completion_config(15,207,0x0a050000,
            entries=4096,source=12,profile=0xb,service=8)
        self.assertEqual(frame,struct.pack('<13IH',0x40002,0xcf000f,0xa050000,0,
            0xc1000,0xb,0,8,0,0,0,0,0,0))
        self.assertEqual(len(frame),54)

    def test_transmit_submission_binds_its_logical_completion_queue(self):
        frame=extended_submission_config(25,0xa040000,receive=False,
            completion_queue=76,service=3)
        self.assertEqual(frame,struct.pack('<15IH',0x40001,0x190019,0xa040000,0,
            0xffffffff,0xffffffff,0x4c0800,0x80012,0,3,0,0,0,0,0,0))
        self.assertEqual(len(frame),62)

    def test_receive_submission_uses_its_source_selector(self):
        frame=extended_submission_config(12,0xa070000,receive=True,
            completion_queue=12,service=8)
        self.assertEqual(frame,struct.pack('<15IH',1,0xc000c,0xa070000,0,
            0xffffffff,0xffffffff,0xc0800,0x228000b,0,8,0,0,0,0,0,0))


if __name__=='__main__':
    unittest.main()
