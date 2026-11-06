#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute the shared retained-context selector and actual cross-pool lease gate."""
import argparse,hashlib,json,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
r=Path(__file__).resolve().parents[1];sources={n:(r/n).read_text() for n in ['g17p_render_lifecycle.rs','g17p_boot.rs','g17p_render_runtime.rs']}
def item(name,marker):
 s=sources[name];start=s.index(marker);end=s.index('{',start)+1;depth=1
 while depth:depth+=(s[end]=='{')-(s[end]=='}');end+=1
 return s[start:end]
code=r'''
#![allow(dead_code,non_upper_case_globals)]
use std::sync::atomic::{AtomicI32,Ordering};use std::cell::Cell;
mod module_parameters{
 use super::*;
 thread_local!{pub static SETTINGS:Cell<[u32;7]>=const{Cell::new([2,1,0,0,0,0,0])};}
 pub struct P(usize);impl P{pub fn value(&self)->u32{SETTINGS.with(|v|v.get()[self.0])}}
 pub static render_cache_slots:P=P(0);pub static compute_queues:P=P(1);
 pub static native_compute_vms:P=P(2);pub static native_render_vms:P=P(3);
 pub static native_barriers:P=P(4);pub static partial_independent_owner:P=P(5);
 pub static alternate_queue_pairs:P=P(6);
}
mod life{pub const POOL_SLOTS:u32=15;}
mod render{use super::*;
'''+item('g17p_render_runtime.rs','pub(crate) fn serialized_cache_enabled(')+'\n'+item('g17p_render_runtime.rs','pub(crate) fn pool_count(')+'\n}\n'
code+=item('g17p_render_lifecycle.rs','pub(crate) fn select_affine_pool(')+'\n'+item('g17p_render_lifecycle.rs','pub(crate) fn select_pool(')+'\n'+item('g17p_render_lifecycle.rs','pub(crate) fn select_cached_pool(')+r'''
struct Fence(AtomicI32);impl Fence{fn raw(&self)->*const AtomicI32{&self.0}}
mod kernel{pub mod bindings{pub unsafe fn dma_fence_get_status(p:*const super::super::AtomicI32)->i32{unsafe{(*p).load(super::super::Ordering::Acquire)}}}}
struct Wave(Vec<u32>);impl Wave{fn pool_busy(&self,p:u32)->bool{self.0.contains(&p)}}
enum PendingWork{Render(Wave),WrappedRender(Wave),Control}
struct Session{automatic_render_parallel:bool,render_cache_clock:u64,render_cache_used:[u64;15],render_cache_hits:[u8;15],render_cache_cost:[u16;15],render_cache_owner:[Option<(u64,u32)>;15],pending:Option<PendingWork>,render_cpu_leases:Vec<(u32,Fence)>,preparing:u32}
impl Session{
 fn render_preparing(&self,p:u32)->bool{self.preparing&(1<<p)!=0}
 fn live_render_count(&self)->usize{match &self.pending{Some(PendingWork::Render(w)|PendingWork::WrappedRender(w))=>w.0.len(),_=>0}}
'''+item('g17p_boot.rs','    fn render_pool_available(')+'\n'+item('g17p_boot.rs','    fn retained_render_idle(')+'\n'+item('g17p_boot.rs','    fn touch_render_cache(')+r'''
}
fn main(){
 // Exhaust every availability/match subset for four owners; compare the
 // original simple specification with the actual common selector.
 let ages=[4,1,9,0];let mut checks=0;
 for free in 0..16u32{for exact in 0..16u32{for next in 0..4{
  let got=select_cached_pool(next,4,|p|free&(1<<p)!=0,|p|exact&(1<<p)!=0,&ages);
  let order:Vec<u32>=(0..4).map(|i|(next+i)%4).collect();
  let expected=order.iter().copied().find(|p|free&exact&(1<<p)!=0).or_else(||order.iter().copied().filter(|p|free&(1<<p)!=0).min_by_key(|p|ages[*p as usize]));
  assert_eq!(got,expected);
  let expected_rr=order.iter().copied().find(|p|free&exact&(1<<p)!=0)
      .or_else(||order.iter().copied().find(|p|free&(1<<p)!=0));
  assert_eq!(select_pool(next,4,|p|free&(1<<p)!=0,|p|exact&(1<<p)!=0),expected_rr);
  checks+=1;
 }}}
 assert_eq!(select_cached_pool(0,0,|_|true,|_|true,&[]),None);
 assert_eq!(select_cached_pool(0,5,|_|true,|_|true,&ages),None);
 // An exact retained caller should not inspect unrelated owners' fences.
 for next in 0..15 {
  let probes=Cell::new(0);let available=|_:&u32|{probes.set(probes.get()+1);true};
  assert_eq!(select_pool(next,15,available,|p|*p==14),Some(14));assert_eq!(probes.get(),1);
  probes.set(0);
  assert_eq!(select_cached_pool(next,15,available,|p|*p==14,&[0;15]),Some(14));assert_eq!(probes.get(),1);
 }
 // Keep one physical pool across immutable binding snapshots. A leased
 // resident is preferred over an unrelated context when retired.
 for free in 0..16u32{for exact in 0..16u32{for owned in 0..16u32{for next in 0..4 {
  let order:Vec<u32>=(0..4).map(|i|(next+i)%4).collect();
  for hits in [[0;4],[0,9,2,1]] {
   let expected=order.iter().copied().find(|p|free&exact&(1<<p)!=0)
    .or_else(||order.iter().copied().find(|p|free&owned&(1<<p)!=0))
    .or_else(||if owned!=0 {None}else {order.iter().copied().filter(|p|free&(1<<p)!=0)
      .min_by_key(|p|(hits[*p as usize],ages[*p as usize]))});
   assert_eq!(select_affine_pool(next,4,|p|free&(1<<p)!=0,|p|exact&(1<<p)!=0,
    |p|owned&(1<<p)!=0,&hits,&[1;4],&ages),expected);
  }
 }}}}
 let mut s=Session{automatic_render_parallel:false,render_cache_clock:0,render_cache_used:[0;15],render_cache_hits:[0;15],render_cache_cost:[1;15],render_cache_owner:[None;15],pending:None,render_cpu_leases:vec![],preparing:0};
 assert!(s.render_pool_available(0)&&s.render_pool_available(1));
 for wrapped in [false,true]{
  s.pending=Some(if wrapped{PendingWork::WrappedRender(Wave(vec![0]))}else{PendingWork::Render(Wave(vec![0]))});
  assert!(!s.render_pool_available(0));assert!(!s.render_pool_available(1),"a cached different PB must not overlap the live owner");
 }
 s.pending=None;s.render_cpu_leases.push((0,Fence(AtomicI32::new(0))));assert!(!s.render_pool_available(1),"CPU visibility outlives firmware retirement");
 for status in [1,-5]{s.render_cpu_leases[0].1.0.store(status,Ordering::Release);assert!(s.render_pool_available(1));}
 s.preparing=2;assert!(!s.render_pool_available(1));assert!(s.render_pool_available(0));
 // Protect a hot working set from a short oversubscribed stream, then
 // decay inactive history so new long-lived workloads can take its place.
 for pool in 0..14 {for _ in 0..30{s.touch_render_cache(pool,(1,pool),1);}}
 s.touch_render_cache(14,(1,14),1);
 for i in 0..600 {
  for pool in 0..14{s.touch_render_cache(pool,(1,pool),1);}
  let owner=(1,14+i%3);
  let selected=select_affine_pool(i,15,|_|true,|p|s.render_cache_owner[*p as usize]==Some(owner),
    |p|s.render_cache_owner[*p as usize]==Some(owner),&s.render_cache_hits,&s.render_cache_cost,&s.render_cache_used).unwrap();
  assert_eq!(selected,14);s.touch_render_cache(selected,owner,1);
 }
 assert_eq!(select_affine_pool(0,4,|_|true,|_|false,|_|false,&[255,8,8,8],&[1;4],&[1,90,99,100]),Some(0));
 for pool in 0..14{assert_eq!(s.render_cache_owner[pool],Some((1,pool as u32)));}
 for _ in 0..8192{s.touch_render_cache(14,(1,99),1);}
 assert!(s.render_cache_hits[..14].iter().all(|&n|n==0));
 let victim=select_affine_pool(0,15,|_|true,|_|false,|_|false,&s.render_cache_hits,&s.render_cache_cost,&s.render_cache_used).unwrap();
 assert!(victim<14);s.touch_render_cache(victim,(2,1),1);assert_eq!(s.render_cache_hits[victim as usize],1);
 let clock=s.render_cache_clock;s.touch_render_cache(15,(9,9),1);assert_eq!(s.render_cache_clock,clock);
 s.render_cache_clock=u64::MAX-1;s.touch_render_cache(14,(1,99),1);s.touch_render_cache(14,(1,99),1);assert_eq!(s.render_cache_clock,u64::MAX);
 // Equal-frequency active roots retain their accumulated binding-walk
 // cost. A large compositor must not be the cheapest victim merely because
 // it published slightly earlier than small clients. Inactive roots yield.
 assert_eq!(select_affine_pool(0,4,|_|true,|_|false,|_|false,&[8;4],&[1,1,1,64],&[94,91,99,90]),Some(1));
 assert_eq!(select_affine_pool(0,4,|_|true,|_|false,|_|false,&[8;4],&[1,1,1,64],&[94,91,99,0]),Some(3));
 assert_eq!(select_affine_pool(0,4,|p|*p==3,|_|false,|_|false,&[8;4],&[1,1,1,64],&[94,91,99,90]),Some(3));
 assert_eq!(select_affine_pool(0,4,|_|true,|_|false,|_|false,&[8;4],&[1;3],&[94,91,99,90]),None);
 s.touch_render_cache(0,(1,0),usize::MAX);assert_eq!(s.render_cache_cost[0],u16::MAX);
 s.touch_render_cache(1,(1,1),0);assert_eq!(s.render_cache_cost[1],1);
 // Legacy overlap is still separately opted into. Cached contexts cannot
 // enable it or alter native/diagnostic profiles.
 for slots in [0,1,2,4,15,16,u32::MAX]{
  module_parameters::SETTINGS.with(|v|v.set([slots,1,0,0,0,0,0]));
  assert_eq!(render::serialized_cache_enabled(),slots>1);assert_eq!(render::pool_count(),if slots>1{slots.min(15)as u64}else{1},"slots {slots}");
 }
 for setting in 1..6{let mut v=[2,1,0,0,0,0,0];v[setting]=if setting==1{0}else{1};module_parameters::SETTINGS.with(|c|c.set(v));assert!(!render::serialized_cache_enabled());}
 println!("PASS {checks} original selector cases +32768 affine cases; actual GPU/CPU cross-pool lease exclusion, preparation, failure fences, bounded cache and legacy/native policy isolation");
}
'''
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);(a.evidence/'source.json').write_text(json.dumps({'sources':{n:hashlib.sha256(s.encode()).hexdigest() for n,s in sources.items()},'GPU_execution':False},indent=2)+'\n');print(out,end='')
