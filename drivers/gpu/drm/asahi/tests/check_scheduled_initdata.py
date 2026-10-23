#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Check scheduled initdata layouts, templates and complete source encodings.

Reads RE declarations and owned source builders only; no device access.
"""

import argparse
import ast
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
from types import SimpleNamespace

import host_versions

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("m1n1", type=Path, help="qualified Max source checkout")
parser.add_argument("--g17p-initdata", type=Path, required=True, help="Neo RE declarations")
parser.add_argument("--rustc", default="rustc")
parser.add_argument("--evidence", type=Path, help="new directory for complete compared images")
parser.add_argument("--native-root", type=Path, action="append", default=[], help="captured Max root DATA")
parser.add_argument("--native-main", type=Path, action="append", default=[], help="captured Max main-config DATA")
args = parser.parse_args()
tests = Path(__file__).resolve().parent
root = tests.parents[4]
baseline = "a6883c3c6e5aa42548543a5a7b82bb6e375d6922"
sys.path.insert(0, str(args.m1n1.resolve() / "proxyclient"))
from m1n1.g16c.firmware import Channel, HardwareRegion, Main, Root

known = {}
for node in ast.parse(args.g17p_initdata.read_text()).body:
    if isinstance(node, ast.Assign) and isinstance(node.value, ast.Constant):
        for target in node.targets:
            if isinstance(target, ast.Name):
                known[target.id] = node.value.value

assertions = []


def offset(ty, field, reference):
    assertions.append(f"assert!(core::mem::offset_of!({ty}, {field}) == {known[reference]});")


for ty in ["InitRootG16C", "InitRootG17P"]:
    for field, name in {
        "version": "ROOT_VERSION", "region_a": "ROOT_REGION_A", "unk_10": "ROOT_UNK_10",
        "main": "ROOT_MAIN_CONFIG", "region_c": "ROOT_REGION_C", "kind": "ROOT_KIND",
        "unk_2c": "ROOT_UNK_2C", "granule": "ROOT_GRANULE", "granule_bits": "ROOT_GRANULE_BITS",
        "level_count": "ROOT_LEVEL_COUNT", "levels": "ROOT_LEVELS",
        "status_a": "ROOT_STATUS_A", "status_b": "ROOT_STATUS_B",
    }.items():
        offset(ty, field, name)
for ty in ["InitRootG16C", "SecondaryInitRootG17P"]:
    offset(ty, "extra_0", "ROOT_SECONDARY_EXTRA_0")
    offset(ty, "extra_1", "ROOT_SECONDARY_EXTRA_1")
for ty in ["MainConfigG16C", "MainConfigG17P"]:
    for field, name in {
        "hardware": "MAIN_HWDATA_ADDR", "repeated": "MAIN_REPEATED_ADDR",
        "repeated_2": "MAIN_REPEATED_ADDR_2", "channels": "MAIN_CHANNEL_TABLE",
        "regions": "MAIN_REGION_TRIPLES", "kind": "MAIN_SECONDARY_KIND",
        "byte_mask": "MAIN_BYTE_MASK", "unaligned": "MAIN_SECONDARY_ADDR", "interval": "MAIN_INTERVAL",
    }.items():
        offset(ty, field, name)
offset("MainConfigG17P", "auxiliary", "MAIN_ADDR_ARRAY")
for field, name in {
    "lead": "REGION_RECORD_LEAD", "low": "REGION_RECORD_VALUE", "low_tag": "REGION_RECORD_KIND",
    "high": "REGION_RECORD_ADDR", "count": "REGION_RECORD_SIZE_A",
    "stride": "REGION_RECORD_SIZE_B", "trail": "REGION_RECORD_TRAIL",
}.items():
    offset("HardwareRegion", field, name)
for ty, name in {
    "InitRootG17P": "ROOT_SIZE", "SecondaryInitRootG17P": "ROOT_SECONDARY_SIZE",
    "UatLevel": "LEVEL_DESC_SIZE", "MainConfigG17P": "MAIN_SIZE", "Channel": "CHANNEL_ENTRY_SIZE",
    "RegionTriple": "MAIN_REGION_TRIPLE_STRIDE", "MmioRegion": "REGISTER_ENTRY_SIZE",
    "HardwareRegion": "REGION_RECORD_STRIDE",
}.items():
    assertions.append(f"assert!(core::mem::size_of::<{ty}>() == {known[name]});")

# Execute only the actual register-packing loop in Platform.hwdata over owned
# scalar inputs. The rest of platform discovery and hardware setup never runs.
platform_source = args.m1n1 / "proxyclient/m1n1/g16c/platform.py"
platform_ast = ast.parse(platform_source.read_text())
platform = next(n for n in platform_ast.body if isinstance(n, ast.ClassDef) and n.name == "Platform")
hwdata = next(n for n in platform.body if isinstance(n, ast.FunctionDef) and n.name == "hwdata")
register_loop = next(n for n in hwdata.body if isinstance(n, ast.For))
assert ast.unparse(register_loop.iter) == "self.registers"
register_code = compile(ast.Module(body=[register_loop], type_ignores=[]), str(platform_source), "exec")


def git_file(relative):
    return subprocess.check_output(["git", "-C", str(root), "show", baseline + ":" + relative])


def compile_harness(source, binary, flags):
    subprocess.run([args.rustc, "--edition=2021", "-Dwarnings", str(source), *flags, "-o", str(binary)], check=True)


with tempfile.TemporaryDirectory(prefix="scheduled-initdata-") as temporary:
    tmp = Path(temporary)
    flags = host_versions.build(tmp, args.rustc)
    checked = tmp / "checked_initdata.rs"
    checked.write_text((tests.parent / "scheduled_initdata.rs").read_text()
                       + "\nconst _: () = {\n" + "\n".join(assertions) + "\n};\n")
    harness = tmp / "max.rs"
    harness.write_text((tests / "scheduled_initdata.rs").read_text()
                       .replace('"../scheduled_initdata.rs"', json.dumps(str(checked))))
    binary = tmp / "max"
    compile_harness(harness, binary, flags)
    max_images = tmp / "max-images"
    subprocess.run([str(binary), str(max_images)], check=True)
    count = 0
    for case in range(64):
        def address(index):
            return 0xfffffc2000000000 + (case << 24) + index * 0x4143

        expected = {
            "root": Root(address(1), address(2), address(3), address(4), address(5),
                         0 if case % 3 == 0 else address(6),
                         0 if case % 3 == 1 else address(7)).encode(),
            "main": Main(address(70), address(71), tuple(
                Channel(*(address(i * 4 + j) for j in range(4))) for i in range(17)),
                tuple(address(i) for i in range(72, 78)), tuple(address(i) for i in range(80, 86)),
                address(90)).encode(),
            "region": HardwareRegion(case, 0x7000000000 + case * 0x4000,
                                     0xfffffc20c0000000 + case * 0x4000, case + 1, 0x40,
                                     ((case << 9) | (case >> 23)) & 0xffffffff).encode(),
        }
        out = bytearray(0x3db4)
        r = SimpleNamespace(slot=0, physical=0x288000000 + case * 0x8000, address=address(91),
                            size=(case + 1) * 0x1000, stride=0x4000 << (case & 1), relative=case * 0x1000,
                            flags=case & 3)
        exec(register_code, {"self": SimpleNamespace(registers=[r]), "out": out, "struct": struct})
        expected["mmio"] = bytes(out[0x640:0x668])
        for name, image in expected.items():
            actual = (max_images / f"{name}{case}.bin").read_bytes()
            assert actual == image, (name, case, len(actual), len(image))
            count += 1

    # The original tag's whole ABI oracle remains independent of the new
    # templating. Its own input fixture retains the original bool role API.
    old = tmp / "baseline"
    old.mkdir()
    for name in ["g17p_abi.rs", "g17p_constants.rs"]:
        (old / name).write_bytes(git_file("drivers/gpu/drm/asahi/" + name))
    old_harness = old / "oracle.rs"
    old_harness.write_text(git_file("drivers/gpu/drm/asahi/tests/g17p_abi.rs").decode()
                           .replace('"../g17p_abi.rs"', '"g17p_abi.rs"'))
    current_binary, old_binary = tmp / "neo", tmp / "old-neo"
    compile_harness(tests / "g17p_abi.rs", current_binary, flags)
    compile_harness(old_harness, old_binary, [])
    current_images, old_images = tmp / "neo-images", tmp / "old-images"
    subprocess.run([str(current_binary), str(current_images)], check=True)
    subprocess.run([str(old_binary), str(old_images)], check=True)
    names = sorted(p.name for p in current_images.iterdir())
    assert names == sorted(p.name for p in old_images.iterdir())
    for name in names:
        assert (current_images / name).read_bytes() == (old_images / name).read_bytes(), name

    for name in ["g17p_initgraph.rs", "g17p_layout.rs", "g17p_queue.rs"]:
        (old / name).write_bytes(git_file("drivers/gpu/drm/asahi/" + name))
    old_graph_harness = old / "graph.rs"
    old_graph_text = git_file("drivers/gpu/drm/asahi/tests/g17p_initgraph.rs").decode()
    # This historical fixture has a stale mailbox assertion. Correct only its
    # expected value, independently established by the queue/publication gate;
    # all baseline production serializers and graph builders remain untouched.
    assert old_graph_text.count("0x0083000a00000000") == 1
    old_graph_harness.write_text(old_graph_text.replace('"../', '"')
                                .replace("0x0083000a00000000", "0x008300000000000a"))
    current_graph, old_graph = tmp / "neo-graph", tmp / "old-graph"
    compile_harness(tests / "g17p_initgraph.rs", current_graph, flags)
    compile_harness(old_graph_harness, old_graph, [])
    graph_images, old_graph_images = tmp / "graph-images", tmp / "old-graph-images"
    subprocess.run([str(current_graph), str(graph_images)], check=True)
    subprocess.run([str(old_graph), str(old_graph_images)], check=True)
    graph_names = sorted(p.name for p in graph_images.iterdir())
    assert graph_names == sorted(p.name for p in old_graph_images.iterdir())
    for name in graph_names:
        assert (graph_images / name).read_bytes() == (old_graph_images / name).read_bytes(), name
    for kind, files in [("root", args.native_root), ("main", args.native_main)]:
        for file in files:
            subprocess.run([str(binary), "--native-" + kind, str(file.resolve())], check=True)
    if args.evidence:
        args.evidence.mkdir()
        shutil.copytree(max_images, args.evidence / "max-images")
        shutil.copytree(current_images, args.evidence / "neo-images")
        shutil.copytree(graph_images, args.evidence / "graph-images")
        (args.evidence / "summary.json").write_text(json.dumps({
            "baseline": baseline, "independent_RE_layout_assertions": len(assertions),
            "Max_whole_images": count, "Neo_whole_images": len(names),
            "Neo_graph_files": len(graph_names),
            "baseline_harness_stale_mailbox_assertion_corrected": True,
            "native_root_images": [str(p.resolve()) for p in args.native_root],
            "native_main_images": [str(p.resolve()) for p in args.native_main],
            "firmware_roles": {"G16C": ["Primary"], "G17P": ["Primary", "Secondary"]},
            "hardware_qualification": False,
        }, indent=2) + "\n")
    print(f"PASS: {len(assertions)} independent RE layout assertions; {count} complete Max images; "
          f"{len(names)} complete Neo images identical to tagged baseline; "
          f"{len(graph_names)} unchanged full graph images/manifests; "
          f"{len(args.native_root) + len(args.native_main)} complete native Max DATA images; "
          "explicit primary/secondary templates")
