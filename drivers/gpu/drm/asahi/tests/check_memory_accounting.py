#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Run actual heap ownership/accounting methods with a fallible host allocator.

No page mapping, cache maintenance or GPU execution is simulated as proof.
Exercise rounding, prepared reuse, detached transfer and failure boundaries.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence', type=Path, required=True)
a = p.parse_args()
source = Path(__file__).resolve().parent.parent / 'g17p_memory.rs'
text = source.read_text()


def item(marker):
    start = text.index(marker)
    at = text.index('{', start)
    depth = 1
    end = at + 1
    while depth:
        depth += (text[end] == '{') - (text[end] == '}')
        end += 1
    return text[start:end]


code = r'''
#![allow(dead_code,non_camel_case_types,non_upper_case_globals)]
use std::cell::Cell;
type Result<T=()> = std::result::Result<T,i32>;
const EINVAL:i32=22;const EOVERFLOW:i32=75;const ENOMEM:i32=12;
const EIO:i32=5;const EAGAIN:i32=11;const GFP_KERNEL:u32=0;const PAGE:usize=0x4000;
thread_local! {
 static FAIL:Cell<usize>=const{Cell::new(usize::MAX)};
 static LIVE:Cell<usize>=const{Cell::new(0)};
 static NEXT:Cell<u64>=const{Cell::new(0x10000000000)};
}
fn fallible()->Result {
 FAIL.with(|f| {let n=f.get();if n==0 {Err(ENOMEM)}else{f.set(n.saturating_sub(1));Ok(())}})
}
struct KVec<T>(Vec<T>);
impl<T>KVec<T> {
 fn new()->Self{Self(Vec::new())}
 fn reserve(&mut self,n:usize,_:u32)->Result{fallible()?;self.0.reserve(n);Ok(())}
 fn push(&mut self,v:T,_:u32)->Result{if self.0.len()==self.0.capacity(){fallible()?;}self.0.push(v);Ok(())}
 fn drain(&mut self,r:std::ops::RangeFull)->std::vec::Drain<'_,T>{self.0.drain(r)}
 fn remove(&mut self,i:usize)->Result<T>{if i>=self.0.len(){Err(EINVAL)}else{Ok(self.0.remove(i))}}
}
impl<T>std::ops::Deref for KVec<T>{type Target=[T];fn deref(&self)->&[T]{&self.0}}
struct Arc<T>(std::sync::Arc<T>);
impl<T>Arc<T>{fn new(v:T,_:u32)->Result<Self>{fallible()?;Ok(Self(std::sync::Arc::new(v)))}}
impl<T>std::ops::Deref for Arc<T>{type Target=T;fn deref(&self)->&T{&self.0}}
struct RBTree<K,V>(Vec<(K,V)>);
struct RBTreeNodeReservation;
struct Node<K,V>(K,V);
impl RBTreeNodeReservation {
 fn new(_:u32)->Result<Self>{fallible()?;Ok(Self)}
 fn into_node<K,V>(self,k:K,v:V)->Node<K,V>{Node(k,v)}
}
impl<K,V>Node<K,V>{fn into_reservation(self)->RBTreeNodeReservation{RBTreeNodeReservation}}
struct Cursor<'a,K,V>(&'a mut RBTree<K,V>);
impl<K,V>RBTree<K,V>{
 fn new()->Self{Self(Vec::new())}fn insert(&mut self,n:Node<K,V>){self.0.push((n.0,n.1));}
 fn cursor_front_mut(&mut self)->Option<Cursor<'_,K,V>>{if self.0.is_empty(){None}else{Some(Cursor(self))}}
}
impl<K:Copy,V:Copy>Cursor<'_,K,V>{
 fn current(&self)->(&K,&V){(&self.0.0[0].0,&self.0.0[0].1)}
 fn remove_current(self)->((),Node<K,V>){let(k,v)=self.0.0.remove(0);((),Node(k,v))}
}
mod page{pub const PAGE_SIZE:usize=0x4000;}
mod bindings{
 pub struct page{pub base:u64}
 pub const GFP_KERNEL:u32=0;pub const __GFP_NOWARN:u32=0;pub const ___GFP_NORETRY_BIT:u32=0;
 pub unsafe fn alloc_pages(_:u32,_:u32)->*mut page {
  if super::fallible().is_err(){return std::ptr::null_mut();}
  let base=super::NEXT.with(|n|{let p=n.get();n.set(p+0x400000);p});
  super::LIVE.with(|n|n.set(n.get()+1));Box::into_raw(Box::new(page{base}))
 }
 pub unsafe fn page_to_phys(p:*mut page)->u64{unsafe{(*p).base}}
 pub unsafe fn __free_pages(p:*mut page,_:u32){super::LIVE.with(|n|n.set(n.get()-1));drop(unsafe{Box::from_raw(p)});}
}
mod g17p_growth{pub const INCREMENT:usize=1;pub const BLOCK:u64=0x200000;}
mod heap {
 use super::*;
 struct Mapping;
'''
code += item('struct Allocation {') + '\n' + item('impl Drop for Allocation {') + '\n'
code += item('pub(crate) struct Memory {') + '\nimpl Memory {\n'
for marker in ['pub(crate) fn allocation_stats(', 'pub(crate) fn detached(',
               'pub(crate) fn absorb(', 'pub(crate) fn prepared_count(',
               'pub(crate) fn prepare_blocks(', 'pub(crate) fn allocate(',
               'fn allocate_new(']:
    code += item(marker) + '\n'
code += r'''
 fn zero(&self,_:u64,_:usize)->Result{fallible()}
 fn clean(&self,_:u64,_:usize)->Result{fallible()}
 fn check(&self) {
  assert_eq!(self.allocation_stats(),(self.allocations.len(),self.allocations.iter().map(|a|a.size as u64).sum()));
  assert_eq!(self.allocation_ends.0.len(),self.allocations.len());
  for &(end,i) in &self.allocation_ends.0 {assert_eq!(end,self.allocations[i].base+self.allocations[i].size as u64);}
 }
}
pub fn run()->Result {
 let mut m=Memory::detached();assert_eq!(m.allocation_stats(),(0,0));
 m.allocate(0xc000)?;assert_eq!(m.allocation_stats(),(1,0x10000));m.check();
 m.prepare_blocks(0x4000,2)?;let before=m.allocation_stats();
 assert_eq!(m.prepared_count(0x4000),2);m.allocate(0x4000)?;assert_eq!(m.allocation_stats(),before);m.check();
 let mut child=Memory::detached();child.prepare_blocks(0xc000,1)?;child.allocate(0x200000)?;
 m.absorb(child)?;m.check();assert_eq!(m.allocation_stats(),(5,0x228000));
 let before=m.allocation_stats();m.allocate(0xc000)?;assert_eq!(m.allocation_stats(),before);m.check();
 // Fail before page allocation, at Arc allocation, and after ownership is
 // committed (zeroing). Any retained allocation must still be accounted.
 for fail in 0..7 {
  let mut t=Memory::detached();FAIL.with(|n|n.set(fail));let result=t.allocate(0xc000);
  FAIL.with(|n|n.set(usize::MAX));t.check();
  if result.is_err(){assert!(matches!(t.allocation_stats(),(0,0)|(1,0x10000)));}
 }
 for fail in 0..4 {
  let mut into=Memory::detached();into.allocate(0x4000)?;
  let mut from=Memory::detached();from.prepare_blocks(0xc000,1)?;
  FAIL.with(|n|n.set(fail));let result=into.absorb(from);FAIL.with(|n|n.set(usize::MAX));into.check();
  assert_eq!(into.allocation_stats(),if result.is_ok(){(2,0x14000)}else{(1,0x4000)});
 }
 let mut overflow=Memory::detached();overflow.allocated_bytes=u64::MAX;
 assert_eq!(overflow.allocate(0x4000),Err(EOVERFLOW));
 let child=Memory::detached();overflow.absorb(child)?;assert_eq!(overflow.allocated_bytes,u64::MAX);
 drop(m);assert_eq!(LIVE.with(Cell::get),0);
 println!("PASS rounded backing, prepared reuse, detached transfer, allocation/transfer failures and overflow; constant-time receipts");Ok(())
}
}
fn main(){heap::run().unwrap();}
'''
a.evidence.mkdir(parents=True, exist_ok=False)
(a.evidence / 'test.rs').write_text(code)
subprocess.run(['rustc', '--edition=2021', '-O', str(a.evidence / 'test.rs'),
                '-o', str(a.evidence / 'test')], check=True)
result = subprocess.run([str(a.evidence / 'test')], check=True, capture_output=True, text=True)
receipt = dict(result='PASS', production_source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
               stdout=result.stdout, hardware_execution=False)
(a.evidence / 'result.json').write_text(json.dumps(receipt, indent=2) + '\n')
print(result.stdout, end='')
