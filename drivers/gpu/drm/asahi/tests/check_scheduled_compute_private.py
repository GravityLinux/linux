#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Run the common Max generation's actual private builder in bounded DATA RAM.

Allocation/UAT stand-ins retain extents, PTEs and authored bytes; no hardware
cache operations or GPU execution are simulated. Check two disjoint roots,
all directory/table entries, sparse save guards, and the full relocation set.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import host_fdt
import host_versions

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence',type=Path,required=True)
a=p.parse_args();tests=Path(__file__).resolve().parent

def item(s,marker):
 a=s.index(marker);b=s.index('{',a);depth=1;j=b+1
 while depth:
  if s[j]=='{':depth+=1
  elif s[j]=='}':depth-=1
  j+=1
 return s[a:j]

prelude=host_fdt.rust_prelude().replace('pub type KVec<T> = Vec<T>;', '''
    pub const EIO:Error=Error(5);pub const EOVERFLOW:Error=Error(75);pub const EBUSY:Error=Error(16);
    pub struct KVec<T>(Vec<T>);pub type KVVec<T>=KVec<T>;
    impl<T>KVec<T>{
        pub fn new()->Self{Self(Vec::new())}
        pub fn with_capacity(n:usize,_:u32)->Result<Self>{Ok(Self(Vec::with_capacity(n)))}
        pub fn push(&mut self,x:T,_:u32)->Result{self.0.push(x);Ok(())}
        pub fn resize(&mut self,n:usize,x:T,_:u32)->Result where T:Clone{self.0.resize(n,x);Ok(())}
    }
    impl<T>std::ops::Deref for KVec<T>{type Target=[T];fn deref(&self)->&[T]{&self.0}}
    impl<T>std::ops::DerefMut for KVec<T>{fn deref_mut(&mut self)->&mut[T]{&mut self.0}}
''')
code=prelude+'''
use prelude::*;
mod g17p_user_vm {
    use super::*;use std::collections::BTreeMap;
    pub struct UserVm{pub pages:BTreeMap<u64,u64>}
    impl UserVm{
        pub fn new()->Self{Self{pages:BTreeMap::new()}}
        pub fn prepare(&mut self,a:u64,n:u64)->Result{assert_eq!(a%0x4000,0);assert_eq!(n%0x4000,0);Ok(())}
        pub fn map_page(&mut self,a:u64,p:u64,write:bool)->Result{assert!(write);assert_eq!(p%0x4000,0);assert!(self.pages.insert(a,p|0x0080000000000c8b).is_none());Ok(())}
        pub fn pte(&self,a:u64)->Result<u64>{Ok(*self.pages.get(&a).unwrap_or(&0))}
    }
}
struct Memory{next:u64,allocations:Vec<(u64,usize)>,writes:std::collections::BTreeMap<u64,Vec<u8>>}
impl Memory{
 fn new()->Self{Self{next:0x10100000000,allocations:Vec::new(),writes:std::collections::BTreeMap::new()}}
 fn allocate(&mut self,n:usize)->Result<u64>{assert!(n<=0x200000);let span=n.next_power_of_two()as u64;
  let p=(self.next+span-1)&!(span-1);self.next=p+span+0x4000;self.allocations.push((p,span as usize));Ok(p)}
 fn valid(&self,p:u64,n:usize)->Result{if self.allocations.iter().any(|&(a,size)|a<=p&&p+n as u64<=a+size as u64){Ok(())}else{Err(EINVAL)}}
 fn clean(&self,p:u64,n:usize)->Result{self.valid(p,n)}
 fn invalidate(&self,p:u64,n:usize)->Result{self.valid(p,n)}
 fn write(&mut self,p:u64,b:&[u8])->Result{self.valid(p,b.len())?;self.writes.insert(p,b.to_vec());Ok(())}
 fn zero(&mut self,p:u64,n:usize)->Result{self.write(p,&vec![0;n])}
}
struct Vm;impl Vm{fn write(&self,_:&mut Memory,_:usize,_:u64,_:&[u8])->Result{unreachable!()}
 fn physical(&self,_:&Memory,_:usize,_:u64)->Result<u64>{unreachable!()}}
mod g17p_compute_runtime{
 use super::*;use super::g17p_user_vm::UserVm;
 const PAGE:usize=0x4000;
'''+item((tests.parent/'g17p_compute_runtime.rs').read_text(),'pub(crate) fn client_storage(')+'''
}
'''
for name in ['g17p_queue','scheduled_dispatch','scheduled_operands','scheduled_lifetime']:
 code+=f'#[path={json.dumps(str(tests.parent/(name+".rs")))}]mod {name};\n'
source=(tests.parent/'scheduled_compute_preparation.rs').read_text()
params=item((tests.parent/'g17p_compute_runtime.rs').read_text(),'pub(crate) struct Parameters')
layout=(tests.parent/'scheduled_generation.rs').read_text();layout=layout[layout.index('#[derive(Clone, Copy, PartialEq, Eq)]'):]
code+='mod queues{use super::*;\n'
code+='const CONTEXTS:usize=60;const FIRST_GRID:u8=32;const FW_BASE:u64=0xfffffc20e0000000;const LOW_BASE:u64=0x7200000000;const STRIDE:u64=0x200000;\n'+layout
code+='\n'+params+'\n'+item(source,'pub(crate) struct WrappedSeed')+'\n'
code+='struct Client{root:super::g17p_user_vm::UserVm}\n'
code+=f'#[path={json.dumps(str(tests.parent/"scheduled_compute_queue.rs"))}]mod wire;\n'
code+='''
pub fn run()->Result<usize>{
 use std::collections::BTreeSet;
 let mut memory=Memory::new();let mut previous=BTreeSet::new();let mut leaves=0;
 for base in [0x7000000000,0x7100000000] {
  let p=Parameters{preempt:base,kernel_end:base+0x20000000,cdm:0,end:4,sampler:0,sampler_count:0,timestamps:[0;2]};
  let layout=wire::PrivateLayout::new(&p)?;
  assert_eq!((layout.table,layout.state,layout.buffers,layout.save,layout.robustness,layout.end),
   (base+0x1008000,base+0x1020000,base+0x10a0000,base+0xb1c0000,base+0x129c0000,base+0x129c4000));
  let mut client=Client{root:g17p_user_vm::UserVm::new()};layout.build(&mut memory,&mut client)?;
  let offsets=layout.offsets()?;let unique:BTreeSet<_>=offsets.iter().copied().collect();assert_eq!(unique.len(),offsets.len());
  let mapped:BTreeSet<_>=client.root.pages.keys().map(|a|a-base).collect();assert_eq!(unique,mapped);
  assert_eq!(client.root.pte(base+0x109c000)?,0); // Last state page is followed by its guard.
  let mask=0x000003ffffffc000;
  for i in 0..20u64 {
   let list=client.root.pte(base+i*0x4000)?&mask;let data=memory.writes.get(&list).unwrap();
   for page in 0..2048 {assert_eq!(u64::from_le_bytes(data[page*8..page*8+8].try_into().unwrap()),base+0x10a0000+i*0x808000+page as u64*0x1000);}
   assert_eq!(client.root.pte(base+0x10a0000+i*0x808000+0x800000)?,0);
  }
  let table=client.root.pte(layout.table)?&mask;let data=memory.writes.get(&table).unwrap();let mut expected=vec![0;0x10000];
  for i in 0..20usize {expected[i*64..i*64+8].copy_from_slice(&((base+0x10a0000+i as u64*0x808000)|(1<<63)).to_le_bytes());}
  assert_eq!(*data,expected);
  for slot in 0..256u64 {for offset in [0,0x4000,0x8000] {
    assert_ne!(client.root.pte(layout.save+slot*0x78000+offset)?,0);
   }assert_eq!(client.root.pte(layout.save+slot*0x78000+0xc000)?,0);}
  let physical:BTreeSet<_>=client.root.pages.values().map(|p|p&mask).collect();assert_eq!(physical.len(),mapped.len());
  assert!(previous.is_disjoint(&physical));previous.extend(physical);leaves+=mapped.len();
  let mut invalid=p;invalid.kernel_end=layout.end-1;assert!(wire::PrivateLayout::new(&invalid).is_err());
 }
 Ok(leaves)
}
}
fn main()->Result{println!("{}",queues::run()?);Ok(())}
'''
with tempfile.TemporaryDirectory(prefix='scheduled-compute-private-')as d:
 tmp=Path(d);flags=host_versions.build(tmp);main=tmp/'main.rs';main.write_text(code)
 subprocess.run(['rustc','--edition=2021',*flags,str(main),'-o',str(tmp/'gate')],check=True)
 leaves=int(subprocess.check_output([str(tmp/'gate')],text=True));assert leaves>22000
 a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'harness.rs').write_text(code)
 (a.evidence/'summary.json').write_text(json.dumps(dict(disjoint_roots=2,exact_relocation_leaves=leaves,
  all_directory_and_table_bytes_checked=True,all256_sparse_save_guards_checked=True,
  physical_aliases_absent=True,largest_contiguous_allocation=0x200000,
  actual_production_private_builder_and_client_storage=True,native_GPU_execution=False,
  source_sha256={n:hashlib.sha256((tests.parent/n).read_bytes()).hexdigest()for n in ['scheduled_compute_queue.rs','g17p_compute_runtime.rs']}),indent=2)+'\n')
print('PASS: two disjoint compute roots,',leaves,'exact relocation leaves, full directory/table bytes, all256 sparse save guards')
