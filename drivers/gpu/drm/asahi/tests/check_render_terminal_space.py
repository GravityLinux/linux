#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Run the real terminal-report handler with all retained render event pairs."""
import argparse,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args();r=Path(__file__).resolve().parents[1]
def item(s,m):
 i=s.index(m);j=s.index('{',i)+1;depth=1
 while depth:depth+=(s[j]=='{')-(s[j]=='}');j+=1
 return s[i:j]
s=(r/'g17p_growth_runtime.rs').read_text();handler=item(s,'        if opcode == 1 {');life=(r/'g17p_render_lifecycle.rs').read_text();constant=next(x for x in life.splitlines()if x.startswith('pub(crate) const POOL_SLOTS:'))
code=r'''
#![allow(dead_code,unused_variables)]
#[derive(Debug,Clone,Copy,PartialEq,Eq)]enum Error{Invalid,Io,Overflow}
type Result<T>=std::result::Result<T,Error>;const EIO:Error=Error::Io;const EOVERFLOW:Error=Error::Overflow;
CONSTANT
GRIDS
struct Memory;struct Vm;
#[derive(Debug,PartialEq,Eq)]enum Action{Consumed}
struct KVec<T>(Vec<T>);impl<T>KVec<T>{fn is_empty(&self)->bool{self.0.is_empty()}fn remove(&mut self,i:usize)->Result<T>{if i>=self.0.len(){Err(EIO)}else{Ok(self.0.remove(i))}}}
struct Pool{terminal_mask:u128,terminals:u32}
struct Service{pools:Vec<Pool>,independent_compute_mask:u128,independent_compute_terminals:[u32;128],compute_owners:KVec<()>,compute_terminals:u32,consumed:bool}
impl Service{
 fn consume(&mut self,_:&mut Memory,_:&Vm,_:u32)->Result<()>{assert!(!self.consumed);self.consumed=true;Ok(())}
 fn decode(&mut self,mut body:[u8;0x48],mixed:bool,compute_ordinal:Option<u32>)->Result<Action>{let opcode=1;let memory=&mut Memory;let vm=&Vm;let next=1;
 HANDLER
 Err(EIO)}
}
fn make()->Service{Service{pools:(0..POOL_SLOTS).map(|p|{let g=pool_grids(p).unwrap();Pool{terminal_mask:(1u128<<g[0])|(1u128<<g[1]),terminals:0}}).collect(),independent_compute_mask:((1u128<<60)-1)<<32,independent_compute_terminals:[0;128],compute_owners:KVec(vec![]),compute_terminals:0,consumed:false}}
fn body(flags:u128)->[u8;0x48]{let mut b=[0;0x48];b[..4].copy_from_slice(&1u32.to_le_bytes());b[4..20].copy_from_slice(&flags.to_le_bytes());b}
fn main(){
 let s=make();let mut used=0u128;
 for p in 0..POOL_SLOTS{let g=pool_grids(p).unwrap();assert!(g[1]<128);let bits=(1u128<<g[0])|(1u128<<g[1]);assert_eq!(used&bits,0);assert_eq!(s.independent_compute_mask&bits,0);used|=bits;
  if p<15{assert_eq!(g,[2*p+if p>=2{2}else{0},2*p+if p>=2{3}else{1}]);}
  let mut s=make();assert_eq!(s.decode(body(bits),false,None),Ok(Action::Consumed));assert!(s.consumed);assert_eq!(s.pools.iter().enumerate().filter(|(_,p)|p.terminals==1).map(|(i,_)|i).collect::<Vec<_>>(),vec![p as usize]);
  // Half a pair can never complete its render, or any other render.
  for bit in g{let mut s=make();assert_eq!(s.decode(body(1u128<<bit),false,None),Err(EIO));assert!(!s.consumed);assert!(s.pools.iter().all(|p|p.terminals==0));}
 }
 assert!(pool_grids(POOL_SLOTS).is_err());assert!(pool_grids(u32::MAX).is_err());
 let mut seed=87u64;
 for _ in 0..20000{seed^=seed<<13;seed^=seed>>7;seed^=seed<<17;let selected=seed&((1u64<<POOL_SLOTS)-1);let cs=((seed.rotate_left(17)&((1u64<<60)-1))as u128)<<32;
  let mut s=make();let flags=s.pools.iter().enumerate().fold(cs,|b,(i,p)|b|if selected&(1u64<<i)!=0{p.terminal_mask}else{0});
  if flags==0{continue;}
  assert_eq!(s.decode(body(flags),false,None),Ok(Action::Consumed));assert!(s.consumed);
  for (i,p)in s.pools.iter().enumerate(){assert_eq!(p.terminals,u32::from(selected&(1u64<<i)!=0));}
  for g in 32..92{assert_eq!(s.independent_compute_terminals[g],u32::from(cs&(1u128<<g)!=0));}
 }
 for mixed in [false,true]{let mut s=make();s.compute_owners=KVec(vec![()]);let result=s.decode(body(1<<4),mixed,None);assert_eq!(result.is_ok(),mixed);assert_eq!(s.compute_terminals,u32::from(mixed));}
 // A valid low render plus an unmatched high half-pair is rejected atomically.
 let mut s=make();let bits=s.pools[0].terminal_mask|(1u128<<127);assert_eq!(s.decode(body(bits),false,None),Err(EIO));assert!(s.pools.iter().all(|p|p.terminals==0));assert!(!s.consumed);
 println!("PASS all {POOL_SLOTS} render pairs, disjoint compute32..91, unchanged low identities, 20000 coalesced high/low/compute reports, unpaired/unknown bits, exact per-owner counts and legacy compute isolation");}
'''.replace('CONSTANT',constant).replace('GRIDS',item(life,'pub(crate) fn pool_grids(')).replace('HANDLER',handler)
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);print(out,end='')
