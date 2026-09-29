#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Compare the Rust compute builders with current source Python constructors."""
import argparse,importlib,struct,subprocess,sys,tempfile,types
from pathlib import Path
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1',type=Path)
parser.add_argument('--rustc',default='rustc')
args=parser.parse_args()
# Load the dependency-free byte builders without executing m1n1 package setup.
package=types.ModuleType('compute_reference')
package.__path__=[str(args.m1n1/'proxyclient/m1n1/agx')]
sys.modules[package.__name__]=package
ref=importlib.import_module('compute_reference.g17p_compute')
expected=[]
def emit(name,body):expected.append((name,body))
for case in range(96):
    ordinal=(0,1,3,255,256,1024,0xffffffff)[case%7]
    preempt=0x10000200000+case*0x4000;cdm=0x10003000000+case*0x8000
    regs=list(ref.build_compute_register_program(preempt,cdm,0x0200034503000346+case,case%64,ordinal,
        0x10004000000,0x7000220000,usc_exec_base=0x10000000000,helper_binary=case*0x4000|5,
        helper_data=0x10000300000+case*0x4000,helper_cfg=case<<16,execution_gate=case%3))
    if case%3==1:regs.append((0x1a440,0xdeadbeef))
    if case%3==2:regs.extend((0x20000+i,i*9821) for i in range(40,128))
    emit(f'{case}:registers',struct.pack('<I',len(regs))+b''.join(struct.pack('<IQ',n,v) for n,v in regs))
    emit(f'{case}:descriptor',ref.build_compute_descriptor(regs,0xfffffc20c0900100,0x7002000000+case*0x4000,cdm+0x100,
        submit_sequence=case<<40|case,context_id=case%64,grid_index=case%12,
        user_timestamp_start=case*0x4000,user_timestamp_end=0x7000500000+case*8,support_control=0x21000001+case,
        support_flags=case%4,work_ordinal=ordinal,queue_submission=case+1,queue_ordinal=ordinal,submission_index=case*3+1,
        sampler_array=0 if case%2==0 else 0x10000800000+case*8,sampler_count=0 if case%2==0 else case+1))
    emit(f'{case}:optional',ref.build_compute_optional(0x70004d8000,0xfffffc2000278000,grid_index=case%12,
        submission_ordinal=ordinal,channel_control=0xfffffc20c07b8040,uuid=case*257,field_46=case%5,
        field_1e=case%3,field_32=case+1,field_56=case+2,field_5e=case+3,first_submit=case%2==0,item_index=ordinal))
    emit(f'{case}:event',ref.build_compute_event(case*171,case%12,case*257))
    values=[(i%128,case*19000+i) for i in range(case%32+1)]
    context=ref.build_compute_queue_context_item(0xfffffc20c0358000+case*0x1040,0xfffffc20c0000300,grid_index=case%12,
        flags_200=0x1000000000000000,word_330=case%5,word_338=case*123,item_index=ordinal,
        context_points=None if case%3==0 else values,context_event_slot=None if case%2==0 else case%128,
        completion_value=None if case%4==0 else case+1)
    emit(f'{case}:context',context)
    previous=bytes((i*137+case)&255 for i in range(0x200))
    emit(f'{case}:context-reuse',ref.update_compute_queue_context_item(previous,context))
    for records in (2,3,128,255,256):emit(f'{case}:offset-{records}',struct.pack('<I',ref.compute_queue_context_record_offset(ordinal,records)))
    emit(f'{case}:scheduler',ref.build_compute_scheduler_record(0xfffffc2001640000+case*4,case*77,case%3,
        job_list=0 if case%2==0 else 0xfffffc2000000000,node_id=0 if case%3==0 else case*123,completion_kind=case%4))
    emit(f'{case}:scheduler-slot',ref.build_compute_scheduler_slot(case*17,case%9))
with tempfile.TemporaryDirectory() as tmp:
    binary=Path(tmp)/'compute'
    subprocess.run([args.rustc,'--edition=2021',str(Path(__file__).with_name('g17p_compute.rs')),'-o',str(binary)],check=True)
    actual=subprocess.check_output([str(binary)])
offset=0
for name,body in expected:
    chunk=actual[offset:offset+len(body)]
    if chunk!=body:
        differences=[hex(i) for i,(a,b) in enumerate(zip(chunk,body)) if a!=b][:16]
        raise AssertionError((name,differences,len(chunk),len(body)))
    offset+=len(body)
assert offset==len(actual)
print(f'PASS: {len(expected)} complete compute objects/offsets ({offset} bytes), duplicate registers, sampler/timestamp fields, dependency lists and firmware-state-preserving slot reuse; unsupported USC base and invalid bounds rejected')
