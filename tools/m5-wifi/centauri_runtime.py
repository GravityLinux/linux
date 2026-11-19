# SPDX-License-Identifier: GPL-2.0-only
"""Generated host interfaces after the shared Centauri firmware boot.

All memory belongs to this proxy process. The firmware image and its device
records are explicit external inputs; there is no native-driver handoff.
Executable components and firmware-produced radio resources remain opaque.
"""
import time

import centauri_regs as R
from centauri_ftab import Directory
from centauri_ipc import ControlIPC
from centauri_bluetooth_host import OwnedBluetoothHost
from centauri_queues import DataArena, encode_runtime_message, decode_runtime_message
from centauri_wifi import WiFiControl, make_wifi_context, ARENA_SIZE
from centauri_wifi_queues import WiFiBasicQueues, POOL_SIZE
from probe_bootstrap_dma import ZeroedPages


class RuntimeHost:
    """Own fresh control, Bluetooth and Wi-Fi host contexts and basic queues.

    The caller must keep the successful ``probe_bootstrap_dma`` image owners
    alive. Their translated SID1 allocation supplies the current radio regions.
    ``close`` stops host DMA; it does not restore any previous driver or free
    DMA memory. Reset the chip before another complete initialization.
    IOVAs here are host allocation choices, not device register constants.
    """

    def __init__(self, utils, driver, firmware, *, timeout=5):
        from m1n1.hw.dart import DART
        from m1n1.hw.dart8110 import R_TCR, R_TTBR

        self.u, self.p, self.driver = utils, utils.proxy, driver
        self.dart_base = utils.adt['/arm-io/dart-apcie0'].get_reg(0)[0]
        self.config = driver.ecam + (1 << 20)
        self.control = self.bluetooth = None
        self.closed = False
        self.resources = {}
        self.observed_packets = []
        if not 0 < timeout <= 30:
            raise ValueError('Runtime initialization timeout must be between 0 and 30 seconds')
        for function, device in enumerate((R.CENTAURI_CONTROL_DEVICE, R.CENTAURI_WIFI_DEVICE,
                                            R.CENTAURI_BETA_DEVICE)):
            if driver.cfg(1, 0, function, 0) != device << 16 | R.PCI_VENDOR_APPLE:
                raise RuntimeError('Expected the three Centauri runtime PCI functions')
        template = Directory(firmware).opaque_entry(b'2ftb')
        self.layout = Directory(template, layout=True)
        pool = getattr(driver, 'boot_buffer_dma', None)
        owners = getattr(driver, 'owned_image_domains', ())
        if pool is None or not any(owner['utils'] is utils and any(
                address == pool['physical'] and size >= pool['length']
                for address, size in owner['pages'].pages) for owner in owners):
            raise ValueError('Runtime allocation must belong to this process image loader')
        self.pool = dict(pool)
        self.dart = DART.from_adt(utils, '/arm-io/dart-apcie0')
        regs = self.dart.dart.regs
        if not any(owner['utils'] is utils and owner['ttbr'] == regs.TTBR[1].reg.value
                   for owner in owners):
            raise RuntimeError('The owned image DMA domain is no longer active')
        if self.dart.iotranslate(1, pool['iova'], pool['length']) != [(pool['physical'], pool['length'])]:
            raise RuntimeError('Runtime allocation translation differs from its owner')
        if pool['length'] != max(len(template), max(e.offset + e.size for e in self.layout.entries)):
            raise ValueError('Runtime allocation does not match the supplied firmware layout')
        if driver.cfg(1, 0, 1, 4, 2) & R.PCI_COMMAND_BUS_MASTER:
            raise RuntimeError('Wi-Fi host DMA must initially be disabled')
        self.wifi_bank = driver.runtime_bars[1][0]['physical']
        if utils.read(self.wifi_bank + R.WIFI_IPC_STATE, 32):
            raise RuntimeError('Wi-Fi IPC is already published')
        self.pages = ZeroedPages(utils)
        self.dart.dart.u = self.pages
        try:
            self.bluetooth = OwnedBluetoothHost(utils, driver)
            self.control = ControlIPC(utils, driver, iova=0x0c000000, retain_domain=True)
            self.control.publish(timeout=timeout)
            regs.DISABLE_STREAMS[0].val = 1 << 2
            self.dart.dart.enabled_streams &= ~(1 << 2)
            regs.TCR[2].reg = R_TCR(TRANSLATE_ENABLE=1)
            regs.TTBR[2].reg = R_TTBR(VALID=0)
            self._allocate_resource(1, b'msww', 0x0e000000, self.dart, self.pages)
            self._allocate_resource(2, b'mswb', 0x12000000, self.bluetooth.dart, self.bluetooth.pages)
            mapping = (R.PORT_RID_TO_SID_VALID | 2 << R.PORT_RID_TO_SID_STREAM_SHIFT | 0x0101)
            port = driver.node.get_reg(8)[0]
            utils.write(port + R.PORT_RID_TO_SID_TABLE + 2 * 4, mapping, 32)
            if utils.read(port + R.PORT_RID_TO_SID_TABLE + 2 * 4, 32) != mapping:
                raise RuntimeError('Wi-Fi requester mapping did not read back')
            self.control_queues = DataArena(self.control, iova=0x07000000)
            for number in (8, 2):
                self.control_queues.queues[number].configure()
            self.control_queues.queues[2].post_receive(15)
            self.control_queues.queues[8].post_receive(3)
            for function in (1, 2):
                self._publish_resource(function)
            for number in (4, 3):
                self.control_queues.queues[number].configure()
            self.control_queues.queues[4].post_receive(7)
            self.control_queues.queues[3].send_inline(encode_runtime_message(0, 0, b'CentauriTransport\0'))
            self._wait_for_greeting(timeout)
            driver.wait32(self.wifi_bank + R.WIFI_STARTUP_STATUS, 0xffffffff,
                          R.WIFI_STARTUP_READY, 'Wi-Fi startup after radio publication', timeout=timeout)
            self._publish_wifi(timeout)
        except BaseException:
            self.close()
            raise

    def _flush_domain(self, domain, sid):
        from m1n1.hw.dart8110 import R_TLB_OP

        for address in domain.dart.pt_cache:
            self.p.dc_cvac(address, 0x4000)
        domain.dart.regs.TLB_OP.val = R_TLB_OP(STREAM=sid, OP=1).value
        self.driver.wait32(self.dart_base + 0x80, 1 << 31, 0, 'Runtime DMA invalidation', timeout=1)

    def _allocate_resource(self, function, tag, iova, domain, pages):
        entry = self.layout.entry(tag)
        if not entry.size or entry.size & 0x3fff or entry.offset + entry.size > self.pool['length']:
            raise ValueError('Radio region does not fit the owned firmware allocation')
        domain.invalidate_cache()
        if domain.iotranslate(function + 1, iova, entry.size) != [(None, entry.size)]:
            raise ValueError('Radio resource overlaps an existing mapping')
        physical = pages.memalign(0x4000, entry.size)
        domain.iomap_at(function + 1, iova, physical, entry.size)
        self._flush_domain(domain, function + 1)
        self.resources[function] = dict(tag=tag, offset=entry.offset, bytes=entry.size,
                                        iova=iova, physical=physical)

    def _publish_resource(self, function):
        resource = self.resources[function]
        source = self.pool['physical'] + resource['offset']
        self.p.dc_ivac(source, resource['bytes'])
        data = self.u.iface.readmem(source, resource['bytes'])
        self.u.iface.writemem(resource['physical'], data)
        self.p.dc_cvac(resource['physical'], resource['bytes'])
        config = self.config + (function << 12)
        command = self.driver.cfg(1, 0, function, R.PCI_COMMAND, 2)
        self.u.write(config + R.PCI_COMMAND, command | R.PCI_COMMAND_BUS_MASTER, 16)
        timestamp = time.time_ns() // 1000
        for offset, value in ((R.CENTAURI_PCI_UNIX_TIME_US_LOW, timestamp & 0xffffffff),
                              (R.CENTAURI_PCI_UNIX_TIME_US_HIGH, timestamp >> 32),
                              (R.CENTAURI_PCI_BOOT_BUFFER_LOW, resource['iova']),
                              (R.CENTAURI_PCI_BOOT_BUFFER_HIGH, resource['iova'] >> 32),
                              (R.CENTAURI_PCI_BOOT_BUFFER_LENGTH, resource['bytes'])):
            self.u.write(config + offset, value, 32)

    def _wait_for_greeting(self, timeout):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            greeted = False
            for number in (2, 4, 8):
                for packet in self.control_queues.queues[number].receive():
                    info = dict(queue=number, bytes=packet['size'])
                    if number == 4 and packet['payload'] is not None:
                        decoded = decode_runtime_message(packet['payload'])
                        info.update(kind=decoded['kind'], sequence=decoded['sequence'])
                        greeted |= decoded['kind'] == 0 and decoded['sequence'] == 0
                    self.observed_packets.append(info)
            self.control_queues.replenish()
            if greeted:
                return
            time.sleep(.01)
        raise TimeoutError('ControlApp greeting did not arrive')

    def _publish_wifi(self, timeout):
        self.dart.invalidate_cache()
        iova = 0x14000000
        if self.dart.iotranslate(2, iova, ARENA_SIZE + POOL_SIZE) != [(None, ARENA_SIZE + POOL_SIZE)]:
            raise ValueError('Wi-Fi context or pool overlaps an existing mapping')
        physical = self.pages.memalign(0x4000, ARENA_SIZE)
        pool_iova = iova + ARENA_SIZE
        pool_physical = self.pages.memalign(0x4000, POOL_SIZE)
        self.dart.iomap_at(2, iova, physical, ARENA_SIZE)
        self.dart.iomap_at(2, pool_iova, pool_physical, POOL_SIZE)
        self.u.iface.writemem(physical, make_wifi_context(iova))
        self.p.dc_cvac(physical, 0x4000)
        self._flush_domain(self.dart, 2)
        # Independent232 qualifies Wi-Fi without Bluetooth or a ControlApp
        # greeting. Its window covers only PF1's mapped resource/context/pool;
        # the global arena belongs to PF0 and is not aliased in this domain.
        window = min(self.resources[1]['iova'], iova)
        window_end = max(self.resources[1]['iova'] + self.resources[1]['bytes'],
                         pool_iova + POOL_SIZE)
        self.u.write(self.wifi_bank + R.WIFI_IPC_HANDSHAKE, 1, 32)
        self.driver.wait32(self.wifi_bank + R.WIFI_IPC_STATE, 0xffffffff, 1, 'Wi-Fi IPC handshake', timeout=timeout)
        for offset, value in ((R.WIFI_DMA_WINDOW_BASE_LOW, window), (R.WIFI_DMA_WINDOW_BASE_HIGH, window >> 32),
                              (R.WIFI_DMA_WINDOW_LENGTH, window_end - window),
                              (R.WIFI_IPC_CONTEXT_LOW, iova), (R.WIFI_IPC_CONTEXT_HIGH, iova >> 32)):
            self.u.write(self.wifi_bank + offset, value, 32)
        self.u.write(self.wifi_bank + R.WIFI_IPC_HANDSHAKE, 2, 32)
        self.driver.wait32(self.wifi_bank + R.WIFI_IPC_STATE, 0xffffffff, 2, 'Wi-Fi context publication', timeout=timeout)
        self.wifi = WiFiControl(self.u, self.driver, iova, physical, self.pages)
        self.wifi_queues = WiFiBasicQueues(self.wifi, pool_iova, pool_physical)
        self.wifi_queues.configure()

    def snapshot(self):
        if self.closed:
            raise RuntimeError('Runtime host DMA has been stopped')
        return dict(wifi=self.wifi.snapshot(), bluetooth=self.bluetooth.transport.snapshot(),
                    control=self.control.snapshot()['ipc_state'],
                    resources={function: {**resource, 'tag': resource['tag'].decode('ascii')}
                               for function, resource in self.resources.items()},
                    observed_control_packets=list(self.observed_packets))

    def close(self):
        if self.closed:
            return
        try:
            for function in (1, 0, 2):
                command = self.driver.cfg(1, 0, function, R.PCI_COMMAND, 2)
                self.u.write(self.config + (function << 12) + R.PCI_COMMAND,
                             command & ~R.PCI_COMMAND_BUS_MASTER, 16)
            if self.bluetooth is not None:
                self.bluetooth.close()
            if self.control is not None:
                self.control.close()
        finally:
            self.dart.dart.regs.DISABLE_STREAMS[0].val = (1 << 1) | (1 << 2) | (1 << 3)
            self.closed = True
