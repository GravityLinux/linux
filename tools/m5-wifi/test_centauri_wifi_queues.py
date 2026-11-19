# SPDX-License-Identifier: GPL-2.0-only
"""Actual completion vectors from bounded Wi-Fi captures."""
import struct
import unittest
from types import SimpleNamespace
from unittest.mock import Mock
from centauri_wifi_queues import (decode_completion, WiFiBasicQueues, PROFILES,
                                  DEVICE_COMPLETION_INDICES, ManagementCommandError)


class WiFiCompletionTests(unittest.TestCase):
    def test_runtime_receive_completion_identifies_source_length_cookie(self):
        record = struct.pack('<4I', 0x60401, 0x1fdc0000, 0, 0)
        self.assertEqual(decode_completion(record),
                         dict(source=6, bytes_completed=8156, cookie=0, extra_words=(0, 0)))

    def test_admin_completion_uses_source_zero(self):
        record = struct.pack('<4I', 0x401, 0x2c0002, 0, 0)
        self.assertEqual(decode_completion(record),
                         dict(source=0, bytes_completed=44, cookie=2, extra_words=(0, 0)))

    def test_unrecognized_format_or_extent_is_not_a_packet(self):
        for raw in (bytes(16), bytes(15), bytes(32)):
            with self.assertRaises(ValueError):
                decode_completion(raw)

    def make_pending_receive(self):
        """One firmware completion, with bytes outside its extent unreadable."""
        queues = WiFiBasicQueues.__new__(WiFiBasicQueues)
        completion = struct.pack('<4I', 0x30401, 3 << 16, 0, 0)
        order = []

        def read(address, size):
            order.append(('read', address, size))
            if address == 0x3000 and size == 16:
                return completion
            if address == 0x8000 and 0 < size <= 3:
                return b'abc'[:size]
            raise AssertionError('Read outside the completed owned buffer')

        def write(address, data):
            order.append(('write', address, len(data)))

        queues.u = SimpleNamespace(iface=SimpleNamespace(readmem=read, writemem=write))
        queues.p = Mock()
        queues.control = SimpleNamespace(_require_ready=lambda: None)
        queues.queues = {n: dict(cursor=0, producer=0, pool_physical=0x8000) for n in PROFILES}
        queues.rings = {(2, n): dict(physical=n * 0x1000, entries=PROFILES[n].completion_entries)
                        for n in PROFILES}
        queues._index = lambda offset, limit: int(offset == DEVICE_COMPLETION_INDICES + 6)
        queues._set_index = Mock()
        queues._notify = Mock()
        queues.post_receive = Mock()
        queues.completion_counts = {n: 0 for n in PROFILES}
        queues.receive_observers = {}
        queues.control_event_observers = {}
        queues.received_headers = []
        return queues, order

    def test_receive_observer_stays_in_completion_extent_before_recycling(self):
        queues, order = self.make_pending_receive()
        received = []
        queues.observe_receive(3, 64, lambda metadata, prefix: received.append((metadata, prefix)))
        queues.poll()
        self.assertEqual(received[0][1], b'abc')
        self.assertEqual(received[0][0]['queue'], 3)
        self.assertLess(order.index(('read', 0x8000, 3)), order.index(('write', 0x3000, 16)))
        self.assertEqual(queues.completion_counts[3], 1)

    def test_explicit_scan_reply_alias_still_matches_transaction(self):
        queues = WiFiBasicQueues.__new__(WiFiBasicQueues)
        queues.received_headers = []
        queues.next_transaction = 7
        queues.poll = Mock()
        queues.send_control = Mock(return_value=7)
        reads = []

        def poll():
            reads.append(queues.read_body_for)
            queues.received_headers.extend([
                dict(command=0x4001000f, transaction=6),
                dict(command=0x80000005, transaction=7),
                dict(command=0x4001000f, transaction=7, prefix_words=(0, 0, 0, 0)),
            ])

        queues.read_body_for = None
        calls = [0]

        def dispatch():
            calls[0] += 1
            if calls[0] > 1:
                poll()

        queues.poll.side_effect = dispatch
        reply = queues.query(0x1000f, b'generated', read_body=True,
                             reply_command=0x4001000f)
        self.assertEqual(reply, dict(command=0x4001000f, transaction=7, prefix_words=(0, 0, 0, 0)))
        self.assertEqual(reads, [(7, 0x4001000f)])
        self.assertIsNone(queues.read_body_for)

    def test_rejected_async_request_is_reported_without_a_timeout(self):
        queues = WiFiBasicQueues.__new__(WiFiBasicQueues)
        queues.received_headers = []
        queues.next_transaction = 14
        queues.read_body_for = None
        queues.send_control = Mock(return_value=14)
        calls = [0]
        rejected = dict(command=0x1000f, transaction=14, prefix_words=(0, 0, 0, 0x1510000))

        def poll():
            calls[0] += 1
            if calls[0] > 1:
                queues.received_headers.append(rejected)

        queues.poll = poll
        with self.assertRaises(ManagementCommandError) as caught:
            queues.query(0x1000f, b'generated', reply_command=0x4001000f)
        self.assertEqual(caught.exception.header, rejected)
        self.assertIsNone(queues.read_body_for)
        # Connection rejection can carry the asynchronous reply command too.
        queues.received_headers.clear()
        calls[0] = 0
        rejected['command'] = 0x4001000f
        with self.assertRaises(ManagementCommandError) as caught:
            queues.query(0x1000f, b'generated', reply_command=0x4001000f)
        self.assertEqual(caught.exception.header, rejected)
        self.assertIsNone(queues.read_body_for)

    def test_failed_observer_keeps_the_completion_available_for_retry(self):
        queues, order = self.make_pending_receive()

        def fail(metadata, prefix):
            raise RuntimeError('observer failed')

        queues.observe_receive(3, 2, fail)
        with self.assertRaisesRegex(RuntimeError, 'observer failed'):
            queues.poll()
        self.assertEqual(queues.queues[3]['cursor'], 0)
        self.assertNotIn(('write', 0x3000, 16), order)
        queues.observe_receive(3, 0, None)
        queues.poll()
        self.assertEqual(queues.completion_counts[3], 1)


if __name__ == '__main__':
    unittest.main()
