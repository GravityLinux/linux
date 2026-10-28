#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare off-lock Max preparation with the saved in-lock production encoder.

Runs actual Rust constructors on both sides and compares every packet byte and
write address. This is an encoding/refactor check, not a GPU execution claim.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import host_versions
import host_fdt

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence',type=Path,required=True)
a=p.parse_args();tests=Path(__file__).resolve().parent;root=tests.parents[4]
revision='a1f4ccadb'
old=subprocess.check_output(['git','show',revision+':drivers/gpu/drm/asahi/scheduled_compute_runtime.rs'],cwd=root,text=True)
block=old[old.index('        let alias = l.descriptor_low(q.generation.count % 240);'):old.index('        vm.write(memory, 2, scheduler_slot, &1u32.to_le_bytes())?;',old.index('    fn stage('))]
current=(tests.parent/'g17p_compute_queues.rs').read_text()
def structure(text,name):
    m=re.search(r'pub\(crate\) struct '+name+r'\s*\{[^}]+\}',text)
    assert m,name
    return m.group()
layout=(tests.parent/'scheduled_generation.rs').read_text()
layout=layout[layout.index('#[derive(Clone, Copy, PartialEq, Eq)]'):]
# The actual layout's declaration and impl are pure address arithmetic.
# Later kernel-only items are absent from this source section.
assert layout.rstrip().endswith('}')
prelude=host_fdt.rust_prelude().replace('pub type KVec<T> = Vec<T>;', '''
    pub const EIO:Error=Error(5);pub const EOVERFLOW:Error=Error(75);pub const EBUSY:Error=Error(16);
    pub struct KVVec<T>(Vec<T>);
    impl<T> KVVec<T>{
        pub fn with_capacity(n:usize,_:u32)->Result<Self>{Ok(Self(Vec::with_capacity(n)))}
        pub fn resize(&mut self,n:usize,v:T,_:u32)->Result where T:Clone{self.0.resize(n,v);Ok(())}
    }
    impl<T> AsRef<[T]> for KVVec<T>{fn as_ref(&self)->&[T]{&self.0}}
    impl<T> std::ops::Deref for KVVec<T>{type Target=[T];fn deref(&self)->&[T]{&self.0}}
    impl<T> std::ops::DerefMut for KVVec<T>{fn deref_mut(&mut self)->&mut[T]{&mut self.0}}
''')
code=prelude+'\nuse prelude::*;\n'
code+=f'#[path={json.dumps(str(tests.parent/"g17p_compute.rs"))}]mod g17p_compute;\n'
code+='mod queues {use super::*;type Key=(u64,u32);\n'
code+='const CONTEXTS:usize=60;const FIRST_GRID:u8=32;const FW_BASE:u64=0xfffffc20e0000000;const LOW_BASE:u64=0x7200000000;const STRIDE:u64=0x200000;\n'
code+=layout+'\n'
parameters=structure((tests.parent/'g17p_compute_runtime.rs').read_text(),'Parameters')
code+='#[derive(Clone,Copy,PartialEq,Eq)]\n'+parameters+'\n'
code+=f'#[path={json.dumps(str(tests.parent/"scheduled_compute_preparation.rs"))}]mod wrapped;\n'
code+='use wrapped::WrappedSeed;\n#[derive(Clone,Copy,PartialEq,Eq)]\n'+structure(current,'PreparationSeed')+'\n'+structure(current,'PreparedWork')+'\n'
def implementation(text,name):
    start=text.index('impl '+name+' {');pos=text.index('{',start);depth=1
    while depth:
        pos+=1
        if text[pos]=='{':depth+=1
        elif text[pos]=='}':depth-=1
    return text[start:pos+1]
neo=subprocess.check_output(['git','show','a6883c3c6e5aa42548543a5a7b82bb6e375d6922:drivers/gpu/drm/asahi/g17p_compute_queues.rs'],cwd=root,text=True)
code+='use g17p_compute as c;const RECORDS:u32=256;\n'+implementation(current,'PreparationSeed')+'\n'
code+='mod original_neo {use super::*;\n#[derive(Clone,Copy,PartialEq,Eq)]\n'+structure(neo,'PreparationSeed')+'\n'+structure(neo,'PreparedWork')+'\n'+implementation(neo,'PreparationSeed')+'\n'
code+="""
pub(super) fn compare(seed:super::PreparationSeed)->Result {
 let original=PreparationSeed{layout:seed.layout,key:seed.key,ordinal:seed.ordinal,
    parameters:seed.parameters,priority:seed.priority,dependencies:seed.dependencies,
    dependency_count:seed.dependency_count,optional:seed.optional,first:seed.first}.prepare()?;
 let current=seed.prepare()?;
 assert_eq!(original.descriptor.as_ref(),current.descriptor.as_ref());
 assert_eq!(original.context.as_ref(),current.context.as_ref());
 assert_eq!(original.optional,current.optional);Ok(())
}
}
"""

code+='''
use g17p_compute::{self as protocol,work};
fn bytes(n:usize)->Result<KVVec<u8>>{let mut v=KVVec::with_capacity(n,GFP_KERNEL)?;v.resize(n,0,GFP_KERNEL)?;Ok(v)}
struct StageError;impl StageError{fn allocation(e:Error)->Error{e}}
struct Lane{queue:u64,scratch_high:u64,scratch_low:u64}
struct Storage{lanes:[Lane;3],context:u64,uuid:u16}
struct Generation{count:u32}
struct Queue{storage:Storage,ordinal:u32,operand_state:u64,generation:Generation}
struct Owner{context:u16,support:u64}
struct Writer;impl Writer{fn write(&self,m:&mut Vec<(u64,Vec<u8>)>,group:u32,at:u64,b:&[u8])->Result{assert_eq!(group,2);m.push((at,b.to_vec()));Ok(())}}
const PAGE:usize=0x4000;
fn original(seed:PreparationSeed)->Result<Vec<(u64,Vec<u8>)>>{
 let x=seed.wrapped.unwrap();let l=seed.layout;let sequence=seed.ordinal;let command=sequence%256;
 let p=&seed.parameters;let priority=seed.priority;let event=l.grid;
 let q=Queue{storage:Storage{lanes:std::array::from_fn(|_|Lane{queue:x.queue,scratch_high:l.context(),scratch_low:l.context_low()}),context:x.queue_control,uuid:x.uuid},ordinal:x.queue_ordinal,operand_state:x.operand_state,generation:Generation{count:sequence}};
 let o=Owner{context:x.context,support:x.support};let support_ordinal=x.support_ordinal;
 let command_save=x.save;let command_robustness=x.robustness;
 let scheduler_address=l.scheduler(command);let scheduler_slot=l.scheduler_slot(command);
 let descriptor_address=l.descriptor(sequence%240);let wrapper_address=l.event(command);
 let driver_address=l.dispatch(command)[0];let status=l.status(command);
 let stamp=sequence.wrapping_add(1).wrapping_shl(8);let namespace=x.context-1;
 let mut dependencies=[None;3];for(i,p)in seed.dependencies[..seed.dependency_count].iter().enumerate(){dependencies[i]=Some(*p);}
 let mut trace=Vec::new();let memory=&mut trace;let vm=Writer;
'''+block+'''
 Ok(trace)
}
pub fn run()->Result<usize>{let mut cases=0;
 for sequence in [0,1,2,127,239,240,254,255,256,65535,65536] {
  for context in [1,2,63] {for priority in 0..4 {for deps in 0..=3 {
   let mut layout=Layout::g16c((context-1)as usize)?;layout.grid=(context+1)as u8;
   let base=0x7000000000+u64::from(context)*0x100000000;
   let x=WrappedSeed{context,root:0x1000000000+u64::from(context)*0x4000,
      queue:layout.queue(),queue_control:layout.control(),uuid:context+16,
      queue_ordinal:sequence%257,support_ordinal:sequence.wrapping_add(17),support:layout.support(),
      save:base+0x2000000,robustness:base+0x2100000,operand_state:base+0x2200000,save_slot:3};
   let dependencies=[(1,5),(126,7),(127,3)];
   let seed=PreparationSeed{layout,key:(19,3),ordinal:sequence,
      parameters:Parameters{preempt:base,kernel_end:base+0x20000000,cdm:0x2000000000,end:0x2000001000,
         sampler:0x2000400000,sampler_count:29,timestamps:[layout.status(0),layout.status(0)+8]},
      priority,dependencies,dependency_count:deps,optional:false,first:sequence==0,wrapped:Some(x)};
   let mut neo_seed=seed;neo_seed.wrapped=None;neo_seed.layout=Layout::new((context-1)as usize%60,context)?;
   for optional in [false,true] {neo_seed.optional=optional;original_neo::compare(neo_seed)?;}
   let expected=original(seed)?;let prepared=x.prepare(seed)?;
   let actual=vec![(layout.descriptor(sequence%240),prepared.descriptor[..0x1000].to_vec()),
      (layout.event(sequence%256),prepared.wrapped.as_ref().unwrap().wrapper.to_vec()),
      (layout.event(sequence%256)+0xc0,prepared.wrapped.as_ref().unwrap().marker.to_vec()),
      (layout.context()+((u64::from(sequence)+1)&255)*0x200,prepared.context.to_vec()),
      (layout.scheduler(sequence%256),prepared.wrapped.as_ref().unwrap().scheduler.to_vec())];
   assert_eq!(actual,expected,"sequence={sequence} context={context} priority={priority} deps={deps}");cases+=1;
  }}}
 }
 Ok(cases)
}
}
fn main()->Result{println!("{}",queues::run()?);Ok(())}
'''
with tempfile.TemporaryDirectory(prefix='scheduled-compute-preparation-')as d:
    tmp=Path(d);flags=host_versions.build(tmp);main=tmp/'main.rs';main.write_text(code)
    subprocess.run(['rustc','--edition=2021',*flags,str(main),'-o',str(tmp/'gate')],check=True)
    count=int(subprocess.check_output([str(tmp/'gate')],text=True));assert count==528
    a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'harness.rs').write_text(code)
    (a.evidence/'summary.json').write_text(json.dumps(dict(cases=count,complete_images_per_case=5, exact_tagged_Neo_preparations=count*2,
        bytes_per_case=5120,all_bytes_and_addresses_equal=True,baseline_revision=revision,
        baseline_sha256=hashlib.sha256(old.encode()).hexdigest(),
        encoder_sha256=hashlib.sha256((tests.parent/'scheduled_compute_preparation.rs').read_bytes()).hexdigest(),
        native_GPU_execution=False),indent=2)+'\n')
print('PASS: 1056 tagged-Neo preparations and 528 Max preparation cases match saved production; all descriptor/wrapper/marker/context/scheduler bytes and addresses')
