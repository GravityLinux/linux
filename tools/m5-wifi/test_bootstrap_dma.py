# SPDX-License-Identifier: GPL-2.0-only
"""Exercise DMA enable/cleanup failure boundaries without a device."""
import unittest
from types import SimpleNamespace as NS
from unittest.mock import patch

from probe_bootstrap_dma import probe


class DMAFailureTests(unittest.TestCase):
    def test_companion_buffer_is_mapped_before_publication_and_dma_is_closed_on_failure(self):
        self.startup = 0xb82028000
        self.driver.runtime_bars = {0: {0: dict(physical=self.startup - 0x8000)}}
        old_cfg = self.driver.cfg
        self.driver.cfg = lambda *a: ((0x1901 if a[2] == 0 else 0x1903) << 16 | 0x106b) if a[3] == 0 else old_cfg(*a)
        self.driver.image_dma = dict(record_physical=self.physical,
                                     record_iova=0x01000000,
                                     payload_physical=self.physical + 0x4000,
                                     payload_iova=0x02000000, payload_size=0x4000)
        copies, mappings = [], []
        companion_physical = self.physical + 0x8000
        self.utils.memalign = lambda *a: companion_physical
        self.utils.iface = NS(writemem=lambda a, data: copies.append((a, len(data))))
        self.dart.iomap_at = lambda *a: mappings.append(a)
        def fail(address, value, width):
            self.write(address, value, width)
            if address == self.startup + 0x1070:
                raise IOError('stop after companion publication')
        self.utils.write = fail
        with patch('m1n1.hw.dart.DART.from_adt', return_value=self.dart):
            with self.assertRaisesRegex(IOError, 'companion publication'):
                probe(self.utils, self.driver, runtime=True, reuse_dma=True,
                      boot_buffer_data=bytes(0xae3c00))
        self.assertEqual(copies, [(companion_physical, 0xae3c00)])
        self.assertIn((1, 0x06000000, companion_physical, 0xae4000), mappings)
        self.assertIn((self.cfg + 0xf88, 0x06000000, 32), self.writes)
        self.assertIn((self.cfg + 0xf90, 0xae3c00, 32), self.writes)
        self.assertLess(self.writes.index((self.cfg + 4, 6, 16)),
                        self.writes.index((self.cfg + 0xf88, 0x06000000, 32)))
        self.assertEqual(self.writes[-1], (self.cfg + 4, 2, 16))
        self.assertEqual(self.dart.dart.regs.DISABLE_STREAMS[0].val, 2)

    def test_runtime_transfer_waits_past_initial_state_one_and_uses_second_doorbell(self):
        self.startup = 0xb82028000
        self.driver.runtime_bars = {0: {0: dict(physical=self.startup - 0x8000)}}
        old_cfg = self.driver.cfg
        self.driver.cfg = lambda *a: 0x1901106b if a[3] == 0 else old_cfg(*a)
        states = iter((1, 1, 2))  # Before, first poll, completion.
        self.utils.read = lambda a, w: next(states) if a == self.startup else (1 if a == self.startup + 0x40 else 0x80010100)
        self.utils.proxy.dc_ivac = lambda *a: None
        self.utils.iface = NS(readmem=lambda *a: bytes(32))
        regs = self.dart.dart.regs
        for name in ('ERROR_ADDR_LO', 'ERROR_ADDR_HI', 'ERROR_DISABLE'):
            setattr(regs, name, NS(val=0))
        with patch('m1n1.hw.dart.DART.from_adt', return_value=self.dart):
            result = probe(self.utils, self.driver, runtime=True)
        self.assertEqual(result['status'], '0x2')
        self.assertEqual([t['status'] for t in result['transitions']], ['0x1', '0x2'])
        self.assertIn((self.startup + 0x1070, 1, 32), self.writes)
        self.assertFalse(any(a == self.startup + 0x1000 for a, _, _ in self.writes))
        self.assertEqual(self.writes[-1], (self.cfg + 4, 2, 16))
        self.assertEqual(regs.DISABLE_STREAMS[0].val, 2)

    def setUp(self):
        self.writes = []
        self.cfg = 0x1cb0100000
        self.physical = 0x10010000000
        self.startup = 0xb82008000
        regs = NS(DISABLE_STREAMS=[NS(val=0)], ENABLE_STREAMS=[NS(val=0)],
                  TCR=[NS(reg=None) for _ in range(19)],
                  TTBR=[NS(reg=None) for _ in range(19)], TLB_OP=NS(val=0), ERROR=NS(val=0))
        def mapped(*args):
            regs.DISABLE_STREAMS[0].val = 0
        self.dart = NS(dart=NS(regs=regs, enabled_streams=0, u=None),
                       iomap_at=mapped,
                       iotranslate=lambda *a: [(self.physical, 32)])
        proxy = NS(memset8=lambda *a: None, dc_civac=lambda *a: None,
                   dc_cvac=lambda *a: None)
        self.utils = NS(proxy=proxy, memalign=lambda *a: self.physical,
                        adt={'/arm-io/dart-apcie0': NS(get_reg=lambda i: (0x488000000, 0x20000)),
                             '/chosen': NS(chip_id=0x8142, board_id=0x24)},
                        write=self.write, read=lambda *a: 0x80010100)
        self.driver = NS(ecam=self.cfg - 0x100000,
                         assigned_bars={0: dict(physical=self.startup - 0x8000)},
                         node=NS(get_reg=lambda i: (0x488028000, 0x8000)),
                         cfg=lambda *a: 0x1900106b if a[3] == 0 else next((v for address, v, _ in reversed(self.writes)
                                              if address == self.cfg + 4), 2),
                         probe=lambda: dict(link_active=True),
                         wait32=lambda *a, **kw: None)

    def write(self, address, value, width):
        self.writes.append((address, value, width))

    def test_mapping_mismatch_never_enables_endpoint_dma(self):
        self.dart.iotranslate = lambda *a: [(self.physical + 0x4000, 32)]
        with patch('m1n1.hw.dart.DART.from_adt', return_value=self.dart):
            with self.assertRaisesRegex(RuntimeError, 'mapping did not read back'):
                probe(self.utils, self.driver)
        self.assertEqual(self.writes, [])

    def test_reuse_keeps_image_and_record_pages_without_cpu_rewrites(self):
        self.startup = 0xb82028000
        self.driver.runtime_bars = {0: {0: dict(physical=self.startup - 0x8000)}}
        old_cfg = self.driver.cfg
        self.driver.cfg = lambda *a: 0x1901106b if a[3] == 0 else old_cfg(*a)
        self.driver.image_dma = dict(record_physical=self.physical,
                                     record_iova=0x01000000,
                                     payload_physical=self.physical + 0x4000,
                                     payload_iova=0x02000000, payload_size=0x4000)
        mappings = []
        self.dart.iomap_at = lambda *a: mappings.append(a)
        self.utils.memalign = lambda *a: self.fail('Reused buffers must not be reallocated')
        self.utils.iface = NS(writemem=lambda *a: self.fail('Reused buffers must not be rewritten'))
        def fail(address, value, width):
            self.write(address, value, width)
            if address == self.startup + 0x1070:
                raise IOError('stop after DMA reuse setup')
        self.utils.write = fail
        with patch('m1n1.hw.dart.DART.from_adt', return_value=self.dart):
            with self.assertRaisesRegex(IOError, 'DMA reuse setup'):
                probe(self.utils, self.driver, runtime=True, reuse_dma=True)
        self.assertEqual(mappings, [(1, 0x01000000, self.physical, 0x4000),
                                    (1, 0x02000000, self.physical + 0x4000, 0x4000)])
        self.assertEqual(self.writes[-1], (self.cfg + 4, 2, 16))
        self.assertEqual(self.dart.dart.regs.DISABLE_STREAMS[0].val, 2)

    def test_unmapped_payload_record_uses_owned_header_and_zero_reserved_bytes(self):
        import struct
        records, mappings = [], []
        addresses = iter([self.physical, self.physical + 0x4000])
        self.utils.memalign = lambda *a: next(addresses)
        self.utils.iface = NS(writemem=lambda a, data: records.append((a, data)))
        self.dart.iomap_at = lambda *a: mappings.append(a)
        def fail(address, value, width):
            self.write(address, value, width)
            if address == self.startup + 0x78:
                raise IOError('stop after record preparation')
        self.utils.write = fail
        with patch('m1n1.hw.dart.DART.from_adt', return_value=self.dart):
            with self.assertRaisesRegex(IOError, 'record preparation'):
                probe(self.utils, self.driver, payload_size=0x4000, payload_mapped=False)
        self.assertEqual(records, [(self.physical, struct.pack('<QQQQ', 0x02000000, 0x4000, 0, 0))])
        self.assertEqual(mappings, [(1, 0x01000000, self.physical, 0x4000)])

    def test_setup_failure_after_enable_restores_endpoint_command(self):
        def fail(address, value, width):
            self.write(address, value, width)
            if address == self.startup + 0x78:
                raise IOError('injected control write failure')
        self.utils.write = fail
        with patch('m1n1.hw.dart.DART.from_adt', return_value=self.dart):
            with self.assertRaisesRegex(IOError, 'injected'):
                probe(self.utils, self.driver)
        commands = [v for a, v, w in self.writes if a == self.cfg + 4]
        self.assertEqual(commands, [6, 2])
        self.assertEqual(self.dart.dart.regs.DISABLE_STREAMS[0].val, 2)

    def test_command_restore_failure_still_disables_stream(self):
        def fail(address, value, width):
            self.write(address, value, width)
            if address == self.startup + 0x78:
                raise IOError('injected control write failure')
            if address == self.cfg + 4 and value == 2:
                raise IOError('injected command restore failure')
        self.utils.write = fail
        with patch('m1n1.hw.dart.DART.from_adt', return_value=self.dart):
            with self.assertRaisesRegex(IOError, 'command restore'):
                probe(self.utils, self.driver)
        self.assertEqual(self.dart.dart.regs.DISABLE_STREAMS[0].val, 2)


if __name__ == '__main__':
    unittest.main()
