# SPDX-License-Identifier: GPL-2.0-only
"""Runtime-only Centauri discovery; run with run_guest.py --script.

No firmware, executable, or DMA-buffer reads. Configuration observations
discover BARs; only their first 16 KiB is traced on this initial pass.
Receive timestamps describe host receipt, not device transaction time.
"""

import json
import os
import time
from pathlib import Path

from m1n1.hv import TraceMode
from m1n1.utils import irange


class CentauriTrace:
    BAR_CAPTURE_SIZE = 0x4000

    def __init__(self, hv, output):
        self.hv = hv
        self.apcie = hv.adt["/arm-io/apcie"]
        self.ecam, self.ecam_size = self.apcie.get_reg(0)
        self.output = Path(output)
        self.output.parent.mkdir(parents=True, exist_ok=True)
        self.logfile = self.output.open("x", buffering=1)
        self.sequence = 0
        self.config = {}
        self.valid = {}
        self.windows = {}
        self.discovery_pages = {}
        self.gpio_snapshots = []
        self.emit("capture", policy="base observer: runtime MMIO only; no executable or DMA reads",
                  scope="additional observers declare any extra reads separately",
                  chip_id=hv.adt["/chosen"].chip_id, ecam=hex(self.ecam),
                  bar_capture_size=self.BAR_CAPTURE_SIZE,
                  timestamp_semantics="host receive monotonic ns")

    def emit(self, kind, **fields):
        self.sequence += 1
        self.logfile.write(json.dumps(dict(seq=self.sequence,
                                         host_ns=time.monotonic_ns(),
                                         kind=kind, **fields)) + "\n")

    def event(self, evt, region, base=0):
        self.emit("mmio", region=region, address=hex(evt.addr),
                  offset=hex(evt.addr - base), write=bool(evt.flags.WRITE),
                  width=1 << evt.flags.WIDTH, value=hex(evt.data),
                  cpu=evt.flags.CPU, pc=hex(evt.pc), multi=bool(evt.flags.MULTI))

    def discover_read(self, evt, region, base):
        # Do not preserve returned bytes from unidentified device memory.
        page = evt.addr & ~0x3fff
        write = bool(evt.flags.WRITE)
        state = self.discovery_pages.setdefault((page, write), [0, set()])
        state[0] += 1
        offset = evt.addr - page
        if len(state[1]) < 32 and offset not in state[1]:
            state[1].add(offset)
            self.emit("aperture_write" if write else "aperture_read",
                      region=region, page=hex(page), write=write,
                      address=hex(evt.addr), offset=hex(offset),
                      aperture_offset=hex(evt.addr - base),
                      width=1 << evt.flags.WIDTH, cpu=evt.flags.CPU,
                      pc=hex(evt.pc), value_omitted=True)
        elif state[0] % 4096 == 0:
            self.emit("aperture_count", page=hex(page), write=write, accesses=state[0])

    def config_event(self, evt):
        offset = evt.addr - self.ecam
        bus, dev, fn = offset >> 20, (offset >> 15) & 31, (offset >> 12) & 7
        bdf = f"{bus:02x}:{dev:02x}.{fn}"
        reg = offset & 0xfff
        self.event(evt, f"config/{bdf}", self.ecam + (offset & ~0xfff))
        if (evt.flags.WRITE and bus == 0 and reg == 0x18 and self.hv.ctx is not None):
            for name, address in self.gpio_snapshots:
                value = self.hv.u.read(address, 32)
                self.emit("gpio_snapshot", region=name, address=hex(address),
                          value=hex(value), trigger="root bus-number write")
        # Track observed bytes only; never read configuration proactively.
        width = 1 << evt.flags.WIDTH
        if width > 8 or reg + width > 0x1000:
            return
        image = self.config.setdefault(bdf, bytearray(0x1000))
        valid = self.valid.setdefault(bdf, set())
        image[reg:reg + width] = int(evt.data).to_bytes(width, "little")
        valid.update(range(reg, reg + width))
        if bus and reg < 0x28 and reg + width > 0x10:
            self.update_bars(bdf, image, valid)

    def physical_bar(self, address, size):
        for r in self.apcie.ranges:
            ba = r.bus_addr
            if not isinstance(ba, list) or len(ba) != 3:
                continue
            # ADT uses the unusual low-word/high-word order documented by
            # the existing Gravity/Asahi ADT parser.
            start = (ba[2] << 32) | ba[1]
            if start <= address and address + size <= start + r.size:
                return r.parent_addr + address - start
        return None

    def update_bars(self, bdf, image, valid):
        index = 0
        while index < 6:
            bar = index
            off = 0x10 + 4 * bar
            index += 1
            if not all(x in valid for x in range(off, off + 4)):
                continue
            low = int.from_bytes(image[off:off + 4], "little")
            if low & 1:  # I/O BARs do not select memory apertures.
                continue
            address = low & ~15
            if low & 6 == 4:
                index += 1
                if bar == 5 or not all(x in valid for x in range(off + 4, off + 8)):
                    continue
                address |= int.from_bytes(image[off + 4:off + 8], "little") << 32
            name = f"centauri/{bdf}/bar{bar}"
            size = self.BAR_CAPTURE_SIZE
            physical = self.physical_bar(address, size) if address else None
            # Probe masks and disabled BARs never match a translated aperture.
            old = self.windows.get(name)
            if physical == old:
                continue
            self.hv.clear_tracers(name)
            self.windows[name] = physical
            self.emit("bar", bdf=bdf, bar=bar, bus_address=hex(address),
                      physical=None if physical is None else hex(physical),
                      captured_size=size, coverage="first 16 KiB, not inferred BAR size")
            if physical is not None:
                self.hv.add_tracer(irange(physical, size), name, TraceMode.ASYNC,
                                   self.event, self.event, region=name, base=physical)

    def start(self):
        # Capture the full advertised configuration aperture: do not assume
        # the physical bus stride of this new controller during discovery.
        self.hv.add_tracer(irange(self.ecam, self.ecam_size),
                           "centauri/config", TraceMode.WSYNC,
                           self.config_event, self.config_event)
        # Shared controller and port-0 MMIO, including the PHY IP bank.
        # These indices come from this device's ADT; other ports are omitted.
        for index in (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13):
            address, size = self.apcie.get_reg(index)
            name = f"apcie/reg{index}"
            self.hv.add_tracer(irange(address, size), "centauri/" + name,
                               TraceMode.ASYNC, self.event, self.event,
                               region=name, base=address)
            self.emit("region", name=name, physical=hex(address), size=size)
        piodma = self.hv.adt["/arm-io/apcie0-piodma"]
        for index in range(len(piodma.reg)):
            address, size = piodma.get_reg(index)
            name = f"apcie-piodma/reg{index}"
            self.hv.add_tracer(irange(address, size), "centauri/" + name,
                               TraceMode.ASYNC, self.event, self.event,
                               region=name, base=address)
            self.emit("region", name=name, physical=hex(address), size=size)
        for index, region in enumerate(self.apcie.ranges):
            address = self.apcie._parent.translate(region.parent_addr)
            name = f"aperture{index}"
            self.hv.add_tracer(irange(address, region.size), "centauri/" + name,
                               TraceMode.ASYNC, self.discover_read, None,
                               region=name, base=address)
            self.emit("region", name=name, physical=hex(address), size=region.size,
                      policy="read-address discovery only; no values or writes")
        for raw in json.loads(os.environ.get("CENTAURI_FOCUS_PAGES", "[]")):
            address = int(raw, 0)
            assert address & 0x3fff == 0, "Focus address must be 16 KiB aligned"
            assert any(self.apcie._parent.translate(r.parent_addr) <= address and
                       address + 0x100 <= self.apcie._parent.translate(r.parent_addr) + r.size
                       for r in self.apcie.ranges), "Focus address is outside PCI apertures"
            for raw_offset in json.loads(os.environ.get("CENTAURI_FOCUS_OFFSETS", '["0x0"]')):
                offset = int(raw_offset, 0)
                assert 0 <= offset <= 0x3f00, "Focus window exceeds its 16 KiB page"
                name = f"focus/{address:x}/{offset:x}"
                self.hv.add_tracer(irange(address + offset, 0x100), "centauri/" + name,
                                   TraceMode.ASYNC, self.event, self.event,
                                   region=name, base=address)
                self.emit("region", name=name, physical=hex(address + offset), size=0x100,
                          policy="control-window selection from preceding address-only capture")
        for raw_address, raw_size in json.loads(os.environ.get("CENTAURI_WRITE_DISCOVERY", "[]")):
            address, size = int(raw_address, 0), int(raw_size, 0)
            assert address & 0x3fff == 0 and size > 0
            assert any(self.apcie._parent.translate(r.parent_addr) <= address and
                       address + size <= self.apcie._parent.translate(r.parent_addr) + r.size
                       for r in self.apcie.ranges), "Write discovery is outside PCI apertures"
            name = f"write-discovery/{address:x}"
            self.hv.add_tracer(irange(address, size), "centauri/" + name,
                               TraceMode.ASYNC, None, self.discover_read,
                               region=name, base=address)
            self.emit("region", name=name, physical=hex(address), size=size,
                      policy="write-address discovery only; no values preserved")
        address, size = self.hv.adt["/arm-io/dart-apcie0"].get_reg(0)
        self.hv.add_tracer(irange(address, size), "centauri/dart", TraceMode.ASYNC,
                           self.event, self.event, region="dart", base=address)
        self.emit("region", name="dart", physical=hex(address), size=size,
                  limitation="guest MMIO only; EL2 monitor operations bypass stage 2")
        self.hv.trace_irq("centauri-msi", self.apcie.msi_vector_offset, 32,
                          self.hv.IRQTRACE_IRQ)
        self.hv.log(f"Centauri runtime trace: {self.output}")


if "hv" in globals():
    centauri_trace = CentauriTrace(hv, os.environ["CENTAURI_TRACE"])
    centauri_trace.start()
