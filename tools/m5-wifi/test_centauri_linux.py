# SPDX-License-Identifier: GPL-2.0-only
"""Handoff translation boundaries for the Linux-local packet prototype."""
import unittest

from centauri_linux import RetainedTranslations


class RetainedTranslationTests(unittest.TestCase):
    def test_frame_crossing_noncontiguous_physical_pages(self):
        table = RetainedTranslations([
            dict(iova=0x4000, physical=0x10000400000, length=0x4000),
            dict(iova=0x8000, physical=0x10000800000, length=0x4000)])
        self.assertEqual(table.iotranslate(2, 0x7ff0, 64),
                         [(0x10000403ff0, 16), (0x10000800000, 48)])

    def test_missing_mapping_never_becomes_a_physical_read(self):
        table = RetainedTranslations([
            dict(iova=0x4000, physical=0x10000400000, length=0x4000)])
        with self.assertRaises(ValueError):
            table.iotranslate(2, 0x7ff0, 64)
        with self.assertRaises(ValueError):
            table.iotranslate(1, 0x4000, 64)

    def test_overlapping_handoff_ranges_are_rejected(self):
        with self.assertRaises(ValueError):
            RetainedTranslations([
                dict(iova=0x4000, physical=0x10000400000, length=0x4000),
                dict(iova=0x6000, physical=0x10000800000, length=0x4000)])


if __name__ == '__main__':
    unittest.main()
