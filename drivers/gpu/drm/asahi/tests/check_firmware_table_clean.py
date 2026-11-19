#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exercise actual firmware VM dirty publication, including failed cache clean."""
import argparse
import json
from pathlib import Path
import subprocess
import host_vm
import host_versions

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence',type=Path,required=True)
a=p.parse_args();a.evidence.mkdir(parents=True,exist_ok=False)
root=Path(__file__).resolve().parents[1]
code=host_vm.prelude().replace(
 '    pub struct Memory {',
 '    pub static CLEANS:std::sync::Mutex<Vec<(u64,usize)>>=std::sync::Mutex::new(Vec::new());\n'
 '    pub static FAIL_CLEAN:std::sync::atomic::AtomicBool=std::sync::atomic::AtomicBool::new(false);\n'
 '    pub struct Memory {')
code=code.replace('pub fn clean(&self,base:u64,size:usize)->Result{self.read(base,size).map(|_|())}',
 'pub fn clean(&self,base:u64,size:usize)->Result{CLEANS.lock().unwrap().push((base,size));'
 'if FAIL_CLEAN.swap(false,std::sync::atomic::Ordering::Relaxed){return Err(EIO);}self.read(base,size).map(|_|())}')
for name in ['scheduled_profile','scheduled_initdata','scheduled_boot_records','scheduled_hwdata',
             'g17p_abi','g17p_layout','g17p_initgraph','g17p_opening','g17p_topology','g17p_vm']:
 code+=f'#[path={json.dumps(str(root/(name+".rs")))}] mod {name};\n'
code+='''
use g17p_memory::{Memory,CLEANS,FAIL_CLEAN};use g17p_vm::Vm;use prelude::*;
fn take()->Vec<(u64,usize)>{std::mem::take(&mut *CLEANS.lock().unwrap())}
fn main()->Result{
 let mut m=Memory::new();let mut pages=[0;5];for p in &mut pages{*p=m.allocate(0x4000)?;}
 let mut v=Vm::g16c(pages[0],[pages[1],pages[2]],[pages[3],pages[4]])?;
 v.flush_tables(&m)?;assert_eq!(take().len(),5);
 for _ in 0..1000{v.flush_tables(&m)?;}assert!(take().is_empty());
 let a=m.allocate(0x8000)?;let va=0xfffffc2180000000;let flags=0x00c000000000044b;
 v.span(&mut m,2,va,a,0x8000,flags)?;take();
 FAIL_CLEAN.store(true,std::sync::atomic::Ordering::Relaxed);
 assert_eq!(v.flush_tables(&m),Err(EIO));assert_eq!(take().len(),1);
 v.flush_tables(&m)?;let count=take().len();assert!(count>5);
 for _ in 0..1000{v.flush_tables(&m)?;}assert!(take().is_empty());
 // Identical mappings do not dirty the tree. A leaf change with no new
 // parent allocation still needs a clean before publication.
 v.span(&mut m,2,va,a,0x8000,flags)?;v.flush_tables(&m)?;assert!(take().is_empty());
 v.span(&mut m,2,va+0x8000,a,0x4000,flags)?;v.flush_tables(&m)?;assert_eq!(take().len(),count);
 assert_eq!(v.physical(&m,2,va)?,a);assert_eq!(v.physical(&m,2,va+0x4000)?,a+0x4000);
 // Submission DATA writes clean their own ranges without dirtying PTEs.
 v.write(&mut m,2,va,&[7;16])?;assert_eq!(take(),vec![(a,16)]);
 v.flush_tables(&m)?;assert!(take().is_empty());
 println!("PASS actual VM: unchanged publications skip cleans; parent/leaf changes publish; failed clean retries; identical maps and DATA writes stay clean");Ok(())
}
'''
flags=host_versions.build(a.evidence)
(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021','-Dwarnings',str(a.evidence/'test.rs'),*flags,'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True)
(a.evidence/'output.txt').write_text(out);print(out,end='')
