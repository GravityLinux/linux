#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare actual old/new retained-pool selection, including duplicate owners."""
import argparse,subprocess,json,hashlib
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args();r=Path(__file__).resolve().parents[1]
path='drivers/gpu/drm/asahi/scheduled_render_submit.rs';new=(r/'scheduled_render_submit.rs').read_text();old=subprocess.check_output(['git','show','dad8de9ac37d:'+path],text=True)
def item(s,marker):
 i=s.index(marker);j=s.index('{',i)+1;d=1
 while d:d+=(s[j]=='{')-(s[j]=='}');j+=1
 return s[i:j]
def method(s,name):
 return item(s,'    pub(crate) fn render_pool(').replace('fn render_pool(',f'fn {name}(').replace('super::super::super::g17p_render_lifecycle','life').replace('super::super::super::g17p_render_runtime','runtime').replace('use life as life;','').replace('compute::Client','Client')
l=(r/'g17p_render_lifecycle.rs').read_text()
code=r'''
#![allow(dead_code)]
use std::cell::Cell;
thread_local!{static CALLS:Cell<usize>=const{Cell::new(0)};static LIMIT:Cell<usize>=const{Cell::new(33)};}
mod life{CONSTANT ASID SELECT}
mod runtime{pub fn pool_count()->usize{super::LIMIT.with(std::cell::Cell::get)}}
mod command{pub struct Parameters{pub kernel_start:u64,pub kernel_end:u64}}
struct Client{owner:(u64,u32),mapping:u64}fn same(a:&Client,b:&Client)->bool{a.owner==b.owner&&a.mapping==b.mapping}
struct Render(bool);impl Render{fn admitted(&self)->bool{self.0}}
struct Owner{pool:u32,client:Option<Client>,directory:u64,private_next:u64,render:Option<Render>}
struct Generation(bool);impl Generation{fn idle(&self)->bool{self.0}}
struct Queue{owner:usize,generation:Generation}
struct Compute(u64);impl Compute{fn asid_mask(&self)->u64{self.0}}
struct Session{available:[bool;33],independent_compute:Compute,automatic_render_parallel:bool,render_cache_hits:[u8;33],render_cache_cost:[u16;33],render_cache_used:[u64;33]}
impl Session{fn render_pool_availability(&self)->[bool;33]{self.available}}
fn visit(_:&&Owner){CALLS.with(|v|v.set(v.get()+1));}
struct Owners(Vec<Owner>);impl Owners{fn iter(&self)->std::iter::Inspect<std::slice::Iter<'_,Owner>,fn(&&Owner)>{self.0.iter().inspect(visit as fn(&&Owner))}fn get(&self,i:usize)->Option<&Owner>{self.0.get(i)}fn len(&self)->usize{self.0.len()}fn push(&mut self,o:Owner){self.0.push(o)}}
impl std::ops::Deref for Owners{type Target=[Owner];fn deref(&self)->&Self::Target{&self.0}}
impl<'a>IntoIterator for &'a Owners{type Item=&'a Owner;type IntoIter=std::iter::Inspect<std::slice::Iter<'a,Owner>,fn(&&Owner)>;fn into_iter(self)->Self::IntoIter{self.iter()}}
impl FromIterator<Owner> for Owners{fn from_iter<T:IntoIterator<Item=Owner>>(i:T)->Self{Self(i.into_iter().collect())}}
struct State{owners:Owners,queues:Vec<Queue>,render_ordinal:u32}
impl State{OLD NEW}
fn rng(s:&mut u64)->u64{*s^=*s<<13;*s^=*s>>7;*s^=*s<<17;*s}
fn main(){let mut seed=0x60418140u64;let mut cases=0;
 for n in 0..30000{
  let mut r=||rng(&mut seed);
  let incoming=Client{owner:(r()%4,r()as u32%3),mapping:r()%8};
  let start=if n%7==0{u64::MAX-31}else{r()%0x10000};let p=command::Parameters{kernel_start:start,kernel_end:start.saturating_add(r()%0x10000)};
  let mut session=Session{available:[true;33],independent_compute:Compute(if n%11==0{u64::MAX}else{r()}),automatic_render_parallel:n%2==0,render_cache_hits:[0;33],render_cache_cost:[0;33],render_cache_used:[0;33]};
  for i in 0..33{session.available[i]=r()%5!=0;session.render_cache_hits[i]=r()as u8;session.render_cache_cost[i]=r()as u16;session.render_cache_used[i]=r()%10000;}
  LIMIT.with(|v|v.set([1,2,7,15,33][n%5]));
  let mut state=State{owners:Owners(vec![]),queues:vec![],render_ordinal:r()as u32};
  let count=r()%70;
  for _ in 0..count{let pool=(r()%36)as u32;let directory=r()%0x10000;let next=if r()%7==0{directory.saturating_sub(1)}else{directory+r()%0x10000};let client=if r()%5==0{None}else{Some(Client{owner:(r()%4,r()as u32%3),mapping:r()%8})};state.owners.push(Owner{pool,client,directory,private_next:next,render:if r()%5==0{None}else{Some(Render(r()%3!=0))}});}
  for _ in 0..r()%90{state.queues.push(Queue{owner:(r()%(count+4))as usize,generation:Generation(r()%5!=0)});}
  assert_eq!(state.new(&session,&incoming,&p),state.old(&session,&incoming,&p),"case {n}");cases+=1;
 }
 // A fully resident set with a late exact match used to compare every owner
 // for every candidate. Preserve its answer while making membership linear.
 let s=Session{available:[true;33],independent_compute:Compute(0),automatic_render_parallel:true,render_cache_hits:[0;33],render_cache_cost:[1;33],render_cache_used:[0;33]};
 let state=State{owners:(0..33).map(|pool|Owner{pool,client:Some(Client{owner:(pool as u64,0),mapping:pool as u64}),directory:0,private_next:4096,render:Some(Render(true))}).collect(),queues:vec![],render_ordinal:0};let incoming=Client{owner:(32,0),mapping:32};let p=command::Parameters{kernel_start:0,kernel_end:8192};
 CALLS.with(|v|v.set(0));assert_eq!(state.old(&s,&incoming,&p),Some(32));let before=CALLS.with(Cell::get);CALLS.with(|v|v.set(0));assert_eq!(state.new(&s,&incoming,&p),Some(32));let after=CALLS.with(Cell::get);assert_eq!(after,33);assert!(before>after*20);
 println!("PASS {cases} actual old/new selections: duplicate/dormant owners, busy/invalid queues, address-space pressure, lease availability, private-window overflow, cache policy; resident owner examinations {before} -> {after}");}
'''.replace('CONSTANT',next(x for x in l.splitlines()if x.startswith('pub(crate) const POOL_SLOTS:'))).replace('ASID',item(l,'pub(crate) fn render_asid(')).replace('SELECT',item(l,'pub(crate) fn select_affine_pool(')).replace('OLD',method(old,'old')).replace('NEW',method(new,'new'))
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code);subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True);out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);(a.evidence/'source.json').write_text(json.dumps({'scheduled_render_submit_sha256':hashlib.sha256(new.encode()).hexdigest(),'comparison_base':'dad8de9ac37d'},indent=2)+'\n');print(out,end='')
