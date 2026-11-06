#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare actual IRQ ready-list mutations/wakes with a submission-order oracle."""
import argparse,hashlib,json,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);p.add_argument('--source',type=Path);a=p.parse_args()
root=Path(__file__).resolve().parents[1]
s=(a.source or root/'g17p_async.rs').read_text()
def item(marker):
 start=s.index(marker);end=s.index('{',start)+1;depth=1
 while depth:depth+=(s[end]=='{')-(s[end]=='}');end+=1
 return s[start:end]
code=r'''
#![allow(dead_code,unused_imports,non_upper_case_globals)]
use std::{cell::{Cell,RefCell},sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering}}};
type Result<T=()> = std::result::Result<T,i32>;const GFP_ATOMIC:u32=0;
thread_local!{static FAIL:Cell<usize>=const{Cell::new(usize::MAX)};static WAKES:RefCell<Vec<usize>>=const{RefCell::new(Vec::new())};}
struct KVec<T>(Vec<T>);impl<T>KVec<T>{
 fn new()->Self{Self(Vec::new())}fn reserve(&mut self,n:usize,_:u32)->Result{FAIL.with(|v|{let x=v.get();if x==0{return Err(12)}v.set(x.saturating_sub(1));self.0.reserve(n);Ok(())})}
 fn push(&mut self,v:T,_:u32)->Result{assert!(self.0.len()<self.0.capacity());self.0.push(v);Ok(())}
 fn remove(&mut self,i:usize)->Result<T>{Ok(self.0.remove(i))}
}
impl<T>std::ops::Deref for KVec<T>{type Target=[T];fn deref(&self)->&[T]{&self.0}}
impl<T>std::ops::DerefMut for KVec<T>{fn deref_mut(&mut self)->&mut[T]{&mut self.0}}
impl<'a,T>IntoIterator for &'a KVec<T>{type Item=&'a T;type IntoIter=std::slice::Iter<'a,T>;fn into_iter(self)->Self::IntoIter{self.0.iter()}}
struct Mutex<T>(RefCell<T>);impl<T>Mutex<T>{fn lock(&self)->std::cell::RefMut<'_,T>{self.0.borrow_mut()}}
struct Job{id:usize,queue_key:(u64,u32),firmware_waiting:AtomicBool}
mod workqueue{use super::*;pub fn system_dfl()->Q{Q}pub struct Q;impl Q{pub fn enqueue<T:std::ops::Deref<Target=Job>,const N:usize>(&self,j:T)->bool{WAKES.with(|w|w.borrow_mut().push(j.id));true}}}
'''+item('struct JobSet {')+'\n'+item('impl JobSet {')+'''
struct Events{jobs:Mutex<JobSet>,wake_sequence:AtomicU64}
impl Events{
'''
for marker in ['fn add(&self','fn remove(&self','fn is_head(&self','pub(super) fn prefer_queue(&self','pub(super) fn release_queue(&self','fn is_poll_head(&self','fn wake_backend(&self','fn wake_job(&self']:code+=item(marker).replace('pub(super) ', '')+'\n'
code+=r'''
}
fn take()->Vec<usize>{WAKES.with(|v|std::mem::take(&mut *v.borrow_mut()))}
fn check(e:&Events,live:&[Arc<Job>]){
 let held=e.jobs.lock();assert_eq!(held.jobs.iter().map(|j|j.id).collect::<Vec<_>>(),live.iter().map(|j|j.id).collect::<Vec<_>>());
 let mut expected=Vec::new();for j in live{if !expected.iter().any(|h:&Arc<Job>|h.queue_key==j.queue_key){expected.push(j.clone());}}
 assert_eq!(held.heads.len(),expected.len());for h in &held.heads{assert!(expected.iter().any(|j|j.id==h.id));}
 let ready:Vec<_>=held.heads.iter().filter(|h|h.firmware_waiting.load(Ordering::SeqCst)).map(|h|h.id).collect();
 let preferences:Vec<_>=held.preferred.iter().flatten().filter_map(|key|held.heads.iter().find(|h|h.queue_key==*key&&ready.contains(&h.id)).map(|h|h.id)).collect();
 let poller=preferences.first().copied().or(ready.first().copied());
 drop(held);for j in live{assert_eq!(e.is_head(j),expected.iter().any(|h|h.id==j.id));assert_eq!(e.is_poll_head(j),poller==Some(j.id));}
 e.wake_backend(false);let wakes=take();
 assert!(wakes.len()<=3);assert_eq!(wakes.is_empty(),ready.is_empty());
 for id in &preferences{assert!(wakes.contains(id));}
 for (i,id) in wakes.iter().enumerate(){assert!(ready.contains(id));assert!(!wakes[..i].contains(id));}
 e.wake_backend(true);assert_eq!(take(),live.iter().map(|j|j.id).collect::<Vec<_>>());
}
fn main(){
 // Final IRQ already consumed: the old poller withdraws its waiting flag
 // and completes with no successor on its queue. A different waiting queue
 // must be explicitly queued so it can arm the next fallback timer.
 let test=Events{jobs:Mutex(RefCell::new(JobSet::new())),wake_sequence:AtomicU64::new(0)};
 let x=Arc::new(Job{id:41,queue_key:(1,0),firmware_waiting:AtomicBool::new(true)});
 let y=Arc::new(Job{id:42,queue_key:(2,0),firmware_waiting:AtomicBool::new(true)});
 test.add(x.clone()).unwrap();test.add(y.clone()).unwrap();assert!(test.is_poll_head(&x));
 x.firmware_waiting.store(false,Ordering::SeqCst);test.remove(&x);assert_eq!(take(),vec![42]);assert!(test.is_poll_head(&y));
 test.prefer_queue(0,y.queue_key);test.release_queue(0,y.queue_key);assert_eq!(take(),vec![42]);
 let e=Events{jobs:Mutex(RefCell::new(JobSet::new())),wake_sequence:AtomicU64::new(0)};
 let mut live:Vec<Arc<Job>>=Vec::new();let mut seed=73u64;
 for id in 0..20000{
  seed=seed.wrapping_mul(6364136223846793005).wrapping_add(1);
  if live.len()>64 || seed&3==0&& !live.is_empty(){
   let i=(seed>>8)as usize%live.len();let j=live.remove(i);
   let was_head=e.is_head(&j);e.remove(&j);let wakes=take();
   let next=live.iter().find(|h|h.queue_key==j.queue_key);
   let mut expected=if was_head{next.map(|h|vec![h.id]).unwrap_or_default()}else{vec![]};
   if was_head {if let Some(poller)=e.jobs.lock().poll_head(){if !expected.contains(&poller.id){expected.push(poller.id);}}}
   assert_eq!(wakes,expected);
  }else{
   let j=Arc::new(Job{id,queue_key:((seed>>12)%4,(seed>>24)as u32%7),firmware_waiting:AtomicBool::new(seed&4!=0)});
   e.add(j.clone()).unwrap();live.push(j);
  }
  if !live.is_empty() && id%3==0 {
   e.prefer_queue((seed&1) as usize,live[(seed>>16) as usize%live.len()].queue_key);
  }
  check(&e,&live);
  // An advancing head withdraws its waiting flag. Poll ownership follows
  // another ready queue and returns when the earlier head waits again.
  if !live.is_empty() && id%5==0 {
   let j=&live[(seed>>32) as usize%live.len()];
   j.firmware_waiting.fetch_xor(true,Ordering::SeqCst);check(&e,&live);
  }
 }
 // Admission allocation failures cannot publish half a head/membership pair.
 for fail in 0..2{
  let before=e.jobs.lock().jobs.len();let j=Arc::new(Job{id:30000,queue_key:(99,99),firmware_waiting:AtomicBool::new(true)});
  FAIL.with(|v|v.set(fail));assert_eq!(e.add(j),Err(12));FAIL.with(|v|v.set(usize::MAX));assert_eq!(e.jobs.lock().jobs.len(),before);check(&e,&live);
 }
 for j in live{e.remove(&j);take();}assert!(e.jobs.lock().heads.is_empty());
 assert_eq!(e.jobs.lock().preferred,[None;2]);
 // Every ready queue must be serviced in at most one rotation, even when
 // both engines permanently prefer another queue. A second job on a queue
 // must remain dormant until its predecessor is removed.
 for id in 0..16 {e.add(Arc::new(Job{id,queue_key:(0,id as u32),firmware_waiting:AtomicBool::new(true)})).unwrap();}
 e.prefer_queue(0,(0,0));e.prefer_queue(1,(0,1));let mut seen=vec![false;16];
 for _ in 0..16 {e.wake_backend(false);for id in take(){seen[id]=true;}}
 assert!(seen.into_iter().all(|x|x));e.release_queue(0,(0,1));assert_eq!(e.jobs.lock().preferred[0],Some((0,0)));
 e.release_queue(0,(0,0));assert_eq!(e.jobs.lock().preferred[0],None);take();
 let successor=Arc::new(Job{id:100,queue_key:(0,0),firmware_waiting:AtomicBool::new(true)});let identity=&*successor as *const Job as usize;e.add(successor).unwrap();
 e.wake_job(identity);assert_eq!(take(),vec![100]);e.wake_job(10000);assert!(take().is_empty());
 println!("PASS 20000 actual admission/retirement/wake transitions: interleaved file-scoped queues, out-of-order removals, promotion, bounded preferred wakes, fair rotation, ready filtering, fatal wakes and transactional allocation failure");
}
'''
poll=(root/'g17p_schedule.rs').read_text(); start=poll.index('fn poll_due(');end=poll.index('\n}',start)+2
code=code.replace('fn main(){', poll[start:end]+'''
fn check_poll(){
 for control in [false,true]{let limit=if control{20000}else{200000};
  for age in [0,1,limit-1,limit,limit+1]{assert_eq!(poll_due(1,1,age,control),age>=limit);assert!(poll_due(1,2,age,control));}
 }
 // A poll records the captured epoch; a notification arriving during it
 // remains due even at age zero. Wrapping to zero also changes the epoch.
 assert!(poll_due(9,10,0,false));assert!(poll_due(u64::MAX,0,0,false));
}
fn main(){check_poll();''')
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);(a.evidence/'source.json').write_text(json.dumps({'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'GPU_execution':False},indent=2)+'\n');print(out,end='')
