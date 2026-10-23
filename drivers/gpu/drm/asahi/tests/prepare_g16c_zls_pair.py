#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Own-Mesa ZLS store/load pair, with CPU byte oracles and no shader changes."""
import argparse,ctypes,hashlib,json,struct,subprocess,sys
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('bootloader',type=Path)
p.add_argument('source',type=Path)
p.add_argument('output',type=Path)
p.add_argument('--kind',choices=['depth','stencil'],required=True)
p.add_argument('--no-load',action='store_true')
p.add_argument('--compressed',action='store_true',help='source-qualified opaque main/metadata regions plus exact color/guards')
a=p.parse_args()
tests=Path(__file__).resolve().parent
subprocess.run([sys.executable,str(tests/'prepare_g16c_mesa_render_fixture.py'),str(a.bootloader),str(a.source),str(a.output),
                '--oracle=layered-'+a.kind+'32-final','--flags=12'],check=True)
sys.path.insert(0,str(a.bootloader/'proxyclient'))
from m1n1.g16c import uapi
blob=bytearray((a.output/'inputs.bin').read_bytes())
h=list(struct.unpack_from('<8I',blob));h[1]=3;struct.pack_into('<8I',blob,0,*h)
off=32+h[2]*16
ops=[uapi.drm_asahi_gem_bind_op.from_bytes(blob[off+i*32:off+(i+1)*32])for i in range(h[3])]
command_at=off+h[3]*32
raw=bytes(blob[command_at:]);command,=uapi.parse_command_buffer(raw);c=command.payload
target=c.depth if a.kind=='depth' else c.stencil
zls=next(op for op in ops if op.addr<=target.base<op.addr+op.range)
assert target.base==zls.addr and zls.offset==0 and target.stride==1
assert (c.width_px,c.height_px,c.layers,c.samples)==(32,32,2,1)
vdm=next(op for op in ops if op.addr<=c.vdm_ctrl_stream_base<op.addr+op.range)
vdm_file=a.output/f'gem-{vdm.handle}.bin';stream=vdm_file.read_bytes();start=vdm.offset+c.vdm_ctrl_stream_base-vdm.addr
assert struct.unpack_from('<I',stream,start)[0]==0x20000013
groups=(4,120,236)if a.kind=='depth'else(4,120,236,352)
for at in groups:
    assert struct.unpack_from('<I',stream,start+at)[0]==0x4000002e
    assert struct.unpack_from('<I',stream,start+at+100)[0]==0x61c40600
end=groups[-1]+116
assert struct.unpack_from('<I',stream,start+end)[0]==0xc0000000
selections=((120,116),(236,116))if a.kind=='depth'else((4,232),(236,232))
streams=[]
for begin,size in selections:
    selected=stream[:start+4]+stream[start+begin:start+begin+size]+struct.pack('<I',0xc0000000)+bytes(end-4-size)+stream[start+end+4:]
    assert len(selected)==len(stream)
    streams.append(selected)
vdm_file.write_bytes(streams[0]);(a.output/f'phase1-input-gem-{vdm.handle}.bin').write_bytes(streams[1])
source_bg_values=c.isp_bgobjvals
old_zls_base=target.base
if a.compressed:
    assert not any(op.handle in (125,126) for op in ops)
    for handle,address,length in ((125,0x6000050000,65536),(126,0x6000070000,16384)):
        assert all(address+length<=op.addr or address>=op.addr+op.range for op in ops)
        ops.append(uapi.drm_asahi_gem_bind_op(flags=6,handle=handle,offset=0,range=length,addr=address))
    target.base=0x6000050000;target.stride=0x4001
    target.comp_base=0x6000070000;target.comp_stride=0x4000
def command_image(load):
    c.zls_ctrl=(0x80000 if a.kind=='depth'else 0x40000)|((1<<(15 if a.kind=='depth'else 14))if load else 0)|((0x44 if a.kind=='depth'else 0x110)if a.compressed else 0)
    c.isp_bgobjdepth=0x3f800000;c.isp_bgobjvals=source_bg_values
    parts=[];cursor=0
    while cursor<len(raw):
        head=uapi.drm_asahi_cmd_header.from_bytes(raw[cursor:cursor+8]);cursor+=8;payload=raw[cursor:cursor+head.size]
        if head.cmd_type==uapi.DRM_ASAHI_CMD_RENDER:payload=c.to_bytes()
        elif a.compressed and head.cmd_type==uapi.DRM_ASAHI_SET_FRAGMENT_ATTACHMENTS:
            attachments=[]
            for at in range(0,len(payload),24):
                attachment=uapi.drm_asahi_attachment.from_bytes(payload[at:at+24])
                if attachment.pointer==old_zls_base:attachment.pointer=target.base;attachment.size=65536
                attachments.append(attachment.to_bytes())
            attachments.append(uapi.drm_asahi_attachment(pointer=target.comp_base,size=16384,flags=0).to_bytes())
            payload=b''.join(attachments)
        cursor+=head.size
        head.size=len(payload);parts.append(head.to_bytes()+payload)
    assert cursor==len(raw)
    return b''.join(parts)
first_command=command_image(False)
if a.compressed:
    objects=bytes(blob[32:off])+struct.pack('<IIQIIQ',125,3,65536,126,3,16384)
    h[1]=4;h[2]+=2;h[3]+=2;h[4]=len(first_command)
    blob=bytearray(struct.pack('<8I',*h)+objects+b''.join(op.to_bytes()for op in ops)+first_command)
else:
    blob[command_at:]=first_command
(a.output/'inputs.bin').write_bytes(blob)
(a.output/'phase1-command.bin').write_bytes(command_image(not a.no_load))
color_seed=(a.output/f'gem-{h[5]}.bin').read_bytes();zls_seed=(a.output/f'gem-{zls.handle}.bin').read_bytes()
assert len(color_seed)==len(zls_seed)==32768
def expected(phase):
    color=bytearray(color_seed);plane=bytearray(zls_seed)
    if not phase:
        colors=['ff0000ff','0000ffff'];values=[0.25,0.75]if a.kind=='depth'else[7,9]
    elif not a.no_load:
        colors=['ff0000ff','00ff00ff'];values=[0.25,0.5]if a.kind=='depth'else[8,10]
    elif a.kind=='depth':
        colors=['00ff00ff','00ff00ff'];values=[0.5,0.5]
    else:
        colors=['ff0000ff','0000ffff'];values=[0,0]
    for layer in range(2):
        color[layer*16384:layer*16384+4096]=bytes.fromhex(colors[layer])*1024
        value=struct.pack('<f',values[layer])if a.kind=='depth'else bytes([values[layer]])
        plane[layer*16384:layer*16384+1024*len(value)]=value*1024
    return bytes(color),bytes(plane),dict(colors=colors,values=values)
phases=[]
for phase in range(2):
    color,plane,oracle=expected(phase);phases.append(oracle)
    prefix='expected'if not phase else'phase1-expected'
    (a.output/f'{prefix}-gem-{h[5]}.bin').write_bytes(color)
    if not a.compressed:(a.output/f'{prefix}-gem-{zls.handle}.bin').write_bytes(plane)
if a.compressed:
    (a.output/f'expected-gem-{zls.handle}.bin').unlink()
    main=b'\xa5'*65536
    meta=bytes(128)+b'\xa5'*128+bytes(128)+b'\xa5'*(16384-384)
    for handle,seed,ranges in ((125,main,[(0,4096 if a.kind=='depth'else 1024),(32768,4096 if a.kind=='depth'else 1024)]),(126,meta,[(0,128),(256,128)])):
        mask=bytearray(len(seed))
        for start,length in ranges:mask[start:start+length]=b'\1'*length
        (a.output/f'gem-{handle}.bin').write_bytes(seed)
        (a.output/f'opaque-writes-gem-{handle}.bin').write_bytes(mask)
        for prefix in ('expected','phase1-expected'):(a.output/f'{prefix}-gem-{handle}.bin').write_bytes(seed)
proof=json.loads((a.output/'provenance.json').read_text())
proof.update(phase_pair=dict(kind=a.kind,load_second=not a.no_load,phases=phases,VDM_handle=vdm.handle,
                            shader_and_launch_unchanged=True,only_complete_source_VDM_state_draw_groups_selected=True,
                            prior_GPU_color_and_ZLS_preserved_for_second_submission=True),
             source_isp_bgobjvals_preserved=hex(source_bg_values),
             changed_UAPI_fields=['flags','zls_ctrl','isp_bgobjdepth'],
             sha256={p.name:hashlib.sha256(p.read_bytes()).hexdigest()for p in a.output.iterdir()if p.name!='provenance.json'})
if a.compressed:
    proof['CPU_oracle'].pop('ZLS_expected')
    proof.update(mutable_BO_ids=[h[5],125,126],compressed=True,
                 changed_UAPI_fields=['flags','zls_ctrl','isp_bgobjdepth',a.kind+'.base',a.kind+'.stride',a.kind+'.comp_base',a.kind+'.comp_stride','fragment_attachments'],
                 compression_validation=dict(main_layer_stride=32768,metadata_layer_stride=256,
                     main_payload_bytes_per_layer=4096 if a.kind=='depth'else 1024,metadata_payload_bytes_per_layer=128,
                     exact_color_and_guards=True,opaque_payload_not_a_CPU_byte_oracle=True,
                     required_payload_change_from_source_seed=True,subsequent_consumer_color_CPU_oracle=True))
(a.output/'provenance.json').write_text(json.dumps(proof,indent=2)+'\n')
print('Exported',a.kind,'store /',('no-load control'if a.no_load else'load'),'pair;',('exact CPU color/guards and opaque compressed regions'if a.compressed else'complete CPU color/ZLS oracles'))
