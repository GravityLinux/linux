#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Differentially execute the real Python build_initdata function without hardware.

Only the two builder function definitions are compiled from the experiment's AST.
Its module setup, hardware imports and experiment entrypoint are never executed.
"""
import argparse
import ast
import contextlib
import copy
import importlib.util
import io
from pathlib import Path
import struct
import subprocess
import tempfile
from types import SimpleNamespace

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("m1n1", type=Path)
parser.add_argument("--rustc", default="rustc")
args = parser.parse_args()

def load(name):
    path = args.m1n1 / "proxyclient/m1n1/agx" / (name + ".py")
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

g17p, abi = load("g17p"), load("g17p_initdata")
path = args.m1n1 / "proxyclient/experiments/agx_g17p_boot.py"
parsed = ast.parse(path.read_text(), filename=str(path))
functions = [node for node in parsed.body if isinstance(node, ast.FunctionDef)
             and node.name in ("initialize_work_rings", "build_initdata")]
assert len(functions) == 2
code = compile(ast.Module(body=functions, type_ignores=[]), str(path), "exec")
PAGE = 0x4000

class Arena:
    def __init__(self, base):
        self.va = base + g17p.NATIVE_HWDATA_OFFSET
        self.next_pa = 0x11000000000
        self.entries = []
        self.flags = {}

    def alloc(self, size, name):
        address = self.va
        self.va += (size + PAGE - 1) & -PAGE
        return self.alloc_at(address, size, name)

    def alloc_at(self, va, size, name):
        assert va % PAGE == 0
        size = (size + PAGE - 1) & -PAGE
        pa = self.next_pa
        self.next_pa += size
        self.entries.append(dict(va=va, pa=pa, size=size, data=bytearray(size)))
        self.iomap_at(0, va, pa, size, AttrIndex=2, AP=1)
        return va, pa

    def write(self, pa, data):
        for entry in self.entries:
            offset = pa - entry["pa"]
            if 0 <= offset and offset + len(data) <= entry["size"]:
                entry["data"][offset:offset + len(data)] = data
                return
        raise AssertionError(f"unallocated reference write {pa:#x}+{len(data):#x}")

    def memset32(self, pa, value, size):
        assert value == 0
        self.write(pa, bytes(size))

    def iomap_at(self, context, va, pa, size, **flags):
        value = 3 | (1 << 55) | (flags.get("UXN", 1) << 54) | (1 << 10)
        value |= flags.get("AttrIndex", 0) << 2 | flags.get("AP", 1) << 6
        for address in range(va, va + size, PAGE):
            self.flags[address] = value

    def flush_dirty(self):
        pass

    def invalidate_cache(self):
        pass

    def dc_civac(self, *args):
        pass

with tempfile.TemporaryDirectory(prefix="g17p-graph-") as tmp:
    tmp = Path(tmp)
    executable = tmp / "oracle"
    subprocess.run([args.rustc, "--edition=2021", str(Path(__file__).with_name("g17p_initgraph.rs")), "-o", str(executable)], check=True)
    subprocess.run([str(executable), str(tmp / "bytes")], check=True)
    original_performance = copy.deepcopy(g17p.PERF_TABLES)
    checked = 0
    for case in range(4):
        base = 0xfffffc2000000000 + case * 0x200000000
        arena = Arena(base)
        g17p.PERF_TABLES = copy.deepcopy(original_performance)
        g17p.PERF_TABLES["freq_a"][10] += case
        g17p.PERF_TABLES["core_voltage"][3] += case
        namespace = dict(g17p=g17p, build=abi, PAGE=PAGE, CONTEXT=0, p=arena,
                         struct=struct, legacy_aug5_topology=lambda: False,
                         MemoryAttr=SimpleNamespace(Normal=0, Device=1, Shared=2))
        exec(code, namespace)
        with contextlib.redirect_stdout(io.StringIO()):
            reference = namespace["build_initdata"](arena, arena, base)
        manifest = (tmp / "bytes" / f"{case}-objects.txt").read_text().splitlines()
        manifest = {int(va, 16): (int(size, 16), int(flags, 16))
                    for va, size, flags in (line.split() for line in manifest)}
        assert len(manifest) == len(arena.entries) == 12
        for entry in arena.entries:
            va, size = entry["va"], entry["size"]
            actual = (tmp / "bytes" / f"{case}-{va:x}.bin").read_bytes()
            expected = entry["data"]
            if actual != expected:
                offset = next(i for i, (a, b) in enumerate(zip(actual, expected)) if a != b)
                raise AssertionError(f"case {case}, object {va:#x} first mismatch at {offset:#x}")
            assert manifest[va][0] == size
            for page in range(va, va + size, PAGE):
                assert manifest[va][1] == arena.flags[page], f"memory attributes differ at {page:#x}"
            checked += 1
        channels = b"".join(struct.pack("<4Q", *states, ring)
                            for instance in reference["instances"] for states, ring in instance["channels"])
        assert channels == (tmp / "bytes" / f"{case}-channels.bin").read_bytes()
        by_pa = {entry["pa"]: entry["va"] for entry in arena.entries}
        aliases = b"".join(struct.pack("<QQ", low, by_pa[pa])
                           for low, (pa, size) in reference["primary_region_aliases"].items())
        assert aliases == (tmp / "bytes" / f"{case}-aliases.bin").read_bytes()
    print(f"PASS: {checked} complete boot allocations, channel identities, aliases and all page attributes match Python build_initdata")
