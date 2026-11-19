# SPDX-License-Identifier: GPL-2.0-only
"""Boundary vectors that catch the original connection security shift."""
import struct
import unittest
from centauri_wifi_connection import (set_connection_security, decode_connection_complete,
                                      AUTH_OPEN, AKM_SELECTOR_PSK, CIPHER_SELECTOR_CCMP,
                                      ConnectionParameters, psk_connection_request)


class ConnectionTests(unittest.TestCase):
    def test_native_security_boundary_does_not_shift_or_overwrite_neighbors(self):
        prefix = bytes([0xaa]) * 159
        key = bytes(range(32))
        result = set_connection_security(prefix, auth_selector=3, akm_selector=8,
                                         cipher_selector=4, flags=0, key=key)
        self.assertEqual(result[70:76], bytes.fromhex('030804002000'))
        self.assertEqual(result[76:108], key)
        self.assertEqual(result[:70], prefix[:70])
        self.assertEqual(result[108:], prefix[108:])
        self.assertEqual(struct.unpack_from('<H', result, 74)[0], 32)

    def test_native_psk_selector_differs_from_rsn_suite_type(self):
        result = set_connection_security(bytes(159), auth_selector=AUTH_OPEN,
            akm_selector=AKM_SELECTOR_PSK, cipher_selector=CIPHER_SELECTOR_CCMP,
            flags=0, key=bytes(32))
        self.assertEqual(result[70:76], bytes.fromhex('000404002000'))
        self.assertNotEqual(AKM_SELECTOR_PSK, 2)

    def test_only_qualified_prefix_and_key_extents_are_encoded(self):
        for prefix, key in ((bytes(158), bytes(32)), (bytes(159), bytes(31)),
                            (bytes(160), bytes(32))):
            with self.assertRaises(ValueError):
                set_connection_security(prefix, auth_selector=0, akm_selector=4,
                                        cipher_selector=4, flags=0, key=key)

    def test_pmk_only_request_omits_plaintext_and_preserves_fixed_field_boundaries(self):
        from centauri_wifi_messages import decode_tlvs
        parameters=ConnectionParameters(1,1,4,(2000,6000,6000,60000,300))
        bssid=bytes.fromhex('021122334455')
        rsn=bytes.fromhex('0100000fac040100000fac040100000fac020000')
        body=psk_connection_request(b'test',bssid,11,bytes(range(32)),parameters=parameters,rsn_ie=rsn)
        self.assertEqual(body[1:9],bssid+b'\x0b\0')
        self.assertEqual(body[70:76],bytes.fromhex('000404002000'))
        self.assertEqual(body[76:108],bytes(range(32)))
        self.assertEqual(struct.unpack_from('<H',body,132)[0],1)
        self.assertEqual(struct.unpack_from('<5I',body,139),parameters.timing_words)
        self.assertEqual(decode_tlvs(body[159:]),[(0x156,b'\1\4\4test'),(0x30,rsn)])

    def test_connection_outcome_is_separate_from_request_acceptance(self):
        ssid, bssid = b'test-network', bytes.fromhex('021122334455')
        for status in (0, 1):
            body = struct.pack('<32sI6sH', ssid, len(ssid), bssid, status) + bytes(46)
            result = decode_connection_complete(body)
            self.assertEqual((result['ssid'], result['bssid'], result['status']), (ssid, bssid, status))
            self.assertEqual(len(result['remaining']), 46)
        for body in (bytes(89), bytes(91), struct.pack('<32sI6sH', ssid, 33, bssid, 0) + bytes(46)):
            with self.assertRaises(ValueError):
                decode_connection_complete(body)


if __name__ == '__main__':
    unittest.main()
