#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute actual scheduler handoff gates for shared and independent render roots."""
import argparse,json,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
root=Path(__file__).resolve().parents[1];s=(root/'g17p_schedule.rs').read_text()
def item(marker):
 start=s.index(marker);end=s.index('{',start)+1;depth=1
 while depth:depth+=(s[end]=='{')-(s[end]=='}');end+=1
 return s[start:end]
context=(root/'g17p_render_context_session.rs').read_text()
a1=context.index('    pub(crate) fn render_handoff_required(');b1=context.index('{',a1)+1;depth=1
while depth:depth+=(context[b1]=='{')-(context[b1]=='}');b1+=1
policy=context[a1:b1].replace('pub(crate) ','').replace('compute::Client','Client').replace('->Result<bool>','->Result<bool,i32>')
assert s.count('runtime.session.render_handoff_required(snapshot.client(),&parameters)?')==2
code=r'''
use std::{cell::Cell,sync::{Arc,atomic::{AtomicI32,Ordering}}};
#[derive(Clone)]struct Fence(Arc<AtomicI32>);impl Fence{fn raw(&self)->*const AtomicI32{Arc::as_ptr(&self.0)}}
fn status(f:&Fence)->i32{f.0.load(Ordering::Acquire)}
struct Point{publication:Fence}struct Node{point:Point}
struct Client;struct Parameters;
struct Owner(bool);impl Owner{fn render_needs_private(&self,_:&Session,_:&Client,_:&Parameters)->bool{self.0}}
struct Session{concurrent:bool,template:bool,render:Option<()>,g16c_compute:Option<Owner>}
impl Session{
 fn render_pipelining_enabled(&self)->bool{self.concurrent}
 fn render_template_size(&self,_:&Client,_:&Parameters)->Result<Option<usize>,i32>{Ok(self.template.then_some(1))}
}
struct Runtime{session:Session,engine_handoff:[Option<Fence>;2]}
struct Events(Cell<usize>);impl Events{fn prefer_queue(&self,_:usize,_:(u64,u32)){self.0.set(self.0.get()+1)}}
struct Device{events:Events}struct Schedule;
impl Schedule{
'''+item('    fn preferred(')+'\n'+item('    fn handoff(').replace('super::super::g17p_compute_queues::Key','(u64,u32)')+r'''
}
fn node()->Node{Node{point:Point{publication:Fence(Arc::new(AtomicI32::new(0)))}}}
impl Session{POLICY}
fn main(){
 let a=node();let b=node();let dev=Device{events:Events(Cell::new(0))};
 for concurrent in [false,true]{for private in [false,true]{for template in [false,true]{for max in [false,true]{
  let mut rt=Runtime{session:Session{concurrent,template,render:(!private).then_some(()),g16c_compute:max.then_some(Owner(private))},engine_handoff:[None,None]};
  let cold=rt.session.render_handoff_required(&Client,&Parameters).unwrap();assert_eq!(cold,!concurrent||private||template);
  Schedule::handoff(&dev,&mut rt,0,&a,cold,(1,0));
  assert_eq!(rt.engine_handoff[0].is_some(),cold);
  assert!(Schedule::preferred(&mut rt,0,&a));assert_eq!(Schedule::preferred(&mut rt,0,&b),!cold);
  Schedule::handoff(&dev,&mut rt,1,&a,true,(1,0));
  assert!(!Schedule::preferred(&mut rt,1,&b));assert!(Schedule::preferred(&mut rt,1,&a));
  for result in [1,-5]{a.point.publication.0.store(result,Ordering::Release);
   assert!(Schedule::preferred(&mut rt,0,&b));assert!(Schedule::preferred(&mut rt,1,&b));
  }
  a.point.publication.0.store(0,Ordering::Release);
 }}}}

 println!("PASS actual scheduler gates: warm independent renders do not claim global handoff; cold owner/template construction, serialized render and compute retain admission and success/error release");
}
'''
code=code.replace('POLICY',policy)
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out);print(out,end='')
