#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute actual batched support preparation with warm/cold/error/race inputs."""
import argparse
import subprocess
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence', type=Path, required=True)
a = p.parse_args()
s = (Path(__file__).resolve().parents[1] / 'g17p_schedule.rs').read_text()
start = s.index('    fn prepare_render_support(')
end = s.index('{', start) + 1
depth = 1
while depth:
    depth += (s[end] == '{') - (s[end] == '}')
    end += 1
method = s[start:end].replace('super::super::g17p_user_vm::TableStorage', 'TableStorage')
code = r'''
#![allow(dead_code)]
use std::{cell::{Cell,RefCell,RefMut},ops::{Deref,DerefMut}};
type Result<T=()> = std::result::Result<T,i32>;const ENODEV:i32=19;const EIO:i32=5;
thread_local!{static LOCKED:Cell<bool>=const{Cell::new(false)};
 static LOCKS:Cell<usize>=const{Cell::new(0)};static MODE:Cell<usize>=const{Cell::new(0)};}
fn locked(){assert!(LOCKED.with(Cell::get))}fn unlocked(){assert!(!LOCKED.with(Cell::get))}
#[derive(Clone,Copy)]struct Parameters{queue_owner:Option<(u64,u32)>}
#[derive(Clone,Copy)]enum Command{Render(Parameters),Compute}
struct Client;struct Snapshot;impl Snapshot{fn client(&self)->&Client{&Client}}
struct Point{completion:Cell<i32>}impl Point{fn finish(&self,e:Option<i32>){self.completion.set(e.unwrap_or(1))}}
fn status(f:&Cell<i32>)->i32{f.get()}
struct Node{staged:bool,ready:bool,point:Point}
struct Prepared{parameters:Vec<Command>,render_snapshot:Option<Snapshot>,queue_key:(u64,u32)}
struct Session{scratch:bool,template:bool}
struct Runtime{session:Session}struct Device{runtime:Lock}
struct Lock(RefCell<Option<Runtime>>);struct Guard<'a>(RefMut<'a,Option<Runtime>>);
impl Lock{fn lock(&self)->Guard<'_>{unlocked();LOCKED.with(|v|v.set(true));LOCKS.with(|v|v.set(v.get()+1));Guard(self.0.borrow_mut())}}
impl Deref for Guard<'_>{type Target=Option<Runtime>;fn deref(&self)->&Self::Target{&self.0}}
impl DerefMut for Guard<'_>{fn deref_mut(&mut self)->&mut Self::Target{&mut self.0}}
impl Drop for Guard<'_>{fn drop(&mut self){LOCKED.with(|v|v.set(false))}}
struct ScratchSeed;impl ScratchSeed{fn prepare(self)->Result<()>{unlocked();if MODE.with(Cell::get)==1{Err(41)}else{Ok(())}}}
struct TemplateSeed;impl TemplateSeed{fn prepare(self)->Result<()>{unlocked();if MODE.with(Cell::get)==2{Err(42)}else{Ok(())}}}
struct TableStorage;impl TableStorage{fn new(n:usize)->Result<Self>{unlocked();assert_eq!(n,2);if MODE.with(Cell::get)==3{Err(12)}else{Ok(Self)}}}
impl Session{
 fn render_scratch_seed_for(&self,_:&Client,_:&Parameters,_:(u64,u32))->Result<Option<ScratchSeed>>{locked();Ok(self.scratch.then_some(ScratchSeed))}
 fn render_template_size(&self,_:&Client,p:&Parameters)->Result<Option<usize>>{locked();assert!(!self.scratch,"size only after scratch publication");assert_eq!(p.queue_owner,Some((7,8)));Ok(self.template.then_some(2))}
 fn commit_render_scratch(&mut self,_:())->Result{locked();self.scratch=false;Ok(())}
 fn capture_render_template(&self,_:&Client,_:&Parameters,_:TableStorage)->Result<Option<TemplateSeed>>{locked();Ok((MODE.with(Cell::get)!=4).then_some(TemplateSeed))}
 fn commit_render_template(&mut self,_:&Client,_:&Parameters,_:())->Result{locked();self.template=false;Ok(())}
}
struct RuntimeInterval;impl RuntimeInterval{fn waiting(){}fn acquired(_:(),_:(u64,u32),_:&str){}}
struct Schedule{nodes:Vec<Node>}
impl Schedule{
 fn dependencies(n:&Node,_:&Command)->Result<Option<()>>{Ok(n.ready.then_some(()))}
 METHOD
}
fn main(){let mut cases=0;
 for scratch in [false,true]{for template in [false,true]{for mode in 0..5{
  MODE.with(|v|v.set(mode));LOCKS.with(|v|v.set(0));
  let dev=Device{runtime:Lock(RefCell::new(Some(Runtime{session:Session{scratch,template}})))};
  let p=Prepared{parameters:vec![Command::Render(Parameters{queue_owner:None})],render_snapshot:Some(Snapshot),queue_key:(7,8)};
  let s=Schedule{nodes:vec![Node{staged:false,ready:true,point:Point{completion:Cell::new(0)}}]};
  let result=s.prepare_render_support(&dev,&p);unlocked();
  if mode==0{
   assert!(result.is_ok());assert_eq!(LOCKS.with(Cell::get),1+usize::from(scratch)+2*usize::from(template));
   let held=dev.runtime.lock();let session=&held.as_ref().unwrap().session;assert!(!session.scratch&&!session.template);
  }else if mode==1&&scratch{assert_eq!(s.nodes[0].point.completion.get(),41)}
  else if mode==2&&template{assert_eq!(s.nodes[0].point.completion.get(),42)}
  else if mode==3&&template{assert_eq!(result,Err(12))}
  else{assert!(result.is_ok())}
  cases+=1;
 }}}
 for (staged,ready) in [(true,true),(false,false)]{
  let d=Device{runtime:Lock(RefCell::new(None))};let p=Prepared{parameters:vec![Command::Render(Parameters{queue_owner:None})],render_snapshot:None,queue_key:(7,8)};
  let s=Schedule{nodes:vec![Node{staged,ready,point:Point{completion:Cell::new(0)}}]};LOCKS.with(|v|v.set(0));
  s.prepare_render_support(&d,&p).unwrap();assert_eq!(LOCKS.with(Cell::get),0);
 }
 println!("PASS {} actual support-preparation cases: one warm read acquisition, post-scratch sizing, off-lock allocations/encoding, failed preparation/storage and stale capture retry",cases);
}
'''.replace('METHOD', method)
a.evidence.mkdir(parents=True, exist_ok=False)
(a.evidence / 'test.rs').write_text(code)
subprocess.run(['rustc', '--edition=2021', '-O', str(a.evidence / 'test.rs'),
                '-o', str(a.evidence / 'gate')], check=True)
result = subprocess.check_output([str(a.evidence / 'gate')], text=True)
(a.evidence / 'output.txt').write_text(result)
print(result, end='')
