#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exercise actual deferred queue-release destructors with a callback still running."""
import argparse,hashlib,json,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',required=True,type=Path);a=p.parse_args();r=Path(__file__).resolve().parents[1];src=r/'g17p_async.rs';s=src.read_text()
def item(marker):
 i=s.index(marker);j=s.index('{',i)+1;depth=1
 while depth:depth+=(s[j]=='{')-(s[j]=='}');j+=1
 return s[i:j]
code=r'''
#![allow(dead_code,non_camel_case_types)]
use std::{pin::Pin,sync::{Arc,Mutex,Condvar,atomic::{AtomicUsize,Ordering}},cell::Cell};
type KBox<T>=Box<T>;
trait PinnedDrop{fn drop(self:Pin<&mut Self>);}
trait WorkItem{type Pointer;fn run(this:Self::Pointer);}
#[derive(Default)]struct State{callbacks:Mutex<usize>,changed:Condvar,attempts:AtomicUsize,destroyed:AtomicUsize}
thread_local!{static OWN_CALLBACK:Cell<bool>=const{Cell::new(false)};}
mod bindings{use super::*;pub struct workqueue_struct(pub Arc<State>);
 pub unsafe fn destroy_workqueue(ptr:*mut workqueue_struct){assert!(!OWN_CALLBACK.with(Cell::get),"destroy/flush on own worker deadlocks");let q=unsafe{Box::from_raw(ptr)};q.0.attempts.fetch_add(1,Ordering::Release);let mut n=q.0.callbacks.lock().unwrap();while *n!=0{n=q.0.changed.wait(n).unwrap();}assert_eq!(q.0.destroyed.fetch_add(1,Ordering::AcqRel),0,"double release");}}
struct JobQueue{queue:*mut bindings::workqueue_struct}
unsafe impl Send for JobQueue{}unsafe impl Sync for JobQueue{}
struct Events{queue_owner:Option<Pin<KBox<JobQueue>>>}
mod workqueue{use super::*;pub static PENDING:Mutex<Vec<Pin<KBox<JobQueue>>>>=Mutex::new(Vec::new());pub fn system_long()->Q{Q}pub struct Q;
 pub trait Cleanup{fn enqueue(self);}impl Cleanup for Pin<KBox<JobQueue>>{fn enqueue(self){PENDING.lock().unwrap().push(self);}}
 impl Q{pub fn enqueue<T:Cleanup,const ID:u64>(&self,owner:T){owner.enqueue();}}
}
QUEUE_DROP
EVENTS_DROP
WORK_RUN
impl Drop for Events{fn drop(&mut self){unsafe{<Self as PinnedDrop>::drop(Pin::new_unchecked(self));}}}
impl Drop for JobQueue{fn drop(&mut self){unsafe{<Self as PinnedDrop>::drop(Pin::new_unchecked(self));}}}
fn queue(state:&Arc<State>)->Pin<KBox<JobQueue>>{Box::pin(JobQueue{queue:Box::into_raw(Box::new(bindings::workqueue_struct(state.clone())))})}
fn main(){
 let state=Arc::new(State::default());let events=Arc::new(Events{queue_owner:Some(queue(&state))});let queued_job=events.clone();drop(events);
 assert_eq!(state.destroyed.load(Ordering::Acquire),0);assert!(workqueue::PENDING.lock().unwrap().is_empty());
 *state.callbacks.lock().unwrap()=1;OWN_CALLBACK.with(|v|v.set(true));drop(queued_job);OWN_CALLBACK.with(|v|v.set(false));
 assert_eq!(state.attempts.load(Ordering::Acquire),0,"last Events drop must only enqueue cleanup");
 let cleanup=workqueue::PENDING.lock().unwrap().pop().unwrap();
 let worker=std::thread::spawn(move||<JobQueue as WorkItem>::run(cleanup));
 while state.attempts.load(Ordering::Acquire)==0{std::thread::yield_now();}
 assert_eq!(state.destroyed.load(Ordering::Acquire),0,"queue must outlive its active C callback");
 *state.callbacks.lock().unwrap()=0;state.changed.notify_all();worker.join().unwrap();assert_eq!(state.destroyed.load(Ordering::Acquire),1);
 // Failed Events construction releases the initialized queue on the probe
 // thread; an unallocated/null queue does not call the C destructor.
 let failed=Arc::new(State::default());drop(queue(&failed));assert_eq!(failed.destroyed.load(Ordering::Acquire),1);drop(Box::pin(JobQueue{queue:std::ptr::null_mut()}));
 for _ in 0..1000{let state=Arc::new(State::default());let events=Events{queue_owner:Some(queue(&state))};drop(events);assert_eq!(state.attempts.load(Ordering::Acquire),0);let cleanup=workqueue::PENDING.lock().unwrap().pop().unwrap();<JobQueue as WorkItem>::run(cleanup);assert_eq!(state.destroyed.load(Ordering::Acquire),1);}
 assert!(workqueue::PENDING.lock().unwrap().is_empty());println!("PASS actual pinned destructors/release callback: queued-job lifetime, last drop on own worker, deferred flush waiting for active callback, constructor unwind/null queue and 1000 exactly-once releases");
}
'''.replace('QUEUE_DROP',item('impl PinnedDrop for JobQueue {')).replace('EVENTS_DROP',item('impl PinnedDrop for Events {')).replace('WORK_RUN',item('impl WorkItem for JobQueue {'))
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code);subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True);out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);(a.evidence/'source.json').write_text(json.dumps({'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'GPU_execution':False},indent=2)+'\n');print(out,end='')
