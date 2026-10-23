#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Export qualified source compute->indirect render without Apple pages.

All shader/launch/command bytes are preserved. Complete argument, one-pixel
color and counting-query expectations are derived from source workload inputs.
"""
import argparse,hashlib,json,struct,sys
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('bootloader',type=Path);p.add_argument('source',type=Path);p.add_argument('output',type=Path)
a=p.parse_args();sys.path.insert(0,str(a.bootloader/'proxyclient'))
from m1n1.g16c import uapi
from m1n1.g16c.memory import GPU_SHARED_RO,GPU_SHARED_RW
m=json.loads((a.source/'manifest.json').read_text());description=m['source_indirect_mixed']
assert m['offline_only'] and m['offline_pair'] and not m['gpu_execution'] and m['apple_binary_pages']==0
instances=description['instances'];assert instances in (2,200000) and not description['known_argument_results_uploaded']
raw=(a.source/'cmdbuf.bin').read_bytes();assert hashlib.sha256(raw).hexdigest()==m['cmdbuf_sha256']
compute,render=uapi.parse_command_buffer(raw)
assert [compute.header.cmd_type,render.header.cmd_type]==[1,0] and render.header.cdm_barrier==1
assert (render.payload.width_px,render.payload.height_px,render.payload.layers,render.payload.samples)==(1,1,1,1)
objects={o['handle']:dict(o)for o in m['objects']};assert len(objects)==30 and max(objects)<128
images={}
for h,o in objects.items():
    image=(a.source/o['filename']).read_bytes()
    assert len(image)==o['size'] and hashlib.sha256(image).hexdigest()==o['sha256'];images[h]=image
assert len(images[1])==0x800000 and images[1][0x4000:]==bytes(0x7fc000)
images[1]=images[1][:0x4000];objects[1]['size']=0x4000
rows=[]
for b in m['bindings']:
    if b['handle']==1 and b['offset']>=0x4000:continue
    b=dict(b)
    if b['handle']==1:b['size']=min(b['size'],0x4000-b['offset'])
    assert b['offset']+b['size']<=objects[b['handle']]['size'];rows.append(b)
ops=[]
for b in sorted(rows,key=lambda b:b['address']):
    assert b['attributes'] in (GPU_SHARED_RO,GPU_SHARED_RW)
    op=uapi.drm_asahi_gem_bind_op(flags=2|(4 if b['attributes']==GPU_SHARED_RW else 0),handle=b['handle'],offset=b['offset'],range=b['size'],addr=b['address'])
    if ops and (ops[-1].handle,ops[-1].flags,ops[-1].addr+ops[-1].range,ops[-1].offset+ops[-1].range)==(op.handle,op.flags,op.addr,op.offset):ops[-1].range+=op.range
    else:ops.append(op)
expanded=lambda records:sorted((b['handle'],b['attributes'],b['address']+i,b['offset']+i)for b in records for i in range(0,b['size'],0x4000))
assert expanded(rows)==expanded([dict(handle=o.handle,attributes=GPU_SHARED_RW if o.flags&4 else GPU_SHARED_RO,address=o.addr,offset=o.offset,size=o.range)for o in ops])
expected={}
for out in description['outputs']:
    h=out['handle'];image=bytearray(images[h]);n=out['bytes']
    assert out['offset']==0
    value={'indirect-arguments':struct.pack('<4I',3,instances,0,0),'color':bytes.fromhex('ff0000ff'),
           'counting-query':struct.pack('<Q',instances)}[out['name']]
    assert len(value)==n and image[:n]==(bytes(8)if out['name']=='counting-query'else b'\xa5'*n)
    image[:n]=value;expected[h]=bytes(image)
    original=(a.source/out['expected_file']).read_bytes()
    assert hashlib.sha256(original).hexdigest()==out['expected_sha256'] and expected[h]==original
assert set(expected)=={10,12,26}
a.output.mkdir(parents=True,exist_ok=False)
header=struct.pack('<8I',0x3242434d,2,len(objects),len(ops),len(raw),instances,0,0)
(a.output/'inputs.bin').write_bytes(header+b''.join(struct.pack('<IIQ',h,o['flags'],o['size'])for h,o in objects.items())+b''.join(o.to_bytes()for o in ops)+raw)
for h,image in images.items():(a.output/f'gem-{h}.bin').write_bytes(image)
for h,image in expected.items():(a.output/f'expected-gem-{h}.bin').write_bytes(image)
(a.output/'provenance.json').write_text(json.dumps(dict(source=str(a.source),instances=instances,
    source_manifest_sha256=hashlib.sha256((a.source/'manifest.json').read_bytes()).hexdigest(),
    commands_exact=True,shader_launch_bytes_preserved=True,only_zero_shader_aperture_tail_omitted=True,
    page_equivalent_coalesced_bindings=True,apple_binary_pages=0,known_argument_results_uploaded=False,
    expected_CPU_source_inputs=True,complete_output_bytes=sum(map(len,expected.values())),guard_bytes=294884,
    sha256={f.name:hashlib.sha256(f.read_bytes()).hexdigest()for f in a.output.iterdir()}),indent=2)+'\n')
print('PASS source-only native pressure fixture',instances,len(ops),'bindings; three independent complete CPU oracles')
