# SPDX-License-Identifier: GPL-2.0-only
"""Reference capture of startup submissions and their DMA translation metadata.

Run after trace_centauri.py. By default this reads only translation metadata.
The optional bootstrap record capture reads exactly the initial 32-byte protocol
record. Runtime capture additionally recognizes and bounds host IPC metadata.
A separate explicit opaque-export option copies the referenced image
for replay without parsing or interpreting its contents. It does not start
a guest or inspect executable instructions.
"""

from m1n1.hw.dart import DART
from m1n1.hv import TraceMode
from m1n1.utils import irange
import os
import struct
import hashlib
import json
from pathlib import Path


class PageTableReader:
    def __init__(self, interface, base, size):
        self.interface, self.base, self.end = interface, base, base + size

    def readmem(self, address, size):
        if (size != 0x4000 or address & 0x3fff or
                not self.base <= address or address + size > self.end):
            raise ValueError('DART page-table read outside aligned DRAM page')
        return self.interface.readmem(address, size)


class BootstrapCapture:
    # Runtime-reference startup bank, not a guessed firmware aperture.
    BASE = 0xb82008000
    PREFIX = 'bootstrap'
    EXPECTED_DEVICE = '0x1900'
    TRACE_RANGES = ((0x44, 12), (0x1000, 4))
    RECORD_POLICY = 'fixed 32-byte submissions only'
    IMAGE_READY_STATE = 4

    def __init__(self, hv, capture, capture_record=False, opaque_export=None):
        self.hv, self.capture = hv, capture
        self.fields = {}
        self.submissions = 0
        self.dart = None
        self.capture_record = capture_record
        self.opaque_export = Path(opaque_export).resolve() if opaque_export else None
        self.asset_exported = False
        self.vendor_ready_recorded = False
        self.failure_recorded = False
        if self.opaque_export and (not capture_record or
                self.opaque_export.is_relative_to(Path(__file__).resolve().parents[2])):
            raise ValueError('Opaque export requires record capture and a path outside the Linux repository')

    def emit(self, kind, **fields):
        self.capture.emit(f'{self.PREFIX}_{kind}', **fields)

    def write(self, event):
        offset = event.addr - self.BASE
        if not event.flags.WRITE or event.flags.WIDTH != 2:
            return
        if offset in (0x44, 0x48, 0x4c):
            self.fields[offset] = event.data
        elif offset == 0x1000 and event.data == 1:
            if self.submissions >= 8 or not all(o in self.fields for o in (0x44, 0x48, 0x4c)):
                return
            self.submissions += 1
            # Bound the investigation to the observed fixed-size initial
            # submission. Larger buffers might contain executable payloads.
            size = self.fields[0x4c]
            if size != 32:
                self.emit('mapping_skipped', size=size)
                return
            address = self.fields[0x44] | self.fields[0x48] << 32
            try:
                self.mapping(address, size)
            except Exception as error:
                # A metadata decoder disagreement should not suspend XNU.
                self.emit('mapping_error', iova=hex(address),
                                  error=f'{type(error).__name__}: {error}')

    def device(self):
        # run_guest loads scripts with runpy; make this script's sibling
        # standalone driver importable without requiring a launcher-specific path.
        import sys
        from pathlib import Path
        sibling = str(Path(__file__).resolve().parent)
        if sibling not in sys.path:
            sys.path.insert(0, sibling)
        from centauri import Centauri
        return Centauri(self.hv.u)

    def status_read(self, event):
        if (not event.flags.WRITE and event.flags.WIDTH == 2 and event.data == 3
                and self.PREFIX == 'runtime_control' and not self.failure_recorded
                and os.environ.get('CENTAURI_CONTROL_RECORD') == '1'):
            self.failure_recorded = True
            try:
                if self.device().cfg(1, 0, 0, 0) == 0x1901106b:
                    self.control_registers('state_3')
            except Exception as error:
                self.emit('control_snapshot_error', error=f'{type(error).__name__}: {error}')
        if (event.flags.WRITE or event.flags.WIDTH != 2 or
                event.data != self.IMAGE_READY_STATE or self.vendor_ready_recorded):
            return
        self.vendor_ready_recorded = True
        try:
            device = self.device()
            if device.cfg(1, 0, 0, 0) != (int(self.EXPECTED_DEVICE, 0) << 16 | 0x106b):
                return
            self.emit('image_ready_vendor_configuration',
                      words={hex(o): hex(device.cfg(1, 0, 0, o)) for o in
                             (0xf80, 0xf84, 0xf88, 0xf8c, 0xf90, 0xf94, 0xf98, 0xff8, 0xffc)},
                      policy='PCI configuration registers only; no BAR or firmware memory reads')
            self.control_registers('image_ready')
        except Exception as error:
            self.emit('pci_snapshot_error', error=f'{type(error).__name__}: {error}')

    def control_registers(self, stage):
        if os.environ.get('CENTAURI_CONTROL_RECORD') != '1':
            return
        self.emit('control_register_snapshot', stage=stage, base=hex(self.BASE),
                  words={hex(o): hex(self.hv.u.read(self.BASE + o, 32))
                         for o in range(0, 0x100, 4)},
                  policy='bounded 256-byte control register bank only; no embedded addresses followed')

    def mapping(self, address, size):
        hv = self.hv
        device = self.device()
        endpoint = device.function(1, 0, 0)
        related = [device.function(1, 0, fn) for fn in (1, 2)] if endpoint and endpoint['device'] == '0x1901' else []
        self.emit('pci_configuration',
                          root_command=hex(device.cfg(0, 0, 0, 4, 2)),
                          endpoint=endpoint, related_functions=related)
        if endpoint and endpoint['device'] == self.EXPECTED_DEVICE:
            self.control_registers('before_submission')
        if (os.environ.get('CENTAURI_PCI_RECORD') == '1'
                and not getattr(self, 'pci_recorded', False)):
            self.pci_recorded = True
            functions = ((0, 0), (1, 0), (1, 1), (1, 2)) if self.PREFIX == 'runtime_control' else ((0, 0), (1, 0))
            words = {f'{bus}:{fn}': [device.cfg(bus, 0, fn, offset)
                                   for offset in range(0, 4096, 4)]
                     for bus, fn in functions}
            self.emit('pci_register_snapshot', words=words, bytes_per_function=4096,
                      policy='PCI configuration registers only; no BAR or firmware memory reads')
        if self.dart is None:
            self.dart = DART.from_adt(hv.u, '/arm-io/dart-apcie0')
            base = hv.u.ba.phys_base & ~0xffffffff
            reader = PageTableReader(hv.iface, base, hv.u.ba.mem_size_actual)
            self.dart.dart.iface = reader
        port = hv.adt['/arm-io/apcie'].get_reg(8)[0]
        # Existing Linux apple-pcie RID2SID layout; keep raw words as evidence
        # since the exact T8142 stream assignment is still being established.
        rid_words = [hv.u.read(port + 0x3000 + i * 4, 32) for i in range(16)]
        streams = []
        self.dart.invalidate_cache()
        for sid in range(hv.adt['/arm-io/dart-apcie0'].getprop('sid-count')):
            tcr = self.dart.dart.regs.TCR[sid].reg
            if not tcr.TRANSLATE_ENABLE or tcr.BYPASS_DART:
                continue
            ttbr = self.dart.dart.regs.TTBR[sid].reg
            if not ttbr.VALID:
                continue
            ranges = self.dart.iotranslate(sid, address, size)
            streams.append(dict(sid=sid, tcr=hex(tcr.value), ttbr=hex(ttbr.value),
                                ranges=[dict(physical=None if p is None else hex(p),
                                             size=n) for p, n in ranges]))
        self.emit('mapping', iova=hex(address), size=size,
                          rid2sid=[hex(w) for w in rid_words], streams=streams,
                          policy='page-table metadata only; no DMA payload reads')
        if self.capture_record and endpoint and endpoint['device'] == self.EXPECTED_DEVICE:
            self.record_header(address, size, rid_words, streams)

    def record_header(self, iova, size, rid_words, streams):
        """Read one fixed protocol record, never any embedded address."""
        if size != 32:
            return
        sids = {word >> 16 & 0xff for word in rid_words
                if word & (1 << 31) and word & 0xffff == 0x0100}
        if len(sids) != 1:
            return
        selected = [s for s in streams if s['sid'] in sids]
        if len(selected) != 1 or len(selected[0]['ranges']) != 1:
            return
        region = selected[0]['ranges'][0]
        if region['physical'] is None or region['size'] != 32:
            return
        physical = int(region['physical'], 0)
        base = self.hv.u.ba.phys_base & ~0xffffffff
        end = base + self.hv.u.ba.mem_size_actual
        if not base <= physical or physical + 32 > end:
            raise ValueError('Protocol record is outside DRAM')
        data = self.hv.iface.readmem(physical, 32)
        self.emit('submission_record', iova=hex(iova), size=32,
                          sid=selected[0]['sid'], physical=hex(physical),
                          words=[hex(w) for w in struct.unpack('<8I', data)],
                          policy='fixed runtime protocol record only; no embedded pointers followed')
        if self.opaque_export and not self.asset_exported:
            source_iova, length, reserved0, reserved1 = struct.unpack('<4Q', data)
            # Scope the opaque copy to this build's initial observed transfer,
            # not arbitrary later control messages. The bytes are never parsed.
            if length == 0x1dce000 and not reserved0 and not reserved1:
                self.export_opaque_asset(selected[0]['sid'], source_iova, length)
        return selected[0]['sid'], physical, data

    def export_opaque_asset(self, sid, address, size):
        """Copy the opaque transfer image for replay, without inspecting it."""
        if self.asset_exported or self.opaque_export is None or not 0 < size <= 0x2000000:
            return
        ranges = self.dart.iotranslate(sid, address, size)
        base = self.hv.u.ba.phys_base & ~0xffffffff
        end = base + self.hv.u.ba.mem_size_actual
        if sum(n for _, n in ranges) != size or any(
                p is None or n <= 0 or p < base or p + n > end for p, n in ranges):
            raise ValueError('Opaque image translation is incomplete or outside DRAM')
        path = self.opaque_export
        path.parent.mkdir(parents=True, exist_ok=True)
        digest = hashlib.sha256()
        with path.open('xb') as output:
            for physical, length in ranges:
                for offset in range(0, length, 1024 * 1024):
                    count = min(1024 * 1024, length - offset)
                    # Preserve CPU writes before invalidating this declared
                    # DMA input alias. Keep firmware bytes opaque throughout.
                    self.hv.p.dc_civac(physical + offset, count)
                    data = self.hv.iface.readmem(physical + offset, count)
                    if len(data) != count:
                        raise IOError('Short opaque image copy')
                    output.write(data)
                    digest.update(data)
        metadata = dict(source='operator-device initial runtime transfer buffer',
                        observer=self.PREFIX,
                        sid=sid, iova=hex(address), size=size, sha256=digest.hexdigest(),
                        cache_sync='dc_civac before each copied chunk',
                        policy='opaque firmware copy for replay; no contents parsed or instructions inspected')
        path.with_suffix(path.suffix + '.json').write_text(json.dumps(metadata, indent=2) + '\n')
        self.asset_exported = True
        self.emit('opaque_asset', path=str(path), **metadata)

    def start(self):
        for offset, size in self.TRACE_RANGES:
            self.hv.add_tracer(irange(self.BASE + offset, size),
                               f'centauri/{self.PREFIX}/{offset:x}', TraceMode.WSYNC,
                               None, self.write)
        if (os.environ.get('CENTAURI_PCI_RECORD') == '1' or
                os.environ.get('CENTAURI_CONTROL_RECORD') == '1'):
            self.hv.add_tracer(irange(self.BASE, 4), f'centauri/{self.PREFIX}/ready',
                               TraceMode.SYNC, self.status_read, None)
        self.emit('mapping_capture', base=hex(self.BASE), expected_device=self.EXPECTED_DEVICE,
                          record_capture=self.capture_record,
                          opaque_export=bool(self.opaque_export),
                          policy=(self.RECORD_POLICY +
                                  ('; opaque image copy only, no executable inspection' if self.opaque_export
                                   else '; no firmware payload reads')))


class RuntimeControlCapture(BootstrapCapture):
    """Observe the relocated control function's image and IPC startup records."""
    BASE = 0xb82028000
    PREFIX = 'runtime_control'
    EXPECTED_DEVICE = '0x1901'
    TRACE_RANGES = ((0x44, 12), (0x1000, 4), (0x1070, 4), (0x107c, 4))
    RECORD_POLICY = 'fixed image record or signature-checked same-page IPC metadata, maximum 0x3b0 bytes'
    IMAGE_READY_STATE = 2

    def __init__(self, hv, capture, opaque_export=None):
        # Only the fixed image record can enable an opaque copy; the context
        # has a different signature and is never interpreted as an image.
        super().__init__(hv, capture, capture_record=True, opaque_export=opaque_export)
        self.stages_seen = set()

    def write(self, event):
        if not event.flags.WRITE or event.flags.WIDTH != 2:
            return
        offset = event.addr - self.BASE
        if offset in (0x44, 0x48, 0x4c):
            self.fields[offset] = event.data
            return
        stage = {(0x1070, 1): 'before_1070', (0x107c, 2): 'before_107c_2'}.get((offset, event.data))
        if offset == 0x1000:
            stage = 'before_first_1000'
        if stage is None or stage in self.stages_seen or not all(o in self.fields for o in (0x44, 0x48, 0x4c)):
            return
        self.stages_seen.add(stage)
        if self.fields[0x4c] != 32:
            self.emit('mapping_skipped', size=self.fields[0x4c])
            return
        address = self.fields[0x44] | self.fields[0x48] << 32
        self.emit('context_stage', stage=stage, doorbell_value=hex(event.data))
        try:
            self.mapping(address, 32)
        except Exception as error:
            self.emit('mapping_error', iova=hex(address), error=f'{type(error).__name__}: {error}')

    def record_header(self, iova, size, rid_words, streams):
        result = super().record_header(iova, size, rid_words, streams)
        if result is None:
            return
        sid, physical, data = result
        # Boot-15 identifies this specific host-owned context allocation.
        # Never follow arbitrary embedded pointers or read device firmware.
        if struct.unpack('<4Q', data) != (0x680300, iova + 0x68,
                                          iova + 0x78, iova + 0x7a):
            self.emit('context_skipped', reason='unrecognized fixed context header')
            return
        extent = 0x3b0
        if (iova & 0x3fff) + extent > 0x4000:
            self.emit('context_skipped', reason='context crosses allocation page')
            return
        ranges = self.dart.iotranslate(sid, iova, extent)
        base = self.hv.u.ba.phys_base & ~0xffffffff
        end = base + self.hv.u.ba.mem_size_actual
        if ranges != [(physical, extent)] or not base <= physical or physical + extent > end:
            self.emit('context_skipped', reason='context translation outside original DRAM page')
            return
        data = self.hv.iface.readmem(physical, extent)
        self.emit('initial_context', iova=hex(iova), sid=sid, size=extent,
                  words=[hex(w) for w in struct.unpack('<236I', data)],
                  policy='bounded host IPC context and ring metadata; no firmware payload reads')


class PCIBootWatch:
    """Observe vendor configuration changes during the runtime restart window.

    PIO register 0xdc is sampled by the guest after submissions. We use that
    timing to read known PCI configuration registers, never PIO descriptors
    or their unknown data buffers. An explicit option copies one bounded
    transfer candidate opaquely, without decoding its bytes.
    """
    OFFSETS = (0xf80, 0xf84, 0xf88, 0xf8c, 0xf90, 0xf94, 0xf98, 0xff8, 0xffc)

    def __init__(self, hv, capture, bootstrap, runtime, opaque_export=None):
        self.hv, self.capture = hv, capture
        self.bootstrap, self.runtime = bootstrap, runtime
        self.last_pio, self.last_words, self.samples = None, None, 0
        self.reader = BootstrapCapture(hv, capture, capture_record=True, opaque_export=opaque_export)
        self.reader.PREFIX = 'pci_boot'

    def active(self):
        return (self.bootstrap.submissions > 0 and
                'before_1070' not in self.runtime.stages_seen and self.samples < 4096)

    def read(self, event):
        if (event.flags.WRITE or event.flags.WIDTH != 2 or not self.active()
                or event.data == self.last_pio):
            return
        self.last_pio = event.data
        self.samples += 1
        try:
            device = self.reader.device()
            # Root topology and link guard accesses while PERST is active.
            if device.cfg(0, 0, 0, 0x19, 1) != 1:
                return
            port = device.node.get_reg(8)[0]
            if not self.hv.u.read(port + 0x208, 32) & 1:
                return
            if device.cfg(1, 0, 0, 0) != 0x1901106b:
                return
            words = tuple(device.cfg(1, 0, 0, o) for o in self.OFFSETS)
            if words == self.last_words:
                return
            self.last_words = words
            self.capture.emit('pci_boot_vendor_change', pio_value=hex(event.data),
                              words={hex(o): hex(v) for o, v in zip(self.OFFSETS, words)},
                              policy='PCI configuration metadata only; no PIO descriptor or firmware reads')
            if words[2] or words[3]:
                self.source_metadata(device, words[2] | words[3] << 32, words[4])
        except Exception as error:
            self.capture.emit('pci_boot_watch_error', error=f'{type(error).__name__}: {error}')

    def source_metadata(self, device, address, length):
        # The length is a candidate register interpretation. Query translation
        # metadata only; no arbitrary pointer is followed to read its contents.
        if not 0 < length <= 0x2000000 or address >= 1 << 36:
            return
        word = self.hv.u.read(device.node.get_reg(8)[0] + 0x3004, 32)
        if not word & (1 << 31) or word & 0xffff != 0x0100:
            return
        sid = word >> 16 & 0xff
        if self.reader.dart is None:
            dart = DART.from_adt(self.hv.u, '/arm-io/dart-apcie0')
            base = self.hv.u.ba.phys_base & ~0xffffffff
            dart.dart.iface = PageTableReader(self.hv.iface, base, self.hv.u.ba.mem_size_actual)
            self.reader.dart = dart
        dart = self.reader.dart
        if not dart.dart.regs.TCR[sid].reg.TRANSLATE_ENABLE or not dart.dart.regs.TTBR[sid].reg.VALID:
            return
        dart.invalidate_cache()
        ranges = dart.iotranslate(sid, address, 32)
        self.capture.emit('pci_boot_address_translation', iova=hex(address), candidate_length=length,
                          sid=sid, ranges=[dict(physical=None if p is None else hex(p), size=n)
                                           for p, n in ranges], policy='page-table metadata only')
        if length == 0xae3c00 and self.reader.opaque_export:
            self.reader.export_opaque_asset(sid, address, length)

    def start(self):
        base = self.hv.adt['/arm-io/apcie0-piodma'].get_reg(0)[0]
        self.hv.add_tracer(irange(base + 0xdc, 4), 'centauri/pci-boot-watch',
                           TraceMode.SYNC, self.read, None)
        self.capture.emit('pci_boot_watch', base=hex(base), observed_register='0xdc',
                          maximum_observations=4096, opaque_export=bool(self.reader.opaque_export),
                          policy='bounded PCI configuration observations; no PIO descriptor inspection')


if 'hv' in globals():
    centauri_bootstrap_trace = BootstrapCapture(
        hv, centauri_trace, capture_record=os.environ.get('CENTAURI_BOOTSTRAP_RECORD') == '1',
        opaque_export=os.environ.get('CENTAURI_BOOTSTRAP_OPAQUE_EXPORT'))
    centauri_bootstrap_trace.start()
    if os.environ.get('CENTAURI_RUNTIME_RECORD') == '1':
        centauri_runtime_trace = RuntimeControlCapture(hv, centauri_trace,
            opaque_export=os.environ.get('CENTAURI_RUNTIME_OPAQUE_EXPORT'))
        centauri_runtime_trace.start()
        if os.environ.get('CENTAURI_PCI_BOOT_WATCH') == '1':
            centauri_pci_boot_watch = PCIBootWatch(hv, centauri_trace,
                centauri_bootstrap_trace, centauri_runtime_trace,
                opaque_export=os.environ.get('CENTAURI_PCI_BOOT_OPAQUE_EXPORT'))
            centauri_pci_boot_watch.start()
