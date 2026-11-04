# SPDX-License-Identifier: GPL-2.0-only
"""Runtime query vectors from reference-66 and owned target-67."""
import struct
import unittest
from centauri_wifi_messages import (decode_tlvs, decode_country_reply, country_setting,
                                    decode_channel_list_reply, address_template,
                                    interface_operation_body, decode_interface_info)


class WiFiMessageTests(unittest.TestCase):
    def test_initial_parameter_prefix_has_packed_tlvs(self):
        raw = bytes.fromhex('09fe04000800000008fe0400000000000bfe040000010000')
        self.assertEqual(decode_tlvs(raw), [(0xfe09, struct.pack('<I', 8)),
                         (0xfe08, bytes(4)), (0xfe0b, struct.pack('<I', 0x100))])
        for invalid in (raw[:-1], raw + b'\x01'):
            with self.assertRaises(ValueError):
                decode_tlvs(invalid)

    def test_country_changes_after_reference_configuration(self):
        self.assertEqual(decode_country_reply(bytes.fromhex('585a00')),
                         dict(country_code='XZ', extra_byte=0))
        self.assertEqual(decode_country_reply(bytes.fromhex('555301')),
                         dict(country_code='US', extra_byte=1))
        with self.assertRaises(ValueError):
            decode_country_reply(b'US')
        self.assertEqual(country_setting(b'US', 1), bytes.fromhex('555301'))

    def test_channel_count_must_fit_before_trailing_area(self):
        with self.assertRaises(ValueError):
            decode_channel_list_reply(b'\xa7' + bytes(4))
        with self.assertRaises(ValueError):
            decode_channel_list_reply(b'')

    def test_observed_interface_operation_wire_vectors(self):
        self.assertEqual(interface_operation_body(0, 11), bytes.fromhex('000b000000'))
        self.assertEqual(interface_operation_body(0, 0), bytes(5))
        self.assertEqual(interface_operation_body(3, 0x12345678), bytes.fromhex('0378563412'))
        for selector, options in ((256, 0), (-1, 0), (0, 1 << 32), (0, -1)):
            with self.assertRaises(ValueError):
                interface_operation_body(selector, options)

    def test_interface_info_distinguishes_inactive_and_active_with_address(self):
        address = bytes.fromhex('021122334455')
        inactive = decode_interface_info(b'\0\0' + address)
        self.assertEqual(inactive, dict(selector_byte=0, state=0, address=address, tlvs=[]))
        payload = bytes(range(26))
        wire = b'\0\2' + address + struct.pack('<HH', 0x17d, len(payload)) + payload
        active = decode_interface_info(wire)
        self.assertEqual(active, dict(selector_byte=0, state=2, address=address,
                                      tlvs=[(0x17d, payload)]))
        for bad in (wire[:7], wire[:-1]):
            with self.assertRaises(ValueError):
                decode_interface_info(bad)

    def test_address_template_requires_a_real_unicast_address(self):
        for address in (bytes.fromhex('021122334455'), bytes.fromhex('001122334455')):
            self.assertEqual(address_template(address), address)
        for address in (bytes(6), bytes([255]) * 6, bytes.fromhex('011122334455'), bytes(5)):
            with self.assertRaises(ValueError):
                address_template(address)


if __name__ == '__main__':
    unittest.main()
