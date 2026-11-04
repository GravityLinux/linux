# SPDX-License-Identifier: GPL-2.0-only
"""Reset setup must preserve kernel sequencing and unrelated traffic."""
import struct
import unittest
from unittest.mock import patch
from centauri_bluetooth import ClassicCreateSetup, HCI_COMMAND, HCI_EVENT, HCI_ACL


def complete(opcode, status=0):
    return b'\x0e\x04\x01' + struct.pack('<H', opcode) + bytes([status])


class Transport:
    vendor_probe_opcode = None
    accepted = True

    def send(self, kind, packet):
        self.last = kind, packet, self.vendor_probe_opcode
        return self.accepted


class ResetSetupTests(unittest.TestCase):
    def test_holds_reset_until_owned_setup_and_preserves_other_packets(self):
        setup, transport = ClassicCreateSetup(), Transport()
        reset = complete(0x0c03)
        self.assertEqual(setup.receive(HCI_EVENT, reset), [])
        self.assertTrue(setup.pending)
        for kind, body in [(HCI_ACL, b'payload'), (HCI_EVENT, b'\x05\x04\x00\x01\x00\x13'),
                           (HCI_EVENT, complete(0x1009))]:
            self.assertEqual(setup.receive(kind, body), [(kind, body)])
        setup.submit(transport)
        self.assertEqual(transport.last, (HCI_COMMAND, b'\x77\xff\x01\x00', 0xff77))
        self.assertIsNone(transport.vendor_probe_opcode)
        self.assertEqual(setup.receive(HCI_EVENT, complete(0xff77)), [(HCI_EVENT, reset)])
        self.assertFalse(setup.pending)
        self.assertEqual(setup.receive(HCI_EVENT, complete(0xff77)), [(HCI_EVENT, complete(0xff77))])
        self.assertEqual(setup.receive(HCI_EVENT, reset), [])  # Every reset reapplies setup.

    def test_backpressure_retries_and_permission_is_scoped(self):
        setup, transport = ClassicCreateSetup(), Transport()
        setup.receive(HCI_EVENT, complete(0x0c03))
        transport.accepted = False
        transport.vendor_probe_opcode = 0xfe44
        setup.submit(transport)
        self.assertFalse(setup.sent)
        self.assertEqual(transport.vendor_probe_opcode, 0xfe44)
        transport.accepted = True
        setup.submit(transport)
        self.assertTrue(setup.sent)
        with patch('centauri_bluetooth.time.monotonic', return_value=setup.started + 4):
            with self.assertRaises(TimeoutError):
                setup.submit(transport)

    def test_rejected_setup_and_overlapping_reset_fail(self):
        for reply in [complete(0xff77, 0x0c), b'\x0f\x04\x0c\x01\x77\xff']:
            setup = ClassicCreateSetup()
            setup.receive(HCI_EVENT, complete(0x0c03))
            setup.submit(Transport())
            with self.assertRaises(RuntimeError):
                setup.receive(HCI_EVENT, reply)
        setup = ClassicCreateSetup()
        setup.receive(HCI_EVENT, complete(0x0c03))
        with self.assertRaises(RuntimeError):
            setup.receive(HCI_EVENT, complete(0x0c03))
        failed = complete(0x0c03, 1)
        self.assertEqual(ClassicCreateSetup().receive(HCI_EVENT, failed), [(HCI_EVENT, failed)])


if __name__ == '__main__':
    unittest.main()
