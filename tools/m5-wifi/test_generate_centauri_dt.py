# SPDX-License-Identifier: GPL-2.0-only
"""DT generation must preserve PCI cell order and reject stale/active staging."""
import copy
from types import SimpleNamespace as NS
import unittest
from generate_centauri_dt import describe_adt, generate
import test_pack_bluetooth_handoff as bluetooth_fixture


class Node(dict):
    def getprop(self, name):
        return self[name]


class CentauriDeviceTreeTests(unittest.TestCase):
    def setUp(self):
        fixture = bluetooth_fixture.BluetoothMetadataTests()
        fixture.setUp()
        self.bt = fixture.state
        self.bt['streams']['3']['mappings'][0]['physical'] = 0x10000300000
        self.wifi = dict(self.bt, context_iova=0x2000000, context_physical=0x10000200000,
                         bar0_physical=0xb82010000, owned_tx_arena_iova=0xf000000,
                         owned_tx_arena_physical=0x10000400000, owned_tx_arena_size=0x200000,
                         station_mac='020000000001', queue_configs=[],
                         reserved_dram=[(0x10000400000, 0x200000)],
                         streams={'2': {'mappings': [dict(iova=0xf000000, physical=0x10000400000, length=0x200000)]}})
        self.info = dict(ecam=[0x1cb0000000, 0x10000000], bus_range=[0, 8],
                         ranges=[(0x02000000, 0x80000000, 0xb80000000, 0x40000000)],
                         dart=[0x488000000, 0x20000], dart_irq=[1463],
                         wifi_mac='020000000001', bluetooth_mac='020000000002')

    def test_adt_pci_address_word_order(self):
        bridge = Node({'#msi-vectors': 32, 'centauri-alpha': Node({'local-mac-address': b'\x02\0\0\0\0\1'})})
        controller = Node({'pci-bridge0': bridge, 'bus-range': 8 << 32, 'msi-vector-offset': 1741})
        controller.ranges = [NS(bus_addr=[0x43000000, 0xc0000000, 0xb], parent_addr=0xbc0000000, size=0x20000000)]
        controller._parent = NS(translate=lambda value: value)
        controller.interrupts = [1462, 1471, 1480]
        controller.get_reg = lambda index: (0x1cb0000000, 0x10000000)
        dart = NS(get_reg=lambda index: (0x488000000, 0x20000), interrupts=[1463])
        adt = {'/arm-io/apcie': controller, '/arm-io/dart-apcie0': dart,
               '/arm-io/bluetooth': Node({'local-mac-address': b'\x02\0\0\0\0\2'})}
        self.assertEqual(describe_adt(adt)['ranges'][0], (0x43000000, 0xbc0000000, 0xbc0000000, 0x20000000))

    def test_generated_tree_describes_functions_and_reserved_handoffs(self):
        text, blobs, loads = generate(self.info, self.wifi, self.bt, 0x10000500000, 0x10000508000,
                                      base_dts='/dts-v1/;\n/ { };\n')
        self.assertIn('reg = <0x10100 0 0 0 0>', text)
        self.assertIn('bus-range = <1 1>', text)
        self.assertIn('<0x2000000 0x0 0x80000000 0xb 0x80000000 0x0 0x40000000>', text)
        self.assertEqual([blob[:8] for _, blob, _ in blobs], [b'CENH0001', b'CENB0002'])
        self.assertEqual([load['physical'] for load in loads], [0x10000500000, 0x10000508000])

    def test_rejects_other_boot_and_packet_storage(self):
        other = copy.deepcopy(self.bt)
        other['handoff_nonce'] = 'aa' * 32
        with self.assertRaisesRegex(ValueError, 'different boots'):
            generate(self.info, self.wifi, other, 0x10000500000, 0x10000508000)
        with self.assertRaisesRegex(ValueError, 'unused tail'):
            generate(self.info, self.wifi, self.bt, 0x10000400000, 0x10000508000)
        with self.assertRaisesRegex(ValueError, 'Overlapping'):
            generate(self.info, self.wifi, self.bt, 0x10000500000, 0x10000500000)


if __name__ == '__main__':
    unittest.main()
