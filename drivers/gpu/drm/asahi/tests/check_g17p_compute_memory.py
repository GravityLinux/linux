#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Compare generated allocator/control memory against current Python builders."""
import argparse,importlib.util,struct,subprocess,tempfile
from pathlib import Path
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1',type=Path);parser.add_argument('--rustc',default='rustc');args=parser.parse_args()
spec=importlib.util.spec_from_file_location('compute',args.m1n1/'proxyclient/m1n1/agx/g17p_compute.py')
ref=importlib.util.module_from_spec(spec);spec.loader.exec_module(ref)
expected=[]
def emit(name,body):expected.append((name,body))
for case in range(32):
    entries=(0,1,8,21,28,255,256)[case%7];base=0x7000220000+case*0x4000
    emit(f'{case}:state',ref.build_compute_shared_state(case))
    emit(f'{case}:table',ref.build_compute_operand_table(base,entries))
    emit(f'{case}:selected-table',ref.build_compute_operand_table_bases([base+i*0x123000 for i in range(entries)]))
    body=ref.build_compute_operand_page_lists(base,entries=(1,8,21,28,256)[case%5],buffer_size=(0x100000,0x30000,0x8000)[case%3],page_size=(0x1000,0x4000)[case%2])
    emit(f'{case}:lists',struct.pack('<I',len(body))+body)
    emit(f'{case}:full-list',ref.build_compute_operand_page_list(base))
    common=dict(resource_class=0x13+case,word_20=None if case%2==0 else 0xfeed1234,word_28=None if case%3==0 else 0xdead2345,
        cursor=case*8+0x98,field_54=case%4,field_5c=case%5,final_kind=case%3)
    emit(f'{case}:support',ref.build_compute_shared_support(base,0xfffffc2001630000+case*0x4000,word_08=case%3,header=case+1,**common))
    emit(f'{case}:compact',ref.build_compute_compact_control_support(case%3+1,base,0x7000250000+case*0x1000,0xfffffc2001630000+case*0x4000,
        active=case%3,header_value=case+1,**common))
    active=[(5,3,7,1)[i%4] for i in range(case%18)]
    emit(f'{case}:pool',ref.build_compute_class2_pool(base,0xfffffc2001630000,0xfffffc2001640000,record_count=(1,35,36,80,128)[case%5],index_base=0x808000+case*0x100,active=active))
    emit(f'{case}:pool-state',ref.build_compute_class2_pool_state(case+8,case%6))
    slots=0xfffffc2001640000+case*0x4000
    emit(f'{case}:seed',ref.build_compute_class2_predecessor_seed(slots))
    emit(f'{case}:active',ref.build_compute_class2_predecessor(slots,0xfffffc2000000000))
    emit(f'{case}:minimal',ref.build_compute_minimal_class2_predecessor(slots,0xfffffc2000000000))
    emit(f'{case}:active-slots',ref.build_compute_class2_predecessor_slots())
    emit(f'{case}:minimal-slots',ref.build_compute_minimal_class2_predecessor_slots())
with tempfile.TemporaryDirectory() as tmp:
    binary=Path(tmp)/'compute-memory'
    subprocess.run([args.rustc,'--edition=2021',str(Path(__file__).with_name('g17p_compute_memory.rs')),'-o',str(binary)],check=True)
    actual=subprocess.check_output([str(binary)])
offset=0
for name,body in expected:
    chunk=actual[offset:offset+len(body)]
    assert chunk==body,(name,[hex(i) for i,(a,b) in enumerate(zip(chunk,body)) if a!=b][:16],len(chunk),len(body))
    offset+=len(body)
assert offset==len(actual)
print(f'PASS: {len(expected)} complete source-built compute memory objects ({offset} bytes), padded page lists, allocator-selected bases, pool wrap, compact/ordinary support and predecessor profiles')
