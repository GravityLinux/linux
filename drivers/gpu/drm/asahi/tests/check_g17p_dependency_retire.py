#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Compare native retirement predicates with actual source queue/status gates.

Synthetic owned observations only; no firmware, hardware or captured objects.
"""
import argparse
import ast
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace as NS

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("m1n1", type=Path)
p.add_argument("--rustc", default="rustc")
args = p.parse_args()
agx = args.m1n1 / "proxyclient/m1n1/agx"
backend = ast.parse((agx / "g17p_backend.py").read_text())
queue_fence = next(n for n in backend.body if isinstance(n, ast.ClassDef) and n.name == "G17PQueueFence")
g17p = ast.parse((agx / "g17p.py").read_text())
reached = next(n for n in g17p.body if isinstance(n, ast.FunctionDef) and n.name == "producer_reached")
ns = {}
producer_mask = next(n for n in g17p.body if isinstance(n, ast.Assign) and any(
    isinstance(t, ast.Name) and t.id == "PRODUCER_MASK" for t in n.targets))
exec(compile(ast.Module(body=[producer_mask, reached], type_ignores=[]), "source-producer-reached", "exec"), ns)
ns["g17p"] = NS(producer_reached=ns["producer_reached"])
exec(compile(ast.Module(body=[queue_fence], type_ignores=[]), "source-queue-fence", "exec"), ns)
shim = ast.parse((agx / "shim.py").read_text())
wave = next(n for n in ast.walk(shim) if isinstance(n, ast.FunctionDef) and n.name == "_submit_dependency_wave")
order_statements = [n for n in wave.body if isinstance(n, ast.Assign) and any(
    isinstance(t, ast.Name) and t.id in ("last_compute", "finish_order") for t in n.targets)]
order_ns = dict(staged=[("compute", None), ("render", None), ("compute", None)])
exec(compile(ast.Module(body=order_statements, type_ignores=[]), "source-finish-order", "exec"), order_ns)
assert order_ns["finish_order"] == [2, 1, 0]

render = ast.parse((agx / "g17p_shim.py").read_text())
snapshot_method = next(n for n in ast.walk(render) if isinstance(n, ast.FunctionDef) and n.name == "render_completion_snapshot")
exec(compile(ast.Module(body=[snapshot_method], type_ignores=[]), "source-render-status", "exec"), ns)

def oracle(mask, start, growth):
    completed, changed, statuses = [], [], []
    for i in range(4):
        target = (start + (2 if i == 3 else 1)) & 255
        counters = [target if mask & (1 << (4 + i)) else start,
                    target if mask & (1 << (8 + i)) else start, target]
        status = bytearray(0x40)
        if mask & (1 << (12 + i)):
            status[0x3f if i in (1, 2) else 7] = 1
        statuses.append(bytes(status))
        queue = NS(address=i + 1, indices=lambda i=i: {"done": 3 if mask & (1 << i) else 2})
        fence = ns["G17PQueueFence"](None, None, queue,
            {"producer": target, "consumers_before": [start, start], "write_after": 3},
            status_read=lambda body=bytes(status): body, status_initial=bytes(0x40))
        completed.append(fence._observe(counters))
        changed.append(fence.status_changed())
    owner = NS(_read_completion_data=lambda address, size: statuses[address], read_report_channels=lambda: {})
    render_snapshot = ns["render_completion_snapshot"](owner, {"status_before": {
        kind: {"address": i, "before": bytes(0x40).hex()}
        for kind, i in (("tiling", 1), ("fragment", 2))}})
    gates = [completed[0] and changed[0],
             completed[1] and completed[2] and render_snapshot["statuses_changed"] and growth,
             completed[3] and changed[3]]
    result = ""
    for i in order_ns["finish_order"]:
        if not gates[i]:
            break
        result += "ORC"[i]
    return result or "-"

# Exhaust every independent done, primary consumer, secondary consumer and
# own-status predicate. Repeat the ready predecessors at the wrap boundary.
cases = [(mask, 0, growth) for mask in range(0x10000) for growth in (0, 1)]
cases += [(mask, start, growth) for start in (254, 255) for mask in range(0xf000, 0x10000) for growth in (0, 1)]
expected = [oracle(*case) for case in cases]
with tempfile.TemporaryDirectory() as tmp:
    binary = Path(tmp) / "retire"
    subprocess.run([args.rustc, "--edition=2021", "-Dwarnings",
                    str(Path(__file__).with_name("g17p_dependency_retire.rs")), "-o", str(binary)], check=True)
    actual = subprocess.check_output([str(binary)], input="".join(
        f"{mask} {start} {growth}\n" for mask, start, growth in cases).encode()).decode().splitlines()
    assert len(actual) == len(expected)
    for case, a, b in zip(cases, actual, expected):
        assert a == b, (case, a, b)
print(f"PASS: {len(cases)} retirement cases against source queue fences/render status/finish order; wrap, independent consumers, own statuses and growth gating; terminal failure/order checks")
