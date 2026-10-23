#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Check versioned work records against native Max DATA/source and tagged Neo.

Executes the actual qualified Max ComputeWork.prepare over owned host DATA.
No target, firmware executable content or shader images are accessed.
"""
import argparse
from dataclasses import replace
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
from types import SimpleNamespace as NS

import host_versions

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1', type=Path)
p.add_argument('--evidence', type=Path, required=True)
args = p.parse_args()
tests = Path(__file__).resolve().parent
linux = tests.parents[4]
baseline = 'a6883c3c6e5aa42548543a5a7b82bb6e375d6922'
sys.path.insert(0, str(args.m1n1 / 'proxyclient'))
from m1n1.g16c import compute as ref
from m1n1.g16c.records import DESCRIPTOR_BASE
from m1n1.g16c.work import ComputeWork
from m1n1.g16c.scheduler import Completion
from m1n1.g16c.transport import Stamp

cases, native = [], {}

def emit(kind, name, words, expected):
    cases.append((name, bytes([kind, len(words)]) + struct.pack('<' + 'Q' * len(words), *words), expected))

def record(name, r, expected=None):
    encoded = r.encode()
    if expected is not None:
        assert encoded == expected, name
    emit(1, name, [r.descriptor, r.queue, r.grid, r.item, r.primary_count,
        r.timeout, int(r.ready), r.retire_control, r.flags, r.control,
        r.control_130, (1 << 64) - 1 if r.header_grid is None else r.header_grid,
        (1 << 64) - 1 if r.header_count is None else r.header_count,
        r.control_low, r.field_30], encoded)

def wrapper(name, w, expected=None):
    encoded = w.encode()
    if expected is not None:
        assert encoded == expected, name
    emit(2, name, [w.scratch_low, w.scratch_high, w.support, w.queue_control,
        w.grid, w.item, w.ordinal, w.context, w.support_id, w.uuid,
        w.scheduler, w.scheduler_hint, int(w.first),
        int(w.first) if w.field_52 is None else w.field_52,
        w.scheduler if w.field_5e is None else w.field_5e,
        int(w.first) if w.field_62 is None else w.field_62], encoded)

for i in range(96):
    item = (0, 1, 255, 256, 65535, (1 << 30) - 2)[i % 6]
    record(f'Max-record-{i}', ref.Record(DESCRIPTOR_BASE + i * 0x4000,
        0xfffffc20c0000000 + i * 0x20000, i % 128, item,
        primary_count=(1, 34, 38, 127)[i % 4], timeout=i * 617,
        ready=i % 2 == 0, retire_control=i * 0x100010001,
        flags=0 if i % 3 == 0 else 1 << 60, control=(0x804, 0x1804, 0xc04)[i % 3],
        control_130=i * 0x100000000001,
        header_grid=None if i % 3 == 0 else (i + 17) % 128,
        header_count=None if i % 3 == 0 else i * 4567,
        control_low=i * 0x1020304, field_30=i << 40 | item))
    wrapper(f'Max-wrapper-{i}', ref.Wrapper(0x70004d8000 + i * 0x4000,
        0xfffffc2000278000 + i * 0x4000, 0xfffffc20c0998000 + i * 8,
        0xfffffc20c07b8040 + i * 0x40, i % 128,
        (0, 1, 255, 256, 65535)[i % 5], i * 617, i % 63 + 1,
        i * 123, i * 257, i % 4, i * 17, i % 2 == 0,
        field_52=None if i % 3 == 0 else i % 4,
        field_5e=None if i % 3 == 0 else i * 337,
        field_62=None if i % 3 == 0 else (i + 1) % 5))

# Exercise the actual Work method, including its fixed scheduler template,
# without replacing it with a Python model of our Rust implementation.
for i in range(64):
    images = {}
    arena = NS(space=NS(write=lambda address, body: images.__setitem__(address, bytes(body))))
    base = DESCRIPTOR_BASE + 0x800000 + i * 0x20000
    lane = NS(queue=0xfffffc20c8000000 + i * 0x20000, priority=i % 4)
    queues = NS(lanes=(None, None, lane), scratch_low=(0, 0, 0x7000400000),
        scratch_high=(0, 0, 0xfffffc2000400000), context=0xfffffc20c9000000, uuid=1000 + i)
    ordinal = (0, 1, 255, 256, 65535, (1 << 30) - 2)[i % 6]
    stamp = ((ordinal + 1) << 8) & 0xffffffff
    completion = Completion(Stamp(lane.queue + 0x1040, stamp),
        Stamp(base + 0xc100, stamp), i % 128, i + 1, (queues, arena))
    state = ref.State(base + 0x8000, 0x7000000000 + base - DESCRIPTOR_BASE,
        ordinal, i % 128, completion.driver.address, completion.firmware.address,
        base + 0xc120, base + 0xc128, base + 0x14000, base + 0x10000,
        0x21000001, i % 4, i * 257, i % 2, stamp >> 8,
        ordinal, ordinal + 1, ordinal)
    context = i % 63 + 1
    parameters = ref.Parameters(0x10000200000, 0x10003000000, 0x10003000100,
        0x0200034503000346, context, ordinal & 255, 0x10004000000, 0x7000220000,
        usc_exec_base=None if i % 2 == 0 else 1 << 40)
    w = ComputeWork(arena, queues, base, state, completion, ordinal, i * 17,
        i * 131, i * 31, context, wrapper_field_5e=None if i % 3 == 0 else i * 7)
    stage = NS(stage=2, completion=completion, timestamps=(base + 0x18000, base + 0x18008),
        attachments=(), dependency_records=lambda: ())
    request = w.prepare(parameters, stage)
    assert request.works == (base + 0x4000, base + 0x40c0)
    record(f'Work.prepare-record-{i}', ref.Record(base, lane.queue, state.grid, ordinal,
        primary_count=len(ref.registers(parameters)) - 2),
        images[queues.scratch_high[2] + ((ordinal + 1) & 255) * 0x200])
    wrapper(f'Work.prepare-wrapper-{i}', ref.Wrapper(queues.scratch_low[2], queues.scratch_high[2],
        state.support, queues.context, state.grid, ordinal & 255, i * 131, context,
        i * 31, queues.uuid, lane.priority, i * 17, ordinal == 0,
        field_5e=w.wrapper_field_5e), images[base + 0x4000])
    emit(3, f'Work.prepare-scheduler-{i}', [base + 0xc140], images[state.scheduler])

# Native protocol DATA: consumed and ready records, and one full Work15 wrapper.
native_dir = args.m1n1 / 'build/m4max-gpu/uat-metal-before-producer'
for path in [native_dir / 'compute-1-record.bin', native_dir / 'compute-2-record.bin',
             args.m1n1 / 'build/m4max-gpu/python-native-compute-inputs/item-1-record.bin']:
    body = path.read_bytes()
    q = lambda at: struct.unpack_from('<Q', body, at)[0]
    r = ref.Record(q(0x10), q(0x18), q(0x28) >> 40, q(0x28) & 0xffffffff,
        primary_count=(q(0x150) >> 40) // 8, timeout=q(0x20) >> 48,
        ready=bool(struct.unpack_from('<I', body, 0x144)[0]), retire_control=q(0x138),
        flags=q(0) & (1 << 60), control=q(0x20) >> 32 & 65535, control_130=q(0x130),
        header_grid=q(0) >> 42 & 127, header_count=(q(0) & 0xfffffffc) >> 2,
        control_low=q(0x20) & 0xffffffff, field_30=q(0x30))
    record('native-' + str(path), r, body)
    native[str(path)] = hashlib.sha256(body).hexdigest()
path = native_dir / 'publication.json'
body = bytes.fromhex(json.loads(path.read_text())['work_items'][0]['bytes'])
q = lambda at: struct.unpack_from('<Q', body, at)[0]
h = lambda at: struct.unpack_from('<H', body, at)[0]
wrapper('native-wrapper', ref.Wrapper(q(8), q(0x10), q(0x36), q(0x4a), h(0x18), h(0x2a),
    h(0x3e), h(0x32), h(0x56), h(0x5a), h(0x1e), h(0x46), bool(h(0x1a)),
    field_52=h(0x52), field_5e=h(0x5e), field_62=h(0x62)), body)
native[str(path)] = hashlib.sha256(body).hexdigest()

with tempfile.TemporaryDirectory(prefix='scheduled-work-') as directory:
    tmp = Path(directory)
    flags = host_versions.build(tmp)
    legacy = tmp / 'legacy.rs'
    legacy.write_bytes(subprocess.check_output(['git', 'show',
        baseline + ':drivers/gpu/drm/asahi/g17p_compute.rs'], cwd=linux))
    harness = tmp / 'main.rs'
    harness.write_text((tests / 'scheduled_work.rs').read_text()
        .replace('"LEGACY_COMPUTE"', json.dumps(str(legacy)))
        .replace('"../g17p_compute.rs"', json.dumps(str(tests.parent / 'g17p_compute.rs'))))
    binary = tmp / 'gate'
    subprocess.run(['rustc', '--edition=2021', '-Dwarnings', str(harness), *flags,
                    '-o', str(binary)], check=True)
    actual = subprocess.check_output([str(binary)], input=b''.join(c[1] for c in cases))
    neo_bytes = 256 * (0xc0 + 0x200 + 0x200 + 0x100)
    neo = actual[:neo_bytes]
    actual = actual[neo_bytes:]
    at = 0
    for name, _, expected in cases:
        got = actual[at:at + len(expected)]
        assert got == expected, (name, [hex(i) for i, (a,b) in enumerate(zip(got, expected)) if a != b][:16])
        at += len(expected)
    assert at == len(actual)
    args.evidence.mkdir(parents=True, exist_ok=False)
    summary = dict(baseline=baseline, Neo_whole_objects=256 * 4,
        Neo_identical_bytes=neo_bytes, Max_whole_objects=len(cases),
        source_ComputeWork_prepare_cases=64, independent_native_DATA_objects=len(native),
        native_DATA_sha256=native, Neo_sha256=hashlib.sha256(neo).hexdigest(),
        Max_sha256=hashlib.sha256(actual).hexdigest(), GPU_execution_tested=False,
        source_sha256={str(f):hashlib.sha256(f.read_bytes()).hexdigest() for f in [
            tests.parent / 'g17p_compute.rs', tests.parent / 'scheduled_work.rs',
            args.m1n1 / 'proxyclient/m1n1/g16c/compute.py',
            args.m1n1 / 'proxyclient/m1n1/g16c/work.py']})
    (args.evidence / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print(f'PASS: {len(cases)} complete Max objects / 4 native DATA objects / 64 actual Work.prepare cases; '
      '1024 complete Neo objects and reuse images identical to tag')
