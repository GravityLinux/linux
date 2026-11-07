#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute control publication against outstanding cleanup and ring wraparound."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence', type=Path, required=True)
p.add_argument('--source', type=Path)
a = p.parse_args()
root = Path(__file__).resolve().parents[1]
source = (a.source or root / 'scheduled_bootstrap.rs').read_text()
queue = (root / 'g17p_queue.rs').read_text()


def item(text, marker):
    start = text.index(marker)
    end = text.index('{', start) + 1
    depth = 1
    while depth:
        depth += (text[end] == '{') - (text[end] == '}')
        end += 1
    return text[start:end]


code = r'''
#![allow(dead_code)]
use std::cell::RefCell;
type Result<T=()> = std::result::Result<T,i32>;
const EIO:i32=5;const EBUSY:i32=16;const ENOMEM:i32=12;
const CONTROL_STATE:u64=0x20300;const CONTROL_RING:u64=0x1e5c0;
thread_local!{static TRACE:RefCell<Vec<&'static str>>=const{RefCell::new(Vec::new())};}
#[derive(Default)]struct Memory{cursors:[u32;3],writes:Vec<(u64,Vec<u8>)>,fail_at:Option<usize>}
struct Vm;
impl Vm{fn read_firmware32s<const N:usize>(&self,m:&Memory,context:u32,addresses:[u64;N])->Result<[u32;N]>{assert_eq!(context,2);Ok(addresses.map(|at|m.cursors[((at-CONTROL_STATE)/0x10) as usize]))}
fn write(&self,m:&mut Memory,context:u32,at:u64,data:&[u8])->Result{
 assert_eq!(context,2);
 if m.fail_at==Some(m.writes.len()){return Err(ENOMEM);}
 TRACE.with(|t|t.borrow_mut().push(if at==CONTROL_STATE+0x20{"tail"}else{"data"}));
 m.writes.push((at,data.to_vec()));
 if at==CONTROL_STATE+0x20{m.cursors[2]=u32::from_le_bytes(data.try_into().unwrap());}
 Ok(())
}}
fn read32(m:&Memory,_:&Vm,at:u64)->Result<u32>{Ok(m.cursors[((at-CONTROL_STATE)/0x10) as usize])}
mod g17p_memory{pub fn sync(){super::TRACE.with(|t|t.borrow_mut().push("sync"));}}
mod g17p_queue{
 #[derive(Debug)]pub enum Error{Invalid,Full}
 type Result<T>=std::result::Result<T,Error>;
 #[derive(Clone,Copy)]pub(crate)struct Counters(pub(crate)[u8;3]);
 COUNTERS
 REACHED
}
mod production{use super::*;
 CURSORS
 PUBLISH
}
fn memory(values:[u32;3])->Memory{TRACE.with(|t|t.borrow_mut().clear());Memory{cursors:values,..Memory::default()}}
fn main(){
 let record=[0xa5;64];let mut count=0;
 for producer in 0u32..256{
  for read_lag in [0u8,1,2,7,128,254,255]{for done_lag in [0u8,1,2,7,128,254,255]{
   let values=[u32::from((producer as u8).wrapping_sub(read_lag)),u32::from((producer as u8).wrapping_sub(done_lag)),producer];
   let mut m=memory(values);let result=production::publish_control(&mut m,&Vm,&record);
   if read_lag==255||done_lag==255{
    assert_eq!(result,Err(EBUSY));assert!(m.writes.is_empty());
    TRACE.with(|t|assert!(t.borrow().is_empty()));
   }else{
    let target=(producer+1)%256;assert_eq!(result,Ok(target));
    assert_eq!(m.cursors,[values[0],values[1],target]);
    assert_eq!(m.writes,vec![(CONTROL_RING+producer as u64*64,record.to_vec()),(CONTROL_STATE+0x20,target.to_le_bytes().to_vec())]);
    TRACE.with(|t|assert_eq!(*t.borrow(),["data","sync","tail"]));
    // An outstanding cleanup receipt owns the earlier producer. Its actual
    // wrap-aware test still accepts retirement observed after our new tail.
    for (start,lag) in [(values[0] as u8,read_lag),(values[1] as u8,done_lag)]{
     if lag!=0{
      assert!(!g17p_queue::reached(start,start,producer as u8));
      assert!(g17p_queue::reached(start,target as u8,producer as u8));
     }
    }
   }
   count+=1;
  }}
 }
 // Isolate the reported failure: cleanup has advanced the producer but
 // neither consumer. A distinct record must append, preserving that slot.
 let mut m=memory([254,254,255]);assert_eq!(production::publish_control(&mut m,&Vm,&record),Ok(0));
 assert_eq!(m.writes[0].0,CONTROL_RING+255*64);
 for index in 0..3{for value in [256,1024,u32::MAX]{
  let mut values=[0;3];values[index]=value;let mut m=memory(values);
  assert_eq!(production::publish_control(&mut m,&Vm,&record),Err(EIO));assert!(m.writes.is_empty());
 }}
 for fail in 0..2{
  let mut m=memory([11,10,12]);m.fail_at=Some(fail);
  assert_eq!(production::publish_control(&mut m,&Vm,&record),Err(ENOMEM));assert_eq!(m.cursors[2],12);
  assert_eq!(m.writes.len(),fail);
 }
 println!("PASS {count} actual control publications: outstanding cleanup, independent consumers, full-ring protection, wraparound, exact data/barrier/tail ordering, retained cleanup receipts, malformed cursors and failed writes");
}
'''
code = code.replace('COUNTERS', item(queue, 'impl Counters {'))
code = code.replace('REACHED', item(queue, 'pub(crate) fn reached('))
code = code.replace('CURSORS', item(source, 'pub(crate) fn control_cursors('))
code = code.replace('PUBLISH', item(source, 'pub(crate) fn publish_control('))
a.evidence.mkdir(parents=True, exist_ok=False)
(a.evidence / 'test.rs').write_text(code)
subprocess.run(['rustc', '--edition=2021', '-O', str(a.evidence / 'test.rs'),
                '-o', str(a.evidence / 'gate')], check=True)
out = subprocess.check_output([str(a.evidence / 'gate')], text=True)
(a.evidence / 'output.txt').write_text(out)
(a.evidence / 'source.json').write_text(json.dumps({
    'bootstrap_sha256': hashlib.sha256(source.encode()).hexdigest(),
    'queue_sha256': hashlib.sha256(queue.encode()).hexdigest(),
    'GPU_execution': False,
}, indent=2) + '\n')
print(out, end='')
