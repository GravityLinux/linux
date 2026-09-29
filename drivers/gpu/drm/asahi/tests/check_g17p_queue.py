#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Compare ordered Rust publication writes with actual G17PSubmitter.stage."""
import argparse,ast,importlib.util,struct,subprocess,tempfile,sys
from pathlib import Path
from types import SimpleNamespace as NS
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1',type=Path)
parser.add_argument('--rustc',default='rustc')
args=parser.parse_args()
source=args.m1n1/'proxyclient/m1n1/agx'
spec=importlib.util.spec_from_file_location('g17p',source/'g17p.py')
g17p=importlib.util.module_from_spec(spec);spec.loader.exec_module(g17p)
parsed=ast.parse((source/'g17p_backend.py').read_text())
ns=dict(struct=struct,g17p=g17p)
exec(compile(ast.Module(body=[n for n in parsed.body if isinstance(n,ast.ClassDef) and n.name in ('G17PSubmitter','G17PChannels')],type_ignores=[]),'g17p_backend.py','exec'),ns)
# Execute the runtime's actual mailbox register definitions, independently of
# the Rust literal. The firmware's channel-table index is a separate value.
sys.path.insert(0, str(args.m1n1/'proxyclient'))
from m1n1.utils import Register64
mailbox_ast=ast.parse((args.m1n1/'proxyclient/experiments/agx_g17p_boot.py').read_text())
mailbox_ns=dict(Register64=Register64)
exec(compile(ast.Module(body=[n for n in mailbox_ast.body if isinstance(n,ast.ClassDef) and n.name in ('GpuMsg','DoorbellMsg')],type_ignores=[]),'agx_g17p_boot.py','exec'),mailbox_ns)
mailbox=mailbox_ns['DoorbellMsg'](TYPE=g17p.MSG_WORK_DOORBELL, CHANNEL=0x0a)
expected=[f'compute-mailbox {int(mailbox.value):x}']
for case in range(192):
    kind=('tiling','fragment','compute')[case%3]
    producer=(0,1,254,255)[case%4]
    count=(1,3,9)[case%3]
    items=[0xfffffc20c1000000+i*0x4000 for i in range(count)]
    writes=[]
    queue=NS(address=0xfffffc20c0000000,pointers_addr=0xfffffc2000400000,item_ring=0xfffffc20c0040000,grid_index=case%12,
        indices=lambda:dict(write=12))
    entry=dict(ring_addr=0xfffffc20c0060000,state_addrs=[0,0,0xfffffc2000410020])
    channel=NS(next_free_slot=lambda _:producer,counters=lambda _:[producer]*3)
    stage=ns['G17PSubmitter'](None,lambda at,data:writes.append((at,bytes(data))),None,channel)
    inner=[] if case&2 else None
    stage.deferred_producers=[] if case&4 else None
    pub=stage.stage(entry,queue,items,case*129,kind=kind,first_submit=bool(case&1),in_place=bool(case&16),announce=bool(case&32),
        queue_producer_writes=inner,event_subtype=0x1001a if case&64 else None,
        event_counter=0xfedcba98 if case%11==0 else None,event_counter_low=case&7)
    expected.append(f'case {case}')
    expected.extend(f'write {at:x} {data.hex()}' for at,data in writes)
    for name,deferred in [('inner',inner),('outer',stage.deferred_producers)]:
        expected.extend(f'{name} {at:x} {data.hex()}' for at,data in deferred or [])
    record=g17p.build_queue_record(queue.pointers_addr,queue.item_ring,0xfffffc2000000000,0xfffffc20c07b8000,
        uuid=case*913,priority=case%5,prio5=case%3,unk_2c=case<<16,unk_38=case&1,
        unk_30=0xfffe0000deadbeef if case&8 else None,unk_94=case*734,sentinel_size=case%7)
    expected.extend(['record '+record.hex(),'pointers '+g17p.build_queue_pointers((case-1)&0xffffffff).hex(),
        'jobs '+g17p.build_job_list(0xfffffc2000000000+case*0x4000).hex(),f'meta {pub["slot"]} {pub["producer"]} {pub["write_after"]}'])

digest=0xcbf29ce484222325
def add(value):
    global digest
    digest=((digest^value)*0x100000001b3)&0xffffffffffffffff
for producer in (0,1,127,128,254,255):
    for a in range(256):
        for b in range(256):
            view=NS(counters=lambda _:[a,b,producer])
            add(ns['G17PChannels'].available_slots(view,None))
            try:ns['G17PChannels'].next_free_slot(view,None);valid=1
            except RuntimeError:valid=0
            add(valid)
            for distance in (1,17,255):add(g17p.producer_reached(producer,a,(producer+distance)&255))
expected.append(f'digest {digest:x}')
with tempfile.TemporaryDirectory() as temp:
    binary=Path(temp)/'queue'
    subprocess.run([args.rustc,'--edition=2021',str(Path(__file__).with_name('g17p_queue.rs')),'-o',str(binary)],check=True)
    actual=subprocess.check_output([str(binary)],text=True).splitlines()
assert len(actual)==len(expected),(len(actual),len(expected))
for index,(a,b) in enumerate(zip(actual,expected)):
    assert a==b,(index,a,b)
print('PASS: 192 ordered Python/Rust publication traces, queue/pointer/job/event/ring objects; 393216 dual-consumer ring states; pre-store rejection and completion checks')
