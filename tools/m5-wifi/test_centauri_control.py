# SPDX-License-Identifier: GPL-2.0-only
"""Keep control captures inside bounded, completely mapped host DRAM."""
import unittest
from types import SimpleNamespace as NS

from trace_centauri_control import ControlFrameCapture


class ControlCaptureTests(unittest.TestCase):
    def setUp(self):
        self.reads = []
        def read(address, size):
            self.reads.append((address, size))
            return bytes(size)
        hv = NS(u=NS(ba=NS(phys_base=0x100000000, mem_size_actual=0x1000000)),
                iface=NS(readmem=read))
        self.capture = ControlFrameCapture(hv, None)

    def test_fragmented_control_metadata_is_read_at_exact_extent(self):
        self.capture.dart = NS(iotranslate=lambda *a: [(0x100004000, 32), (0x100008000, 32)])
        self.assertEqual(self.capture.read_dma(1, 0x1000000, 64), bytes(64))
        self.assertEqual(self.reads, [(0x100004000, 32), (0x100008000, 32)])

    def test_holes_and_outside_dram_are_rejected_before_any_read(self):
        for ranges in ([(None, 64)], [(0x100004000, 32)], [(0xfffffff0, 64)],
                       [(0x100fffff0, 64)], [(0x100004000, -64)]):
            self.capture.dart = NS(iotranslate=lambda *a: ranges)
            with self.assertRaises(ValueError):
                self.capture.read_dma(1, 0x1000000, 64)
            self.assertEqual(self.reads, [])

    def test_capture_bounds_are_checked_before_walking_page_tables(self):
        self.capture.dart = NS(iotranslate=lambda *a: self.fail('Unexpected page-table walk'))
        for address, size in ((0, 0), (0, 0x3b1), (-1, 4), ((1 << 36) - 4, 8)):
            with self.assertRaises(ValueError):
                self.capture.read_dma(1, address, size)
        self.assertEqual(self.reads, [])


if __name__ == '__main__':
    unittest.main()
