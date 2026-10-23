#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Differential Max publication/retirement gate against actual Python Publisher.

Compiles the production Rust transaction with bounded DATA memory, an identity
translation stand-in and counted ClientLease stand-ins. This checks protocol
ordering/retention, not native UAT, actual GEM/cache behavior or GPU execution.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import weakref

import host_versions
import host_vm

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1',type=Path)
p.add_argument('--evidence',type=Path,required=True)
args=p.parse_args();tests=Path(__file__).resolve().parent
sys.path.insert(0,str(args.m1n1/'proxyclient'))
from m1n1.g16c.transport import Publisher,Lane,Request,Requirements,Stamp,ring_room,ring_reached
from m1n1.g16c.firmware import Channel

QS=0xfffffc2208000000;ST=0xfffffc2000020000;RB=0xfffffc20c0800000
def route(i):
    a=ST+i*0x100
    return Channel(a,a+0x10,a+0x20,RB+i*0x2000)
class Owner: pass
class RAM:
    def __init__(self):
        self.ranges=[(QS,0x84000),(ST,0x4000),(RB,0x18000)]
        self.data={};self.trace=[];self.active=False;self.nw=self.ns=0;self.fw=self.fs=0
    def valid(self,a,n):
        assert any(lo<=a<=a+n<=lo+size for lo,size in self.ranges),(hex(a),n)
    def read(self,a,n):
        self.valid(a,n);return bytes(self.data.get(a+i,0) for i in range(n))
    def write(self,a,b):
        self.valid(a,len(b));self.data.update((a+i,v) for i,v in enumerate(b))
        if self.active:
            self.trace.append(f'W {a:x} {b.hex()}');self.nw+=1
            if self.nw==self.fw:raise OSError(5,'uncertain write after effect')
    def put(self,a,v):self.write(a,struct.pack('<I',v))
    def fence(self):
        if self.active:self.trace.append('F')
    def send(self,ep,msg):
        self.trace.append(f'S {ep:x} {msg:x}');self.ns+=1
        if self.ns==self.fs:raise OSError(5,'uncertain send after effect')

def expected(case,priority,stages,head,tail,mode,fw=0,fs=0,reject=0):
    m=RAM();pub=Publisher(m,[route(i)for i in range(12)],m.send,fence=m.fence)
    lanes=[Lane(QS+s*0x2c000,QS+s*0x2c000+0x4000,QS+s*0x2c000+0x8000,
                priority,s,capacity=1280,head=head,first=case%2==0)for s in stages]
    for l in lanes:
        r=route(priority*3+l.stage)
        for a in (l.pointers,l.pointers+0x30,l.pointers+0x40):m.put(a,head)
        for a in (r.read,r.write,r.completion):m.put(a,tail)
    target=0 if case%3==0 else 0x100
    for a in (QS+0x1040,QS+0x4200):m.put(a,(target-0x100)&0xffffffff)
    m.put(QS+0x4300,16);m.put(QS+0x4100,1)
    for a in (ST+0x2000,ST+0x2010,ST+0x2020):m.put(a,9)
    if reject==1:m.put(lanes[0].pointers+0x40,(head+1)%1280)
    if reject==2:m.put(lanes[0].pointers,(head+1280-639)%1280)
    if reject==3:m.put(route(priority*3+stages[0]).write,(tail+256-127)%256)
    if reject==4:m.put(QS+0x4200,target)
    if reject==5:lanes[1].stage=lanes[0].stage
    requests=[Request(l,(0xfffffc20d1000000,0xfffffc20d10000c0),77+l.stage,
                      tag=0xdeadbeef00000000+l.stage)for l in lanes]
    def requirements():
        return Requirements((Stamp(QS+0x1040,target),)if mode==0 else (),
            (Stamp(QS+0x4200,target),),(QS+0x4100,)if mode==0 else (),
            drains=()if mode==0 else(Channel(ST+0x2000,ST+0x2010,ST+0x2020,RB),),
            counters=()if mode==0 else(Stamp(QS+0x4300,17),))
    owner=Owner();weak=weakref.ref(owner);m.active=True;m.fw=fw;m.fs=fs
    try:flight=pub.submit(requests,requirements(),retained=(owner,));ok=True
    except (ValueError,OSError):ok=False;flight=pub.flights[-1]if pub.flights else None
    m.active=False;del owner
    rows=[f'CASE {case}',*m.trace,f'RESULT {"OK"if ok else"ERR"} {int(pub.poisoned)} {int(weak()is not None)}',
          'HEADS '+','.join(f'{l.head}:{int(l.first)}'for l in lanes)]
    def poll():
        f=pub.poll(flight);rows.append('P '+('E'if f.error else str(int(f.quiescent))))
    if ok:
        poll()
        for a in (QS+0x1040,QS+0x4200):m.put(a,target)
        poll()
        for l in lanes:
            for a in(l.pointers,l.pointers+0x30):m.put(a,l.head)
        poll()
        for l in lanes:
            r=route(l.priority*3+l.stage)
            for a in(r.read,r.write):m.put(a,(tail+1)%256)
        poll()
        m.put(QS+0x4100,0);m.put(QS+0x4300,17);m.put(ST+0x2010,8);poll()
        m.put(ST+0x2010,9);poll()
        pub.release(flight);assert weak()is None;rows.append('RELEASE 0')
    elif pub.poisoned:
        assert weak()is not None
        try:pub.submit(requests,requirements())
        except OSError:pass
        else:raise AssertionError('poisoned publisher retried')
        try:pub.release(flight)
        except OSError:pass
        else:raise AssertionError('uncertain flight released')
        rows.append('RETAIN 1')
    else:assert weak()is None
    return rows

def live(case,kind):
    m=RAM();pub=Publisher(m,[route(i)for i in range(12)],m.send,fence=m.fence)
    lane=Lane(QS,QS+0x4000,QS+0x8000,2,2,capacity=1280)
    request=Request(lane,(0xfffffc20d1000000,0xfffffc20d10000c0),77)
    def req(target):return Requirements((Stamp(QS+0x1040,target),),(Stamp(QS+0x4200,target),),(QS+0x4100,))
    m.put(QS+0x4100,1);a=Owner();w1=weakref.ref(a)
    first=pub.submit((request,),req(0x100),retained=(a,));del a
    r=route(8)
    if kind==3:
        lane.head=641
        for a in (lane.pointers,lane.pointers+0x30,lane.pointers+0x40):m.put(a,641)
    if kind==4:
        for a in (r.read,r.write,r.completion):m.put(a,128)
    target={1:0x100,2:0x80}.get(kind,0x200)
    m.trace=[];m.ns=m.nw=0;m.active=True;b=Owner();w2=weakref.ref(b)
    try:second=pub.submit((request,),req(target),retained=(b,));ok=True
    except(ValueError,OSError):second=None;ok=False
    m.active=False;del b
    rows=[f'LIVE {case}',*m.trace,f'LIVE_RESULT {"OK"if ok else"ERR"} {int(pub.poisoned)} {int(w1()is not None)} {int(w2()is not None)}']
    for a in (QS+0x1040,QS+0x4200):m.put(a,0x200 if ok else 0x100)
    m.put(QS+0x4100,0)
    for a in (lane.pointers,lane.pointers+0x30):m.put(a,lane.head)
    tail=struct.unpack('<I',m.read(r.completion,4))[0]
    for a in (r.read,r.write):m.put(a,tail)
    pub.release(first);rows.append(f'LIVE_RELEASE {int(w1()is not None)} {int(w2()is not None)}')
    if ok:pub.release(second);rows.append(f'LIVE_RELEASE {int(w1()is not None)} {int(w2()is not None)}')
    return rows

want=[]
for i in range(16):
    stages=([2],[0,1],[2,0,1])[i%3]
    want+=expected(i,i%4,stages,[0,1279,1278,640][i%4],[0,255,254,128][i%4],i%2)
for n in range(1,16):want+=expected(100+n,2,[2,0,1],1279,255,0,fw=n)
for n in range(1,5):want+=expected(200+n,2,[2,0,1],1279,255,0,fs=n)
for n in range(1,6):want+=expected(300+n,2,[2,0,1],0,0,0,reject=n)
for n in range(5):want+=live(400+n,n)
for cap in [4,256,1280,65536]:
    for tail in [0,1,cap-1]:
        for delta in [0,1,cap//2-2,cap//2-1,cap//2]:
            read=(tail+cap-delta)%cap
            for entries in [0,1,2,cap//2-1,cap]:want.append(f'ROOM {cap} {tail} {read} {entries} {int(ring_room(tail,read,tail,entries,cap))}')
            want.append(f'REACHED {cap} {tail} {read} {int(ring_reached(tail,read,cap))}')

with tempfile.TemporaryDirectory(prefix='scheduled-transport-')as directory:
    tmp=Path(directory);flags=host_versions.build(tmp)
    code=host_vm.prelude().replace('pub fn sync(){}','pub fn sync(){crate::trace::fence();}')
    code+='\nmod module_parameters {\n    pub static FAIL:std::sync::atomic::AtomicU32=std::sync::atomic::AtomicU32::new(0);\n    pub struct Parameter;\n    impl Parameter {pub fn value(&self)->&u32{if FAIL.load(std::sync::atomic::Ordering::Relaxed)==9{&9}else{&0}}}\n    #[allow(non_upper_case_globals)]pub static g16c_initial_alloc_fail:Parameter=Parameter;\n}\n'
    code=code.replace('pub fn write(&mut self,base:u64,b:&[u8])->Result{self.write_inner(base,b)}',
        'pub fn write(&mut self,base:u64,b:&[u8])->Result{self.write_inner(base,b)?;crate::trace::write(base,b)}')
    code+='''
mod g17p_initgraph {pub struct Graph;}
mod g17p_compute_runtime {pub struct ClientLease {pub owner:(u64,u32),pub pin:std::sync::Arc<()>}}
mod g17p_vm {
    use super::{g17p_memory::Memory,prelude::*};pub struct Vm;
    impl Vm {
        pub fn require_g16c_graph(&self)->Result{Ok(())}
        pub fn physical(&self,m:&Memory,group:usize,a:u64)->Result<u64>{assert_eq!(group,2);m.read(a,1)?;Ok(a)}
        pub fn write(&self,m:&mut Memory,group:usize,a:u64,b:&[u8])->Result{assert_eq!(group,2);m.write(a,b)}
    }
}
mod trace {
    use super::prelude::*;use std::cell::RefCell;
    #[derive(Default)]struct Log{rows:Vec<String>,active:bool,nw:usize,ns:usize,fw:usize,fs:usize}
    thread_local!{static LOG:RefCell<Log>=RefCell::new(Log::default());}
    pub fn start(fw:usize,fs:usize){LOG.with(|l|*l.borrow_mut()=Log{active:true,fw,fs,..Default::default()});}
    pub fn stop(){LOG.with(|l|l.borrow_mut().active=false);}
    pub fn rows()->Vec<String>{LOG.with(|l|l.borrow().rows.clone())}
    pub fn write(a:u64,b:&[u8])->Result{LOG.with(|l|{let mut l=l.borrow_mut();if !l.active{return Ok(());}
        l.rows.push(format!("W {a:x} {}",b.iter().map(|b|format!("{b:02x}")).collect::<String>()));l.nw+=1;
        if l.nw==l.fw{Err(EIO)}else{Ok(())}})}
    pub fn fence(){LOG.with(|l|{let mut l=l.borrow_mut();if l.active{l.rows.push("F".into());}});}
    pub fn send(ep:u8,msg:u64)->Result{LOG.with(|l|{let mut l=l.borrow_mut();l.rows.push(format!("S {ep:x} {msg:x}"));l.ns+=1;
        if l.ns==l.fs{Err(EIO)}else{Ok(())}})}
}
'''
    lane=re.search(r'pub\(crate\) struct LaneG16C\s*\{[^}]*\}',(tests.parent/'scheduled_queues.rs').read_text()).group()
    code+='mod scheduled_queues {\n'+lane+'\n}\n'
    for name in ['scheduled_initdata','g17p_queue','scheduled_transport']:
        code+=f'#[path={json.dumps(str(tests.parent/(name+".rs")))}]mod {name};\n'
    code+=(tests/'scheduled_transport.rs').read_text()
    (tmp/'main.rs').write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'main.rs'),*flags,'-o',str(tmp/'gate')],check=True)
    raw=subprocess.check_output([str(tmp/'gate')],text=True).splitlines()
    assert raw[-1]=='PREPUBLICATION_ENOMEM_PASS no_stores=1 no_sends=1 same_state_retry=1 id=1'
    actual=raw[:-1]
    assert len(actual)==len(want),(len(actual),len(want))
    for i,(a,b)in enumerate(zip(actual,want)):
        assert a==b,(i,a,b,actual[max(0,i-3):i+3],want[max(0,i-3):i+3])
    args.evidence.mkdir(parents=True,exist_ok=False)
    trace='\n'.join(actual)+'\n';(args.evidence/'trace.txt').write_text(trace)
    summary=dict(positive_publications=22,
        prepublication_ENOMEM_no_stores_or_sends=True,
        prepublication_ENOMEM_drops_unpublished_owner=True,
        same_transport_retry_reuses_first_id=True,multiple_live_flight_scenarios=5,
        independent_live_retirements=True,live_stamp_and_half_ring_rejections=4,
        post_write_failure_cases=15,post_send_failure_cases=4,
        rejected_before_writes=5,ring_room_boundary_cases=300,ring_reached_boundary_cases=60,
        retirement_polls=96,ClientLease_retention_and_release_checked=True,
        source_Publisher_submit_poll_release_executed=True,complete_effect_trace_equal=True,
        trace_sha256=hashlib.sha256(trace.encode()).hexdigest(),
        bounded_DATA_and_translation_standins=True,actual_native_GEM_or_GPU_execution_tested=False,
        source_sha256={str(f):hashlib.sha256(f.read_bytes()).hexdigest()for f in[
            tests.parent/'scheduled_transport.rs',tests.parent/'g17p_queue.rs',
            args.m1n1/'proxyclient/m1n1/g16c/transport.py']})
    (args.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print('PASS: actual Publisher traces/retirement match; 22 positive publications, 19 uncertain-effect failures, '
      '9 pre-write/live-flight rejects, 360 ring boundaries and independent counted lease retirement')
