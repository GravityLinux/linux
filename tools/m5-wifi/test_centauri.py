# SPDX-License-Identifier: GPL-2.0-only
"""Check direct transport probing without accessing a device."""

import unittest
import struct
from types import SimpleNamespace as NS

from centauri import Centauri


class Fake:
    def __init__(self):
        self.base = 0x1cb0000000
        self.adt = {"/arm-io/apcie": NS(get_reg=lambda i: (self.base, 0x10000000))}
        self.reads = []
        self.registers = {(0, 32): 0x100c106b, (6, 16): 0x10, (0x0e, 8): 1,
                          (0x34, 8): 0x40, (0x40, 16): 0x10,
                          (0x52, 16): 0, (0x19, 8): 1, (0x1a, 8): 1}

    def read(self, address, width):
        self.reads.append(address)
        offset = address - self.base
        if offset & 0xfff == 0 and offset >> 20:
            return self.registers.get((offset, width), 0xffffffff)
        return self.registers.get((offset, width), 0)


class ProbeTests(unittest.TestCase):
    def test_runtime_msi_allocations_are_distinct_and_do_not_enable_dma(self):
        u = Fake()
        node = u.adt['/arm-io/apcie']
        node.getprop = lambda n: 0xfffff000
        for fn, identifier in enumerate((0x1901, 0x1902, 0x1903)):
            base = 0x100000 + (fn << 12)
            u.registers.update({(base, 32): identifier << 16 | 0x106b,
                                (base + 4, 16): 2, (base + 6, 16): 0x10,
                                (base + 0x34, 8): 0x50, (base + 0x50, 16): 5,
                                (base + 0x52, 16): 0x8a})
        writes = []
        def write(a, v, w):
            writes.append((a, v, w))
            u.registers[a - u.base, w] = v
        u.write = write
        d = Centauri(u)
        result = d.enable_runtime_msi()
        self.assertEqual([result[f]['message_control'] for f in range(3)], ['0xab', '0xcb', '0x9b'])
        self.assertEqual([u.registers[0x10005c + (f << 12), 16] for f in range(3)], [0, 16, 4])
        self.assertTrue(all(u.registers[0x100004 + (f << 12), 16] == 0x402 for f in range(3)))
        writes.clear()
        for count, data in ((3, 0), (16, 4), (32, 32), (4, -4), (64, 0)):
            with self.assertRaises(ValueError):
                d.enable_function_msi(0, count, data)
        self.assertEqual(writes, [])

    def test_runtime_resources_do_not_overlap_and_leave_all_dma_disabled(self):
        u = Fake()
        node = u.adt['/arm-io/apcie']
        node.ranges = [NS(bus_addr=[0x02000000, 0x80000000, 0],
                          parent_addr=0xb80000000, size=0x40000000)]
        node._parent = NS(translate=lambda a: a)
        for function, identifier in enumerate([0x1901, 0x1902, 0x1903]):
            u.registers[0x100000 + (function << 12), 32] = identifier << 16 | 0x106b
        u.write = lambda a, v, w: u.registers.__setitem__((a - u.base, w), v)
        driver = Centauri(u)
        def sized(bus=1, device=0, function=0):
            resources = [dict(index=0, size='0x10000', is64=True,
                              prefetchable=True, original='0xc')]
            if function == 0:
                resources.append(dict(index=4, size='0x2000000', is64=True,
                                      prefetchable=True, original='0xc'))
            return resources
        driver.size_bars = sized
        driver.assign_runtime_bars()
        self.assertEqual(driver.runtime_bars[0][0]['physical'], 0xb82020000)
        self.assertEqual(driver.runtime_bars[1][0]['physical'], 0xb82010000)
        self.assertEqual(driver.runtime_bars[2][0]['physical'], 0xb82000000)
        ordered = sorted((b['address'], b['address'] + b['size'])
                         for resources in driver.runtime_bars.values() for b in resources.values())
        self.assertTrue(all(a[1] <= b[0] for a, b in zip(ordered, ordered[1:])))
        for function in range(3):
            self.assertEqual(u.registers[0x100004 + (function << 12), 16] & 6, 2)

    def test_msi_receiver_precedes_enable_and_does_not_enable_bus_mastering(self):
        u = Fake()
        node = u.adt['/arm-io/apcie']
        node.get_reg = lambda i: {0: (u.base, 0x10000000),
                                  8: (0x488028000, 0x8000),
                                  11: (0x488024000, 0x4000)}[i]
        node.getprop = lambda name: 0xfffff000
        u.registers.update({(0x100000, 32): 0x1900106b, (0x100004, 16): 2,
                            (0x100006, 16): 0x10, (0x100034, 8): 0x50,
                            (0x100050, 16): 5, (0x100052, 16): 0x8a})
        writes = []
        def write(a, v, w):
            writes.append((a, v, w))
            u.registers[a - u.base, w] = v
        u.write = write
        result = Centauri(u).enable_bootstrap_msi()
        self.assertEqual(result['message_control'], '0xab')
        self.assertEqual(u.registers[0x100004, 16], 0x402)
        self.assertEqual(u.registers[0x100054, 32], 0xfffff000)
        self.assertEqual(u.registers[0x100058, 32], 0)
        self.assertEqual(u.registers[0x10005c, 16], 0)
        self.assertEqual(writes[-1], (u.base + 0x100052, 0xab, 16))
        self.assertIn((0x488028124, 1, 32), writes[:-1])
        self.assertEqual([v for a, v, w in writes if 0x48802b800 <= a < 0x48802b880],
                         [0x80000000 | i for i in range(32)])

    def test_cold_control_snapshot_never_reads_later_stage_bank(self):
        u = Fake()
        driver = Centauri(u)
        driver.assigned_bars[0] = dict(physical=0xb82000000, size=0x10000)
        driver.control_snapshot()
        self.assertEqual(u.reads, [0xb82008000, 0xb82008004, 0xb82008008,
                                   0xb8200800c, 0xb82008010])

    def test_resource_windows_translate_and_endpoint_dma_stays_disabled(self):
        u = Fake()
        node = u.adt['/arm-io/apcie']
        node.ranges = [NS(bus_addr=[0x02000000, 0x80000000, 0],
                          parent_addr=0xb80000000, size=0x40000000)]
        node._parent = NS(translate=lambda a: a)
        writes = []
        def write(address, value, width):
            writes.append((address, value, width))
            u.registers[address - u.base, width] = value
        u.write = write
        u.registers[0x100004, 16] = 7
        u.registers[0x100000, 32] = 0x1900106b
        driver = Centauri(u)
        driver.size_bars = lambda: [dict(index=i, size=hex(size), is64=True,
                                         prefetchable=True, original='0xc')
                                    for i, size in [(0, 0x10000), (4, 0x2000000)]]
        driver.assign_bars()
        self.assertEqual(driver.assigned_bars[0]['physical'], 0xb82000000)
        self.assertEqual(driver.assigned_bars[4]['physical'], 0xb80000000)
        self.assertEqual(u.registers[0x24, 32], 0x82018001)
        self.assertEqual(u.registers[4, 16] & 6, 6)
        self.assertEqual(u.registers[0x100004, 16] & 6, 2)
        self.assertTrue(all(u.base <= a < u.base + 0x200000 for a, _, _ in writes))

    def test_bootstrap_allocator_rejects_runtime_device_before_bar_probe(self):
        u = Fake()
        u.registers[0x100000, 32] = 0x1901106b
        with self.assertRaisesRegex(RuntimeError, 'only the bootstrap function'):
            Centauri(u).assign_bars()
        self.assertEqual(u.reads, [u.base + 0x100000])

    def test_raw_adt_tunable_preserves_unmasked_register_bits(self):
        u = Fake()
        u.adt["/arm-io/apcie"].getprop = lambda name: struct.pack("<IIQQ", 0x10, 4, 0xff, 0xa5)
        u.registers[0x10, 32] = 0xdeadbeef
        writes = []
        u.write = lambda *args: writes.append(args)
        Centauri(u).apply_tunables("fixture", 0)
        self.assertEqual(writes, [(u.base + 0x10, 0xdeadbea5, 32)])

    def test_no_downstream_access_before_link_active(self):
        u = Fake()
        result = Centauri(u).probe()
        self.assertFalse(result["link_active"])
        self.assertEqual(result["endpoints"], [])
        self.assertTrue(all(a < u.base + 0x100000 for a in u.reads))

    def test_cyclic_capability_list_terminates_without_downstream_access(self):
        u = Fake()
        u.registers[0x40, 16] = 0x4401
        u.registers[0x44, 16] = 0x4002
        self.assertIsNone(Centauri(u).capability(0, 0, 0, 0x10))
        self.assertLess(len(u.reads), 10)

    def test_endpoint_snapshot_distinguishes_msi_and_msix_enable_state(self):
        u = Fake()
        base = 0x100000
        u.registers.update({(base, 32): 0x1900106b, (base + 4, 16): 0x406,
                            (base + 6, 16): 0x10, (base + 0x34, 8): 0x40,
                            (base + 0x40, 16): 0x5001,
                            (base + 0x50, 16): 0x6005,
                            (base + 0x52, 16): 0x80,
                            (base + 0x54, 32): 0xfffff000,
                            (base + 0x58, 32): 0,
                            (base + 0x5c, 16): 0x10,
                            (base + 0x60, 16): 0x7011,
                            (base + 0x62, 16): 0x8000,
                            (base + 0x70, 16): 0x10,
                            (base + 0x78, 16): 0x213f})
        endpoint = Centauri(u).function(1, 0, 0)
        self.assertEqual(endpoint['command'], '0x406')
        self.assertEqual(endpoint['capabilities']['msi']['message_control'], '0x80')
        self.assertEqual(endpoint['capabilities']['msi']['address'], '0xfffff000')
        self.assertEqual(endpoint['capabilities']['msi']['data'], '0x10')
        self.assertEqual(endpoint['capabilities']['msix']['message_control'], '0x8000')
        self.assertEqual(endpoint['capabilities']['pcie']['device_control'], '0x213f')

    def test_active_link_probes_only_advertised_bus_and_keeps_64bit_bar(self):
        u = Fake()
        u.registers[0x52, 16] = 0x2000
        u.registers[0x208, 32] = 1
        u.registers[0x100000, 32] = 0x1234106b  # Synthetic endpoint ID.
        u.registers[0x100010, 32] = 0x82000004
        u.registers[0x100014, 32] = 1
        result = Centauri(u).probe()
        self.assertEqual(len(result["endpoints"]), 1)
        endpoint = result["endpoints"][0]
        self.assertEqual(endpoint["bdf"], "01:00.0")
        self.assertEqual(endpoint["bars"][0]["address"], "0x182000000")
        self.assertTrue(all(a < u.base + 0x200000 for a in u.reads))


if __name__ == "__main__":
    unittest.main()
