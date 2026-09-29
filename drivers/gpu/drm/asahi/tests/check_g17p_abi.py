#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Compare the Rust serializers with the current Python shim, without hardware.

Usage: check_g17p_abi.py /path/to/m1n1 [--rustc /path/to/rustc]
No captured objects are loaded; all expected bytes come from Python builders.
"""
import argparse
import importlib.util
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("m1n1", type=Path)
parser.add_argument("--rustc", default="rustc")
args = parser.parse_args()
source = args.m1n1 / "proxyclient/m1n1/agx/g17p_initdata.py"
spec = importlib.util.spec_from_file_location("reference", source)
reference = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reference)

with tempfile.TemporaryDirectory(prefix="g17p-abi-") as tmp:
    tmp = Path(tmp)
    executable = tmp / "oracle"
    subprocess.run([args.rustc, "--edition=2021", str(Path(__file__).with_name("g17p_abi.rs")), "-o", str(executable)], check=True)
    subprocess.run([str(executable), str(tmp / "bytes")], check=True)
    checked = 0

    def check(name, expected):
        global checked
        actual = (tmp / "bytes" / name).read_bytes()
        if actual != expected:
            first = next((i for i, (a, b) in enumerate(zip(actual, expected)) if a != b), min(len(actual), len(expected)))
            raise AssertionError(f"{name}: first mismatch at {first:#x}, lengths {len(actual)}/{len(expected)}")
        checked += 1

    for case in range(64):
        def address(index):
            return 0xfffffc2000000000 + (case << 24) + index * 0x4143

        extra = [(0, 0), (address(6), 0), (0, address(7))][case % 3]
        check(f"root{case}.bin", reference.build_root(
            (0x04c0 ^ case, 0x0396, 0xa322, 0x0c8a), address(1), address(2),
            address(3), address(4), address(5), kind=case & 1,
            secondary_extra_0=extra[0], secondary_extra_1=extra[1]))
        channels = [([address(i * 4 + j) for j in range(3)], address(i * 4 + 3)) for i in range(17)]
        views = [(0xffffffffffffffff, case), (address(77), 0x1840000), (address(78), None)]
        if case & 1:
            main = reference.build_secondary_main_config(address(70), address(71), channels,
                views, address(79) if case & 2 else 0)
        else:
            main = reference.build_main_config(address(70), address(71), channels,
                [address(i) for i in range(72, 77)], views)
        check(f"main{case}.bin", main)
        check(f"channel{case}.bin", reference.build_channel_entry(*channels[case % 17]))
        check(f"register{case}.bin", reference.build_register_entry(
            0x480000000 + case * 0x1800, address(80), 0x21400 + case, case,
            unk_18=0xffffffffffffffff - case))
        check(f"region{case}.bin", reference.build_region_record(
            0x100 + case, 0x1848000 + case, address(81),
            size_a=0x800 + case, size_b=0x40, trail=case & 3))
        perf = {name: [case * 1000 + column * 100 + i for i in range(11)]
                for column, name in enumerate(("freq_a", "freq_b", "core_voltage", "memory_voltage",
                    "scale_b", "relative_a", "relative_b", "index_a", "index_b"))}
        registers = {
            17: dict(phys=0x480000000, device_va=address(80), size=0x21400, flag=2, unk_18=case),
            41: dict(phys=0x480e1f800, device_va=address(81), size=0x4000, flag=2, unk_18=0xffffffffffffffff),
        }
        opaque = [(0x20, bytes.fromhex("12345678"))]
        regions = [dict(lead=0x100, value=case, addr=address(82), size_a=0x800, size_b=0x40, trail=2)]
        check(f"hardware{case}.bin", reference.build_hwdata(registers, {0: case, 52: 2}, perf,
            opaque_fields=[None, [], opaque][case % 3], chip_id=0x8140 if case & 1 else None,
            region_records=regions))
        check(f"primary_status{case}.bin", reference.build_primary_status_b(
            0x5000, address(83), address(84), 0x100, 0x4000 if case & 1 else 0x4900, opaque))
    check("dispatch.bin", reference.build_compute_dispatch_record())
    check("region_c.bin", reference.build_region_c())
    for acknowledged in (False, True):
        check(f"status{int(acknowledged)}.bin", reference.build_status_block(acknowledged))
    print(f"PASS: {checked} complete serialized objects match {source}")
