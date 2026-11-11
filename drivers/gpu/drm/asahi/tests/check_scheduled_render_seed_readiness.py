#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compile the actual retained descriptor seed method and check its prerequisites.

Host stubs make premature serialization fatal; this checks CPU preparation
ordering, not GPU execution or firmware output.
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
source = a.source or Path(__file__).resolve().parents[1] / 'scheduled_render_submit.rs'
s = source.read_text()
start = s.index('    pub(crate) fn render_preparation_seed(')
brace = s.index('{', start)
end, depth = brace + 1, 1
while depth:
    depth += (s[end] == '{') - (s[end] == '}')
    end += 1
method = s[start:end].replace('super::super::super::g17p_render_runtime::PreparationSeed', 'PreparationSeed')
code = r'''
#![allow(dead_code,unused_variables)]
use std::cell::Cell;
type KBox<T> = Box<T>;
#[derive(Debug)] struct Error;
const EIO: Error = Error;
type Result<T> = std::result::Result<T,Error>;
struct Root;
struct Client { id:u32, root:Root }
mod compute { pub(super) use super::Client; }
fn same(a:&Client,b:&Client)->bool { a.id==b.id }
#[derive(Clone,Copy)] struct Parameters {
    kernel_start:u64,kernel_end:u64,tilemap:u64,tpc:u64,heapmeta:u64,
    ta_user_timestamp_start:u64,ta_user_timestamp_end:u64,
    fragment_user_timestamp_start:u64,fragment_user_timestamp_end:u64,
}
mod command { pub(super) use super::Parameters; }
fn geometry_sizes(_: &Parameters)->Result<[u64;3]> { Ok([1,2,3]) }
struct Resource { needs_geometry:bool }
impl Resource {
    fn geometry_plan(&self,_:&Root,_:[u64;3])->Result<Option<()>> {
        Ok(self.needs_geometry.then_some(()))
    }
}
struct Owner { client:Option<Client>,directory:u64,private_limit:u64,render:Option<Resource> }
struct Queue { owner:usize }
struct State { owners:Vec<Owner>,queues:Vec<Queue>,render_ordinal:u32,
    forbid_serialization:bool,serialized:Cell<u32> }
struct Session;
impl Session {
    fn next_wrapped_render_storage(&self,_:u32)->Option<u32> { Some(0) }
    fn timestamp_cache_seed(&self,_:&[u64])->Result<()> { Ok(()) }
}
struct PreparationSeed;
impl PreparationSeed {
    fn wrapped(_:u32,_:Parameters,_:&Client,_:())->Result<KBox<Self>> { Ok(Box::new(Self)) }
}
impl State {
    fn retained_render_index(&self,_:&Session,_:&Client,_:&Parameters)->Option<usize> {Some(0)}
    fn render_seed(&self,_:usize,_:u32,p:&Parameters)->Result<(Parameters,u32)> {
        assert!(!self.forbid_serialization,"descriptor seed serialized before caller/window/geometry was ready");
        self.serialized.set(self.serialized.get()+1); Ok((*p,0))
    }
''' + method + r'''
}
fn main() {
    let p=Parameters{kernel_start:0x80000000,kernel_end:0x90000000,
        tilemap:1,tpc:2,heapmeta:3,ta_user_timestamp_start:0,ta_user_timestamp_end:0,
        fragment_user_timestamp_start:0,fragment_user_timestamp_end:0};
    let incoming=Client{id:1,root:Root};
    let mut state=State{owners:vec![Owner{client:Some(Client{id:2,root:Root}),
        directory:p.kernel_start,private_limit:p.kernel_end,render:Some(Resource{needs_geometry:false})}],
        queues:vec![Queue{owner:0}],render_ordinal:184,forbid_serialization:true,serialized:Cell::new(0)};
    // Different caller needs its root plan before any descriptor/scratch seed.
    assert!(state.render_preparation_seed(&Session,&p,&incoming).unwrap().is_none());
    state.owners[0].client.as_mut().unwrap().id=1;
    // Matching caller still needs the private window moved first.
    state.owners[0].directory+=0x4000;
    assert!(state.render_preparation_seed(&Session,&p,&incoming).unwrap().is_none());
    state.owners[0].directory=p.kernel_start;
    state.owners[0].private_limit-=0x4000;
    assert!(state.render_preparation_seed(&Session,&p,&incoming).unwrap().is_none());
    state.owners[0].private_limit=p.kernel_end;
    // Incoming geometry may exceed the installed pool's old scratch capacity.
    state.owners[0].render.as_mut().unwrap().needs_geometry=true;
    assert!(state.render_preparation_seed(&Session,&p,&incoming).unwrap().is_none());
    assert_eq!(state.serialized.get(),0);
    state.owners[0].render.as_mut().unwrap().needs_geometry=false;
    state.forbid_serialization=false;
    assert!(state.render_preparation_seed(&Session,&p,&incoming).unwrap().is_some());
    assert_eq!(state.serialized.get(),1);
    println!("PASS actual descriptor seed waits for caller, private-window and geometry prerequisites; ready path serializes once");
}
'''
with tempfile.TemporaryDirectory(prefix='render-seed-readiness-') as tmp:
    tmp = Path(tmp)
    (tmp / 'main.rs').write_text(code)
    subprocess.run(['rustc', '--edition=2021', str(tmp / 'main.rs'), '-o', str(tmp / 'gate')], check=True)
    output = subprocess.check_output([str(tmp / 'gate')], text=True)
    a.evidence.mkdir(parents=True, exist_ok=False)
    (a.evidence / 'harness.rs').write_text(code)
    (a.evidence / 'summary.json').write_text(json.dumps(dict(pass_=True,
        source_sha256=hashlib.sha256(s.encode()).hexdigest(), output=output), indent=2)+'\n')
    print(output, end='')
