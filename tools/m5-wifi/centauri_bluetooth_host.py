# SPDX-License-Identifier: GPL-2.0-only
"""Host Python Bluetooth startup with a newly owned SID3 domain and queues.

The shared chip firmware must already be running. All Bluetooth host memory,
queue descriptions and HCI requests are generated here. No handoff file,
serial identifier, previous host context or firmware memory is consumed.
"""
from types import SimpleNamespace
import struct

from m1n1.hw.dart import DART
from m1n1.hw.dart8110 import R_TCR, R_TTBR, R_TLB_OP
from probe_bootstrap_dma import ZeroedPages
import centauri_regs as R
import centauri_bluetooth as hci
from centauri_bluetooth_control import BluetoothControl, core_host_queue_layout, CAPACITY_MODES, inline_capacity_mode


class OwnedBluetoothHost:
    SID = 3
    FUNCTION = 2
    ARENA_BYTES = 0x8000

    def __init__(self, utils, driver, *, iova=0x10000000):
        self.u, self.driver = utils, driver
        if iova < 0 or iova & 0x3fff or iova + self.ARENA_BYTES > 1 << 36:
            raise ValueError('Bluetooth arena must fit an aligned DART window')
        if driver.cfg(1, 0, self.FUNCTION, 0) != R.CENTAURI_BETA_DEVICE << 16 | R.PCI_VENDOR_APPLE:
            raise RuntimeError('Expected the runtime Bluetooth PCI function')
        self.config = driver.ecam + (1 << 20) + (self.FUNCTION << 12)
        self.bank = driver.runtime_bars[self.FUNCTION][0]['physical']
        self.saved_command = driver.cfg(1, 0, self.FUNCTION, R.PCI_COMMAND, 2)
        if self.saved_command & R.PCI_COMMAND_BUS_MASTER:
            raise RuntimeError('Stop Bluetooth DMA before creating its owned domain')
        if utils.read(self.bank, 32) != 0 or utils.read(self.bank + hci.BT_FIRMWARE_STATE, 32) != 2:
            raise RuntimeError('Expected uninitialized Bluetooth IPC with running firmware')
        self.dart_base = utils.adt['/arm-io/dart-apcie0'].get_reg(0)[0]
        self.dart = DART.from_adt(utils, '/arm-io/dart-apcie0')
        self.pages = ZeroedPages(utils)
        self.dart.dart.u = self.pages
        self.regs = self.dart.dart.regs
        self.active = False
        self.regs.DISABLE_STREAMS[0].val = 1 << self.SID
        self.dart.dart.enabled_streams &= ~(1 << self.SID)
        self.regs.TCR[self.SID].reg = R_TCR(TRANSLATE_ENABLE=1)
        self.regs.TTBR[self.SID].reg = R_TTBR(VALID=0)
        self.physical = self.pages.memalign(0x4000, self.ARENA_BYTES)
        self.iova = iova
        self.dart.iomap_at(self.SID, iova, self.physical, self.ARENA_BYTES)
        for address, size in self.pages.pages:
            utils.proxy.dc_cvac(address, size)
        self.regs.TLB_OP.val = R_TLB_OP(STREAM=self.SID, OP=1).value
        driver.wait32(self.dart_base + 0x80, 1 << 31, 0, 'Bluetooth domain invalidation', timeout=1)
        self.ttbr = self.regs.TTBR[self.SID].reg.value
        if self.dart.iotranslate(self.SID, iova, self.ARENA_BYTES) != [(self.physical, self.ARENA_BYTES)]:
            raise RuntimeError('Bluetooth arena translation differs from its allocation')
        requester = (1 << 8) | self.FUNCTION
        mapping = R.PORT_RID_TO_SID_VALID | self.SID << R.PORT_RID_TO_SID_STREAM_SHIFT | requester
        port = driver.node.get_reg(8)[0]
        utils.write(port + R.PORT_RID_TO_SID_TABLE + self.SID * 4, mapping, 32)
        if utils.read(port + R.PORT_RID_TO_SID_TABLE + self.SID * 4, 32) != mapping:
            raise RuntimeError('Bluetooth requester mapping did not read back')
        self.state = dict(bluetooth_bar0_physical=self.bank, bluetooth_context_iova=iova,
                          dart_base=self.dart_base,
                          streams={'3': {'mappings': [dict(iova=iova, physical=self.physical, length=self.ARENA_BYTES)]}},
                          bluetooth_queues=core_host_queue_layout(iova, arena_bytes=self.ARENA_BYTES))
        carrier = SimpleNamespace(memory=self, state=self.state, bank=self.bank, context=iova,
                                  maps=self.state['streams']['3']['mappings'],
                                  read=self.read_dma, write=self.write_dma, ready=self.require_ready)
        self.control = BluetoothControl(carrier)
        try:
            utils.write(self.config + R.PCI_COMMAND, self.saved_command | R.PCI_COMMAND_BUS_MASTER, 16)
            self.active = True
            self.control.publish_context(iova, iova + self.ARENA_BYTES, completion_entry_bytes=32)
            for number in (hci.EVENT_QUEUE, hci.COMMAND_QUEUE):
                self.control.register(next(q for q in self.state['bluetooth_queues'] if q['number'] == number))
            self.transport = hci.BluetoothQueues(self, self.state, bootstrap_only=True)
            sizes = hci.command_complete(self.transport, hci.HCI_OP_READ_BUFFER_SIZE)
            self.acl_mtu, self.sco_mtu, self.acl_packets, self.sco_packets = struct.unpack('<HBHH', sizes)
            layout = core_host_queue_layout(iova, acl_mtu=self.acl_mtu, arena_bytes=self.ARENA_BYTES)
            if layout[:2] != self.state['bluetooth_queues']:
                raise RuntimeError('Adding ACL queues changed the bootstrap ring addresses')
            for queue in layout[2:]:
                if queue['capacity'] not in CAPACITY_MODES:
                    CAPACITY_MODES[queue['capacity']] = inline_capacity_mode(queue['capacity'])
                self.control.register(queue)
            self.state['bluetooth_queues'] = layout
            self.transport = hci.BluetoothQueues(self, self.state)
        except BaseException:
            self.close()
            raise

    def validate_current_owner(self, state):
        if state is not self.state or self.pages.u is not self.u:
            raise RuntimeError('Bluetooth transport does not belong to this allocation owner')
        if self.regs.TTBR[self.SID].reg.value != self.ttbr:
            raise RuntimeError('Bluetooth DMA root changed')
        if self.dart.iotranslate(self.SID, self.iova, self.ARENA_BYTES) != [(self.physical, self.ARENA_BYTES)]:
            raise RuntimeError('Bluetooth DMA allocation mapping changed')

    def require_ready(self):
        if self.read(self.bank, 32) != 2 or self.read(self.bank + hci.BT_FIRMWARE_STATE, 32) != 2:
            raise RuntimeError('Bluetooth firmware/IPC are not ready')
        if self.read(self.dart_base + hci.DART_ERROR_STATUS, 32):
            raise RuntimeError('Bluetooth DART reported an error')

    def _physical(self, iova, length):
        if not 0 < length <= 4096 or iova < self.iova or iova + length > self.iova + self.ARENA_BYTES:
            raise ValueError('Bluetooth DMA access exceeds the owned arena')
        return self.physical + iova - self.iova

    def read_dma(self, iova, length):
        return self.readmem(self._physical(iova, length), length)

    def write_dma(self, iova, data):
        self.writemem(self._physical(iova, len(data)), data)

    def _check_physical(self, address, length):
        if not 0 < length <= 4096 or address < self.physical or address + length > self.physical + self.ARENA_BYTES:
            raise ValueError('Bluetooth access exceeds its physical allocation')

    def readmem(self, address, length):
        self._check_physical(address, length)
        self.u.proxy.dc_ivac(address, length)
        return self.u.iface.readmem(address, length)

    def writemem(self, address, data):
        self._check_physical(address, len(data))
        self.u.iface.writemem(address, data)
        self.u.proxy.dc_cvac(address, len(data))

    def read(self, address, width):
        if width != 32 or (address != self.dart_base + hci.DART_ERROR_STATUS and
                           address - self.bank not in (0, 4, 8, 0xc, 0x10, 0x14, 0x18)):
            raise ValueError('Read outside Bluetooth register metadata')
        return self.u.read(address, width)

    def write(self, address, value, width):
        offset = address - self.bank
        if width != 32 or (offset not in (4, 8, 0xc, 0x10, 0x14, 0x107c) and
                           not 0x1000 <= offset < 0x1058):
            raise ValueError('Write outside qualified Bluetooth registers')
        self.u.write(address, value, width)

    def close(self):
        if self.active:
            self.u.write(self.config + R.PCI_COMMAND, self.saved_command & ~R.PCI_COMMAND_BUS_MASTER, 16)
            self.active = False
        self.regs.DISABLE_STREAMS[0].val = 1 << self.SID
