# SPDX-License-Identifier: GPL-2.0-only
"""Standalone host-side Centauri bring-up, using the bootloader proxy.

This module never starts a hypervisor or an Apple operating system. Its default
probe reads transport state; optional initialization enables the controller.
It is a hardware bring-up prototype, not a functional network driver yet.
"""

import argparse
import json
import os
import struct
import time

import centauri_regs as R


class Centauri:
    def __init__(self, utils):
        self.u = utils
        self.node = utils.adt["/arm-io/apcie"]
        self.ecam, self.ecam_size = self.node.get_reg(0)
        self.pcie_caps = {}
        self.assigned_bars = {}
        self.runtime_bars = {}

    def update32(self, address, clear=0, set_bits=0):
        self.u.write(address, (self.u.read(address, 32) & ~clear) | set_bits, 32)

    def wait32(self, address, mask, expected, description, timeout=0.25):
        deadline = time.monotonic() + timeout
        while True:
            value = self.u.read(address, 32)
            if value & mask == expected:
                return value
            if time.monotonic() >= deadline:
                raise TimeoutError(f"{description}: {address:#x}={value:#x}, "
                                   f"expected mask {mask:#x}={expected:#x}")
            time.sleep(0.0001)

    def apply_tunables(self, property_name, index, source=None):
        base, size = self.node.get_reg(index)
        source = self.node if source is None else source
        records = source.getprop(property_name)
        if isinstance(records, (bytes, bytearray)):
            entries = struct.iter_unpack("<IIQQ", records)
        else:
            entries = ((item.offset, item.size, item.mask, item.value) for item in records)
        for offset, access_size, mask, value in entries:
            if access_size not in (4, 8) or offset + access_size > size:
                raise ValueError(f"Tunable outside {property_name} register bank")
            address, width = base + offset, access_size * 8
            old = self.u.read(address, width)
            updated = (old & ~mask) | (value & mask)
            if updated != old:
                self.u.write(address, updated, width)

    def initialize_root(self):
        """Initial T8142 controller experiment, before endpoint initialization.

        Clock-request/reset conventions follow Gravity/Asahi src/pcie.c.
        Bank locations and the extra global-PHY steps are from the operator
        ADT and boot-08 runtime transactions. No endpoint DMA is configured.
        """
        common = self.node.get_reg(2)[0] + 0x4000
        phy = self.node.get_reg(3)[0]
        rc = self.node.get_reg(1)[0]
        perst = self.perst_state()
        self.update32(int(perst["address"], 0),
                      clear=R.GPIO_PIN_MODE_MASK | R.GPIO_PIN_DATA)
        self.u.write(rc + 4, 0, 32)
        self.apply_tunables("apcie-phy-tunables", 2)
        self.update32(common, clear=R.PHY_COMMON_CLOCK_MODE_MASK)
        self.update32(phy + R.PHY_CONTROL, set_bits=R.PHY_CLK0_REQUEST)
        self.wait32(phy + R.PHY_CONTROL, R.PHY_CLK0_ACK, R.PHY_CLK0_ACK,
                    "Global PHY CLK0 acknowledgement")
        self.update32(phy + R.PHY_CONTROL, set_bits=R.PHY_CLK1_REQUEST)
        self.wait32(phy + R.PHY_CONTROL, R.PHY_CLK1_ACK, R.PHY_CLK1_ACK,
                    "Global PHY CLK1 acknowledgement")
        self.update32(phy + R.PHY_CONTROL, clear=R.PHY_RESET)
        self.update32(phy + R.PHY_STARTUP_CONTROL, set_bits=1)
        # Boot-09 establishes bank 4 by matching both metadata tables to
        # the observed PHY-IP accesses, including the final bit-16 clear.
        self.apply_tunables("apcie-phy-ip-pll-tunables", 4)
        self.apply_tunables("apcie-phy-ip-auspma-tunables", 4)
        phy_ip = self.node.get_reg(4)[0]
        self.update32(phy_ip + R.PHY_IP_REG_90, clear=R.PHY_IP_REG_90_BIT16)
        # T8142 differs from the older generic sequence: runtime records
        # 70..86 set this bit, wait for ready, then switch the common clock.
        self.update32(phy + R.PHY_STARTUP_CONTROL, set_bits=0x10)
        self.wait32(phy + R.PHY_CLOCK_STATUS, R.PHY_CLOCK_READY, R.PHY_CLOCK_READY,
                    "Global PHY clock ready")
        self.update32(common, clear=R.PHY_COMMON_CLOCK_MODE_MASK,
                      set_bits=R.PHY_COMMON_CLOCK_MODE_RUN)
        self.update32(phy, set_bits=0x08000000)
        self.u.write(rc + 0x54, 0x140, 32)
        self.u.write(rc + R.ROOT_CONTROL, R.ROOT_CONTROL_RUN, 32)
        self.wait32(rc + R.ROOT_STATUS, R.ROOT_STATUS_READY, R.ROOT_STATUS_READY,
                    "Root controller ready")
        self.initialize_port()

    def initialize_port(self):
        """Initialize only port 0, preserving the shared PHY and chip power."""
        port = self.node.get_reg(8)[0]
        # Basic port setup from the pre-enumeration transaction prefix.
        # IRQ/MSI and endpoint DMA remain unconfigured at this stage.
        for offset, value in ((0x88, 0x110), (0x80, 0), (0x84, 0),
                              (R.PORT_MSI_CONFIG, 0x100),
                              (R.PORT_MSI_ADDRESS_LOW, 0), (0x13c, 0),
                              (0x800, 0x100100), (0x808, 0x1001ff),
                              (0x82c, 0x10000), (0x130, 0x03020000),
                              (0x140, 0x10), (0x144, 0x253770)):
            self.u.write(port + offset, value, 32)
        self.apply_tunables("apcie-config-tunables", 8, self.node["pci-bridge0"])
        self.update32(port + R.PORT_APP_CLOCK_CONTROL, set_bits=R.PORT_APP_CLOCK_ENABLE)
        # T8142 keeps the per-port PHY in reset during root enumeration.
        # Its requests are enabled later (reference records 828..850).
        self.update32(port + R.PORT_RESET_CONTROL, set_bits=R.PORT_RESET_RELEASE)
        self.wait32(port + R.PORT_STATUS, R.PORT_RUNNING, R.PORT_RUNNING, "Port running")
        debug = self.node.get_reg(9)[0]
        for offset, value in ((0x10, 2), (0x1c, 8), (8, 0x3fff),
                              (0x0c, 0xffffc000), (0x20, 2), (0x14, 1)):
            self.u.write(debug + offset, value, 32)
        self.update32(port + R.PORT_APP_CLOCK_CONTROL, clear=R.PORT_APP_CLOCK_GATING_DISABLE)
        self.wait32(port + R.PORT_LINK_STATUS, R.PORT_LINK_BUSY, 0, "Port initialization idle")

    def root_ready(self):
        if self.cfg(0, 0, 0, 0) != 0x100c106b:
            return False
        port = self.node.get_reg(8)[0]
        return (bool(self.u.read(port + R.PORT_STATUS, 32) & R.PORT_RUNNING) and
                not self.u.read(port + R.PORT_LINK_STATUS, 32) & R.PORT_LINK_BUSY)

    def reset_key_state(self, output_level=None):
        """Access only the SMC key named by the internal bridge's reset function."""
        from m1n1.fw.smc import SMCClient

        reset = self.node["pci-bridge0"].getprop("function-reset")
        if reset.name != "pKW4" or len(reset.args) != 2:
            raise ValueError("Unsupported bridge reset function")
        key = int(reset.args[0]).to_bytes(4, "big").decode("ascii")
        smc = SMCClient(self.u, self.u.adt["/arm-io/smc"].get_reg(0)[0], None)
        smc.start()
        smc.start_ep(0x20)
        try:
            length, typecode, flags = smc.smcep.get_key_info(key)
            if length != 4:
                raise ValueError(f"Unexpected reset-key size: {length}")
            if output_level is not None:
                smc.smcep.write32(key, int(reset.args[1]) | int(bool(output_level)))
            value = smc.smcep.read32(key)
            return dict(key=key, value=hex(value), type=typecode, flags=flags,
                        adt_argument=hex(reset.args[1]))
        finally:
            smc.stop()

    def perst_state(self):
        """Read the GPIO pin explicitly named by the bridge PERST function."""
        reference = self.node["pci-bridge0"].getprop("function-perst")
        if reference.name != "GPIO" or len(reference.args) != 2:
            raise ValueError("Unsupported bridge PERST function")
        controller = next(n for n in self.u.adt.walk_tree()
                          if n.getprop("AAPL,phandle") == reference.phandle)
        pin = int(reference.args[0])
        base, size = controller.get_reg(0)
        if pin * 4 + 4 > size:
            raise ValueError("PERST pin is outside its GPIO register bank")
        value = self.u.read(base + pin * 4, 32)
        return dict(controller=controller.name, pin=pin, address=hex(base + pin * 4),
                    register=hex(value), level=bool(value & R.GPIO_PIN_DATA),
                    mode=(value & R.GPIO_PIN_MODE_MASK) >> R.GPIO_PIN_MODE_SHIFT)

    def start_link(self, power_on=True):
        """Initial standalone chip-release and PCIe link-training experiment."""
        # Program topology while the endpoint is still held in reset, as in
        # the reference trace, after the controller has become idle.
        self.u.write(self.ecam + R.PCI_PRIMARY_SECONDARY_SUBORDINATE_BUS, 0x00010100, 32)
        pin = self.perst_state()
        if pin["mode"] not in (R.GPIO_MODE_INPUT, R.GPIO_MODE_OUTPUT):
            raise RuntimeError("Unexpected PERST pad mode")
        # Preserve pad/pull configuration; PERST is active low.
        self.update32(int(pin["address"], 0), clear=R.GPIO_PIN_MODE_MASK | R.GPIO_PIN_DATA,
                      set_bits=R.GPIO_MODE_OUTPUT << R.GPIO_PIN_MODE_SHIFT)
        reset = self.reset_key_state(output_level=True) if power_on else None
        time.sleep(0.1)
        port_phy = self.node.get_reg(10)[0]
        self.update32(port_phy + R.PHY_CONTROL, set_bits=R.PHY_CLK0_REQUEST)
        self.wait32(port_phy + R.PHY_CONTROL, R.PHY_CLK0_ACK, R.PHY_CLK0_ACK,
                    "Port PHY CLK0 acknowledgement")
        self.update32(port_phy + R.PHY_CONTROL, set_bits=R.PHY_CLK1_REQUEST)
        self.wait32(port_phy + R.PHY_CONTROL, R.PHY_CLK1_ACK, R.PHY_CLK1_ACK,
                    "Port PHY CLK1 acknowledgement")
        self.update32(port_phy + R.PHY_CONTROL, clear=R.PHY_RESET,
                      set_bits=R.PHY_REFERENCE_CLOCK_ENABLE)
        self.update32(int(pin["address"], 0), set_bits=R.GPIO_PIN_DATA)
        port = self.node.get_reg(8)[0]
        self.update32(port + R.PORT_LTSSM_CONTROL, set_bits=R.PORT_LTSSM_ENABLE)
        self.update32(port_phy, set_bits=0x80000000)
        # UP can remain latched through a failed reset. GPIO-only trial213
        # has UP|BUSY and inaccessible config space; it is not a trained link.
        self.wait32(port + R.PORT_LINK_STATUS, R.PORT_LINK_UP | R.PORT_LINK_BUSY,
                    R.PORT_LINK_UP,
                    "Centauri PCIe link up", timeout=3)
        time.sleep(0.1)
        return reset

    def warm_restart_after_image(self, request_d3hot=False, *, request_pme=True):
        """Re-enumerate after image-ready without cycling Centauri power.

        The SMC power pin and shared PHY stay active, preserving the image.
        Reduced resets206/207/211/212 qualify the named clock/reset controls
        and the measured quiesce handshake without port/debug presets.
        """
        if self.cfg(1, 0, 0, 0) != (R.CENTAURI_BOOTSTRAP_DEVICE << 16 | R.PCI_VENDOR_APPLE):
            raise RuntimeError('Expected the bootstrap function before warm restart')
        startup = self.assigned_bars[0]['physical'] + R.BOOTSTRAP_BANK
        if self.u.read(startup + R.BOOTSTRAP_STATUS, 32) != R.BOOTSTRAP_IMAGE_READY:
            raise RuntimeError('The bootstrap image is not ready')
        endpoint = self.ecam + (1 << 20)
        command = self.cfg(1, 0, 0, R.PCI_COMMAND, 2)
        self.u.write(endpoint + R.PCI_COMMAND, command & ~R.PCI_COMMAND_BUS_MASTER, 16)
        if request_d3hot:
            # Native124 requests D3hot+PME after the initial ready4, before
            # resetting the port. Do not issue further BAR reads in D3hot.
            capability = self.capability(1, 0, 0, R.PCI_CAP_ID_PM)
            if capability is None:
                raise RuntimeError('Bootstrap function has no PCI power-management capability')
            offset = capability + R.PCI_PM_CONTROL
            control = self.cfg(1, 0, 0, offset, 2)
            control = ((control & ~(R.PCI_PM_STATE_MASK | R.PCI_PM_PME_STATUS |
                                    R.PCI_PM_PME_ENABLE)) | R.PCI_PM_STATE_D3HOT |
                       (R.PCI_PM_PME_ENABLE if request_pme else 0))
            self.u.write(endpoint + offset, control, 16)
            readback = self.cfg(1, 0, 0, offset, 2)
            if readback & R.PCI_PM_STATE_MASK != R.PCI_PM_STATE_D3HOT:
                raise RuntimeError('Bootstrap function did not enter D3hot')
            if bool(readback & R.PCI_PM_PME_ENABLE) != bool(request_pme):
                raise RuntimeError('Bootstrap PME enable did not match the requested state')
            # Linux's PCI_PM_D3HOT_WAIT is10ms; allow that transition time.
            time.sleep(.010)
        self.u.write(self.ecam + R.PCI_COMMAND, 0, 16)
        self.u.write(self.ecam + R.PCI_PRIMARY_SECONDARY_SUBORDINATE_BUS, 0, 32)
        port = self.node.get_reg(8)[0]
        phy = self.node.get_reg(10)[0]
        self.update32(port + R.PORT_APP_CLOCK_CONTROL, set_bits=R.PORT_APP_CLOCK_GATING_DISABLE)
        # Clear stale completion before issuing a new request. Direct211
        # isolates bit4's W1C behavior and bit0's completion; no resume bit or
        # opaque startup preset is required by the reduced reset in212.
        self.u.write(port + R.PORT_QUIESCE_CONTROL, R.PORT_QUIESCE_COMPLETE, 32)
        self.u.write(port + R.PORT_QUIESCE_CONTROL, R.PORT_QUIESCE_REQUEST, 32)
        self.wait32(port + R.PORT_QUIESCE_CONTROL, R.PORT_QUIESCE_COMPLETE,
                    R.PORT_QUIESCE_COMPLETE, 'Port quiesce completion')
        pin = self.perst_state()
        self.update32(int(pin['address'], 0), clear=R.GPIO_PIN_DATA)
        self.update32(phy + R.PHY_CONTROL, clear=0x80000000)
        # The UP bit remains latched during the reference's reset transition;
        # do not wait for it to clear before resetting the controller port.
        return self.finish_port_restart()

    def finish_port_restart(self):
        """Complete a port restart after PERST and PHY bit 31 are low."""
        port = self.node.get_reg(8)[0]
        phy = self.node.get_reg(10)[0]
        self.update32(port + R.PORT_RESET_CONTROL, clear=R.PORT_RESET_RELEASE)
        self.update32(phy + R.PHY_CONTROL, clear=R.PHY_REFERENCE_CLOCK_ENABLE)
        self.update32(phy + R.PHY_CONTROL, set_bits=R.PHY_RESET)
        self.update32(phy + R.PHY_CONTROL, clear=R.PHY_CLK0_REQUEST)
        self.wait32(phy + R.PHY_CONTROL, R.PHY_CLK0_ACK, 0, 'Warm-reset PHY CLK0 off')
        self.update32(phy + R.PHY_CONTROL, clear=R.PHY_CLK1_REQUEST)
        self.wait32(phy + R.PHY_CONTROL, R.PHY_CLK1_ACK, 0, 'Warm-reset PHY CLK1 off')
        self.update32(port + R.PORT_APP_CLOCK_CONTROL, clear=R.PORT_APP_CLOCK_ENABLE)
        self.wait32(port + R.PORT_STATUS, R.PORT_RUNNING, 0, 'Warm-reset port stopped')
        # Keep Linux/host-owned interrupt and requester-routing configuration.
        # Full initialize_port resets MSI registers and replays unrelated
        # tunables/debug presets;206/207/211/212 qualify their omission here.
        self.update32(port + R.PORT_APP_CLOCK_CONTROL, set_bits=R.PORT_APP_CLOCK_ENABLE)
        self.update32(port + R.PORT_RESET_CONTROL, set_bits=R.PORT_RESET_RELEASE)
        self.wait32(port + R.PORT_STATUS, R.PORT_RUNNING, R.PORT_RUNNING, 'Port running')
        self.update32(port + R.PORT_APP_CLOCK_CONTROL, clear=R.PORT_APP_CLOCK_GATING_DISABLE)
        self.wait32(port + R.PORT_LINK_STATUS, R.PORT_LINK_BUSY, 0, 'Port initialization idle')
        self.u.write(port + R.PORT_QUIESCE_CONTROL, R.PORT_QUIESCE_COMPLETE, 32)
        self.start_link(power_on=False)
        self.assigned_bars = {}
        self.runtime_bars = {}
        return self.probe()

    def publish_host_time(self, functions=(0,), timestamp_us=None):
        """Publish Unix microseconds through observed PCI configuration words.

        Native124 writes PF0 before the first image submission as well as
        after runtime enumeration. Earlier direct probes only did the latter.
        This helper does not claim that timestamp publication alone boots FW.
        """
        if timestamp_us is None:
            timestamp_us = time.time_ns() // 1000
        if not 0 <= timestamp_us < 1 << 64:
            raise ValueError('Host timestamp must fit its unsigned64-bit field')
        for function in functions:
            allowed = ((R.CENTAURI_BOOTSTRAP_DEVICE, R.CENTAURI_CONTROL_DEVICE)
                       if function == 0 else
                       (R.CENTAURI_BETA_DEVICE,) if function == 2 else ())
            if not allowed or self.cfg(1, 0, function, 0) not in tuple(
                    device << 16 | R.PCI_VENDOR_APPLE for device in allowed):
                raise RuntimeError('Host-time target is not a qualified PCI function')
        for function in functions:
            config = self.ecam + (1 << 20) + (function << 12)
            self.u.write(config + R.CENTAURI_PCI_UNIX_TIME_US_LOW,
                         timestamp_us & 0xffffffff, 32)
            self.u.write(config + R.CENTAURI_PCI_UNIX_TIME_US_HIGH,
                         timestamp_us >> 32, 32)
        return timestamp_us

    def configure_declared_dart_bypass_streams(self):
        """Qualify the control state of ADT-declared reserved streams.

        Native139 enables bypass-N streams with TCR zero and invalid TTBRs.
        Direct140 instead starts with TCR one. Rebuild only these declared
        reserved controls; do not import page tables. Their precise bypass
        semantics remain unqualified, including their relationship to MSI.
        """
        from m1n1.hw.dart import DART
        from m1n1.hw.dart8110 import R_TCR, R_TTBR, R_TLB_OP
        node = self.u.adt['/arm-io/dart-apcie0']
        absent = object()
        selected = [sid for sid in range(node.getprop('sid-count'))
                    if node.getprop(f'bypass-{sid}', absent) is not absent]
        dart = DART.from_adt(self.u, '/arm-io/dart-apcie0')
        regs = dart.dart.regs
        base = node.get_reg(0)[0]
        before = {sid:dict(tcr=hex(regs.TCR[sid].reg.value),
                           ttbr=hex(regs.TTBR[sid].reg.value)) for sid in selected}
        for sid in selected:
            if regs.TTBR[sid].reg.VALID:
                raise RuntimeError('Reserved stream has an active domain; refuse to replace it')
        for sid in selected:
            word, mask = sid // 32, 1 << (sid % 32)
            regs.DISABLE_STREAMS[word].val = mask
            regs.TTBR[sid].reg = R_TTBR(VALID=0)
            regs.TCR[sid].reg = R_TCR(0)
            regs.TLB_OP.val = R_TLB_OP(STREAM=sid, OP=1).value
            self.wait32(base + 0x80, 1 << 31, 0, 'Reserved DART stream invalidation', timeout=1)
            regs.ENABLE_STREAMS[word].val = regs.ENABLE_STREAMS[word].val | mask
            if not regs.ENABLE_STREAMS[word].val & mask or regs.TCR[sid].reg.value != 0:
                raise RuntimeError('Declared reserved stream controls did not read back')
        return dict(streams=selected, before=before,
                    enabled=hex(regs.ENABLE_STREAMS[0].val))

    def configure_dart_adt_parameters(self):
        """Apply the device's declared masked DART parameters before DMA.

        Cold129 differs from these metadata values despite having working
        page tables. Offsets retain their neutral names pending isolated
        hardware qualification; no register semantics are inferred here.
        """
        node = self.u.adt['/arm-io/dart-apcie0']
        base, extent = node.get_reg(0)
        data = node.getprop('dart-tunables-instance-0')
        if not isinstance(data, bytes) or len(data) % struct.calcsize('<IIQQ'):
            raise ValueError('Invalid declared DART parameter table')
        result = []
        for offset, size, mask, value in struct.iter_unpack('<IIQQ', data):
            if size not in (4, 8) or offset + size > extent or offset % size:
                raise ValueError('Declared DART parameter exceeds its register bank')
            before = self.u.read(base + offset, size * 8)
            requested = (before & ~mask) | (value & mask)
            if requested != before:
                self.u.write(base + offset, requested, size * 8)
            after = self.u.read(base + offset, size * 8)
            if after & mask != value & mask:
                raise RuntimeError(f'DART parameter {offset:#x} did not read back')
            result.append(dict(offset=hex(offset), mask=hex(mask),
                               before=hex(before), after=hex(after)))
        return result

    def configure_ptm_profile(self):
        """Qualify the native PTM enable profile before both image stages.

        PTM fields follow Linux pci_regs.h. The root's observed enable-only
        control differs from the generic Linux root-selection policy; this
        is a controlled prototype profile, not an upstream PCI-core quirk.
        """
        root = self.extended_capability(0, 0, 0, R.PCI_EXT_CAP_ID_PTM)
        endpoint = self.extended_capability(1, 0, 0, R.PCI_EXT_CAP_ID_PTM)
        if root is None or endpoint is None:
            raise RuntimeError('Both link partners must advertise PTM')
        root_cap = self.cfg(0, 0, 0, root + R.PCI_PTM_CAP)
        endpoint_cap = self.cfg(1, 0, 0, endpoint + R.PCI_PTM_CAP)
        granularity = max(root_cap & R.PCI_PTM_GRANULARITY_MASK,
                          endpoint_cap & R.PCI_PTM_GRANULARITY_MASK)
        self.u.write(self.ecam + root + R.PCI_PTM_CONTROL, R.PCI_PTM_ENABLE, 32)
        self.u.write(self.ecam + (1 << 20) + endpoint + R.PCI_PTM_CONTROL,
                     granularity | R.PCI_PTM_ENABLE, 32)
        return dict(root_control=hex(self.cfg(0, 0, 0, root + R.PCI_PTM_CONTROL)),
                    endpoint_control=hex(self.cfg(1, 0, 0, endpoint + R.PCI_PTM_CONTROL)),
                    effective_granularity=granularity >> 8)

    def extended_capability(self, bus, device, function, wanted):
        pointer, visited = 0x100, set()
        while 0x100 <= pointer <= 0xffc and not pointer & 3 and pointer not in visited:
            visited.add(pointer)
            header = self.cfg(bus, device, function, pointer)
            if header & 0xffff == wanted:
                return pointer
            if header in (0, 0xffffffff):
                return None
            pointer = header >> 20
        return None

    def cfg(self, bus, device, function, offset, width=4):
        if not (0 <= bus < 256 and 0 <= device < 32 and 0 <= function < 8):
            raise ValueError("Invalid PCI function")
        if width not in (1, 2, 4) or offset < 0 or offset + width > 4096:
            raise ValueError("Invalid configuration register")
        relative = (bus << 20) | (device << 15) | (function << 12) | offset
        if relative + width > self.ecam_size:
            raise ValueError("Function exceeds the advertised configuration aperture")
        return self.u.read(self.ecam + relative, width * 8)

    def capability(self, bus, device, function, wanted):
        if not self.cfg(bus, device, function, 6, 2) & 0x10:
            return None
        pointer = self.cfg(bus, device, function, 0x34, 1) & ~3
        visited = set()
        while 0x40 <= pointer <= 0xfc and pointer not in visited:
            visited.add(pointer)
            header = self.cfg(bus, device, function, pointer, 2)
            if header & 0xff == wanted:
                return pointer
            pointer = (header >> 8) & ~3
        return None

    def function(self, bus, device, function):
        identifier = self.cfg(bus, device, function, 0)
        if identifier & 0xffff in (0, 0xffff):
            return None
        header = self.cfg(bus, device, function, 0x0e, 1)
        result = dict(bdf=f"{bus:02x}:{device:02x}.{function}",
                      vendor=hex(identifier & 0xffff), device=hex(identifier >> 16),
                      revision_class=hex(self.cfg(bus, device, function, 8)),
                      header_type=header,
                      command=hex(self.cfg(bus, device, function, R.PCI_COMMAND, 2)),
                      bars=[], capabilities={})
        for name, cap_id in [('pcie', 0x10), ('msi', 0x05), ('msix', 0x11)]:
            pointer = self.capability(bus, device, function, cap_id)
            if pointer is not None:
                entry = dict(offset=hex(pointer))
                if name == 'pcie' and pointer <= 0xf4:
                    entry['device_control'] = hex(self.cfg(bus, device, function, pointer + 8, 2))
                elif name in ('msi', 'msix'):
                    message_control = self.cfg(bus, device, function, pointer + 2, 2)
                    entry['message_control'] = hex(message_control)
                    if name == 'msi' and pointer <= 0xec:
                        address = self.cfg(bus, device, function, pointer + R.PCI_MSI_ADDRESS_LOW)
                        if message_control & R.PCI_MSI_ADDRESS_64BIT:
                            address |= self.cfg(bus, device, function, pointer + R.PCI_MSI_ADDRESS_HIGH) << 32
                            data_offset = R.PCI_MSI_DATA_64BIT
                        else:
                            data_offset = R.PCI_MSI_DATA_32BIT
                        entry['address'] = hex(address)
                        entry['data'] = hex(self.cfg(bus, device, function, pointer + data_offset, 2))
                result['capabilities'][name] = entry
        index = 0
        count = 2 if header & 0x7f == 1 else 6
        while index < count:
            bar = index
            low = self.cfg(bus, device, function, 0x10 + index * 4)
            index += 1
            if low & 1:
                continue
            address = low & ~15
            if low & 6 == 4:
                if index >= count:
                    break
                address |= self.cfg(bus, device, function, 0x10 + index * 4) << 32
                index += 1
            if address:
                result["bars"].append(dict(index=bar, address=hex(address),
                                            prefetchable=bool(low & 8)))
        return result

    def size_bars(self, bus=1, device=0, function=0):
        """Use standard PCI sizing while endpoint memory decoding is disabled."""
        command = self.cfg(bus, device, function, 4, 2)
        cfg_base = self.ecam + (bus << 20) + (device << 15) + (function << 12)
        results = []
        self.u.write(cfg_base + 4, command & ~6, 16)
        try:
            index = 0
            while index < 6:
                bar = index
                offset = 0x10 + 4 * bar
                low = self.cfg(bus, device, function, offset)
                if low & 1:
                    index += 1
                    continue
                is64 = low & 6 == 4
                if is64 and bar == 5:
                    raise ValueError("Invalid 64-bit BAR at the last slot")
                high = self.cfg(bus, device, function, offset + 4) if is64 else 0
                index += 2 if is64 else 1
                try:
                    self.u.write(cfg_base + offset, 0xffffffff, 32)
                    if is64:
                        self.u.write(cfg_base + offset + 4, 0xffffffff, 32)
                    mask = self.cfg(bus, device, function, offset) & ~15
                    if is64:
                        mask |= self.cfg(bus, device, function, offset + 4) << 32
                finally:
                    if is64:
                        self.u.write(cfg_base + offset + 4, high, 32)
                    self.u.write(cfg_base + offset, low, 32)
                if not mask:
                    continue
                width_mask = (1 << (64 if is64 else 32)) - 1
                size = ((~mask) & width_mask) + 1
                if size & (size - 1):
                    raise ValueError(f"Non-power-of-two BAR size {size:#x}")
                results.append(dict(index=bar, size=hex(size), is64=is64,
                                    prefetchable=bool(low & 8), original=hex(low | high << 32)))
        finally:
            self.u.write(cfg_base + 4, command, 16)
        return results

    def pci_to_physical(self, address, size):
        for region in self.node.ranges:
            cells = region.bus_addr
            if not isinstance(cells, list) or len(cells) != 3:
                continue
            if (cells[0] >> 24) & 3 not in (2, 3):
                continue
            start = (cells[2] << 32) | cells[1]
            if start <= address and address + size <= start + region.size:
                return self.node._parent.translate(region.parent_addr + address - start)
        raise ValueError(f"PCI address {address:#x}+{size:#x} is outside the ADT apertures")

    def assign_bars(self):
        """Allocate this endpoint's memory BARs in the observed 32-bit bus aperture."""
        bootstrap_id = R.CENTAURI_BOOTSTRAP_DEVICE << 16 | R.PCI_VENDOR_APPLE
        if self.cfg(1, 0, 0, 0) != bootstrap_id:
            raise RuntimeError('BAR allocation currently supports only the bootstrap function')
        bars = self.size_bars()
        region = next(r for r in self.node.ranges
                      if isinstance(r.bus_addr, list) and (r.bus_addr[0] >> 24) & 3 == 2)
        cursor = (region.bus_addr[2] << 32) | region.bus_addr[1]
        end = cursor + region.size
        endpoint_cfg = self.ecam + (1 << 20)
        command = self.cfg(1, 0, 0, R.PCI_COMMAND, 2)
        self.u.write(endpoint_cfg + R.PCI_COMMAND,
                     command & ~(R.PCI_COMMAND_MEMORY | R.PCI_COMMAND_BUS_MASTER), 16)
        assigned = {}
        for bar in sorted(bars, key=lambda b: int(b["size"], 0), reverse=True):
            size = int(bar["size"], 0)
            address = (cursor + size - 1) & ~(size - 1)
            if address + size > end:
                raise MemoryError("Endpoint BARs exceed the internal PCI aperture")
            original = int(bar["original"], 0)
            offset = 0x10 + bar["index"] * 4
            if bar["is64"]:
                self.u.write(endpoint_cfg + offset + 4, address >> 32, 32)
            self.u.write(endpoint_cfg + offset, (address & 0xfffffff0) | (original & 15), 32)
            assigned[bar["index"]] = dict(address=address, size=size,
                                           physical=self.pci_to_physical(address, size))
            cursor = address + size
        self.configure_bridge_window(assigned.values())
        self.u.write(endpoint_cfg + R.PCI_COMMAND,
                     (command | R.PCI_COMMAND_MEMORY) & ~R.PCI_COMMAND_BUS_MASTER, 16)
        self.assigned_bars = assigned
        return {str(i): {k: hex(v) for k, v in b.items()} for i, b in assigned.items()}

    def configure_bridge_window(self, resources):
        resources = list(resources)
        first = min(b['address'] for b in resources)
        last = max(b['address'] + b['size'] - 1 for b in resources)
        # Bridge prefetch window uses 1 MiB granularity; both endpoint BARs
        # report prefetchable memory. The root bridge may forward transactions;
        # keep endpoint bus mastering disabled until DART and our DMA buffers
        # have been configured.
        self.u.write(self.ecam + R.PCI_MEMORY_BASE_LIMIT, 0x0000fff0, 32)
        self.u.write(self.ecam + R.PCI_PREFETCH_BASE_UPPER, first >> 32, 32)
        self.u.write(self.ecam + R.PCI_PREFETCH_LIMIT_UPPER, last >> 32, 32)
        window = ((first >> 16) & 0xfff0) | (last & 0xfff00000) | 0x00010001
        self.u.write(self.ecam + R.PCI_PREFETCH_BASE_LIMIT, window, 32)
        root_command = self.cfg(0, 0, 0, R.PCI_COMMAND, 2)
        self.u.write(self.ecam + R.PCI_COMMAND,
                     root_command | R.PCI_COMMAND_MEMORY | R.PCI_COMMAND_BUS_MASTER, 16)
    def assign_runtime_bars(self):
        """Allocate the three runtime functions without enabling endpoint DMA."""
        identifiers = [R.CENTAURI_CONTROL_DEVICE, R.CENTAURI_WIFI_DEVICE, R.CENTAURI_BETA_DEVICE]
        commands, bars = {}, []
        for function, identifier in enumerate(identifiers):
            if self.cfg(1, 0, function, 0) != (identifier << 16 | R.PCI_VENDOR_APPLE):
                raise RuntimeError(f'Runtime function {function} has not enumerated')
            commands[function] = self.cfg(1, 0, function, R.PCI_COMMAND, 2)
            for resource in self.size_bars(function=function):
                bars.append(dict(resource, function=function))
        region = next(r for r in self.node.ranges
                      if isinstance(r.bus_addr, list) and (r.bus_addr[0] >> 24) & 3 == 2)
        cursor = (region.bus_addr[2] << 32) | region.bus_addr[1]
        end = cursor + region.size
        assigned = {function: {} for function in commands}
        # Largest resource first; descending function number for equal BARs
        # reproduces the observed beta/alpha/control 64-KiB window ordering.
        for bar in sorted(bars, key=lambda b: (-int(b['size'], 0), -b['function'], b['index'])):
            size = int(bar['size'], 0)
            address = (cursor + size - 1) & ~(size - 1)
            if address + size > end:
                raise MemoryError('Runtime BARs exceed the PCI aperture')
            assigned[bar['function']][bar['index']] = dict(
                address=address, size=size, physical=self.pci_to_physical(address, size))
            cursor = address + size
        for function, command in commands.items():
            config = self.ecam + (1 << 20) + (function << 12)
            self.u.write(config + R.PCI_COMMAND,
                         command & ~(R.PCI_COMMAND_MEMORY | R.PCI_COMMAND_BUS_MASTER), 16)
        for bar in bars:
            config = self.ecam + (1 << 20) + (bar['function'] << 12)
            address = assigned[bar['function']][bar['index']]['address']
            offset = 0x10 + bar['index'] * 4
            if bar['is64']:
                self.u.write(config + offset + 4, address >> 32, 32)
            self.u.write(config + offset, address & 0xfffffff0 | int(bar['original'], 0) & 15, 32)
        self.configure_bridge_window(b for resources in assigned.values() for b in resources.values())
        for function, command in commands.items():
            config = self.ecam + (1 << 20) + (function << 12)
            self.u.write(config + R.PCI_COMMAND,
                         (command | R.PCI_COMMAND_MEMORY) & ~R.PCI_COMMAND_BUS_MASTER, 16)
        self.runtime_bars = assigned
        return {str(f): {str(i): {k: hex(v) for k, v in b.items()} for i, b in resources.items()}
                for f, resources in assigned.items()}

    def runtime_snapshot(self):
        """Read only runtime registers observed during reference initialization."""
        selections = {'control_boot': (0, R.BOOTSTRAP_BANK, [0, 4, 8, 12, 16]),
                      'wifi': (1, 0, [8, 12]), 'beta': (2, 0, [0, 0x18])}
        result = {}
        for name, (function, bank, offsets) in selections.items():
            base = self.runtime_bars[function][0]['physical'] + bank
            result[name] = {hex(o): hex(self.u.read(base + o, 32)) for o in offsets}
        return result

    def configure_reference_auxiliary_banks(self):
        """Apply the remaining observed port setup; field meanings unresolved.

        These are controller MMIO writes from boot-09, not values inferred
        from firmware code. Bank 13's value matches the ADT bypass-SID base.
        """
        port = self.node.get_reg(8)[0]
        aux12 = self.node.get_reg(12)[0]
        aux13 = self.node.get_reg(13)[0]
        before = dict(bank12=hex(self.u.read(aux12, 32)), bank13=hex(self.u.read(aux13, 32)))
        self.u.write(port + 0x4020, 3, 32)
        self.u.write(aux12, 0xff01, 32)
        self.update32(aux12, set_bits=1 << 31)
        self.u.write(aux13, int(self.node.getprop('first-bypass-sid')), 32)
        return dict(before=before, bank12=hex(self.u.read(aux12, 32)),
                    bank13=hex(self.u.read(aux13, 32)))

    def configure_bootstrap_pcie(self):
        """Match standard payload/read-request fields observed before bootstrap."""
        result = {}
        for bus, read_request in [(0, 0), (1, R.PCI_EXP_DEVICE_MAX_READ_REQUEST_512)]:
            cap = self.capability(bus, 0, 0, R.PCI_CAP_ID_EXPRESS)
            if cap is None:
                raise RuntimeError('Missing PCIe capability in the bootstrap path')
            address = self.ecam + (bus << 20) + cap + R.PCI_EXP_DEVICE_CONTROL
            old = self.u.read(address, 16)
            value = ((old & ~(R.PCI_EXP_DEVICE_MAX_PAYLOAD_MASK |
                             R.PCI_EXP_DEVICE_MAX_READ_REQUEST_MASK)) |
                     R.PCI_EXP_DEVICE_MAX_PAYLOAD_256 | read_request |
                     R.PCI_EXP_DEVICE_ERROR_REPORT_ENABLE)
            self.u.write(address, value, 16)
            result[str(bus)] = hex(self.u.read(address, 16))
        return result

    def enable_bootstrap_msi(self):
        """Configure the four-message MSI transport observed before bootstrap.

        MSI destination and vector-map conventions follow Linux apple-pcie
        and Gravity/Asahi src/pcie.c. This does not enable CPU AIC interrupts;
        the host prototype polls device state rather than dispatching IRQs.
        """
        if self.cfg(1, 0, 0, 0) != (R.CENTAURI_BOOTSTRAP_DEVICE << 16 | R.PCI_VENDOR_APPLE):
            raise RuntimeError('MSI setup currently supports only the bootstrap function')
        return self.enable_function_msi(0, 4, 0)

    def enable_runtime_msi(self):
        """Allocate disjoint MSI ranges observed at runtime context submission.

        These are host allocations, not fixed device register encodings.
        Control uses 0..3, beta 4..5 and Wi-Fi 16..31 of the port's vectors.
        No endpoint bus mastering or CPU interrupt delivery is enabled here.
        """
        for fn, expected in enumerate((R.CENTAURI_CONTROL_DEVICE,
                                       R.CENTAURI_WIFI_DEVICE, R.CENTAURI_BETA_DEVICE)):
            if self.cfg(1, 0, fn, 0) != (expected << 16 | R.PCI_VENDOR_APPLE):
                raise RuntimeError('Runtime MSI setup requires all three runtime functions')
        return {fn: self.enable_function_msi(fn, messages, data)
                for fn, messages, data in ((0, 4, 0), (1, 16, 16), (2, 2, 4))}

    def enable_function_msi(self, function, messages, data):
        """Program a standard MSI capability after validating the allocation."""
        if function not in (0, 1, 2) or messages not in (1, 2, 4, 8, 16, 32):
            raise ValueError('Invalid function or MSI message count')
        if data < 0 or data % messages or data + messages > R.PORT_MSI_VECTOR_COUNT:
            raise ValueError('MSI vector range must be aligned and inside the port receiver')
        cap = self.capability(1, 0, function, R.PCI_CAP_ID_MSI)
        if cap is None:
            raise RuntimeError('Function has no MSI capability')
        config = self.ecam + (1 << 20) + (function << 12)
        control = self.cfg(1, 0, function, cap + R.PCI_MSI_CONTROL, 2)
        if messages > 1 << ((control >> 1) & 7):
            raise ValueError('MSI allocation exceeds the advertised capability')
        address = int(self.node.getprop('msi-address'))
        if address >> 32 and not control & R.PCI_MSI_ADDRESS_64BIT:
            raise ValueError('MSI destination exceeds the function address width')
        # Disable emission until the receiver and destination are programmed.
        self.u.write(config + cap + R.PCI_MSI_CONTROL, control & ~R.PCI_MSI_ENABLE, 16)
        port = self.node.get_reg(8)[0]
        interrupt_to_axi = self.node.get_reg(11)[0]
        self.u.write(interrupt_to_axi + R.INTERRUPT_TO_AXI_CONTROL,
                     R.INTERRUPT_TO_AXI_ENABLE, 32)
        self.u.write(port + R.PORT_MSI_ADDRESS_LOW, address & 0xffffffff, 32)
        self.u.write(port + R.PORT_MSI_ADDRESS_HIGH, address >> 32, 32)
        for vector in range(R.PORT_MSI_VECTOR_COUNT):
            self.u.write(port + R.PORT_MSI_MAP_TABLE + vector * 4,
                         R.PORT_MSI_MAP_ENABLE | vector, 32)
        self.u.write(port + R.PORT_MSI_CONFIG, R.PORT_MSI_ENABLE, 32)
        self.u.write(config + cap + R.PCI_MSI_ADDRESS_LOW, address & 0xffffffff, 32)
        if control & R.PCI_MSI_ADDRESS_64BIT:
            self.u.write(config + cap + R.PCI_MSI_ADDRESS_HIGH, address >> 32, 32)
            data_offset = R.PCI_MSI_DATA_64BIT
        else:
            data_offset = R.PCI_MSI_DATA_32BIT
        self.u.write(config + cap + data_offset, data, 16)
        command = self.cfg(1, 0, function, R.PCI_COMMAND, 2)
        self.u.write(config + R.PCI_COMMAND, command | R.PCI_COMMAND_INTX_DISABLE, 16)
        control = ((control & ~R.PCI_MSI_MULTIPLE_ENABLE_MASK) |
                   (messages.bit_length() - 1) << 4 | R.PCI_MSI_ENABLE)
        self.u.write(config + cap + R.PCI_MSI_CONTROL, control, 16)
        actual = self.cfg(1, 0, function, cap + R.PCI_MSI_CONTROL, 2)
        if actual != control:
            raise RuntimeError(f'MSI control did not read back: {actual:#x} != {control:#x}')
        return dict(capability=hex(cap), message_control=hex(actual),
                    address=hex(address), data=data, messages=messages)

    def control_snapshot(self):
        """Read the startup bank proven accessible before device initialization.

        BAR0 + 0 is a later-stage bank: reading it at cold startup faults.
        The large BAR is never read by this probe.
        """
        if 0 not in self.assigned_bars:
            raise RuntimeError("Assign BARs before reading the control interface")
        resource = self.assigned_bars[0]
        offsets = tuple(R.BOOTSTRAP_BANK + o for o in (
            R.BOOTSTRAP_STATUS, R.BOOTSTRAP_IDENTITY_WORD0,
            R.BOOTSTRAP_CHIP_BOARD_ID, R.BOOTSTRAP_IDENTITY_WORD2,
            R.BOOTSTRAP_IDENTITY_WORD3))
        result = {}
        for offset in offsets:
            if offset + 4 > resource["size"]:
                raise ValueError("Observed control offset exceeds this BAR")
            result[hex(offset)] = hex(self.u.read(resource["physical"] + offset, 32))
        return result

    def probe(self):
        root = self.function(0, 0, 0)
        result = dict(transport="direct bootloader proxy; no hypervisor", root=root,
                      endpoints=[], link_active=False)
        if root is None:
            result["root_config_id"] = hex(self.cfg(0, 0, 0, 0))
            result["next_step"] = "Power and initialize the internal PCI controller"
            return result
        cap = self.capability(0, 0, 0, R.PCI_CAP_ID_EXPRESS)
        if cap is None:
            result["next_step"] = "Identify the internal root-port capability layout"
            return result
        status = self.cfg(0, 0, 0, cap + R.PCI_EXP_LINK_STATUS, 2)
        result["link_status"] = hex(status)
        port = self.node.get_reg(8)[0]
        port_status = self.u.read(port + R.PORT_LINK_STATUS, 32)
        result["port_link_status"] = hex(port_status)
        # The Apple port status is also used by Gravity/Asahi's PCIe code.
        # Do not require optional PCIe DLL-active reporting support.
        result["link_active"] = bool(port_status & R.PORT_LINK_UP)
        result["secondary_bus"] = self.cfg(0, 0, 0, 0x19, 1)
        result["subordinate_bus"] = self.cfg(0, 0, 0, 0x1a, 1)
        if not result["link_active"]:
            result["next_step"] = "Establish the Centauri power/reset/link sequence"
            return result
        secondary, subordinate = result["secondary_bus"], result["subordinate_bus"]
        if not 0 < secondary <= subordinate < 256:
            result["next_step"] = "Assign downstream PCI bus numbers"
            return result
        # This internal ADT port has built-in functions, not arbitrary slots.
        for bus in range(secondary, subordinate + 1):
            for fn in range(8):
                endpoint = self.function(bus, 0, fn)
                if endpoint is not None:
                    result["endpoints"].append(endpoint)
        result["next_step"] = "Identify endpoint BARs and validate the observed control banks"
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--enable-controller-power", action="store_true",
                        help="Enable only the internal controller's ADT power gates")
    parser.add_argument("--initialize-root", action="store_true",
                        help="Run the observed controller clock/reset sequence")
    parser.add_argument("--query-reset-key", action="store_true",
                        help="Read only the bridge-reset SMC key named by this device's ADT")
    parser.add_argument("--start-link", action="store_true",
                        help="Release chip/PERST signals and start PCIe link training")
    parser.add_argument("--size-bars", action="store_true",
                        help="Probe endpoint BAR sizes with memory decoding disabled")
    parser.add_argument("--assign-bars", action="store_true",
                        help="Assign endpoint BARs and read the observed control registers")
    args = parser.parse_args()
    if not os.environ.get("M1N1DEVICE"):
        parser.error("Set M1N1DEVICE to the explicitly selected proxy device")

    from m1n1.proxy import UartInterface, M1N1Proxy
    from m1n1.proxyutils import ProxyUtils, bootstrap_port

    interface = UartInterface()
    proxy = M1N1Proxy(interface)
    bootstrap_port(interface, proxy)
    utils = ProxyUtils(proxy, heap_size=128 * 1024 * 1024)
    utils.allow_disassembly = False
    if args.enable_controller_power or args.initialize_root or args.start_link:
        proxy.pmgr_adt_power_enable("/arm-io/apcie")
    driver = Centauri(utils)
    if args.initialize_root or (args.start_link and not driver.root_ready()):
        driver.initialize_root()
    if args.start_link:
        driver.start_link()
    result = driver.probe()
    if args.size_bars:
        if not result["link_active"] or not result["endpoints"]:
            raise RuntimeError("A linked endpoint is required for BAR sizing")
        result["bar_sizes"] = driver.size_bars()
    if args.assign_bars:
        if not result["link_active"] or not result["endpoints"]:
            raise RuntimeError("A linked endpoint is required for BAR assignment")
        result["assigned_bars"] = driver.assign_bars()
        print("Assigned PCI resources:", json.dumps(result["assigned_bars"]), flush=True)
        print("PCI command registers:",
              hex(driver.cfg(0, 0, 0, R.PCI_COMMAND, 2)),
              hex(driver.cfg(1, 0, 0, R.PCI_COMMAND, 2)), flush=True)
        result["control_registers"] = driver.control_snapshot()
    if args.query_reset_key:
        result["reset_key"] = driver.reset_key_state()
        result["perst"] = driver.perst_state()
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
