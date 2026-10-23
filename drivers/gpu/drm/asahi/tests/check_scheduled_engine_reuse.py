#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Compare full initialization and resumed writes to native reuse MMIO DATA."""
import argparse,hashlib,json,struct,subprocess,tempfile
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('capture',type=Path);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
root=Path(__file__).resolve().parent.parent
native=json.loads((a.capture/'native-verification.json').read_text());assert native['pass']and native['output_changed']and native['output_expected']
history=json.loads((a.capture/'startup-history.json').read_text());assert history['single_user']and not history['truncated'];base=history['aperture'][0]
plan=json.loads((a.capture/'uat/replay-plan.json').read_text());assert plan['two_compute_queues']and plan['both_compute_producers_held']
initial=[w for w in history['writes']if not w['before_init']and not w['capture_armed']and w['address']-base in (0xd06030,0x4090,0x4098,0x21068,0x21070)]
resume=[w for w in history['writes']if not w['before_init']and w['capture_armed']and w['address']-base in (0xd06030,0x4090,0x4098,0x21068,0x21070)]
assert len(initial)==24 and len(resume)==8
source=root/'scheduled_execution_g16c.rs'
code='#![allow(dead_code)]\nuse std::io::Write;\n'+f'#[path={json.dumps(str(source))}]mod wire;\n'
code+='fn emit(t:&[(usize,u8,u64)]){for &(o,w,v)in t{let mut s=std::io::stdout().lock();s.write_all(&(o as u32).to_le_bytes()).unwrap();s.write_all(&[w]).unwrap();s.write_all(&v.to_le_bytes()).unwrap();}}\nfn main(){\n'
for resumed in (False,True):
 for event,held in enumerate(plan['submissions']):
  method='resume_transactions'if resumed else'transactions'
  code+=f'emit(&wire::BindingG16C::compute({event},{held["scratch_low"]}).unwrap().{method}());\n'
code+='}\n'
with tempfile.TemporaryDirectory(prefix='engine-reuse-')as t:
 t=Path(t);(t/'main.rs').write_text(code)
 subprocess.run(['rustc','--edition=2021','-Dwarnings',str(t/'main.rs'),'-o',str(t/'test')],check=True)
 actual=subprocess.check_output([str(t/'test')])
expected=b''.join(struct.pack('<IBQ',w['address']-base,w['width'],w['value'])for w in initial+resume)
assert actual==expected
for w in resume:assert w['address']-base not in (0x21068,0x21070)
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'transactions.bin').write_bytes(actual)
(a.evidence/'summary.json').write_text(json.dumps(dict(initial_binding_writes=24,owned_resume_writes=8,complete_native_MMIO_trace_matches=True,resume_does_not_clear_or_rebind_scratch=True,source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),transactions_sha256=hashlib.sha256(actual).hexdigest(),GPU_execution=False),indent=2)+'\n')
print('PASS24 complete native initial writes and8 resumed writes, no scratch rebind/clear on resume')
