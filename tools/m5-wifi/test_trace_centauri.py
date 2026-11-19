# SPDX-License-Identifier: GPL-2.0-only
"""Run with Gravity proxyclient on PYTHONPATH; no hardware is accessed."""

import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace as NS

from trace_centauri import CentauriTrace


class CaptureTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = Path(self.tmp.name) / "capture.jsonl"
        self.additions = []
        self.removals = []
        apcie = NS(get_reg=lambda i: (0x1cb0000000, 0x10000000),
                   ranges=[NS(bus_addr=[0x02000000, 0x80000000, 0],
                              parent_addr=0xb80000000, size=0x40000000)])
        hv = NS(adt={"/arm-io/apcie": apcie, "/chosen": NS(chip_id=0x8142)}, ctx=None,
                add_tracer=lambda *a, **kw: self.additions.append((a, kw)),
                clear_tracers=self.removals.append)
        self.trace = CentauriTrace(hv, self.path)

    def tearDown(self):
        self.trace.logfile.close()
        self.tmp.cleanup()

    def cfg(self, offset, value):
        self.trace.config_event(NS(addr=self.trace.ecam + (1 << 20) + offset,
                                   data=value, pc=0, flags=NS(WIDTH=2, WRITE=True,
                                                            CPU=6, MULTI=False)))

    def test_64bit_assignment_requires_both_halves_and_disable_removes_it(self):
        self.cfg(0x10, 0x80000004)
        self.assertFalse(self.additions)
        self.cfg(0x14, 0)
        self.assertEqual(self.trace.windows["centauri/01:00.0/bar0"], 0xb80000000)
        self.assertEqual(len(self.additions), 1)
        self.cfg(0x10, 0)
        self.assertIsNone(self.trace.windows["centauri/01:00.0/bar0"])

    def test_probe_mask_and_io_bar_do_not_map_memory(self):
        self.cfg(0x10, 0xfffffff0)
        self.cfg(0x14, 0x80000001)
        self.assertFalse(self.additions)

    def test_discovery_never_records_returned_data_and_bounds_unique_offsets(self):
        secret = 0xabcdef0123456789
        for i in range(128):
            self.trace.discover_read(NS(addr=0xb80000000 + 8 * i, data=secret,
                                       pc=0, flags=NS(WIDTH=3, CPU=6, WRITE=False)),
                                     "aperture0", 0xb80000000)
        records = [json.loads(l) for l in self.path.read_text().splitlines()]
        reads = [e for e in records if e["kind"] == "aperture_read"]
        self.assertEqual(len(reads), 32)
        self.assertNotIn(hex(secret), self.path.read_text())
        self.assertTrue(all(e["value_omitted"] for e in reads))

    def test_write_discovery_keeps_only_addresses(self):
        self.trace.discover_read(NS(addr=0xb82000000, data=0x123456789abcdef,
                                   pc=0, flags=NS(WIDTH=3, CPU=6, WRITE=True)),
                                 "control", 0xb82000000)
        record = json.loads(self.path.read_text().splitlines()[-1])
        self.assertEqual(record["kind"], "aperture_write")
        self.assertTrue(record["write"])
        self.assertNotIn("value", record)


if __name__ == "__main__":
    unittest.main()
