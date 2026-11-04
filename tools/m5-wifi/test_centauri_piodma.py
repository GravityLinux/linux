# SPDX-License-Identifier: GPL-2.0-only
"""Actual generated hardware requests and bounded configuration decoding."""
import struct
import unittest

from centauri_piodma import config_read, config_write, decode_config_descriptor


class PIOConfigurationTests(unittest.TestCase):
    def test_owned122_read_vendor_device_id(self):
        # Actual request returned1901106b using independently allocated RAM.
        observed = struct.pack('<4I', 0x4c007, 0xcb01000, 0xe00030, 4)
        self.assertEqual(config_read(0x1cb0100000, 0xe000300), observed)
        self.assertEqual(decode_config_descriptor(observed),
                         dict(direction='read', source=0xcb0100000,
                              destination=0xe000300, size=4))

    def test_owned123_reversible_intx_mask_write(self):
        # Actual writes/readbacks406 ->6 ->406. Low destination nibble4
        # occupies bits63:60, outside the source's36-bit field.
        observed = struct.pack('<QQ', 0x400e00030020c007, 0x2cb010000)
        self.assertEqual(config_write(0xe000300, 0x1cb0100004, 2), observed)
        self.assertEqual(decode_config_descriptor(observed),
                         dict(direction='write', source=0xe000300,
                              destination=0xcb0100004, size=2))

    def test_reject_unqualified_profiles_and_extents(self):
        for data in (bytes(15), bytes(17), bytes(16),
                     struct.pack('<QQ', 0x4c007, 128 << 32)):
            with self.assertRaises(ValueError):
                decode_config_descriptor(data)
        for function, args in ((config_write, (1 << 36, 0, 4)),
                               (config_write, (-1, 0, 4)),
                               (config_read, (0, 1 << 36, 4)),
                               (config_read, (0, (1 << 36) - 1, 4)),
                               (config_read, (3, 0, 4)),
                               (config_write, (0, 0, 0))):
            with self.assertRaises(ValueError):
                function(*args)


if __name__ == '__main__':
    unittest.main()
