#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Export qualified source-owned sampler GEMs for the native DRM test.

No instruction decoding or rewriting; the shader image stays byte-identical.
The native table is ordinary userspace DATA populated by g16c_drm_sampler.c.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('source', type=Path)
p.add_argument('output', type=Path)
a = p.parse_args()
m = json.loads((a.source / 'manifest.json').read_text())
assert m['source_only'] and m['opaque_Apple_client_pages'] == 0
assert m['opaque_client_code_bytes'] == 0 and m['source_whole_shader_rebuilt']
a.output.mkdir(parents=True, exist_ok=False)
objects = []
for o in m['objects']:
    data = (a.source / o['filename']).read_bytes()
    assert len(data) == o['size']
    assert hashlib.sha256(data).hexdigest() == o['sha256']
    assert o['filename'] == f"gem-{o['handle']}.bin"
    (a.output / o['filename']).write_bytes(data)
    objects.append((o['handle'], o['flags'], o['size'], o['address']))
table, capacity = 0x6001000000, 16 << 20
assert len(objects) < 127 and all(row[0] != 124 for row in objects)
assert all(table + capacity <= va or va + size <= table
           for _, _, size, va in objects)
objects.append((124, 3, capacity, table))
(a.output / 'gem-124.bin').write_bytes(bytes(capacity))
header = struct.pack('<8I', 0x53434741, 1, len(objects), m['output_handle'],
                     m['output_offset'], 0, 0, 0)
config = struct.pack('<9Q', m['root_sampler_slot'], m['initial_sampler_table'],
                     table, m['source_index_GPU_VA'], m['source_arguments_GPU_VA'],
                     m['cdm_single'], m['heap_A'], m['heap_B'], capacity)
image = header + config + b''.join(struct.pack('<IIQQ', *row) for row in objects)
(a.output / 'inputs.bin').write_bytes(image)
provenance = dict(source_fixture=str(a.source),
    source_manifest_sha256=hashlib.sha256((a.source / 'manifest.json').read_bytes()).hexdigest(),
    source_only=True, Apple_client_pages=0, firmware_objects=0,
    source_shader_unchanged=True, objects=len(objects),
    new_caller_owned_native_table=dict(address=table, size=capacity),
    inputs_sha256=hashlib.sha256(image).hexdigest())
(a.output / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
print(f'Exported {len(objects)} source-owned GEMs; shader unchanged; caller table DATA only')
