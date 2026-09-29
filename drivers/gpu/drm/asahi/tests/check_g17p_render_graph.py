#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Source-only differential checks for G17P render allocator and context objects."""
import argparse
import importlib.util
from pathlib import Path
import struct
import subprocess
import tempfile

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1',type=Path)
parser.add_argument('--rustc',default='rustc')
args=parser.parse_args()
path=args.m1n1/'proxyclient/m1n1/agx/g17p_submission.py'
spec=importlib.util.spec_from_file_location('submission',path)
ref=importlib.util.module_from_spec(spec)
spec.loader.exec_module(ref)
expected=[]
def emit(name,body): expected.append((name,bytes(body)))
for case in range(64):
    pair=case%4
    work=(0,1,32767,8192)[case%4]
    slots=0xfffffc2001640004+case*0x4000
    shared=0xfffffc2001698040+case*0x4000
    emit(f'{case}:pool-a',ref.build_record_array_a(slots,work))
    emit(f'{case}:pool-b',ref.build_record_array_b(slots,shared,pair,work))
    pointers=(slots+0x10000,slots+0x18000,shared,slots+0x20000)
    emit(f'{case}:shared',ref.build_shared_object(pointers,pair,case%32+1,work))
    emit(f'{case}:context2-shared',ref.build_context2_shared_object(pointers,case))
    ranges=(((0x11,6),(0x4a,26)),((0x11,6),(0x3c,2)),((case*3+1,case%23),(case*9+0x30,case%32)))[case%3]
    for name,body in ref.build_submission_leaf_pages(pair,ranges,case%32+1,work).items(): emit(f'{case}:{name}',body)
    for kind in ('tiling','fragment'):
        emit(f'{case}:{kind}-optional',ref.build_optional_item(kind,
            0x7000000000+case*0x4000,slots,shared,shared+0x4000,
            tiling_shared_object=slots+0x8000 if kind=='tiling' else None,
            grid_index=case%12,item_index=case%37,submission_ordinal=case*173,
            context_id=None if case%3==0 else case%4,
            uuid=None if case%2==0 else case*131,
            scheduler_class=None if case%3==0 else case%7,
            queue_context_index=None if case%3==0 else case*307,
            queue_context_phase=None if case%3==0 else case*317,
            first_record=(None,False,True)[case%3],
            lifecycle_ordinal=None if case%3==0 else case*337,
            queue_namespace=None if case%3==0 else case*347,
            u16_overrides={} if case%2==0 else {0x1e:case+40,0x46:case+41,0xbe:0x5aa5}))
    emit(f'{case}:event-adjacent',ref.build_event_record(case*37,case*419,case*421)+b'\xa5'*0x3c0)
for count in (0,1,7,8,9,21,28,29,256):
    base=0x7000238000+count*0x4000
    emit(f'{count}:operand-table',ref.build_partial_operand_table(base,count))
    emit(f'{count}:operand-directory',ref.build_partial_operand_page_directory(base,count))
for case in range(256):
    for kind in ('tiling','fragment'):
        fragment=kind=='fragment'
        mode=case%8
        pair=case//8%2 if mode in (0,1) else 2 if mode==4 else 3 if mode==5 else 0
        item=0 if mode in (2,3) else (0,1,2,35,36,255,256,1024,0xffffffff)[case//8%9]
        context=1 if mode in (2,3) else 2 if mode in (4,6,7) else 3 if mode==5 else None
        base=0xfffffc20c00b0000 if fragment else 0xfffffc20c0018000
        body=ref.build_queue_context_item(kind,
            descriptor=0 if mode==0 else base+case%37*(0x2240 if fragment else 0x9c0),
            queue=0xfffffc20c0000000+case*0xc0,pair=pair,item_index=item,context_id=context,
            grid_index=12+int(fragment) if mode==5 else int(fragment) if mode==6 else None,
            locator_context_id=3 if mode==6 else None,partial_opening=mode==2,
            dependency_grid=(case//8%6)*2+int(fragment) if mode==3 else None,
            context_points=None if case%3==0 else tuple(((i*3+case)%128,case*1000+i) for i in range(case%32+1)),
            context_event_slot=case%128 if case%5==0 else None,
            completion_value=None if case%4==0 else case*503+1)
        emit(f'{case}:{kind}-context',body)
        previous=bytes((i*137+case)&255 for i in range(0x180))
        emit(f'{case}:{kind}-context-reuse',ref.update_queue_context_item(kind,previous,body))
        page=bytearray(0x4000)
        page[0x200:0x380]=body
        emit(f'{case}:{kind}-context-page',page)
        for pair in range(2):
            dependencies=ref.paired_queue_context_dependencies(kind,pair=pair,item_index=case)
            points=dependencies['context_points']
            emit(f'{case}:{kind}-dependencies-{pair}',bytes((dependencies['context_event_slot'],len(points)))+
                b''.join(struct.pack('<BI',queue,value) for queue,value in points))
with tempfile.TemporaryDirectory() as tmp:
    binary=Path(tmp)/'render-graph'
    subprocess.run([args.rustc,'--edition=2021','-Dwarnings',str(Path(__file__).with_name('g17p_render_graph.rs')),'-o',str(binary)],check=True)
    actual=subprocess.check_output([str(binary)])
offset=0
for name,body in expected:
    chunk=actual[offset:offset+len(body)]
    if chunk!=body:
        differences=[(hex(i),hex(a),hex(b)) for i,(a,b) in enumerate(zip(chunk,body)) if a!=b][:16]
        raise AssertionError((name,differences,len(chunk),len(body)))
    offset+=len(body)
assert offset==len(actual),(offset,len(actual))
print(f'PASS: {len(expected)} complete render graph/context objects ({offset} bytes), pools/leaf pages/operand directories, optional/event records, 512 varied contexts with dependencies and firmware-state-preserving reuse, bounds/overflow rejection')
