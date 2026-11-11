#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare every ordered Max engine-register transaction with qualified source."""
import argparse,hashlib,json,struct,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1',type=Path);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
source=Path(__file__).resolve().parent.parent/'scheduled_execution_g16c.rs'
sys.path.insert(0,str(a.m1n1/'proxyclient'))
from m1n1.g16c.execution import ExecutionSetup,RenderExecutionSetup
expected=[];cases=[]
for i in range(64):
 ta=0x7406000000+i*0x4000;fr=0x7407000000+i*0x4000
 for event in range(4):
  cases.append(f'emit(&[wire::BindingG16C::compute({event},{ta}).unwrap()]);')
  expected.extend(ExecutionSetup(ta,independent_cdm=event==2,second_compute_engine=event==1,fourth_compute_engine=event==3).transactions())
 for first in (0,1,2):
  cases.append(f'emit(&wire::BindingG16C::render({first},{ta},{fr}).unwrap());')
  expected.extend(RenderExecutionSetup(ta,fr,tiling_engine=1<<first,fragment_engine=2<<first).transactions())
code='''#![allow(dead_code)]
use std::io::Write;
'''+f'#[path={json.dumps(str(source))}]mod wire;\n'+'''
fn emit(bindings:&[wire::BindingG16C]) {for b in bindings {for(offset,width,value)in b.transactions(){
 let w=std::io::stdout();let mut w=w.lock();w.write_all(&(offset as u32).to_le_bytes()).unwrap();
 w.write_all(&[width]).unwrap();w.write_all(&value.to_le_bytes()).unwrap();}}}
fn main(){
'''+''.join(cases)+'''
for event in [4,255]{assert!(wire::BindingG16C::compute(event,0x7406000000).is_none());}
for event in [3,255]{assert!(wire::BindingG16C::render(event,0x7406000000,0x7407000000).is_none());}
for address in [0,1,0x3fff,0x4001,1<<42]{
 assert!(wire::BindingG16C::compute(0,address).is_none());
 assert!(wire::BindingG16C::render(0,address,0x7407000000).is_none());
 assert!(wire::BindingG16C::render(0,0x7406000000,address).is_none());}
assert!(wire::BindingG16C::render(0,0x7406000000,0x7406000000).is_none());}
'''
with tempfile.TemporaryDirectory(prefix='g16c-execution-')as t:
 t=Path(t);(t/'gate.rs').write_text(code)
 subprocess.run(['rustc','--edition=2021','-Dwarnings',str(t/'gate.rs'),'-o',str(t/'gate')],check=True)
 actual=subprocess.check_output([str(t/'gate')])
want=b''.join(struct.pack('<IBQ',offset,width,value)for offset,width,value in expected)
assert actual==want
summary=dict(complete_ordered_plans=len(cases),transactions=len(expected),wrong_binding_rejections=20,
             compute_first_and_render_first=True,fragment_before_tiling=True,GPU_execution=False,
             source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),whole_sha256=hashlib.sha256(actual).hexdigest())
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print('PASS:',len(cases),'whole source transaction plans;',len(expected),'register writes, 20 rejected bindings')
