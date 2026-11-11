#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Inspect only our compiled Linux driver: startup stores and major stack frames."""
import argparse
import json
from pathlib import Path
import subprocess

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--ssh',required=True)
parser.add_argument('--object',required=True)
parser.add_argument('--llvm',default='/opt/llvm-21/bin/')
parser.add_argument('--evidence',type=Path,required=True)
args=parser.parse_args()
assert Path(args.object).name=='asahi_neo.o','This tool is restricted to our Linux driver object'
program='''
from pathlib import Path
import subprocess,re,json,hashlib,struct
obj=OBJECT;llvm=LLVM
image=Path(obj).read_bytes();assert image[:4]==b'\\x7fELF' and struct.unpack_from('<H',image,18)[0]==183
raw=subprocess.check_output([llvm+'llvm-nm','--defined-only',obj],text=True)
dem=subprocess.check_output([llvm+'llvm-nm','--defined-only','--demangle',obj],text=True)
result=[]
for line in dem.splitlines():
    if line.split()[1].lower()!='t':continue
    needles=['ImageG16C>::new','Session>::unstarted','Session>::prepare_g16c::<',
             'ScheduledGpu as kernel::platform::Driver>::probe','ScheduledGpu>>::probe_callback',
             'ScheduledGpu>::probe_g16c','ScheduledGpu>::probe_g17p']
    if not any(n in line for n in needles):continue
    address=line.split()[0]
    symbol=next(v.split()[-1] for v in raw.splitlines() if v.split()[0]==address and v.split()[1].lower()=='t')
    data=subprocess.check_output([llvm+'llvm-objdump','--disassemble-symbols='+symbol,obj],text=True)
    ins=[v for v in data.splitlines() if re.match(r'\\s*[0-9a-f]+:',v)]
    frame=0;prefix=[];release=[]
    for v in ins[:80]:
        prefix.append(v.strip())
        if re.search(r'\\b(bl|cbz|cbnz|tbnz|tbz|b\\.)\\b',v):break
        m=re.search(r'\\bsub\\s+sp, sp, #0x([0-9a-f]+)(, lsl #12)?',v)
        if m:frame+=int(m[1],16)*(4096 if m[2] else 1)
        m=re.search(r'\\[sp, #-0x([0-9a-f]+)\\]!',v)
        if m:frame+=int(m[1],16)
    for i in range(len(ins)-2):
        if re.search(r'\\bstr\\s+w(?!zr)\\w*,',ins[i]) and re.search(r'\\bstr\\s+wzr,',ins[i+1]) and re.search(r'\\bdsb\\s+sy',ins[i+2]):
            release.append([v.strip() for v in ins[i:i+3]])
    result.append({'function':line,'stack_frame_bytes':frame,'prologue':prefix,'RUN_gate_sequences':release})
print(json.dumps({'scope':'Own compiled Linux driver only; no Apple binary inspection',
                  'object_sha256':hashlib.sha256(image).hexdigest(),'functions':result}))
'''.replace('OBJECT',repr(args.object)).replace('LLVM',repr(args.llvm))
r=subprocess.run(['ssh','-o','BatchMode=yes','-o','ConnectTimeout=10',args.ssh,'python3 -'],input=program,text=True,capture_output=True,check=True)
receipt=json.loads(r.stdout)
startup=[f for f in receipt['functions'] if 'Session>::prepare_g16c::<' in f['function'] or 'ScheduledGpu>::probe_g16c' in f['function']]
assert sum(len(f['RUN_gate_sequences']) for f in startup)==1,'Missing or ambiguous atomic RUN/gate release'
frames={f['function']:f['stack_frame_bytes'] for f in receipt['functions']}
assert len(frames)>=5,frames
receipt['native_execution_tested']=False
major_max=[f for f in receipt['functions'] if any(n in f['function'] for n in
    ['probe_callback','ScheduledGpu as kernel::platform::Driver>::probe','ScheduledGpu>::probe_g16c','ImageG16C>::new'])]
receipt['Max_major_constructor_chain_bytes']=sum(f['stack_frame_bytes'] for f in major_max)
receipt['nested_callee_stack_not_included']=True
args.evidence.write_text(json.dumps(receipt,indent=2)+'\n')
for name,size in frames.items():print(f'{size:5d} bytes: {name.split(" t ")[-1].split(" T ")[-1]}')
print('PASS: adjacent ASC RUN / gate release / DSB in our ARM64 driver')
