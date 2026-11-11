#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compile the shared render wave; test independent retirement and CPU leases.

The firmware checks are bounded stand-ins. This exercises actual production
wave ownership, not GPU execution or a firmware protocol simulation.
"""
import argparse,hashlib,json,subprocess,tempfile
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
source=Path(__file__).resolve().parents[1]/'g17p_render_wave.rs';body=source.read_text().split('impl RetainedFrame for RenderPending {')[0].replace('super::super::g17p_render_lifecycle','crate::g17p_render_lifecycle')
life=source.with_name('g17p_render_lifecycle.rs').read_text();start=life.index('pub(crate) fn select_storage(');end=life.index('\n}',start)+2;selection=life[start:end]
start=life.index('pub(crate) fn select_pool(');end=life.index('\n}',start)+2;selection+='\n'+life[start:end]
boot=source.with_name('g17p_boot.rs').read_text();start=boot.index('    fn render_pool_available(');end=boot.index('    fn retained_render_idle(',start);availability=boot[start:end]
runtime=source.with_name('g17p_render_runtime.rs').read_text()
start=runtime.rindex('#[derive',0,runtime.index('pub(crate) enum PreparationKey'))
lease_types=runtime[start:runtime.index('pub(crate) struct PreparedAppend',start)]
start=boot.index('    fn render_preparation_active(')
lease_check=boot[start:boot.index('    fn render_preparing(',start)].replace('self.render_prepare_leases','self.leases')
code=r'''#![allow(dead_code,unused_imports)]
extern crate self as kernel;
use std::{cell::RefCell,collections::VecDeque,sync::{Arc,atomic::{AtomicI32,Ordering}}};
pub type Error=i32;
pub type Result<T=()> = std::result::Result<T,i32>;
pub const EIO:i32=5;pub const ENOMEM:i32=12;pub const GFP_KERNEL:()=();
thread_local!{static FAIL:RefCell<Option<usize>>=const{RefCell::new(None)};static TRACE:RefCell<Vec<(u32,&'static str)>>=const{RefCell::new(Vec::new())};}
fn allocation()->Result{FAIL.with(|v|{let before=*v.borrow();match before{Some(0)=>Err(ENOMEM),Some(n)=>{*v.borrow_mut()=Some(n-1);Ok(())},None=>Ok(())}})}
fn trace(id:u32,event:&'static str){TRACE.with(|v|v.borrow_mut().push((id,event)));}
fn reset(){FAIL.with(|v|*v.borrow_mut()=None);TRACE.with(|v|v.borrow_mut().clear());}
pub struct KVec<T>(Vec<T>);
impl<T>KVec<T>{fn new()->Self{Self(Vec::new())}fn reserve(&mut self,n:usize,_:())->Result{if n>self.0.capacity()-self.0.len(){allocation()?;self.0.reserve(n);}Ok(())}fn push(&mut self,v:T,f:())->Result{self.reserve(1,f)?;self.0.push(v);Ok(())}fn remove(&mut self,i:usize)->Result<T>{if i>=self.0.len(){Err(EIO)}else{Ok(self.0.remove(i))}}fn drain(&mut self,r:std::ops::RangeTo<usize>)->std::vec::Drain<'_,T>{self.0.drain(r)}}
impl<T>std::ops::Deref for KVec<T>{type Target=[T];fn deref(&self)->&[T]{&self.0}}
impl<T>std::ops::DerefMut for KVec<T>{fn deref_mut(&mut self)->&mut[T]{&mut self.0}}
pub mod prelude{pub use crate::{Result,Error,EIO,GFP_KERNEL,KVec};}
pub mod sync{pub use std::sync::Arc;}
pub mod device{pub struct Device;}
pub mod dma_fence{
use super::*;
#[derive(Clone)]pub struct Fence(pub Arc<AtomicI32>);
pub trait RawDmaFence{fn raw(&self)->*const AtomicI32;}
impl RawDmaFence for Fence{fn raw(&self)->*const AtomicI32{Arc::as_ptr(&self.0)}}
impl Fence{pub fn new()->Self{Self(Arc::new(AtomicI32::new(0)))}pub fn set_error(&self,e:i32){self.0.store(-e,Ordering::Relaxed);}pub fn signal(&self){let _=self.0.compare_exchange(0,1,Ordering::Relaxed,Ordering::Relaxed);}pub fn status(&self)->i32{self.0.load(Ordering::Relaxed)}}
}
pub mod bindings{use super::*;pub unsafe fn dma_fence_get_status(p:*const AtomicI32)->i32{unsafe{(*p).load(Ordering::Relaxed)}}}
use dma_fence::{Fence,RawDmaFence};
mod g17p_render_lifecycle{pub const STORAGE_SUBMISSIONS:u32=256;SELECTOR}
struct Image;
struct RenderReceipt{fence:Fence}
impl RenderReceipt{fn fail(&self,error:i32){if self.fence.status()==0{self.fence.set_error(error);self.fence.signal();}}}
struct Completion{id:u32,fence:Fence,owner:Arc<()>,error:Option<i32>}
enum PendingWork{Render(RenderWave<RenderPending>),WrappedRender(RenderWave<RenderPending>),Control}
mod render{LEASE_TYPES}
struct Session{leases:Vec<(render::PreparationKey,Arc<std::sync::atomic::AtomicBool>)>,pending:Option<PendingWork>,preparing:Vec<u32>,defer_cache:bool,completions:KVec<Completion>,render_cpu_leases:KVec<(u32,Fence)>}
impl Session{
LEASE_CHECK
AVAILABILITY
fn render_preparing(&self,pool:u32)->bool{self.preparing.contains(&pool)}
fn new(defer_cache:bool)->Self{Self{leases:Vec::new(),pending:None,preparing:Vec::new(),defer_cache,completions:KVec::new(),render_cpu_leases:KVec::new()}}
fn cpu(&mut self,error:Option<i32>){while !self.completions.is_empty(){let c=self.completions.remove(0).unwrap();trace(c.id,"cpu");if let Some(e)=error.or(c.error){c.fence.set_error(e);}c.fence.signal();}}
fn busy(&self,pool:u32)->bool{self.render_cpu_leases.iter().any(|(p,f)|*p==pool&&f.status()==0)}
}
struct RenderPending{id:u32,pool:u32,receipt:Arc<RenderReceipt>,owner:Arc<()>,steps:VecDeque<(bool,bool)>,control:bool,cache_error:Option<i32>,release_error:Option<i32>,completion_error:Option<i32>}
impl RenderPending{
fn new(id:u32,pool:u32,steps:&[(bool,bool)])->Self{Self{id,pool,receipt:Arc::new(RenderReceipt{fence:Fence::new()}),owner:Arc::new(()),steps:steps.iter().copied().collect(),control:false,cache_error:None,release_error:None,completion_error:None}}
fn fence(&self)->Fence{self.receipt.fence.clone()}
}
MOD
impl render_wave::RetainedFrame for RenderPending{
 type Cache=();
 fn receipt(&self)->&Arc<RenderReceipt>{&self.receipt}
 fn fence(&self)->&Fence{&self.receipt.fence}
 fn pool(&self)->u32{self.pool}
 fn storage(&self)->u32{self.id%256}
 fn control_done(&self)->bool{self.control}
 fn poll(&mut self,_:&mut Session,_:&device::Device,_:Option<&Image>,_:bool)->Result<bool>{trace(self.id,"poll");let (done,control)=self.steps.pop_front().unwrap_or((true,self.control));self.control=control;Ok(done)}
 fn cache_seed(&self,_:&Session)->Result{trace(self.id,"cache_seed");self.cache_error.map_or(Ok(()),Err)}
 fn release(&mut self,_:&mut Session)->Result{trace(self.id,"release");self.release_error.map_or(Ok(()),Err)}
 fn completion(self,_:())->Completion{Completion{id:self.id,fence:self.receipt.fence.clone(),owner:self.owner,error:self.completion_error}}
}
use render_wave::RenderWave;
fn wave(frames:Vec<RenderPending>)->RenderWave{RenderWave{frames:KVec(frames),next:0}}
fn log()->Vec<(u32,&'static str)>{TRACE.with(|v|v.borrow().clone())}
fn main(){let dev=device::Device;let mut cases=0;
 // A slow first ticket cannot retain another pool's credit. CPU completion,
 // rather than transport consumption, releases that second pool's lease.
 reset();let mut s=Session::new(true);let a=RenderPending::new(1,0,&[(false,false),(true,false)]);let b=RenderPending::new(2,1,&[(true,false)]);let fa=a.fence();let fb=b.fence();let owner=b.owner.clone();let mut w=wave(vec![a,b]);
 assert!(!w.poll(&mut s,&dev,None).unwrap());assert_eq!(w.frames.len(),1);assert_eq!(w.frames[0].id,1);assert_eq!(s.completions.len(),1);assert_eq!(s.completions[0].id,2);assert_eq!(fb.status(),0);assert!(s.busy(1));assert_eq!(Arc::strong_count(&owner),2);s.cpu(None);assert_eq!(fb.status(),1);assert!(!s.busy(1));assert_eq!(Arc::strong_count(&owner),1);assert!(w.poll(&mut s,&dev,None).unwrap());assert_eq!(fa.status(),0);s.cpu(None);assert_eq!(fa.status(),1);cases+=1;
 // Fresh control closure is probed immediately, once, with no polling delay.
 reset();let mut s=Session::new(true);let mut w=wave(vec![RenderPending::new(3,2,&[(false,true),(true,true)])]);assert!(w.poll(&mut s,&dev,None).unwrap());assert_eq!(log().iter().filter(|&&(id,e)|id==3&&e=="poll").count(),2);assert_eq!(s.completions.len(),1);cases+=1;
 // Every fallible cache/queue reservation precedes hardware release/removal.
 for failure in 0..3{reset();let mut s=Session::new(true);let mut f=RenderPending::new(4,3,&[(true,false)]);if failure==2{f.cache_error=Some(ENOMEM);}let fence=f.fence();let owner=f.owner.clone();let mut w=wave(vec![f]);if failure<2{FAIL.with(|b|*b.borrow_mut()=Some(failure));}assert_eq!(w.poll(&mut s,&dev,None),Err(ENOMEM));assert_eq!(w.frames.len(),1);assert_eq!(fence.status(),0);assert_eq!(Arc::strong_count(&owner),2);assert!(s.completions.is_empty());assert!(!log().contains(&(4,"release")));reset();w.fail(EIO);assert_eq!(fence.status(),-EIO);cases+=1;}
 // A later failed release retains its exact frame and does not discard a
 // previously completed owner's deferred visibility work.
 reset();let mut s=Session::new(true);let a=RenderPending::new(5,4,&[(true,false)]);let mut b=RenderPending::new(6,5,&[(true,false)]);b.release_error=Some(EIO);let fa=a.fence();let fb=b.fence();let mut w=wave(vec![a,b]);assert_eq!(w.poll(&mut s,&dev,None),Err(EIO));assert_eq!(w.frames.len(),1);assert_eq!(w.frames[0].id,6);assert_eq!(s.completions.len(),1);w.fail(EIO);assert_eq!(fb.status(),-EIO);assert_eq!(fa.status(),0);s.cpu(None);assert_eq!(fa.status(),1);cases+=1;
 // Synchronous Neo completion signals after its protocol poll's cache work.
 reset();let mut s=Session::new(false);let f=RenderPending::new(7,6,&[(true,false)]);let fence=f.fence();let mut w=wave(vec![f]);assert!(w.poll(&mut s,&dev,None).unwrap());assert_eq!(fence.status(),1);assert!(s.completions.is_empty()&&s.render_cpu_leases.is_empty());cases+=1;
 // Retired protocol errors survive successful CPU visibility. An independently
 // successful fence is never overwritten by a later wave failure.
 reset();let mut s=Session::new(true);let mut f=RenderPending::new(8,7,&[(true,false)]);f.completion_error=Some(ENOMEM);let fence=f.fence();let mut w=wave(vec![f]);assert!(w.poll(&mut s,&dev,None).unwrap());s.cpu(None);assert_eq!(fence.status(),-ENOMEM);let f=RenderPending::new(9,8,&[]);let success=f.fence();success.signal();let w=wave(vec![f]);w.fail(EIO);assert_eq!(success.status(),1);cases+=1;
 // Removing a retired prefix does not release or poll those tickets twice.
 reset();let mut s=Session::new(true);let mut w=wave(vec![RenderPending::new(10,9,&[]),RenderPending::new(11,10,&[])]);w.next=1;assert!(w.poll(&mut s,&dev,None).unwrap());assert!(log().iter().all(|&(id,_)|id==11));cases+=1;
 // Storage remains leased independently of ordinal/pool. Scan past a slow
 // owner, including modular wrap, instead of stalling every other pool.
 reset();let w=wave(vec![RenderPending::new(0,0,&[]),RenderPending::new(1,1,&[]),RenderPending::new(255,2,&[])]);assert_eq!(w.next_storage(0,|a,b|a==b),Some(2));assert_eq!(w.next_storage(255,|a,b|a==b),Some(2));assert_eq!(w.next_storage(257,|a,b|a==b),Some(2));cases+=1;
 reset();let mut w=wave((0..256).map(|i|RenderPending::new(i,i%15,&[])).collect());assert_eq!(w.next_storage(0,|a,b|a==b),None);w.frames.remove(13).unwrap();assert_eq!(w.next_storage(0,|a,b|a==b),Some(13));assert_eq!(w.next_storage(14,|a,b|a==b),Some(13));cases+=1;
 // A preparation lease excludes a retired slot even when no GPU frame
 // references it. Active tickets and CPU preparations are independent owners.
 reset();let w=wave(vec![]);assert_eq!(w.next_storage_available(0,|a,b|a==b,|s|s!=0),Some(1));cases+=1;
 let w=wave(vec![RenderPending::new(0,0,&[]),RenderPending::new(1,1,&[]),RenderPending::new(255,2,&[])]);
 for next in [0,1,2,255,256,257] {assert_eq!(w.next_storage_available(next,|a,b|a==b,|s|s!=2),Some(3));}
 assert_eq!(w.next_storage_available(0,|a,b|a==b,|_|false),None);cases+=1;
 // A dropped preparation releases its slot. Selection can reuse it while
 // preserving every other publication and preparation claim at modular wrap.
 for slot in 0..256 {assert_eq!(w.next_storage_available(slot,|a,b|a==b,|s|s==slot),if [0,1,255].contains(&slot){None}else{Some(slot)});}
 cases+=1;
 // Compile the actual shared key and RAII reservation, including its
 // acquire/release observation. Equal numeric pool/slot IDs do not alias.
 for slot in 0..256 {
  reset();let mut s=Session::new(true);let active=Arc::new(std::sync::atomic::AtomicBool::new(true));
  s.leases.push((render::PreparationKey::Storage(slot),active.clone()));
  let held=render::PreparationReservation(active.clone());
  assert!(s.render_preparation_active(render::PreparationKey::Storage(slot)));
  assert!(!s.render_preparation_active(render::PreparationKey::Pool(slot)));
  let w=wave(vec![]);
  assert_eq!(w.next_storage_available(slot,|a,b|a==b,|v|!s.render_preparation_active(render::PreparationKey::Storage(v))),Some((slot+1)%256));
  drop(held);
  assert_eq!(w.next_storage_available(slot,|a,b|a==b,|v|!s.render_preparation_active(render::PreparationKey::Storage(v))),Some(slot));
 }cases+=1;
 // A preparation failure drops its claim before returning the error; an
 // independent preparation keeps its own slot pinned.
 reset();let mut s=Session::new(true);let a=Arc::new(std::sync::atomic::AtomicBool::new(true));let b=Arc::new(std::sync::atomic::AtomicBool::new(true));
 s.leases.push((render::PreparationKey::Storage(0),a.clone()));s.leases.push((render::PreparationKey::Storage(1),b.clone()));
 let held=render::PreparationReservation(a);let other=render::PreparationReservation(b);
 let failed=(|_claim:render::PreparationReservation|->Result{Err(ENOMEM)})(held);assert_eq!(failed,Err(ENOMEM));
 assert!(!s.render_preparation_active(render::PreparationKey::Storage(0)));assert!(s.render_preparation_active(render::PreparationKey::Storage(1)));
 drop(other);assert!(!s.render_preparation_active(render::PreparationKey::Storage(1)));cases+=1;
 // Moving the CPU worker to another thread preserves its release ordering.
 let flag=Arc::new(std::sync::atomic::AtomicBool::new(true));let held=render::PreparationReservation(flag.clone());
 std::thread::spawn(move||drop(held)).join().unwrap();assert!(!flag.load(Ordering::Acquire));cases+=1;
 // RX routing selects the retained owner by identity, not wave order.
 reset();let w=wave(vec![RenderPending::new(17,3,&[]),RenderPending::new(18,4,&[]),RenderPending::new(19,3,&[])]);
 assert_eq!(RenderWave::<RenderPending>::find_owner(&w.frames,|p|p.id==18),Ok(1));cases+=1;
 assert_eq!(RenderWave::<RenderPending>::find_owner(&w.frames,|p|p.id==99),Err(EIO));cases+=1;
 assert_eq!(RenderWave::<RenderPending>::find_owner(&w.frames,|p|p.pool==3),Err(EIO));cases+=1;
 // Pool selection preserves exact-snapshot reuse and skips leased matches.
 macro_rules! select{($($args:expr),*)=>{g17p_render_lifecycle::select_pool($($args),*)}}
 assert_eq!(select!(0,4,|_|true,|p|*p==3),Some(3));
 assert_eq!(select!(1,4,|p|*p!=1,|p|*p==1),Some(2));
 assert_eq!(select!(5,4,|_|true,|_|false),Some(1));
 assert_eq!(select!(0,4,|_|false,|_|true),None);
 assert_eq!(select!(0,0,|_|true,|_|true),None);cases+=1;
 // One common availability check retains preparation, GPU and deferred CPU
 // leases independently, for both Neo and wrapped Max frame variants.
 for wrapped in [false,true]{reset();let mut s=Session::new(true);let w=wave(vec![RenderPending::new(30,0,&[])]);s.pending=Some(if wrapped{PendingWork::WrappedRender(w)}else{PendingWork::Render(w)});s.preparing.push(2);let f=Fence::new();s.render_cpu_leases.push((1,f.clone()),()).unwrap();assert!(!s.render_pool_available(0)&&!s.render_pool_available(1)&&!s.render_pool_available(2)&&s.render_pool_available(3));f.signal();assert!(s.render_pool_available(1));s.preparing.clear();assert!(s.render_pool_available(2));drop(s.pending.take());assert!(s.render_pool_available(0));cases+=1;}
 println!("PASS {cases} shared-wave ownership cases: out-of-order retirement, CPU leases, fresh control, allocation/cache/release failure, synchronous completion, error preservation and retired prefix");
}
'''
with tempfile.TemporaryDirectory(prefix='g17p-render-wave-')as d:
 t=Path(d);(t/'wave.rs').write_text(body);code=code.replace('LEASE_TYPES',lease_types).replace('LEASE_CHECK',lease_check).replace('SELECTOR',selection).replace('AVAILABILITY',availability).replace('MOD',f'#[path={json.dumps(str(t/"wave.rs"))}]mod render_wave;');(t/'main.rs').write_text(code)
 subprocess.run(['rustc','--edition=2021',str(t/'main.rs'),'-o',str(t/'gate')],check=True)
 result=subprocess.check_output([str(t/'gate')],text=True);assert result.startswith('PASS 23 '),result
 a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'harness.rs').write_text(code);(a.evidence/'production-wave.rs').write_text(body)
 (a.evidence/'summary.json').write_text(json.dumps(dict(source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),cases=23,result=result.strip(),actual_production_wave_compiled=True,GPU_execution=False),indent=2)+'\n')
 print(result,end='')
