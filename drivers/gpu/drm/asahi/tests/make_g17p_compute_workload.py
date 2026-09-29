#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Export the shim's source-owned add3 caller workload for the native test.

Only dependency-free workload constructors and the named launch-header AST
are evaluated. No proxy setup, captured firmware pages or reference output.
"""
import argparse
import ast
import importlib
import importlib.util
from pathlib import Path
import struct
import sys
import types

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("m1n1", type=Path)
p.add_argument("output", type=Path)
p.add_argument("--batch-count", type=int, default=0)
args = p.parse_args()
experiments = args.m1n1 / "proxyclient/experiments"
package = types.ModuleType("workload_reference")
package.__path__ = [str(args.m1n1 / "proxyclient/m1n1/agx")]
sys.modules[package.__name__] = package
compute = importlib.import_module("workload_reference.g17p_compute")
spec = importlib.util.spec_from_file_location("add3_owned", experiments / "g17p_add3_code.py")
code = importlib.util.module_from_spec(spec)
spec.loader.exec_module(code)
tree = ast.parse((experiments / "agx_g17p_native_add3.py").read_text())
names = {"NATIVE_ADD3_SHADER", "SHADER", "CODE_IMAGE"}
selected = [node for node in tree.body if
            (isinstance(node, ast.FunctionDef) and node.name == "build_add3_preamble") or
            (isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id in names for t in node.targets))]
scope = {"struct": struct}
exec(compile(ast.Module(body=selected, type_ignores=[]), "source-owned-launch-header", "exec"), scope)
usc = 0x10000000000
shader, cdm, resource = (0x10000120000, 0x10000140000, 0x10000170000)
input_a, input_b, output = 0x10000030000, 0x10000038000, 0x10000108000
stream = compute.build_cdm_stream((compute.build_direct_dispatch(shader, (64, 1, 1),
    (32, 1, 1), config=0x80000, constant=0x1000000, tail=0x60000160),))
bodies = [("code", usc, code.build_add3_code_image(), 0x4000, False),
          ("shader", shader, scope["build_add3_preamble"](shader, usc), 0x4000, True),
          ("cdm", cdm, stream, 0x4000, False),
          ("resource", resource, compute.build_buffer_resource_table((input_a,input_b,output), size=0xc000), 0xc000, True)]
batch = []
if args.batch_count:
    batch_tree = ast.parse((experiments / "agx_g17p_modern_compute_batch.py").read_text())
    fn = next(n for n in batch_tree.body if isinstance(n, ast.FunctionDef) and n.name == "build_workloads")
    batch_scope = {"DEFAULT_SHADER_START": 0x200000, "PAGE": 0x4000, "RESOURCE_SIZE": 0xc000,
                   "USC_BASE": usc, "CODE_IMAGE": usc, "struct": struct, "compute": compute,
                   "build_add3_code_image": code.build_add3_code_image,
                   "build_add3_preamble": scope["build_add3_preamble"]}
    exec(compile(ast.Module(body=[fn], type_ignores=[]), "source-owned-batch-workloads", "exec"), batch_scope)
    mapping, batch = batch_scope["build_workloads"](args.batch_count)
    bodies = [(f"batch_{i}", address, body, (len(body)+0x3fff)&~0x3fff, writable)
              for i,(address,(body,writable)) in enumerate(mapping.items())]
text = ["/* Generated from source-owned add3 constructors; caller workload only. */",
        "struct workload { uint64_t address; size_t size; const unsigned char *data; int writable; };",
        f"#define COMPUTE_CDM 0x{cdm:x}ULL", f"#define COMPUTE_CDM_SIZE {len(stream)}",
        f"#define COMPUTE_INPUT_A 0x{input_a:x}ULL", f"#define COMPUTE_INPUT_B 0x{input_b:x}ULL",
        f"#define COMPUTE_OUTPUT 0x{output:x}ULL"]
for name, address, body, size, writable in bodies:
    assert len(body) <= size
    text.append(f"static const unsigned char workload_{name}[{size}] = {{")
    fields = [f"[{i}]=0x{b:02x}" for i,b in enumerate(body) if b]
    text.extend("  " + ",".join(fields[i:i+12]) + "," for i in range(0,len(fields),12))
    text.append("};")
text.append("static const struct workload workloads[] = {")
for name, address, body, size, writable in bodies:
    text.append(f"  {{0x{address:x}ULL, {size}, workload_{name}, {int(writable)}}},")
text.append("};")
if batch:
    text.append("struct batch_workload { uint64_t cdm, cdm_size, input_a, input_b, output; };")
    text.append("static const struct batch_workload batch_workloads[] = {")
    for work in batch:
        values = (work["cdm"],len(mapping[work["cdm"]][0]),work["input_b"]-0x4000,work["input_b"],work["output"])
        text.append("  {" + ",".join(f"0x{v:x}ULL" for v in values) + "},")
    text.append("};")
args.output.write_text("\n".join(text) + "\n")
print(f"Generated {args.output}; {sum(len(b) for _,_,b,_,_ in bodies)} source-owned bytes; USC {usc:#x}")
