# SPDX-License-Identifier: GPL-2.0-only
"""Exercise startup DMA with a host-owned 32-byte submission.

This standalone prototype loads an opaque image and can re-enumerate runtime
functions. It runs directly through the bootloader proxy. DMA memory and page tables
are newly allocated and zeroed. An optional opaque image is copied unchanged
for replay without parsing or inspecting its contents. A device rejection
is a useful result, not a successful Wi-Fi boot.
"""

import argparse
import json
import os
import time
import struct
from pathlib import Path

from centauri import Centauri
import centauri_regs as R


class ZeroedPages:
    def __init__(self, utils):
        self.u, self.pages = utils, []

    def memalign(self, alignment, size):
        address = self.u.memalign(alignment, size)
        self.u.proxy.memset8(address, 0, size)
        self.u.proxy.dc_civac(address, size)
        self.pages.append((address, size))
        return address


def probe(utils, driver, mapped=True, payload_size=0, payload_mapped=True,
          payload_data=None, record_iova=0x01000000, payload_iova=0x02000000, timeout=10,
          runtime=False, reuse_dma=False, boot_buffer_data=None, boot_buffer_iova=0x06000000,
          keep_ready_dma=False, retain_owned_domain=False, before_submit_delay=0):
    if not 0 <= before_submit_delay <= 30 or before_submit_delay and not runtime:
        raise ValueError('Submission delay is bounded and runtime-only')
    from m1n1.hw.dart import DART
    from m1n1.hw.dart8110 import R_TCR, R_TTBR, R_TLB_OP

    if boot_buffer_data is not None and (not runtime or not 0 < len(boot_buffer_data) <= 0x2000000):
        raise ValueError('The runtime allocation must fit the bounded DMA resource size')
    resources = driver.runtime_bars[0] if runtime else driver.assigned_bars
    if not resources or not driver.probe()['link_active']:
        raise RuntimeError('Assign BARs on the linked endpoint before the DMA experiment')
    expected_device = R.CENTAURI_CONTROL_DEVICE if runtime else R.CENTAURI_BOOTSTRAP_DEVICE
    if driver.cfg(1, 0, 0, 0) != (expected_device << 16 | R.PCI_VENDOR_APPLE):
        raise RuntimeError('Image transfer does not match the current PCI function identity')
    endpoint = driver.ecam + (1 << 20)
    command = driver.cfg(1, 0, 0, R.PCI_COMMAND, 2)
    if command & R.PCI_COMMAND_BUS_MASTER:
        raise RuntimeError('The experiment requires endpoint DMA initially disabled')
    retained = getattr(driver, 'image_dma', None) if reuse_dma else None
    if reuse_dma and (retained is None or payload_data is not None):
        raise ValueError('DMA reuse requires a retained owned image and no replacement payload')
    dart = DART.from_adt(utils, '/arm-io/dart-apcie0')
    pages = ZeroedPages(utils)
    dart.dart.u = pages
    regs = dart.dart.regs
    sid = 1  # Boot-10 RID2SID mapping for 01:00.0, not the PIO-DMA SID 17.
    base = utils.adt['/arm-io/dart-apcie0'].get_reg(0)[0]

    def invalidate():
        regs.TLB_OP.val = R_TLB_OP(STREAM=sid, OP=1).value
        driver.wait32(base + 0x80, 1 << 31, 0, 'DART stream invalidation', timeout=1)

    if retain_owned_domain:
        owners = getattr(driver, 'owned_image_domains', [])
        if (not runtime or reuse_dma or not mapped or not payload_mapped or
                not any(o['utils'] is utils and o['sid'] == sid and
                        o['ttbr'] == regs.TTBR[sid].reg.value for o in owners)):
            raise ValueError('Retain only the live domain established by this prototype')
        tcr = regs.TCR[sid].reg
        if not tcr.TRANSLATE_ENABLE or tcr.BYPASS_DART or not regs.TTBR[sid].reg.VALID:
            raise RuntimeError('Owned image domain is no longer active')
    else:
        regs.DISABLE_STREAMS[0].val = 1 << sid
        dart.dart.enabled_streams &= ~(1 << sid)
        regs.TCR[sid].reg = R_TCR(TRANSLATE_ENABLE=1)
        regs.TTBR[sid].reg = R_TTBR(VALID=0)
        invalidate()

    def map_owned(address, physical, length):
        if retain_owned_domain and any(p is not None for p, _ in dart.iotranslate(sid, address, length)):
            raise ValueError('New image allocation would replace an existing DMA mapping')
        dart.iomap_at(sid, address, physical, length)
    physical = retained['record_physical'] if retained else pages.memalign(0x4000, 0x4000)
    iova = retained['record_iova'] if retained else record_iova
    if mapped:
        map_owned(iova, physical, 0x4000)
    if payload_data is not None:
        payload_size = len(payload_data)
    if retained:
        payload_size = retained['payload_size']
        payload_iova = retained['payload_iova']
    if payload_size:
        if not 0 < payload_size <= 0x2000000:
            raise ValueError('Controlled payload must be between 1 byte and 32 MiB')
        payload_allocation = (payload_size + 0x3fff) & ~0x3fff
        payload_physical = retained['payload_physical'] if retained else pages.memalign(0x4000, payload_allocation)
        if payload_data is not None:
            # Mechanical opaque firmware copy only; no parsing or inspection.
            utils.iface.writemem(payload_physical, payload_data)
        if iova < payload_iova + payload_allocation and payload_iova < iova + 0x4000:
            raise ValueError('Record and payload IOVA ranges overlap')
        if payload_mapped:
            map_owned(payload_iova, payload_physical, payload_allocation)
        # Candidate address/length layout from the fixed runtime record.
        # The last sixteen bytes remain zero; no opcode meaning is asserted.
        if not retained:
            record = bytearray(R.BOOTSTRAP_IMAGE_RECORD_SIZE)
            struct.pack_into('<Q', record, R.BOOTSTRAP_IMAGE_SOURCE_OFFSET, payload_iova)
            struct.pack_into('<Q', record, R.BOOTSTRAP_IMAGE_LENGTH_OFFSET, payload_size)
            utils.iface.writemem(physical, bytes(record))
        driver.image_dma = dict(record_physical=physical, record_iova=iova,
                                payload_physical=payload_physical, payload_iova=payload_iova,
                                payload_size=payload_size)
    boot_buffer = None
    if boot_buffer_data is not None:
        length = len(boot_buffer_data)
        allocation = (length + 0x3fff) & ~0x3fff
        if boot_buffer_iova < 0 or boot_buffer_iova & 0x3fff or boot_buffer_iova + allocation > 1 << 36:
            raise ValueError('Companion IOVA must be a 16-KiB-aligned DART range')
        occupied = [(iova, 0x4000)]
        if payload_size:
            occupied.append((payload_iova, (payload_size + 0x3fff) & ~0x3fff))
        if any(boot_buffer_iova < start + size and start < boot_buffer_iova + allocation
               for start, size in occupied):
            raise ValueError('Companion DMA range overlaps the owned image or record')
        boot_physical = pages.memalign(0x4000, allocation)
        utils.iface.writemem(boot_physical, boot_buffer_data)
        map_owned(boot_buffer_iova, boot_physical, allocation)
        boot_buffer = dict(iova=boot_buffer_iova, physical=boot_physical, length=length)
        driver.boot_buffer_dma = boot_buffer
    for address, size in pages.pages:
        utils.proxy.dc_cvac(address, size)
    invalidate()
    if mapped and dart.iotranslate(sid, iova, 32) != [(physical, 32)]:
        raise RuntimeError('DART mapping did not read back as the owned submission page')
    if not mapped:
        if regs.TTBR[sid].reg.VALID:
            raise RuntimeError('Unmapped experiment unexpectedly has a valid root table')
        regs.ENABLE_STREAMS[0].val = 1 << sid
    port = driver.node.get_reg(8)[0]
    mapping = R.PORT_RID_TO_SID_VALID | sid << R.PORT_RID_TO_SID_STREAM_SHIFT | 0x0100
    utils.write(port + R.PORT_RID_TO_SID_TABLE + 4, mapping, 32)
    if utils.read(port + R.PORT_RID_TO_SID_TABLE + 4, 32) != mapping:
        raise RuntimeError('Endpoint RID-to-SID mapping did not read back')
    startup = resources[0]['physical'] + R.BOOTSTRAP_BANK
    print('DMA setup:', json.dumps(dict(sid=sid, iova=hex(iova),
                                        physical=hex(physical), rid2sid=hex(mapping),
                                        mapped=mapped, payload_size=payload_size,
                                        payload_mapped=payload_mapped, reuse_dma=reuse_dma)), flush=True)
    before = dict(status=hex(utils.read(startup + R.BOOTSTRAP_STATUS, 32)),
                  submission_status=hex(utils.read(startup + R.BOOTSTRAP_SUBMISSION_STATUS, 32)),
                  dart_error=hex(regs.ERROR.val))
    kept_dma = False
    try:
        utils.write(endpoint + R.PCI_COMMAND, command | R.PCI_COMMAND_BUS_MASTER, 16)
        enabled_command = driver.cfg(1, 0, 0, R.PCI_COMMAND, 2)
        print('Endpoint DMA command:', hex(enabled_command), flush=True)
        if not enabled_command & R.PCI_COMMAND_BUS_MASTER:
            raise RuntimeError('Endpoint bus mastering did not enable')
        if boot_buffer:
            timestamp = time.time_ns() // 1000
            # Both control and beta receive this Unix-microsecond value in
            # reference PCI initialization. This is register metadata, not
            # a timestamp extracted from a firmware binary.
            for function, expected in ((0, R.CENTAURI_CONTROL_DEVICE), (2, R.CENTAURI_BETA_DEVICE)):
                if driver.cfg(1, 0, function, 0) != (expected << 16 | R.PCI_VENDOR_APPLE):
                    raise RuntimeError('Unexpected function for the observed PCI time setup')
                config = endpoint + (function << 12)
                utils.write(config + R.CENTAURI_PCI_UNIX_TIME_US_LOW, timestamp & 0xffffffff, 32)
                utils.write(config + R.CENTAURI_PCI_UNIX_TIME_US_HIGH, timestamp >> 32, 32)
            utils.write(endpoint + R.CENTAURI_PCI_BOOT_BUFFER_LOW, boot_buffer_iova & 0xffffffff, 32)
            utils.write(endpoint + R.CENTAURI_PCI_BOOT_BUFFER_HIGH, boot_buffer_iova >> 32, 32)
            utils.write(endpoint + R.CENTAURI_PCI_BOOT_BUFFER_LENGTH, boot_buffer['length'], 32)
            print('Runtime companion setup:', json.dumps({k: hex(v) for k, v in boot_buffer.items()}), flush=True)
            print('Runtime companion registers:', {hex(o): hex(driver.cfg(1, 0, 0, o)) for o in
                  (R.CENTAURI_PCI_BOOT_BUFFER_LOW, R.CENTAURI_PCI_BOOT_BUFFER_HIGH,
                   R.CENTAURI_PCI_BOOT_BUFFER_LENGTH, R.CENTAURI_PCI_UNIX_TIME_US_LOW,
                   R.CENTAURI_PCI_UNIX_TIME_US_HIGH)}, flush=True)
        buffer_observation = None
        if before_submit_delay:
            time.sleep(before_submit_delay)
            if boot_buffer:
                import hashlib
                count = min(0x4000, boot_buffer['length'])
                utils.proxy.dc_ivac(boot_buffer['physical'], count)
                prefix = utils.iface.readmem(boot_buffer['physical'], count)
                buffer_observation = dict(delay_seconds=before_submit_delay, bytes=count,
                    sha256=hashlib.sha256(prefix).hexdigest(),
                    equals_zero=hashlib.sha256(prefix).digest() == hashlib.sha256(bytes(count)).digest())
                print('Runtime allocation observation:', json.dumps(buffer_observation), flush=True)
        # The host identity matches the device ADT and IPSW manifest. The
        # neighboring zero-valued setup fields remain unresolved, not opcodes.
        chosen = utils.adt['/chosen']
        host_platform = ((chosen.board_id << R.BOOTSTRAP_HOST_BOARD_ID_SHIFT) |
                         chosen.chip_id)
        setup = [] if runtime else [(0x3c, 0), (R.BOOTSTRAP_HOST_PLATFORM_ID, host_platform), (0x78, 0)]
        setup += [(R.BOOTSTRAP_SUBMISSION_ADDRESS_LOW, iova & 0xffffffff),
                              (R.BOOTSTRAP_SUBMISSION_ADDRESS_HIGH, iova >> 32),
                              (R.BOOTSTRAP_SUBMISSION_LENGTH, 32)]
        for offset, value in setup:
            utils.write(startup + offset, value, 32)
        utils.write(startup + (R.CONTROL_IMAGE_SUBMIT if runtime else R.BOOTSTRAP_SUBMIT), 1, 32)
        started = time.monotonic()
        deadline = started + timeout
        status = 0
        transitions = []
        previous = None
        expected_status = R.CONTROL_IMAGE_READY if runtime else R.BOOTSTRAP_IMAGE_READY
        while time.monotonic() < deadline:
            status = utils.read(startup + R.BOOTSTRAP_STATUS, 32)
            submission_status = utils.read(startup + R.BOOTSTRAP_SUBMISSION_STATUS, 32)
            if (status, submission_status) != previous:
                transitions.append(dict(seconds=round(time.monotonic() - started, 4),
                                        status=hex(status), submission_status=hex(submission_status)))
                previous = status, submission_status
            if status == expected_status:
                break
            time.sleep(0.005)
        submission_status = utils.read(startup + R.BOOTSTRAP_SUBMISSION_STATUS, 32)
        utils.proxy.dc_ivac(physical, 0x4000)
        owned_data = utils.iface.readmem(physical, 32)
        outcome = dict(before=before, status=hex(status), submission_status=hex(submission_status),
                    transitions=transitions,
                    dart_error=hex(regs.ERROR.val),
                    dart_error_address=hex(regs.ERROR_ADDR_LO.val |
                                           regs.ERROR_ADDR_HI.val << 32),
                    dart_error_disable=hex(regs.ERROR_DISABLE.val),
                    dart_tcr=hex(regs.TCR[sid].reg.value),
                    dart_enabled_streams=hex(regs.ENABLE_STREAMS[0].val),
                    owned_submission_after=owned_data.hex(),
                    interpretation=('runtime image transfer ready' if runtime and status == expected_status else
                                    'firmware image ready for warm restart' if status == expected_status
                                    else 'image transfer did not reach ready state; error codes unresolved'))
        if keep_ready_dma and status == expected_status:
            owners = getattr(driver, 'owned_image_domains', [])
            owners.append(dict(utils=utils, sid=sid, ttbr=regs.TTBR[sid].reg.value,
                               dart=dart, pages=pages, image=dict(driver.image_dma)))
            driver.owned_image_domains = owners
            kept_dma = True
        outcome.update(dma_kept=kept_dma, domain_kept=retain_owned_domain)
        if buffer_observation is not None:
            outcome['boot_buffer_observation'] = buffer_observation
        return outcome
    finally:
        # The script must not leave a device able to DMA into a later proxy heap.
        if not kept_dma:
            try:
                utils.write(endpoint + R.PCI_COMMAND, command, 16)
            finally:
                regs.DISABLE_STREAMS[0].val = 1 << sid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--unmapped', action='store_true',
                        help='Leave the submission IOVA unmapped to test the DMA fault path')
    parser.add_argument('--enable-msi', action='store_true',
                        help='Configure the reference four-message MSI transport before submission')
    parser.add_argument('--reuse-link', action='store_true',
                        help='Use an already linked, unsubmitted bootstrap function without resetting it')
    parser.add_argument('--reference-pcie', action='store_true',
                        help='Match the observed standard PCIe payload/read-request configuration')
    parser.add_argument('--reference-aux', action='store_true',
                        help='Apply the observed auxiliary controller-bank setup (field meanings unresolved)')
    parser.add_argument('--bootstrap-host-time', action='store_true',
                        help='Publish current Unix microseconds before the first firmware submission')
    parser.add_argument('--bootstrap-d3hot', action='store_true',
                        help='Request observed D3hot/PME before the runtime port restart')
    parser.add_argument('--bootstrap-ptm', action='store_true',
                        help='Enable the observed PTM profile before both image submissions')
    parser.add_argument('--keep-ready-dma', action='store_true',
                        help='Keep this process\'s successful image DMA/domain alive through the shell')
    parser.add_argument('--dart-enable-declared-bypass', action='store_true',
                        help='Rebuild the observed TCR/TTBR/enable state of ADT reserved streams')
    parser.add_argument('--dart-adt-parameters', action='store_true',
                        help='Apply the declared masked DART parameters before the initial image')
    parser.add_argument('--payload-size', type=lambda s: int(s, 0), default=0,
                        help='Submit a candidate record pointing to this many owned zero bytes (max 32 MiB)')
    parser.add_argument('--payload-unmapped', action='store_true',
                        help='Leave the candidate payload IOVA unmapped while mapping its record')
    parser.add_argument('--opaque-payload', type=str,
                        help='Copy this opaque firmware asset to the device without inspecting its contents')
    parser.add_argument('--record-iova', type=lambda s: int(s, 0), default=0x01000000)
    parser.add_argument('--payload-iova', type=lambda s: int(s, 0), default=0x02000000)
    parser.add_argument('--timeout', type=float, default=10,
                        help='Seconds to poll startup state after submission')
    parser.add_argument('--shell', action='store_true',
                        help='Keep this proxy and Python heap alive in an interactive host shell afterward')
    parser.add_argument('--boot-runtime', action='store_true',
                        help='After image-ready, warm restart and allocate the three runtime functions')
    parser.add_argument('--runtime-image', action='store_true',
                        help='Resubmit the retained owned image DMA buffers on runtime control PF0')
    parser.add_argument('--runtime-opaque-payload', type=str,
                        help='Use this separate opaque asset for the second-stage image transfer')
    parser.add_argument('--runtime-boot-buffer', type=str,
                        help='Copy the observed opaque companion buffer and publish its PCI metadata')
    parser.add_argument('--runtime-boot-buffer-size', type=lambda s: int(s,0),
                        help='Generate a zero-filled runtime allocation instead of copying a resource')
    parser.add_argument('--runtime-boot-buffer-from-image', action='store_true',
                        help='Generate the runtime allocation from the firmware FTAB 2ftb directory')
    parser.add_argument('--assert-chip-reset', action='store_true',
                        help='Assert the ADT chip reset before releasing it during link startup')
    parser.add_argument('--bootstrap-d3hot-no-pme', action='store_true',
                        help='Qualify D3hot with PME disabled; requires --bootstrap-d3hot')
    parser.add_argument('--initialize-runtime-host', action='store_true',
                        help='Create generated control/Bluetooth/Wi-Fi contexts and basic queues after runtime firmware')
    parser.add_argument('--detach-bootstrap-domain', action='store_true',
                        help='Stop first-stage DMA and replace its domain for runtime; requires owned runtime DMA')
    parser.add_argument('--runtime-before-submit-delay', type=float, default=0,
                        help='Bounded diagnostic delay after runtime resource publication')
    args = parser.parse_args()
    if args.runtime_boot_buffer_from_image and (not args.runtime_image or args.runtime_boot_buffer or args.runtime_boot_buffer_size is not None):
        parser.error('Firmware-derived allocation requires a runtime image and no other allocation input')
    if args.assert_chip_reset and args.reuse_link:
        parser.error('Chip reset requires new link startup, without --reuse-link')
    if args.bootstrap_d3hot_no_pme and not args.bootstrap_d3hot:
        parser.error('Disabling transition PME requires --bootstrap-d3hot')
    if args.initialize_runtime_host and not (args.runtime_image and args.keep_ready_dma and args.shell):
        parser.error('Runtime host initialization requires --runtime-image --keep-ready-dma --shell')
    if args.detach_bootstrap_domain and not (args.runtime_image and args.keep_ready_dma):
        parser.error('Detaching bootstrap DMA requires --runtime-image --keep-ready-dma')
    if args.runtime_boot_buffer_size is not None:
        if args.runtime_boot_buffer or not args.runtime_image or not 0 < args.runtime_boot_buffer_size <= 0x2000000:
            parser.error('Generated runtime allocation requires runtime image, a positive size up to 32MiB, and no copied resource')
    if not 0 <= args.runtime_before_submit_delay <= 30 or args.runtime_before_submit_delay and not args.runtime_image:
        parser.error('Runtime diagnostic delay requires a runtime image and 0..30 seconds')
    if args.keep_ready_dma and not args.shell:
        parser.error('--keep-ready-dma requires --shell to keep the owning process alive')
    if args.keep_ready_dma and args.runtime_image and not args.runtime_opaque_payload:
        parser.error('--keep-ready-dma with --runtime-image requires --runtime-opaque-payload')
    if args.runtime_image and (not args.boot_runtime or not args.opaque_payload):
        parser.error('--runtime-image requires --boot-runtime and --opaque-payload')
    if args.runtime_opaque_payload and not args.runtime_image:
        parser.error('--runtime-opaque-payload requires --runtime-image')
    if args.runtime_boot_buffer and not args.runtime_image:
        parser.error('--runtime-boot-buffer requires --runtime-image')
    if not os.environ.get('M1N1DEVICE'):
        parser.error('Set M1N1DEVICE to the explicitly selected proxy device')
    from m1n1.proxy import UartInterface, M1N1Proxy
    from m1n1.proxyutils import ProxyUtils, bootstrap_port
    interface = UartInterface()
    proxy = M1N1Proxy(interface)
    bootstrap_port(interface, proxy)
    utils = ProxyUtils(proxy, heap_size=128 * 1024 * 1024)
    utils.allow_disassembly = False
    proxy.pmgr_adt_power_enable('/arm-io/apcie')
    driver = Centauri(utils)
    if args.reuse_link:
        if not driver.probe()['link_active']:
            raise RuntimeError('--reuse-link requires an already trained link')
    else:
        if args.assert_chip_reset:
            # Earlier link startup only released this line. Explicitly
            # assert it to test independence from a previous radio session.
            # This hold duration is a prototype qualification parameter.
            print('Asserted chip reset:', driver.reset_key_state(output_level=False), flush=True)
            time.sleep(0.1)
        if not driver.root_ready():
            driver.initialize_root()
        driver.start_link()
    print('Resources:', driver.assign_bars(), flush=True)
    print('Startup:', driver.control_snapshot(), flush=True)
    if args.reference_aux:
        print('Auxiliary setup:', driver.configure_reference_auxiliary_banks(), flush=True)
    if args.reference_pcie:
        print('PCIe setup:', driver.configure_bootstrap_pcie(), flush=True)
    if args.enable_msi:
        print('MSI setup:', driver.enable_bootstrap_msi(), flush=True)
        print('Endpoint:', driver.function(1, 0, 0), flush=True)
    if args.bootstrap_host_time:
        print('Bootstrap host time:', driver.publish_host_time(), flush=True)
    if args.dart_adt_parameters:
        print('DART ADT parameters:', driver.configure_dart_adt_parameters(), flush=True)
    if args.dart_enable_declared_bypass:
        print('DART reserved streams:', driver.configure_declared_dart_bypass_streams(), flush=True)
    if args.bootstrap_ptm:
        print('Bootstrap PTM:', driver.configure_ptm_profile(), flush=True)
    opaque_data = Path(args.opaque_payload).read_bytes() if args.opaque_payload else None
    result = probe(utils, driver, mapped=not args.unmapped,
                   payload_size=args.payload_size,
                   payload_mapped=not args.payload_unmapped,
                   payload_data=opaque_data,
                   record_iova=args.record_iova, payload_iova=args.payload_iova,
                   timeout=args.timeout,
                   keep_ready_dma=args.keep_ready_dma and not args.detach_bootstrap_domain)
    print(json.dumps(result, indent=2), flush=True)
    if args.boot_runtime:
        if result['status'] != '0x4':
            raise RuntimeError('The image did not reach ready state; runtime restart was not attempted')
        print('Runtime enumeration:', driver.warm_restart_after_image(
            request_d3hot=args.bootstrap_d3hot, request_pme=not args.bootstrap_d3hot_no_pme), flush=True)
        print('Runtime resources:', driver.assign_runtime_bars(), flush=True)
        print('Runtime registers:', driver.runtime_snapshot(), flush=True)
        if args.runtime_image:
            if args.reference_aux:
                print('Runtime auxiliary setup:', driver.configure_reference_auxiliary_banks(), flush=True)
            print('Runtime MSI:', driver.enable_runtime_msi(), flush=True)
            print('Runtime PCIe:', driver.configure_bootstrap_pcie(), flush=True)
            if args.bootstrap_ptm:
                print('Runtime PTM:', driver.configure_ptm_profile(), flush=True)
            runtime_data = Path(args.runtime_opaque_payload).read_bytes() if args.runtime_opaque_payload else None
            boot_data = Path(args.runtime_boot_buffer).read_bytes() if args.runtime_boot_buffer else None
            if args.runtime_boot_buffer_size is not None:
                boot_data = bytes(args.runtime_boot_buffer_size)
            if args.runtime_boot_buffer_from_image:
                from centauri_ftab import make_boot_allocation
                boot_data = make_boot_allocation(runtime_data if runtime_data is not None else opaque_data)
            result = probe(utils, driver, payload_data=runtime_data, timeout=args.timeout, runtime=True,
                           record_iova=0x08000000 if args.keep_ready_dma else args.record_iova,
                           payload_iova=0x09000000 if args.keep_ready_dma else args.payload_iova,
                           reuse_dma=runtime_data is None, boot_buffer_data=boot_data,
                           keep_ready_dma=args.keep_ready_dma,
                           retain_owned_domain=args.keep_ready_dma and not args.detach_bootstrap_domain,
                           before_submit_delay=args.runtime_before_submit_delay)
            print('Runtime image:', json.dumps(result, indent=2), flush=True)
            print('After runtime image:', driver.runtime_snapshot(), flush=True)
    runtime_host = None
    if args.initialize_runtime_host:
        from centauri_runtime import RuntimeHost
        runtime_host = RuntimeHost(utils, driver, runtime_data if runtime_data is not None else opaque_data)
        print('Generated runtime host:', json.dumps(runtime_host.snapshot(), indent=2), flush=True)
    if args.shell:
        import code
        try:
            code.interact(banner='Direct Centauri host shell; p=proxy, u=utils, d=driver, result=last probe',
                          local=dict(p=proxy, u=utils, d=driver, result=result, runtime_host=runtime_host))
        finally:
            if runtime_host is not None:
                runtime_host.close()
            if args.keep_ready_dma:
                config = driver.ecam + (1 << 20)
                utils.write(config + R.PCI_COMMAND,
                            driver.cfg(1, 0, 0, R.PCI_COMMAND, 2) & ~R.PCI_COMMAND_BUS_MASTER, 16)
                for owner in getattr(driver, 'owned_image_domains', []):
                    owner['dart'].dart.regs.DISABLE_STREAMS[0].val = 1 << owner['sid']


if __name__ == '__main__':
    main()
