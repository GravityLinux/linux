# SPDX-License-Identifier: GPL-2.0-only
"""Compare Wi-Fi queue encodings with actual bounded reference-59 records."""
import struct
import unittest
from centauri_wifi import (completion_queue_config, submission_queue_config,
                           make_wifi_context, control_request, decode_control_header)


class WiFiQueueWireTests(unittest.TestCase):
    def test_completion_record_matches_reference_59(self):
        captured = (0x2, 0x60006, 0x43a0000, 0, 0xffff0400, 0xffff,
                    0, 2, 0, 0, 0)
        self.assertEqual(completion_queue_config(6, 0x43a0000, 1024),
                         struct.pack('<11I', *captured))

    def test_submission_record_matches_reference_59(self):
        captured = (0x1, 0x20002, 0x43d8000, 0, 0xffffffff, 0xffffffff,
                    0x2009a, 0x80000, 0, 2, 0, 0, 0)
        self.assertEqual(submission_queue_config(2, 0x43d8000, 154),
                         struct.pack('<13I', *captured))

    def test_out_of_range_fields_do_not_silently_truncate(self):
        for builder, maximum in ((completion_queue_config, 234), (submission_queue_config, 192)):
            for queue, iova, entries in ((-1, 0xf000000, 16), (maximum, 0xf000000, 16),
                                          (1, 0, 16), (1, 1 << 64, 16),
                                          (1, 0xf000000, 0), (1, 0xf000000, 0x10000)):
                with self.subTest(builder=builder.__name__, queue=queue, iova=iova, entries=entries):
                    with self.assertRaises(ValueError):
                        builder(queue, iova, entries)


class WiFiContextAndMessageTests(unittest.TestCase):
    def test_fresh_context_has_generated_header_and_zero_indices(self):
        words = [0x680300, 0, 0x3cf8068, 0, 0x3cf8078, 0, 0x3cf824c, 0,
                 0x3cf83cc, 0, 0x3cf85a0, 0, 0xc000ea, 0x3cf8820, 0,
                 0x3cf8720, 0, 0x100010, 0, 0, 0, 0, 0x3cf8920, 0,
                 0x500000, 0, 2, 2, 0, 0]
        # Independent hardware experiment 222 qualified zero at +0x48;
        # the older reference-header fixture retained 0xffff0000 there.
        context = make_wifi_context(0x3cf8000)
        self.assertEqual(context[:0x78], struct.pack('<30I', *words))
        self.assertEqual(context[0x78:], bytes(0x960 - 0x78))

    def test_first_control_request_and_reply_header_match_reference_61(self):
        self.assertEqual(control_request(8), struct.pack('<6I', 0, 0, 0, 0, 0x10018, 8))
        raw = bytes.fromhex('41180100548e1e0000000000000000005202010008000000')
        decoded = decode_control_header(raw, received_size=594)
        self.assertEqual((decoded['length'], decoded['transaction'], decoded['command']), (594, 1, 8))
        self.assertEqual(decoded['prefix_words'], (0x11841, 0x1e8e54, 0, 0))
        with self.assertRaises(ValueError):
            decode_control_header(raw, received_size=593)
        with self.assertRaises(ValueError):
            decode_control_header(raw[:23])

    def test_reference_66_transaction_increments_and_is_echoed(self):
        # The fiftieth request repeats command 8, now with transaction 50.
        request = control_request(8, transaction=50)
        self.assertEqual(struct.unpack('<6I', request), (0, 0, 0, 0, 0x320018, 8))
        self.assertEqual(decode_control_header(request)['transaction'], 50)
        for transaction in (-1, 0x10000):
            with self.assertRaises(ValueError):
                control_request(8, transaction=transaction)


if __name__ == '__main__':
    unittest.main()
