#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute warm/cold compute preparation, keeping all allocations off-lock."""
import argparse,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args();s=(Path(__file__).resolve().parents[1]/'g17p_schedule.rs').read_text();i=s.index('    fn prepare_compute(');j=s.index('{',i)+1;d=1
while d:d+=(s[j]=='{')-(s[j]=='}');j+=1
m=s[i:j]
for module,name in [('g17p_compute_queues','PreparedWork'),('g17p_compute_queues','PreparedComputeRebind'),('g17p_user_vm','TableStorage')]:m=m.replace(f'super::super::{module}::{name}',name)
code=r'''
#![allow(dead_code,unused_variables)]
use std::{cell::{Cell,RefCell,RefMut},ops::{Deref,DerefMut},sync::atomic::AtomicBool};
type Result<T=()>=std::result::Result<T,i32>;type KBox<T>=Box<T>;const ENODEV:i32=19;const EIO:i32=5;
thread_local!{static LOCKED:Cell<bool>=const{Cell::new(false)};static LOCKS:Cell<usize>=const{Cell::new(0)};static FAIL:Cell<u32>=const{Cell::new(0)};static RACE:Cell<bool>=const{Cell::new(false)};static ALLOCS:Cell<usize>=const{Cell::new(0)};}
fn locked(){assert!(LOCKED.with(Cell::get));}fn unlocked(){assert!(!LOCKED.with(Cell::get));}fn fail(n:u32)->Result{if FAIL.with(Cell::get)==n{Err(n as i32)}else{Ok(())}}
macro_rules! pr_info{($($t:tt)*)=>{}}
mod kernel{pub mod time{pub struct Delta;impl Delta{pub fn from_millis(_:u64)->Self{Self}}pub mod delay{pub fn fsleep(_:super::Delta){}}}}
mod module_parameters{pub struct Parameter;impl Parameter{pub fn value(&self)->u32{0}}pub static cpu_prepare_pause_queue:Parameter=Parameter;}
static PREPARATION_PAUSED:AtomicBool=AtomicBool::new(false);
struct RuntimeInterval;impl RuntimeInterval{fn waiting(){}fn acquired(_:(),_:(u64,u32),_:&str)->Self{Self}}
struct Client;impl Client{fn cache(&self,_:bool)->Result{unlocked();fail(3)}}
struct Snapshot;impl Snapshot{fn client(&self)->&Client{&Client}}
struct Parameters{timestamps:[u64;2]}enum Command{Compute(Parameters),Render}
struct Point{completion:i32,error:Cell<Option<i32>>}impl Point{fn finish(&self,e:Option<i32>){self.error.set(e)}}fn status(f:&i32)->i32{*f}
struct Node{staged:bool,point:Point}
struct Prepared{parameters:Vec<Command>,queue_key:(u64,u32),queue_priority:u32,compute_snapshot:Option<Snapshot>}
struct Lock(RefCell<Option<Runtime>>);struct Guard<'a>(RefMut<'a,Option<Runtime>>);impl Lock{fn lock(&self)->Guard<'_>{unlocked();LOCKED.with(|v|v.set(true));LOCKS.with(|v|v.set(v.get()+1));Guard(self.0.borrow_mut())}}
impl Deref for Guard<'_>{type Target=Option<Runtime>;fn deref(&self)->&Self::Target{&self.0}}impl DerefMut for Guard<'_>{fn deref_mut(&mut self)->&mut Self::Target{&mut self.0}}impl Drop for Guard<'_>{fn drop(&mut self){LOCKED.with(|v|v.set(false));}}
struct Device{runtime:Lock}struct Runtime{session:Session,image:()}
struct TableStorage;impl TableStorage{fn new(n:usize)->Result<Self>{unlocked();assert_eq!(n,2);ALLOCS.with(|v|v.set(v.get()+1));fail(1)?;Ok(Self)}}
struct PreparedComputeRebind;struct Rebind;impl Rebind{fn prepare(self,_:&Client)->Result<PreparedComputeRebind>{unlocked();fail(2)?;Ok(PreparedComputeRebind)}}
struct PreparedWork;struct Seed;impl Seed{fn prepare(self)->Result<Box<PreparedWork>>{unlocked();fail(5)?;Ok(Box::new(PreparedWork))}}
struct Stamps;impl Stamps{fn run(self,_:bool)->Result{unlocked();fail(4)}}
struct Session{ready:bool,enabled:bool,rebind:bool}
impl Session{
fn can_stage_independent_compute(&mut self,_:&(),key:(u64,u32),_:&Client,_:&Parameters,p:u32)->Result<(bool,bool)>{locked();assert_eq!(key,(7,8));assert_eq!(p,1);Ok((self.ready,false))}
fn compute_rebind_size(&self,_:(u64,u32),_:&Client,_:&Parameters,_:u32)->Option<usize>{locked();self.rebind.then_some(2)}
fn independent_compute_enabled(&self)->bool{locked();self.enabled}
fn compute_preparation_seed(&self,_:(u64,u32),_:&Client,_:&Parameters,_:u32,waits:&[(u8,u32)])->Result<Option<Seed>>{locked();assert_eq!(waits,&[(2,42),(4,17)]);Ok(Some(Seed))}
fn timestamp_cache_seed(&self,_:&[u64;2])->Result<Stamps>{locked();Ok(Stamps)}
fn capture_compute_rebind(&self,_:(u64,u32),_:&Client,_:&Parameters,_:u32,storage:Option<TableStorage>)->Result<Option<Option<Rebind>>>{locked();Ok(if RACE.with(Cell::get){None}else if self.rebind{storage.map(|_|Some(Rebind))}else{Some(None)})}
}
struct Schedule{nodes:Vec<Node>}impl Schedule{
fn dependencies(_:&Node,_:&Command)->Result<Option<(Option<(u8,u32)>,[Option<(u8,u32)>;2])>>{Ok(Some((Some((2,42)),[Some((2,42)),Some((4,17))])))}
METHOD
}
fn main(){let mut cases=0;for rebind in [false,true]{for ready in [false,true]{for enabled in [false,true]{for race in [false,true]{for error in 0..6{
 LOCKS.with(|v|v.set(0));ALLOCS.with(|v|v.set(0));FAIL.with(|v|v.set(error));RACE.with(|v|v.set(race));
 let d=Device{runtime:Lock(RefCell::new(Some(Runtime{image:(),session:Session{ready,enabled,rebind}})))};let p=Prepared{parameters:vec![Command::Compute(Parameters{timestamps:[17,19]})],queue_key:(7,8),queue_priority:1,compute_snapshot:Some(Snapshot)};let s=Schedule{nodes:vec![Node{staged:false,point:Point{completion:0,error:Cell::new(None)}}]};let result=s.prepare_compute(&d,&p);unlocked();
 if !ready{assert!(result.unwrap().is_none());assert_eq!(LOCKS.with(Cell::get),1);assert_eq!(ALLOCS.with(Cell::get),0);}
 else if rebind&&error==1{assert!(matches!(result,Err(1)));}
 else{assert_eq!(LOCKS.with(Cell::get),if rebind{2}else{1});let result=result.unwrap();if !enabled||race{assert!(result.is_none());}else if error==2&&rebind||error>=3{assert!(result.is_none());assert!(s.nodes[0].point.error.get().is_some());}else{let (_,_,root)=result.unwrap();assert_eq!(root.is_some(),rebind);}}
 cases+=1;
 }}}}}
 println!("PASS {cases} production compute preparations: one warm/two cold locks, off-lock allocation/cache/encoding, control waits, owner races, dependency deduplication and error attribution");}
'''.replace('METHOD',m)
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code);subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True);out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);print(out,end='')
