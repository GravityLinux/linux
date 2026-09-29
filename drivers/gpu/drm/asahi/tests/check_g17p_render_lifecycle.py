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
scope={'struct':struct,'print':lambda *a,**kw:None};exec(compile(ast.Module(body=[method],type_ignores=[]),'<source tilemap method>','exec'),scope)
expected=[]
source=ast.parse((args.m1n1/'proxyclient/experiments/agx_g17p_boot.py').read_text())
tick=next(n for n in ast.walk(source) if isinstance(n,ast.FunctionDef) and n.name=='announce_runtime_tick')
clock_scope=dict(struct=struct,g17p=types.SimpleNamespace(CONTROL_MESSAGE_SIZE=64),instances=[None],ascs=[None],runtime_control_sequence=[0],announce_control_entry=lambda a,b,body,label:dict(consumed=True,body=body))
exec(compile(ast.Module(body=[tick],type_ignores=[]),'<source tick>','exec'),clock_scope)
for ordinal in range(2,32):expected.append((f'tick:{ordinal}',clock_scope['announce_runtime_tick'](ordinal-1)['body']))
prestate=next(n for n in ast.walk(source) if isinstance(n,ast.FunctionDef) and n.name=='announce_runtime_submission')
values=next(n.value for n in ast.walk(prestate) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='values' for t in n.targets))
expected.append(('prestate',struct.pack('<4I',*ast.literal_eval(values))))
for case in range(64):
 ordinal=case%31+1
 values={f.name:0 for f in dataclasses.fields(r.G17PRenderParameters) if f.default is dataclasses.MISSING}
 values.update(width=128+case,height=128,context_base=0x1000000000,tilemap=0x10001b0000,heapmeta=0x10001b1000,tpc=0x10001d8000,
  deflake_1=0x10000682a0,deflake_2=0x1000068020,deflake_3=0x1000068000,ta_status=0x1000078000,fragment_status=0x10001a8000,aux_fb=0x10000300000,
  encoder=0x1000200000+case*0x4000,store_pipeline=0x10000000140,load_pipeline=0x10000000240,
  ta_user_timestamp_start=0xfffffc2181400040+case*32,ta_user_timestamp_end=0xfffffc2181400048+case*32,
  fragment_user_timestamp_start=0xfffffc2181400050+case*32,fragment_user_timestamp_end=0xfffffc2181400058+case*32,
  reactive_tvb_growth=True,emit_uapi_fields=True,lifecycle_ordinal=ordinal,queue_item_index=ordinal,local_item_registers=True,native_status_registers=True,tvb_pool_id=0)
 params=r.G17PRenderParameters(**values)
 memory={};pair={};bodies={}
 def write(at,body):
  for i,value in enumerate(body):memory[at+i]=value
 def read(at,size):return bytes(memory[at+i] for i in range(size))
 for i,kind in enumerate(('tiling','fragment')):
  size=b.G17PWorkBuilder.BODY_STRIDE[kind];address=(0xfffffc20c0018000,0xfffffc20c00b0000)[i]+ordinal*size
  builder=b.G17PWorkBuilder(lambda size,name:address,write,kind,0);builder.use_pools(0xfffffc20c0820100,0xfffffc20c0830080)
  builder.write_tail=builder.write_lifecycle_fields=builder.write_item_fields=builder.write_structural_tail=True
  builder.status_base=(0xfffffc2001608000,0xfffffc2001628000)[i]
  builder.tail_pointer_overrides={0x934 if i==0 else 0x21ce:0xfffffc20c0828000}
  regs=r.build_tiling_registers(params) if i==0 else r.build_fragment_registers(params)
  pair[kind]=builder.item(ordinal,(0xfffffc20c0860000,0xfffffc20c0832800),regs,0,0,context_id=1,
   record_indices=g.paired_item_pool_record_indices(ordinal),submission_ordinal=ordinal,queue_pair=0,parameters=params)
 fake=types.SimpleNamespace(pair_resource_namespace=False,pair_resource_namespace_after_first=False,native_b2_full_descriptor_shape=False,
  render_context_base=0x1000000000,TILEMAP_BLOCKS=8,TILEMAP_BLOCK_STRIDE=0x1200,PAIR_RESOURCE_STRIDE=0x5e0000,_read_dva=read,_write_dva=write)
 scope['_advance_tilemap_block'](fake,pair,ordinal,0)
 for i,kind in enumerate(('tiling','fragment')):
  expected.append((f'{case}:{kind}:descriptor',read(pair[kind][0],b.G17PWorkBuilder.BODY_STRIDE[kind])))
  expected.append((f'{case}:{kind}:optional',g.build_optional_item(kind,(0x7000438000,0x7000460000)[i],(0xfffffc20001d8000,0xfffffc2000200000)[i],0xfffffc20c0828000,0xfffffc20c07b8000,
   tiling_shared_object=0xfffffc20c0860000 if i==0 else None,grid_index=i,item_index=ordinal,submission_ordinal=ordinal,context_id=1,uuid=0x15)))
  expected.append((f'{case}:{kind}:context',g.build_queue_context_item(kind,pair[kind][0],(0xfffffc20c0000000,0xfffffc20c00000c0)[i],pair=0,item_index=ordinal,context_id=1,grid_index=i,locator_context_id=1,
   **g.paired_queue_context_dependencies(kind,pair=0,item_index=ordinal))))
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
print(f'PASS: {len(expected)} complete retained-render append objects, {offset} bytes; 64 caller variations, real Python tilemap advancement and queue dependencies; generation bounds')
