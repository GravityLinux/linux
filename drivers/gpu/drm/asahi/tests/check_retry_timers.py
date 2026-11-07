#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Run the actual pending-job branch: one bounded retry, not two timer wakes."""
import argparse,subprocess,hashlib,json
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
s=(Path(__file__).resolve().parents[1]/'g17p_async.rs').read_text();i=s.index('            Ok(None) => {',s.index('    fn advance('));j=s.index('{',i)+1;start=j;depth=1
while depth:depth+=(s[j]=='{')-(s[j]=='}');j+=1
body=s[start:j-1]
code=r'''
#![allow(dead_code)]
use std::{cell::RefCell,sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering}}};
thread_local!{static TRACE:RefCell<Vec<(u8,i64)>>=const{RefCell::new(Vec::new())};}
fn log(kind:u8,us:i64){TRACE.with(|v|v.borrow_mut().push((kind,us)));}
struct Delta(i64);impl Delta{fn from_micros(us:i64)->Self{Self(us)}}struct Handle;
struct Events{head:bool,poll:bool,wake_sequence:AtomicU64}impl Events{fn is_head(&self,_:&Job)->bool{self.head}fn is_poll_head(&self,_:&Job)->bool{self.poll}}
struct Job{events:Events,firmware_waiting:AtomicBool}
struct Schedule{fast:bool,control:bool}impl Schedule{fn fast_retry(&self)->bool{self.fast}fn fast_retry_us(&self)->i64{if self.control{20}else{200}}}
struct Execution{schedule:Option<Schedule>,control_timer_handle:Option<Handle>}
impl Job{fn request(_:Arc<Self>){log(0,0)}fn watchdog(_:Arc<Self>){log(1,10000)}fn start(self:Arc<Self>,d:Delta)->Handle{log(2,d.0);Handle}
fn wait(this:Arc<Self>,e:&mut Execution,wake_sequence:u64){BODY}}
fn main(){let mut cases=0;
 for head in [false,true]{for poll in [false,true]{for scheduled in [0,1,2]{for control in [false,true]{for raced in [false,true]{
  let j=Arc::new(Job{events:Events{head,poll,wake_sequence:AtomicU64::new(u64::from(raced))},firmware_waiting:AtomicBool::new(false)});
  let mut e=Execution{schedule:(scheduled!=0).then_some(Schedule{fast:scheduled==2,control}),control_timer_handle:None};
  TRACE.with(|v|v.borrow_mut().clear());Job::wait(j.clone(),&mut e,0);assert!(j.firmware_waiting.load(Ordering::SeqCst));
  let t=TRACE.with(|v|v.borrow().clone());let has=|kind|t.iter().filter(|&&(k,_)|k==kind).count();
  assert_eq!(has(0),usize::from(head&&poll&&raced));
  let fast=head&&poll&&scheduled==2;assert_eq!(has(2),usize::from(fast));assert_eq!(e.control_timer_handle.is_some(),fast);
  assert_eq!(has(1),usize::from(head&&(scheduled==0||(poll&&!fast))));
  assert!(has(1)+has(2)<=1,"a pending owner armed duplicate timers");
  for &(kind,us)in &t{if kind==2{assert_eq!(us,if control{20}else{200})}}
  cases+=1;
 }}}}}
 println!("PASS {cases} production pending-job branches: elected head, legacy/slow fallback, 20/200us retries, lost-wake requeue, and no redundant 10ms watchdog");}
'''.replace('BODY',body)
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);(a.evidence/'source.json').write_text(json.dumps({'g17p_async_sha256':hashlib.sha256(s.encode()).hexdigest()},indent=2)+'\n');print(out,end='')
