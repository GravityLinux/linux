# SPDX-License-Identifier: GPL-2.0-only
"""Factory-container extraction and malformed-input checks use synthetic data."""
import contextlib
import io
from pathlib import Path
import struct
import sys
import tempfile
import unittest
from unittest.mock import patch

from centauri_calibration import elements, extract_calibration, factory_components


def der(identifier, value):
    length = len(value)
    if length < 128:
        encoded_length = bytes((length,))
    else:
        encoded = length.to_bytes((length.bit_length() + 7) // 8, 'big')
        encoded_length = bytes((128 + len(encoded),)) + encoded
    return identifier + encoded_length + value


def sequence(*children):
    return der(b'\x30', b''.join(children))


def ia5(value):
    return der(b'\x16', value)


def private_tag(name):
    number = int.from_bytes(name, 'big')
    digits = [number & 127]
    number >>= 7
    while number:
        digits.append(128 | (number & 127))
        number >>= 7
    return b'\xff' + bytes(reversed(digits))


def factory(records, *, kind=b'BWC3', selector=b'.'):
    children = []
    for name, data in records:
        numeric = int.from_bytes(name, 'little').to_bytes(4, 'big')
        children.append(sequence(der(b'\x02', numeric), ia5(selector), der(b'\x04', data)))
    spay = der(private_tag(b'SPAY'), der(b'\x31', b''.join(children)))
    body = sequence(der(b'\x02', b'\x01'), der(b'\x02', b'\x02'), spay)
    payload = sequence(ia5(b'IM4P'), ia5(kind), ia5(b'synthetic'), der(b'\x04', body))
    # The signature envelope remains opaque and is explicitly not verified.
    return sequence(ia5(b'IMG4'), payload, der(b'\xa0', b'opaque signature'))


RECORDS = ((b'BTCS', b'B' * 4097), (b'W2CS', b'W' * 132), (b'WFCS', b'F' * 257))


class CalibrationTests(unittest.TestCase):
    def test_identifier_order_and_variable_sizes_do_not_depend_on_record_order(self):
        first = factory(RECORDS)
        second = factory(tuple(reversed(RECORDS)))
        self.assertEqual(factory_components(first), dict(RECORDS))
        cal = extract_calibration(first)
        self.assertEqual(cal, extract_calibration(second))
        self.assertEqual(struct.unpack_from('<I', cal)[0], 3)
        self.assertEqual(cal[4:64], bytes(60))
        cursor = 256
        for index, value in enumerate((b'W' * 132, b'F' * 257, b'B' * 4097)):
            descriptor = 64 * (index + 1)
            self.assertEqual(struct.unpack_from('<II', cal, descriptor + 8),
                             (cursor, len(value)))
            self.assertEqual(cal[descriptor:descriptor + 8], bytes(8))
            self.assertEqual(cal[descriptor + 16:descriptor + 64], bytes(48))
            self.assertEqual(cal[cursor:cursor + len(value)], value)
            cursor += len(value)
        self.assertEqual(cursor, len(cal))

    def test_component_identity_selector_and_nonempty_data_are_required(self):
        invalid = (RECORDS[:-1], RECORDS + (RECORDS[0],),
                   RECORDS + ((b'EXTR', b'other'),),
                   ((b'BTCS', b''),) + RECORDS[1:])
        for records in invalid:
            with self.subTest(records=[name for name, _ in records]):
                with self.assertRaises(ValueError):
                    extract_calibration(factory(records))
        with self.assertRaisesRegex(ValueError, 'record string'):
            extract_calibration(factory(RECORDS, selector=b'another'))

    def test_other_image_types_and_trailing_or_truncated_envelopes_are_rejected(self):
        for data in (factory(RECORDS, kind=b'code'), factory(RECORDS)[:-1],
                     factory(RECORDS) + b'\x05\0', b'not a factory file'):
            with self.assertRaises(ValueError):
                extract_calibration(data)

    def test_der_bounds_canonical_lengths_and_tags_are_enforced(self):
        for data in (b'\x04', b'\x04\x80\0\0', b'\x04\x81\x01X',
                     b'\x04\x82\0\x80' + bytes(128), b'\x04\x02X',
                     b'\xff', b'\xff\x80\x20\0', b'\xff\x1e\0',
                     b'\xff' + b'\x81' * 6, b'\x05\0' * 65):
            with self.subTest(data=data[:8]):
                with self.assertRaises(ValueError):
                    elements(data)
        self.assertEqual(bytes(elements(der(b'\x04', bytes(256)))[0].value), bytes(256))
        with patch('centauri_calibration.MAX_FACTORY_BYTES', 1):
            with self.assertRaisesRegex(ValueError, 'bounds'):
                extract_calibration(factory(RECORDS))

    def test_packager_needs_only_factory_file_and_independent_platform_inputs(self):
        from centauri_ftab import Directory
        from prepare_centauri_firmware import main
        from test_centauri_ftab import firmware
        with tempfile.TemporaryDirectory(prefix='centauri-factory-') as tmp:
            root = Path(tmp)
            bwc3 = factory(RECORDS)
            for name, value in (('vendor', firmware(b'opaque vendor')), ('manifest', b'signed'),
                    ('bwc3', bwc3), ('sku', b'Q8\0'), ('policy', b'opaque policy')):
                (root/name).write_bytes(value)
            arguments = ['prepare', '--vendor', str(root/'vendor'),
                '--manifest', str(root/'manifest'), '--antenna-sku-file', str(root/'sku'),
                '--calibration-factory-record', str(root/'bwc3'),
                '--ap-policy', str(root/'policy'), '--output', str(root/'output')]
            with patch.object(sys, 'argv', arguments), contextlib.redirect_stdout(io.StringIO()):
                main()
            result = Directory((root/'output').read_bytes())
            self.assertEqual(result.opaque_entry(b'_cal'), extract_calibration(bwc3))
            self.assertEqual(result.opaque_entry(b'2ftb'), b'opaque vendor')
            self.assertEqual(result.opaque_entry(b'nvrm')[40:], b'Q8')


if __name__ == '__main__':
    unittest.main()
