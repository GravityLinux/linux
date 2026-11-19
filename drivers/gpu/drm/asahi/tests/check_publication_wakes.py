#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exercise actual publication notification and late receipt registration selection."""
import argparse,hashlib,json,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
root=Path(__file__).resolve().parents[1]
boot=(root/'g17p_boot.rs').read_text();schedule=(root/'g17p_schedule.rs').read_text()
def item(s,marker,begin=0):
 start=s.index(marker,begin);end=s.index('{',start)+1;depth=1
 while depth:depth+=(s[end]=='{')-(s[end]=='}');end+=1
 return s[start:end]
mark=item(boot,'    fn mark_published(')
assert mark==item(boot,'    fn mark_published(',boot.index('impl ComputeReceipt {'))
get=item(schedule,'    pub(super) fn receipt_fence(').replace('pub(super) ', '')
code=r'''
#![allow(dead_code)]
use std::{cell::{Cell,RefCell},sync::{Arc,atomic::{AtomicBool,Ordering}}};
struct Notify{ready:Arc<AtomicBool>,signals:Cell<usize>}
impl Notify{fn signal(&self){assert!(self.ready.load(Ordering::Acquire));self.signals.set(self.signals.get()+1)}}
struct RawReceipt{publication_ready:Arc<AtomicBool>,publication_fence:Notify,completion:Cell<bool>}
impl RawReceipt{
MARK
}
#[derive(Clone)]struct Fence(u32,i32);fn status(f:&Fence)->i32{f.1}
struct Receipt{pub_fence:Fence,completion:Fence}
impl Receipt{fn publication_fence(&self)->&Fence{&self.pub_fence}fn fence(&self)->&Fence{&self.completion}}
struct Lock<T>(RefCell<T>);impl<T>Lock<T>{fn lock(&self)->std::cell::RefMut<'_,T>{self.0.borrow_mut()}}
struct Point{receipt:Lock<Option<Receipt>>,publication:Fence,completion:Fence}struct Node{point:Point}struct Schedule{nodes:Vec<Node>}
impl Schedule{
GET
}
fn main(){
 for _ in 0..2 {let ready=Arc::new(AtomicBool::new(false));let r=RawReceipt{publication_ready:ready.clone(),publication_fence:Notify{ready,signals:Cell::new(0)},completion:Cell::new(false)};
 r.mark_published();assert_eq!(r.publication_fence.signals.get(),1);assert!(!r.completion.get());}
 let mut s=Schedule{nodes:vec![Node{point:Point{receipt:Lock(RefCell::new(Some(Receipt{pub_fence:Fence(1,1),completion:Fence(2,0)}))),publication:Fence(11,0),completion:Fence(12,0)}},Node{point:Point{receipt:Lock(RefCell::new(None)),publication:Fence(21,0),completion:Fence(22,0)}}]};
 assert_eq!(s.receipt_fence(0,true).unwrap().0,1);assert_eq!(s.receipt_fence(0,false).unwrap().0,2);
 assert!(s.receipt_fence(1,true).is_none());assert!(s.receipt_fence(1,false).is_none());
 // Selection must not inspect an already-published flag: publication may
 // race the last Point refresh, in which case late binding must requeue.
 assert_eq!(s.receipt_fence(0,true).unwrap().0,1);
 s.nodes[0].point.publication.1=1;
 assert!(s.receipt_fence(0,true).is_none());assert_eq!(s.receipt_fence(0,false).unwrap().0,2);
 s.nodes[0].point.completion.1=-5;assert!(s.receipt_fence(0,false).is_none());
 s.nodes[0].point.publication.1=0;assert_eq!(s.receipt_fence(0,true).unwrap().0,1);
 println!("PASS actual render/compute publication marker orders state before exact wake without completing GPU work; actual staged/unstaged receipt selection preserves late-registration recheck");
}
'''.replace('MARK',mark).replace('GET',get)
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True)
(a.evidence/'output.txt').write_text(out);(a.evidence/'source.json').write_text(json.dumps({'boot_sha256':hashlib.sha256(boot.encode()).hexdigest(),'schedule_sha256':hashlib.sha256(schedule.encode()).hexdigest(),'GPU_execution':False},indent=2)+'\n');print(out,end='')
