// SPDX-License-Identifier: GPL-2.0-only
// Included after the bounded DATA fixture and production modules.
use prelude::*;
use scheduled_transport::{StateG16C,Request,Requirements,Stamp,Route};
use scheduled_queues::LaneG16C;
use g17p_memory::Memory;
use std::sync::{Arc,Weak};
const QS:u64=0xfffffc2208000000;
const ST:u64=0xfffffc2000020000;
const RB:u64=0xfffffc20c0800000;
fn kv<T>(values:impl IntoIterator<Item=T>)->KVec<T>{let mut v=KVec::new();for x in values{v.push(x,0).unwrap();}v}
fn put(m:&mut Memory,a:u64,v:u32){m.write32(a,v).unwrap();}
fn route(index:usize)->Route{let s=ST+index as u64*0x100;Route{read:s,done:s+0x10,producer:s+0x20,ring:RB+index as u64*0x2000}}
fn requirements(mode:u32,target:u32)->Requirements {
    Requirements{firmware:kv(if mode==0{vec![Stamp{address:QS+0x1040,value:target}]}else{vec![]}),
        driver:kv([Stamp{address:QS+0x4200,value:target}]),notifiers:kv(if mode==0{vec![QS+0x4100]}else{vec![]}),
        counters:kv(if mode==0{vec![]}else{vec![Stamp{address:QS+0x4300,value:17}]}),
        drains:kv(if mode==0{vec![]}else{vec![Route{read:ST+0x2000,done:ST+0x2010,producer:ST+0x2020,ring:RB}]})}
}
fn owners()->(KVec<g17p_compute_runtime::ClientLease>,Weak<()>) {
    let a=Arc::new(());let w=Arc::downgrade(&a);(kv([g17p_compute_runtime::ClientLease{owner:(31,7),pin:a}]),w)
}
fn status(result:Result<bool>)->&'static str{match result{Ok(true)=>"1",Ok(false)=>"0",Err(_)=>"E"}}
fn run(case:u32,priority:u8,stages:&[u8],head:u32,tail:u32,mode:u32,fail_write:usize,fail_send:usize,reject:u32) {
    let vm=g17p_vm::Vm;let mut m=Memory::new();m.reserve(QS,0x84000);m.reserve(ST,0x4000);m.reserve(RB,0x18000);
    let channels:Vec<_>=(0..12).map(|i|{let r=route(i);scheduled_initdata::Channel{
        states:[r.read,r.done,r.producer].map(u64::to_le_bytes),ring:r.ring.to_le_bytes()}}).collect();
    let mut state=StateG16C::new(&channels).unwrap();
    let mut lanes:Vec<_>=stages.iter().map(|&stage|{let a=QS+stage as u64*0x2c000;LaneG16C{
        queue:a,pointers:a+0x4000,ring:a+0x8000,scratch_high:a+0xc000,scratch_low:0x7400000000,
        capacity:1280,head,first:case%2==0,priority,stage}}).collect();
    for l in &lanes {let r=route((priority*3+l.stage) as usize);
        put(&mut m,l.pointers,head);put(&mut m,l.pointers+0x30,head);put(&mut m,l.pointers+0x40,head);
        for a in [r.read,r.done,r.producer]{put(&mut m,a,tail);}}
    let target:u32=if case%3==0{0}else{0x100};
    put(&mut m,QS+0x1040,target.wrapping_sub(0x100));put(&mut m,QS+0x4200,target.wrapping_sub(0x100));
    put(&mut m,QS+0x4300,16);put(&mut m,QS+0x4100,1);
    for a in [ST+0x2000,ST+0x2010,ST+0x2020]{put(&mut m,a,9);}
    if reject==1 {put(&mut m,lanes[0].pointers+0x40,(head+1)%1280);}
    if reject==2 {put(&mut m,lanes[0].pointers,(head+1280-639)%1280);}
    if reject==3 {put(&mut m,route((priority*3+stages[0]) as usize).done,(tail+256-127)%256);}
    if reject==4 {put(&mut m,QS+0x4200,target);}
    if reject==5 {lanes[1].stage=lanes[0].stage;}
    let works=[0xfffffc20d1000000,0xfffffc20d10000c0];
    let mut requests:Vec<_>=lanes.iter_mut().map(|lane|Request{event:77+lane.stage,tag:0xdeadbeef00000000+lane.stage as u64,lane,works:&works}).collect();
    let (keep,weak)=owners();trace::start(fail_write,fail_send);
    let result=state.submit(&mut m,&vm,&mut requests,requirements(mode,target),keep,trace::send);
    trace::stop();
    assert_eq!(state.retains_owner((31,7)), result.is_ok() || state.poisoned());
    assert!(!state.retains_owner((31,8)));
    println!("CASE {case}");for row in trace::rows(){println!("{row}");}
    println!("RESULT {} {} {}",if result.is_ok(){"OK"}else{"ERR"},state.poisoned() as u8,weak.strong_count());
    println!("HEADS {}",requests.iter().map(|r|format!("{}:{}",r.lane.head,r.lane.first as u8)).collect::<Vec<_>>().join(","));
    if let Ok(id)=result {
        println!("P {}",status(state.poll(&m,&vm,id)));
        put(&mut m,QS+0x1040,target);put(&mut m,QS+0x4200,target);
        println!("P {}",status(state.poll(&m,&vm,id)));
        for r in &requests {put(&mut m,r.lane.pointers,r.lane.head);put(&mut m,r.lane.pointers+0x30,r.lane.head);}
        println!("P {}",status(state.poll(&m,&vm,id)));
        for r in &requests {let rr=route((r.lane.priority*3+r.lane.stage) as usize);
            for a in [rr.read,rr.done]{put(&mut m,a,(tail+1)%256);}}
        println!("P {}",status(state.poll(&m,&vm,id)));
        put(&mut m,QS+0x4100,0);put(&mut m,QS+0x4300,17);put(&mut m,ST+0x2010,8);
        println!("P {}",status(state.poll(&m,&vm,id)));
        put(&mut m,ST+0x2010,9);
        println!("P {}",status(state.poll(&m,&vm,id)));
        state.release(&m,&vm,id).unwrap();assert!(!state.retains_owner((31,7)));assert_eq!(weak.strong_count(),0);println!("RELEASE 0");
    } else if state.poisoned() {
        assert_eq!(weak.strong_count(),1);
        let (keep,_)=owners();assert!(state.submit(&mut m,&vm,&mut requests,requirements(mode,target),keep,trace::send).is_err());
        assert!(state.release(&m,&vm,1).is_err());println!("RETAIN {}",weak.strong_count());
    } else {assert_eq!(weak.strong_count(),0);}
}
fn live(case:u32,kind:u32) {
    let vm=g17p_vm::Vm;let mut m=Memory::new();m.reserve(QS,0x84000);m.reserve(ST,0x4000);m.reserve(RB,0x18000);
    let channels:Vec<_>=(0..12).map(|i|{let r=route(i);scheduled_initdata::Channel{
        states:[r.read,r.done,r.producer].map(u64::to_le_bytes),ring:r.ring.to_le_bytes()}}).collect();
    let mut state=StateG16C::new(&channels).unwrap();let r=route(8);
    let mut lane=LaneG16C{queue:QS,pointers:QS+0x4000,ring:QS+0x8000,scratch_high:QS+0xc000,
        scratch_low:0x7400000000,capacity:1280,head:0,first:true,priority:2,stage:2};
    put(&mut m,QS+0x4100,1);
    let works=[0xfffffc20d1000000,0xfffffc20d10000c0];
    let (keep,w1)=owners();
    let first=state.submit(&mut m,&vm,&mut [Request{lane:&mut lane,works:&works,event:77,tag:0}],requirements(0,0x100),keep,trace::send).unwrap();
    if kind==3 {lane.head=641;for a in [lane.pointers,lane.pointers+0x30,lane.pointers+0x40]{put(&mut m,a,641);}}
    if kind==4 {for a in [r.read,r.done,r.producer]{put(&mut m,a,128);}}
    let target=match kind{1=>0x100,2=>0x80,_=>0x200};
    let (keep,w2)=owners();trace::start(0,0);
    let second=state.submit(&mut m,&vm,&mut [Request{lane:&mut lane,works:&works,event:77,tag:0}],requirements(0,target),keep,trace::send);
    trace::stop();println!("LIVE {case}");for row in trace::rows(){println!("{row}");}
    println!("LIVE_RESULT {} {} {} {}",if second.is_ok(){"OK"}else{"ERR"},state.poisoned()as u8,w1.strong_count(),w2.strong_count());
    let stamp=if second.is_ok(){0x200}else{0x100};for a in [QS+0x1040,QS+0x4200]{put(&mut m,a,stamp);}
    put(&mut m,QS+0x4100,0);for a in [lane.pointers,lane.pointers+0x30]{put(&mut m,a,lane.head);}
    let tail=m.read_firmware32(r.producer).unwrap();for a in [r.read,r.done]{put(&mut m,a,tail);}
    state.release(&m,&vm,first).unwrap();println!("LIVE_RELEASE {} {}",w1.strong_count(),w2.strong_count());
    if let Ok(second)=second{state.release(&m,&vm,second).unwrap();println!("LIVE_RELEASE {} {}",w1.strong_count(),w2.strong_count());}
}
fn unpublished_allocation_failure() {
    let vm=g17p_vm::Vm;let mut m=Memory::new();m.reserve(QS,0x84000);m.reserve(ST,0x4000);m.reserve(RB,0x18000);
    let channels:Vec<_>=(0..12).map(|i|{let r=route(i);scheduled_initdata::Channel{
        states:[r.read,r.done,r.producer].map(u64::to_le_bytes),ring:r.ring.to_le_bytes()}}).collect();
    let mut state=StateG16C::new(&channels).unwrap();
    let mut lane=LaneG16C{queue:QS,pointers:QS+0x4000,ring:QS+0x8000,scratch_high:QS+0xc000,
        scratch_low:0x7400000000,capacity:1280,head:0,first:true,priority:2,stage:2};
    put(&mut m,QS+0x4100,1);let before=m.snapshot(&[(QS,0x84000),(ST,0x4000),(RB,0x18000)]);
    let works=[0xfffffc20d1000000,0xfffffc20d10000c0];let(keep,weak)=owners();
    module_parameters::FAIL.store(9,std::sync::atomic::Ordering::Relaxed);trace::start(0,0);
    let result=state.submit(&mut m,&vm,&mut[Request{lane:&mut lane,works:&works,event:77,tag:0}],requirements(0,0x100),keep,trace::send);
    trace::stop();assert_eq!(result,Err(ENOMEM));assert!(trace::rows().is_empty());assert!(!state.poisoned());
    assert_eq!(lane.head,0);assert!(lane.first);assert_eq!(weak.strong_count(),0);
    assert_eq!(m.snapshot(&[(QS,0x84000),(ST,0x4000),(RB,0x18000)]),before);
    module_parameters::FAIL.store(0,std::sync::atomic::Ordering::Relaxed);let(keep,prepared_owner)=owners();
    trace::start(0,0);
    let prepared=state.prepare(&mut m,&vm,&mut[Request{lane:&mut lane,works:&works,event:77,tag:0}],requirements(0,0x100),keep).unwrap();
    trace::stop();assert!(trace::rows().is_empty());assert_eq!(prepared_owner.strong_count(),1);
    assert!(state.retains_owner((31,7)));assert_eq!(lane.head,0);assert!(lane.first);
    assert_eq!(m.snapshot(&[(QS,0x84000),(ST,0x4000),(RB,0x18000)]),before);
    let id=state.publish(&mut m,&vm,&mut[Request{lane:&mut lane,works:&works,event:77,tag:0}],prepared,trace::send).unwrap();
    assert_eq!(id,1);assert_eq!(lane.head,2);assert!(!lane.first);
    println!("PREPUBLICATION_ENOMEM_PASS no_stores=1 no_sends=1 same_state_retry=1 id=1");
}
fn main(){
    for case in 0u32..16 {let p=(case%4) as u8;let stages:&[u8]=match case%3{0=>&[2],1=>&[0,1],_=>&[2,0,1]};
        run(case,p,stages,[0,1279,1278,640][case as usize%4],[0,255,254,128][case as usize%4],case%2,0,0,0);}
    for n in 1..=15 {run(100+n,2,&[2,0,1],1279,255,0,n as usize,0,0);}
    for n in 1..=4 {run(200+n,2,&[2,0,1],1279,255,0,0,n as usize,0);}
    for n in 1..=5 {run(300+n,2,&[2,0,1],0,0,0,0,0,n);}
    for n in 0..5{live(400+n,n);}
    for cap in [4u32,256,1280,65536] {for tail in [0,1,cap-1] {for delta in [0,1,cap/2-2,cap/2-1,cap/2] {
        let read=(tail+cap-delta)%cap;
        for entries in [0usize,1,2,(cap/2-1) as usize,cap as usize] {
            println!("ROOM {cap} {tail} {read} {entries} {}",scheduled_transport::ring_room(tail,read,tail,entries,cap) as u8);
        }
        println!("REACHED {cap} {tail} {read} {}",scheduled_transport::ring_reached(tail,read,cap) as u8);
    }}}
    unpublished_allocation_failure();
}
