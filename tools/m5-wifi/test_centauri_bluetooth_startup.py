# SPDX-License-Identifier: GPL-2.0-only
"""Startup probing must not overwrite pending commands or reuse unknown bytes."""
import unittest
from unittest.mock import patch
from centauri_bluetooth import probe_radio_initialize


class Transport:
    vendor_probe_opcode = None
    host_indices, device_indices = 0x10, 0x20
    queues = {1: dict(entries=2, capacity=260, stride=276, ring=0x4000)}
    producer, consumer = 1, 1

    def __init__(self):
        self.writes = []

    def index(self, vector, number):
        return self.producer if vector == self.host_indices else self.consumer

    def write(self, address, data):
        self.writes.append((address, data))


class StartupTests(unittest.TestCase):
    def test_pending_or_invalid_cursor_never_writes(self):
        for producer, consumer in [(1, 0), (2, 2)]:
            transport = Transport()
            transport.producer, transport.consumer = producer, consumer
            with self.assertRaises(RuntimeError):
                probe_radio_initialize(transport)
            self.assertEqual(transport.writes, [])

    def test_empty_command_uses_zeroed_inactive_body(self):
        transport = Transport()
        def complete(owner, opcode):
            self.assertIs(owner, transport)
            self.assertEqual(opcode, 0xff0b)
            self.assertEqual(owner.vendor_probe_opcode, opcode)
            self.assertEqual(owner.writes, [(0x4000 + 276 + 16, bytes(260))])
            return b''
        with patch('centauri_bluetooth.command_complete', side_effect=complete):
            self.assertEqual(probe_radio_initialize(transport), 0)
        self.assertIsNone(transport.vendor_probe_opcode)

    def test_command_failure_closes_permission(self):
        transport = Transport()
        with patch('centauri_bluetooth.command_complete', side_effect=TimeoutError):
            with self.assertRaises(TimeoutError):
                probe_radio_initialize(transport)
        self.assertIsNone(transport.vendor_probe_opcode)


if __name__ == '__main__':
    unittest.main()
