# SPDX-License-Identifier: GPL-2.0-only
"""Owned global control context, reconstructed from bounded runtime metadata.

Boot-17 captured this allocation before the IPC publish handshake. The
initial state words are host-supplied values, not evidence of device readiness.
Unknown flags and queue directions retain neutral names pending experiments.
"""

import struct
import time

import centauri_regs as R
from probe_bootstrap_dma import ZeroedPages


CONTEXT_HEADER_SIZE = 0x68
CONTEXT_EXTENT = 0x3b0
CONTEXT_FORMAT_WORD = 0x00680300
CONTEXT_STATE = 0x68
CONTEXT_DEVICE_INDICES = 0x78
CONTEXT_HOST_INDICES = 0x94
CONTEXT_SUBMISSION_RING = 0xb0
CONTEXT_COMPLETION_RING = 0x1b0
# Preserve the initial capture's neutral labels for its snapshot consumers.
CONTEXT_INDEX_PAIR_A = CONTEXT_DEVICE_INDICES
CONTEXT_INDEX_PAIR_B = CONTEXT_HOST_INDICES
CONTEXT_RING_A = CONTEXT_SUBMISSION_RING
CONTEXT_RING_B = CONTEXT_COMPLETION_RING
CONTEXT_AUXILIARY_AREA = 0x2b0
CONTEXT_RING_ENTRIES = 16
CONTEXT_RING_ENTRY_SIZE = 16
CONTROL_FRAME_AREA = 0x1000
CONTROL_FRAME_STRIDE = 0x80


def make_context(iova):
    """Build the observed pre-publish host metadata with relocated pointers."""
    if iova < 0 or iova & 0x3fff or iova + 0x4000 > 1 << 36:
        raise ValueError('Context IOVA must be a 16-KiB page inside the DART address space')
    data = bytearray(CONTEXT_EXTENT)
    struct.pack_into('<Q', data, 0, CONTEXT_FORMAT_WORD)
    for field, target in ((0x08, CONTEXT_STATE), (0x10, CONTEXT_INDEX_PAIR_A),
                          (0x18, CONTEXT_INDEX_PAIR_A + 2),
                          (0x20, CONTEXT_INDEX_PAIR_B), (0x28, CONTEXT_INDEX_PAIR_B + 2),
                          (0x34, CONTEXT_COMPLETION_RING), (0x3c, CONTEXT_SUBMISSION_RING),
                          (0x58, CONTEXT_AUXILIARY_AREA)):
        struct.pack_into('<Q', data, field, iova + target)
    # One completion cursor and thirteen source-queue cursors per side.
    # Independent221 changes both u16 ring sizes to8 and wraps twice.
    struct.pack_into('<HH', data, 0x30, 1, 13)
    struct.pack_into('<HH', data, 0x44, CONTEXT_RING_ENTRIES, CONTEXT_RING_ENTRIES)
    # Independent222 qualifies zero at+0x48 through generated Wi-Fi queries
    # and a Bluetooth connection/role/disconnect. Its role stays unassigned.
    struct.pack_into('<II', data, CONTEXT_STATE, 2, 2)
    return bytes(data)


class ControlIPC:
    """Publish a host-owned context using runtime PF0's observed handshake.

    ``retain_domain`` is for firmware-primed handoffs. The previous driver
    must be stopped and all of its DMA allocations must stay alive. Adding
    an unused IOVA preserves firmware boot-buffer mappings. ``close`` stops
    this stream's DMA; it does not resume the previous driver.
    """

    def __init__(self, utils, driver, iova=0x01000000, retain_domain=False):
        from m1n1.hw.dart import DART
        from m1n1.hw.dart8110 import R_TCR, R_TTBR, R_TLB_OP

        data = make_context(iova)
        if driver.cfg(1, 0, 0, 0) != (R.CENTAURI_CONTROL_DEVICE << 16 | R.PCI_VENDOR_APPLE):
            raise RuntimeError('Global IPC requires the runtime control function')
        self.u, self.driver, self.iova, self.sid = utils, driver, iova, 1
        self.config = driver.ecam + (1 << 20)
        self.command = driver.cfg(1, 0, 0, R.PCI_COMMAND, 2)
        if not retain_domain and self.command & R.PCI_COMMAND_BUS_MASTER:
            raise RuntimeError('IPC setup requires endpoint DMA initially disabled')
        self.bank = driver.runtime_bars[0][0]['physical'] + R.CONTROL_BANK
        if utils.read(self.bank + R.BOOTSTRAP_STATUS, 32) != R.CONTROL_IMAGE_READY:
            raise RuntimeError('Complete the runtime image transfer before global IPC')
        if utils.read(self.bank + R.CONTROL_IPC_STATE, 32) != 0:
            raise RuntimeError('An IPC context is already published')
        self.dart = DART.from_adt(utils, '/arm-io/dart-apcie0')
        self.pages = ZeroedPages(utils)
        self.dart.dart.u = self.pages
        self.regs = self.dart.dart.regs
        if retain_domain:
            # The caller must have stopped the previous host driver while
            # preserving its DMA allocations. Never replace or reuse those
            # mappings: firmware may continue accessing its boot buffers.
            tcr = self.regs.TCR[self.sid].reg
            if not tcr.TRANSLATE_ENABLE or tcr.BYPASS_DART or not self.regs.TTBR[self.sid].reg.VALID:
                raise RuntimeError('Retaining DMA requires an existing translated domain')
            if self.dart.iotranslate(self.sid, iova, 0x4000) != [(None, 0x4000)]:
                raise ValueError('IPC context would overwrite an existing DMA mapping')
            self.command &= ~R.PCI_COMMAND_BUS_MASTER
        else:
            self.regs.DISABLE_STREAMS[0].val = 1 << self.sid
            self.dart.dart.enabled_streams &= ~(1 << self.sid)
            self.regs.TCR[self.sid].reg = R_TCR(TRANSLATE_ENABLE=1)
            self.regs.TTBR[self.sid].reg = R_TTBR(VALID=0)
        self.physical = self.pages.memalign(0x4000, 0x4000)
        utils.iface.writemem(self.physical, data)
        self.dart.iomap_at(self.sid, iova, self.physical, 0x4000)
        for address, size in self.pages.pages:
            utils.proxy.dc_cvac(address, size)
        if retain_domain:
            # iomap_at also updates parent tables allocated by the old owner.
            for address in self.dart.dart.pt_cache:
                utils.proxy.dc_cvac(address, 0x4000)
        self.regs.TLB_OP.val = R_TLB_OP(STREAM=self.sid, OP=1).value
        dart_base = utils.adt['/arm-io/dart-apcie0'].get_reg(0)[0]
        driver.wait32(dart_base + 0x80, 1 << 31, 0, 'IPC DART invalidation', timeout=1)
        if self.dart.iotranslate(self.sid, iova, CONTEXT_EXTENT) != [(self.physical, CONTEXT_EXTENT)]:
            raise RuntimeError('IPC context does not map to the owned allocation')
        port = driver.node.get_reg(8)[0]
        mapping = R.PORT_RID_TO_SID_VALID | self.sid << R.PORT_RID_TO_SID_STREAM_SHIFT | 0x0100
        utils.write(port + R.PORT_RID_TO_SID_TABLE + 4, mapping, 32)

    def publish(self, timeout=3):
        try:
            self.u.write(self.config + R.PCI_COMMAND, self.command | R.PCI_COMMAND_BUS_MASTER, 16)
            self.u.write(self.bank + R.CONTROL_IPC_HANDSHAKE, 1, 32)
            self.driver.wait32(self.bank + R.CONTROL_IPC_STATE, 0xffffffff, 1,
                               'IPC handshake state 1', timeout=timeout)
            self.u.write(self.bank + R.CONTROL_IPC_ADDRESS_LOW, self.iova & 0xffffffff, 32)
            self.u.write(self.bank + R.CONTROL_IPC_ADDRESS_HIGH, self.iova >> 32, 32)
            self.u.write(self.bank + R.CONTROL_IPC_HANDSHAKE, 2, 32)
            self.driver.wait32(self.bank + R.CONTROL_IPC_STATE, 0xffffffff, 2,
                               'IPC handshake state 2', timeout=timeout)
            return self.snapshot()
        except BaseException:
            self.close()
            raise

    def snapshot(self):
        self.u.proxy.dc_ivac(self.physical, 0x4000)
        data = self.u.iface.readmem(self.physical, CONTEXT_EXTENT)
        return dict(iova=hex(self.iova), physical=hex(self.physical),
                    image_state=hex(self.u.read(self.bank + R.BOOTSTRAP_STATUS, 32)),
                    ipc_state=hex(self.u.read(self.bank + R.CONTROL_IPC_STATE, 32)),
                    dart_error=hex(self.regs.ERROR.val),
                    state_words=[hex(v) for v in struct.unpack_from('<4I', data, CONTEXT_STATE)],
                    index_pairs=[list(struct.unpack_from('<2H', data, o))
                                 for o in (CONTEXT_INDEX_PAIR_A, CONTEXT_INDEX_PAIR_B)],
                    rings={hex(o): [(i, hex(a), hex(b)) for i, (a, b) in enumerate(
                        struct.iter_unpack('<QQ', data[o:o + 0x100])) if a or b]
                        for o in (CONTEXT_RING_A, CONTEXT_RING_B)},
                    auxiliary_words=[(hex(CONTEXT_AUXILIARY_AREA + i * 4), hex(v))
                                     for i, v in enumerate(struct.unpack_from(
                                         '<64I', data, CONTEXT_AUXILIARY_AREA)) if v])

    def submit(self, frame, timeout=3):
        """Transfer one observed control frame and verify its DMA completion.

        Each ring slot retains a separate 128-byte frame buffer. Completion
        echoes the transferred byte count and cookie; it does not establish
        the higher-level command's result. Only one command may be outstanding.
        """
        if len(frame) not in (4, 52):
            raise ValueError('Only observed four- and 52-byte control frames are supported')
        before = self.snapshot()
        if before['image_state'] != '0x2' or before['ipc_state'] != '0x2':
            raise RuntimeError('Firmware and IPC must be ready before submission')
        device, host = before['index_pairs']
        producer = host[1]
        if not all(0 <= value < CONTEXT_RING_ENTRIES for value in device + host):
            raise RuntimeError('Control index outside the observed ring')
        if device != [producer, producer]:
            raise RuntimeError('A previous control transfer is still outstanding')
        mask = CONTEXT_RING_ENTRIES - 1
        pending = (device[0] - host[0]) & mask
        if pending > 1:
            raise RuntimeError('Unexpected number of unacknowledged completions')
        if pending:
            address = self.physical + CONTEXT_COMPLETION_RING + host[0] * CONTEXT_RING_ENTRY_SIZE
            self.u.iface.writemem(address, bytes(CONTEXT_RING_ENTRY_SIZE))
            self.u.proxy.dc_cvac(address, CONTEXT_RING_ENTRY_SIZE)

        frame_offset = CONTROL_FRAME_AREA + producer * CONTROL_FRAME_STRIDE
        entry_offset = CONTEXT_SUBMISSION_RING + producer * CONTEXT_RING_ENTRY_SIZE
        self.u.iface.writemem(self.physical + frame_offset, frame)
        self.u.iface.writemem(self.physical + entry_offset,
            struct.pack('<IQI', len(frame) << 8 | 1, self.iova + frame_offset, producer))
        self.u.proxy.dc_cvac(self.physical + frame_offset, len(frame))
        self.u.proxy.dc_cvac(self.physical + entry_offset, CONTEXT_RING_ENTRY_SIZE)
        next_producer = (producer + 1) & mask
        self.u.write(self.physical + CONTEXT_HOST_INDICES, device[0], 16)
        self.u.write(self.physical + CONTEXT_HOST_INDICES + 2, next_producer, 16)
        self.u.proxy.dc_cvac(self.physical + CONTEXT_HOST_INDICES, 4)
        self.u.write(self.bank + R.CONTROL_SUBMISSION_DOORBELL, next_producer, 32)
        start = time.monotonic()
        while True:
            state = self.snapshot()
            if state['index_pairs'][0] == [next_producer, next_producer]:
                address = self.physical + CONTEXT_COMPLETION_RING + producer * CONTEXT_RING_ENTRY_SIZE
                words = struct.unpack('<4I', self.u.iface.readmem(address, CONTEXT_RING_ENTRY_SIZE))
                if words[0] != 0x401 or words[1] != ((len(frame) << 16) | producer):
                    raise RuntimeError(f'Unexpected control completion: {words!r}')
                return dict(slot=producer, cookie=producer, bytes_completed=len(frame),
                            completion_words=[hex(word) for word in words],
                            elapsed_ms=(time.monotonic() - start) * 1000)
            if time.monotonic() - start >= timeout:
                raise TimeoutError(f'Control transfer is still pending: {state!r}')
            time.sleep(0.001)

    def close(self):
        try:
            self.u.write(self.config + R.PCI_COMMAND, self.command, 16)
        finally:
            self.regs.DISABLE_STREAMS[0].val = 1 << self.sid


BETA_CONTEXT_METADATA_SIZE = 0xd4
BETA_CONTEXT_QUEUE_COUNT = 22
BETA_CONTEXT_HOST_INDICES = 0xa6
BETA_CONTEXT_SUBMISSION_RING = 0xd4
BETA_CONTEXT_COMPLETION_RING = 0x1d4
BETA_CONTEXT_COMPLETION_ENTRY_SIZE = 32
BETA_EXTENDED_CONTROL_COMPLETION = 1 << 26
BETA_CONTEXT_AUXILIARY_AREA = 0x3d8


def make_beta_context(iova, completion_entry_bytes=32):
    """Build the sibling's observed header, state and initially zero indices.

    Reference-53 supplies this exact prefix. Bare target-55 publishes it and
    receives a 52-byte queue-setup transport completion. The caller must zero
    the rest of its owned 16-KiB context page before publication; the unknown
    auxiliary area's contents and geometry are not inferred here.
    """
    # Retain the common header and its existing alignment/address validation.
    control = make_context(iova)
    data = bytearray(BETA_CONTEXT_METADATA_SIZE)
    data[:CONTEXT_HEADER_SIZE] = control[:CONTEXT_HEADER_SIZE]
    for field, target in ((0x20, BETA_CONTEXT_HOST_INDICES),
                          (0x28, BETA_CONTEXT_HOST_INDICES + 2),
                          (0x34, BETA_CONTEXT_COMPLETION_RING),
                          (0x3c, BETA_CONTEXT_SUBMISSION_RING),
                          (0x58, BETA_CONTEXT_AUXILIARY_AREA)):
        struct.pack_into('<Q', data, field, iova + target)
    struct.pack_into('<I', data, 0x30, BETA_CONTEXT_QUEUE_COUNT << 16 | 1)
    # Independent226 qualifies zero at +0x48 on this sibling as well, through
    # a real Bluetooth connection, role response and disconnect.
    # Owned context-word probe: zero selects 16-byte control completions;
    # bit 26 selects 32-byte entries. Inline queue tag status is independent.
    if completion_entry_bytes not in (16, 32):
        raise ValueError('Unsupported control completion size')
    struct.pack_into('<I', data, 0x50, BETA_EXTENDED_CONTROL_COMPLETION if completion_entry_bytes == 32 else 0)
    struct.pack_into('<II', data, CONTEXT_STATE, 2, 2)
    return bytes(data)
