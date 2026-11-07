#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exercise the production grouped counter reader with stale CPU cache lines."""
import argparse, hashlib, json, subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
r=Path(__file__).resolve().parents[1]
def item(s,marker):
 start=s.index(marker);end=s.index('{',start)+1;depth=1
 while depth:depth+=(s[end]=='{')-(s[end]=='}');end+=1
 return s[start:end]
m=(r/'g17p_memory.rs').read_text();v=(r/'g17p_vm.rs').read_text()
method=item(m,'    pub(crate) fn read_firmware32s<const N:usize>')
asm='''unsafe {core::arch::asm!("dc ivac, {pointer}",pointer=in(reg)pointer,
                    options(nostack,preserves_flags));}'''
assert asm in method;method=method.replace(asm,'self.invalidate_mock(pointer);')
code=r'''
#![allow(dead_code)]
use std::{cell::{Cell,RefCell,UnsafeCell},collections::BTreeMap};
type Result<T=()> = std::result::Result<T,i32>;const EINVAL:i32=22;
thread_local!{static SYNCS:Cell<usize>=const{Cell::new(0)};}
fn sync(){SYNCS.with(|v|v.set(v.get()+1));}
#[repr(align(64))]struct Bytes([u8;32768]);
struct Memory{cpu:UnsafeCell<Box<Bytes>>,firmware:Box<Bytes>,invalidations:RefCell<Vec<usize>>}
impl Memory{
 fn new()->Self{Self{cpu:UnsafeCell::new(Box::new(Bytes([0xaa;32768]))),firmware:Box::new(Bytes([0;32768])),invalidations:RefCell::new(Vec::new())}}
 fn access<T>(&self,address:u64,size:usize,f:impl FnOnce(*mut u8)->T)->Result<T>{
  let off=address.checked_sub(0x4000).ok_or(EINVAL)? as usize;
  if off.checked_add(size).is_none_or(|end|end>32768){return Err(EINVAL)}
  Ok(f(unsafe{(*self.cpu.get()).0.as_mut_ptr().add(off)}))
 }
 fn invalidate_mock(&self,p:*mut u8){
  let base=unsafe{(*self.cpu.get()).0.as_mut_ptr()};
  let off=(p as usize-base as usize)&!63;
  self.invalidations.borrow_mut().push(off);
  unsafe{base.add(off).copy_from_nonoverlapping(self.firmware.0.as_ptr().add(off),64)}
 }
 MEMORY_METHOD
}
struct Vm{pages:BTreeMap<(usize,u64),u64>}
impl Vm{
 fn physical(&self,_:&Memory,group:usize,address:u64)->Result<u64>{
  Ok(self.pages.get(&(group,address&!0x3fff)).ok_or(EINVAL)?+(address&0x3fff))
 }
 VM_METHOD
}
fn main(){
 let mut m=Memory::new();
 for (i,word)in m.firmware.0.chunks_exact_mut(4).enumerate(){word.copy_from_slice(&(i as u32*17+3).to_le_bytes());}
 let mut cases=0;
 for start in (0..32768-256).step_by(4){
  // Re-poison clean CPU copies before every pass; use repeated, same-line,
  // cross-line, and cross-page addresses in a single group.
  unsafe{(*m.cpu.get()).0.fill(0xaa)};
  m.invalidations.borrow_mut().clear();SYNCS.with(|x|x.set(0));
  let offsets=[start,start+4,start+32,start+64,start+128,start];
  let values=m.read_firmware32s(offsets.map(|x|x as u64+0x4000)).unwrap();
  assert_eq!(values,offsets.map(|x|(x/4) as u32*17+3));
  assert_eq!(*m.invalidations.borrow(),offsets.map(|x|x&!63));
  SYNCS.with(|x|assert_eq!(x.get(),2));cases+=1;
 }
 let vm=Vm{pages:[((0,0x10000),0x8000),((0,0x14000),0x4000),((2,0x10000),0x4000)].into()};
 assert_eq!(vm.read_firmware32s(&m,0,[0x13ffc,0x14000,0x10000]).unwrap(),[0x7ffc/4*17+3,3,0x4000/4*17+3]);
 assert_eq!(vm.read_firmware32s(&m,2,[0x10000]).unwrap(),[3]);
 // Failed translation is detected before the first cache operation.
 m.invalidations.borrow_mut().clear();
 assert_eq!(vm.read_firmware32s(&m,0,[0x10000,0x18000]),Err(EINVAL));assert!(m.invalidations.borrow().is_empty());
 for address in [0,0x3ffc,0x4001,0xc000,u64::MAX]{assert_eq!(m.read_firmware32s([address]),Err(EINVAL));}
 // Every call observes new firmware values, rather than reusing a cached
 // counter across polls, even at exactly the same physical address.
 for n in 0..10000u32{m.firmware.0[0..4].copy_from_slice(&n.to_le_bytes());assert_eq!(m.read_firmware32s([0x4000,0x4000]).unwrap(),[n,n]);}
 println!("PASS {} stale-cache batches; separate VM groups and discontinuous pages; invalid spans; 10000 changing firmware epochs; two barriers per batch",cases);
}
'''.replace('MEMORY_METHOD',method).replace('VM_METHOD',item(v,'    pub(crate) fn read_firmware32s<const N:usize>'))
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out)
(a.evidence/'source.json').write_text(json.dumps({n:hashlib.sha256((r/n).read_bytes()).hexdigest() for n in ['g17p_memory.rs','g17p_vm.rs']},indent=2)+'\n');print(out,end='')
