#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Export own Mesa private-array inputs verbatim, with an independent CPU oracle."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import sys

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1', type=Path)
p.add_argument('capture', type=Path)
p.add_argument('output', type=Path)
p.add_argument('--words', type=int, choices=(256, 4096), required=True)
p.add_argument('--pressure-threads', type=int, choices=(256, 16384))
p.add_argument('--oracle', type=Path, help='independent CPU-only XOR-bin oracle for pressure mode')
a = p.parse_args()
sys.path.insert(0, str(a.m1n1 / 'proxyclient'))
from m1n1.g16c import uapi
from m1n1.g16c.memory import GPU_SHARED_RW

m = json.loads((a.capture / 'manifest.json').read_text())
assert m['offline_only'] and not m['gpu_execution'] and m['apple_binary_pages'] == 0
command = (a.capture / 'cmdbuf.bin').read_bytes()
assert hashlib.sha256(command).hexdigest() == m['cmdbuf_sha256']
c, = uapi.parse_command_buffer(command)
assert c.header.cmd_type == 1 and len(command) == 72
objects, images = [], {}
for o in m['objects']:
    data = (a.capture / o['filename']).read_bytes()
    assert len(data) == o['size'] and hashlib.sha256(data).hexdigest() == o['sha256']
    images[o['handle']] = data
    objects.append((o['handle'], o['flags'], len(data)))
output, = [h for h, b in images.items() if b[:128] == b'\xa5' * 128]
assert len(images[output]) == 16384
binds = [uapi.drm_asahi_gem_bind_op(flags=2 | (4 if r['attributes'] == GPU_SHARED_RW else 0),
    handle=r['handle'], offset=r['offset'], range=r['size'], addr=r['address']) for r in m['bindings']]

def rotate(v, n):
    return ((v << n) | (v >> (32 - n))) & 0xffffffff

def oracle(lane):
    value, scratch = 0x5a110000 ^ lane ^ 9, []
    for j in range(a.words):
        value = (rotate(value, 7) + 0x9e3779b9 + j) & 0xffffffff
        scratch.append(value ^ (((j + 1) * 0x045d9f3b) & 0xffffffff))
    for step in range(a.words):
        index = (step * 13 + 9 + lane) & (a.words - 1)
        scratch[index] = (rotate(scratch[index] ^ scratch[(index + 17) & (a.words - 1)], 5) + step + 9) & 0xffffffff
    value = 0x31415926 ^ lane
    for j in range(a.words):
        value = rotate(value, 3) ^ scratch[(j * 29 + 9) & (a.words - 1)]
    return value

if a.pressure_threads:
    assert a.words == 4096 and a.oracle
    independent = json.loads(a.oracle.read_text())
    assert independent['pass_'] and independent['threads'] == a.pressure_threads
    assert len(independent['bins']) == 16
    assert images[output][128:192] == bytes(64)
    expected = images[output][:128] + struct.pack('<16I', *independent['bins']) + images[output][192:]
else:
    assert a.oracle is None
    expected = struct.pack('<16I', *(oracle(i) for i in range(16))) + images[output][64:]
a.output.mkdir(parents=True, exist_ok=False)
for h, data in images.items():
    (a.output / f'gem-{h}.bin').write_bytes(data)
(a.output / f'expected-gem-{output}.bin').write_bytes(expected)
header = struct.pack('<8I', 0x39504347, 2 if a.pressure_threads else 1, len(objects), len(binds), len(command), output, a.words, a.pressure_threads or 9)
(a.output / 'inputs.bin').write_bytes(header + b''.join(struct.pack('<IIQ', *o) for o in objects)
    + b''.join(op.to_bytes() for op in binds) + command)
(a.output / 'provenance.json').write_text(json.dumps(dict(own_Mesa_inputs=True,
    source_capture=str(a.capture), source_manifest_sha256=hashlib.sha256((a.capture/'manifest.json').read_bytes()).hexdigest(),
    shader_and_all_GEMs_verbatim=True, command_verbatim=True, Apple_pages=0,
    independent_CPU_oracle=True, private_words=a.words, output_words=16, guard_bytes=16320,
    pressure_threads=a.pressure_threads, independent_oracle=str(a.oracle) if a.oracle else None,
    independent_oracle_sha256=hashlib.sha256(a.oracle.read_bytes()).hexdigest() if a.oracle else None,
    files={f.name: hashlib.sha256(f.read_bytes()).hexdigest() for f in a.output.iterdir()}), indent=2) + '\n')
print(f'PASS: verbatim own-Mesa private{a.words}; independent full output and guards')
