# SPDX-License-Identifier: GPL-2.0-only
"""Check metadata-only capture boundaries without hardware."""
import unittest
from unittest.mock import patch
import tempfile
import struct
from pathlib import Path
from types import SimpleNamespace as NS
from trace_centauri_bootstrap import BootstrapCapture, RuntimeControlCapture, PageTableReader, PCIBootWatch


class BootstrapTests(unittest.TestCase):
    def test_control_register_snapshot_is_explicit_and_bounded(self):
        reads, events = [], []
        hv = NS(u=NS(read=lambda a, w: reads.append((a, w)) or 0))
        trace = RuntimeControlCapture(hv, NS(emit=lambda *a, **kw: events.append((a, kw))))
        with patch.dict('os.environ', {'CENTAURI_CONTROL_RECORD': '0'}):
            trace.control_registers('before_submission')
        self.assertEqual(reads, [])
        with patch.dict('os.environ', {'CENTAURI_CONTROL_RECORD': '1'}):
            trace.control_registers('image_ready')
        self.assertEqual(reads, [(trace.BASE + o, 32) for o in range(0, 0x100, 4)])
        self.assertEqual(events[0][0], ('runtime_control_control_register_snapshot',))
        self.assertEqual(events[0][1]['stage'], 'image_ready')
        trace.device = lambda: NS(cfg=lambda *a: 0x1901106b)
        with patch.dict('os.environ', {'CENTAURI_CONTROL_RECORD': '1'}):
            for _ in range(2):
                trace.status_read(NS(data=3, flags=NS(WRITE=False, WIDTH=2)))
        self.assertEqual(len(reads), 128)
        self.assertEqual(len(events), 2)
        self.assertEqual(events[1][1]['stage'], 'state_3')

    def test_pci_watch_is_bounded_to_restart_and_reads_configuration_only(self):
        events, reads, mappings = [], [], []
        boot, runtime = NS(submissions=0), NS(stages_seen=set())
        hv = NS(u=NS(read=lambda *a: 1))
        watch = PCIBootWatch(hv, NS(emit=lambda *a, **kw: events.append((a, kw))), boot, runtime)
        def cfg(bus, dev, fn, offset, width=4):
            reads.append((bus, offset))
            if bus == 0:
                self.assertEqual(offset, 0x19)
                return 1
            return {0: 0x1901106b, 0xf88: 0x1000000, 0xf90: 0xae3c00}.get(offset, 0)
        watch.reader.device = lambda: NS(cfg=cfg, node=NS(get_reg=lambda i: (0x1000, 0x8000)))
        watch.source_metadata = lambda dev, a, n: mappings.append((a, n))
        def read(value):
            watch.read(NS(data=value, flags=NS(WRITE=False, WIDTH=2)))
        read(1)
        self.assertEqual(reads, [])
        boot.submissions = 1
        read(1)
        first_reads = list(reads)
        read(1)
        self.assertEqual(reads, first_reads)
        read(2)  # Re-sample but do not emit unchanged vendor words.
        self.assertEqual(mappings, [(0x1000000, 0xae3c00)])
        self.assertEqual(len(events), 1)
        self.assertEqual(events[0][0], ('pci_boot_vendor_change',))
        runtime.stages_seen.add('before_1070')
        read(3)
        self.assertEqual(len(reads), len(first_reads) * 2)
        runtime.stages_seen.clear()
        watch.samples = 4096
        read(4)
        self.assertEqual(len(reads), len(first_reads) * 2)

    def test_ready_vendor_snapshot_reads_only_once_for_the_expected_device(self):
        events, reads = [], []
        trace = BootstrapCapture(None, NS(emit=lambda *a, **kw: events.append((a, kw))))
        def cfg(bus, dev, fn, offset):
            reads.append(offset)
            return 0x1900106b if offset == 0 else 0
        trace.device = lambda: NS(cfg=cfg)
        for value in (0, 1, 4, 4):
            trace.status_read(NS(data=value, flags=NS(WRITE=False, WIDTH=2)))
        self.assertEqual(reads, [0, 0xf80, 0xf84, 0xf88, 0xf8c, 0xf90, 0xf94, 0xf98, 0xff8, 0xffc])
        self.assertEqual(events[0][0], ('bootstrap_image_ready_vendor_configuration',))

    def test_runtime_allocation_read_requires_known_header_and_same_page(self):
        reads, events = [], []
        address, physical = 0x1000000, 0x10300000000
        data = bytearray(0x3b0)
        struct.pack_into('<4Q', data, 0, 0x680300, address + 0x68,
                         address + 0x78, address + 0x7a)
        def read(a, n):
            reads.append((a, n))
            return bytes(data[:n])
        hv = NS(u=NS(ba=NS(phys_base=0x10000000000, mem_size_actual=0x400000000)),
                iface=NS(readmem=read))
        trace = RuntimeControlCapture(hv, NS(emit=lambda *a, **kw: events.append((a, kw))))
        trace.dart = NS(iotranslate=lambda *a: [(physical, a[-1])])
        streams = [dict(sid=1, ranges=[dict(physical=hex(physical), size=32)])]
        trace.record_header(address, 32, [0x80010100], streams)
        self.assertEqual(reads, [(physical, 32), (physical, 0x3b0)])
        self.assertEqual(events[-1][0], ('runtime_control_initial_context',))
        reads.clear()
        data[0] = 1
        trace.record_header(address, 32, [0x80010100], streams)
        self.assertEqual(reads, [(physical, 32)])
        self.assertEqual(events[-1][0], ('runtime_control_context_skipped',))

    def test_runtime_context_does_not_consume_bootstrap_alias_writes(self):
        trace = RuntimeControlCapture(None, NS(emit=lambda *a, **kw: None))
        calls = []
        trace.mapping = lambda *a: calls.append(a)
        def write(base, offset, value):
            trace.write(NS(addr=base + offset, data=value, flags=NS(WRITE=True, WIDTH=2)))
        for base in [BootstrapCapture.BASE, trace.BASE]:
            write(base, 0x44, 0x8000000)
            write(base, 0x48, 0)
            write(base, 0x4c, 32)
            write(base, 0x1000, 1)
        self.assertEqual(calls, [(0x8000000, 32)])
        self.assertIsNone(trace.opaque_export)

    def test_runtime_captures_init_before_variable_first_ring_index(self):
        trace = RuntimeControlCapture(None, NS(emit=lambda *a, **kw: None))
        calls = []
        trace.mapping = lambda *a: calls.append(a)
        def write(offset, value):
            trace.write(NS(addr=trace.BASE + offset, data=value, flags=NS(WRITE=True, WIDTH=2)))
        for offset, value in ((0x44, 0x1000000), (0x48, 0), (0x4c, 32),
                              (0x1070, 1), (0x107c, 1), (0x107c, 2),
                              (0x1000, 2), (0x1000, 3), (0x1000, 1)):
            write(offset, value)
        self.assertEqual(calls, [(0x1000000, 32)] * 3)
        self.assertEqual(trace.stages_seen, {'before_1070', 'before_107c_2', 'before_first_1000'})
    def test_opaque_export_copies_without_decoding_and_rejects_unmapped_ranges(self):
        with tempfile.TemporaryDirectory() as tmp:
            reads = []
            def read(a, n):
                reads.append((a, n))
                return bytes(n)
            hv = NS(u=NS(ba=NS(phys_base=0x10000000000, mem_size_actual=0x400000000)),
                    iface=NS(readmem=read), p=NS(dc_civac=lambda *a: None))
            path = Path(tmp) / 'opaque.bin'
            trace = BootstrapCapture(hv, NS(emit=lambda *a, **kw: None), True, path)
            trace.dart = NS(iotranslate=lambda *a: [(None, 0x8000)])
            with self.assertRaises(ValueError):
                trace.export_opaque_asset(1, 0x8000000, 0x8000)
            self.assertEqual(reads, [])
            self.assertFalse(path.exists())
            trace.dart.iotranslate = lambda *a: [(0x10300000000, 0x8000)]
            trace.export_opaque_asset(1, 0x8000000, 0x8000)
            self.assertEqual(reads, [(0x10300000000, 0x8000)])
            self.assertEqual(path.stat().st_size, 0x8000)
            self.assertTrue(path.with_suffix('.bin.json').exists())
            trace.export_opaque_asset(1, 0x8000000, 0x8000)
            self.assertEqual(len(reads), 1)

    def test_record_capture_reads_only_the_selected_rid_and_exact_record(self):
        reads, events = [], []
        def read(address, size):
            reads.append((address, size))
            return bytes(32)
        hv = NS(u=NS(ba=NS(phys_base=0x10000000000, mem_size_actual=0x400000000)),
                iface=NS(readmem=read))
        capture = BootstrapCapture(hv, NS(emit=lambda *a, **kw: events.append((a, kw))), True)
        mapped = [dict(sid=1, ranges=[dict(physical='0x10300000000', size=32)]),
                  dict(sid=17, ranges=[dict(physical='0x10200000000', size=32)])]
        capture.record_header(0x1000000, 64, [0x80010100], mapped)
        capture.record_header(0x1000000, 32, [0x80010101], mapped)
        self.assertEqual(reads, [])
        capture.record_header(0x1000000, 32, [0x80010100], mapped)
        self.assertEqual(reads, [(0x10300000000, 32)])
        self.assertEqual(events[-1][1]['words'], ['0x0'] * 8)
        mapped[0]['ranges'][0]['physical'] = '0x10400000000'
        with self.assertRaises(ValueError):
            capture.record_header(0x1000000, 32, [0x80010100], mapped)
        self.assertEqual(len(reads), 1)

    def test_page_table_reader_rejects_payload_sizes_and_out_of_dram(self):
        reads = []
        reader = PageTableReader(NS(readmem=lambda a, n: reads.append((a, n))),
                                 0x10000000000, 0x400000000)
        for address, size in [(0x10000000000, 32), (0, 0x4000),
                              (0x10400000000, 0x4000), (0x10000000004, 0x4000)]:
            with self.assertRaises(ValueError):
                reader.readmem(address, size)
        self.assertEqual(reads, [])
        reader.readmem(0x10000004000, 0x4000)
        self.assertEqual(reads, [(0x10000004000, 0x4000)])

    def test_submission_is_bounded_and_never_maps_larger_payload(self):
        capture = BootstrapCapture(None, NS(emit=lambda *a, **kw: None))
        mapped = []
        capture.mapping = lambda a, n: mapped.append((a, n))
        def write(offset, value):
            capture.write(NS(addr=capture.BASE + offset, data=value,
                             flags=NS(WRITE=True, WIDTH=2)))
        write(0x1000, 1)
        self.assertEqual(mapped, [])
        write(0x44, 0x1234000)
        write(0x48, 0)
        write(0x4c, 0x10000)
        write(0x1000, 1)
        self.assertEqual(mapped, [])
        write(0x4c, 32)
        for _ in range(20):
            write(0x1000, 1)
        self.assertEqual(mapped, [(0x1234000, 32)] * 7)


if __name__ == '__main__':
    unittest.main()
