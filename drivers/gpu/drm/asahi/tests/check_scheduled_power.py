#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Validate G16C controller fields, templates and soft-float derived bytes.

The independent expected encoder is the qualified source-only Python backend.
Native comparisons read only a bounded, known power-controller DATA object.
"""
import argparse
import ast
import copy
import hashlib
import json
from pathlib import Path
import random
import re
import struct
import subprocess
import sys
import tempfile

import host_versions
import host_fdt

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1',type=Path)
parser.add_argument('--inputs',type=Path,required=True)
parser.add_argument('--fdt-template',type=Path,required=True)
parser.add_argument('--native-controller',type=Path,action='append',default=[])
parser.add_argument('--evidence',type=Path)
parser.add_argument('--rustc',default='rustc')
args=parser.parse_args()
tests=Path(__file__).resolve().parent
sys.path.insert(0,str(args.m1n1.resolve()/'proxyclient'))
from m1n1.g16c.platform import Platform
from m1n1.g16c import power
from m1n1.g16c.softfloat import F32

inputs=json.loads(args.inputs.read_text())
properties={k:bytes.fromhex(v) for k,v in inputs['properties'].items()}
platform=Platform(inputs['sgx_base'],inputs['sgx_span'],inputs['revision'],properties,inputs['core'],inputs['sram'])
variants=[{},None,{'gpu-turbo-controller-input':0},{'gpu-turbo-controller-input':1},
          {'gpu-turbo-controller-input':2},{'gpu-turbo-controller-input':3},
          {'gpu-avg-power-ki':F32(3.25).bits},
          {'gpu-avg-power-ki':F32(3.25).bits,'gpu-avg-power-ki-only':F32(8.0).bits},
          {'gpu-avg-power-ki-only':F32(8.0).bits},
          {'gpu-avg-power-filter-tc-ms':19,'gpu-ppm-filter-time-constant-ms':19},
          {'gpu-pwr-filter-time-constant':17,'gpu-perf-filter-time-constant2':31,
           'gpu-se-filter-time-constant':7,'gpu-perf-base-pstate':3,'gpu-perf-dz':7,
           'gpu-fast-die0-release-temp':77,'gpu-fast-die0-prop-tgt-delta':12}]
rng=random.Random(0x60412662)
for i in range(32):
    variants.append({'gpu-pwr-min-duty-cycle':rng.randrange(0,1001),
        'gpu-pwr-integral-min-clamp':rng.randrange(platform.maximum_power+1),
        'gpu-perf-base-pstate':rng.randrange(1,11),'gpu-perf-tgt-utilization':rng.randrange(50,101),
        'gpu-perf-dz':rng.randrange(0,30),'gpu-perf-boost-min-util':rng.randrange(0,101),
        'gpu-perf-integral-min-clamp':rng.randrange(0,96),
        'gpu-avg-power-min-duty-cycle':rng.randrange(0,1001),
        'gpu-pwr-filter-time-constant':rng.randrange(1,512),
        'gpu-perf-filter-time-constant':rng.randrange(1,32),
        'gpu-perf-filter-time-constant2':rng.randrange(1,512),
        'gpu-ppm-filter-time-constant-ms':rng.randrange(8,128),
        'gpu-se-filter-time-constant':rng.randrange(1,32),
        'gpu-se-filter-time-constant-1':rng.randrange(1,32),
        'gpu-avg-power-target-filter-tc':rng.randrange(1,16),
        'gpu-avg-power-filter-tc-ms':rng.randrange(8,256),
        'gpu-fast-die0-release-temp':rng.randrange(0,128),
        'gpu-fast-die0-prop-tgt-delta':rng.randrange(0,128),
        'gpu-turbo-controller-input':rng.randrange(0,4),
        'gpu-avg-power-ki-only':F32(0.25+i/16).bits,
        'gpu-fast-die0-integral-gain':F32(300.0+i*4).bits})

writes={}
put32,put64=power.put32,power.put64

def traced32(out,at,value):
    writes[at]=4
    return put32(out,at,value)


def traced64(out,at,value):
    writes[at]=8
    return put64(out,at,value)

power.put32,power.put64=traced32,traced64
expected={}
with tempfile.TemporaryDirectory(prefix='scheduled-power-') as temporary:
    tmp=Path(temporary)
    macro_flags=host_versions.build(tmp,args.rustc)
    loader=host_fdt.Loader(args.m1n1,tmp,args.fdt_template)
    _,baseline_node=loader.build(platform)
    prefix=host_fdt.rust_prelude()+'use std::{fs,path::Path};\n'
    for module in ['float','scheduled_initdata','scheduled_hwdata','scheduled_board']:
        prefix+=f'#[path={json.dumps(str(tests.parent/(module+".rs")))}] mod {module};\n'
    code='fn main(){let root=std::env::args().nth(1).unwrap();let root=Path::new(&root);fs::create_dir_all(root).unwrap();\n'
    code+='let baseline_node='+host_fdt.rust_node(baseline_node)+';\n'
    code+='let mut board=scheduled_board::BoardG16C::from_fdt(&baseline_node).unwrap();\n'
    cases=[]
    for override in variants:
        props={k:v for k,v in properties.items() if not k.startswith('gpu-')} if override is None else dict(properties,**{k:struct.pack('<I',v) for k,v in override.items()})
        cases.append((props,platform.period_clocks,None))
    for period in [1,2,4,7,16]:
        props=dict(properties)
        props['gpu-ppm-filter-time-constant-ms']=struct.pack('<I',period*2+1)
        props['gpu-avg-power-filter-tc-ms']=struct.pack('<I',period*4+1)
        cases.append((props,period*24000+8000,None))
    for path in args.native_controller:cases.append((properties,platform.period_clocks,path))
    for case,(props,clocks,native_path) in enumerate(cases):
        p=copy.copy(platform);p.properties=props;p.period_clocks=clocks;p.period_ms=clocks//24000
        image=bytearray(power.encode_power(p,bytearray(0x20000),max_policy=True)[0x5380:0xb754])
        links=[0xfffffc20000f34c0+case*0x4000,0xfffffc20c0500000+case*0x4000]
        if native_path:
            native=native_path.read_bytes();assert len(native)==0x63d4
            links=[struct.unpack_from('<Q',native,at-0x5380)[0] for at in [0x8ed0,0xb698]]
        for at,value in zip([0x8ed0,0xb698],links):struct.pack_into('<Q',image,at-0x5380,value)
        if native_path:assert bytes(image)==native
        expected[f'power-{case}.bin']=bytes(image)
        code+=' {\n'
        code+=f'board.period_clocks={clocks};board.period_ms={clocks//24000};\n'
        _,node=loader.build(p)
        code+='let node='+host_fdt.rust_node(node)+';\n'
        code+='let board=scheduled_board::BoardG16C::from_fdt(&node).unwrap();let settings=power::SettingsG16C::from_fdt(&node).unwrap();\n'
        code+='let params=power::ParametersG16C::derive(&board,&settings).unwrap();\n'
        code+=f'let links=power::PowerLinks{{thermal_context:{links[0]},auxiliary:{links[1]}}};\n'
        code+='let mut out=vec![0xa5;power::SIZE+32];params.encode(&mut out[16..],links).unwrap();assert_eq!(&out[..16],&[0xa5;16]);assert_eq!(&out[16+power::SIZE..],&[0xa5;16]);\n'
        code+=f'fs::write(root.join("power-{case}.bin"),&out[16..16+power::SIZE]).unwrap();\n'
        code+='out.fill(0xa5);assert_eq!(params.encode(&mut out[..power::SIZE-1],links),Err(power::InvalidPower));assert!(out.iter().all(|b|*b==0xa5));\n'
        code+='}\n'
    negative=[('gpu-pwr-filter-time-constant',0),('gpu-pwr-min-duty-cycle',1001),
        ('gpu-pwr-integral-min-clamp',platform.maximum_power+1),('gpu-perf-base-pstate',0),('gpu-perf-base-pstate',11),
        ('gpu-perf-tgt-utilization',101),('gpu-perf-dz',101),('gpu-perf-boost-min-util',101),('gpu-perf-integral-min-clamp',96),
        ('gpu-avg-power-min-duty-cycle',1001),('gpu-fast-die0-release-temp',0xffffffff),('gpu-fast-die0-prop-tgt-delta',0xffffffff),
        ('gpu-ppm-filter-time-constant-ms',0),('gpu-se-filter-time-constant',0),('gpu-se-filter-time-constant',0xffffffff),
        ('gpu-avg-power-target-filter-tc',0),('gpu-avg-power-filter-tc-ms',0),
        ('gpu-avg-power-ki',0x7f800000),('gpu-avg-power-ki-only',0x7fc00000),('gpu-ppm-ki',0xff800000),
        ('gpu-fast-die0-integral-gain',0x7fc00000)]
    code+=f'board.period_clocks={platform.period_clocks};board.period_ms={platform.period_ms};\n'
    for name,value in negative:
        field=name.removeprefix('gpu-').replace('-','_')
        code+=f'assert!(power::ParametersG16C::derive(&board,&power::SettingsG16C{{{field}:Some({value}),..Default::default()}}).is_err());\n'
    code+='let mut bad=baseline_node.clone();bad.props.insert("apple,pwr-min-duty-cycle".to_owned(),vec![1,2,3]);assert!(power::SettingsG16C::from_fdt(&bad).is_err());\n'
    code+='board.period_ms=0;assert!(power::ParametersG16C::derive(&board,&Default::default()).is_err());\n'
    code+='board.period_ms=u32::MAX;assert!(power::ParametersG16C::derive(&board,&Default::default()).is_err());\n'
    code+='}\n'
    # Independent field map: trace actual qualified-source pack operations.
    # Aggregate arrays are asserted as arrays rather than redundant scalars.
    field_map={}
    for at,size in writes.items():
        if 0x5440<=at<0x546c:continue
        field_map[f'field_{at-0x140:04x}']=(at-0x5380,size)
    for field,at,size in [('thermal_context',0x8ed0,8),('auxiliary',0xb698,8),
                         ('scale',0x5440,44),('initial_power',0x6f6f,1024),
                         ('field_7758',0x7898,1),('field_7759',0x7899,1)]:
        field_map[field]=(at-0x5380,size)
    assertions=[]
    for field,(at,size) in field_map.items():
        assertions+=[f'assert!(core::mem::offset_of!(PowerControllerG16C,{field})=={at});',
                     f'assert!(core::mem::size_of_val(&template().{field})=={size});']
    wire_source=(tests.parent/'scheduled_power.rs').read_text()
    declared=set(re.findall(r'pub\(crate\) (\w+): \[u8;',wire_source.split('const fn template()')[0]))
    declared={f for f in declared if not f.startswith('pad_')}
    assert declared==set(field_map),(declared-set(field_map),set(field_map)-declared)
    checked=tmp/'checked_power.rs';checked.write_text(wire_source+'\nconst _: () = {\n'+'\n'.join(assertions)+'\n};\n')
    prefix+=f'#[path={json.dumps(str(checked))}] mod power;\n'
    (tmp/'main.rs').write_text(prefix+code)
    subprocess.run([args.rustc,'--edition=2021','-Dwarnings',str(tmp/'main.rs'),*macro_flags,'-o',str(tmp/'gate')],check=True)
    subprocess.run([str(tmp/'gate'),str(tmp/'images')],check=True)
    for name,want in expected.items():
        actual=(tmp/'images'/name).read_bytes()
        assert actual==want,(name,len(actual),len(want),[(hex(i+0x5240),a,b) for i,(a,b) in enumerate(zip(actual,want)) if a!=b][:16])
    if args.evidence:
        args.evidence.mkdir(parents=True,exist_ok=False)
        for name in expected:(args.evidence/name).write_bytes((tmp/'images'/name).read_bytes())
        summary={'independent_field_offset_and_width_assertions':len(assertions),'wire_fields':len(field_map),
            'whole_source_images':len(cases)-len(args.native_controller),'whole_native_DATA_images':len(args.native_controller),
            'invalid_property_or_board_checks':len(negative)+3,'rejection_before_encode_mutation_checks':len(cases),
            'native_driver_startup_qualified':False,'no_hardware_access':True,
            'source_sha256':{str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in
                [tests.parent/'scheduled_power.rs',tests.parent/'float.rs',args.m1n1/'proxyclient/m1n1/g16c/power.py',args.inputs]}}
        (args.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(f'PASS: {len(field_map)} wire fields / {len(assertions)} independent offset-width assertions; {len(cases)} whole controller images; {len(negative)+3} invalid property/board checks')
