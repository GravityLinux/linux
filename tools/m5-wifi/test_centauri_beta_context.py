# SPDX-License-Identifier: GPL-2.0-only
"""Check sibling geometry with the independently qualified zero profile."""
import struct
import unittest
from centauri_ipc import make_beta_context


class BetaContextTests(unittest.TestCase):
    def test_reference_geometry_with_qualified_zero_profile(self):
        # Reference53 geometry, with header+0x48 zero as qualified by226.
        words = [
            0x680300, 0, 0x5020068, 0, 0x5020078, 0, 0x502007a, 0,
            0x50200a6, 0, 0x50200a8, 0, 0x160001, 0x50201d4, 0,
            0x50200d4, 0, 0x100010, 0, 0, 0x4000000, 0,
            0x50203d8, 0, 0, 0, 2, 2, 0, 0,
        ] + [0] * 23
        self.assertEqual(make_beta_context(0x5020000), struct.pack('<53I', *words))


if __name__ == '__main__':
    unittest.main()
