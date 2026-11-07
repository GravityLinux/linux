#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare actual Point refresh with original publication/error-join behavior."""
import argparse,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args();r=Path(__file__).resolve().parents[1]
def item(s,m):
 i=s.index(m);j=s.index('{',i)+1;depth=1
 while depth:depth+=(s[j]=='{')-(s[j]=='}');j+=1
 return s[i:j]
s=(r/'g17p_schedule.rs').read_text();old=subprocess.check_output(['git','show','dad8de9ac37d:drivers/gpu/drm/asahi/g17p_schedule.rs'],text=True)
methods=item(s,'    fn finish(&self')+item(s,'    fn refresh(&self')+item(old,'    fn refresh(&self').replace('fn refresh(','fn old_refresh(')
code=r'''
use std::cell::{Cell,RefCell,RefMut};
#[derive(Clone,Copy)]struct Error(i32);impl Error{fn from_errno(v:i32)->Self{Self(v)}}
struct Fence{state:Cell<i32>,error:Cell<i32>,signals:Cell<u32>}
fn fence(v:i32)->Fence{Fence{state:Cell::new(v),error:Cell::new(if v<0{v}else{0}),signals:Cell::new(0)}}
fn status(f:&Fence)->i32{f.state.get()}
impl Fence{fn set_error(&self,e:Error){self.error.set(e.0)}fn signal(&self){self.signals.set(self.signals.get()+1);if self.state.get()==0{self.state.set(if self.error.get()<0{self.error.get()}else{1})}}}
struct Lock<T>{value:RefCell<T>,locks:Cell<u32>}
fn lock<T>(v:T)->Lock<T>{Lock{value:RefCell::new(v),locks:Cell::new(0)}}
impl<T>Lock<T>{fn lock(&self)->RefMut<'_,T>{self.locks.set(self.locks.get()+1);self.value.borrow_mut()}}
struct Receipt{fence:Fence,published:bool}impl Receipt{fn fence(&self)->&Fence{&self.fence}fn published(&self)->bool{self.published}}
struct Point{completion:Fence,publication:Fence,previous:Lock<Option<()>>,receipt:Lock<Option<Receipt>>,dependencies:[Option<Fence>;2]}
impl Point{METHODS}
fn main(){let mut cases=0;
 for complete in [-5,0,1]{for published in [0,1]{if complete!=0&&published==0{continue;}
 for previous in [false,true]{
 if complete>0 && previous{continue;}
 for receipt in [false,true]{for retired in [-5,0,1]{for ready in [false,true]{for d0 in [-7,0,1,2]{for d1 in [-7,0,1,2]{
  let make=||Point{completion:fence(complete),publication:fence(published),previous:lock(previous.then_some(())),receipt:lock(receipt.then(||Receipt{fence:fence(retired),published:ready})),dependencies:[d0,d1].map(|v|(v!=2).then(||fence(v)))};
  let old=make();let new=make();old.old_refresh();new.refresh();
  assert_eq!(status(&old.completion),status(&new.completion));assert_eq!(status(&old.publication),status(&new.publication));assert_eq!(old.previous.value.borrow().is_some(),new.previous.value.borrow().is_some());
  if complete>0{assert_eq!(new.receipt.locks.get(),0)}
  cases+=1;
 }}}}}}}}
 println!("PASS {cases} actual old/new Point refresh comparisons: completed histories avoid receipt locks, pending/error producer joins preserve outcomes");}
'''.replace('METHODS',methods)
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);print(out,end='')
