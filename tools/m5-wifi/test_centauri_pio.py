# SPDX-License-Identifier: GPL-2.0-only
"""Bound source reads to the six configuration writes validated on reference-53."""
import struct
import unittest
from centauri_pio import decode_boot_buffer_config_write, encode_boot_buffer_config_write


class PIOConfigTests(unittest.TestCase):
    def test_all_observed_destination_tokens_and_source_bytes(self):
        for function in (1, 2):
            for offset, high in ((0xf88, 8), (0xf8c, 12), (0xf90, 0)):
                # Reference-51 command prefixes; reference-53 independently
                # validates the source against each programmed register value.
                prefix = struct.pack('<4I', 0x9020c007, high << 28 | 0x61d29,
                                     0xcb010000 | function << 8 | offset >> 4, 4)
                decoded = decode_boot_buffer_config_write(prefix)
                self.assertEqual((decoded.source_iova, decoded.function, decoded.offset, decoded.length),
                                 (0x061d2990, function, offset, 4))
        prefix = struct.pack('<4I', 0x1020c007, 0x8005021c, 0xcb0101f8, 4)
        self.assertEqual(decode_boot_buffer_config_write(prefix).source_iova, 0x05021c10)

    def test_unobserved_commands_cannot_authorize_payload_reads(self):
        words = [0x9020c007, 0x80061d29, 0xcb0101f8, 4]
        for index, value in ((0, 0x9020c006), (1, 0x81061d29),
                             (2, 0xcb0101f9), (2, 0xcb0103f8), (3, 32)):
            changed = words.copy()
            changed[index] = value
            self.assertIsNone(decode_boot_buffer_config_write(struct.pack('<4I', *changed)))
        for source in (0, 1, 0xfffffffe):
            changed = words.copy()
            changed[0] = (source & 255) << 24 | 0x20c007
            changed[1] = 0x80000000 | source >> 8
            self.assertIsNone(decode_boot_buffer_config_write(struct.pack('<4I', *changed)))

    def test_encoder_matches_captured_words_and_owned_hardware_command(self):
        expected = struct.pack('<4I', 0x9020c007, 0x80061d29, 0xcb0101f8, 4)
        self.assertEqual(encode_boot_buffer_config_write(1, 0xf88, 0x061d2990), expected)
        # Target-54: own source at 0x40200, eight completed writes and a
        # changed sibling-length readback followed by restoration.
        expected_owned = struct.pack('<4I', 0x0020c007, 0x00000402, 0xcb0102f9, 4)
        self.assertEqual(encode_boot_buffer_config_write(2, 0xf90, 0x40200), expected_owned)
        for function, offset, source in ((3, 0xf88, 0x400), (1, 0xf80, 0x400),
                                         (1, 0xf88, 0), (1, 0xf88, 1),
                                         (1, 0xf88, 1 << 32)):
            with self.assertRaises(ValueError):
                encode_boot_buffer_config_write(function, offset, source)

    def test_prefix_extent_is_exact(self):
        for size in (0, 12, 15, 17, 32):
            with self.assertRaises(ValueError):
                decode_boot_buffer_config_write(bytes(size))


if __name__ == '__main__':
    unittest.main()
