#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Exercise the actual UserVm::grow body with owned table/allocator doubles.

Inject failure at every metadata/page allocation, late collisions, and bad
addresses. Verify no active PTE changes on refusal and child-first publication.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--rustc', default='rustc')
args = p.parse_args()
source = (Path(__file__).resolve().parents[1] / 'g17p_user_vm.rs').read_text()
start = source.index('    pub(crate) fn grow(')
brace = source.index('{', start)
depth = 1
end = brace + 1
while depth:
    depth += (source[end] == '{') - (source[end] == '}')
    end += 1
method = source[start:end]
rust = r'''#![allow(dead_code)]
use std::cell::Cell;
thread_local! {static BUDGET:Cell<Option<usize>>=const{Cell::new(None)};static BARRIERS:Cell<usize>=const{Cell::new(0)};}
type Result<T=()> = std::result::Result<T,i32>;
const EINVAL:i32=22;const EBUSY:i32=16;const ENOMEM:i32=12;const GFP_KERNEL:()=();
fn allocation()->Result {BUDGET.with(|b|match b.get(){Some(0)=>Err(ENOMEM),Some(n)=>{b.set(Some(n-1));Ok(())},None=>Ok(())})}
struct KVec<T>(Vec<T>);
impl<T> KVec<T>{fn new()->Self{Self(Vec::new())}fn push(&mut self,v:T,_:())->Result{allocation()?;self.0.push(v);Ok(())}}
impl<T> std::ops::Deref for KVec<T>{type Target=[T];fn deref(&self)->&[T]{&self.0}}
impl<T> IntoIterator for KVec<T>{type Item=T;type IntoIter=std::vec::IntoIter<T>;fn into_iter(self)->Self::IntoIter{self.0.into_iter()}}
mod g17p_memory{pub fn sync(){super::BARRIERS.with(|b|b.set(b.get()+1));}}
mod candidate {
use super::*;
use std::collections::BTreeMap;
const PAGE:u64=0x4000;const ADDRESS:u64=0x3ffffffc000;
#[derive(Clone)]
pub struct UserVm {tables:BTreeMap<u64,Vec<u64>>,next:u64,writes:Vec<(u64,usize,u64)>,links:Vec<(u64,Vec<u64>)>}
impl UserVm {
pub fn new()->Self{Self{tables:BTreeMap::from([(PAGE,vec![0;2048])]),next:PAGE*2,writes:Vec::new(),links:Vec::new()}}
fn root(&self)->u64{PAGE}
fn table(&mut self)->Result<u64>{allocation()?;let p=self.next;self.next+=PAGE;self.tables.insert(p,vec![0;2048]);Ok(p)}
fn page(&self,pa:u64)->Result{if self.tables.contains_key(&pa){Ok(())}else{Err(EINVAL)}}
fn read(&self,pa:u64,index:usize)->Result<u64>{Ok(*self.tables.get(&pa).and_then(|v|v.get(index)).ok_or(EINVAL)?)}
fn write(&mut self,pa:u64,index:usize,entry:u64)->Result{
 self.page(pa)?;if index>=2048{return Err(EINVAL)}
 let child=entry&ADDRESS;
 if let Some(body)=self.tables.get(&child){self.links.push((child,body.clone()));}
 self.tables.get_mut(&pa).unwrap()[index]=entry;self.writes.push((pa,index,entry));Ok(())
}
METHOD
fn pte(&self,va:u64)->u64{let mut at=self.root();for shift in [36,25]{let e=self.read(at,((va>>shift)&if shift==36{63}else{2047})as usize).unwrap();if e==0{return 0}at=e&ADDRESS;}self.read(at,((va>>14)&2047)as usize).unwrap()}
fn reset_trace(&mut self){self.writes.clear();self.links.clear();BARRIERS.with(|b|b.set(0));}
fn check_links(&self){for (pa,body) in &self.links{assert_eq!(self.tables.get(pa).unwrap(),body,"child changed after parent publication");}assert_eq!(BARRIERS.with(|b|b.get()),3);}
}
pub fn tests(){
 let mut base=UserVm::new();base.grow(&[(0x1000000000,0x11000000000)]).unwrap();base.reset_trace();
 let mut pages=Vec::new();for b in 0..10{for p in 0..8{pages.push((0x1002000000+b*0x28000+p*PAGE,0x12000000000+(b*8+p)*PAGE));}}
 pages.extend([(0x2000000000,0x13000000000),(0x2002000000,0x13000004000),(0x2100000000,0x13000008000)]);
 let before=base.tables.clone();let mut successes=0;let mut refusals=0;
 for budget in 0..128{
  let mut vm=base.clone();vm.reset_trace();BUDGET.with(|b|b.set(Some(budget)));let r=vm.grow(&pages);BUDGET.with(|b|b.set(None));
  if r==Err(ENOMEM){refusals+=1;assert!(vm.writes.is_empty());for (pa,body) in &before{assert_eq!(vm.tables.get(pa).unwrap(),body);}vm.grow(&pages).unwrap();}
  else{r.unwrap();successes+=1;}
  vm.check_links();assert_eq!(vm.pte(0x1000000000),0x11000000000|0x00c0000000000c8b);
  for &(va,pa) in &pages{assert_eq!(vm.pte(va),pa|0x00c0000000000c8b);}
 }
 assert!(successes>0&&refusals>80);
 for last in [(0x1000000000,0x14000000000),(pages[0].0,0x14000000000),(1<<42,0x14000000000),(0x2300000001,0x14000000000),(0x2300000000,1),(0x2300000000,0)]{
  let mut vm=base.clone();let mut bad=pages.clone();bad.push(last);assert!(vm.grow(&bad).is_err());assert!(vm.writes.is_empty());for (pa,body)in &before{assert_eq!(vm.tables.get(pa).unwrap(),body);}
 }
 let mut vm=base.clone();assert_eq!(vm.grow(&[]),Err(EINVAL));assert!(vm.writes.is_empty());
 println!("PASS: actual UserVm::grow, {} injected allocation refusals, {} successful plans, child-first links, retry after failure, 7 late-collision/address/empty rejections; retained mappings unchanged",refusals,successes);
}
}
fn main(){candidate::tests();}
'''.replace('METHOD', method)
with tempfile.TemporaryDirectory() as tmp:
    root = Path(tmp)
    (root / 'tables.rs').write_text(rust)
    subprocess.run([args.rustc, '--edition=2021', '-Dwarnings', str(root / 'tables.rs'), '-o', str(root / 'tables')], check=True)
    subprocess.run([str(root / 'tables')], check=True)
