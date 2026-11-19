#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Run production receipt binding: retiring jobs must not enqueue themselves."""
import argparse,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args();s=(Path(__file__).resolve().parents[1]/'g17p_async.rs').read_text();i=s.index('            let result = e.progress()?;',s.index('    fn advance('));j=s.index('            Ok(result)',i)
block=s[i:j]
code=r'''
#![allow(dead_code)]
use std::cell::Cell;
type Result<T>=std::result::Result<T,i32>;
thread_local!{static BINDS:Cell<usize>=const{Cell::new(0)};static FAIL:Cell<bool>=const{Cell::new(false)};}
struct Fence;struct Callback{fence:Option<Fence>}impl Callback{fn bind(&mut self,f:Fence)->Result<()>{BINDS.with(|v|v.set(v.get()+1));if FAIL.with(Cell::get){Err(42)}else{self.fence=Some(f);Ok(())}}}
struct Schedule;impl Schedule{fn receipt_fence(&self,_:usize,_:bool)->Option<Fence>{Some(Fence)}}
struct Execution{schedule:Option<Schedule>,receipt_callbacks:Vec<[Callback;2]>,result:Option<Option<i32>>}
impl Execution{fn progress(&mut self)->Result<Option<Option<i32>>>{Ok(self.result)}}
fn run(e:&mut Execution)->Result<Option<Option<i32>>>{BLOCK Ok(result)}
fn main(){let mut cases=0;for result in [None,Some(None),Some(Some(5))]{for scheduled in [false,true]{for fail in [false,true]{for count in 0..33{
 BINDS.with(|v|v.set(0));FAIL.with(|v|v.set(fail));let mut e=Execution{schedule:scheduled.then_some(Schedule),receipt_callbacks:(0..count).map(|_|[Callback{fence:None},Callback{fence:None}]).collect(),result};let r=run(&mut e);
 if result.is_some()||!scheduled||count==0{assert_eq!(r,Ok(result));assert_eq!(BINDS.with(Cell::get),0);}
 else if fail{assert_eq!(r,Err(42));assert_eq!(BINDS.with(Cell::get),1);}
 else{assert_eq!(r,Ok(None));assert_eq!(BINDS.with(Cell::get),count*2);BINDS.with(|v|v.set(0));assert_eq!(run(&mut e),Ok(None));assert_eq!(BINDS.with(Cell::get),0);}
 cases+=1;
 }}}}println!("PASS {cases} production receipt-binding passes: pending/staged/retiring jobs, errors, unused slots and already-bound callbacks; terminal jobs never register or self-wake");}
'''.replace('BLOCK',block)
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code);subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True);out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);print(out,end='')
