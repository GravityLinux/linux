#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare common off-lock render preparation with the saved Max encoder.

Compile both production encoders; compare all ten images and store addresses.
Only owned Rust and DATA images are handled. No GPU execution is simulated.
"""
import argparse
import hashlib
import json
import re
from pathlib import Path
import subprocess
import tempfile
import host_fdt
import host_versions

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence',type=Path,required=True)
p.add_argument('--counter-reference',type=Path,help='Native child/parent DATA: independent lifecycle, milestone and render event slot')
p.add_argument('--retained-native',type=Path,help='Native warm-A/warm-B/pair DATA: retained queue entry fields')
a=p.parse_args();tests=Path(__file__).resolve().parent;root=tests.parents[4]
revision='900762872a930a8c5f7677893d459baacf821ae1'
old=subprocess.check_output(['git','show',revision+':drivers/gpu/drm/asahi/scheduled_render_submit.rs'],cwd=root,text=True)
x=old.index('        let ps = program::ProgramStateG16C {');y=old.index('        let receipt =',x);program=re.sub(r"\bBASE\b","base",old[x:y])
x=old.index('        for stage in 0..2usize {');y=old.index('        let mut event = [0u8; 0x28];',x);block=re.sub(r"\bBASE\b","base",old[x:y])
prelude=host_fdt.rust_prelude().replace('pub type KVec<T> = Vec<T>;', '''
    pub const EIO:Error=Error(5);pub const EOVERFLOW:Error=Error(75);
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
code=prelude+'\nuse prelude::*;\n'
for name in ['g17p_compute','g17p_compute_memory','g17p_render','g17p_render_graph']:
 code+=f'#[path={json.dumps(str(tests.parent/(name+".rs")))}]mod {name};\n'
life=(tests.parent/'g17p_render_lifecycle.rs').read_text();start=life.index('pub(crate) fn pool_grids(');end=life.index('\n}',start)+2
code+='mod g17p_render_lifecycle{use crate::g17p_compute::{Error,Result};pub const POOL_SLOTS:u32=15;pub const STORAGE_SUBMISSIONS:u32=256;'+life[start:end]+'}\n'
code+='''mod render_runtime {
use super::*;use g17p_render::{self as command,Parameters,program,wire as body};use g17p_compute::work;
struct Deferred{address:u64,body:KVVec<u8>,previous:Option<KVVec<u8>>}
'''
code+=f'#[path={json.dumps(str(tests.parent/"scheduled_render_preparation.rs"))}]mod wrapped;\n'
code+='''
struct Resource{sequence:u32,parameter:Parameter}struct Parameter{slot:u32}
struct Storage{lanes:[wrapped::Lane;3],context:u64,uuid:u16}struct Queue{storage:Storage}
struct State{queues:Vec<Queue>}
struct Writer;impl Writer{fn write(&self,m:&mut Vec<(u64,Vec<u8>)>,group:u32,a:u64,b:&[u8])->Result{assert_eq!(group,2);m.push((a,b.to_vec()));Ok(())}}
const PAGE:u64=0x4000;
fn bytes(n:usize)->Result<KVVec<u8>>{let mut v=KVVec::with_capacity(n,GFP_KERNEL)?;v.resize(n,0,GFP_KERNEL)?;Ok(v)}
struct StageError;impl StageError{fn allocation(e:Error)->Error{e}}
impl State {
fn original(&self,wire:wrapped::WrappedSeed,p:&Parameters)->Result<Vec<(u64,Vec<u8>)>>{
 let base=wire.base;let qi=0usize;let n=wire.ordinal;let local=wire.local;let first_event=0u8;let context=u64::from(wire.context);
 let r=Resource{sequence:wire.sequence,parameter:Parameter{slot:wire.slot}};
 let geo=[p.tilemap,p.tpc,wire.layermeta];let scene_slot=wire.sequence%79+1;
 let tpc_size=p.scratch_layout().map_err(|_|EINVAL)?.2.checked_mul(4).ok_or(EOVERFLOW)?;
 let slot=wire.slot;let support=wire.support;let manager=wire.manager;let scene=wire.scene;let empty=wire.empty;
 // Native descriptors and their owned driver stamps advance per queue.
 let stamp=(local+1).wrapping_shl(8);
 let addresses=[p.ta_user_timestamp_start,p.ta_user_timestamp_end,p.fragment_user_timestamp_start,p.fragment_user_timestamp_end];
'''+program+'''
 let mut trace=Vec::new();let memory=&mut trace;let vm=Writer;
 let mut descriptor=bytes(body::FragmentDescriptorG16C::SIZE)?;
'''+block+'''
 Ok(trace)
}
}
pub fn run()->Result<usize>{let mut cases=0;
 for n in [0,1,2,78,79,255,256,65535,65536] {for local in [n,n%3] {
  for storage in [0,1,17,255] {for context in [1,63] {for priority in [0,3] {for layers in [1,2] {
   let base=0x1000000000;
   let p=Parameters{width:64,height:32,layers,context_base:base,tilemap:base+0x200000,tpc:base+0x300000,
    heapmeta:base+0x400000,deflake_1:base+0x68da0,deflake_2:base+0x68120,deflake_3:base+0x68000,
    encoder:base+0x10000,ta_status:base+0x78000,fragment_status:base+0x490000,aux_fb:base+0x4d0000,
    reactive_tvb_growth:true,aux_fb_flags:0xc000,..Default::default()};
   let lanes=std::array::from_fn(|i|wrapped::Lane{queue:0xfffffc2208000000+i as u64*0x2c000,
     scratch_low:0x7400000000+i as u64*0x2c000,scratch_high:0xfffffc220800c000+i as u64*0x2c000,priority});
   let state=State{queues:vec![Queue{storage:Storage{lanes,context:0xfffffc2208000140,uuid:17}}]};
   let wire=wrapped::WrappedSeed {base:wrapped::storage_base(storage)?,storage,ordinal:n,local,sequence:n%99,milestone:n,grids:[0,1],context,slot:u32::from(context%15),
    support:0xfffffc2207000000,manager:0xfffffc2209000000,scene:0xfffffc2209100000,
    empty:0xfffffc2209200000,layermeta:base+0x400000,queue_control:state.queues[0].storage.context,uuid:17,lanes:[lanes[0],lanes[1]]};
   let mut old=state.original(wire,&p)?;let actual=wire.build(&p)?;
   // The saved single-pool encoder always wrote event slot zero. Neo's
   // points(kind, event_slot, ...) and independent native Max records use
   // the physical render pool. Adjust only that byte in the saved oracle.
   for (address,body) in &mut old {
    if [wire.base+0x4000,wire.base+0xc000].contains(address) {
     // Native warm/retained pairs clear queue initialization after the first
     // publication and advance the shifted pool milestone, independently of
     // the global render ordinal. The saved multi-owner branch did neither.
     body[0x52..0x54].copy_from_slice(&u16::from(wire.local==0).to_le_bytes());
     body[0x62..0x64].copy_from_slice(&u16::from(wire.local==0).to_le_bytes());
     body[0x2e..0x30].copy_from_slice(&(((wire.milestone&255)<<8)as u16).to_le_bytes());
    }
    if lanes.iter().any(|lane|*address==lane.scratch_high+u64::from((n+1)&255)*0x200) {
     assert_eq!(body[0x24],0);body[0x24]=wire.slot as u8;
    }
    if [0x9c0,0x2240].contains(&body.len()) {
     // The saved first-context template also fixed the context identity to1.
     // Native UUID/lifecycle DATA identifies the selected UAT context here.
     let fragment=body.len()==0x2240;let uuid_at=if fragment{0x2168}else{0x8ce};
     body[uuid_at+3]=wire.context as u8;
     // Native retained TA keeps descriptor ordinal zero; fragment advances
     // its queue-local ordinal. Both retain the same tail flag as first use.
     let ordinal_at=if fragment{0x2170}else{0x8d2};
     body[ordinal_at..ordinal_at+4].copy_from_slice(&(if fragment{wire.local}else{0}).to_le_bytes());
     body[if fragment{0x21e7}else{0x94d}]=1;
     let start=if fragment{0xa0}else{0x60};let count=if fragment{91}else{83};
     for register in body[start..start+count*12].chunks_exact_mut(12) {
      let number=u32::from_le_bytes(register[..4].try_into().unwrap());
      if [0x160e0,0x1499,0xa341,0x1ca10,0x14a1,0xa349].contains(&number) {
       register[7]=wire.context as u8;
      }
     }
    }
   }
   let actual:Vec<_>=actual.iter().flat_map(|v|v.iter()).map(|d|(d.address,d.body.to_vec())).collect();
   assert_eq!(actual,old,"n={n} local={local} context={context} priority={priority} layers={layers}");
   assert_eq!(actual.len(),10);cases+=1;
  }}}}}
 }
 assert!(wrapped::storage_base(256).is_err());
 Ok(cases)
}
}
fn main()->Result{println!("{}",render_runtime::run()?);Ok(())}
'''
counter_references={}
if a.counter_reference:
 import struct
 rows=[]
 for ordinal,phase in enumerate(['child','parent']):
  path=a.counter_reference/phase/'stage-0-wrapper.bin';raw=path.read_bytes();assert len(raw)==0xc0
  assert json.loads((path.parent/'capture-validation.json').read_text())['pass']
  lifecycle=struct.unpack_from('<H',raw,0x76)[0];item=struct.unpack_from('<I',raw,0x2a)[0]
  namespace=struct.unpack_from('<H',raw,0x7e)[0]
  native_context=struct.unpack_from('<H',raw,0x32)[0]
  controls=[];uuid_contexts=[];targets=[]
  for stage in (0,1):
   record_path=path.parent/f'stage-{stage}-record.bin';record=record_path.read_bytes();assert len(record)==0x200
   controls.append(struct.unpack_from('<H',record,0x24)[0])
   counter_references[str(record_path)]=hashlib.sha256(record).hexdigest()
   descriptor_path=path.parent/f'stage-{stage}-descriptor.bin';descriptor=descriptor_path.read_bytes()
   assert struct.unpack_from('<I',descriptor,0xc)[0]==native_context
   uuid_contexts.append(struct.unpack_from('<I',descriptor,0x2168 if stage else 0x8ce)[0]>>24)
   targets.append([struct.unpack_from('<I',descriptor,at)[0]
     for at in ([0x2160] if stage else [0x7a8,0x7b0,0x8c6,0x8d6])])
   registers=dict(struct.unpack_from('<IQ',descriptor,(0xa0 if stage else 0x60)+i*12)
     for i in range(91 if stage else 83))
   for number in ([0x160e0,0x1499,0xa341] if stage else [0x1ca10,0x14a1,0xa349]):
    assert (registers[number]>>24)&255==native_context
   counter_references[str(descriptor_path)]=hashlib.sha256(descriptor).hexdigest()
  assert uuid_contexts==[native_context]*2
  assert targets==[[item+1,((item+1)&255)*0x101,item+1,item],[item+1]]
  assert controls==[0xc00|namespace,0x1800|namespace]
  rows.append(f'({ordinal}u32,{item}u32,{lifecycle}u16,{namespace}u32,{native_context}u16,[{controls[0]}u16,{controls[1]}u16],'+
    '['+','.join(f'{value}u32'for value in targets[0])+'],['+f'{targets[1][0]}u32'+'])')
  counter_references[str(path)]=hashlib.sha256(raw).hexdigest()
 extra="""
   if cases==0 {
    for (ordinal,milestone,native_lifecycle,slot,context,native_controls,ta_targets,fr_targets) in [ROWS] {
     let probe=wrapped::WrappedSeed {ordinal,milestone,local:0,slot,context,..wire};
     let objects=probe.build(&p)?;
     let entry=objects[0].iter().find(|object|object.address==probe.base+0x4000).unwrap();
     assert_eq!(u16::from_le_bytes(entry.body[0x76..0x78].try_into().unwrap()),native_lifecycle,
       "native Work15 lifecycle is independent of pool milestone: global={ordinal} pool={milestone}");
     assert_eq!(u32::from_le_bytes(entry.body[0x2a..0x2e].try_into().unwrap()),milestone);
     for stage in 0..2 {
      let address=probe.lanes[stage].scratch_high+u64::from((milestone+1)&255)*0x200;
      let record=objects[stage].iter().find(|object|object.address==address).unwrap();
      assert_eq!(u16::from_le_bytes(record.body[0x24..0x26].try_into().unwrap()),native_controls[stage],
        "native Max event slot follows render pool: pool={slot} stage={stage}");
      let descriptor=objects[stage].iter().find(|object|object.address==probe.base+
        if stage==0{0x2940}else{0x9b40}).unwrap();
      let offsets:&[usize]=if stage==0{&[0x7a8,0x7b0,0x8c6,0x8d6]}else{&[0x2160]};
      let expected:&[u32]=if stage==0{&ta_targets}else{&fr_targets};
      for (&at,&value)in offsets.iter().zip(expected) {
       assert_eq!(u32::from_le_bytes(descriptor.body[at..at+4].try_into().unwrap()),value,
        "native descriptor target follows pool milestone: stage={stage} offset={at:#x} ordinal={ordinal} milestone={milestone}");
      }
      let uuid_at=if stage==0{0x8ce}else{0x2168};
      assert_eq!(u32::from_le_bytes(descriptor.body[uuid_at..uuid_at+4].try_into().unwrap())>>24,
        u32::from(context),"native descriptor UUID contains its application context: stage={stage}");
      let start=if stage==0{0x60}else{0xa0};let count=if stage==0{83}else{91};
      for register in descriptor.body[start..start+count*12].chunks_exact(12) {
       let number=u32::from_le_bytes(register[..4].try_into().unwrap());
       if [0x160e0,0x1499,0xa341,0x1ca10,0x14a1,0xa349].contains(&number) {
        let value=u64::from_le_bytes(register[4..12].try_into().unwrap());
        assert_eq!((value>>24)&255,u64::from(context),"native lifecycle context: stage={stage}");
       }
      }
     }
    }
   }
 """.replace('ROWS',','.join(rows))
 needle='   let mut old=state.original(wire,&p)?;let actual=wire.build(&p)?;';assert needle in code
 code=code.replace(needle,extra+needle)

retained_references={}
if a.retained_native:
 import struct
 rows=[]
 for tag in ('A','B'):
  previous=None
  for phase in ('warm-'+tag,'pair/'+tag):
   parent=a.retained_native/phase
   verification=a.retained_native/('pair'if phase.startswith('pair/')else phase)/'native-verification.json'
   qualified=json.loads(verification.read_text());assert qualified['single_user']
   output=qualified['outputs'][tag]if 'outputs'in qualified else qualified
   assert output['output_changed']and output['output_expected']
   retained_references[str(verification)]=hashlib.sha256(verification.read_bytes()).hexdigest()
   raw=[(parent/f'stage-{stage}-wrapper.bin').read_bytes()for stage in (0,1)]
   identity=[tuple(struct.unpack_from('<Q',b,at)[0]for at in (8,16,0x4a))for b in raw]
   if previous is not None:assert identity==previous,'not the same retained physical queues'
   previous=identity
   ordinal=struct.unpack_from('<H',raw[0],0x76)[0]
   item=struct.unpack_from('<I',raw[0],0x2a)[0]
   slot=struct.unpack_from('<H',raw[0],0x56)[0]
   context=struct.unpack_from('<H',raw[0],0x32)[0]
   for stage,b in enumerate(raw):
    assert len(b)==0xc0
    expected=[struct.unpack_from('<H',b,at)[0]for at in (0x1a,0x2e,0x52,0x62)]
    descriptor_path=parent/f'stage-{stage}-descriptor.bin';descriptor=descriptor_path.read_bytes()
    assert len(descriptor)>=(0x2240 if stage else 0x9c0)
    descriptor_ordinal=struct.unpack_from('<I',descriptor,0x2170 if stage else 0x8d2)[0]
    tail=descriptor[0x21e7 if stage else 0x94d]
    stamp=struct.unpack_from('<I',descriptor,0x2150 if stage else 0x8b6)[0]
    rows.append('('+','.join(map(str,[ordinal,item,slot,context,stage,*expected,descriptor_ordinal,tail,stamp]))+')')
    retained_references[str(descriptor_path)]=hashlib.sha256(descriptor).hexdigest()
    path=parent/f'stage-{stage}-wrapper.bin';retained_references[str(path)]=hashlib.sha256(b).hexdigest()
 extra='''
   if cases==0 {
    for (ordinal,item,slot,context,stage,first,shifted,init52,init62,descriptor_ordinal,tail,stamp) in [ROWS] {
     let probe=wrapped::WrappedSeed {ordinal,local:item,sequence:item,milestone:item,slot,context,..wire};
     let objects=probe.build(&p)?;
     let address=probe.base+if stage==0 {0x4000}else{0xc000};
     let body=&objects[stage].iter().find(|object|object.address==address).unwrap().body;
     for (at,expected)in [(0x1a,first),(0x2e,shifted),(0x52,init52),(0x62,init62)] {
      assert_eq!(u16::from_le_bytes(body[at..at+2].try_into().unwrap()),expected,
        "native retained wrapper field {at:#x}: global={ordinal} queue={item} pool={slot} stage={stage}");
     }
     let address=probe.base+if stage==0 {0x2940}else{0x9b40};
     let body=&objects[stage].iter().find(|object|object.address==address).unwrap().body;
     let at=if stage==0 {0x8d2}else{0x2170};
     assert_eq!(u32::from_le_bytes(body[at..at+4].try_into().unwrap()),descriptor_ordinal,
       "native retained descriptor ordinal: global={ordinal} queue={item} pool={slot} stage={stage}");
     assert_eq!(body[if stage==0 {0x94d}else{0x21e7}],tail,
       "native retained descriptor tail: global={ordinal} queue={item} pool={slot} stage={stage}");
     let at=if stage==0 {0x8b6}else{0x2150};
     assert_eq!(u32::from_le_bytes(body[at..at+4].try_into().unwrap()),stamp,
       "native retained descriptor stamp: global={ordinal} queue={item} pool={slot} stage={stage}");
    }
   }
 '''.replace('ROWS',','.join(rows))
 needle='   let mut old=state.original(wire,&p)?;let actual=wire.build(&p)?;';assert needle in code
 code=code.replace(needle,extra+needle)

with tempfile.TemporaryDirectory(prefix='scheduled-render-preparation-')as d:
 tmp=Path(d);flags=host_versions.build(tmp);main=tmp/'main.rs';main.write_text(code)
 subprocess.run(['rustc','--edition=2021',*flags,str(main),'-o',str(tmp/'gate')],check=True)
 count=int(subprocess.check_output([str(tmp/'gate')],text=True));assert count==576
 a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'harness.rs').write_text(code)
 (a.evidence/'summary.json').write_text(json.dumps(dict(cases=count,full_images_per_case=10,
    all_packet_bytes_and_addresses_match=True,reference_allows_only_coherent_body_relocation=False,reference_revision=revision,
    reference_adjustments=['coherent descriptor body relocation','native render event slot in record byte0x24','native UAT context in UUID/lifecycle bits24..31','native queue initialization fields0x52/0x62 and shifted milestone0x2e','native retained descriptor ordinal and tail flag','native queue-local completion stamp'],
    reference_source_sha256=hashlib.sha256(old.encode()).hexdigest(),
    current_source_sha256=hashlib.sha256((tests.parent/'scheduled_render_preparation.rs').read_bytes()).hexdigest(),
    reference_render_event_slots_corrected_from_native=True,
    actual_GPU_execution=False,native_counter_domain_cases=2 if counter_references else 0,
    native_render_event_slot_cases=4 if counter_references else 0,native_counter_reference_sha256=counter_references,
    native_retained_wrapper_cases=8 if retained_references else 0,native_retained_reference_sha256=retained_references),indent=2)+'\n')
print('PASS: 576 Max render preparation cases, ten complete images/addresses; saved encoder with relocated bodies and native render event slots')
