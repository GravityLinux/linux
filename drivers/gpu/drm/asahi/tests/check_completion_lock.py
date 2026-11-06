#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exercise the actual retirement/publication guard transition and cache errors."""
import argparse
import subprocess
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence', type=Path, required=True)
a = p.parse_args()
s = (Path(__file__).resolve().parents[1] / 'g17p_schedule.rs').read_text()
start = s.index('        drop(interval);', s.index('// CPU cache completion'))
end = s.index('        let runtime =', start)
transition = s[start:end]
code = r'''
#![allow(dead_code)]
use std::{cell::Cell,rc::Rc};
struct State{held:Cell<bool>,locks:Cell<usize>,cached:Cell<usize>}
struct Lock(Rc<State>);struct Guard(Rc<State>);
impl Lock{fn lock(&self)->Guard{assert!(!self.0.held.replace(true));
 self.0.locks.set(self.0.locks.get()+1);Guard(self.0.clone())}}
impl Drop for Guard{fn drop(&mut self){assert!(self.0.held.replace(false));}}
struct Device{runtime:Lock}
struct Completion{state:Rc<State>,error:Option<i32>}
impl Completion{fn cache(&mut self)->Result<(),i32>{
 assert!(!self.state.held.get(),"cache completion must run off-lock");
 self.state.cached.set(self.state.cached.get()+1);
 self.error.map_or(Ok(()),Err)
}}
struct RuntimeInterval;impl RuntimeInterval{
 fn waiting(){}fn acquired(_:(),_:(u64,u32),_:&str)->Self{Self}
}
struct Prepared{queue_key:(u64,u32)}
fn main(){for count in [0,1,4]{for failure in [false,true]{
 let state=Rc::new(State{held:Cell::new(false),locks:Cell::new(0),cached:Cell::new(0)});
 let dev=Device{runtime:Lock(state.clone())};let prepared=Prepared{queue_key:(1,2)};
 let mut completions:Vec<_>=(0..count).map(|i|Completion{state:state.clone(),
  error:if failure{Some(42+i)}else{None}}).collect();
 let held=dev.runtime.lock();let interval=RuntimeInterval;
 TRANSITION
 assert!(state.held.get(),"publication requires the runtime guard");
 assert_eq!(state.locks.get(),if count==0{1}else{2});
 assert_eq!(state.cached.get(),count as usize);
 assert_eq!(cache_error,if failure&&count>0{Some(42)}else{None});
 drop(held);assert!(!state.held.get());
 }}println!("PASS actual completion guard transition: empty passes retain one acquisition; all cache completions run unlocked; first cache error survives; publication reacquires locking");}
'''.replace('TRANSITION', transition)
a.evidence.mkdir(parents=True, exist_ok=False)
(a.evidence / 'test.rs').write_text(code)
subprocess.run(['rustc', '--edition=2021', '-O', str(a.evidence / 'test.rs'),
                '-o', str(a.evidence / 'gate')], check=True)
result = subprocess.check_output([str(a.evidence / 'gate')], text=True)
(a.evidence / 'output.txt').write_text(result)
print(result, end='')
