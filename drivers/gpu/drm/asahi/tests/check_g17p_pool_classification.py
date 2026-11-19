#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Differential Neo pool choice after replacing repeated cached-caller scans."""
import argparse,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args();r=Path(__file__).resolve().parents[1]
def item(s,marker):
 i=s.index(marker);j=s.index('{',i)+1;d=1
 while d:d+=(s[j]=='{')-(s[j]=='}');j+=1
 return s[i:j]
s=(r/'g17p_boot.rs').read_text();old=subprocess.check_output(['git','show','dad8de9ac37d:drivers/gpu/drm/asahi/g17p_boot.rs'],text=True);l=(r/'g17p_render_lifecycle.rs').read_text()
def method(s,name):return item(s,'    fn next_render_pool_for(&self').replace('fn next_render_pool_for(',f'fn {name}(').replace('super::g17p_render_lifecycle','life').replace('compute::Client','Client')
code=r'''
#![allow(dead_code,non_upper_case_globals)]
use std::cell::Cell;type Result<T>=std::result::Result<T,i32>;const EINVAL:i32=22;const EOVERFLOW:i32=75;
thread_local!{static SERIAL:Cell<bool>=const{Cell::new(false)};static ALT:Cell<u32>=const{Cell::new(0)};}
mod life{CONSTANT SELECT}
mod render{pub fn serialized_cache_enabled()->bool{super::SERIAL.with(std::cell::Cell::get)}}
mod module_parameters{pub struct Parameter;impl Parameter{pub fn value(&self)->u32{super::ALT.with(std::cell::Cell::get)}}pub static alternate_queue_pairs:Parameter=Parameter;}
struct Client{owner:(u64,u32),bindings:u64}struct Layout{independent:bool,native:bool,pair:u32}
struct Growth([bool;33]);impl Growth{fn caller_compatible(&self,p:u32,_:&u64)->bool{self.0[p as usize]}}
struct Work{ordinal:u32,layout:Layout,client:Client,count:u32,growth:Option<Growth>,queue_owners:[bool;33]}
impl Work{fn pool_count(&self)->u32{self.count}fn pair_queue_owner(&self,p:u32)->Option<()>{self.queue_owners[p as usize].then_some(())}}
struct Compute(u64);impl Compute{fn asid_mask(&self)->u64{self.0}}
struct Session{render:Option<Work>,render_pool_clients:Vec<(u32,Client)>,render_pool_asids:[u16;33],independent:bool,independent_compute:Compute,availability:[bool;33],render_cache_hits:[u8;33],render_cache_cost:[u16;33],render_cache_used:[u64;33]}
impl Session{fn independent_render_roots(&self)->bool{self.independent}fn render_pool_availability(&self)->[bool;33]{self.availability}fn same_client(a:&Client,b:&Client)->bool{a.owner==b.owner&&a.bindings==b.bindings}OLD NEW}
fn rng(s:&mut u64)->u64{*s^=*s<<13;*s^=*s>>7;*s^=*s<<17;*s}
fn main(){let mut seed=0x81406041;let mut cases=0;for n in 0..30000{let mut r=||rng(&mut seed);let c=Client{owner:(r()%4,r()as u32%3),bindings:r()%8};
 SERIAL.with(|v|v.set(r()%2==0));ALT.with(|v|v.set((r()%2)as u32));
 let mut s=Session{render:None,render_pool_clients:vec![],render_pool_asids:[0;33],independent:r()%2==0,independent_compute:Compute(if r()%3==0{u64::MAX}else{r()}),availability:[true;33],render_cache_hits:[0;33],render_cache_cost:[0;33],render_cache_used:[0;33]};
 let mut compat=[true;33];let mut owners=[true;33];for i in 0..33{s.availability[i]=r()%4!=0;s.render_pool_asids[i]=(r()%3)as u16;s.render_cache_hits[i]=r()as u8;s.render_cache_cost[i]=r()as u16;s.render_cache_used[i]=r()%10000;compat[i]=r()%5!=0;owners[i]=r()%3!=0;}
 for _ in 0..r()%70{s.render_pool_clients.push(((r()%36)as u32,Client{owner:(r()%4,r()as u32%3),bindings:r()%8}));}
 if n%13!=0{s.render=Some(Work{ordinal:if n%17==0{u32::MAX}else{r()as u32},layout:Layout{independent:r()%2==0,native:r()%2==0,pair:(r()%36)as u32},client:Client{owner:(r()%4,r()as u32%3),bindings:r()%8},count:(r()%34)as u32,growth:if r()%3==0{None}else{Some(Growth(compat))},queue_owners:owners});}
 for incoming in [None,Some(&c)]{assert_eq!(s.new(incoming),s.old(incoming),"case {n}");cases+=1;}
 }println!("PASS {cases} production Neo pool selections: active/cached/duplicate callers, related affinity, geometry compatibility, lease/ASID pressure, legacy/native modes and overflow");}
'''.replace('CONSTANT',next(x for x in l.splitlines()if x.startswith('pub(crate) const POOL_SLOTS:'))).replace('SELECT',item(l,'pub(crate) fn select_affine_pool(')).replace('OLD',method(old,'old')).replace('NEW',method(s,'new'))
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code);subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True);out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);print(out,end='')
