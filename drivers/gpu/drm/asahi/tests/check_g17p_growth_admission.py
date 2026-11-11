#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute the production Neo pool selector for a growing caller binding.

The fixture reproduces a render encoder expanding into a retained pool's
growth reservation. Both the selector and compatibility methods are compiled
from production source. Hardware retirement is represented by availability.
"""
from pathlib import Path
import argparse,hashlib,json,re,subprocess,tempfile
p=argparse.ArgumentParser();p.add_argument('--revision');p.add_argument('--expect-failure',action='store_true');p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
root=Path(__file__).resolve().parents[5];driver=root/'drivers/gpu/drm/asahi'
def source(name):
 return subprocess.check_output(['git','-C',str(root),'show',a.revision+':drivers/gpu/drm/asahi/'+name],text=True) if a.revision else (driver/name).read_text()
def item(s,marker):
 start=s.index(marker);b=s.index('{',start);end=b+1;depth=1
 while depth:depth+=(s[end]=='{')-(s[end]=='}');end+=1
 return s[start:end]
growth=source('g17p_growth.rs');service=source('g17p_growth_runtime.rs');boot=source('g17p_boot.rs');life=source('g17p_render_lifecycle.rs')
helper=item(growth,'pub(crate) fn caller_fits_growth(') if 'fn caller_fits_growth(' in growth else ''
method=item(service,'pub(crate) fn caller_compatible(') if 'fn caller_compatible(' in service else ''
code=r'''#![allow(dead_code,non_upper_case_globals)]
type Result<T>=std::result::Result<T,i32>;const EINVAL:i32=22;const EOVERFLOW:i32=75;
mod module_parameters {pub struct P;impl P{pub fn value(&self)->u32{1}}pub static alternate_queue_pairs:P=P;}
mod compute {#[derive(Clone,PartialEq,Eq)]pub struct Client{pub owner:(u64,u32),pub bindings:Vec<(u64,u64,u64,u32)>}}
mod g {CONSTANTS
HELPER}
struct Owner{pool:u32}struct Pool{identity:Owner,growth_base:u64,request_limit:u32}
struct Service{pools:Vec<Pool>}impl Service{METHOD}
struct Layout{independent:bool,native:bool,pair:u32}
struct Work{layout:Layout,ordinal:u32,client:compute::Client,growth:Option<Service>}
impl Work{fn pool_count(&self)->u32{3}}
struct Compute;impl Compute{fn asid_mask(&self)->u64{0}}
struct Session{render:Option<Work>,render_pool_asids:[u16;15],render_pool_clients:Vec<(u32,compute::Client)>,independent_compute:Compute,available:[bool;3]}
mod g17p_render_lifecycle {SELECT}
mod candidate{use super::*;impl Session{
 fn independent_render_roots(&self)->bool{true}
 fn render_pool_available(&self,p:u32)->bool{self.available[p as usize]}
 fn same_client(a:&compute::Client,b:&compute::Client)->bool{a==b}
 SELECTOR
}pub fn run(){
 let old=compute::Client{owner:(3,1),bindings:vec![]};
 let mut s=Session{render:Some(Work{layout:Layout{independent:true,native:false,pair:0},ordinal:0,client:old,growth:Some(Service{pools:vec![Pool{identity:Owner{pool:0},growth_base:0x1002000000,request_limit:32},Pool{identity:Owner{pool:1},growth_base:0x1008048000,request_limit:32},Pool{identity:Owner{pool:2},growth_base:0x100b248000,request_limit:32}]})}),render_pool_asids:[0;15],render_pool_clients:vec![],independent_compute:Compute,available:[true;3]};
 let mut incoming=compute::Client{owner:(15,1),bindings:vec![(0x1008000000,0x20000,0,6)]};
 assert_eq!(s.next_render_pool_for(Some(&incoming)).unwrap(),Some(1),"nonoverlapping original encoder must retain normal selection");
 incoming.bindings[0].1=0x4c000;
 assert_eq!(s.next_render_pool_for(Some(&incoming)).unwrap(),Some(2),"expanded encoder must skip its conflicting retained growth pool");
 s.available[2]=false;
 assert_eq!(s.next_render_pool_for(Some(&incoming)).unwrap(),Some(0),"another retired compatible owner stays admissible");
 s.available[0]=false;
 assert_eq!(s.next_render_pool_for(Some(&incoming)).unwrap(),None,"unavailable compatible owners defer without admitting a collision");
 s.available=[true;3];incoming.bindings[0]=(0x1008048000+32*10*0x28000,0x4000,0,6);
 assert_eq!(s.next_render_pool_for(Some(&incoming)).unwrap(),Some(1),"adjacent range is not an overlap");
 incoming.bindings[0]=(0x1008100000,0x4000,0,6);
 assert_eq!(s.next_render_pool_for(Some(&incoming)).unwrap(),Some(2),"the unused reservation must remain available for later firmware growth");
 incoming.bindings[0]=(0x8048000,0x4000,0,6);
 assert_eq!(s.next_render_pool_for(Some(&incoming)).unwrap(),Some(2),"render-context-relative binding uses the same address projection");
 DIRECT_TESTS
 println!("PASS: actual pool selector keeps original admission, skips colliding retained growth, finds another retired owner, defers exhausted owners, preserves adjacency and future growth, and projects relative bindings");
}}
fn main(){candidate::run();}
'''
direct=r'''
 assert!(g::caller_fits_growth(0x1008048000,32,&[]));
 assert!(!g::caller_fits_growth(0,32,&[]));
 assert!(!g::caller_fits_growth(0x1008048000,u32::MAX,&[]));
 assert!(!g::caller_fits_growth(0x1008048000,32,&[(0x1008000000,u64::MAX,0,6)]));
 assert!(!g::caller_fits_growth(0x1008048000,32,&[(0x1008000000,0,0,6)]));
 assert!(g::caller_fits_growth(0x1008048000,0,&[(0x1008048000,0x4000,0,6)]));
 // An existing compatible caller keeps the usual preference over rotation.
 incoming.bindings[0]=(0x1008000000,0x20000,0,6);
 s.render_pool_clients.push((2,incoming.clone()));
 assert_eq!(s.next_render_pool_for(Some(&incoming)).unwrap(),Some(2));
'''
code=code.replace('DIRECT_TESTS',direct if helper else '')
selector=item(boot,'fn next_render_pool_for(&self,')
constants='\n'.join(re.search(r'pub\(crate\) const '+name+r'[^;]+;',growth).group(0) for name in ('INCREMENT','CONTEXT_BASE'))
code=code.replace('CONSTANTS',constants).replace('HELPER',helper).replace('METHOD',method).replace('SELECT}',item(life,'pub(crate) fn select_pool(')+'}').replace(' SELECTOR',' '+selector)
with tempfile.TemporaryDirectory() as tmp:
 t=Path(tmp);(t/'test.rs').write_text(code);subprocess.run(['rustc','--edition=2021','-Dwarnings',str(t/'test.rs'),'-o',str(t/'test')],check=True)
 r=subprocess.run([str(t/'test')],capture_output=True,text=True)
 a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'summary.json').write_text(json.dumps({'revision':a.revision,'expect_baseline_failure':a.expect_failure,'returncode':r.returncode,'actual_pool_selector':True,'actual_growth_compatibility':bool(helper and method),'GPU_execution':False,'source_sha256':{name:hashlib.sha256(source(name).encode()).hexdigest() for name in ('g17p_growth.rs','g17p_growth_runtime.rs','g17p_boot.rs','g17p_render_lifecycle.rs')}},indent=2)+'\n');(a.evidence/'harness.rs').write_text(code);(a.evidence/'stdout.txt').write_text(r.stdout);(a.evidence/'stderr.txt').write_text(r.stderr)
 if a.expect_failure:assert r.returncode and 'expanded encoder must skip' in r.stderr;print('PASS: unchanged production selector rejects regression oracle')
 else:assert not r.returncode,r.stderr;print(r.stdout.strip())
