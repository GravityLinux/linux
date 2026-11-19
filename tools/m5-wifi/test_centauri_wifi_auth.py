# SPDX-License-Identifier: GPL-2.0-only
"""Bound the SAE method observation to its declared radio header and peer."""
import struct
import unittest
from centauri_wifi_scan import decode_sae_authentication


STATION = bytes.fromhex('021122334455')
PEER = bytes.fromhex('0266778899aa')


def report(status=126, *, transaction=1, radio_bytes=96, flags=0):
    radio = struct.pack('<BBHIB', 0, 0, radio_bytes, 1 << 1, flags)
    radio += bytes(radio_bytes - len(radio))
    frame = struct.pack('<HH', 0xb0, 0) + STATION + PEER * 2 + bytes(2)
    frame += struct.pack('<HHH', 3, transaction, status)
    frame += struct.pack('<H', 19) + b'opaque-synthetic-commit-data'
    return radio + frame + (bytes(4) if flags & 0x10 else b'')


class AuthenticationTests(unittest.TestCase):
    def decode(self, body):
        return decode_sae_authentication(body, station=STATION, bssid=PEER)

    def test_method_comes_from_commit_status_and_no_transcript_is_returned(self):
        for status, method in ((126, 'hash-to-element'), (0, 'hunt-and-peck')):
            result = self.decode(report(status))
            self.assertEqual(result, dict(radiotap_bytes=96, station_transmits=False,
                transaction=1, status=status, group=19, method=method))

    def test_confirm_and_token_request_do_not_claim_a_password_element_method(self):
        self.assertNotIn('method', self.decode(report(0, transaction=2)))
        self.assertNotIn('method', self.decode(report(76)))

    def test_peer_and_header_checks_prevent_false_method_observations(self):
        original = report()
        for offset, value in ((0, 1), (2, 0xff), (96, 0x80), (97, 0x40),
                              (100, 0), (106, 0), (112, 0), (118, 1), (120, 0)):
            altered = bytearray(original)
            altered[offset] = value
            with self.assertRaises(ValueError):
                self.decode(altered)
        for truncated in (original[:7], original[:96 + 29], original[:96 + 31]):
            with self.assertRaises(ValueError):
                self.decode(truncated)

    def test_bad_fcs_is_rejected_and_declared_radio_length_is_used(self):
        with self.assertRaises(ValueError):
            self.decode(report(flags=0x40))
        self.assertEqual(self.decode(report(flags=0x10))['method'], 'hash-to-element')
        self.assertEqual(self.decode(report(radio_bytes=24))['radiotap_bytes'], 24)


if __name__ == '__main__':
    unittest.main()
