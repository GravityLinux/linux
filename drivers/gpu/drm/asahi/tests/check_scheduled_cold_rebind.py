#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute actual off-lock preparation with cold geometry and a needed rebind.

The preparation method is compiled from production source. Host identities and
allocation results model the surrounding APIs; no GPU execution is simulated.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--source', type=Path)
p.add_argument('--evidence', type=Path, required=True)
a = p.parse_args()
source = a.source or Path(__file__).resolve().parents[1] / 'g17p_schedule.rs'
s = source.read_text()
start = s.index('    fn prepare_render(')
brace = s.index('{', start)
end, depth = brace + 1, 1
while depth:
    depth += (s[end] == '{') - (s[end] == '}')
    end += 1
method = s[start:end]
optional_objects = 'Option<KBox<' in method.split('{', 1)[0]
for module, name in [('g17p_render_runtime', 'PreparedAppend'),
                     ('g17p_render_runtime', 'PreparationSeed'),
                     ('g17p_boot', 'PreparedPoolRebind'),
                     ('g17p_user_vm', 'TableStorage')]:
    method = method.replace(f'super::super::{module}::{name}', name)
code = r'''
#![allow(dead_code,non_upper_case_globals,unused_variables)]
use std::cell::{Cell,RefCell,RefMut};
use std::sync::atomic::AtomicBool;
type KBox<T>=Box<T>;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]struct Error;
type Result<T>=std::result::Result<T,Error>;
macro_rules! pr_info{($($t:tt)*)=>{}}
mod kernel{pub mod time{
    pub struct Delta;impl Delta{pub fn from_millis(_:u64)->Self{Self}}
    pub mod delay{pub fn fsleep(_:super::Delta){}}
}}
mod module_parameters{
    pub struct Parameter;impl Parameter{pub fn value(&self)->u32{0}}
    pub static cpu_prepare_pause_queue:Parameter=Parameter;
}
static PREPARATION_PAUSED:AtomicBool=AtomicBool::new(false);
#[derive(Clone,Copy)]struct Client;
struct Snapshot(Client);impl Snapshot{fn client(&self)->&Client{&self.0}}
#[derive(Clone,Copy)]struct Parameters{queue_owner:Option<(u64,u32)>,
    prior_queue_ta:(),vdm_dependency:(),cdm_dependency:()}
#[derive(Clone,Copy)]enum Command{Render(Parameters),Compute}
struct Fence;fn status(_:&Fence)->i32{0}
struct Point{completion:Fence,error:Cell<Option<Error>>}
impl Point{fn finish(&self,e:Option<Error>){self.error.set(e)}}
struct Node{staged:bool,point:Point}
struct Prepared{parameters:[Command;1],queue_key:(u64,u32),
    render_snapshot:Option<Snapshot>,render_client:Option<Client>}
struct RuntimeInterval;
impl RuntimeInterval{
    fn waiting(){}fn acquired(_:(),_:(u64,u32),_:&str){}
}
struct Mutex(RefCell<Option<Runtime>>);
impl Mutex{fn lock(&self)->RefMut<'_,Option<Runtime>>{self.0.borrow_mut()}}
struct Device{runtime:Mutex}
struct Runtime{session:Session}
struct Session{cold:bool,rebind:bool,can_stage:bool,root_error:bool,descriptor_error:bool}
struct TableStorage;impl TableStorage{fn new(_:usize)->Result<Self>{Ok(Self)}}
struct RebindSeed{fail:bool}
struct PreparedPoolRebind;
impl RebindSeed{fn prepare(self,_:&Client)->Result<PreparedPoolRebind>{
    if self.fail{Err(Error)}else{Ok(PreparedPoolRebind)}
}}
struct PreparationSeed{fail:bool}
struct PreparedAppend;
impl PreparationSeed{fn prepare(self:Box<Self>)->Result<Box<PreparedAppend>>{
    if self.fail{Err(Error)}else{Ok(Box::new(PreparedAppend))}
}}
impl Session{
    fn render_rebind_size(&self,_:&Client,_:&Parameters)->Result<Option<usize>>{
        Ok(self.rebind.then_some(1))
    }
    fn can_stage_render(&self,_:Option<&Client>,_:&Parameters)->Result<bool>{Ok(self.can_stage)}
    fn capture_render_rebind(&self,_:&Client,_:&Parameters,storage:Option<TableStorage>)
        ->Result<Option<Option<RebindSeed>>>{
        Ok(if self.rebind{storage.map(|_|Some(RebindSeed{fail:self.root_error}))}else{Some(None)})
    }
    fn claim_render_preparation(&mut self,_:&Parameters,_:&Client)->Result<Option<Box<PreparationSeed>>>{
        Ok((!self.cold).then(||Box::new(PreparationSeed{fail:self.descriptor_error})))
    }
}
struct Schedule{nodes:Vec<Node>}
impl Schedule{
    fn dependencies(_:&Node,_:&Command)->Result<Option<((),[();2])>>{Ok(Some(((),[();2])))}
''' + method + r'''
}
fn main(){
    let schedule=Schedule{nodes:vec![Node{staged:false,point:Point{completion:Fence,error:Cell::new(None)}}]};
    let mut prepared=Prepared{parameters:[Command::Render(Parameters{queue_owner:None,
        prior_queue_ta:(),vdm_dependency:(),cdm_dependency:()})],queue_key:(3,1),
        render_snapshot:Some(Snapshot(Client)),render_client:Some(Client)};
    let dev=Device{runtime:Mutex(RefCell::new(Some(Runtime{session:Session{
        cold:true,rebind:true,can_stage:true,root_error:false,descriptor_error:false}})))};
    let first=schedule.prepare_render(&dev,&prepared)
        .expect("cold pool discarded a needed root plan while waiting for descriptor geometry");
    assert_eq!(first.0,0);assert!(first.2.is_some());
''' + ('    assert!(first.1.is_none());\n' if optional_objects else '') + r'''
    // Installing that returned plan allows the existing geometry phase to
    // run; normal descriptor preparation then makes progress independently.
    {let mut held=dev.runtime.lock();let s=&mut held.as_mut().unwrap().session;s.rebind=false;s.cold=false;}
    let warm=schedule.prepare_render(&dev,&prepared).unwrap();assert!(warm.2.is_none());
''' + ('    assert!(warm.1.is_some());\n' if optional_objects else '') + r'''
    {let mut held=dev.runtime.lock();held.as_mut().unwrap().session.rebind=true;}
    assert!(schedule.prepare_render(&dev,&prepared).unwrap().2.is_some());
    {let mut held=dev.runtime.lock();let s=&mut held.as_mut().unwrap().session;s.rebind=false;s.cold=true;}
    assert!(schedule.prepare_render(&dev,&prepared).is_none());
    {let mut held=dev.runtime.lock();let s=&mut held.as_mut().unwrap().session;s.rebind=true;s.root_error=true;}
    assert!(schedule.prepare_render(&dev,&prepared).is_none());
    assert_eq!(schedule.nodes[0].point.error.replace(None),Some(Error));
    {let mut held=dev.runtime.lock();let s=&mut held.as_mut().unwrap().session;
        s.cold=false;s.rebind=false;s.root_error=false;s.descriptor_error=true;}
    assert!(schedule.prepare_render(&dev,&prepared).is_none());
    assert_eq!(schedule.nodes[0].point.error.replace(None),Some(Error));
    {let mut held=dev.runtime.lock();let s=&mut held.as_mut().unwrap().session;
        s.descriptor_error=false;s.rebind=true;s.can_stage=false;}
    assert!(schedule.prepare_render(&dev,&prepared).is_none());
    assert!(schedule.nodes[0].point.error.get().is_none());
    {let mut held=dev.runtime.lock();held.as_mut().unwrap().session.can_stage=true;}
    prepared.render_client=None;
    assert!(schedule.prepare_render(&dev,&prepared).is_none());
    assert!(schedule.nodes[0].point.error.get().is_none());
    println!("PASS actual cold/warm preparation, independent root/descriptor plans and failure/retry ownership");
}
'''
with tempfile.TemporaryDirectory(prefix='cold-rebind-') as tmp:
    tmp = Path(tmp)
    (tmp / 'main.rs').write_text(code)
    subprocess.run(['rustc', '--edition=2021', str(tmp / 'main.rs'), '-o', str(tmp / 'gate')], check=True)
    output = subprocess.check_output([str(tmp / 'gate')], text=True)
    a.evidence.mkdir(parents=True, exist_ok=False)
    (a.evidence / 'harness.rs').write_text(code)
    (a.evidence / 'summary.json').write_text(json.dumps(dict(pass_=True,
        actual_production_preparation_method=True,independent_root_and_descriptor_plans=True,
        source_sha256=hashlib.sha256(s.encode()).hexdigest(),GPU_execution=False),indent=2)+'\n')
    print(output,end='')
