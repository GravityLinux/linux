#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Export qualified own-source vertex sampling with caller-built native tables.

Only resource DATA and UAPI fields change. No shader/launch decoding or rewriting
and no captured Apple pages. This is not automatic Mesa root-table integration.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import sys

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('bootloader', type=Path)
p.add_argument('source', type=Path)
p.add_argument('output', type=Path)
p.add_argument('--count', type=int, default=515)
p.add_argument('--heap', choices=['A', 'B'], default='A')
p.add_argument('--red-texture', action='store_true')
p.add_argument('--empty-tiles', action='store_true')
p.add_argument('--fragment-barrier', action='store_true')
a = p.parse_args()
sys.path.insert(0, str(a.bootloader / 'proxyclient'))
from m1n1.g16c import uapi
from m1n1.g16c.memory import GPU_SHARED_RW, GPU_SHARED_RO

assert 515 <= a.count <= 65535  # Render UAPI count is u16; firmware count+1 is u32.
m = json.loads((a.source / 'source-input-manifest.json').read_text())
r = json.loads((a.source / 'result.json').read_text())
assert r['pass'] and r['completed'] and r['quiescent']
assert r['own_mesa_resources'] and r['source_GPR_sampling']
assert r['source_weights'] == [1.0, 0.0]
assert m['opaque_Apple_client_pages'] == r['opaque_apple_client_pages'] == 0
assert r['source_vertex_main_bytes'] == 584 and r['source_vertex_preamble_bytes'] == 306
objects = {o['handle']: dict(o) for o in m['objects']}
images = {}
for handle, o in objects.items():
    data = (a.source / f'input-gem-{handle}.bin').read_bytes()
    assert len(data) == o['size']
    assert hashlib.sha256(data).hexdigest() == m['actual_source_input_sha256'][str(handle)]
    images[handle] = bytearray(data)
bindings = [dict(b) for b in m['bindings']]
def address(va, size):
    for b in bindings:
        if b['address'] <= va and va + size <= b['address'] + b['size']:
            offset = b['offset'] + va - b['address']
            assert offset + size <= len(images[b['handle']])
            return images[b['handle']], offset
    raise ValueError(f'unmapped source DATA {va:#x}+{size:#x}')

raw = (a.source / 'command-uapi-0.bin').read_bytes()
command, = uapi.parse_command_buffer(raw)
assert command.header.cmd_type == uapi.DRM_ASAHI_CMD_RENDER
cmd = command.payload
output = next(b['handle'] for b in bindings if b['address'] == command.fragment_attachments[0].pointer)
assert len(images[output]) == 16384
images[output][:] = b'\xa5' * len(images[output])
nearest = bytes.fromhex('00000e0088070000')
linear = bytes.fromhex('00008e0288070000')
logical = bytearray(a.count * 8)
for index in range(a.count):
    record = nearest if index == 513 else linear
    if a.heap == 'B':
        record = linear if index == 513 else nearest
    logical[index * 8:index * 8 + 8] = record
# Replace only the old caller logical-heap object, preserving its address.
assert objects[121]['flags'] == 0
heap_va = 0x6000000000
heap_size = (len(logical) + 0x3fff) & -0x4000
objects[121]['size'] = heap_size
images[121] = logical + bytes(heap_size - len(logical))
bindings = [b for b in bindings if b['handle'] != 121]
bindings.append(dict(handle=121, address=heap_va, size=heap_size, offset=0, attributes=GPU_SHARED_RO))

roots = json.loads((a.source / 'submit-0.json').read_text())['sampler_binding']['roots']
assert [root['stage'] for root in roots] == ['vertex', 'fragment']
native_va = 0x6001000000
stride = ((28 + a.count) * 8 + 0x3fff) & -0x4000
assert heap_va + heap_size <= native_va and 124 not in objects
assert all(native_va + stride * len(roots) <= b['address'] or b['address'] + b['size'] <= native_va for b in bindings)
native = bytearray(stride * len(roots))
for index, root in enumerate(roots):
    if root['sampler_table']:
        for inline in range(28):
            try:
                data, offset = address(root['sampler_table'] + inline * 32, 8)
            except ValueError:
                break
            native[index * stride + inline * 8:index * stride + inline * 8 + 8] = data[offset:offset + 8]
    native[index * stride + 28 * 8:index * stride + 28 * 8 + len(logical)] = logical
    data, offset = address(root['sampler_slot'], 8)
    struct.pack_into('<Q', data, offset, native_va + index * stride)
objects[124] = dict(handle=124, flags=0, size=len(native))
images[124] = native
bindings.append(dict(handle=124, address=native_va, size=len(native), offset=0, attributes=GPU_SHARED_RO))
if a.red_texture:
    assert images[10][:16] == bytes.fromhex('ff0000ff0000ffff00ff00ffffffffff')
    images[10][:16] = bytes.fromhex('ff0000ff') * 4
cmd.sampler_heap, cmd.sampler_count = heap_va, a.count
if a.empty_tiles:
    cmd.flags |= uapi.DRM_ASAHI_RENDER_PROCESS_EMPTY_TILES
if a.fragment_barrier:
    cmd.flags |= uapi.DRM_ASAHI_RENDER_VDM_BARRIER_FRAGMENT
# Preserve attachment headers and stream bytes; replace only this render DATA.
cursor = 0
updated = bytearray(raw)
while cursor < len(raw):
    h = uapi.drm_asahi_cmd_header.from_bytes(raw[cursor:cursor + 8])
    cursor += 8
    if h.cmd_type == uapi.DRM_ASAHI_CMD_RENDER:
        updated[cursor:cursor + h.size] = cmd.to_bytes()
    cursor += h.size
assert cursor == len(raw)
ops = []
for b in sorted(bindings, key=lambda b:b['address']):
    assert b['attributes'] in (GPU_SHARED_RO, GPU_SHARED_RW)
    flags = uapi.DRM_ASAHI_BIND_READ | (uapi.DRM_ASAHI_BIND_WRITE if b['attributes'] == GPU_SHARED_RW else 0)
    op = uapi.drm_asahi_gem_bind_op(flags=flags, handle=b['handle'], offset=b['offset'], range=b['size'], addr=b['address'])
    if ops and (ops[-1].handle,ops[-1].flags,ops[-1].addr+ops[-1].range,ops[-1].offset+ops[-1].range)==(op.handle,op.flags,op.addr,op.offset):
        ops[-1].range += op.range
    else:
        ops.append(op)
header = struct.pack('<8I', 0x3242434d, 1, len(objects), len(ops), len(updated), output, 0, 4)
inputs = header + b''.join(struct.pack('<IIQ', h, o['flags'], o['size']) for h,o in objects.items()) + b''.join(op.to_bytes() for op in ops) + updated
a.output.mkdir(parents=True, exist_ok=False)
for h, data in images.items():
    (a.output / f'gem-{h}.bin').write_bytes(data)
(a.output / 'inputs.bin').write_bytes(inputs)
pixel = 0xff0000ff if a.red_texture else 0xffffffff if a.heap == 'A' else 0xff808080
provenance = dict(source=str(a.source), source_manifest_sha256=hashlib.sha256((a.source/'source-input-manifest.json').read_bytes()).hexdigest(),
                 own_source_shader_and_launch_unchanged=True, Apple_client_pages=0,
                 caller_native_table_entry_bytes=8, caller_native_table_roots=len(roots),
                 kernel_root_discovery=False, count=a.count, heap=a.heap,
                 pixel=hex(pixel), flags=cmd.flags, ppp_ctrl=hex(cmd.ppp_ctrl),
                 shader_sha256=hashlib.sha256(images[19]).hexdigest(),
                 sha256={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in a.output.iterdir()})
assert provenance['shader_sha256'] == m['actual_source_input_sha256']['19']
(a.output / 'provenance.json').write_text(json.dumps(provenance, indent=2)+'\n')
print(f'Exported caller DATA: {a.count} samplers, {a.heap}, expected {pixel:#x}; shader/launch unchanged')
