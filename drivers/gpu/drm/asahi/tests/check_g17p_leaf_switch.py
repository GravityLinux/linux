#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exercise actual shared UserVm, relocation, and leaf-switch code off-device.

Only page allocation/mapping, cache instructions, and TLBIs use host doubles.
Check ownership, immutable snapshots, mixed/private ranges, stale plans,
copy fallback, allocation failures, range reads, and sorting large records.
"""
import argparse
from pathlib import Path
import subprocess,tempfile
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--rustc',default='rustc');p.add_argument('--evidence',type=Path);a=p.parse_args()
root=Path(__file__).resolve().parents[1]
s=(root/'g17p_user_vm.rs').read_text()
# The data algorithms stay unchanged. Only machine instructions are erased;
# record actual invalidation boundaries where the driver calls them.
while 'core::arch::asm!(' in s:
 start=s.index('core::arch::asm!(');i=start+len('core::arch::asm!(');depth=1
 while depth:
  depth+=(s[i]=='(')-(s[i]==')');i+=1
 s=s[:start]+'()'+s[i:]
s=s.replace('    pub(crate) fn invalidate(context: u16) {','    pub(crate) fn invalidate(context: u16) { crate::TLB.with(|t|t.borrow_mut().push(context));')
shim=r'''#![allow(dead_code,unused_variables,unused_unsafe,non_upper_case_globals,private_interfaces)]
extern crate self as kernel;
use std::cell::{Cell,RefCell,UnsafeCell};
use std::sync::atomic::{AtomicU64,Ordering};
thread_local!{static BUDGET:Cell<Option<usize>>=const{Cell::new(None)};static TLB:RefCell<Vec<u16>>=const{RefCell::new(Vec::new())};}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]pub struct Error(i32);
pub type Result<T=()>=std::result::Result<T,Error>;
pub const EINVAL:Error=Error(22);pub const EIO:Error=Error(5);pub const EBUSY:Error=Error(16);pub const EOVERFLOW:Error=Error(75);pub const ENOMEM:Error=Error(12);pub const GFP_KERNEL:u32=0;
impl From<std::num::TryFromIntError>for Error{fn from(_:std::num::TryFromIntError)->Self{EINVAL}}
fn allocation()->Result{BUDGET.with(|b|match b.get(){Some(0)=>Err(ENOMEM),Some(n)=>{b.set(Some(n-1));Ok(())},None=>Ok(())})}
pub mod alloc{pub mod flags{pub const __GFP_ZERO:u32=1;}}
pub mod types{pub type Owned<T>=Box<T>;}
pub mod prelude{pub use crate::{Result,Error,EINVAL,EIO,EBUSY,EOVERFLOW,ENOMEM,GFP_KERNEL,KVec,KVVec};}
pub struct KVec<T>(Vec<T>);pub type KVVec<T>=KVec<T>;
impl<T>KVec<T>{pub fn new()->Self{Self(Vec::new())}pub fn with_capacity(n:usize,_:u32)->Result<Self>{allocation()?;Ok(Self(Vec::with_capacity(n)))}pub fn reserve(&mut self,n:usize,_:u32)->Result{allocation()?;self.0.reserve(n);Ok(())}pub fn push(&mut self,v:T,_:u32)->Result{allocation()?;self.0.push(v);Ok(())}pub fn pop(&mut self)->Option<T>{self.0.pop()}pub fn resize(&mut self,n:usize,v:T,_:u32)->Result where T:Clone{allocation()?;self.0.resize(n,v);Ok(())}pub fn extend_from_slice(&mut self,v:&[T],_:u32)->Result where T:Clone{allocation()?;self.0.extend_from_slice(v);Ok(())}pub fn drain<R:std::ops::RangeBounds<usize>>(&mut self,r:R)->std::vec::Drain<'_,T>{self.0.drain(r)}}
impl<T>std::ops::Deref for KVec<T>{type Target=[T];fn deref(&self)->&[T]{&self.0}}
impl<T>std::ops::DerefMut for KVec<T>{fn deref_mut(&mut self)->&mut[T]{&mut self.0}}
impl<'a,T>IntoIterator for &'a KVec<T>{type Item=&'a T;type IntoIter=std::slice::Iter<'a,T>;fn into_iter(self)->Self::IntoIter{self.0.iter()}}
impl<'a,T>IntoIterator for &'a mut KVec<T>{type Item=&'a mut T;type IntoIter=std::slice::IterMut<'a,T>;fn into_iter(self)->Self::IntoIter{self.0.iter_mut()}}
pub mod sync{pub struct Arc<T>(std::sync::Arc<T>);impl<T>Arc<T>{pub fn new(v:T,_:u32)->crate::Result<Self>{crate::allocation()?;Ok(Self(std::sync::Arc::new(v)))}}impl<T>Clone for Arc<T>{fn clone(&self)->Self{Self(self.0.clone())}}impl<T>std::ops::Deref for Arc<T>{type Target=T;fn deref(&self)->&T{&self.0}}}
pub mod page{
 use super::*;pub const PAGE_SIZE:usize=0x4000;static NEXT:AtomicU64=AtomicU64::new(0x4000);
 pub struct Page{pa:u64,body:Box<[UnsafeCell<u64>]>}
 impl Page{pub fn alloc_page(_:u32)->Result<Box<Self>>{allocation()?;Ok(Box::new(Self{pa:NEXT.fetch_add(0x4000,Ordering::Relaxed),body:(0..2048).map(|_|UnsafeCell::new(0)).collect::<Vec<_>>().into_boxed_slice()}))}pub fn phys(&self)->u64{self.pa}pub fn with_page_mapped<T>(&self,f:impl FnOnce(*mut u8)->T)->T{f(self.body.as_ptr().cast_mut().cast())}}
}
mod g17p_memory{pub fn sync(){}}
mod user;
fn main(){user::tests::run();}
'''
tests=r'''
pub mod tests {
use super::*;
const FLAGS:u64=0x00c0000000000c8b;
fn span(vm:&mut UserVm,va:u64,count:usize,pa:u64){vm.prepare(va,count as u64*PAGE).unwrap();for i in 0..count{vm.map_owned_page(va+i as u64*PAGE,pa+i as u64*PAGE,FLAGS).unwrap();}}
fn bindings(rows:&[(u64,u64)]) -> Vec<(u64,u64,u64,u32)> {rows.iter().map(|&(va,n)|(va,n*PAGE,0,0)).collect()}
fn snapshot(vm:&UserVm)->RebindSnapshot{vm.capture_rebind(TableStorage::new(vm.table_count()).unwrap()).unwrap().unwrap()}
fn words(vm:&UserVm)->Vec<(u64,Vec<u64>)>{vm.tables.iter().map(|p|(p.phys(),vm.read_table(p.phys()).unwrap().to_vec())).collect()}
fn switch(vm:&mut UserVm,next:&UserVm,old:&[(u64,u64,u64,u32)]) {let mut plan=snapshot(vm).prepare_leaf_switch(next,old,&[],&[],&[1]).unwrap();plan.commit(vm).unwrap();}
pub fn run(){
 // A 64 MiB caller image with a partial leaf, a private mixed leaf, and a
 // new top-level range; the source admission trees contain caller leaves only.
 let base=0x2000000000;let mixed=0x1000000000;let private=mixed+PAGE*4;
 let mut a=UserVm::new().unwrap();span(&mut a,base,4200,0x100000000);span(&mut a,mixed+PAGE,2,0x200000000);
 let mut b=UserVm::new().unwrap();span(&mut b,base,4300,0x300000000);span(&mut b,mixed+PAGE,2,0x400000000);span(&mut b,0x5000000000,1,0x500000000);
 let ba=bindings(&[(base,4200),(mixed+PAGE,2)]);let bb=bindings(&[(base,4300),(mixed+PAGE,2),(0x5000000000,1)]);
 crate::TLB.with(|t|t.borrow_mut().clear());
 let view=a.clone_admission_view().unwrap();
 assert!(!view.borrowed.is_empty());assert_eq!(view.pte(base+4199*PAGE).unwrap(),(0x100000000+4199*PAGE)|FLAGS);
 assert!(crate::TLB.with(|t|t.borrow().is_empty()),"unpublished admission view invalidated a live ASID");
 drop(view);
 let mut installed=a.clone_admission_tree().unwrap();span(&mut installed,private,1,0x600000000);
 let root=installed.root();let initial_a=words(&a);let initial_b=words(&b);let private_pte=installed.pte(private).unwrap();
 let oldlease=installed.lease().unwrap();
 let mut first=snapshot(&installed).prepare_leaf_switch(&b,&ba,&[],&[],&[1]).unwrap();
 assert!(first.borrowed.len()>=3);assert!(first.stores.len()<20,"copied caller PTEs instead of splicing ranges");
 assert!(first.matches(&installed));let unchanged=words(&installed);drop(first);
 assert_eq!(words(&installed),unchanged,"cancelled splice modified live tables");
 crate::TLB.with(|t|t.borrow_mut().clear());switch(&mut installed,&b,&ba);
 assert_eq!(installed.root(),root);assert_eq!(installed.pte(base).unwrap(),0x300000000|FLAGS);
 assert_eq!(installed.pte(base+4299*PAGE).unwrap(),(0x300000000+4299*PAGE)|FLAGS);
 assert_eq!(installed.pte(private).unwrap(),private_pte);assert_eq!(words(&a),initial_a);assert_eq!(words(&b),initial_b);
 assert_eq!(crate::TLB.with(|t|t.borrow().clone()),vec![1,1]);
 assert_eq!(installed.private_pte(base+5000*PAGE).unwrap(),1,"private allocator entered a borrowed hole");
 let borrowed=installed.borrowed.to_vec();assert!(!borrowed.is_empty());
 // Ordinary PTE mutators fail closed on a shared admission table.
 assert_eq!(installed.rebind(&[(base,0x300000000|FLAGS,0)],&[1]),Err(EBUSY));
 assert_eq!(installed.grow(&[(base+5000*PAGE,0x800000000)]),Err(EBUSY));assert_eq!(words(&b),initial_b);
 // Sharing repeatedly must not retain every old caller's leaf tables.
 let ceiling=installed.table_count()+8;
 for i in 0..60 {if i%2==0{switch(&mut installed,&a,&bb)}else{switch(&mut installed,&b,&ba)}assert!(installed.table_count()<=ceiling);assert_eq!(installed.pte(private).unwrap(),private_pte);}
 assert_eq!(words(&a),initial_a);assert_eq!(words(&b),initial_b);
 // Moving-private/copy fallback owns copied leaves, not the caller snapshot.
 installed.make_owned(&[1]).unwrap();assert!(installed.borrowed.is_empty());
 installed.rebind(&[(base,0x300000000|FLAGS,0)],&[1]).unwrap();assert_eq!(installed.pte(base).unwrap(),0);assert_eq!(words(&b),initial_b);
 let mut refreshed=oldlease;refreshed.refresh(&installed).unwrap();assert_eq!(refreshed.root(),root);
 // Repeated private-copy fallback and re-sharing must also recycle tables.
 let fallback_ceiling=installed.table_count()+8;
 for i in 0..40 {installed.make_owned(&[1]).unwrap();
  if i%2==0{switch(&mut installed,&a,&bb)}else{switch(&mut installed,&b,&ba)}
  assert!(installed.table_count()<=fallback_ceiling,"copy fallback leaked old table ownership");
  assert_eq!(installed.pte(private).unwrap(),private_pte);
 }
 installed.make_owned(&[1]).unwrap();
 // A late private collision rejects without modifying either tree.
 let mut bad=UserVm::new().unwrap();span(&mut bad,private,1,0x900000000);
 let before=words(&installed);assert!(snapshot(&installed).prepare_leaf_switch(&bad,&bb,&[],&[],&[1]).is_err());assert_eq!(words(&installed),before);
 // A plan captured before another root generation cannot be committed.
 let mut stale=snapshot(&installed).prepare_leaf_switch(&a,&bb,&[],&[],&[1]).unwrap();
 installed.rebind(&[(private,private_pte,private_pte)],&[1]).unwrap();assert_eq!(stale.commit(&mut installed),Err(EBUSY));
 // Allocation failure at each preparation boundary leaves live PTEs intact.
 let mut refusals=0;let mut successes=0;
 for budget in 0..200 {let snap=snapshot(&installed);let before=words(&installed);crate::BUDGET.with(|b|b.set(Some(budget)));let plan=snap.prepare_leaf_switch(&b,&bb,&[],&[],&[1]);crate::BUDGET.with(|b|b.set(None));match plan {Err(e)if e==ENOMEM=>refusals+=1,Ok(_)=>successes+=1,Err(e)=>panic!("unexpected {:?}",e)}assert_eq!(words(&installed),before);}
 assert!(refusals>0&&successes>0);
 // Imported tables must outlive both the incoming snapshot and the plan.
 let ephemeral_va=0x6000000000;
 let mut ephemeral=UserVm::new().unwrap();span(&mut ephemeral,ephemeral_va,1,0xc00000000);
 let mut retained_plan=snapshot(&installed).prepare_leaf_switch(&ephemeral,&bb,&[],&[],&[1]).unwrap();
 drop(ephemeral);retained_plan.commit(&mut installed).unwrap();drop(retained_plan);
 assert_eq!(installed.pte(ephemeral_va).unwrap(),0xc00000000|FLAGS);
 assert_eq!(installed.pte(private).unwrap(),private_pte);
 // Batched reads must match independent per-page queries, including holes,
 // 32 MiB crossings, a 64 GiB crossing, and invalid/overflow boundaries.
 let mut range=UserVm::new().unwrap();span(&mut range,0x2000000-PAGE,3,0xa00000000);span(&mut range,0x1000000000-PAGE,2,0xb00000000);
 for (va,n)in [(0,5),(0x2000000-2*PAGE,6),(0x1000000000-2*PAGE,5)]{let got=range.ptes(va,n*PAGE).unwrap();for(i,&p)in got.iter().enumerate(){assert_eq!(p,range.pte(va+i as u64*PAGE).unwrap());}}
 assert!(range.ptes(1,PAGE).is_err());assert!(range.ptes(0,PAGE+1).is_err());assert!(range.ptes((1<<42)-PAGE,2*PAGE).is_err());assert!(range.ptes(u64::MAX-PAGE+1,PAGE).is_err());
 // Address-checked hints remain safe when table ownership order changes.
 for _ in 0..4{for va in [0x2000000,0x1000000000-PAGE]{let p=range.pte(va).unwrap();let last=range.tables.len()-1;range.tables.swap(1,last);assert_eq!(range.pte(va).unwrap(),p);}}
 let mut duplicates:Vec<_>=(0..8192).map(|i|((i*17)%29,i)).collect();
 let calls=core::cell::Cell::new(0);sort_by_key(&mut duplicates,|r|{calls.set(calls.get()+1);r.0});
 assert_eq!(calls.get(),8192);assert!(duplicates.windows(2).all(|r|r[0].0<=r[1].0));
 for row in duplicates {assert_eq!(row.0,(row.1*17)%29);}
 for fail in [false,true]{let mut rows:Vec<_>=(0..20000).map(|i|((i*733)%20000,[i as u64;8])).collect();if fail{crate::BUDGET.with(|b|b.set(Some(0)));}sort_by_key(&mut rows,|r|r.0);crate::BUDGET.with(|b|b.set(None));assert!(rows.windows(2).all(|r|r[0].0<=r[1].0));for row in rows{assert_eq!(row.0,(row.1[0]as usize*733)%20000);}}
 println!("PASS actual shared UserVm/leaf switch: immutable input tables, held backing, mixed/private preservation, new ranges, 60 switches with bounded table ownership, cancelled/stale/colliding plans, {} allocation refusals/{} preparations, owned-copy fallback, range reads/cache hints, large-record index sort and allocation-failure heapsort",refusals,successes);
}
}
'''
with tempfile.TemporaryDirectory(prefix='asahi-leaf-switch-')as temp:
 d=Path(temp);(d/'main.rs').write_text(shim);(d/'user.rs').write_text(s+tests);(d/'g17p_leaf_switch.rs').write_text((root/'g17p_leaf_switch.rs').read_text())
 r=subprocess.run([a.rustc,'--edition=2021','-O',str(d/'main.rs'),'-o',str(d/'test')],capture_output=True,text=True)
 if r.returncode:raise RuntimeError(r.stderr)
 r=subprocess.run([str(d/'test')],capture_output=True,text=True)
 print(r.stdout,end='')
 if r.returncode:raise RuntimeError(r.stderr)
 if a.evidence:
  a.evidence.mkdir(parents=True,exist_ok=True)
  for name in ['main.rs','user.rs','g17p_leaf_switch.rs']:(a.evidence/name).write_bytes((d/name).read_bytes())
  (a.evidence/'output.txt').write_text(r.stdout)
