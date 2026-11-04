# SPDX-License-Identifier: GPL-2.0-only
"""Classic key loading must not inherit a dual-mode peer's LE address type."""
import configparser
import struct
import unittest
from bluetooth_restore_classic_pair import classic_key_payload, load_key


class Socket:
    def __init__(self, events):
        self.events = iter(events)
    def send(self, packet):
        self.sent = packet
        return len(packet)
    def settimeout(self, seconds):
        pass
    def recv(self, size):
        return next(self.events)


class ClassicPairTests(unittest.TestCase):
    def test_dual_mode_storage_uses_classic_mgmt_type(self):
        info = configparser.ConfigParser()
        info.read_dict({'General': {'AddressType': 'public'}, 'LinkKey': {
            'Key': bytes(range(16)).hex(), 'Type': '4', 'PINLength': '0'}})
        payload = classic_key_payload('A0:A1:A2:A3:A4:A5', info)
        self.assertEqual(len(payload), 28)
        self.assertEqual(struct.unpack('<BH6sBB16sB', payload),
                         (0, 1, bytes.fromhex('a5a4a3a2a1a0'), 0, 4, bytes(range(16)), 0))
        info['LinkKey']['Key'] = '00'
        with self.assertRaises(ValueError):
            classic_key_payload('A0:A1:A2:A3:A4:A5', info)

    def test_completion_must_match_controller_and_command(self):
        sock = Socket([struct.pack('<HHHHB', 1, 1, 3, 0x12, 0),
                       struct.pack('<HHHHB', 1, 0, 3, 0x13, 0),
                       struct.pack('<HHHHB', 1, 0, 3, 0x12, 0)])
        load_key(sock, 0, b'test')
        self.assertEqual(sock.sent, struct.pack('<HHH', 0x12, 0, 4) + b'test')

    def test_kernel_error_is_not_success(self):
        sock = Socket([struct.pack('<HHHHB', 2, 0, 3, 0x12, 0x0d)])
        with self.assertRaisesRegex(RuntimeError, '0xd'):
            load_key(sock, 0, b'test')


if __name__ == '__main__':
    unittest.main()
