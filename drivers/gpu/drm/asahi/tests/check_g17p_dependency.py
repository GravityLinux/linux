#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Compare native dependency objects with executed source-only shim builders.

No target, firmware binaries or captured objects. The topology method runs
over synthetic owned RAM; the actual shim register method runs over explicit
caller state. Dependency constructor checks vary timestamps, CDM, samplers,
completion values and receipt sequences, preserving register store order.
"""
import argparse
import ast
from dataclasses import MISSING, fields
import importlib
import os
import re
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
from types import ModuleType, SimpleNamespace as NS

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1', type=Path)
p.add_argument('--rustc', default='rustc')
args = p.parse_args()
package = ModuleType('dependency_reference')
package.__path__ = [str(args.m1n1/'proxyclient/m1n1/agx')]
sys.modules[package.__name__] = package
c = importlib.import_module('dependency_reference.g17p_compute')
s = importlib.import_module('dependency_reference.g17p_submission')
q = importlib.import_module('dependency_reference.g17p')
b = importlib.import_module('dependency_reference.g17p_backend')
reports = importlib.import_module('dependency_reference.g17p_reports')
r = importlib.import_module('dependency_reference.g17p_render')
tree = ast.parse((args.m1n1/'proxyclient/m1n1/agx/shim.py').read_text())
shim = next(
    n for n in tree.body if isinstance(n, ast.ClassDef) and any(
        isinstance(m, ast.FunctionDef) and m.name == '_submit_dependency_wave' for m in n.body))
methods = {n.name: n for n in shim.body if isinstance(n, ast.FunctionDef)}
expected = []
def emit(name, body): expected.append((name, bytes(body)))
def regs(values): return b''.join(struct.pack('<IQ', *r) for r in values)

# Only the selected source methods are executed. Remove hardware imports;
# existing serializers and an explicit synthetic allocator supply dependencies.
ns = dict(__package__=package.__name__, struct=struct, os=os,
          g17p=q, g17p_compute=c, submission=s, G17PQueue=b.G17PQueue, MemoryAttr=NS(Shared=0))
def method(name):
    n = methods[name]
    selected = ast.FunctionDef(name=n.name, args=n.args,
        body=[stmt for stmt in n.body if not isinstance(stmt, (ast.Import, ast.ImportFrom))],
        decorator_list=[], returns=n.returns, type_comment=n.type_comment)
    exec(compile(ast.fix_missing_locations(ast.Module(body=[selected], type_ignores=[])), name, 'exec'), ns)
    return ns[name]

ram = {}
def write(address, body):
    for i, value in enumerate(body): ram[address+i] = value
def read(address, size): return bytes(ram.get(address+i, 0) for i in range(size))
control = 0xfffffc20c07b8000
for i in range(2):
    write(0xfffffc20c0000000 + i*0xc0, q.build_queue_record(
        0xfffffc2000010000 + i*0x2870, 0xfffffc20c0008000 + i*0x2870,
        0xfffffc2000000000, control))
pair = {name: (name, b.G17PQueue(read, 0xfffffc20c0000000 + i*0xc0, i))
        for i, name in enumerate(('tiling', 'fragment'))}
backend = NS(reset_quiescent_work_channel=lambda name: None,
    muxed_queue_pair=lambda index: pair, muxed_queue_pairs={},
    muxed_queue_pointer_sets={0: {'tiling': {'channel_control': control}, 'fragment': {}}},
    channels=NS(counters=lambda entry: (0,0,0)),
    _ensure_firmware_range=lambda *a: None, _read_dva=read, _write_dva=write,
    _clean_dva_range=lambda *a: None, firmware_high_root=0x10000004000,
    space=NS(context=2, flush=lambda: None, uat=NS(
        iotranslate_root=lambda root, address, size: [(0x10010000000, size)],
        iomap_at=lambda *a, **kw: None, flush_dirty=lambda: None, invalidate_cache=lambda: None)),
    u=NS(inst=lambda *a: None, proxy=NS(memset32=lambda *a: None, dc_civac=lambda *a: None)))
owner = NS(front=NS(g17p=backend))
topology = method('_prepare_dependency_queue_topology')(owner)
first, last = topology['first'], topology['final']
layouts = []
for i, name in enumerate(('first', 'tiling', 'fragment', 'final')):
    if name in topology:
        item = topology[name]
        address, grid = item['queue'], item['grid']
        low, high = item['context_low'], item['context_high']
    else:
        queue = backend.muxed_queue_pairs[0][name][1]
        address, grid = queue.address, queue.grid_index
        low, high = ((0x7000460000, 0xfffffc2000200000) if i==1 else
                     (0x7000488000, 0xfffffc2000228000))
    record = q.parse_queue_record(read(address, 0xc0))
    ptr, ring = record['pointers_addr'], record['ring_addr']
    layouts.append((address, ptr, ring, grid, record['job_list_addr'], record['context_addr'], low, high))
    emit(name+':layout', struct.pack('<8Q', *layouts[-1]))
    emit(name+':record', read(address, 0xc0))
    emit(name+':pointers', read(ptr, 0x80))
    emit(name+':control', read(record['context_addr'], 0x40))
for i, (record, slot, work) in enumerate(((0xfffffc20c0820100,0xfffffc20015f8004,0),
        (0xfffffc20c0828100,0xfffffc2001600004,1), (0xfffffc20c0830100,0xfffffc2001608004,2))):
    emit(f'job-{i}', read(topology['job_list']+i*0x18,0x18))
    emit(f'scheduler-{i}', c.build_compute_scheduler_record(slot, work_id=work))
emit('compute-support', c.build_compute_shared_support(0x7000208000,0xfffffc2001610000,
    word_08=0,word_10=2,header=1,resource_class=0x15,cursor=0xa8,field_54=0,field_5c=1,
    final_kind=2,word_20=0x150000000000,word_28=0x150000000000))
emit('render-support', c.build_compute_compact_control_support(2,0x70019e8000,0x70017e0000,
    0xfffffc2001620000,active=1,resource_class=0x16,cursor=0xb0,final_kind=3))
emit('compute-operands', c.build_compute_operand_table(0x7000238000))
pages=s.build_submission_leaf_pages(0,((0x12,6),(0x3c,2)),8,0)
for name in ('primary_index','secondary_index','pool_a_slots','pool_b_slots','shared_slots','flag'):
    emit(name,pages[name])
emit('pool-a',s.build_record_array_a(0xfffffc2001600004,work_id=1))
emit('pool-b',s.build_record_array_b(0xfffffc2001638004,0xfffffc2001630040,0,1))
emit('shared',s.build_shared_object((0xfffffc20c0860000,0xfffffc20c0850000,
    0xfffffc2001630000,0xfffffc2001640000),0,8,1))

# Pure native constants/functions are imported by AST dependency closure.
ntree=ast.parse((args.m1n1/'proxyclient/experiments/agx_g17p_native_add3.py').read_text())
nodes={}
for node in ntree.body:
    if isinstance(node, ast.FunctionDef): nodes[node.name]=node
    elif isinstance(node, ast.Assign):
        for target in node.targets:
            if isinstance(target, ast.Name): nodes[target.id]=node
wanted={'_registers_for_workload'}
while True:
    dependencies={n.id for name in wanted for n in ast.walk(nodes[name])
        if isinstance(n,ast.Name) and isinstance(n.ctx,ast.Load) and n.id in nodes}
    if dependencies <= wanted: break
    wanted |= dependencies
native={'compute':c}
exec(compile(ast.Module(body=[node for node in ntree.body if any(nodes[name] is node for name in wanted)],
    type_ignores=[]),'source-only-native-registers','exec'),native)
compute_regs=method('_compute_registers')

# Execute the source registration constructor without initializing firmware.
boot_tree=ast.parse((args.m1n1/'proxyclient/experiments/agx_g17p_boot.py').read_text())
registration=next(n for n in boot_tree.body if isinstance(n,ast.FunctionDef) and n.name=='build_control_20_entry_object')
exec(compile(ast.Module(body=[registration],type_ignores=[]),'source-only-registration','exec'),ns)

# Extract the actual non-baseline override expression from the source method.
assignments=[n for n in ast.walk(methods['_submit_dependency_wave']) if isinstance(n,ast.Assign)
    and any(isinstance(t,ast.Attribute) and t.attr=='render_control_overrides' for t in n.targets)
    and isinstance(n.value,ast.Dict)]
override=next(n.value for n in assignments if 'fragment' in [k.value for k in n.value.keys if isinstance(k,ast.Constant)])
override_ns=dict(render_control_overrides={},render_cycle=0x180020,render_record=0x88005,
    tiling_lifecycle=0xc701000114,fragment_lifecycle=0xc701000113,render_stamp=0x101)
overrides=eval(compile(ast.Expression(body=override),'source-only-dependency-overrides','eval'),override_ns)

for case in range(16):
    caller_cdm=0x10000600000+case*0x78000
    for i in range(2):
        descriptor=0xfffffc20c0358000+i*0x1040
        t=layouts[i*3]
        state=NS(cdm_base=caller_cdm,usc_exec_base=0x10000000000,helper_binary=0x1234000,
                 helper_data=0x4321000,helper_cfg=case)
        runtime={'native':NS(**native),'client':{'resource_base':0x30000000000,'cdm_base':caller_cdm},
            'dependency_state_aliases':{0x100:0x7000220000,0x102:0x70035d8000},
            'dependency_robustness_aliases':{0x100:0x1000078000,0x102:0x1000238000},
            'dependency_preempt_aliases':{0x100:caller_cdm+0x30000,0x102:caller_cdm+0x30000},
            'dependency_cdm_aliases':{0x100:0x100000b0000}}
        registers=compute_regs(runtime,NS(hardware_state=state),i+1,{}, {'slot':1-i},
            scheduler_context_word=0x100+i*2)
        emit(f'{case}/{i}:registers',regs(registers))
        timestamps=(0xfffffc2181400000+case*32+i*16,0xfffffc2181400008+case*32+i*16)
        emit(f'{case}/{i}:descriptor',c.build_compute_descriptor(registers,
            (0xfffffc20c0820100,0xfffffc20c0830100)[i],0x7000340000+i*0x1040,
            0x100000b0030 if i==0 else caller_cdm+0x30,submit_sequence=0,context_id=1,
            grid_index=t[3],dispatch_a=0xfffffc20001c8000+i*0xc,dispatch_b=0xfffffc20c07c0000+i*0xc,
            status_a=0xfffffc2000024c68+i*0x20,status_b=0xfffffc2000024c70+i*0x20,
            user_timestamp_start=timestamps[0],user_timestamp_end=timestamps[1],
            zero_page=(0xfffffc2001618000,0xfffffc2001650000)[i],shared_control=0xfffffc20c0838000,
            protection_index=1,support_control=0xe0a00001,support_flags=0,work_ordinal=0,
            queue_submission=1,queue_ordinal=0,submission_index=1,
            sampler_array=0 if case%2==0 else caller_cdm+0x10000,sampler_count=0 if case%2==0 else case))
        emit(f'{case}/{i}:optional',c.build_compute_optional(t[6],t[7],grid_index=t[3],
            submission_ordinal=i*2,shared_control=0xfffffc20c0838000,channel_control=t[5],uuid=0x16,
            field_46=0,field_1e=2,field_32=1,field_56=i*2,field_5e=2,first_submit=True,item_index=0))
        event=bytearray(q.build_event_record(1,'compute',t[3]))
        struct.pack_into('<I',event,q.EVENT_RECORD_COUNTER,0x102)
        emit(f'{case}/{i}:event',event)
        step=(descriptor-0xfffffc20c0358000)//0x20
        emit(f'{case}/{i}:context',c.build_compute_queue_context_item(descriptor,t[0],t[3],
            flags_200=(0x1000000000000004,0x10000c0000000004)[i],
            word_220=(0xffff080000000001,0xffff080200000001)[i],word_330=0,word_338=2,
            word_350=0x110038001a002+step,word_358=0x20038001a03b+step,word_378=0x3fffffffffffff,
            item_index=0,context_points=((0,0),) if i==0 else ((2,1),(3,0)),
            context_event_slot=i*2,completion_value=case+1 if i==0 else 1))
    for i,kind in enumerate(('tiling','fragment')):
        t=layouts[i+1]
        emit(f'{case}:{kind}-optional',s.build_optional_item(kind,t[6],t[7],0xfffffc20c0840000,t[5],
            tiling_shared_object=0xfffffc20c0878000 if i==0 else None,grid_index=t[3],item_index=0,
            submission_ordinal=1,context_id=1,uuid=0x16,scheduler_class=2,
            lifecycle_ordinal=0 if i==0 else None,u16_overrides={0x46:1,0x56:1,0x5e:2}))
        emit(f'{case}:{kind}-context',s.build_queue_context_item(kind,
            (0xfffffc20c0018000,0xfffffc20c00b0000)[i],t[0],pair=0,item_index=0,context_id=1,
            grid_index=t[3],dependency_grid=t[3],
            context_points=((0,case+1),(1,0)) if i==0 else ((1,1),(2,0))))
    parameters={f.name:0 for f in fields(r.G17PRenderParameters) if f.default is MISSING}
    parameters.update(width=(1,32,128,129)[case%4],height=(129,128,32,1)[case%4],
        context_base=0x1000000000,tilemap=0x10001b0000,heapmeta=0x10001b1000,tpc=0x10001d8000,
        ta_status=0x1000078000,fragment_status=0x10001a8000,deflake_1=0x10000682a0,
        deflake_2=0x1000068020,deflake_3=0x1000068000,encoder=0x1000000000+case*0x4000,
        aux_fb=0x10000300000,sampler_array=0 if case%2==0 else 0x10000080000+case*8,
        sampler_count=0 if case%2==0 else case,emit_uapi_fields=True,reactive_tvb_growth=True,
        lifecycle_ordinal=1,timestamp_a=0xfffffc2000024c78,timestamp_b=0xfffffc2000024c80,
        fragment_timestamp_start=0xfffffc2000024c78,fragment_timestamp_end=0xfffffc2000024c80)
    rp=r.G17PRenderParameters(**parameters)
    for i,kind in enumerate(('tiling','fragment')):
        output=[]
        builder=b.G17PWorkBuilder(lambda *a: (0xfffffc20c0018000,0xfffffc20c00b0000)[i],
            lambda address,body:output.append(bytes(body)),kind,0)
        builder.use_pools(0xfffffc20c0828100,0xfffffc20c0848080)
        builder.write_tail=builder.write_lifecycle_fields=builder.write_item_fields=builder.write_structural_tail=True
        builder.low_alias={kind:(0x7000000000,0x7000098000)[i]}
        builder.status_base=(0xfffffc2001628000,0xfffffc2001648000)[i]
        builder.tail_pointer_overrides={0x934 if i==0 else 0x21ce:0xfffffc20c0840000,
            0x8a6 if i==0 else 0x2140:0xfffffc20001c8000+(4 if i==0 else 8),
            0x8ae if i==0 else 0x2148:0xfffffc20c07c0000+(4 if i==0 else 8)}
        builder.item_field_overrides={} if i==0 else {0x215c:0}
        program=(r.build_tiling_registers(rp) if i==0 else r.build_fragment_registers(rp))
        program=[(n,overrides[kind].get(n,v)) for n,v in program]
        builder.item(0,(0xfffffc20c0878000,0xfffffc20c084a800),program,0,0,
            context_id=1,record_indices=(0,0),submission_ordinal=1,queue_pair=0,
            parameters=rp,submit_sequence=1 if i==0 else 0,queue_grid_index=i+1)
        emit(f'{case}:{kind}-descriptor',output[0])
    emit(f'{case}:registration',ns['build_control_20_entry_object'](1,0xfffffc20c0840000,0x70019e8000,
        0x580,case,context_word=1,count=0x28))
    prefix=reports.build_class1_registration_receipt(case,0xfffffc20c0840000,0x70019e8000)
    emit(f'{case}:receipt',prefix)
    def accepts(body,peer=0,owned=True):
        snapshot={('primary_ch13' if peer==0 else 'secondary_ch13'):{'records':[{'record_hex':body.hex(),'slot':0}]}}
        return not reports.unhandled_channel13(snapshot,owned_control_receipts=[prefix] if owned else [])
    receipt=bytearray(prefix+bytes(0x20))
    outcomes=[]
    for at in range(0x28):
        receipt[at]^=1; outcomes.append(accepts(receipt)); receipt[at]^=1
    outcomes.append(accepts(receipt,1))
    outcomes.extend(accepts(receipt[:size]) for size in (0,0x27,0x28,0x47))
    receipt[0x28:]=bytes([case])*0x20
    outcomes.extend((accepts(receipt),accepts(receipt,owned=False)))
    emit(f'{case}:receipt-admission',bytes(outcomes))

for counter in range(3):
    body=bytearray(0x40)
    struct.pack_into('<II',body,0,0x2e,counter)
    struct.pack_into('<I',body,0xc,int(counter==1))
    emit(f'tick-{counter}',body)
for engine in (2,1,0):
    body=bytearray(0x40)
    struct.pack_into('<IIIQ',body,0,0x14,0,0x04000002|(engine<<16),control+engine*0x40)
    emit(f'engine-{engine}',body)

# Interpret only the source-authored host data transition instructions. This
# executes the original loop bounds/store offsets, rather than repeating the
# Rust loop. Mailbox and firmware instructions are outside this bounded slice.
release=next(n for n in ast.walk(boot_tree) if isinstance(n,ast.FunctionDef) and n.name=='release_dependency_window')
assembly=next(n.value for n in ast.walk(release) if isinstance(n,ast.Constant)
    and isinstance(n.value,str) and 'mov w9, #0xd8' in n.value)
assembly=assembly[assembly.index('mov w9, #0xd8'):assembly.index('mov w9, #1\n',assembly.index('mov w9, #0xd8'))]
lines=[line.strip() for line in assembly.splitlines() if line.strip()]
label={line[:-1]:index for index,line in enumerate(lines) if line.endswith(':')}
for case in range(16):
    values={0xfffffc20c0860060+off:case*0x100+off for off in range(0,0x20,4)}
    values.update({0xfffffc20c0850030:0xffffffff if case==15 else case+10,0xfffffc20c0850038:case+20})
    cpu={12:0xfffffc2001620000,14:0xfffffc20c0840048,15:0xfffffc20c0860060,16:0xfffffc20c0850030}
    def operand(value):
        return int(value[1:],0) if value.startswith('#') else cpu[int(value[1:])]
    writes=[]; pc=0; compare=False; steps=0
    while pc<len(lines):
        line=lines[pc]; pc+=1; steps+=1; assert steps<256
        if line.endswith(':'):continue
        if line.startswith('b.lo '):
            if compare: pc=label[line.split()[1][:-1]]
            continue
        ins,*parts=re.split(r',?\s+',line)
        if ins=='mov': cpu[int(parts[0][1:])]=operand(parts[1])
        elif ins=='add': cpu[int(parts[0][1:])]=(operand(parts[1])+operand(parts[2]))&0xffffffff
        elif ins=='cmp': compare=operand(parts[0])<operand(parts[1])
        elif ins in ('ldr','str'):
            register=int(parts[0][1:])
            address=operand(parts[1].lstrip('[').rstrip(']'))
            if len(parts)>2: address+=operand(parts[2].rstrip(']'))
            if ins=='ldr':cpu[register]=values[address]
            else:
                values[address]=cpu[register]&0xffffffff; writes.append((address,values[address]))
        else:raise AssertionError(('unexpected host transition instruction',line))
    emit(f'{case}:host-transition',b''.join(struct.pack('<QI',*w) for w in writes))
numbers=sorted({number for values in overrides.values() for number in values} | {0x10071,0x1a510,0x1a420,0xabcdef})
inputs=[]
for kind in ('tiling','fragment'):
    original=[(n,0xface000000000000+i) for i,n in enumerate(numbers+numbers)]
    inputs.extend(original)
    emit(kind+':override-registers',regs([(n,overrides[kind].get(n,v)) for n,v in original]))
with tempfile.TemporaryDirectory() as tmp:
    binary=Path(tmp)/'dependency'
    subprocess.run([args.rustc,'--edition=2021','-Dwarnings',str(Path(__file__).with_name('g17p_dependency.rs')),
        '-o',str(binary)],check=True)
    actual=subprocess.check_output([str(binary)],input=regs(inputs))
offset=0
for name, body in expected:
    chunk=actual[offset:offset+len(body)]
    assert chunk==body,(name,[(hex(i),hex(a),hex(b)) for i,(a,b) in enumerate(zip(chunk,body)) if a!=b][:16],len(chunk),len(body))
    offset+=len(body)
assert offset==len(actual),(offset,len(actual))
print(f'PASS: {len(expected)} native dependency objects, {offset} bytes; executed topology/register methods, C/R/C contexts and owned receipt prefixes')
