#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute the actual IRQ wake-list methods against host job/queue owners."""
import argparse
from pathlib import Path
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence', type=Path, required=True)
a = p.parse_args()
source = (Path(__file__).resolve().parents[1] / 'g17p_async.rs').read_text()

def method(name):
    start = source.index('    fn ' + name + '(')
    end = source.index('{', start) + 1
    depth = 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]

code = r'''
use std::{ops::Deref,sync::{Arc,Mutex,MutexGuard,atomic::{AtomicBool,AtomicU64,Ordering}}};
struct Lock<T>(Mutex<T>); impl<T> Lock<T>{fn lock(&self)->MutexGuard<'_,T>{self.0.lock().unwrap()}}
struct Job{id:usize,queue_key:(u64,u32),firmware_waiting:AtomicBool}
struct Events{jobs:Lock<Vec<Arc<Job>>>,wake_sequence:AtomicU64}
static WAKES:Mutex<Vec<usize>>=Mutex::new(Vec::new());
mod workqueue{
 pub struct Queue; pub fn system_dfl()->Queue{Queue}
 impl Queue{pub fn enqueue<T:std::ops::Deref<Target=super::Job>,const I:usize>(&self,job:T)->Result<(),()>{super::WAKES.lock().unwrap().push(job.id);Ok(())}}
}
impl Events{ METHODS }
fn wakes()->Vec<usize>{std::mem::take(&mut *WAKES.lock().unwrap())}
fn job(id:usize,file:u64,queue:u32,waiting:bool)->Arc<Job>{Arc::new(Job{id,queue_key:(file,queue),firmware_waiting:AtomicBool::new(waiting)})}
fn main(){
 let aa=job(1,10,1,true);let ab=job(2,10,1,true);let ac=job(3,10,1,true);
 let ba=job(4,20,1,true);let bb=job(5,20,1,true);let ca=job(6,10,2,false);let cb=job(7,10,2,true);
 let events=Events{jobs:Lock(Mutex::new(vec![aa.clone(),ab.clone(),ba.clone(),ca.clone(),ac.clone(),bb.clone(),cb.clone()])),wake_sequence:AtomicU64::new(0)};
 events.wake_backend(false);assert_eq!(wakes(),[1,4]);
 assert!(events.is_head(&aa));assert!(!events.is_head(&ab));
 // A dormant head waiting for input must not be bypassed by a later job.
 assert!(events.is_head(&ca));assert!(!events.is_head(&cb));
 events.remove(&aa);assert_eq!(wakes(),[2]);assert!(events.is_head(&ab));
 assert_eq!(events.jobs.lock().iter().map(|j|j.id).collect::<Vec<_>>(),[2,4,6,3,5,7]);
 events.wake_backend(false);assert_eq!(wakes(),[2,4]);
 events.remove(&ab);assert_eq!(wakes(),[3]);assert!(events.is_head(&ac));
 // Failure must reach every remaining owner, including input-blocked jobs.
 events.wake_backend(true);assert_eq!(wakes(),[4,6,3,5,7]);
 // Explicit fence callbacks remain live for non-head jobs.
 events.wake_job(&*bb as *const Job as usize);assert_eq!(wakes(),[5]);
 events.wake_job(0);assert!(wakes().is_empty());
 events.remove(&ba);assert_eq!(wakes(),[5]);
 events.remove(&ca);assert_eq!(wakes(),[7]);
 for j in [&ac,&bb,&cb]{events.remove(j);wakes();}
 events.wake_backend(true);assert!(wakes().is_empty());assert!(!events.is_head(&aa));
 // Deep single-queue backlog still produces exactly one progress wake.
 for i in 0..700{events.jobs.lock().push(job(i,30,1,true));}
 events.wake_backend(false);assert_eq!(wakes(),[0]);
 for i in 0..700{
  let head=events.jobs.lock()[0].clone();assert_eq!(head.id,i);
  events.remove(&head);
  assert_eq!(wakes(),if i<699{vec![i+1]}else{vec![]});
 }
 println!("PASS actual Events methods: file-scoped queue heads, ordered retirement, successor promotion, input fences, failure broadcast, identity callbacks and 700-job drain");
}
'''.replace('METHODS', '\n'.join(method(n) for n in ['remove', 'is_head', 'wake_backend', 'wake_job']))
a.evidence.mkdir(parents=True, exist_ok=False)
(a.evidence / 'harness.rs').write_text(code)
subprocess.run(['rustc', '--edition=2021', str(a.evidence / 'harness.rs'), '-o', str(a.evidence / 'gate')], check=True)
out = subprocess.check_output([str(a.evidence / 'gate')], text=True)
(a.evidence / 'output.txt').write_text(out)
print(out, end='')
