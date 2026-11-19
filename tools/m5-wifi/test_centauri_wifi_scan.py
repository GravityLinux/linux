# SPDX-License-Identifier: GPL-2.0-only
"""Scan completion correlation and bounded standard radio/frame decoding."""
import struct
import unittest
from centauri_wifi_scan import scan_request, decode_scan_complete, decode_scan_receive


def radio_report(ssid=b'test-network', *, fcs=False, extra_radio=0):
    header = struct.pack('<BBHIQBBHHbb', 0, 0, 24 + extra_radio, 0x6f,
                         1234, 0x10 if fcs else 0, 2, 2412, 0x80, -59, -89)
    address = bytes.fromhex('021122334455')
    frame = struct.pack('<HH', 0x50, 0) + b'\xff' * 6 + address * 2 + bytes(2)
    frame += struct.pack('<QHH', 4567, 100, 0x411)
    frame += bytes((0, len(ssid))) + ssid + b'\x03\x01\x01'
    return header + bytes(extra_radio) + frame + (bytes(4) if fcs else b'')


class ScanTests(unittest.TestCase):
    def test_minimal_generated_channel_request_has_no_retained_options(self):
        body = scan_request(8, [1, 6, 11], report_frames=True,
                            option_words=(0, 0), timing_words=(20, 20, 110))
        self.assertEqual(body[:14], bytes.fromhex('0800008000000000140014006e00'))
        self.assertEqual(body[14:18], bytes.fromhex('04010d00'))
        self.assertEqual(body[18:], struct.pack('<BIII', 3, 1, 6, 11))
        for cookie, channels in ((256, [1]), (0, []), (1, [1 << 32])):
            with self.assertRaises(ValueError):
                scan_request(cookie, channels, report_frames=True,
                             option_words=(0, 0), timing_words=(20, 20, 110))

    def test_completion_echo_is_distinct_from_management_transaction(self):
        self.assertEqual(decode_scan_complete(bytes.fromhex('000007001a00000008')),
                         dict(status=0, frames=7, elapsed_ms=26, cookie=8))
        for body in (bytes(8), bytes(10)):
            with self.assertRaises(ValueError):
                decode_scan_complete(body)

    def test_radio_length_locates_frame_and_signed_signal(self):
        for extra_radio in (0, 72):
            result = decode_scan_receive(radio_report(extra_radio=extra_radio))
            self.assertEqual(result['radiotap_bytes'], 24 + extra_radio)
            self.assertEqual(result['ssid'], b'test-network')
            self.assertEqual(result['bssid'].hex(), '021122334455')
            self.assertEqual((result['frequency_mhz'], result['signal_dbm'], result['noise_dbm']),
                             (2412, -59, -89))
            self.assertEqual((result['channel'], result['kind']), (1, 'probe_response'))

    def test_fcs_is_removed_only_when_radio_flags_indicate_it(self):
        self.assertEqual(decode_scan_receive(radio_report(fcs=True))['ssid'], b'test-network')
        for malformed in (radio_report()[:-1], radio_report() + bytes(4),
                          radio_report()[:7], b'\x01' + radio_report()[1:]):
            with self.assertRaises(ValueError):
                decode_scan_receive(malformed)

    def test_radio_field_cannot_cross_declared_header(self):
        body = bytearray(radio_report())
        struct.pack_into('<H', body, 2, 22)
        with self.assertRaises(ValueError):
            decode_scan_receive(body)


if __name__ == '__main__':
    unittest.main()
