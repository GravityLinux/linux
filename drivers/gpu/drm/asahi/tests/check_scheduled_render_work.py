#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Full render Work15/SKSM images against qualified source and native DATA."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import host_versions

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1', type=Path)
p.add_argument('--evidence', type=Path, required=True)
a = p.parse_args()
tests = Path(__file__).resolve().parent
sys.path.insert(0, str(a.m1n1 / 'proxyclient'))
from m1n1.g16c.records import Wrapper, RenderRecord, DESCRIPTOR_BASE

cases, lines, native = [], [], {}
rust_bool = lambda v: 'true' if v else 'false'
option = lambda v: 'None' if v is None else f'Some({v})'

def wrapper(name, w, captured=None):
    expected = w.encode()
    if captured is not None:
        assert expected == captured, name
    c = dict(scratch_low=w.scratch_low, scratch_high=w.scratch_high,
        support=w.support, queue_control=w.queue_control, grid=w.grid,
        item=0, ordinal=w.ordinal, context=w.context, support_id=w.support_id,
        uuid=w.uuid, scheduler=w.scheduler, scheduler_hint=w.scheduler_hint,
        first=rust_bool(w.first), field_52=w.field_52,
        field_5e=w.scheduler, field_62=w.field_62)
    fields = ','.join(f'{k}:{v}' for k,v in c.items())
    lines.append('let w=work::WorkWrapperG16C::for_render(&work::RenderWrapperParameters{'
        f'common:work::WrapperParameters{{{fields}}},fragment:{rust_bool(w.fragment)},'
        f'buffer_manager:{w.buffer_manager},lifecycle:{w.lifecycle},namespace:{w.namespace},'
        f'item:{w.item},item_control:{w.item_control}'
        '}).unwrap();std::io::stdout().write_all(w.as_bytes()).unwrap();')
    cases.append((name, expected))

def record(name, r, captured=None):
    expected = r.encode()
    if captured is not None:
        assert expected == captured, name
    arrays = r.arrays or (((0xa0,89),(0x7c0,16),(0xee0,23),(0x1600,10)) if r.fragment
                         else ((0x60,73),(0x760,4)))
    fields = dict(descriptor=r.descriptor, queue=r.queue, grid=r.grid,
        item=r.item, fragment=rust_bool(r.fragment), timeout=r.timeout,
        ready=rust_bool(r.ready), control=option(r.control),
        control_low=option(r.control_low), field_30=option(r.field_30), field_38=r.field_38)
    values = ','.join(f'{k}:{v}' for k,v in fields.items())
    array = '&['+','.join(f'({x},{n})' for x,n in arrays)+']'
    lines.append('let r=work::WorkRecordG16C::for_render(&work::RenderRecordParametersG16C{'
        f'{values},arrays:{array}'
        '}).unwrap();std::io::stdout().write_all(r.as_bytes()).unwrap();')
    cases.append((name, expected))

for i in range(128):
    fragment = bool(i % 2)
    grid = i % 127 + 1
    wrapper(f'wrapper-{i}', Wrapper(fragment, 0x7400000000+i*0x20000,
        0xfffffc2206000000+i*0x20000, 0xfffffc2207000000,
        0xfffffc2208000140, 0xfffffc20c3100000, grid,
        (0,1,255,256,65535,0xffffffff)[i%6], (i*601)&65535,
        i%63+1, i*9, i*13, i%4, i*7, i*17, i*19,
        i%3==0, item_control=(i*257)&65535, field_52=i*23,field_62=i*29))
    arrays = tuple((off,(i*7+j)%127+1) for j,off in enumerate(
        (0xa0,0x7c0,0xee0,0x1600) if fragment else (0x60,0x760)))
    record(f'record-{i}', RenderRecord(DESCRIPTOR_BASE+i*0x4000,
        0xfffffc2208000000+i*0x2c000, grid,
        (0,1,255,256,65535,(1<<30)-2)[i%6], fragment,
        timeout=i*499, ready=i%3!=0, arrays=arrays,
        control=None if i%3==0 else (i*499)&65535,
        control_low=None if i%3==0 else i*0x1020304,
        field_30=None if i%3==0 else i*0x100000001,
        field_38=i*0x10100000001))

# Only explicitly named capture DATA records; never client shader/firmware.
for directory in ['single-user-texture-pixel-20261010', 'single-user-render32-msaa2-20261010']:
    for stage in (0,1):
        folder=a.m1n1/'build/m4max-gpu'/directory
        path=folder/f'stage-{stage}-wrapper.bin'
        b=path.read_bytes();assert len(b)==0xc0
        q=lambda at:struct.unpack_from('<Q',b,at)[0]
        h=lambda at:struct.unpack_from('<H',b,at)[0]
        w=lambda at:struct.unpack_from('<I',b,at)[0]
        wrapper(str(path),Wrapper(bool(stage),q(8),q(0x10),q(0x36),q(0x4a),q(0x6e),
            h(0x18),w(0x2a),h(0x3e),h(0x32),h(0x56),h(0x5a),h(0x1e),h(0x46),
            h(0x76),h(0x7e),bool(h(0x1a)),h(0x2e),h(0x52),h(0x62)),b)
        native[str(path)]=hashlib.sha256(b).hexdigest()
        path=folder/f'stage-{stage}-record.bin';b=path.read_bytes();assert len(b)==0x200
        pointers=[q(0x150+j*8) for j in range(4)]
        arrays=tuple((DESCRIPTOR_BASE+(v&0x7fffffff)*32-q(0x10),(v>>40)//8)
                     for v in pointers if v)
        item=(q(0)&0xffffffff)//4-1
        record(str(path),RenderRecord(q(0x10),q(0x18),(q(0)>>42)&127,item,bool(stage),
            timeout=h(0x26),ready=bool(w(0x144)),arrays=arrays,control=h(0x24),
            control_low=w(0x20),field_30=q(0x30),field_38=q(0x38)),b)
        native[str(path)]=hashlib.sha256(b).hexdigest()

with tempfile.TemporaryDirectory(prefix='render-work-') as directory:
    tmp=Path(directory);flags=host_versions.build(tmp)
    code='#![allow(dead_code)]\nuse std::io::Write;\n'
    for name in ['scheduled_compute','scheduled_work']:
        module='wire' if name=='scheduled_compute' else 'work'
        code+=f'#[path={json.dumps(str(tests.parent/(name+".rs")))}]mod {module};\n'
    code+='fn main(){\n'+'\n'.join(lines)+'''
    let arrays=[(0x60,73),(0x760,4)];
    let mut p=work::RenderRecordParametersG16C{descriptor:work::DESCRIPTOR_BASE_G16C,
        queue:0xfffffc2208000000,grid:1,item:0,fragment:false,timeout:4000,
        ready:true,arrays:&arrays,control:None,control_low:None,field_30:None,field_38:0};
    p.grid=128;assert_eq!(work::WorkRecordG16C::for_render(&p).err(),Some(wire::Error::Invalid));p.grid=1;
    p.queue=0;assert!(work::WorkRecordG16C::for_render(&p).is_err());p.queue=1;
    p.item=(1<<30)-1;assert!(work::WorkRecordG16C::for_render(&p).is_err());p.item=0;
    p.fragment=true;p.grid=0;assert!(work::WorkRecordG16C::for_render(&p).is_err());p.fragment=false;p.grid=1;
    p.arrays=&[];assert!(work::WorkRecordG16C::for_render(&p).is_err());
    p.arrays=&[(0x60,1);5];assert!(work::WorkRecordG16C::for_render(&p).is_err());
    p.arrays=&[(0x60,0)];assert!(work::WorkRecordG16C::for_render(&p).is_err());
    p.arrays=&[(0x60,128)];assert!(work::WorkRecordG16C::for_render(&p).is_err());
    p.arrays=&[(0x61,1)];assert!(work::WorkRecordG16C::for_render(&p).is_err());
    p.arrays=&arrays;p.descriptor=0;assert!(work::WorkRecordG16C::for_render(&p).is_err());
    p.descriptor=u64::MAX;assert!(work::WorkRecordG16C::for_render(&p).is_err());
    }
    '''
    (tmp/'main.rs').write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'main.rs'),*flags,'-o',str(tmp/'gate')],check=True)
    actual=subprocess.check_output([str(tmp/'gate')]);cursor=0
    for name,expected in cases:
        got=actual[cursor:cursor+len(expected)]
        assert got==expected,(name,[hex(i)for i,(x,y)in enumerate(zip(got,expected))if x!=y][:16])
        cursor+=len(expected)
    assert cursor==len(actual)
a.evidence.mkdir(parents=True,exist_ok=False)
(a.evidence/'summary.json').write_text(json.dumps(dict(whole_render_objects=len(cases),
    independent_native_DATA_objects=len(native),native_DATA_sha256=native,
    every_field_size_alignment_asserted=True,whole_bytes=len(actual),
    malformed_record_cases=11,
    sha256=hashlib.sha256(actual).hexdigest(),native_GPU_render_execution=False,
    source_sha256=hashlib.sha256((tests.parent/'scheduled_work.rs').read_bytes()).hexdigest()),indent=2)+'\n')
print('PASS',len(cases),'whole render records/wrappers;',len(native),'native DATA images; every layout assertion')
