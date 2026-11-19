#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute the publication check after a physical-pool selection changes.

The actual scheduler branch and replacement helper are compiled unchanged.
Only snapshot identities, pool selection and handoff are host models; this
test neither simulates nor claims GPU execution.
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

def body(marker):
    start = s.index('{', s.index(marker)) + 1
    end, depth = start, 1
    while depth:
        depth += (s[end] == '{') - (s[end] == '}')
        end += 1
    return s[start:end - 1]

start = s.index('if matches!(dev.abi, super::super::GpuAbi::G16C) && render {')
start = s.index('                    let snapshot=', start)
# Include a parameter declaration moved before snapshot by the correction.
declaration = s.rfind('                    let Command::Render(mut parameters)=', 0, start)
branch = s.rfind('if matches!(dev.abi, super::super::GpuAbi::G16C) && render {', 0, start)
if declaration > branch:
    start = declaration
end = s.index('                    // Establish admission before pinning producer pools.', start)
publication = s[start:end]
code = r'''
#![allow(dead_code,irrefutable_let_patterns,unused_variables)]
#[derive(Clone,Copy,Debug,PartialEq,Eq)]struct Client(u32);
#[derive(Clone,Copy)]struct Snapshot(Client);
impl Snapshot{
    fn client(&self)->&Client{&self.0}
    fn same_client(&self,c:Option<&Client>)->bool{c==Some(&self.0)}
}
#[derive(Clone,Copy)]struct Parameters{
    queue_owner:Option<(u64,u32)>,prior_queue_ta:(),vdm_dependency:(),cdm_dependency:(),
}
#[derive(Clone,Copy)]enum Command{Render(Parameters)}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]struct Error;
const EIO:Error=Error;
type Result<T>=core::result::Result<T,Error>;
struct Session{logical:Option<Client>,selected:Option<Client>}
impl Session{
    fn g16c_client(&self,_:(u64,u32))->Option<&Client>{self.logical.as_ref()}
    fn render_selected_client(&self,_:&Client,p:&Parameters)->Result<Option<&Client>>{
        assert_eq!(p.queue_owner,Some((8,1)));Ok(self.selected.as_ref())
    }
}
struct Runtime{session:Session,deferred:bool}
struct Node;
struct Receipt;
struct Prepared{render_snapshot:Option<Snapshot>,render_client:Option<Client>,
    parameters:[Command;1],queue_key:(u64,u32)}
struct Scheduler;
impl Scheduler{
    fn handoff(r:&mut Runtime,_:usize,_:&mut Node,_:bool){r.deferred=true;}
    fn replacement(snapshot:&Snapshot,old:Option<&Client>,prepared:&mut Option<Client>)
        ->Result<(bool,Option<Client>)>{
''' + body('    fn replacement(') + r'''
    }
    fn publication(runtime:&mut Runtime,prepared:&mut Prepared,node:&mut Node)
        ->Result<Option<Receipt>>{
        let index=0;let previous=();let dependencies=[();2];
''' + publication + r'''
        Ok(Some(Receipt))
    }
}
fn main(){
    // Preparation selected caller A; another worker makes pool B eligible
    // before publication, while this logical queue still retains A elsewhere.
    for (logical,selected,prepared_client,ready,error) in [
        (Some(Client(1)),Some(Client(2)),None,false,false),
        (Some(Client(1)),Some(Client(2)),Some(Client(1)),true,false),
        (Some(Client(2)),Some(Client(1)),None,false,false),
        (Some(Client(1)),None,None,false,false),
        (Some(Client(1)),Some(Client(2)),Some(Client(2)),false,true),
        (Some(Client(2)),Some(Client(1)),Some(Client(1)),true,false),
    ]{
        let mut runtime=Runtime{session:Session{logical,selected},deferred:false};
        let mut prepared=Prepared{render_snapshot:Some(Snapshot(Client(1))),
            render_client:prepared_client,queue_key:(8,1),parameters:[Command::Render(
                Parameters{queue_owner:None,prior_queue_ta:(),vdm_dependency:(),cdm_dependency:()})]};
        let result=Scheduler::publication(&mut runtime,&mut prepared,&mut Node);
        assert_eq!(result.is_err(),error);
        if !error{assert_eq!(result.unwrap().is_some(),ready,"publication used logical rather than selected physical owner");
            assert_eq!(runtime.deferred,!ready);}
        assert_eq!(runtime.session.selected,selected,"no retirement before preparation validation");
        if ready && selected!=Some(Client(1)){assert_eq!(prepared.render_client,Some(Client(1)));}
    }
    println!("PASS six actual publication checks across changed physical selection");
}
'''
with tempfile.TemporaryDirectory(prefix='render-replacement-') as tmp:
    tmp = Path(tmp)
    (tmp / 'main.rs').write_text(code)
    subprocess.run(['rustc', '--edition=2021', str(tmp / 'main.rs'), '-o', str(tmp / 'gate')], check=True)
    output = subprocess.check_output([str(tmp / 'gate')], text=True)
    a.evidence.mkdir(parents=True, exist_ok=False)
    (a.evidence / 'harness.rs').write_text(code)
    (a.evidence / 'summary.json').write_text(json.dumps(dict(pass_=True, cases=6,
        source_sha256=hashlib.sha256(s.encode()).hexdigest(),
        actual_publication_branch=True, actual_replacement_helper=True,
        device_access=False, GPU_execution=False), indent=2) + '\n')
    print(output, end='')
