# SPDX-License-Identifier: GPL-2.0-only
"""Check that power tracing reads only selected GPIO-key payloads."""

import unittest
from types import SimpleNamespace as NS

from m1n1.fw.smc import SMCWriteKey, SMCResult
from m1n1.trace.asc import DIR
from m1n1.utils import Register64
from trace_centauri_power import CentauriPowerEndpoint


class PowerCaptureTests(unittest.TestCase):
    def test_only_adt_selected_key_payload_is_read(self):
        events, reads = [], []

        def read(address, size):
            reads.append((address, size))
            return b"\x01\x00\x80\x00"

        tracer = NS(hv=NS(iface=NS(readmem=read)),
                    capture=NS(emit=lambda kind, **data: events.append((kind, data))),
                    keys={int.from_bytes(b"gP13", "big")})
        ep = CentauriPowerEndpoint(tracer, 0x20)
        ep.reply(Register64(0x38de74000))
        ep.write_key(SMCWriteKey(KEY=int.from_bytes(b"NOPE", "big"), ID=1, SIZE=4))
        self.assertEqual(reads, [])
        ep.write_key(SMCWriteKey(KEY=int.from_bytes(b"gP13", "big"), ID=2, SIZE=4))
        self.assertEqual(reads, [(0x38de74000, 4)])
        self.assertEqual(events[-1][1]["key"], "gP13")
        self.assertEqual(events[-1][1]["value"], "0x800001")
        response = SMCResult(RESULT=0, ID=2, SIZE=4, VALUE=1).value | 0x100
        self.assertTrue(ep.handle_msg(DIR.RX, Register64(response), None))
        self.assertEqual(events[-1][0], "smc_power_reply")
        self.assertEqual(events[-1][1]["key"], "gP13")


if __name__ == "__main__":
    unittest.main()
