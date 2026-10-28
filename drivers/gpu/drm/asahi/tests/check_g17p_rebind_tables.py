#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exercise actual UserVm rebind with allocation faults and publication traces."""
import argparse
import hashlib,json
from pathlib import Path
import subprocess
import tempfile

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--rustc', default='rustc')
p.add_argument('--evidence',type=Path)
args = p.parse_args()
source = (Path(__file__).resolve().parents[1] / 'g17p_user_vm.rs').read_text()
def method(name):
    start = source.index(('    fn ' if name.startswith('leaf(') else '    pub(crate) fn ') + name)
    brace = source.index('{', start)
    depth, end = 1, brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]
rust = r'''#![allow(dead_code)]
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64,Ordering};
fn sort_by_key<T,K:Ord>(v:&mut[T],f:impl FnMut(&T)->K){v.sort_by_key(f);}
thread_local! {static BUDGET:Cell<Option<usize>>=const{Cell::new(None)};static TRACE:RefCell<Vec<(u64,usize,u64)>>=const{RefCell::new(Vec::new())};}
type Result<T=()> = std::result::Result<T,i32>;
const EINVAL:i32=22;const EBUSY:i32=16;const ENOMEM:i32=12;const GFP_KERNEL:()=();
fn allocation()->Result {BUDGET.with(|b|match b.get(){Some(0)=>Err(ENOMEM),Some(n)=>{b.set(Some(n-1));Ok(())},None=>Ok(())})}
struct KVec<T>(Vec<T>);
impl<T> KVec<T>{fn with_capacity(n:usize,_:())->Result<Self>{allocation()?;Ok(Self(Vec::with_capacity(n)))}fn extend_from_slice(&mut self,items:&[T],_:())->Result where T:Clone{allocation()?;self.0.extend_from_slice(items);Ok(())}fn new()->Self{Self(Vec::new())}fn push(&mut self,v:T,_:())->Result{allocation()?;self.0.push(v);Ok(())}}
impl<T> std::ops::Deref for KVec<T>{type Target=[T];fn deref(&self)->&[T]{&self.0}}
impl<T>std::ops::DerefMut for KVec<T>{fn deref_mut(&mut self)->&mut[T]{&mut self.0}}
impl<'a,T> IntoIterator for &'a KVec<T>{type Item=&'a T;type IntoIter=std::slice::Iter<'a,T>;fn into_iter(self)->Self::IntoIter{self.0.iter()}}
mod g17p_memory{pub fn sync(){}}
mod candidate {
use super::*;
use std::collections::BTreeMap;
const PAGE:u64=0x4000;const ADDRESS:u64=0x3ffffffc000;const FLAGS:u64=0x00c0000000000c8b;
#[derive(Clone)]struct Page {pa:u64,body:Vec<Cell<u64>>}
pub struct UserVm {tables:BTreeMap<u64,Page>,next:u64,generation:AtomicU64}
impl Clone for UserVm{fn clone(&self)->Self{Self{tables:self.tables.clone(),next:self.next,generation:AtomicU64::new(self.generation.load(Ordering::Relaxed))}}}
PLAN
impl UserVm {
fn new()->Self{Self{tables:BTreeMap::from([(PAGE,Page{pa:PAGE,body:vec![Cell::new(0);2048]})]),next:PAGE*2,generation:AtomicU64::new(0)}}
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
 // A prepared or cancelled plan cannot change the executable tree or TLB.
 let mut cancelled=base.clone();reset();
 let plan=cancelled.prepare_rebind(&changes,&[1]).unwrap();drop(plan);
 unchanged(&cancelled,&before);
 let mut unpublished=base.clone();reset();
 unpublished.prepare_rebind(&changes,&[1]).unwrap().commit_unpublished();
 for &(va,_,new)in &changes{assert_eq!(unpublished.pte(va).unwrap(),new);}
 assert!(trace().iter().all(|row|row.0!=0),"unpublished plan invalidated a live ASID");
 let mut refusals=0;let mut successes=0;
 for budget in 0..300 {
  let mut vm=base.clone();reset();BUDGET.with(|b|b.set(Some(budget)));let r=vm.rebind(&changes,&[1]);BUDGET.with(|b|b.set(None));
  if r==Err(ENOMEM){refusals+=1;unchanged(&vm,&before);vm.rebind(&changes,&[1]).unwrap();}else{r.unwrap();successes+=1;}
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
  let mut vm=base.clone();reset();let mut bad=changes.clone();bad.push(last);assert_eq!(vm.rebind(&bad,&[1]),Err(EINVAL));unchanged(&vm,&before);
 }
 for ctx in [0,64,u16::MAX]{let mut vm=base.clone();reset();assert_eq!(vm.rebind(&changes,&[ctx]),Err(EINVAL));unchanged(&vm,&before);}
 for contexts in [&[][..], &[2,2][..], &[1,0][..]] {let mut vm=base.clone();reset();assert_eq!(vm.rebind(&changes,contexts),Err(EINVAL));unchanged(&vm,&before);}
 let mut vm=base.clone();reset();vm.rebind(&changes,&[2,3]).unwrap();let log=trace();let flushes:Vec<_>=log.iter().enumerate().filter(|(_,v)|v.0==0).map(|(i,v)|(i,v.1)).collect();assert_eq!(flushes,vec![(2,2),(3,3),(log.len()-2,2),(log.len()-1,3)]);for &(va,_,new)in &changes{assert_eq!(vm.pte(va).unwrap(),new);}
 let mut vm=base.clone();reset();vm.rebind(&[(initial[0].0,initial[0].1|FLAGS,initial[0].1|FLAGS)],&[1]).unwrap();assert_eq!(trace(),vec![(0,1,0),(0,1,0)]);
 println!("PASS: actual UserVm::rebind/pte; {} allocation refusals with unchanged live tables and successful retry; {} successful replacement/removal/addition plans; cancelled-plan nonpublication and unpublished-plan no-TLBI, private growth preservation, break-before-make, child-first links, 13 invalid plans, both compute ASIDs flushed before make/release, unchanged-leaf preservation",refusals,successes);
}
}
fn main(){candidate::tests();}
'''.replace('METHODS', '\n'.join(method(n) for n in ('pte','rebind','prepare_rebind','grow'))).replace('PLAN', source[source.index('pub(crate) struct RebindPlan'):source.index('pub(crate) struct TableWord')])
# Compile the actual private-window preparation against the same actual
# UserVm planner. Only the module path changes for this bounded host fixture.
context_source=(Path(__file__).resolve().parents[1]/'g17p_render_context_session.rs').read_text()
private=context_source[context_source.index('struct PrivateRebind {'):context_source.index('impl PoolRebindSeed {')]
private=private.replace('super::super::g17p_user_vm::UserVm','UserVm')
rust=rust.replace('impl UserVm {',private+'\nimpl UserVm {',1)
# UserVm::prepare is an unchanged production method that preflights sparse
# target guards as well as leaves. The ordinary rebind fixture did not need it.
rust=rust.replace('fn snapshot(&self)',method('prepare(&mut self')+'\n'+method('leaf(')+'\nfn snapshot(&self)')
extra=r'''
 let mut relocation_cases=0;
 for delta in [0i64,0x4000,-0x4000,0x100000000,-0x100000000] {
  let old=0x7000000000u64;let new=(old as i64+delta)as u64;let span=8*PAGE;
  let mut live=UserVm::new();
  let source=[(old,0x18000000000),(old+PAGE,0x18000004000),(old+4*PAGE,0x18000010000),(old+5*PAGE,0x18000014000)];
  live.grow(&source).unwrap();live.grow(&[(0x1000000000,0x19000000000)]).unwrap();
  let before=live.snapshot();let root=live.root();
  let plan=PrivateRebind {old:[old,old+span],new:[new,new+span],blocks:KVec(vec![(0,source[0].1,2*PAGE),(4*PAGE,source[2].1,2*PAGE)])};
  let mut shadow=live.clone();let pages=plan.remove(&mut shadow,1).unwrap();
  assert_eq!(live.snapshot(),before);assert_eq!(live.root(),root);
  plan.install(&mut shadow,1,&pages).unwrap();
  for &(va,pa)in &source {assert_eq!(shadow.pte(new+(va-old)).unwrap(),pa|FLAGS);}
  for offset in [2,3,6,7] {assert_eq!(shadow.pte(new+offset*PAGE).unwrap(),0);}
  assert_eq!(shadow.pte(0x1000000000).unwrap(),0x19000000000|FLAGS);
  assert_eq!(live.snapshot(),before);relocation_cases+=1;
 }
 let old=0x7000000000;let new=0x7100000000;let pa=0x18000000000;
 let mut live=UserVm::new();live.grow(&[(old,pa),(old+PAGE,pa+PAGE)]).unwrap();let before=live.snapshot();
 // A last-page ownership mismatch, overlapping extents and invalid spans
 // reject before a single old leaf is removed, even from the shadow copy.
 for blocks in [vec![(0,pa,3*PAGE)],vec![(0,pa,2*PAGE),(PAGE,pa+PAGE,PAGE)],vec![(0,pa,0)],vec![(1,pa,PAGE)]] {
  let mut shadow=live.clone();reset();let p=PrivateRebind {old:[old,old+4*PAGE],new:[new,new+4*PAGE],blocks:KVec(blocks)};
  assert!(p.remove(&mut shadow,1).is_err());assert_eq!(shadow.snapshot(),before);assert!(trace().is_empty());relocation_cases+=1;
 }
 for (from,to)in [([old,old],[new,new+4*PAGE]),([old,old+4*PAGE],[new,new+PAGE]),([old,old+4*PAGE],[1<<42,(1<<42)+4*PAGE])] {
  let mut shadow=live.clone();reset();let p=PrivateRebind {old:from,new:to,blocks:KVec(vec![(0,pa,2*PAGE)])};
  assert!(p.remove(&mut shadow,1).is_err());assert_eq!(shadow.snapshot(),before);assert!(trace().is_empty());relocation_cases+=1;
 }
 // A foreign guard, caller mapping or a compact page within the target
 // span rejects the whole install. Source tree ownership remains untouched.
 for offset in [0,2,3] {
  let mut shadow=live.clone();shadow.grow(&[(new+offset*PAGE,pa+8*PAGE)]).unwrap();
  let p=PrivateRebind {old:[old,old+4*PAGE],new:[new,new+4*PAGE],blocks:KVec(vec![(0,pa,2*PAGE)])};
  let pages=p.remove(&mut shadow,1).unwrap();let cleared=shadow.snapshot();reset();
  assert_eq!(p.install(&mut shadow,1,&pages),Err(EBUSY));assert_eq!(shadow.snapshot(),cleared);assert!(trace().is_empty());
  assert_eq!(live.snapshot(),before);relocation_cases+=1;
 }
 assert_eq!(relocation_cases,15);
 println!("PASS: 15 actual private relocation cases: disjoint/overlapping/same-window moves, full guards, wrong ownership, duplicate extents, bounds, foreign target and immutable live root");
'''
rust=rust.replace(' println!("PASS: actual UserVm::rebind/pte;',extra+'\n println!("PASS: actual UserVm::rebind/pte;',1)
with tempfile.TemporaryDirectory() as tmp:
    root = Path(tmp)
    (root / 'tables.rs').write_text(rust)
    subprocess.run([args.rustc, '--edition=2021', '-Dwarnings', str(root / 'tables.rs'), '-o', str(root / 'tables')], check=True)
    output=subprocess.check_output([str(root / 'tables')],text=True);print(output,end='')
    if args.evidence:
        args.evidence.mkdir(parents=True,exist_ok=False)
        (args.evidence/'harness.rs').write_text(rust);(args.evidence/'output.txt').write_text(output)
        (args.evidence/'summary.json').write_text(json.dumps(dict(actual_UserVm_rebind_planner=True,actual_private_relocation_preparation=True,private_relocation_cases=15,live_root_unchanged_during_preparation=True,source_sha256={n:hashlib.sha256((Path(__file__).resolve().parents[1]/n).read_bytes()).hexdigest()for n in ['g17p_user_vm.rs','g17p_render_context_session.rs']},native_GPU_execution=False),indent=2)+'\n')
