#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Reserve only the source topology's RAM placements; import no page contents."""
import argparse
import importlib.util
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("m1n1", type=Path)
parser.add_argument("--check", action="store_true")
args = parser.parse_args()
path = args.m1n1 / "proxyclient/m1n1/agx/g17p_source_topology.py"
spec = importlib.util.spec_from_file_location("topology", path)
src = importlib.util.module_from_spec(spec)
spec.loader.exec_module(src)
page = src.PAGE_SIZE
assert page == 0x4000
leaves = {pa for pa in src.native_firmware_leaf_pages().values() if pa >= 0x10000000000}
tables = {pa for tree in src.NATIVE_TABLE_TARGETS.values() for pa in tree.values()}
# The second ASC's top table is independent, at the same offset in its half.
tables.add(src.NATIVE_TABLE_TARGETS["firmware_high"][()] + 0x40000)
# This page is already owned by the ADT's gfx-shared-l2-region reservation.
borrowed = {src.NATIVE_TABLE_TARGETS["firmware_high"][(2,)]}
assert borrowed == {0x101fff34000}
ranges = []
for pa in sorted((leaves | tables) - borrowed):
    assert pa % page == 0 and 0x10000000000 <= pa < 0x10200000000
    if ranges and ranges[-1][0] + ranges[-1][1] == pa:
        ranges[-1][1] += page
    else:
        ranges.append([pa, page])

header = [
    "// SPDX-License-Identifier: GPL-2.0-only OR MIT",
    "// Generated from g17p_source_topology.py by tests/import_g17p_topology.py.",
    "// Address-only source topology. No table or firmware page bodies.",
]
rust = header + ["", f"pub(crate) const SHARED_L2: u64 = {next(iter(borrowed)):#x};", "", "#[rustfmt::skip]", f"pub(crate) const RESERVATIONS: [(u64, u64); {len(ranges)}] = ["]
rust += [f"    ({pa:#x}, {size:#x})," for pa, size in ranges]
rust += ["];", "", "#[rustfmt::skip]", "pub(crate) const LEAF_RUNS: &[(u64, u64, usize, i64)] = &["]
rust += [f"    ({va:#x}, {pa:#x}, {count}, {stride:#x}),"
         for va, pa, count, stride in src.NATIVE_FIRMWARE_LEAF_RUNS]
rust += ["];", "", "#[rustfmt::skip]", "pub(crate) const TABLE_TARGETS: &[(usize, &[usize], u64)] = &["]
assert set(src.NATIVE_TABLE_TARGETS) == {"context0", "render_low", "firmware_high"}
for group, name in enumerate(("context0", "render_low", "firmware_high")):
    tree = src.NATIVE_TABLE_TARGETS[name]
    rust.append(f"    // {group}: {name}")
    for indices, pa in tree.items():
        rust.append(f"    ({group}, &[" + ", ".join(str(index) for index in indices) + f"], {pa:#x}),")
rust += ["];", ""]

dts = header + [
    "", "/ {", "\treserved-memory {", "\t\t#address-cells = <2>;", "\t\t#size-cells = <2>;", "\t\tranges;", "",
    f"\t\tneo_source_memory: gpu-source-memory@{ranges[0][0]:x} {{",
    '\t\t\tcompatible = "apple,neo-gpu-source-memory";',
    '\t\t\tstatus = "okay";', "\t\t\tno-map;",
    "\t\t\t/* Excludes MMIO and the existing ADT L2 reservation. */",
]
for index, (pa, size) in enumerate(ranges):
    prefix = "\t\t\treg = " if index == 0 else "\t\t\t      "
    suffix = ";" if index == len(ranges) - 1 else ","
    dts.append(prefix + f"<{pa >> 32:#x} {pa & 0xffffffff:#x} 0 {size:#x}>" + suffix)
dts += ["\t\t};", "\t};", "};", ""]
driver = Path(__file__).resolve().parent.parent
linux = driver.parents[3]
outputs = {
    driver / "g17p_topology.rs": "\n".join(rust),
    linux / "arch/arm64/boot/dts/apple/t8140-neo-gpu-memory.dtsi": "\n".join(dts),
}
for output, text in outputs.items():
    if args.check:
        assert output.read_text() == text, f"source topology differs: {output}"
    else:
        output.write_text(text)
print(f"{'PASS:' if args.check else 'Generated:'} {len(ranges)} RAM reservations, {sum(size for _, size in ranges):#x} bytes; MMIO and ADT-owned L2 excluded")
