#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare complete Max compute FList pages with the qualified source builder."""
import argparse, hashlib, json, struct, subprocess, sys, tempfile
from pathlib import Path

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1',type=Path);p.add_argument('--evidence',type=Path,required=True)
a=p.parse_args();tests=Path(__file__).resolve().parent
sys.path.insert(0,str(a.m1n1/'proxyclient'))
from m1n1.g16c.pools import OperandPool, BUFFER_STRIDE

cases=[]
for i in range(32):
    base=(0x7000000000,0x100000000,0x30000000000,0x20000000000)[i%4]+i*0x4000
    table=base+0x1008000;aux=0xfffffc2207004000+i*0x8000
    n=(1,19,20,25,30,255,256,1024)[i%8]
    pool=OperandPool(base,table,tuple(base+0x10a0000+j*BUFFER_STRIDE for j in range(n)),0x10000)
    expected=bytearray(pool.support(aux,header=1,active=0,registered=n,resource_class=2))
    # ComputeOperands.allocate overrides these two source lifecycle scalars.
    struct.pack_into('<II',expected,0x5c,1,2)
    cases.append((base,table,aux,n,bytes(expected)))
with tempfile.TemporaryDirectory(prefix='scheduled-operands-') as directory:
    tmp=Path(directory);source=tests.parent/'scheduled_operands.rs'
    code='#![allow(dead_code)]\nuse std::io::Write;\n'
    code+=f'#[path={json.dumps(str(source))}] mod operands;\nfn main() {{\n'
    code+='let cases: &[(u64,u64,u64,u32)] = &['+','.join(f'({x},{y},{z},{n})'for x,y,z,n,_ in cases)+'];\n'
    code+='for &(d,t,a,n) in cases {let r=operands::ComputeSupportG16C::new(d,t,a,n).unwrap(); let mut page=[0u8;0x4000];page[..r.as_bytes().len()].copy_from_slice(r.as_bytes());std::io::stdout().write_all(&page).unwrap();}\n'
    code+='assert!(operands::ComputeSupportG16C::new(0,0x1008000,0xfffffc2207004000,20).is_none());\n'
    code+='assert!(operands::ComputeSupportG16C::new(0x7000000000,0x7001008000,0xfffffc2207004000,0).is_none());\n'
    code+='assert!(operands::ComputeSupportG16C::new(0x7000000000,0x7001008000,0xfffffc2207004000,1025).is_none());}\n'
    (tmp/'gate.rs').write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'gate.rs'),'-o',str(tmp/'gate')],check=True)
    actual=subprocess.check_output([str(tmp/'gate')]);expected=b''.join(c[-1]for c in cases)
    assert actual==expected
    a.evidence.mkdir(parents=True,exist_ok=False)
    (a.evidence/'summary.json').write_text(json.dumps(dict(whole_pages=len(cases),whole_page_bytes=len(actual),size_alignment_and_all_field_offsets_asserted=True,
        qualified_Python_ComputeOperands_template_equal=True,whole_page_sha256=hashlib.sha256(actual).hexdigest(),native_GPU_execution_tested=False,
        source_sha256=hashlib.sha256(source.read_bytes()).hexdigest()),indent=2)+'\n')
print('PASS: 32 complete Max compute FList pages; size, alignment and every field offset asserted')
