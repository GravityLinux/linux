#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Whole Rust engine-handoff records vs single-user native protocol DATA."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import tempfile

import host_versions

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1',type=Path)
p.add_argument('--pair',type=Path,required=True)
p.add_argument('--evidence',type=Path,required=True)
p.add_argument('--independent-pair',type=Path)
p.add_argument('--reuse-pair',type=Path)
p.add_argument('--same-api-pair',type=Path)
a=p.parse_args()
tests=Path(__file__).resolve().parent
cases=[]
references=[(a.pair,2,'pair'),(a.m1n1/'build/m4max-gpu/single-user-compute-four-queues-20261010',4,'four')]
if a.independent_pair:references.append((a.independent_pair,2,'independent'))
if a.reuse_pair:references.append((a.reuse_pair,2,'reused'))
if a.same_api_pair:references.append((a.same_api_pair,2,'same-api'))
for directory,count,label in references:
    native=json.loads((directory/'native-verification.json').read_text())
    assert native['pass'] and native['output_changed'] and native['output_expected']
    plan=json.loads((directory/'uat/replay-plan.json').read_text())
    assert plan['single_user'] and len(plan['submissions'])==count
    for index,held in enumerate(plan['submissions']):
        b=(directory/f'submission-{index}-record.bin').read_bytes()
        q=lambda off:struct.unpack_from('<Q',b,off)[0]
        grid=(q(0)>>42)&127
        item=((q(0)&0xfffffffc)>>2)-1
        n=((q(0x20)>>32)&255)+1
        mask=q(0x20)&0xffffffff
        active=mask.bit_count()
        assert 1<=active<=4 and mask==(1<<active)-1
        if label not in ('independent','reused','same-api'):assert active==n
        values=[q(0x28+j*8)for j in range(active)]
        points=[(v>>40,v&0xffffffff)for v in values]
        assert points[-1]==(grid,item)
        params=dict(descriptor=q(0x10),queue=q(0x18),grid=grid,item=item,
            primary_count=(q(0x150)>>40)//8,timeout=q(0x20)>>48,
            retire_control=q(0x138),control_130=q(0x130),flags=q(0)&(1<<60))
        cases.append(dict(name=f'{label}-{index}',params=params,span=n,points=points,
            address=held['scratch_high']+held['item']*0x200,expected=b))

with tempfile.TemporaryDirectory(prefix='compute-handoff-')as d:
    d=Path(d);flags=host_versions.build(d)
    code='#![allow(dead_code)]\nuse std::io::Write;\n'
    for name,module in [('scheduled_compute','wire'),('scheduled_work','work')]:
        code+=f'#[path={json.dumps(str(tests.parent/(name+".rs")))}]mod {module};\n'
    code+='fn main(){\n'
    for c in cases:
        params=','.join(f'{k}:{v}'for k,v in c['params'].items())
        points=','.join(f'({g},{v})'for g,v in c['points'])
        code+=f'let p=work::RecordParametersG16C{{{params},..Default::default()}};\n'
        code+=f'let raw=work::WorkRecordG16C::for_compute_control_dependencies(&p,{0x800+c["span"]-1},&[{points}]).unwrap();\n'
        code+='std::io::stdout().write_all(raw.as_bytes()).unwrap();\n'
    code+='''
    let p=work::RecordParametersG16C{descriptor:work::DESCRIPTOR_BASE_G16C,
        queue:0xfffffc2208000000,grid:1,item:0,..Default::default()};
    for points in [&[][..],&[(0,0)][..],&[(1,1)][..],&[(1,0),(1,0)][..],
        &[(1,0),(0,0)][..],&[(0,1<<30),(1,0)][..],&[(1,0),(128,0)][..],
        &[(0,0),(1,0),(2,0),(3,0),(4,0)][..]] {
        assert!(work::WorkRecordG16C::for_compute_dependencies(&p,points).is_err());
    }
    for span in [0,5,255] {assert!(work::WorkRecordG16C::for_compute_dependency_span(&p,span,&[(1,0)]).is_err());}
    assert!(work::WorkRecordG16C::for_compute_dependency_span(&p,1,&[(0,1),(1,0)]).is_err());
    }
    '''
    (d/'main.rs').write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(d/'main.rs'),*flags,'-o',str(d/'gate')],check=True)
    actual=subprocess.check_output([str(d/'gate')])
assert len(actual)==len(cases)*512
a.evidence.mkdir(parents=True,exist_ok=False)
report=[]
for i,c in enumerate(cases):
    data=actual[i*512:(i+1)*512]
    assert data==c['expected'],(c['name'],[hex(j)for j,(x,y)in enumerate(zip(data,c['expected']))if x!=y])
    path=a.evidence/(c['name']+'.bin');path.write_bytes(data)
    report.append(dict(name=c['name'],params=c['params'],span=c['span'],points=c['points'],
        address=c['address'],file=path.name,sha256=hashlib.sha256(data).hexdigest()))
(a.evidence/'summary.json').write_text(json.dumps(dict(whole_native_records=len(cases),
    native_pair=str(a.pair),all_records_exact=True,all_sizes_alignments_field_offsets_asserted=True,
    malformed_handoffs_rejected=12,source_bytes_copied_from_capture=False,
    source_sha256=hashlib.sha256((tests.parent/'scheduled_work.rs').read_bytes()).hexdigest(),
    GPU_execution=False,same_api_pair=None if a.same_api_pair is None else str(a.same_api_pair),independent_pair=None if a.independent_pair is None else str(a.independent_pair),reuse_pair=None if a.reuse_pair is None else str(a.reuse_pair),records=report),indent=2)+'\n')
print('PASS',len(cases),'entire native dependency records; every layout field;12 malformed vectors/spans rejected')
