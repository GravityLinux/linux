#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute the actual shared admission-view/rematerialization block.

Model selection changing while off-lock: a borrowed read-only view must become
an owned tree before constructing a new private execution owner. Page-table
algorithms themselves are exercised by check_g17p_leaf_switch.py.
"""
import argparse
from pathlib import Path
import subprocess, tempfile
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence',type=Path,required=True)
p.add_argument('--source',type=Path)
a=p.parse_args()
s=(a.source or Path(__file__).resolve().parents[1]/'g17p_schedule.rs').read_text()
start=s.index('                let (old, private) = match command {',s.index('    fn prepare_clients('))
prior=s.rfind('                let runtime = ',0,start)
gate=s[s.index('\n',prior)+1:start]
start=s.index('            let slot = if engine == 0')
end=s.index('            if needed {',start)
block=s[start:end].replace('super::super::g17p_render_runtime::scratch_table_budget(p)?','scratch_table_budget(p)?')
start=s.index('    pub(super) fn fast_retry(')
b=s.index('{',start);end=b+1;depth=1
while depth:depth+=(s[end]=='{')-(s[end]=='}');end+=1
retry=s[start:end].replace('pub(super)', 'pub')
start=s.index('    pub(super) fn fast_retry_us(')
b=s.index('{',start);end=b+1;depth=1
while depth:depth+=(s[end]=='{')-(s[end]=='}');end+=1
retry_us=s[start:end].replace('pub(super)', 'pub')
code=r'''#![allow(dead_code,non_upper_case_globals,unused_variables)]
type Result<T=()>=std::result::Result<T,i32>;const EIO:i32=5;const EOVERFLOW:i32=75;
#[derive(Default)]struct Root{borrowed:bool,spares:usize}
impl Root{fn has_borrowed_tables(&self)->bool{self.borrowed}fn table_count(&self)->usize{3}fn prepare_spare_tables(&mut self,n:usize)->Result{assert!(!self.borrowed);self.spares=n;Ok(())}}
#[derive(Default)]struct Client{root:Root}
impl Client{fn cache(&self,_:bool)->Result{Ok(())}}
struct Snapshot(Client);
impl Snapshot{fn client(&self)->&Client{&self.0}fn deferred_client(&self)->Result<Client>{Ok(Client::default())}fn materialize_view(&self,c:&mut Client,_:bool,readonly:bool)->Result{c.root=Root{borrowed:readonly,spares:0};Ok(())}fn materialize(&self,c:&mut Client,render:bool)->Result{self.materialize_view(c,render,false)}}
struct Prepared{render_client:Option<Client>,compute_client:Option<Client>,queue_key:(u64,u32)}
#[derive(Clone,Copy)]struct Parameters{queue_owner:Option<(u64,u32)>}
enum Command{Render(Parameters),Compute(())}
fn scratch_table_budget(_:Parameters)->Result<usize>{Ok(7)}
fn prepare(prepared:&mut Prepared,engine:usize,needed:bool,readonly:bool)->Result {
 let snapshot=Snapshot(Client::default());let command=if engine==0{Command::Render(Parameters{queue_owner:None})}else{Command::Compute(())};
 BLOCK
 Ok(())
}
struct Node{staged:bool,point:Point}struct Point{completion:i32}
fn status(f:&i32)->i32{*f}
struct Schedule{control_waiting:bool,nodes:Vec<Node>}
struct Runtime{preferred:bool,session:Session}
struct Session{can_stage:bool,checks:std::cell::Cell<usize>}
impl Session{fn can_stage_render(&self,_:Option<&Client>,p:&Parameters)->Result<bool>{
 assert_eq!(p.queue_owner,Some((10,4)));self.checks.set(self.checks.get()+1);Ok(self.can_stage)
}}
impl Schedule{
 fn preferred(r:&mut Runtime,_:usize,_:&Node)->bool{r.preferred}
 fn preparation_cache_touches(runtime:&mut Runtime,command:&Command)->Result<usize>{
  let prepared=Prepared{render_client:None,compute_client:None,queue_key:(10,4)};
  let node=&Node{staged:false,point:Point{completion:0}};
  let snapshot=Snapshot(Client::default());let mut touches=0;
  for _ in 0..1000 { GATE touches+=1; }
  Ok(touches)
 }
 RETRY RETRY_US
}
fn main(){
 for (preferred,ready,render,expected) in [(true,false,true,0),(true,true,true,1000),(false,true,true,0),(false,false,true,0),(false,false,false,1000)]{
  let mut r=Runtime{preferred,session:Session{can_stage:ready,checks:std::cell::Cell::new(0)}};
  let c=if render{Command::Render(Parameters{queue_owner:None})}else{Command::Compute(())};
  assert_eq!(Schedule::preparation_cache_touches(&mut r,&c).unwrap(),expected,
   "blocked/unpreferred render touched retained CPU-map cache");
  assert_eq!(r.session.checks.get(),if render&&preferred{1000}else{0});
 }
 for engine in 0..2 {
  let mut p=Prepared{render_client:None,compute_client:None,queue_key:(10,4)};
  prepare(&mut p,engine,true,true).unwrap();
  let c=if engine==0{p.render_client.as_ref()}else{p.compute_client.as_ref()}.unwrap();
  assert!(c.root.borrowed);assert_eq!(c.root.spares,0);
  // The selected retained owner disappears during off-lock work. A later
  // pass needs a fresh owner/private heap rather than a retained rebind.
  prepare(&mut p,engine,true,false).unwrap();
  let c=if engine==0{p.render_client.as_ref()}else{p.compute_client.as_ref()}.unwrap();
  assert!(!c.root.borrowed);assert!(c.root.spares>3);
  prepare(&mut p,engine,true,true).unwrap();
  let c=if engine==0{p.render_client.as_ref()}else{p.compute_client.as_ref()}.unwrap();
  assert!(!c.root.borrowed,"never replace an already owned prepared tree with a borrowed view");
  let mut p=Prepared{render_client:None,compute_client:None,queue_key:(10,4)};
  prepare(&mut p,engine,false,false).unwrap();assert!(p.render_client.is_none()&&p.compute_client.is_none());
  prepare(&mut p,engine,true,false).unwrap();
  let c=if engine==0{p.render_client.as_ref()}else{p.compute_client.as_ref()}.unwrap();assert!(!c.root.borrowed);assert!(c.root.spares>3);
 }
 // Every pending schedule is eligible. Events elects only one queue head
 // to arm the shared-session polling timer; control retains its short retry.
 for (control,staged,completion,expected) in [
  (false,false,0,true),(false,true,0,true),
  (true,false,0,true),(true,true,1,true),
  (false,true,1,false),(false,false,-1,false)] {
  let s=Schedule{control_waiting:control,nodes:vec![Node{staged,point:Point{completion}}]};assert_eq!(s.fast_retry(),expected);assert_eq!(s.fast_retry_us(),if control{20}else{200});
 }
 println!("PASS actual shared admission view: both engines, borrowed-to-owned selection race, owned reuse, cold/unused paths, fast retry predicate and blocked-render CPU-cache isolation");
}
'''.replace('GATE',gate).replace('BLOCK',block).replace('RETRY_US',retry_us).replace('RETRY',retry)
a.evidence.mkdir(exist_ok=False,parents=True)
(a.evidence/'harness.rs').write_text(code)
subprocess.run(['rustc','--edition=2021',str(a.evidence/'harness.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True)
(a.evidence/'output.txt').write_text(out);print(out,end='')
