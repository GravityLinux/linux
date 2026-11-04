# SPDX-License-Identifier: GPL-2.0-only
"""Check owned IPC address bounds and cleanup on a failed device handshake."""

import struct
import unittest
from types import SimpleNamespace as NS
from unittest.mock import patch

from centauri_ipc import ControlIPC, make_context, CONTEXT_EXTENT


class IPCTests(unittest.TestCase):
    def test_submission_wraps_and_publishes_frame_before_doorbell(self):
        ipc = ControlIPC.__new__(ControlIPC)
        ipc.physical, ipc.iova, ipc.bank = 0x100000, 0x10000, 0x200000
        operations = []
        ipc.u = NS(
            iface=NS(writemem=lambda a, d: operations.append(('memory', a, d)),
                     readmem=lambda a, s: struct.pack('<4I', 0x401, 0x4000f, 0, 0)),
            proxy=NS(dc_cvac=lambda a, s: operations.append(('flush', a, s))),
            write=lambda *a: operations.append(('register', *a)))
        states = iter([
            dict(image_state='0x2', ipc_state='0x2', index_pairs=[[15, 15], [14, 15]]),
            dict(index_pairs=[[0, 0], [15, 0]])])
        ipc.snapshot = lambda: next(states)
        result = ipc.submit(struct.pack('<I', 0x30003))
        self.assertEqual((result['slot'], result['cookie'], result['bytes_completed']), (15, 15, 4))
        frame_write = ('memory', 0x101780, struct.pack('<I', 0x30003))
        descriptor_write = ('memory', 0x1001a0, struct.pack('<IQI', 0x401, 0x11780, 15))
        self.assertIn(frame_write, operations)
        self.assertIn(descriptor_write, operations)
        self.assertEqual(operations[-1], ('register', 0x201000, 0, 32))
        self.assertLess(operations.index(frame_write), operations.index(descriptor_write))
        self.assertIn(('register', 0x100096, 0, 16), operations)
        self.assertFalse(any(op[0] == 'register' and op[1] in (0x100078, 0x10007a)
                             for op in operations))

    def test_pending_submission_cannot_overwrite_a_live_frame(self):
        ipc = ControlIPC.__new__(ControlIPC)
        ipc.snapshot = lambda: dict(image_state='0x2', ipc_state='0x2',
                                    index_pairs=[[1, 1], [0, 2]])
        with self.assertRaisesRegex(RuntimeError, 'outstanding'):
            ipc.submit(bytes(52))

    def test_retained_domain_rejects_occupied_iova_before_any_write(self):
        writes = []
        u = NS(read=lambda a, w: 2 if a == 0x208000 else 0,
               write=lambda *a: writes.append(a))
        driver = NS(ecam=0x10000000, runtime_bars={0: {0: {'physical': 0x200000}}},
                    cfg=lambda b, d, f, o, *w: 0x1901106b if o == 0 else 0x406)
        regs = NS(TCR={1: NS(reg=NS(TRANSLATE_ENABLE=True, BYPASS_DART=False))},
                  TTBR={1: NS(reg=NS(VALID=True))})
        dart = NS(dart=NS(regs=regs), iotranslate=lambda *a: [(0x12340000, 0x4000)])
        with patch('m1n1.hw.dart.DART.from_adt', return_value=dart):
            with self.assertRaisesRegex(ValueError, 'overwrite'):
                ControlIPC(u, driver, retain_domain=True)
        self.assertEqual(writes, [])

    def test_existing_ipc_owner_is_rejected_before_changing_dma(self):
        writes = []
        u = NS(read=lambda a, w: 2, write=lambda *a: writes.append(a))
        driver = NS(ecam=0x10000000, runtime_bars={0: {0: {'physical': 0x200000}}},
                    cfg=lambda b, d, f, o, *w: 0x1901106b if o == 0 else 0x402)
        with self.assertRaisesRegex(RuntimeError, 'already published'):
            ControlIPC(u, driver)
        self.assertEqual(writes, [])

    def test_context_relocation_keeps_all_pointers_in_owned_allocation(self):
        for address in (0x1000000, 0x80000000, (1 << 36) - 0x4000):
            data = make_context(address)
            self.assertEqual(len(data), CONTEXT_EXTENT)
            for offset in (8, 0x10, 0x18, 0x20, 0x28, 0x34, 0x3c, 0x58):
                pointer, = struct.unpack_from('<Q', data, offset)
                self.assertTrue(address <= pointer < address + len(data))
        for address in (-0x4000, 1, 0x1000004, 1 << 36):
            with self.assertRaises(ValueError):
                make_context(address)

    def test_failed_handshake_disables_dma_without_publishing_context(self):
        ipc = ControlIPC.__new__(ControlIPC)
        writes = []
        ipc.config, ipc.bank, ipc.command, ipc.sid = 0x100000, 0x200000, 0x402, 1
        ipc.u = NS(write=lambda *a: writes.append(a))
        ipc.regs = NS(DISABLE_STREAMS=[NS(val=0)])
        def fail(*a, **kw):
            raise TimeoutError('device did not acknowledge state 1')
        ipc.driver = NS(wait32=fail)
        with self.assertRaises(TimeoutError):
            ipc.publish()
        self.assertEqual(writes, [(0x100004, 0x406, 16), (0x20107c, 1, 32),
                                  (0x100004, 0x402, 16)])
        self.assertEqual(ipc.regs.DISABLE_STREAMS[0].val, 2)


if __name__ == '__main__':
    unittest.main()
