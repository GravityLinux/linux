# SPDX-License-Identifier: GPL-2.0-only
"""Package opaque firmware, factory calibration and explicit platform inputs.

No executable payload inspection or signature creation. Factory calibration
extraction parses only the nonexecutable BWC3 data envelope. Outputs remain
external firmware resources; no recorded host state is required.
"""
import argparse
import hashlib
from pathlib import Path

from centauri_ftab import DEVICE_INPUT_TAGS, prepare_image
from centauri_calibration import extract_calibration
from centauri_nvram import make_antenna_nvram, sku_from_dt_property


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--vendor', required=True, type=Path)
    parser.add_argument('--manifest', required=True, type=Path)
    nvram = parser.add_mutually_exclusive_group(required=True)
    nvram.add_argument('--nvram', type=Path, help='Explicit external NVRAM record')
    nvram.add_argument('--antenna-sku', help='Generate NVRAM from the platform antenna SKU')
    nvram.add_argument('--antenna-sku-file', type=Path,
                       help='Generate NVRAM from the DT apple,antenna-sku string property')
    calibration = parser.add_mutually_exclusive_group(required=True)
    calibration.add_argument('--calibration', type=Path,
                             help='Explicit external host calibration record')
    calibration.add_argument('--calibration-factory-record', type=Path,
                             help='Extract unchanged data from this device\'s BWC3 factory file')
    parser.add_argument('--ap-policy', required=True, type=Path)
    parser.add_argument('--boot-parameters', type=Path,
                        help='Optional _bpb record; control-plane tests also pass without it')
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    if args.nvram is not None:
        nvram = args.nvram.read_bytes()
    elif args.antenna_sku_file is not None:
        nvram = make_antenna_nvram(sku_from_dt_property(args.antenna_sku_file.read_bytes()))
    else:
        nvram = make_antenna_nvram(args.antenna_sku)
    paths = (args.calibration, args.ap_policy, args.boot_parameters)
    inputs = {tag: path.read_bytes() for tag, path in zip(DEVICE_INPUT_TAGS[1:], paths)
              if path is not None}
    inputs[b'nvrm'] = nvram
    if args.calibration_factory_record is not None:
        inputs[b'_cal'] = extract_calibration(args.calibration_factory_record.read_bytes())
    data = prepare_image(args.vendor.read_bytes(), args.manifest.read_bytes(),
                         inputs)
    with args.output.open('xb') as stream:
        stream.write(data)
    print(f'{len(data)} bytes; SHA-256 {hashlib.sha256(data).hexdigest()}')


if __name__ == '__main__':
    main()
