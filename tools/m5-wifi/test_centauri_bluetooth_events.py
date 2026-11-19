# SPDX-License-Identifier: GPL-2.0-only
"""The N1 ACL quirk must not reinterpret unknown or unrelated HCI fields."""
import unittest
from centauri_bluetooth import linux_hci_event


def connection(link_type, status=0):
    return bytes([3, 11, status, 1, 0, 1, 2, 3, 4, 5, 6, link_type, 0])


class ConnectionTypeTests(unittest.TestCase):
    def test_only_observed_controller_value_maps_to_acl(self):
        packet = connection(0xf1)
        self.assertEqual(linux_hci_event(packet, translate_vendor_acl=True), connection(1))
        self.assertEqual(linux_hci_event(packet), packet)
        self.assertEqual(linux_hci_event(connection(0xf0), translate_vendor_acl=True), connection(1))
        for kind in (0, 1, 2):
            self.assertEqual(linux_hci_event(connection(kind), translate_vendor_acl=True), connection(kind))

    def test_unknown_success_code_is_not_masked(self):
        for kind in (0xe1, 0xf2, 0xff):
            with self.assertRaises(ValueError):
                linux_hci_event(connection(kind), translate_vendor_acl=True)

    def test_failure_and_other_event_fields_are_unchanged(self):
        failed = connection(0xe1, status=4)
        unrelated = bytes([0xff]) + connection(0xf1)[1:]
        for packet in (failed, unrelated, b''):
            self.assertEqual(linux_hci_event(packet, translate_vendor_acl=True), packet)


if __name__ == '__main__':
    unittest.main()
