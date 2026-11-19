# SPDX-License-Identifier: GPL-2.0-only
"""Lab full-chip reset through Linux GPIO-v2 and PCI sysfs ownership APIs.

The caller supplies the bridge, control supplier, GPIO chip and line. Remove
all endpoint devices before changing the chip enable/reset line. This test
has no serial/model gates and no raw MMIO or firmware-state adoption.
"""
import argparse
import json
from pathlib import Path
import time

from gpio_v2_line import Line, chip_info


def write(path, value):
    with path.open('w') as output:
        output.write(value)


def run(bridge, supplier, gpiochip, line):
    devices = Path('/sys/bus/pci/devices')
    parent = (devices/bridge).resolve(strict=True)
    control = (devices/supplier).resolve(strict=True)
    if control.parent != parent:
        raise ValueError('Supplier must be a direct child of the selected bridge')
    children = sorted(parent.glob('????:??:??.?'), reverse=True)
    report = {'bridge':bridge, 'supplier':supplier, 'gpio':chip_info(gpiochip),
              'line':line, 'removed_children':[p.name for p in children]}
    driver = control/'driver'
    if driver.exists():
        write(driver/'unbind', supplier)
    for child in children:
        write(child/'remove', '1')
    if list(parent.glob('????:??:??.?')):
        raise RuntimeError('PCI children remain registered')
    with Line(gpiochip, line, initial_output=1) as pin:
        report['before'] = pin.get()
        pin.set(0)
        report['asserted'] = pin.get()
        time.sleep(.100)
        pin.set(1)
        report['released'] = pin.get()
        time.sleep(.100)
        if (report['asserted'], report['released']) != (0, 1):
            raise RuntimeError('GPIO output readback mismatch')
        write(parent/'reset_subordinate', '1')
        write(parent/'rescan', '1')
    report['pci_reset_and_rescan_returned'] = True
    print(json.dumps(report, sort_keys=True), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bridge', required=True)
    parser.add_argument('--supplier', required=True)
    parser.add_argument('--gpiochip', required=True)
    parser.add_argument('--line', required=True, type=int)
    args = parser.parse_args()
    run(args.bridge, args.supplier, args.gpiochip, args.line)


if __name__ == '__main__':
    main()
