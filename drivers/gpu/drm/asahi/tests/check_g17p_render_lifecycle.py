#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Compare a retained native append against actual pure Python constructors.

Exercises full descriptors (including embedded partial programs), optionals,
queue dependency records and the source tilemap advancement method. No proxy,
firmware binaries, captures or GPU hardware imports.
"""
import argparse, ast, dataclasses, importlib, os, struct, subprocess, sys, tempfile, types
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('m1n1',type=Path);p.add_argument('--rustc',default='rustc');args=p.parse_args()
package=types.ModuleType('repeat_reference');package.__path__=[str(args.m1n1/'proxyclient/m1n1/agx')];sys.modules[package.__name__]=package
r=importlib.import_module(package.__name__+'.g17p_render');b=importlib.import_module(package.__name__+'.g17p_backend');g=importlib.import_module(package.__name__+'.g17p_submission')
node=next(n for n in ast.parse((args.m1n1/'proxyclient/m1n1/agx/g17p_shim.py').read_text()).body if isinstance(n,ast.ClassDef) and n.name=='G17PShimBackend')
method=next(n for n in node.body if isinstance(n,ast.FunctionDef) and n.name=='_advance_tilemap_block')
scope={'struct':struct,'__package__':package.__name__,'print':lambda *a,**kw:None};exec(compile(ast.Module(body=[method],type_ignores=[]),'<source tilemap method>','exec'),scope)
mirror_method=next(n for n in node.body if isinstance(n,ast.FunctionDef) and n.name=='_apply_scheduler_node')
exec(compile(ast.Module(body=[mirror_method],type_ignores=[]),'<source scheduler method>','exec'),scope)
alloc_method=next(n for n in node.body if isinstance(n,ast.FunctionDef) and n.name=='paired_builder_for')
exec(compile(ast.Module(body=[alloc_method],type_ignores=[]),'<source paired allocator>','exec'),scope)
arrays=ast.literal_eval(next(n.value for n in node.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='ITEM_ARRAYS' for t in n.targets)))
aliases=ast.literal_eval(next(n.value for n in node.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='DESCRIPTOR_LOW_ARRAYS' for t in n.targets)))
retire_method=next(n for n in node.body if isinstance(n,ast.FunctionDef) and n.name=='_complete_native_leaf_publication')
exec(compile(ast.Module(body=[retire_method],type_ignores=[]),'<source leaf retirement>','exec'),scope)
expected=[]
source=ast.parse((args.m1n1/'proxyclient/experiments/agx_g17p_boot.py').read_text())
tick=next(n for n in ast.walk(source) if isinstance(n,ast.FunctionDef) and n.name=='announce_runtime_tick')
clock_scope=dict(struct=struct,g17p=types.SimpleNamespace(CONTROL_MESSAGE_SIZE=64),instances=[None],ascs=[None],runtime_control_sequence=[0],announce_control_entry=lambda a,b,body,label:dict(consumed=True,body=body))
exec(compile(ast.Module(body=[tick],type_ignores=[]),'<source tick>','exec'),clock_scope)
for ordinal in range(2,255):expected.append((f'tick:{ordinal}',clock_scope['announce_runtime_tick'](ordinal-1)['body']))
prestate=next(n for n in ast.walk(source) if isinstance(n,ast.FunctionDef) and n.name=='announce_runtime_submission')
values=next(n.value for n in ast.walk(prestate) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='values' for t in n.targets))
expected.append(('prestate',struct.pack('<4I',*ast.literal_eval(values))))
registration=next(n for n in node.body if isinstance(n,ast.FunctionDef) and n.name=='_publish_partial_index_owner')
reg_scope=dict(struct=struct,__package__=package.__name__)
exec(compile(ast.Module(body=[registration],type_ignores=[]),'<source registration refresh>','exec'),reg_scope)
for count in (32,72,112,0,1312,0xffffffff):
 writes=[];body=bytearray(0x40);struct.pack_into('<Q',body,0x28,0x1000190000);struct.pack_into('<I',body,0x34,count)
 def read(at,size):
  if at==0x100:return bytes(body)
  return struct.pack('<Q',0x1000 if at==0x2018 else 0x4000)
 fake=types.SimpleNamespace(initdata_addr=0x2000,_read_dva=read,_write_dva=lambda at,b:writes.append((at,b)))
 reg_scope['_publish_partial_index_owner'](fake,0x100)
 expected.append((f'index-refresh:{count}',bytes([bool(writes)])+(writes[0][1] if writes else b'')))
extended_ordinals=(255,256,257,510,511,512,1022,1023,1024,2047,2048,4094,4095,4096,65534,65535,65536,131071,131072)
for case in range(256+len(extended_ordinals)):
 ordinal=case%254+1 if case<256 else extended_ordinals[case-256]
 values={f.name:0 for f in dataclasses.fields(r.G17PRenderParameters) if f.default is dataclasses.MISSING}
 values.update(width=128+case,height=128,context_base=0x1000000000,tilemap=0x10001b0000,heapmeta=0x10001b1000,tpc=0x10001d8000,
  deflake_1=0x10000682a0,deflake_2=0x1000068020,deflake_3=0x1000068000,ta_status=0x1000078000,fragment_status=0x10001a8000,aux_fb=0x10000300000,
  encoder=0x1000200000+case*0x4000,store_pipeline=0x10000000140,load_pipeline=0x10000000240,
  ta_user_timestamp_start=0xfffffc2181400040+case*32,ta_user_timestamp_end=0xfffffc2181400048+case*32,
  fragment_user_timestamp_start=0xfffffc2181400050+case*32,fragment_user_timestamp_end=0xfffffc2181400058+case*32,
  reactive_tvb_growth=True,emit_uapi_fields=True,lifecycle_ordinal=ordinal,queue_item_index=ordinal,local_item_registers=True,native_status_registers=True,tvb_pool_id=0)
 params=r.G17PRenderParameters(**values)
 memory={};pair={};bodies={};resets=[]
 def write(at,body):
  if len(body)==0x1200 and at<1<<42:resets.append((at,body))
  for i,value in enumerate(body):memory[at+i]=value
 def read(at,size):return bytes(memory[at+i] for i in range(size))
 allocator=types.SimpleNamespace(paired_builders={},ITEM_ARRAYS=arrays,DESCRIPTOR_LOW_ARRAYS=aliases,
  forced_optional_ordinal_base=None,group_number=ordinal,_ensure_firmware_range=lambda *a:None,
  _map_descriptor_alias=lambda *a:None,_write_dva=write)
 paired=scope['paired_builder_for'](allocator)
 for i,kind in enumerate(('tiling','fragment')):
  builder=getattr(paired,kind);builder.use_pools(0xfffffc20c0820100,0xfffffc20c0830080)
  builder.write_tail=builder.write_lifecycle_fields=builder.write_item_fields=builder.write_structural_tail=True
  builder.status_base=(0xfffffc2001608000,0xfffffc2001628000)[i]
  builder.tail_pointer_overrides={0x934 if i==0 else 0x21ce:0xfffffc20c0828000}
  regs=r.build_tiling_registers(params) if i==0 else r.build_fragment_registers(params)
  pair[kind]=builder.item(ordinal,(0xfffffc20c0860000,0xfffffc20c0832800),regs,0,0,context_id=1,
   record_indices=g.paired_item_pool_record_indices(ordinal),submission_ordinal=ordinal,queue_pair=0,parameters=params)
 fake=types.SimpleNamespace(pair_resource_namespace=False,pair_resource_namespace_after_first=False,native_b2_full_descriptor_shape=False,
  render_context_base=0x1000000000,TILEMAP_BLOCKS=8,TILEMAP_BLOCK_STRIDE=0x1200,PAIR_RESOURCE_STRIDE=0x5e0000,_read_dva=read,_write_dva=write)
 scope['_advance_tilemap_block'](fake,pair,ordinal,0)
 record_a=struct.unpack('<Q',read(pair['tiling'][0]+0x10,8))[0]
 write(record_a,struct.pack('<Q',0x1000))
 scheduler=types.SimpleNamespace(_read_dva=read,_write_dva=write,native_scheduler_publication=True,keep_base_descriptor_mirrors=True)
 scope['_apply_scheduler_node'](scheduler,pair,ordinal+ordinal//2,0,ordinal)
 for kind, offsets in [('tiling',(0x370,0x37c,0x388)),('fragment',(0x470,0x47c))]:
  assert all(struct.unpack('<I',read(pair[kind][0]+at,4))[0] == 0x100+((ordinal+ordinal//2)&0xff) for at in offsets)
 for i,kind in enumerate(('tiling','fragment')):
  expected.append((f'{case}:{kind}:storage',struct.pack('<6Q',pair[kind][0],aliases[kind+'_descriptor']+(pair[kind][0]-arrays[kind+'_descriptor'][0]),
   getattr(paired,kind).alloc(0xc0,kind+'_optional_item'),getattr(paired,kind).alloc(0x40,kind+'_event_item'),
   (0xfffffc2001608000,0xfffffc2001628000)[i]+(ordinal%g.QUEUE_CONTEXT_RECORDS)*0x40,
   (0xfffffc20001d8000,0xfffffc2000200000)[i]+g.queue_context_record_offset(ordinal))))
  expected.append((f'{case}:{kind}:descriptor',read(pair[kind][0],b.G17PWorkBuilder.BODY_STRIDE[kind])))
  expected.append((f'{case}:{kind}:optional',g.build_optional_item(kind,(0x7000438000,0x7000460000)[i],(0xfffffc20001d8000,0xfffffc2000200000)[i],0xfffffc20c0828000,0xfffffc20c07b8000,
   tiling_shared_object=0xfffffc20c0860000 if i==0 else None,grid_index=i,item_index=ordinal,submission_ordinal=ordinal,context_id=1,uuid=0x15)))
  expected.append((f'{case}:{kind}:context',g.build_queue_context_item(kind,pair[kind][0],(0xfffffc20c0000000,0xfffffc20c00000c0)[i],pair=0,item_index=ordinal,context_id=1,grid_index=i,locator_context_id=1,
   **g.paired_queue_context_dependencies(kind,pair=0,item_index=ordinal))))
 assert len(resets)==(ordinal>=8)
 assert all(body==bytes(0x1200) for at,body in resets)
 expected.append((f'{case}:tilemap-reset',bytes([bool(resets)])+b''.join(struct.pack('<Q',at) for at,body in resets)))
# Exercise the actual Python scheduler mutation with distinct live Pool-B
# values, including both record wraps. Compare the fields changed by the
# production Rust append helper, independently of constructor comparisons.
for pair_index in (0,1):
 for item_index in (0,1,35,70,79,254):
  memory={};pair={}
  def write(at,body):
   for i,value in enumerate(body):memory[at+i]=value
  def read(at,size):return bytes(memory.get(at+i,0) for i in range(size))
  for i,kind in enumerate(('tiling','fragment')):
   at=0x10000+i*0x10000;body=bytearray([0xa5])*0x800
   struct.pack_into('<Q',body,0x10,0x30000);struct.pack_into('<Q',body,0x28,0x40000)
   write(at,body);pair[kind]=(at,)
  write(0x30000,struct.pack('<Q',0x50000))
  write(0x40000,struct.pack('<I',0x12345678));write(0x40028,struct.pack('<I',0xabcdef12))
  fake=types.SimpleNamespace(_read_dva=read,_write_dva=write,native_scheduler_publication=True,keep_base_descriptor_mirrors=True)
  scope['_apply_scheduler_node'](fake,pair,7,pair_index,item_index)
  expected.append((f'mirrors:{pair_index}:{item_index}',b''.join(read(pair[kind][0]+at,4) for kind,offsets in [('tiling',(0x310,0x31c,0x328)),('fragment',(0x464,))] for at in offsets)))
for pair_index in (0,1):
 for item_index in (0,1,35,70,79,254):
  writes=[]
  fake=types.SimpleNamespace(native_leaf_publication=True,paired_builders={pair_index:types.SimpleNamespace(leaf_pages={'shared_slots':0xfffffc2001610000})},_write_dva=lambda at,body:writes.append((at,body)))
  scope['_complete_native_leaf_publication'](fake,dict(queue_pair=pair_index,descriptor_pair=pair_index,descriptor_item_index=item_index))
  expected.append((f'retirement-leaf:{pair_index}:{item_index}',bytes([bool(writes)])+b''.join(struct.pack('<Q',at)+body for at,body in writes)))
for pair_index in (0,1):
 for item_index in (0,1,7,8,35,64,254):
  memory={};writes=[];pair={'tiling':(0x10000,),'fragment':(0x20000,)}
  def write(at,body):
   if len(body)==0x1200:writes.append((at,body))
   for i,value in enumerate(body):memory[at+i]=value
  def read(at,size):return bytes(memory.get(at+i,0) for i in range(size))
  write(0x1007c,struct.pack('<I',0x1b0000))
  fake=types.SimpleNamespace(pair_resource_namespace=True,pair_resource_namespace_after_first=False,native_b2_full_descriptor_shape=False,
   render_context_base=0x1000000000,TILEMAP_BLOCKS=8,TILEMAP_BLOCK_STRIDE=0x1200,PAIR_RESOURCE_STRIDE=0x1b0000,_read_dva=read,_write_dva=write)
  scope['_advance_tilemap_block'](fake,pair,item_index,pair_index)
  assert len(writes)==(item_index>=8) and all(body==bytes(0x1200) for at,body in writes)
  expected.append((f'pair-tilemap-reset:{pair_index}:{item_index}',bytes([bool(writes)])+b''.join(struct.pack('<Q',at) for at,body in writes)))
# Execute the production nested lifecycle callback, including its actual
# selected-slot writes, rather than restating the new policy in this oracle.
phase=next(n for n in ast.walk(node) if isinstance(n,ast.FunctionDef) and n.name=='apply_lifecycle_phase')
for current in (0,1,2,4,6,14,0xfffffffd,0xfffffffe,0xffffffff):
 for reused in (False,True):
  writes=[]
  fake=types.SimpleNamespace(native_leaf_publication=False,native_scheduler_publication=True,native_status_publication=False,
   _write_dva=lambda at,body:writes.append((at,body)))
  phase_scope=dict(struct=struct,submission=g,self=fake,phase_current=current,phase_reused=reused,
   context2_pair=False,phase_slot=0x20000,inner_address=None)
  exec(compile(ast.Module(body=[phase],type_ignores=[]),'<source lifecycle callback>','exec'),phase_scope)
  try:
   for name in ('before','fragment','tiling'):phase_scope['apply_lifecycle_phase'](name)
   assert len(writes)==3 and all(at==0x20000 for at,body in writes)
   result=b'\1'+b''.join(body for at,body in writes)
  except ValueError:
   assert not writes
   result=bytes(13)
  expected.append((f'scheduler-phases:{current}:{reused}',result))
with tempfile.TemporaryDirectory() as tmp:
 binary=Path(tmp)/'repeat';subprocess.run([args.rustc,'--edition=2021','-Dwarnings',str(Path(__file__).with_name('g17p_render_lifecycle.rs')),'-o',str(binary)],check=True)
 actual=subprocess.check_output([str(binary)])
offset=0
for name,body in expected:
 got=actual[offset:offset+len(body)]
 if got!=body:
  diffs=[(hex(i),hex(a),hex(b)) for i,(a,b) in enumerate(zip(got,body)) if a!=b]
  raise AssertionError((name,diffs[:20],len(got),len(body)))
 offset+=len(body)
assert offset==len(actual)
print(f'PASS: {len(expected)} retained-render objects/mirror/phase updates, {offset} bytes; {256+len(extended_ordinals)} caller variations, production allocator/storage wraps, full Python scheduler-mutated descriptors, production fresh/recycled phase writes, tilemap/retirement methods and queue dependencies; generation bounds')
