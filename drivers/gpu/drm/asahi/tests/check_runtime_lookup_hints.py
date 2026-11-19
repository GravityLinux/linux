#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare actual ownership/walk hints with the original checked lookup paths."""
import argparse,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args();r=Path(__file__).resolve().parents[1]
def item(s,m):
 i=s.index(m);j=s.index('{',i)+1;depth=1
 while depth:depth+=(s[j]=='{')-(s[j]=='}');j+=1
 return s[i:j]
m=(r/'g17p_memory.rs').read_text();v=(r/'g17p_vm.rs').read_text();oldm=subprocess.check_output(['git','show','6bb0f5e1bc25:drivers/gpu/drm/asahi/g17p_memory.rs'],text=True);oldv=subprocess.check_output(['git','show','6bb0f5e1bc25:drivers/gpu/drm/asahi/g17p_vm.rs'],text=True)
methods=item(m,'    fn owns_allocation(')+item(oldm,'    fn owns_allocation(').replace('fn owns_allocation(','fn old_owns_allocation(')
walk=item(v,'    fn owns_table(')+item(v,'    fn lookup(')+item(v,'    fn indices(')+item(oldv,'    fn lookup(').replace('fn lookup(','fn old_lookup(')
code=r'''
use std::{collections::BTreeMap,cell::RefCell,sync::{Arc,atomic::{AtomicUsize,Ordering}}};
type Result<T> = std::result::Result<T,i32>;const EINVAL:i32=22;const PAGE:u64=0x4000;const ADDRESS:u64=0x0000_ffff_ffff_c000;
struct Allocation{base:u64,size:usize}
struct Tree{rows:BTreeMap<u64,usize>,searches:AtomicUsize}
struct Cursor<'a>(&'a u64,&'a usize);impl Cursor<'_>{fn current(&self)->(&u64,&usize){(self.0,self.1)}}
impl Tree{fn cursor_lower_bound(&self,end:&u64)->Option<Cursor<'_>>{self.searches.fetch_add(1,Ordering::Relaxed);self.rows.range(*end..).next().map(|(k,v)|Cursor(k,v))}}
struct Memory{allocations:Vec<Arc<Allocation>>,allocation_ends:Tree,allocation_hints:[AtomicUsize;16],words:RefCell<BTreeMap<u64,u64>>}
impl Memory{METHODS
 fn read64(&self,pa:u64)->Result<u64>{Ok(*self.words.borrow().get(&pa).unwrap_or(&0))}
}
struct Vm{roots:[u64;3],tables:Vec<u64>,table_hints:[AtomicUsize;3]}
impl Vm{WALK}
fn main(){
 let mut mem=Memory{allocations:vec![],allocation_ends:Tree{rows:BTreeMap::new(),searches:AtomicUsize::new(0)},allocation_hints:[const{AtomicUsize::new(usize::MAX)};16],words:RefCell::new(BTreeMap::new())};
 for i in 0..256{let base=i*0x10000+0x4000;mem.allocation_ends.rows.insert(base+0x8000,mem.allocations.len());mem.allocations.push(Arc::new(Allocation{base,size:0x8000}));}
 let mut seed=77u64;let mut spans=0;
 for _ in 0..200000{seed^=seed<<13;seed^=seed>>7;seed^=seed<<17;let addr=seed%0x1000000;let end=addr+(seed>>32)%0x10001;assert_eq!(mem.owns_allocation(addr,end),mem.old_owns_allocation(addr,end));spans+=1;}
 for hint in &mem.allocation_hints{hint.store(usize::MAX,Ordering::Relaxed);}
 let at=mem.allocations[117].base;assert!(mem.owns_allocation(at,at+8));let searches=mem.allocation_ends.searches.load(Ordering::Relaxed);
 for _ in 0..10000{assert!(mem.owns_allocation(at+4,at+8));}assert_eq!(mem.allocation_ends.searches.load(Ordering::Relaxed),searches);
 // Vector moves/reindexing cannot turn a stale hint into an ownership proof.
 mem.allocations.reverse();mem.allocation_ends.rows.clear();for (i,a)in mem.allocations.iter().enumerate(){mem.allocation_ends.rows.insert(a.base+a.size as u64,i);}
 for a in &mem.allocations{for off in [0,1,0x4000,0x7fff]{assert_eq!(mem.owns_allocation(a.base+off,a.base+off+8),mem.old_owns_allocation(a.base+off,a.base+off+8));}}
 let mut vm=Vm{roots:[0x4000,0x8000,0xc000],tables:vec![0x4000,0x8000,0xc000,0x10000,0x14000],table_hints:[const{AtomicUsize::new(usize::MAX)};3]};let mut walks=0;
 for group in 0..4{for va in [0,0x4000,1<<25,0x000003ffffffffff,0xfffffc2000000000,0xfffffc2000004000,0xfffffc3000000000,17]{
  if let Ok(indices)=Vm::indices(group,va){let root=vm.roots[group];mem.words.borrow_mut().insert(root+indices[0]as u64*8,0x10003);mem.words.borrow_mut().insert(0x10000+indices[1]as u64*8,0x14003);mem.words.borrow_mut().insert(0x14000+indices[2]as u64*8,0x20003);}
  for word in [0,1,2,3,0x24003]{if let Ok(indices)=Vm::indices(group,va){mem.words.borrow_mut().insert(0x14000+indices[2]as u64*8,word);}
   assert_eq!(vm.lookup(&mem,group,va),vm.old_lookup(&mem,group,va));walks+=1;
  }
 }}
 // Cached table indices still check membership and still read changed PTEs.
 let va=0;mem.words.borrow_mut().insert(0x4000,0x10003);mem.words.borrow_mut().insert(0x10000,0x14003);mem.words.borrow_mut().insert(0x14000,0x20003);
 assert_eq!(vm.lookup(&mem,0,va),Ok(Some(0x20003)));mem.words.borrow_mut().insert(0x14000,0x30003);assert_eq!(vm.lookup(&mem,0,va),Ok(Some(0x30003)));
 vm.tables.reverse();assert_eq!(vm.lookup(&mem,0,va),Ok(Some(0x30003)));vm.tables.retain(|p|*p!=0x14000);assert_eq!(vm.lookup(&mem,0,va),Err(EINVAL));
 println!("PASS {spans} allocation spans and {walks} actual old/new walks: exact bounds, stale/out-of-range hints, vector reorder, invalid/missing tables and changing live PTEs; 10000 warm ownership hits avoid the tree search");}
'''.replace('METHODS',methods).replace('WALK',walk)
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);print(out,end='')
