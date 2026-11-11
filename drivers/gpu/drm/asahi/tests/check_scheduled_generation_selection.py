#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exhaustively compare shared owner selection with original Neo ordering."""
from pathlib import Path
import argparse, hashlib, json, subprocess

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence',type=Path,required=True)
a=p.parse_args()
a.evidence.mkdir(parents=True,exist_ok=False)
root=Path(__file__).resolve().parent.parent
source=root/'scheduled_generation_select.rs'
code='''#![allow(dead_code)]
#[path=SOURCE]mod selection;
use selection::{select,Candidate};
fn candidate(index:usize,b:u8)->Candidate {Candidate {index,reserved:b&1!=0,
 eligible:b&2!=0,matches:b&4!=0,same_queue:b&8!=0,idle:b&16!=0,room:b&32!=0}}
fn neo(v:&[Candidate],fresh:Option<usize>)->Option<usize> {
 v.iter().position(|q|q.reserved)
 .or_else(||v.iter().position(|q|q.eligible&&q.matches&&q.room))
 .or_else(||v.iter().position(|q|q.eligible&&q.same_queue&&q.idle))
 .or_else(||v.iter().position(|q|q.eligible&&q.idle)).or(fresh)
}
fn main(){let mut checks=0;
 for x in 0..64 {for y in 0..64 {for z in 0..64 {
  let v=[candidate(0,x),candidate(1,y),candidate(2,z)];
  for fresh in [None,Some(3)] {assert_eq!(select(v.iter().copied(),fresh),neo(&v,fresh));checks+=1;}
 }}}
 assert_eq!(select(std::iter::empty(),None),None);
 assert_eq!(select(std::iter::empty(),Some(0)),Some(0));
 println!("{}",checks+2);
}
'''.replace('SOURCE',json.dumps(str(source)))
rs=a.evidence/'selection.rs';rs.write_text(code)
binary=a.evidence/'selection'
subprocess.run(['rustc','--edition=2021','-O','-Dwarnings',str(rs),'-o',str(binary)],check=True)
checks=int(subprocess.check_output([str(binary)],text=True));assert checks==524290
(a.evidence/'summary.json').write_text(json.dumps(dict(
 original_Neo_ordering_cases=checks,all_exact=True,
 source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
 GPU_execution=False),indent=2)+'\n')
print('PASS',checks,'owner-selection cases match original Neo; exact/reserved/retired/fresh ordering')
