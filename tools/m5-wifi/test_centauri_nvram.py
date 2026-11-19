# SPDX-License-Identifier: GPL-2.0-only
"""Synthetic SKU values test checksum coverage and independent variable sizing."""
import contextlib
import io
from pathlib import Path
import struct
import sys
import tempfile
import unittest
from unittest.mock import patch
import zlib
from centauri_nvram import make_antenna_nvram, sku_from_dt_property


class NvramTests(unittest.TestCase):
    def test_variable_skus_generate_sized_entries_and_entry_only_crc(self):
        for sku in (b'A', b'Q7', b'TEST1234'):
            record = make_antenna_nvram(sku)
            self.assertEqual(len(record), 40 + len(sku))
            signature, crc, size0, size1, profile0, profile1 = struct.unpack_from('<6I', record)
            self.assertEqual(signature, int.from_bytes(b'SvvN', 'little'))
            self.assertEqual((size0, size1), (len(record), len(record)))
            self.assertEqual((profile0, profile1), (1, 1))
            self.assertEqual(crc, zlib.crc32(record[24:]))
            self.assertEqual(record[24:28], b'wsku')
            self.assertEqual(struct.unpack_from('<I', record, 32)[0], len(sku))
            self.assertEqual(record[40:], sku)

    def test_replacing_platform_value_recomputes_checksum_without_fixed_device_data(self):
        first, second = make_antenna_nvram('Q1'), make_antenna_nvram('Q2')
        self.assertNotEqual(first[4:8], second[4:8])
        self.assertEqual(first[8:40], second[8:40])
        self.assertEqual(second[40:], b'Q2')

    def test_dt_property_requires_exactly_one_trailing_terminator(self):
        self.assertEqual(sku_from_dt_property(b'Q7\0'), b'Q7')
        for value in (b'', b'Q7', b'Q\0Z\0', b'Q7\0\0'):
            with self.assertRaises(ValueError):
                sku_from_dt_property(value)

    def test_packager_uses_dt_property_without_any_external_nvram_record(self):
        from centauri_ftab import Directory
        from prepare_centauri_firmware import main
        from test_centauri_ftab import firmware
        with tempfile.TemporaryDirectory(prefix='centauri-sku-') as tmp:
            root = Path(tmp)
            for name, data in (('vendor', firmware(b'opaque vendor')),
                    ('manifest', b'opaque manifest'), ('sku', b'Q7\0'),
                    ('cal', b'opaque cal'), ('apb', b'opaque policy')):
                (root/name).write_bytes(data)
            arguments = ['prepare', '--vendor', str(root/'vendor'),
                '--manifest', str(root/'manifest'), '--antenna-sku-file', str(root/'sku'),
                '--calibration', str(root/'cal'), '--ap-policy', str(root/'apb'),
                '--output', str(root/'output')]
            with patch.object(sys, 'argv', arguments), contextlib.redirect_stdout(io.StringIO()):
                main()
            result = Directory((root/'output').read_bytes())
            self.assertEqual(result.opaque_entry(b'nvrm'), make_antenna_nvram('Q7'))
            self.assertEqual(result.opaque_entry(b'_cal'), b'opaque cal')
            self.assertEqual(result.opaque_entry(b'_apb'), b'opaque policy')
            self.assertEqual(result.opaque_entry(b'2ftb'), b'opaque vendor')

    def test_missing_oversize_or_non_ascii_platform_value_is_not_defaulted(self):
        for value in (b'', b'123456789', b'Q\0', b'\xff', None, 1):
            with self.assertRaises(ValueError):
                make_antenna_nvram(value)


if __name__ == '__main__':
    unittest.main()
