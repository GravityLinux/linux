#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Source-owned compressed PBE store, TFU reload, and exact decoded CPU image.

All shader/launch bytes stay unchanged. Only source VDM groups, caller table
DATA, scissor DATA and declared UAPI fields change. Opaque payload is never an
expected byte image; full decoded color/guards and preserved inputs prove use.
"""
import argparse,hashlib,json,struct,subprocess,sys
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('bootloader',type=Path);p.add_argument('source',type=Path);p.add_argument('output',type=Path)
p.add_argument('--alternate-input',action='store_true',help='consume a known green/white uncompressed texture instead')
p.add_argument('--ppp-ctrl',type=lambda value:int(value,0),default=0x202,help='source-qualified PPP control')
a=p.parse_args();tests=Path(__file__).resolve().parent
assert 0<=a.ppp_ctrl<=0xffffffff
subprocess.run([sys.executable,str(tests/'prepare_g16c_zls_pair.py'),str(a.bootloader),str(a.source),str(a.output),'--kind=depth'],check=True)
sys.path.insert(0,str(a.bootloader/'proxyclient'))
from m1n1.g16c import uapi
out=a.output;blob=(out/'inputs.bin').read_bytes();h=list(struct.unpack_from('<8I',blob));off=32+h[2]*16
objects=blob[32:off];ops=[uapi.drm_asahi_gem_bind_op.from_bytes(blob[off+i*32:off+(i+1)*32])for i in range(h[3])]
raw=blob[off+h[3]*32:];command,=uapi.parse_command_buffer(raw);c=command.payload
assert (c.width_px,c.height_px,c.layers,c.samples,c.depth.stride)==(32,32,2,1,1)
old_depth=c.depth.base;color_id=h[5];assert color_id==9 and h[6]==0
images={i: (out/f'gem-{i}.bin').read_bytes()for i in struct.unpack_from('<'+('IIQ'*h[2]),objects)[::3]}
assert not set(images)&{124,125,126,127}
extra=[(125,0x6000050000,65536),(126,0x6000060000,16384),(124,0x6000070000,16384),(127,0x6000080000,32768)]
for handle,address,size in extra:
 assert all(address+size<=op.addr or address>=op.addr+op.range for op in ops)
 objects+=struct.pack('<IIQ',handle,3,size);ops.append(uapi.drm_asahi_gem_bind_op(flags=6,handle=handle,offset=0,range=size,addr=address))
 images[handle]=b'\xa5'*size
images[126]=bytes(256)+b'\xa5'*(16384-256);images[124]=b'\x7f'*256+b'\xa5'*(16384-256)
def binding(address,size):return next(op for op in ops if op.addr<=address and address+size<=op.addr+op.range)
def read(address,size):
 op=binding(address,size);at=op.offset+address-op.addr;return images[op.handle][at:at+size]
patches=[]
def write(address,data):
 op=binding(address,len(data));at=op.offset+address-op.addr;b=bytearray(images[op.handle]);b[at:at+len(data)]=data;images[op.handle]=bytes(b)
 patches.append(dict(handle=op.handle,offset=at,bytes=len(data),GPU_address=address));return op.handle
def literal(d):return(d[1]&127)|((d[4]&30)<<6)|((d[5]&12)<<9)|(d[6]<<13)|((d[7]&15)<<21)|((d[3]&254)<<24)
def table(usc):
 # Only our own Mesa-generated launch literal words, as in the source gate.
 prefix=read((1<<40)+(usc&~7),16);root=literal(prefix[:8])|(literal(prefix[8:])<<32)
 return struct.unpack_from('<Q',read(root,48))[0]
tfu_address=table(c.bg.usc);pbe_address=table(c.eot.usc);tfu_original=read(tfu_address,32);pbe_original=read(pbe_address,32)
assert c.bg.usc==c.partial_bg.usc and c.eot.usc==c.partial_eot.usc
assert not int.from_bytes(tfu_original[:8],'little')&(1<<59)and not int.from_bytes(pbe_original[:8],'little')&(1<<59)
assert tfu_original[16:]==pbe_original[16:]==bytes(16)
vdm=binding(c.vdm_ctrl_stream_base,4);start=vdm.offset+c.vdm_ctrl_stream_base-vdm.addr
original_stream=(a.source/f'gem-{vdm.handle}.bin').read_bytes()
ppp=0x1000000000+struct.unpack_from('<I',original_stream,start+236+32)[0]
scissor_index=struct.unpack('<I',read(ppp+0xf4,4))[0]&0xffff
scissor_address=c.isp_scissor_base+scissor_index*16
assert 0<=scissor_index<4 and scissor_address+16<=c.isp_dbias_base
original_scissor=read(scissor_address,16);assert struct.unpack_from('<2I',original_scissor)==(32,32)
def command_bytes(phase):
 c.flags=14 if phase else 12
 c.ppp_ctrl=a.ppp_ctrl
 c.depth.base=0x6000050000;c.depth.stride=1;c.depth.comp_base=0x6000060000;c.depth.comp_stride=0
 c.isp_bgobjdepth=0x3f800000;c.zls_ctrl=0x88044 if phase else 0x80044
 parts=[];cursor=0
 while cursor<len(raw):
  head=uapi.drm_asahi_cmd_header.from_bytes(raw[cursor:cursor+8]);cursor+=8;payload=raw[cursor:cursor+head.size];cursor+=head.size
  if head.cmd_type==uapi.DRM_ASAHI_CMD_RENDER:payload=c.to_bytes()
  elif head.cmd_type==uapi.DRM_ASAHI_SET_FRAGMENT_ATTACHMENTS:
   attachments=[]
   for at in range(0,len(payload),24):
    item=uapi.drm_asahi_attachment.from_bytes(payload[at:at+24])
    if item.pointer==old_depth:item.pointer=c.depth.base;item.size=65536
    attachments.append(item.to_bytes())
   for _,address,size in extra[1:]:attachments.append(uapi.drm_asahi_attachment(pointer=address,size=size,flags=0).to_bytes())
   attachments.append(uapi.drm_asahi_attachment(pointer=old_depth,size=32768,flags=0).to_bytes())
   payload=b''.join(attachments)
  head.size=len(payload);parts.append(head.to_bytes()+payload)
 return b''.join(parts)
commands=[command_bytes(0),command_bytes(1)]
# BO10 no longer supplies hardware depth; use it as a declared caller texture
# for the independent known-input control. Its source descriptor is RGBA8.
assert binding(old_depth,32768).handle==10
if a.alternate_input:
 control=bytearray(images[10]);control[:4096]=bytes.fromhex('00ff00ff')*1024;control[16384:20480]=bytes.fromhex('ffffffff')*1024
 images[10]=bytes(control);(out/'gem-10.bin').write_bytes(control)
# Phase0 writes compressed color. Phase1 only loads it and stores elsewhere.
phase_changes=[]
for phase in range(2):
 tfu=list(struct.unpack('<4Q',tfu_original));pbe=list(struct.unpack('<4Q',pbe_original))
 if not phase:pbe[0]|=1<<59;pbe[1]|=1<<63;pbe[2]=0x6000070000>>4
 else:
  if a.alternate_input:
   assert binding((tfu[1]&((1<<40)-1))<<4,32768).handle==color_id
   tfu[1]=(tfu[1]&~((1<<40)-1))|(old_depth>>4)
  else:tfu[0]|=1<<59;tfu[1]|=1<<63;tfu[2]=0x6000070000>>4
  pbe[1]=(pbe[1]&~((1<<40)-1))|(0x6000080000>>4)
 changed={write(tfu_address,struct.pack('<4Q',*tfu)),write(pbe_address,struct.pack('<4Q',*pbe))}
 if phase:changed.add(write(scissor_address,bytes(8)+original_scissor[8:]))
 for handle in changed:(out/f'{"phase1-input-"if phase else""}gem-{handle}.bin').write_bytes(images[handle])
 phase_changes.append(sorted(changed))
# Save only initial seeds for new mutable buffers; never upload GPU-written
# compressed inputs again before the consumer.
for handle,_,_ in extra:(out/f'gem-{handle}.bin').write_bytes(images[handle])
for prefix in ('expected','phase1-expected'):(out/f'{prefix}-gem-10.bin').unlink()
seed=b'\xa5'*32768;decoded=bytearray(seed)
colors=['00ff00ff','ffffffff'] if a.alternate_input else ['ff0000ff','0000ffff']
decoded[:4096]=bytes.fromhex(colors[0])*1024;decoded[16384:20480]=bytes.fromhex(colors[1])*1024
(out/'expected-gem-127.bin').write_bytes(seed);(out/'phase1-expected-gem-127.bin').write_bytes(decoded)
for handle,ranges in [(color_id,[(0,4096),(16384,4096)]),(125,[(0,4096),(16384,4096)]),(126,[(0,256)]),(124,[(0,256)])]:
 initial=(out/f'gem-{handle}.bin').read_bytes();mask=bytearray(len(initial))
 for at,size in ranges:mask[at:at+size]=b'\1'*size
 (out/f'opaque-writes-gem-{handle}.bin').write_bytes(mask)
 for prefix in ('expected','phase1-expected'):(out/f'{prefix}-gem-{handle}.bin').write_bytes(initial)
for handle in (color_id,124):(out/f'phase1-preserve-gem-{handle}.bin').write_bytes(b'\1')
h[1]=6;h[2]+=4;h[3]+=4;h[4]=len(commands[0]);h[5]=127;assert len(commands[1])==h[4]
(out/'inputs.bin').write_bytes(struct.pack('<8I',*h)+objects+b''.join(op.to_bytes()for op in ops)+commands[0])
(out/'phase1-command.bin').write_bytes(commands[1])
proof=json.loads((out/'provenance.json').read_text())
proof.pop('CPU_oracle');proof.pop('phase_pair')
proof.update(compressed_color_pair=True,reload_enabled=True,alternate_input=a.alternate_input,shape=[32,32,2,1],mutable_BO_ids=[9,124,125,126,127],
 only_other_changes=['declared output/control texture seeds','complete source VDM groups','source TFU/PBE descriptor and scissor DATA','requested UAPI fields and attachments','zero shader tail removal and page-equivalent bindings'],
 changed_UAPI_fields=['flags','ppp_ctrl','zls_ctrl','isp_bgobjdepth','depth.base','depth.stride','depth.comp_base','depth.comp_stride','fragment_attachments'],
 ppp_ctrl=hex(a.ppp_ctrl),
 source_isp_bgobjvals_preserved=hex(c.isp_bgobjvals),all_shader_and_launch_bytes_unchanged=True,
 own_table_addresses=dict(tfu=tfu_address,pbe=pbe_address),own_scissor_address=scissor_address,scissor_index=scissor_index,
 own_DATA_patches=patches,phase_DATA_update_handles=phase_changes,compressed_inputs_preserved_in_phase1=[9,124],
 metadata_handle_renamed_from_source_gate=dict(original=128,fixture=124),
 CPU_oracle=dict(phase0_decoded_output='unchanged A5 seed',phase1_decoded_colors=['green','white']if a.alternate_input else['red','blue'],pixels=2048,guard_bytes_total=138752,opaque_payload_not_a_CPU_byte_oracle=True),
 sha256={p.name:hashlib.sha256(p.read_bytes()).hexdigest()for p in out.iterdir()if p.name!='provenance.json'})
(out/'provenance.json').write_text(json.dumps(proof,indent=2)+'\n')
print('Exported compressed color producer/consumer; alternate input',a.alternate_input,'exact decoded CPU image and bounded opaque buffers')
