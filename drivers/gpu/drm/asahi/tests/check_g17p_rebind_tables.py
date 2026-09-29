#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Exercise actual UserVm rebind with allocation faults and publication traces."""
import argparse
from pathlib import Path
import subprocess
import tempfile

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--rustc', default='rustc')
args = p.parse_args()
source = (Path(__file__).resolve().parents[1] / 'g17p_user_vm.rs').read_text()
def method(name):
    start = source.index('    pub(crate) fn ' + name + '(')
    brace = source.index('{', start)
    depth, end = 1, brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]
rust = r'''#![allow(dead_code)]
use std::cell::{Cell, RefCell};
thread_local! {static BUDGET:Cell<Option<usize>>=const{Cell::new(None)};static TRACE:RefCell<Vec<(u64,usize,u64)>>=const{RefCell::new(Vec::new())};}
type Result<T=()> = std::result::Result<T,i32>;
const EINVAL:i32=22;const EBUSY:i32=16;const ENOMEM:i32=12;const GFP_KERNEL:()=();
fn allocation()->Result {BUDGET.with(|b|match b.get(){Some(0)=>Err(ENOMEM),Some(n)=>{b.set(Some(n-1));Ok(())},None=>Ok(())})}
struct KVec<T>(Vec<T>);
impl<T> KVec<T>{fn new()->Self{Self(Vec::new())}fn push(&mut self,v:T,_:())->Result{allocation()?;self.0.push(v);Ok(())}}
impl<T> std::ops::Deref for KVec<T>{type Target=[T];fn deref(&self)->&[T]{&self.0}}
impl<'a,T> IntoIterator for &'a KVec<T>{type Item=&'a T;type IntoIter=std::slice::Iter<'a,T>;fn into_iter(self)->Self::IntoIter{self.0.iter()}}
mod g17p_memory{pub fn sync(){}}
mod candidate {
use super::*;
use std::collections::BTreeMap;
const PAGE:u64=0x4000;const ADDRESS:u64=0x3ffffffc000;const FLAGS:u64=0x00c0000000000c8b;
#[derive(Clone)]struct Page {pa:u64,body:Vec<Cell<u64>>}
#[derive(Clone)]pub struct UserVm {tables:BTreeMap<u64,Page>,next:u64}
impl UserVm {
fn new()->Self{Self{tables:BTreeMap::from([(PAGE,Page{pa:PAGE,body:vec![Cell::new(0);2048]})]),next:PAGE*2}}
fn root(&self)->u64{PAGE}
fn table(&mut self)->Result<u64>{allocation()?;let p=self.next;self.next+=PAGE;self.tables.insert(p,Page{pa:p,body:vec![Cell::new(0);2048]});Ok(p)}
fn page(&self,pa:u64)->Result<&Page>{self.tables.get(&pa).ok_or(EINVAL)}
fn read(&self,pa:u64,index:usize)->Result<u64>{Ok(self.page(pa)?.body.get(index).ok_or(EINVAL)?.get())}
fn store(page:&Page,index:usize,entry:u64){page.body[index].set(entry);TRACE.with(|t|t.borrow_mut().push((page.pa,index,entry)));}
fn write(&mut self,pa:u64,index:usize,entry:u64)->Result{if index>=2048{return Err(EINVAL)}Self::store(self.page(pa)?,index,entry);Ok(())}
fn invalidate(context:u16){TRACE.with(|t|t.borrow_mut().push((0,context as usize,0)));}
METHODS
fn snapshot(&self)->BTreeMap<u64,Vec<u64>>{self.tables.iter().map(|(&pa,p)|(pa,p.body.iter().map(Cell::get).collect())).collect()}
}
fn trace()->Vec<(u64,usize,u64)>{TRACE.with(|t|t.borrow().clone())}
fn reset(){TRACE.with(|t|t.borrow_mut().clear());}
fn unchanged(vm:&UserVm,before:&BTreeMap<u64,Vec<u64>>){let after=vm.snapshot();for(pa,b)in before{assert_eq!(&after[pa],b);}assert!(trace().is_empty());}
pub fn tests(){
 let initial=[(0x1000000000,0x11000000000),(0x1000004000,0x11000004000),(0x1002000000,0x12000000000)];
 let mut base=UserVm::new();base.grow(&initial).unwrap();reset();let before=base.snapshot();
 let mut changes=vec![(initial[0].0,initial[0].1|FLAGS,0),(initial[1].0,initial[1].1|FLAGS,0x13000000000|FLAGS)];
 for i in 0..40 {changes.push((0x2000000000+i*0x2000000,0,(0x14000000000+i*PAGE)|FLAGS));}
 // Read-only permission transition and unchanged leaf are separate cases.
 changes.push((0x2100004000,0,0x15000000000|(FLAGS & !(1<<54))));
 let mut refusals=0;let mut successes=0;
 for budget in 0..300 {
  let mut vm=base.clone();reset();BUDGET.with(|b|b.set(Some(budget)));let r=vm.rebind(&changes,1);BUDGET.with(|b|b.set(None));
  if r==Err(ENOMEM){refusals+=1;unchanged(&vm,&before);vm.rebind(&changes,1).unwrap();}else{r.unwrap();successes+=1;}
  for &(va,_,new)in &changes{assert_eq!(vm.pte(va).unwrap(),new);}
  assert_eq!(vm.pte(initial[2].0).unwrap(),initial[2].1|FLAGS,"private growth leaf changed");
  let log=trace();let flushes:Vec<_>=log.iter().enumerate().filter(|(_,v)|v.0==0).collect();assert_eq!(flushes.len(),2);assert_eq!(flushes[1].0,log.len()-1);
  assert_eq!(flushes[0].0,2,"exactly two old leaves broken before first flush");assert!(log[..flushes[0].0].iter().all(|v|v.2==0));
  // Each new child is fully populated before its parent link is installed.
  let mut replay=before.clone();for(&pa,p)in &vm.tables{replay.entry(pa).or_insert(vec![0;p.body.len()]);}
  for &(table,index,value)in &log{if table==0{continue}let child=value&ADDRESS;if value&0xfff==3{assert_eq!(replay[&child],vm.snapshot()[&child]);}replay.get_mut(&table).unwrap()[index]=value;}
 }
 assert!(refusals>100&&successes>0);
 for last in [(initial[2].0,0,0x16000000000|FLAGS),(changes[0].0,changes[0].1,0),(1<<42,0,FLAGS),(0x3200000001,0,FLAGS),(0x3200000000,0,1),(0x3200000000,0,FLAGS),(0x3200000000,0,(1<<63)|FLAGS)]{
  let mut vm=base.clone();reset();let mut bad=changes.clone();bad.push(last);assert_eq!(vm.rebind(&bad,1),Err(EINVAL));unchanged(&vm,&before);
 }
 for ctx in [0,64,u16::MAX]{let mut vm=base.clone();reset();assert_eq!(vm.rebind(&changes,ctx),Err(EINVAL));unchanged(&vm,&before);}
 let mut vm=base.clone();reset();vm.rebind(&[(initial[0].0,initial[0].1|FLAGS,initial[0].1|FLAGS)],1).unwrap();assert_eq!(trace(),vec![(0,1,0),(0,1,0)]);
 println!("PASS: actual UserVm::rebind/pte; {} allocation refusals with unchanged live tables and successful retry; {} successful replacement/removal/addition plans; private growth preservation, break-before-make, child-first links, 10 invalid plans, unchanged-leaf preservation",refusals,successes);
}
}
fn main(){candidate::tests();}
'''.replace('METHODS', '\n'.join(method(n) for n in ('pte','rebind','grow')))
with tempfile.TemporaryDirectory() as tmp:
    root = Path(tmp)
    (root / 'tables.rs').write_text(rust)
    subprocess.run([args.rustc, '--edition=2021', '-Dwarnings', str(root / 'tables.rs'), '-o', str(root / 'tables')], check=True)
    subprocess.run([str(root / 'tables')], check=True)
