# SPDX-License-Identifier: GPL-2.0-only
"""Controlled page probes must serialize fields and close/cancel owned links."""
import struct
import unittest
from centauri_bluetooth import probe_create_connection, HCI_COMMAND, HCI_EVENT

PEER = 'A0:A1:A2:A3:A4:A5'
ADDRESS = bytes.fromhex('A5A4A3A2A1A0')


class Controller:
    def __init__(self, mode):
        self.mode, self.sent, self.polls = mode, [], 0

    def send(self, kind, packet):
        assert kind == HCI_COMMAND
        self.sent.append(packet)
        return True

    def poll(self):
        self.polls += 1
        opcode, = struct.unpack_from('<H', self.sent[-1])
        if opcode == 0x0406:
            return [(HCI_EVENT, bytes([5, 4, 0, 1, 2, 0x13]))]
        if opcode == 0x0408:
            return [(HCI_EVENT, bytes([0x0e, 10, 1, 8, 4, 0]) + ADDRESS)]
        if self.polls == 1:
            status = 12 if self.mode == 'reject' else 0
            events = [(HCI_EVENT, bytes([0x0f, 4, status, 1, 5, 4]))]
            if self.mode == 'connect':
                events.append((HCI_EVENT, bytes([3, 11, 0, 1, 2]) + ADDRESS + bytes([0xf1, 0])))
            return events
        return []


class CreateProbeTests(unittest.TestCase):
    def test_rejected_command_has_exact_named_fields(self):
        controller = Controller('reject')
        result = probe_create_connection(controller, PEER, page_scan_repetition=1, allow_role_switch=0)
        self.assertEqual(result['command_status'], 12)
        self.assertEqual(len(controller.sent), 1)
        self.assertEqual(controller.sent[0], struct.pack('<HB6sHBBHB', 0x0405, 13, ADDRESS, 0xcc18, 1, 0, 0, 0))

    def test_successful_temporary_link_is_disconnected(self):
        controller = Controller('connect')
        result = probe_create_connection(controller, PEER)
        self.assertTrue(result['disconnected'])
        self.assertEqual(result['connection_complete']['controller_link_type'], 0xf1)
        self.assertEqual(controller.sent[-1], struct.pack('<HBHB', 0x0406, 3, 0x0201, 0x13))

    def test_accepted_page_is_cancelled_at_deadline(self):
        controller = Controller('pending')
        result = probe_create_connection(controller, PEER, timeout=.001)
        self.assertTrue(result['timed_out'])
        self.assertTrue(result['cancelled'])
        self.assertEqual(controller.sent[-1], struct.pack('<HB6s', 0x0408, 6, ADDRESS))


if __name__ == '__main__':
    unittest.main()
