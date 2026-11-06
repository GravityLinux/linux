#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compile actual automatic-pool policy and frontend pipeline guard."""
import argparse,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
r=Path(__file__).resolve().parents[1]
def item(path,marker):
 s=(r/path).read_text();start=s.index(marker);end=s.index('{',start)+1;depth=1
 while depth:depth+=(s[end]=='{')-(s[end]=='}');end+=1
 return s[start:end]
code=r'''
#![allow(dead_code,non_upper_case_globals)]
use std::cell::Cell;
mod module_parameters{
 use super::*;thread_local!{pub static V:Cell<[u32;8]>=const{Cell::new([1,2,1,0,0,0,0,0])};}
 pub struct P(usize);impl P{pub fn value(&self)->u32{V.with(|v|v.get()[self.0])}}
 pub static parallel_render_auto:P=P(0);pub static render_cache_slots:P=P(1);pub static compute_queues:P=P(2);
 pub static native_compute_vms:P=P(3);pub static native_render_vms:P=P(4);pub static native_barriers:P=P(5);
 pub static partial_independent_owner:P=P(6);pub static alternate_queue_pairs:P=P(7);
}
'''+item('g17p_render_runtime.rs','pub(crate) fn auto_parallel_enabled(')+r'''
#[derive(Clone)]struct Parameters{vdm_barrier_fragment:bool}
#[derive(Clone)]enum Command{Render(Parameters),Compute}
struct Prepared{automatic_render_parallel:bool,parameters:Vec<Command>,barriers:Vec<[u16;2]>}
impl Prepared{
'''+item('g17p_drm.rs','    fn pipeline_render(')+r'''
}
fn main(){let mut checks=0;
 for chip in [0x6041,0x6040,0x8140,0x8132,0]{for auto in [0,1]{for slots in [0,1,2,15]{for queues in [0,1]{for flags in 0..32u32{
  let mut values=[auto,slots,queues,0,0,0,0,0];for i in 0..5{values[3+i]=(flags>>i)&1;}
  module_parameters::V.with(|v|v.set(values));
  let expected=chip==0x6041&&auto==1&&slots>1&&queues==1&&flags==0;
  assert_eq!(auto_parallel_enabled(chip),expected,"chip={chip:x} knobs={values:?}");checks+=1;
 }}}}}
 module_parameters::V.with(|v|v.set([1,2,1,0,0,0,0,0]));
 let mut p=Prepared{automatic_render_parallel:auto_parallel_enabled(0x6041),parameters:vec![Command::Render(Parameters{vdm_barrier_fragment:false})],barriers:vec![[u16::MAX;2]]};
 assert!(p.pipeline_render());p.automatic_render_parallel=false;assert!(!p.pipeline_render());
 module_parameters::V.with(|v|v.set([1,2,1,0,0,0,1,1]));assert!(p.pipeline_render(),"explicit legacy parallel control stopped working");
 p.parameters.push(Command::Compute);p.barriers.push([u16::MAX;2]);assert!(!p.pipeline_render());p.parameters.pop();p.barriers.pop();
 p.barriers[0]=[0,0];assert!(!p.pipeline_render());p.parameters[0]=Command::Render(Parameters{vdm_barrier_fragment:true});assert!(p.pipeline_render());
 module_parameters::V.with(|v|v.set([1,2,1,1,0,0,1,1]));assert!(!p.pipeline_render());
 println!("PASS {checks} actual automatic policy cases: Max normal boot enabled, Pro/Neo/other chips retain defaults, explicit/native/one-slot controls preserved; actual frontend command/dependency gate passes");
}
'''
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);print(out,end='')
