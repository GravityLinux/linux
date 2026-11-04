#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Generate a firmware-initialized Centauri PCI tree from ADT and owned handoffs.

Offline only: reads a saved ADT and host metadata, never executable contents.
A loader must copy the output blobs to the manifest's reserved physical ranges
before boot. This does not transfer PCIe power/reset/DART ownership to Linux.
"""
import argparse
import json
from pathlib import Path

from pack_kernel_handoff import pack as pack_wifi
from pack_bluetooth_handoff import pack as pack_bluetooth

PAGE_SIZE = 4096


def cells(value):
    if not 0 <= value < 1 << 64:
        raise ValueError('Address or length does not fit DT cells')
    return f'0x{value >> 32:x} 0x{value & 0xffffffff:x}'


def memory_extent(address, size, state):
    reserved_size = (size + PAGE_SIZE - 1) & -PAGE_SIZE
    if address & (PAGE_SIZE - 1) or not state['dram_base'] <= address < address + reserved_size <= state['dram_base'] + state['dram_size']:
        raise ValueError('Unaligned handoff or extent outside DRAM')
    if not any(base <= address and address + reserved_size <= base + length
               for base, length in state['reserved_dram']):
        raise ValueError('Handoff metadata must be staged in already reserved host RAM')
    return address, reserved_size


def describe_adt(adt):
    controller = adt['/arm-io/apcie']
    bridge = controller['pci-bridge0']
    dart = adt['/arm-io/dart-apcie0']
    # ADT PCI addresses store (flags, low, high), unlike Linux's flags/high/low.
    ranges = []
    for entry in controller.ranges:
        flags, low, high = entry.bus_addr
        if flags >> 24 & 3 in (2, 3):
            ranges.append((flags, high << 32 | low,
                           controller._parent.translate(entry.parent_addr), entry.size))
    bus_range = controller.getprop('bus-range')
    info = dict(ecam=list(controller.get_reg(0)), bus_range=[bus_range & 0xffffffff, bus_range >> 32],
                ranges=ranges, dart=list(dart.get_reg(0)),
                dart_irq=list(dart.interrupts), root_irqs=list(controller.interrupts),
                msi_first=controller.getprop('msi-vector-offset'), msi_count=bridge.getprop('#msi-vectors'))
    info['wifi_mac'] = bytes(bridge['centauri-alpha'].getprop('local-mac-address')).hex()
    info['bluetooth_mac'] = bytes(adt['/arm-io/bluetooth'].getprop('local-mac-address')).hex()
    return info


def generate(info, wifi, bluetooth, wifi_address, bluetooth_address, *, base_dts=None):
    if wifi['handoff_nonce'] != bluetooth['handoff_nonce'] or wifi['dart_base'] != bluetooth['dart_base'] or wifi['dart_base'] != info['dart'][0]:
        raise ValueError('Handoffs are from different boots or DART resources')
    # The host-owned TX arena uses only offsets below 0x60000 for packet slots.
    # Metadata must avoid active packet slots and all Bluetooth DMA.
    blobs = [('centauri-wifi-handoff.bin', pack_wifi(wifi), wifi_address),
             ('centauri-bluetooth-handoff.bin', pack_bluetooth(bluetooth), bluetooth_address)]
    loads = []
    for name, blob, address in blobs:
        address, size = memory_extent(address, len(blob), wifi)
        if any(address < item['physical'] + item['reserved_size'] and item['physical'] < address + size for item in loads):
            raise ValueError('Overlapping metadata staging ranges')
        for stream in bluetooth['streams'].values():
            if any(address < m['physical'] + m['length'] and m['physical'] < address + size for m in stream['mappings']):
                raise ValueError('Metadata overlaps Bluetooth device DMA')
        nonce = wifi['handoff_nonce_physical']
        if address < nonce + 32 and nonce < address + size:
            raise ValueError('Metadata overlaps the current-boot nonce')
        arena = wifi['owned_tx_arena_physical']
        if not arena + 0x60000 <= address < address + size <= arena + wifi['owned_tx_arena_size']:
            raise ValueError('Stage metadata only in the unused tail of the owned TX arena')
        loads.append(dict(file=name, physical=address, bytes=len(blob), reserved_size=size))
    ranges = ',\n\t\t\t'.join(f'<0x{flags:x} {cells(bus)} {cells(physical)} {cells(size)}>' for flags, bus, physical, size in info['ranges'])
    ecam, ecam_size = info['ecam']
    dart, dart_size = info['dart']
    w, b = loads
    def mac(value):
        raw = bytes.fromhex(value)
        if len(raw) != 6:
            raise ValueError('Invalid ADT MAC address')
        return ' '.join(f'{byte:02x}' for byte in raw)
    reservations = []
    for address, length in sorted(set(map(tuple, wifi['reserved_dram']))):
        if not wifi['dram_base'] <= address < address + length <= wifi['dram_base'] + wifi['dram_size']:
            raise ValueError('Retained reservation outside DRAM')
        reservations.append(f'/memreserve/ 0x{address:x} 0x{length:x};')
    base = (Path(__file__).with_name('t8142-j813-minimal.dts').read_text()
            if base_dts is None else base_dts)
    base = base.replace('/dts-v1/;', '/dts-v1/;\n' + '\n'.join(reservations), 1)
    source = f'''// SPDX-License-Identifier: GPL-2.0-only
/* Generated from ADT and current-boot host metadata. Firmware owns reset/DART.
 * Load the manifest blobs before Linux. Do not use this tree for cold startup.
 */
{base}

/ {{
\tchosen {{
\t\tlinux,pci-probe-only = <1>;
\t}};
\treserved-memory {{
\t\tcentauri_wifi_handoff: centauri-wifi-handoff@{w['physical']:x} {{
\t\t\treg = <{cells(w['physical'])} {cells(w['reserved_size'])}>;
\t\t}};
\t\tcentauri_bt_handoff: centauri-bluetooth-handoff@{b['physical']:x} {{
\t\t\treg = <{cells(b['physical'])} {cells(b['reserved_size'])}>;
\t\t}};
\t}};
\tsoc {{
\t\tcentauri_dart: iommu@{dart:x} {{
\t\t\tcompatible = "apple,t8110-dart";
\t\t\treg = <{cells(dart)} {cells(dart_size)}>;
\t\t\t#iommu-cells = <1>;
\t\t\tinterrupt-parent = <&aic>;
\t\t\tinterrupts = <0 {info['dart_irq'][0]} 4>;
\t\t\t/* Preserve firmware's live domains. This node supplies status only. */
\t\t\tstatus = "disabled";
\t\t}};
\t\tcentauri_pcie: pcie@{ecam:x} {{
\t\t\tcompatible = "pci-host-ecam-generic";
\t\t\tdevice_type = "pci";
\t\t\treg = <{cells(ecam)} {cells(ecam_size)}>;
\t\t\tbus-range = <{info['bus_range'][0]} {info['bus_range'][1]}>;
\t\t\t#address-cells = <3>;
\t\t\t#size-cells = <2>;
\t\t\tranges = {ranges};
\t\t\t/* Firmware has trained the port and assigned BARs. Polling only. */
\t\t\tpci@0,0 {{
\t\t\t\tdevice_type = "pci";
\t\t\t\treg = <0 0 0 0 0>;
\t\t\t\tbus-range = <1 1>;
\t\t\t\t#address-cells = <3>;
\t\t\t\t#size-cells = <2>;
\t\t\t\tranges;
\t\t\t\tcontrol@0 {{
\t\t\t\t\tcompatible = "pci106b,1901";
\t\t\t\t\treg = <0x10000 0 0 0 0>;
\t\t\t\t}};
\t\t\t\twifi@0,1 {{
\t\t\t\t\tcompatible = "pci106b,1902";
\t\t\t\t\treg = <0x10100 0 0 0 0>;
\t\t\t\t\tmemory-region = <&centauri_wifi_handoff>;
\t\t\t\t\tapple,firmware-dart = <&centauri_dart>;
\t\t\t\t\tlocal-mac-address = [{mac(info['wifi_mac'])}];
\t\t\t\t}};
\t\t\t\tbluetooth@0,2 {{
\t\t\t\t\tcompatible = "pci106b,1903";
\t\t\t\t\treg = <0x10200 0 0 0 0>;
\t\t\t\t\tmemory-region = <&centauri_bt_handoff>;
\t\t\t\t\tapple,firmware-dart = <&centauri_dart>;
\t\t\t\t\tlocal-mac-address = [{mac(info['bluetooth_mac'])}];
\t\t\t\t}};
\t\t\t}};
\t\t}};
\t}};
}};
'''
    return source, blobs, loads


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('adt', type=Path)
    parser.add_argument('wifi', type=Path)
    parser.add_argument('bluetooth', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--wifi-handoff-physical', required=True, type=lambda v: int(v, 0))
    parser.add_argument('--bluetooth-handoff-physical', required=True, type=lambda v: int(v, 0))
    args = parser.parse_args()
    from m1n1.adt import load_adt
    info = describe_adt(load_adt(args.adt.read_bytes()))
    wifi, bluetooth = json.loads(args.wifi.read_text()), json.loads(args.bluetooth.read_text())
    source, blobs, loads = generate(info, wifi, bluetooth, args.wifi_handoff_physical, args.bluetooth_handoff_physical)
    args.output.mkdir(parents=True, exist_ok=True)
    for name, blob, _ in blobs:
        (args.output / name).write_bytes(blob)
    (args.output / 'centauri.dts').write_text(source)
    (args.output / 'load-manifest.json').write_text(json.dumps(dict(loads=loads, adt=info), indent=2) + '\n')
    print('Generated firmware-initialized PCI DT, two typed handoff blobs and load manifest')


if __name__ == '__main__':
    main()
