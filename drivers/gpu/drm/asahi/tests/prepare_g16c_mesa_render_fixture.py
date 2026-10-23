#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Export one own-Mesa render directly from its hash-verified offline producer.

Shader/launch bytes are unchanged. Only an entirely zero generic shader-aperture
tail is omitted; output seed and explicit UAPI render flags are caller DATA.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import sys

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('bootloader',type=Path)
p.add_argument('source',type=Path)
p.add_argument('output',type=Path)
p.add_argument('--flags',type=lambda x:int(x,0),default=0)
p.add_argument('--output-bytes',type=int,default=4)
p.add_argument('--oracle',choices=['rgba8-red','rgba8-three-layer-colors','layered-depth32-final','layered-stencil32-final','rgba8-empty-tiles'])
p.add_argument('--control-triplet',action='store_true',help='same-queue disabled/enabled/disabled empty-tile controls')
p.add_argument('--utile',type=int,nargs=2)
p.add_argument('--ppp-ctrl',type=lambda x:int(x,0))
a=p.parse_args()
assert not a.control_triplet or (a.oracle=='rgba8-empty-tiles' and a.flags==12)
sys.path.insert(0,str(a.bootloader/'proxyclient'))
from m1n1.g16c import uapi
from m1n1.g16c.memory import GPU_SHARED_RO,GPU_SHARED_RW
m=json.loads((a.source/'manifest.json').read_text())
assert m['offline_only'] and not m['gpu_execution'] and m['apple_binary_pages']==0
raw=(a.source/'cmdbuf.bin').read_bytes()
assert hashlib.sha256(raw).hexdigest()==m['cmdbuf_sha256']
command,=uapi.parse_command_buffer(raw)
assert command.header.cmd_type==uapi.DRM_ASAHI_CMD_RENDER
assert a.flags & ~uapi.DRM_ASAHI_RENDER_FLAGS==0
objects={o['handle']:dict(o) for o in m['objects']}
assert len(objects)==len(m['objects']) and max(objects)<128
images={}
for handle,o in objects.items():
    data=(a.source/o['filename']).read_bytes()
    assert len(data)==o['size'] and hashlib.sha256(data).hexdigest()==o['sha256']
    images[handle]=data
assert len(images[1])==0x800000 and images[1][0x4000:]==bytes(0x800000-0x4000)
images[1]=images[1][:0x4000]
objects[1]['size']=0x4000
bindings=[]
for b in m['bindings']:
    if b['handle']==1 and b['offset']>=0x4000:
        continue
    b=dict(b)
    if b['handle']==1:
        b['size']=min(b['size'],0x4000-b['offset'])
    assert b['offset']+b['size']<=objects[b['handle']]['size']
    bindings.append(b)
attachment=command.fragment_attachments[0].pointer
output_binding=next(b for b in bindings if b['address']<=attachment<b['address']+b['size'])
output=output_binding['handle']
output_offset=output_binding['offset']+attachment-output_binding['address']
assert a.output_bytes>0 and a.output_bytes%4==0
assert output_offset+a.output_bytes<=len(images[output])
images[output]=b'\xa5'*len(images[output])
cmd=command.payload
cmd.flags=a.flags
changed_fields=['flags']
if a.utile:
    assert tuple(a.utile) in ((32,32),(32,16),(16,16))
    cmd.utile_width_px,cmd.utile_height_px=a.utile
    changed_fields+=['utile_width_px','utile_height_px']
if a.ppp_ctrl is not None:
    assert 0<=a.ppp_ctrl<=0xffffffff
    cmd.ppp_ctrl=a.ppp_ctrl
    changed_fields+=['ppp_ctrl']
oracle=None
expected_images={}
if a.oracle:
    from g16c_render_oracle import rgba8_expected
    color=('red' if a.oracle in ('rgba8-red','rgba8-empty-tiles') else 'three-layer-colors' if a.oracle=='rgba8-three-layer-colors' else 'two-layer-red-green')
    expected,points,oracle=rgba8_expected(images[output],cmd.width_px,cmd.height_px,cmd.layers,cmd.samples,
                                        color,output_offset)
    if a.oracle=='rgba8-empty-tiles':
        source=m['source_empty_tile_fixture']
        assert source['wholly_clipped_triangle'] and source['background_from_own_fragment']
        assert source['background_color']==source['foreground_color']=='red'
        assert (cmd.width_px,cmd.height_px,cmd.layers,cmd.samples,cmd.ppp_ctrl)==(33,33,1,1,0x203)
        assert cmd.flags in (12,14)
        positive_expected=expected
        expected=expected if cmd.flags&2 else images[output]
        oracle.update(wholly_clipped_source=True,process_empty_tiles=bool(cmd.flags&2),
                      CPU_expected_unchanged_seed=not bool(cmd.flags&2),positive_pixels=1089,guard_bytes=12028)
    expected_images[output]=expected
    a.output_bytes=len(points)*4
    if a.oracle.startswith('layered-'):
        assert (cmd.width_px,cmd.height_px,cmd.layers,cmd.samples)==(32,32,2,1)
        depth=a.oracle=='layered-depth32-final'
        target=cmd.depth if depth else cmd.stencil
        assert target.base and target.stride==1 and not target.comp_base and not target.comp_stride
        assert cmd.zls_ctrl==(0x88000 if depth else 0x44000)
        binding=next(b for b in bindings if b['address']<=target.base<b['address']+b['size'])
        handle=binding['handle'];at=binding['offset']+target.base-binding['address']
        assert at==0 and len(images[handle])==32768 and handle!=output
        images[handle]=b'\xa5'*len(images[handle])
        wanted=bytearray(images[handle])
        for layer in range(2):
            value=struct.pack('<f',0.25 if layer==0 else 0.5) if depth else bytes([8 if layer==0 else 10])
            wanted[layer*16384:layer*16384+1024*len(value)]=value*1024
        expected_images[handle]=bytes(wanted)
        oracle['ZLS_expected']=dict(kind='float32 depth' if depth else 'stencil8',handle=handle,
                                    values=[0.25,0.5] if depth else [8,10],layer_stride=16384,
                                    whole_bytes=len(wanted),guard_bytes=len(wanted)-2048*len(value))
cursor=0
updated=bytearray(raw)
while cursor<len(raw):
    h=uapi.drm_asahi_cmd_header.from_bytes(raw[cursor:cursor+8])
    cursor+=8
    if h.cmd_type==uapi.DRM_ASAHI_CMD_RENDER:
        updated[cursor:cursor+h.size]=cmd.to_bytes()
    cursor+=h.size
assert cursor==len(raw)
ops=[]
for b in sorted(bindings,key=lambda b:b['address']):
    assert b['attributes'] in (GPU_SHARED_RO,GPU_SHARED_RW)
    flags=uapi.DRM_ASAHI_BIND_READ|(uapi.DRM_ASAHI_BIND_WRITE if b['attributes']==GPU_SHARED_RW else 0)
    op=uapi.drm_asahi_gem_bind_op(flags=flags,handle=b['handle'],offset=b['offset'],range=b['size'],addr=b['address'])
    if ops and (ops[-1].handle,ops[-1].flags,ops[-1].addr+ops[-1].range,ops[-1].offset+ops[-1].range)==(op.handle,op.flags,op.addr,op.offset):
        ops[-1].range+=op.range
    else:
        ops.append(op)
header=struct.pack('<8I',0x3242434d,5 if a.control_triplet else 2 if a.oracle else 1,len(objects),len(ops),len(updated),output,output_offset,a.output_bytes)
inputs=header+b''.join(struct.pack('<IIQ',h,o['flags'],o['size'])for h,o in objects.items())+b''.join(op.to_bytes()for op in ops)+updated
a.output.mkdir(parents=True,exist_ok=False)
for h,data in images.items():
    (a.output/f'gem-{h}.bin').write_bytes(data)
(a.output/'inputs.bin').write_bytes(inputs)
for handle,expected in expected_images.items():
    (a.output/f'expected-gem-{handle}.bin').write_bytes(expected)
if a.control_triplet:
    for phase,flags in ((1,14),(2,12)):
        data=bytearray(updated);cursor=0
        while cursor<len(data):
            h=uapi.drm_asahi_cmd_header.from_bytes(data[cursor:cursor+8]);cursor+=8
            if h.cmd_type==uapi.DRM_ASAHI_CMD_RENDER:
                struct.pack_into('<I',data,cursor+uapi.drm_asahi_cmd_render.flags.offset,flags)
            cursor+=h.size
        parsed,=uapi.parse_command_buffer(bytes(data));assert parsed.payload.flags==flags
        (a.output/f'phase{phase}-command.bin').write_bytes(data)
        (a.output/f'phase{phase}-expected-gem-{output}.bin').write_bytes(positive_expected if flags&2 else images[output])
provenance=dict(source=str(a.source),source_manifest_sha256=hashlib.sha256((a.source/'manifest.json').read_bytes()).hexdigest(),
                Apple_client_pages=0,all_shader_and_launch_bytes_unchanged=True,removed_shader_tail_entirely_zero=True,
                only_other_changes=['declared output BO seeds','requested UAPI fields','page-equivalent binding coalescing'],
                mutable_BO_ids=sorted({output,*expected_images}),
                changed_UAPI_fields=changed_fields,CPU_oracle=oracle,
                shape=[cmd.width_px,cmd.height_px,cmd.layers,cmd.samples,cmd.utile_width_px,cmd.utile_height_px],
                flags=cmd.flags,ppp_ctrl=hex(cmd.ppp_ctrl),output_bytes=a.output_bytes,
                control_triplet_flags=[12,14,12] if a.control_triplet else None,
                control_triplet_resets_seed_each_phase=a.control_triplet,
                source_object_sha256={str(h):o['sha256']for h,o in objects.items()},
                sha256={p.name:hashlib.sha256(p.read_bytes()).hexdigest()for p in a.output.iterdir()})
(a.output/'provenance.json').write_text(json.dumps(provenance,indent=2)+'\n')
print('Exported source-only render',provenance['shape'],'flags',cmd.flags,'output',a.output_bytes,'shader/launch unchanged')
