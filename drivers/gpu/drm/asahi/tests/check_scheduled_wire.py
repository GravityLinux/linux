#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare generated G16C/G17P layouts with the existing source encoders."""

import argparse
import ast
import json
from pathlib import Path
import subprocess
import sys
import tempfile

import host_versions

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("m1n1", type=Path, help="output-qualified Max shim checkout")
parser.add_argument("--rustc", default="rustc")
parser.add_argument("--g17p-layout", type=Path, required=True,
                    help="RE-backed Neo g17p.py layout declarations (parsed, never executed)")
parser.add_argument("--native-queue", type=Path, action="append", default=[],
                    help="captured G16C queue DATA image; compare every byte")
parser.add_argument("--native-pointers", type=Path, action="append", default=[],
                    help="captured initial G16C pointer DATA image; compare every byte")
args = parser.parse_args()
root = Path(__file__).resolve().parents[5]
known = {}
for node in ast.parse(args.g17p_layout.read_text()).body:
    if isinstance(node, ast.Assign) and isinstance(node.value, ast.Constant):
        for target in node.targets:
            if isinstance(target, ast.Name):
                known[target.id] = node.value.value
field_names = {
    "pointers": "QUEUE_POINTERS_ADDR", "ring": "QUEUE_RING_ADDR",
    "jobs": "QUEUE_JOB_LIST_ADDR", "gpu_rptr1": "QUEUE_GPU_RPTR1",
    "gpu_rptr2": "QUEUE_GPU_RPTR2", "gpu_rptr3": "QUEUE_GPU_RPTR3",
    "unk_24": "QUEUE_EVENT_ID", "priority": "QUEUE_PRIORITY",
    "unk_2c": "QUEUE_UNK_2C", "unk_30": "QUEUE_UNK_30",
    "unk_38": "QUEUE_UNK_38", "unk_3c": "QUEUE_UNK_3C",
    "prio5": "QUEUE_PRIO5", "unk_44": "QUEUE_UNK_44", "uuid": "QUEUE_UUID",
    "unk_4c": "QUEUE_UNK_4C", "unk_50": "QUEUE_UNK_50", "busy": "QUEUE_BUSY",
    "unk_78": "QUEUE_UNK_78", "has_commands": "QUEUE_HAS_COMMANDS",
    "inflight": "QUEUE_INFLIGHT", "unk_94": "QUEUE_UNK_94", "unk_98": "QUEUE_UNK_98",
    "context": "QUEUE_CONTEXT_ADDR",
}
reference_assertions = [
    f"assert!(QueueRecordG17P::SIZE == {known['QUEUE_RECORD_SIZE']});",
    f"assert!(QueuePointersG17P::SIZE == {known['QUEUE_PTR_BLOCK_SIZE']});",
]
for field, name in field_names.items():
    reference_assertions.append(f"assert!(core::mem::offset_of!(QueueRecordG17P, {field}) == {known[name]});")
for field, name in [("done", "QUEUE_PTR_DONE"), ("unk_10", "QUEUE_PTR_UNK_10"),
                    ("unk_20", "QUEUE_PTR_UNK_20"), ("read", "QUEUE_PTR_READ"),
                    ("write", "QUEUE_PTR_WRITE"), ("capacity", "QUEUE_PTR_RING_SIZE")]:
    reference_assertions.append(f"assert!(core::mem::offset_of!(QueuePointersG17P, {field}) == {known[name]});")
sys.path.insert(0, str(args.m1n1.resolve() / "proxyclient"))
from m1n1.g16c.queue import Queue, max_pointers

with tempfile.TemporaryDirectory(prefix="scheduled-wire-") as tmp:
    tmp = Path(tmp)
    flags = host_versions.build(tmp, args.rustc)
    checked_wire = tmp / "checked_wire.rs"
    checked_wire.write_text(
        (root / "drivers/gpu/drm/asahi/scheduled_wire.rs").read_text()
        + "\nconst _: () = {\n" + "\n".join(reference_assertions) + "\n};\n"
    )
    baseline = tmp / "baseline.rs"
    baseline.write_bytes(subprocess.check_output([
        "git", "-C", str(root), "show",
        "a6883c3c6e5aa42548543a5a7b82bb6e375d6922:drivers/gpu/drm/asahi/g17p_queue.rs",
    ]))
    harness = tmp / "scheduled_wire.rs"
    harness.write_text(
        Path(__file__).with_name("scheduled_wire.rs").read_text()
        .replace('"../scheduled_wire.rs"', json.dumps(str(checked_wire)))
        .replace('#[path = "../g17p_queue.rs"]\nmod legacy;',
                 f'#[path = {json.dumps(str(baseline))}]\nmod legacy;')
        .replace('"../g17p_queue.rs"', json.dumps(str(root / "drivers/gpu/drm/asahi/g17p_queue.rs")))
    )
    binary = tmp / "scheduled-wire"
    subprocess.run(
        [args.rustc, "--edition=2021", "-Dwarnings",
         str(harness), *flags, "-o", str(binary)],
        check=True,
    )
    lines = subprocess.check_output([str(binary)], text=True).splitlines()
    assert len(lines) == 256
    for case, line in enumerate(lines):
        number, actual, pointer_image = line.split()
        assert int(number) == case
        base = 0xfffffc2000000000 + case * 0x200000
        expected = Queue(
            base + 0x4013, base + 0x8029, base + 0xc043, base + 0x1005b,
            (case * 0x10203041) & 0xffffffff, case % 4,
        ).encode()
        assert bytes.fromhex(actual) == expected, f"Max queue case {case}"
        assert bytes.fromhex(pointer_image) == max_pointers(case * 0x10001), f"Max pointers case {case}"
    for native in args.native_queue:
        subprocess.run([str(binary), "--native-queue", str(native.resolve())], check=True)
    for native in args.native_pointers:
        subprocess.run([str(binary), "--native-pointers", str(native.resolve())], check=True)

print("PASS: all field offsets/sizes/alignment; 256 Max queues/pointers; "
      f"{len(reference_assertions)} independent Neo RE layout assertions; "
      f"3584 Neo queues/256 pointers against tagged baseline; "
      f"{len(args.native_queue) + len(args.native_pointers)} native Max DATA images; "
      "all five legacy macro variants")
