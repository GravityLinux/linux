#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Differential check of retained queue metadata against source-only Python."""
import argparse, ast, importlib.util, struct, subprocess, tempfile
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1',type=Path);p.add_argument('--rustc',default='rustc');args=p.parse_args()
path=args.m1n1/'proxyclient/m1n1/agx/g17p_compute.py'
s=importlib.util.spec_from_file_location('compute',path)
c=importlib.util.module_from_spec(s);s.loader.exec_module(c)
# Extract only pure constant/function dependencies; no proxy or firmware input.
tree=ast.parse((args.m1n1/'proxyclient/experiments/agx_g17p_native_add3.py').read_text())
nodes={}
for node in tree.body:
    if isinstance(node,ast.FunctionDef):nodes[node.name]=node
    elif isinstance(node,ast.Assign):
        for target in node.targets:
            if isinstance(target,ast.Name):nodes[target.id]=node
wanted={'_work_addresses','_scheduler_for_ordinal','_registers_for_workload','_build_channel_control', 'DESCRIPTOR_RING_RECORDS', 'OPTIONAL_STRIDE', 'EVENT_STRIDE', 'RUNTIME_EVENT_BASE'}
while True:
    dependencies={n.id for name in wanted for n in ast.walk(nodes[name]) if isinstance(n,ast.Name) and isinstance(n.ctx,ast.Load) and n.id in nodes}
    if dependencies <= wanted:break
    wanted |= dependencies
ns={'compute':c,'struct':struct}
module=ast.Module(body=[node for node in tree.body if any(nodes[name] is node for name in wanted)],type_ignores=[])
exec(compile(module,'source-only-native-add3','exec'),ns)
expected=[]
def emit(name,body):expected.append((name,body))
emit('opening-program',b''.join(struct.pack('<IQ',*r) for r in ns['_registers_for_workload'](0,resource_base=0x30000000000,cdm_base=0x10000600000)))
for n in (1,2,3,4,34,35,36,127,128,239,240,255,256,383,384,1024):
    for slot in (0,1,2):
        base=ns['_work_addresses'](n)
        persistent=ns['_queue_addresses'](1);transport=ns['_queue_addresses'](0)
        scheduler,scheduler_slot,work_id=ns['_scheduler_for_ordinal'](base,n)
        descriptor=ns['DESCRIPTOR']+(n%ns['DESCRIPTOR_RING_RECORDS'])*ns['DESCRIPTOR_STRIDE']
        low=ns['DESCRIPTOR_LOW']+(descriptor-ns['DESCRIPTOR'])
        optional=ns['OPTIONAL']+n*ns['OPTIONAL_STRIDE'];event=ns['RUNTIME_EVENT_BASE']+n*ns['EVENT_STRIDE']
        ctx=transport['context_high']+c.compute_queue_context_record_offset(n)
        dispatch=(ns['DISPATCH_A']+n*8,ns['DISPATCH_B']+n*8)
        status=(persistent['status_a'],persistent['status_b'])
        emit(f'{n}/{slot}:addresses',struct.pack('<11Q',descriptor,low,optional,event,ctx,scheduler,scheduler_slot,*dispatch,*status))
        regs=ns['_registers_for_workload'](n,command_slot=slot)
        regs=c.apply_compute_uapi_registers(regs,preempt_base=0x30000000000+slot*ns['CLIENT_WORKLOAD_STRIDE'],cdm_base=0x10000600000,usc_exec_base=0x10000000000,helper_binary=0,helper_data=0,helper_cfg=0)
        emit(f'{n}/{slot}:registers',b''.join(struct.pack('<IQ',*r) for r in regs))
        emit(f'{n}/{slot}:scheduler',c.build_compute_scheduler_record(scheduler_slot,work_id=work_id))
        emit(f'{n}/{slot}:descriptor',c.build_compute_descriptor(regs,scheduler,low,0x1000060002c,
            submit_sequence=n,context_id=persistent['context_id'],grid_index=transport['grid'],dispatch_a=dispatch[0],dispatch_b=dispatch[1],status_a=status[0],status_b=status[1],
            user_timestamp_start=0xfffffc2181400000+n*16,user_timestamp_end=0xfffffc2181400008+n*16,zero_page=persistent['zero_page'],shared_control=persistent['shared_support'],protection_index=1,support_control=0xe0a00001,support_flags=0,
            work_ordinal=n,queue_submission=n+1,queue_ordinal=0,submission_index=n+1,sampler_array=0,sampler_count=0))
        emit(f'{n}/{slot}:optional',c.build_compute_optional(transport['context_low'],transport['context_high'],grid_index=transport['grid'],submission_ordinal=persistent['optional_submission']+n,
            shared_control=persistent['shared_support'],channel_control=transport['channel_control'],uuid=persistent['uuid'],field_46=persistent['optional_field_46'],field_1e=2,field_32=persistent['optional_field_32'],field_56=persistent['optional_field_56'],field_5e=2,first_submit=True,item_index=0))
        step=(descriptor-ns['DESCRIPTOR'])//0x20
        emit(f'{n}/{slot}:context',c.build_compute_queue_context_item(descriptor,transport['queue'],transport['grid'],flags_200=persistent['qctx_flags'],word_220=persistent['qctx_word_220'],word_330=0,word_338=persistent['qctx_word_338'],word_350=0x000110038001a002+step,word_358=0x000020038001a03b+step,word_378=0x003fffffffffffff,item_index=n))
        emit(f'{n}/{slot}:control',ns['_build_channel_control'](1))
# Ordinary post-render startup has UAPI registers and a distinct second queue.
regs=c.apply_compute_uapi_registers(ns['_registers_for_workload'](0),preempt_base=0x30000000000,cdm_base=0x10000600000,usc_exec_base=0x10000000000,helper_binary=0,helper_data=0,helper_cfg=0)
emit('post-render:opening-program',b''.join(struct.pack('<IQ',*r) for r in regs))
a=ns['_work_addresses'](1)
regs=c.apply_compute_uapi_registers(ns['_registers_for_workload'](1,command_slot=1),preempt_base=0x30000078000,cdm_base=0x10000600000,usc_exec_base=0x10000000000,helper_binary=0,helper_data=0,helper_cfg=0)
emit('post-render:second-descriptor',c.build_compute_descriptor(regs,a['scheduler'],a['descriptor_low'],0x1000060002c,submit_sequence=1,context_id=a['context_id'],grid_index=a['grid'],dispatch_a=a['dispatch_a'],dispatch_b=a['dispatch_b'],status_a=0xfffffc2001a00010,status_b=0xfffffc2001a00018,user_timestamp_start=0xfffffc2181400010,user_timestamp_end=0xfffffc2181400018,zero_page=a['zero_page'],shared_control=a['shared_support'],protection_index=1,support_control=0xe0a00001,support_flags=0,work_ordinal=1,queue_submission=2,queue_ordinal=0,submission_index=2,sampler_array=0,sampler_count=0))
emit('post-render:second-optional',c.build_compute_optional(a['context_low'],a['context_high'],grid_index=a['grid'],submission_ordinal=a['optional_submission'],shared_control=a['shared_support'],channel_control=a['channel_control'],uuid=a['uuid'],field_46=a['optional_field_46'],field_1e=2,field_32=a['optional_field_32'],field_56=a['optional_field_56'],field_5e=2,first_submit=True,item_index=0))
step=(a['descriptor']-ns['DESCRIPTOR'])//0x20
emit('post-render:second-context',c.build_compute_queue_context_item(a['descriptor'],a['queue'],a['grid'],flags_200=a['qctx_flags'],word_220=a['qctx_word_220'],word_330=0,word_338=a['qctx_word_338'],word_350=0x000110038001a002+step,word_358=0x000020038001a03b+step,word_378=0x003fffffffffffff,item_index=0))
with tempfile.TemporaryDirectory() as tmp:
    binary=Path(tmp)/'lifecycle'
    subprocess.run([args.rustc,'--edition=2021',str(Path(__file__).with_name('g17p_compute_lifecycle.rs')),'-o',str(binary)],check=True)
    actual=subprocess.check_output([str(binary)])
offset=0
for name,body in expected:
    chunk=actual[offset:offset+len(body)]
    assert chunk==body,(name,[hex(i) for i,(x,y) in enumerate(zip(chunk,body)) if x!=y][:16],len(chunk),len(body))
    offset+=len(body)
assert offset==len(actual)
print(f'PASS: {len(expected)} retained compute lifecycle objects, {offset} bytes; early transitions and scheduler/descriptor/context wrap')
