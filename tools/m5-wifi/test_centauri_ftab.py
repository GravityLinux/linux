# SPDX-License-Identifier: GPL-2.0-only
"""Directory bounds and allocation provenance, using synthetic metadata only."""
import struct
import unittest

from centauri_ftab import Directory, make_boot_allocation, prepare_image, DEVICE_INPUT_TAGS


def header(count, kind=0):
    return struct.pack('<8I8sII', 0, 1, kind, 0, 0, 0, 0, 0, b'rkosftab', count, 0)


def firmware(template):
    return header(1) + struct.pack('<4sIII', b'2ftb', 64, len(template), 0) + template


class FtabTests(unittest.TestCase):
    def test_firmware_layout_generates_directory_and_zero_regions(self):
        template = (header(2, 0x42) + struct.pack('<4sIII', b'test', 0x100, 0x20, 0) +
                    struct.pack('<4sIII', b'next', 0x200, 0x30, 0))
        allocation = make_boot_allocation(firmware(template))
        self.assertEqual(len(allocation), 0x230)
        self.assertEqual(allocation[:80], template)
        self.assertEqual(allocation[80:], bytes(0x230 - 80))

    def test_truncated_directory_or_external_payload_is_rejected(self):
        with self.assertRaises(ValueError):
            Directory(header(2) + bytes(16))
        with self.assertRaises(ValueError):
            Directory(header(1) + struct.pack('<4sIII', b'2ftb', 64, 80, 0))

    def test_unbounded_layout_refused_before_allocating_its_extent(self):
        template = header(1, 0x42) + struct.pack('<4sIII', b'test', 0xffffff00, 0x1000, 0)
        with self.assertRaisesRegex(ValueError, 'bounded prototype limit'):
            make_boot_allocation(firmware(template))

    def test_packaging_preserves_vendor_components_and_names_external_inputs(self):
        vendor = firmware(b'opaque vendor component')
        inputs = {tag: b'input:' + tag for tag in DEVICE_INPUT_TAGS}
        manifest = b'opaque signed manifest'
        result = prepare_image(vendor, manifest, inputs)
        original, prepared = Directory(vendor), Directory(result)
        self.assertEqual(len(result) % 4096, 0)
        self.assertEqual(prepared.opaque_entry(b'2ftb'), original.opaque_entry(b'2ftb'))
        offset, length = prepared.header_words[4:6]
        self.assertEqual(offset, prepared.table_end)
        self.assertEqual(result[offset:offset + length], manifest)
        for tag, data in inputs.items():
            self.assertEqual(prepared.opaque_entry(tag), data)
        end = max(e.offset + e.size for e in prepared.entries)
        self.assertEqual(result[end:], bytes(len(result) - end))
        with self.assertRaisesRegex(ValueError, 'unprepared'):
            prepare_image(result, manifest, inputs)

    def test_packaging_refuses_missing_empty_or_oversized_inputs(self):
        vendor = firmware(b'opaque')
        inputs = dict.fromkeys(DEVICE_INPUT_TAGS, b'input')
        with self.assertRaises(ValueError):
            prepare_image(vendor, b'', inputs)
        with self.assertRaises(ValueError):
            prepare_image(vendor, b'manifest', {b'nvrm': b'input'})
        with self.assertRaises(ValueError):
            prepare_image(vendor, b'manifest', {**inputs, b'_cal': b''})
        with self.assertRaisesRegex(ValueError, 'transfer limit'):
            prepare_image(vendor, b'manifest', {**inputs, b'_cal': bytes(32 * 1024 * 1024)})

    def test_optional_boot_parameters_are_absent_from_the_encoded_directory(self):
        inputs = dict.fromkeys(DEVICE_INPUT_TAGS[:3], b'input')
        result = Directory(prepare_image(firmware(b'opaque'), b'manifest', inputs))
        self.assertEqual({e.tag for e in result.entries}, {b'2ftb', *inputs})
        self.assertEqual(result.header_words[4], result.table_end)
        for tag, value in inputs.items():
            self.assertEqual(result.opaque_entry(tag), value)


if __name__ == '__main__':
    unittest.main()
