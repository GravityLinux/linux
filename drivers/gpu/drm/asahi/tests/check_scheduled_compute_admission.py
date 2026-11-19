#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute actual compute preparation; control waits must not rebuild tables."""
import argparse
from pathlib import Path
import subprocess
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
s=(Path(__file__).resolve().parents[1]/'g17p_schedule.rs').read_text()
at=s.index('    fn prepare_compute(');b=s.index('{',at);end=b+1;depth=1
while depth:depth+=(s[end]=='{')-(s[end]=='}');end+=1
method=s[at:end]
for module,name in [('g17p_compute_queues','PreparedWork'),('g17p_compute_queues','PreparedComputeRebind'),('g17p_user_vm','TableStorage')]:method=method.replace('super::super::'+module+'::'+name,name)
code=r'''#![allow(dead_code,unused_imports,non_upper_case_globals,unused_variables,irrefutable_let_patterns)]
use std::cell::{Cell,RefCell,RefMut};use std::sync::atomic::AtomicBool;
type KBox<T>=Box<T>;type Result<T>=std::result::Result<T,i32>;const EIO:i32=5;const ENODEV:i32=19;
thread_local!{static TABLES:Cell<u32>=const{Cell::new(0)};static PLANS:Cell<u32>=const{Cell::new(0)};static DESCRIPTORS:Cell<u32>=const{Cell::new(0)};}
mod kernel {pub use crate::dev_err;pub mod time{pub struct Delta;impl Delta{pub fn from_millis(_:u64)->Self{Self}}pub mod delay{pub fn fsleep(_:super::Delta){}}}}
#[macro_export]macro_rules! dev_err{($($t:tt)*)=>{()}}
macro_rules! pr_info{($($t:tt)*)=>{()}}
mod module_parameters{pub struct P;impl P{pub fn value(&self)->u32{0}}pub static cpu_prepare_pause_queue:P=P;}
static PREPARATION_PAUSED:AtomicBool=AtomicBool::new(false);
struct Client;impl Client{fn cache(&self,_:bool)->Result<()>{Ok(())}}
struct Snapshot(Client);impl Snapshot{fn client(&self)->&Client{&self.0}}
#[derive(Clone,Copy)]struct Parameters{timestamps:[u64;2]}
enum Command{Compute(Parameters)}
struct Prepared{parameters:[Command;1],compute_snapshot:Option<Snapshot>,compute_client:Option<Client>,queue_key:(u64,u32),queue_priority:u32}
struct Point{completion:i32,error:Cell<Option<i32>>}impl Point{fn finish(&self,e:Option<i32>){self.error.set(e)}}
struct Node{point:Point,staged:bool}
fn status(f:&i32)->i32{*f}
struct RuntimeInterval;impl RuntimeInterval{fn waiting(){}fn acquired(_:(),_:(u64,u32),_:&str){}}
struct Mutex(RefCell<Option<Runtime>>);impl Mutex{fn lock(&self)->RefMut<'_,Option<Runtime>>{self.0.borrow_mut()}}
struct Device{runtime:Mutex}struct Runtime{session:Session,image:()}
struct Session{waiting:u32,enabled:bool,error:bool,plan_error:bool,admissions:u32}
impl Session{
 fn can_stage_independent_compute(&mut self,_:&(),_:(u64,u32),_:&Client,_:&Parameters,_:u32)->Result<(bool,bool)>{self.admissions+=1;if self.error{return Err(EIO)}if self.waiting>0{self.waiting-=1;Ok((false,true))}else{Ok((true,false))}}
 fn independent_compute_enabled(&self)->bool{self.enabled}
 fn compute_rebind_size(&self,_:(u64,u32),_:&Client,_:&Parameters,_:u32)->Option<usize>{Some(5)}
 fn compute_preparation_seed(&self,_:(u64,u32),_:&Client,_:&Parameters,_:u32,_:&[(u8,u32)])->Result<Option<DescriptorSeed>>{Ok(Some(DescriptorSeed))}
 fn timestamp_cache_seed(&self,_:&[u64])->Result<Cache>{Ok(Cache)}
 fn capture_compute_rebind(&self,_:(u64,u32),_:&Client,_:&Parameters,_:u32,_:Option<TableStorage>)->Result<Option<Option<RebindSeed>>>{Ok(Some(Some(RebindSeed{error:self.plan_error})))}
}
struct TableStorage;impl TableStorage{fn new(_:usize)->Result<Self>{TABLES.with(|v|v.set(v.get()+1));Ok(Self)}}
struct PreparedComputeRebind;struct PreparedWork;
struct RebindSeed{error:bool}impl RebindSeed{fn prepare(self,_:&Client)->Result<PreparedComputeRebind>{PLANS.with(|v|v.set(v.get()+1));if self.error{Err(EIO)}else{Ok(PreparedComputeRebind)}}}
struct DescriptorSeed;impl DescriptorSeed{fn prepare(self)->Result<Box<PreparedWork>>{DESCRIPTORS.with(|v|v.set(v.get()+1));Ok(Box::new(PreparedWork))}}
struct Cache;impl Cache{fn run(&self,_:bool)->Result<()>{Ok(())}}
struct Schedule{nodes:Vec<Node>}
impl Schedule{
 fn dependencies(_:&Node,_:&Command)->Result<Option<(Option<(u8,u32)>,[Option<(u8,u32)>;2])>>{Ok(Some((None,[None,None])))}
 METHOD
}
fn main(){
 let d=Device{runtime:Mutex(RefCell::new(Some(Runtime{session:Session{waiting:8,enabled:true,error:false,plan_error:false,admissions:0},image:()})))};
 let p=Prepared{parameters:[Command::Compute(Parameters{timestamps:[0;2]})],compute_snapshot:Some(Snapshot(Client)),compute_client:Some(Client),queue_key:(8,1),queue_priority:1};
 let s=Schedule{nodes:vec![Node{point:Point{completion:0,error:Cell::new(None)},staged:false}]};
 for _ in 0..8 {assert!(s.prepare_compute(&d,&p).unwrap().is_none());assert_eq!(TABLES.with(Cell::get),0);assert_eq!(PLANS.with(Cell::get),0);assert_eq!(DESCRIPTORS.with(Cell::get),0);}
 assert!(s.prepare_compute(&d,&p).unwrap().is_some());assert_eq!(PLANS.with(Cell::get),1);assert_eq!(TABLES.with(Cell::get),1);assert_eq!(DESCRIPTORS.with(Cell::get),1);
 // A failed admission cannot allocate a speculative table plan either.
 d.runtime.lock().as_mut().unwrap().session.error=true;
 assert!(s.prepare_compute(&d,&p).is_err());assert_eq!(PLANS.with(Cell::get),1);
 d.runtime.lock().as_mut().unwrap().session.error=false;d.runtime.lock().as_mut().unwrap().session.plan_error=true;
 assert!(s.prepare_compute(&d,&p).unwrap().is_none());assert_eq!(s.nodes[0].point.error.get(),Some(EIO));
 println!("PASS actual compute preparation: eight blocked admissions allocate no tables/descriptors/rebinds, one ready admission builds one plan, admission/plan errors stay owned");
}
'''.replace('METHOD',method)
a.evidence.mkdir(exist_ok=False,parents=True);(a.evidence/'harness.rs').write_text(code)
subprocess.run(['rustc','--edition=2021',str(a.evidence/'harness.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);print(out,end='')
