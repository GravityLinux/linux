# SPDX-License-Identifier: GPL-2.0-only
import struct
import unittest
from centauri_ap_policy import make_ap_policy


def properties():
    return dict(wifi_power_bo_2g_table=[1, 2, 3, 4, 5, 6],
        wifi_power_bo_2g_temp_threshold=51, wifi_power_bo_5g_table=[7, 8, 9, 10, 11, 12],
        wifi_power_bo_5g_temp_threshold=61, wifi_duty_cycle_table=[91, 81, 71, 61, 51, 41],
        wifi_duty_cycle_temp_threshold=71, wifi_volt_lo_threshold=0x40000000,
        wifi_volt_hi_threshold=0x40800000)


class PolicyTests(unittest.TestCase):
    def test_tables_thresholds_raw_voltage_and_tlv_boundaries(self):
        result = make_ap_policy(properties(), record4=b'test')
        self.assertEqual(len(result), 512)
        self.assertEqual(result[:2], bytes((1, 31)))
        self.assertEqual(result[2:9], bytes((1, 2, 3, 4, 5, 6, 51)))
        self.assertEqual(result[9:16], bytes((7, 8, 9, 10, 11, 12, 61)))
        self.assertEqual(result[16:23], bytes((91, 81, 71, 61, 51, 41, 71)))
        self.assertEqual(result[23:31], struct.pack('<II', 0x40000000, 0x40800000))
        self.assertEqual(result[31:42], bytes((2, 3, 1, 4, 6)) + b'test' + bytes((255, 2)))
        self.assertEqual(result[42:], bytes(470))

    def test_record4_is_explicit_and_only_changes_its_own_value(self):
        first = make_ap_policy(properties(), record4=b'aaaa')
        second = make_ap_policy(properties(), record4=b'bbbb')
        self.assertEqual([i for i, (a, b) in enumerate(zip(first, second)) if a != b],
                         [36, 37, 38, 39])
        for record in (None, b'', b'123', b'12345'):
            with self.assertRaises(ValueError):
                make_ap_policy(properties(), record4=record)

    def test_missing_or_out_of_range_data_is_never_silently_truncated(self):
        for name, value in (('wifi_power_bo_2g_table', [0] * 5),
                ('wifi_duty_cycle_table', [0, 0, 0, 0, 0, 256]),
                ('wifi_power_bo_5g_temp_threshold', 256), ('wifi_volt_lo_threshold', -1)):
            with self.assertRaises(ValueError):
                make_ap_policy({**properties(), name: value}, record4=b'test')
        with self.assertRaises(KeyError):
            make_ap_policy({}, record4=b'test')


if __name__ == '__main__':
    unittest.main()
