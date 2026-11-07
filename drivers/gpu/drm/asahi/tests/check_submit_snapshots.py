#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exercise actual admission snapshot policy and unchanged validation in both ABIs."""
import argparse,hashlib,json,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',required=True,type=Path);a=p.parse_args();root=Path(__file__).resolve().parents[1]
s=(root/'g17p_drm.rs').read_text();old=subprocess.check_output(['git','show','6bb0f5e1bc25:drivers/gpu/drm/asahi/g17p_drm.rs'],text=True)
def block(source,new):
 at=source.index('        let '+('scheduled = scheduling::Schedule::enabled();' if new else 'render_snapshot = if render {'),source.index('    fn submit('))
 return source[at:source.index('        let mut buffers = KVec::new();',at)]
code=r'''
#![allow(dead_code)]
use std::cell::Cell;
type Result<T> = std::result::Result<T,i32>;const EINVAL:i32=22;
thread_local!{static SCHEDULED:Cell<bool>=const{Cell::new(false)};static CLONES:Cell<usize>=const{Cell::new(0)};static VIEWS:Cell<u32>=const{Cell::new(0)};static FAIL:Cell<usize>=const{Cell::new(usize::MAX)};}
#[derive(Clone,Copy)]enum GpuAbi{G16C,G17P}
struct Device{abi:GpuAbi}
struct Client{bindings:u64}struct Snapshot(Client);struct Vm;
impl Snapshot{fn client(&self)->&Client{&self.0}fn deferred_client(&self)->Result<Client>{let n=CLONES.with(|x|{x.set(x.get()+1);x.get()});if FAIL.with(|x|x.get()==n){return Err(12)}Ok(Client{bindings:self.0.bindings})}}
impl Vm{fn cached_snapshot(&self,owner:(u64,u32),render:bool)->Result<Snapshot>{assert_eq!(owner,(9,3));VIEWS.with(|x|x.set(x.get()|if render{1}else{2}));Ok(Snapshot(Client{bindings:0xdeadbeef}))}}
struct Parameters{valid:bool,overlap:bool}impl Parameters{fn validate(&self)->Result<()>{if self.valid{Ok(())}else{Err(EINVAL)}}}
enum Command{Render(Parameters),Compute}
mod g17p_render_runtime{use super::*;pub fn validate_client(c:&Client,p:&Parameters)->Result<()>{assert_eq!(c.bindings,0xdeadbeef);p.validate()?;if p.overlap{Err(EINVAL)}else{Ok(())}}}
mod scheduling{pub struct Schedule;impl Schedule{pub fn enabled()->bool{super::SCHEDULED.with(|x|x.get())}}}
mod driver{use super::*;
fn run(new:bool,render:bool,compute:bool,dev:&Device,parameters:Vec<Command>)->Result<(bool,bool,bool,bool)>{
 let vm=Vm;let owner=(9,3);
 if new{NEW
 Ok((render_snapshot.is_some(),compute_snapshot.is_some(),render_client.is_some(),compute_client.is_some()))
 }else{OLD
 let scheduled=scheduling::Schedule::enabled();
 Ok((render_snapshot.is_some(),compute_snapshot.is_some(),!scheduled&&render_client.is_some(),!scheduled&&compute_client.is_some()))}
}
pub fn test(){
 let mut cases=0;
 for scheduled in [false,true]{for abi in [GpuAbi::G16C,GpuAbi::G17P]{for render in [false,true]{for compute in [false,true]{for valid in [false,true]{for overlap in [false,true]{
  SCHEDULED.with(|x|x.set(scheduled));
  let run_case=|new|{CLONES.with(|x|x.set(0));VIEWS.with(|x|x.set(0));let mut cmds=Vec::new();if render{cmds.push(Command::Render(Parameters{valid,overlap}));}if compute{cmds.push(Command::Compute);}
   let result=run(new,render,compute,&Device{abi},cmds);(result,CLONES.with(Cell::get),VIEWS.with(Cell::get))};
  let (old,old_clones,old_views)=run_case(false);let(new,new_clones,new_views)=run_case(true);
  match(old,new){
   (Err(a),Err(b))=>assert_eq!(a,b),
   (Ok(a),Ok(b))=>{assert_eq!((a.0,a.2,a.3),(b.0,b.2,b.3));if scheduled{assert_eq!(new_clones,0);assert_eq!(b.1,compute);assert_eq!(new_views,u32::from(render)|u32::from(compute)*2);assert_eq!(old_clones,usize::from(render)+usize::from(render||compute));}else{assert_eq!(a,b);assert_eq!(new_clones,old_clones);assert_eq!(new_views,old_views);}},
   pair=>panic!("validation changed: {:?}",pair)
  }cases+=1;
 }}}}}}
 // Legacy bootstrap still reports either deferred allocation failure.
 SCHEDULED.with(|x|x.set(false));for fail in [1,2]{FAIL.with(|x|x.set(fail));for new in [false,true]{CLONES.with(|x|x.set(0));assert_eq!(run(new,true,true,&Device{abi:GpuAbi::G16C},vec![Command::Render(Parameters{valid:true,overlap:false}),Command::Compute]),Err(12));}}
 println!("PASS {} actual old/new admission cases: G16C/G17P validation, render/compute/mixed snapshots, zero scheduled scratch clients, retained legacy primer and allocation errors",cases);
}}
fn main(){driver::test()}
'''.replace('NEW',block(s,True)).replace('OLD',block(old,False))
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);(a.evidence/'source.json').write_text(json.dumps({'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'GPU_execution':False},indent=2)+'\n');print(out,end='')
