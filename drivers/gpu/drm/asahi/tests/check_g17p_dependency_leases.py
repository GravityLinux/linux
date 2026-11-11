#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compile actual producer-pool lookup and receipt getters with bounded stand-ins.

A GPU-retired ticket may already have left the wave. Its receipt must retain
physical pool identity through deferred CPU visibility and consumer fencing.
No GPU execution is claimed by this host ownership check.
"""
from pathlib import Path
import argparse,hashlib,json,subprocess
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
r=Path(__file__).resolve().parents[1]
boot=(r/'g17p_boot.rs').read_text();schedule=(r/'g17p_schedule.rs').read_text()
def function(s,marker):
 start=s.index(marker);brace=s.index('{',start);at=brace+1;depth=1
 while depth:
  if s[at]=='{':depth+=1
  if s[at]=='}':depth-=1
  at+=1
 return s[start:at]
methods='\n'.join(function(boot,m)for m in ['    pub(crate) fn is_published(&self) -> bool {','    pub(crate) fn milestone(&self, engine: usize)','    pub(crate) fn retained_pool(&self)'])
methods=methods.replace('super::g17p_render_lifecycle::','crate::life::')
helper=function(schedule,'    fn retain_dependencies(').replace('super::super::g17p_boot::Session','Session')
code=r'''#![allow(dead_code)]
use std::sync::{Arc,atomic::{AtomicBool,AtomicU32,AtomicU64,Ordering}};
type Result=std::result::Result<(),i32>;const EIO:i32=5;
mod life{pub const POOL_SLOTS:u32=15;}
mod render{pub struct Milestone{pub event_slot:u8,pub grid:u8,pub value:u32}}
#[derive(Clone,Debug,PartialEq)]struct Fence(u32);
struct Mutex<T>(std::sync::Mutex<T>);impl<T>Mutex<T>{fn new(v:T)->Self{Self(std::sync::Mutex::new(v))}fn lock(&self)->std::sync::MutexGuard<'_,T>{self.0.lock().unwrap()}}
struct RenderReceipt{publication_ready:AtomicBool,milestones:[AtomicU64;2],retained_pool:AtomicU32}
impl RenderReceipt{METHODS}
enum Receipt{Render(RenderReceipt),Compute}
struct Point{receipt:Mutex<Option<Receipt>>,previous:Mutex<Option<Arc<Point>>>,completion:Fence}
struct Node{point:Arc<Point>,previous:Option<Arc<Point>>,dependencies:[Option<Arc<Point>>;2]}
#[derive(Default)]struct Session{calls:Vec<((u8,u32),Option<u32>,Fence)>}
impl Session{fn retain_dependency(&mut self,p:(u8,u32),pool:Option<u32>,f:&Fence)->Result{self.calls.push((p,pool,f.clone()));Ok(())}}
struct Schedule;impl Schedule{HELPER}
fn p(receipt:Option<Receipt>,previous:Option<Arc<Point>>)->Arc<Point>{Arc::new(Point{receipt:Mutex::new(receipt),previous:Mutex::new(previous),completion:Fence(99)})}
fn render(pool:u32,ready:bool)->Receipt{Receipt::Render(RenderReceipt{publication_ready:AtomicBool::new(ready),milestones:[AtomicU64::new((0x55u64<<40)|7),AtomicU64::new((0x55u64<<40)|(1<<32)|7)],retained_pool:AtomicU32::new(pool)})}
fn main(){let mut cases=0;
 // The physical pool is receipt metadata, independent of an engine grid and
 // of any GPU ticket wave (which may already be gone for deferred CPU work).
 let producer=p(Some(render(9,true)),None);let consumer=p(None,None);
 let node=Node{point:consumer.clone(),previous:None,dependencies:[Some(producer.clone()),None]};let mut s=Session::default();Schedule::retain_dependencies(&mut s,&node,None,[Some((0,7)),None]).unwrap();assert_eq!(s.calls,vec![((0,7),Some(9),Fence(99))]);cases+=1;
 let cs=p(Some(Receipt::Compute),None);let node=Node{point:consumer.clone(),previous:Some(producer.clone()),dependencies:[Some(producer.clone()),Some(cs)]};let mut s=Session::default();Schedule::retain_dependencies(&mut s,&node,Some((0,7)),[Some((0,7)),Some((32,4))]).unwrap();assert_eq!(s.calls,vec![((0,7),Some(9),Fence(99)),((32,4),None,Fence(99))]);cases+=1;
 let skipped=p(None,Some(producer));let node=Node{point:consumer.clone(),previous:None,dependencies:[Some(skipped),None]};let mut s=Session::default();Schedule::retain_dependencies(&mut s,&node,None,[Some((1,7)),None]).unwrap();assert_eq!(s.calls,vec![((1,7),Some(9),Fence(99))]);cases+=1;
 for invalid in [15,u32::MAX]{let node=Node{point:consumer.clone(),previous:None,dependencies:[Some(p(Some(render(invalid,true)),None)),None]};let mut s=Session::default();assert_eq!(Schedule::retain_dependencies(&mut s,&node,None,[Some((0,7)),None]),Err(EIO));assert!(s.calls.is_empty());cases+=1;}
 let receipt=render(9,false);if let Receipt::Render(r)=receipt{assert!(r.milestone(0).is_none()&&r.retained_pool().is_none());}cases+=1;
 let node=Node{point:consumer,previous:None,dependencies:[None,None]};let mut s=Session::default();Schedule::retain_dependencies(&mut s,&node,None,[None,None]).unwrap();assert!(s.calls.is_empty());cases+=1;
 println!("PASS {cases} actual producer lease lookup/getter cases; physical pool, skipped history, deduplication, compute fallback and invalid/unpublished metadata");}
'''.replace('METHODS',methods).replace('HELPER',helper)
folder=a.evidence;folder.mkdir(exist_ok=False);(folder/'harness.rs').write_text(code)
subprocess.run(['rustc','--edition=2021',str(folder/'harness.rs'),'-o',str(folder/'gate')],check=True)
result=subprocess.check_output([str(folder/'gate')],text=True);assert result.startswith('PASS 7 ')
(folder/'summary.json').write_text(json.dumps(dict(cases=7,actual_production_lookup_and_receipt_getters_compiled=True,bounded_fence_and_session_standins=True,GPU_execution=False,runner_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),result=result.strip(),source_sha256={n:hashlib.sha256((r/n).read_bytes()).hexdigest()for n in ['g17p_schedule.rs','g17p_boot.rs']}),indent=2)+'\n');print(result,end='')
