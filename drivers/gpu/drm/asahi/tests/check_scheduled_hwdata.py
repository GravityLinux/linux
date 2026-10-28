#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Qualify actual macro-expanded hardware-data layouts and complete encodings.

Reads only source builders and saved ADT scalar inputs. No device access.
"""
import argparse
import ast
import copy
from dataclasses import replace
import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

import host_versions

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1', type=Path)
parser.add_argument('--inputs', type=Path, required=True)
parser.add_argument('--g17p-initdata', type=Path, required=True)
parser.add_argument('--rustc', default='rustc')
parser.add_argument('--evidence', type=Path)
parser.add_argument('--native-hwdata', type=Path, action='append', default=[])
args = parser.parse_args()
tests = Path(__file__).resolve().parent
sys.path.insert(0, str(args.m1n1.resolve() / 'proxyclient'))
from m1n1.g16c.platform import Platform
from m1n1.g16c.abi_defaults import apply
from m1n1.g16c.firmware import HardwareRegion, hardware_regions

inputs = json.loads(args.inputs.read_text())
platform = Platform(inputs['sgx_base'], inputs['sgx_span'], inputs['revision'],
                    {k:bytes.fromhex(v) for k,v in inputs['properties'].items()},
                    inputs['core'], inputs['sram'])
spec = importlib.util.spec_from_file_location('g17p_reference', args.g17p_initdata)
reference = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reference)
known = vars(reference)
assertions = []
for ty in ['HardwareDataG16C', 'HardwareDataG17P']:
    for field, const in {'registers':'REGISTER_ARRAY_OFFSET', 'chip':'HWDATA_CHIP_ID',
                        'table_a':'FREQ_LADDER_A', 'table_b':'FREQ_LADDER_B_COPY',
                        'freq_b':'FREQ_LADDER_B', 'scale_b':'SCALE_LADDER_B',
                        'relative_a':'RELATIVE_LADDER_A', 'relative_b':'RELATIVE_LADDER_B',
                        'index_a':'INDEX_MAP_A', 'index_b':'INDEX_MAP_B'}.items():
        assertions.append(f'assert!(core::mem::offset_of!({ty}, {field}) == {known[const]});')
    assertions.append(f'assert!(core::mem::size_of::<{ty}>() == {known["HWDATA_SIZE"]});')
for field, const in {'channel_count':'HWDATA_CHANNEL_COUNT', 'flag_groups':'HWDATA_FLAG_GROUPS',
                     'unassigned':'HWDATA_UNASSIGNED_TABLE', 'regions':'REGION_RECORD_OFFSET'}.items():
    assertions.append(f'assert!(core::mem::offset_of!(HardwareDataG17P, {field}) == {known[const]});')
for field, const in {'core':'GROUP_VOLTAGE_DELTA', 'memory':'GROUP_MEMORY_VOLTAGE_DELTA'}.items():
    assertions.append(f'assert!(core::mem::offset_of!(ClockVoltageTable, {field}) == {known[const]});')
# The Max builder provides independent source offsets for the changed fields.
platform_ast = ast.parse((args.m1n1/'proxyclient/m1n1/g16c/platform.py').read_text())
platform_class = next(n for n in platform_ast.body if isinstance(n,ast.ClassDef) and n.name=='Platform')
hwdata_method = next(n for n in platform_class.body if isinstance(n,ast.FunctionDef) and n.name=='hwdata')
constants = {n.value for n in ast.walk(hwdata_method) if isinstance(n,ast.Constant) and type(n.value) is int}
for field, offset in {'unk_2596':0x2596, 'unassigned':0x25bc, 'computed':0x26b8,
                      'cs_max_state':0x1b90, 'cs_frequency':0x1b98,
                      'cs_core_voltage':0x1bd8, 'cs_memory_voltage':0x1c58}.items():
    assert offset in constants, (field, offset)
    assertions.append(f'assert!(core::mem::offset_of!(HardwareDataG16C, {field}) == {offset});')


def scalar_array(values):
    return '[' + ','.join(str(v) for v in values) + ']'


def matrix(values, columns=4):
    return '[' + ','.join(scalar_array(values[i:i+columns]) for i in range(0,len(values),columns)) + ']'


def register(r):
    fields = {'physical':(r.physical,64), 'address':(r.address,64),
              'size':(r.size,32), 'stride':(r.stride,32), 'relative':(r.relative,64), 'flags':(r.flags,32)}
    return f'({r.slot}, wire::MmioRegion {{' + ','.join(f'{k}:{v}u{bits}.to_le_bytes()' for k,(v,bits) in fields.items()) + ',..Default::default()})'

expected = {}
with tempfile.TemporaryDirectory(prefix='scheduled-hwdata-') as temporary:
    tmp=Path(temporary)
    macro_flags=host_versions.build(tmp,args.rustc)
    # Append external RE assertions to an unmodified copy of production source.
    checked=tmp/'scheduled_hwdata.rs'
    code=(tests.parent/'scheduled_hwdata.rs').read_text().replace('"g17p_constants.rs"',json.dumps(str(tests.parent/'g17p_constants.rs')))
    checked.write_text(code+'\nconst _: () = {\n'+'\n'.join(assertions)+'\n};\n')
    harness='''#![allow(dead_code)]
use std::{fs,path::Path};
'''
    harness+=f'#[path={json.dumps(str(tests.parent/"scheduled_initdata.rs"))}] mod scheduled_initdata;\n'
    harness+='use scheduled_initdata as wire;\n'
    harness+=f'#[path={json.dumps(str(checked))}] mod hw;\n'
    harness+='fn main() { let root=std::env::args().nth(1).unwrap();let root=Path::new(&root);fs::create_dir_all(root).unwrap();\n'
    harness+='fs::write(root.join("neo-template.bin"),hw::template_g17p_bytes()).unwrap();\n'
    harness+='assert_eq!(hw::HardwareDataG16C::default().as_bytes(),hw::template_g16c_bytes());\n'
    harness+='assert_eq!(hw::HardwareDataG17P::default().as_bytes(),hw::template_g17p_bytes());\n'
    expected['neo-template.bin']=reference.build_hwdata({}, {}, {n:[0]*11 for n in
         ['freq_a','freq_b','core_voltage','memory_voltage','scale_b','relative_a','relative_b','index_a','index_b']})
    for case, native_path in enumerate([None]*32 + args.native_hwdata):
        p=copy.copy(platform)
        p.properties=dict(platform.properties)
        if case and native_path is None:
            p.revision=case*17
            p.gpc=bool(case&1)
            p.period_ms=case+1
            for field in ['freq_a','freq_b','core_voltage','memory_voltage','relative_a','relative_b','index_a','index_b']:
                setattr(p,field,tuple(v+case*13+i for i,v in enumerate(getattr(p,field))))
            for field in ['cluster_core_voltage','cluster_memory_voltage']:
                setattr(p,field,tuple(v+case*11+i for i,v in enumerate(getattr(p,field))))
            p.registers=tuple(replace(r,address=r.address+case*0x1000000) for r in p.registers)
            count=(case-1)%16+1
            rows=[(600000+i*10000,(200+i*7)*1000000) for i in range(count)]
            p.properties['cs-perf-states']=struct.pack('<QQ',1,count)+b''.join(struct.pack('<QQ',*r) for r in rows)+struct.pack('<Q',650000)
        hardware_status=0xfffffc218100c000+case*0x4000
        computed=0xfffffc2000120000+case*0x8000
        timestamp=0xfffffc2181400000+case*0x10000
        native = native_path.read_bytes() if native_path else None
        if native is not None:
            assert len(native)==reference.HWDATA_SIZE
            # Decode only understood variable fields; every other byte must
            # still come from board inputs and our source template.
            updated=[]
            for r in p.registers:
                physical,address,size,stride,relative,flags=struct.unpack_from('<QQIIQI',native,0x640+r.slot*0x28)
                assert (physical,size,stride,relative,flags)==(r.physical,r.size,r.stride,r.relative,r.flags)
                updated.append(replace(r,address=address))
            p.registers=tuple(updated)
            timestamp=struct.unpack_from('<Q',native,0x28)[0]
            hardware_status=struct.unpack_from('<Q',native,0xe88)[0]
            computed=struct.unpack_from('<Q',native,0x26b8)[0]
        cs=p.properties['cs-perf-states']
        _,count=struct.unpack_from('<QQ',cs)
        minimum=struct.unpack_from('<Q',cs,16+count*16)[0]//1000
        states=[struct.unpack_from('<QQ',cs,16+i*16) for i in range(count)]
        out=bytearray(apply(p.hwdata(),hardware_status_address=hardware_status))
        struct.pack_into('<Q',out,0x26b8,computed)
        struct.pack_into('<Q',out,0x28,timestamp)
        regions=[HardwareRegion(0x100+i,0x7000200000+i*0x40000+case*0x4000,
                    0xfffffc20c0200000+i*0x40000+case*0x4000,0x800,0x40,2+i) for i in range(2)]
        if native is not None:
            regions=[]
            for at in (0x261c,0x265c):
                lead,low,tag,high,count,stride,trail=struct.unpack_from('<IIIQIII',native,at)
                assert tag==0x70
                region=HardwareRegion(lead,0x7000000000+low,high,count,stride,trail)
                assert region.encode()==native[at:at+0x40]
                regions.append(region)
        include_regions = bool(case&1 or native is not None)
        if include_regions:out=hardware_regions(out,regions)
        if native is not None:
            assert bytes(out)==native, ('native source reconstruction',[(hex(i),a,b) for i,(a,b) in enumerate(zip(out,native)) if a!=b][:16])
        expected[f'max-{case}.bin']=bytes(out)
        harness+=' {\n'
        harness+='let registers=['+','.join(register(r) for r in p.registers)+'];\n'
        harness+='let cs=['+','.join(f'hw::CsState{{frequency:{f//1000000},core_voltage:{v//1000},memory_voltage:{max(v//1000,minimum)}}}' for v,f in states)+'];\n'
        harness+='let _regions=['+','.join('wire::HardwareRegion{'+f'lead:{r.lead}u32.to_le_bytes(),low:{r.low-0x7000000000}u32.to_le_bytes(),high:{r.high}u64.to_le_bytes(),count:{r.count}u32.to_le_bytes(),stride:{r.stride}u32.to_le_bytes(),trail:{r.trail}u32.to_le_bytes(),..Default::default()'+'}' for r in regions)+'];\n'
        harness+='let mut params=hw::ParametersG16C { registers:&registers,cs:&cs,\n'
        for field in ['freq_a','freq_b','core_voltage','memory_voltage','relative_a','relative_b','index_a','index_b']:
            harness+=f'{field}:&{scalar_array(getattr(p,field))},\n'
        harness+='cluster_core:&'+matrix(p.cluster_core_voltage)+',cluster_memory:&'+matrix(p.cluster_memory_voltage)+',\n'
        harness+=f'revision:{p.revision},gpc:{str(p.gpc).lower()},period_ms:{p.period_ms},hardware_status:{hardware_status},computed:{computed},timestamp:{timestamp},regions:'+('Some(&_regions)' if include_regions else 'None')+'};\n'
        harness+='let mut out=vec![0xa5;hw::SIZE+16];params.build(&mut out).unwrap();assert_eq!(&out[hw::SIZE..],&[0xa5;16]);\n'
        harness+=f'fs::write(root.join("max-{case}.bin"),&out[..hw::SIZE]).unwrap();\n'
        # Rejection must leave the caller's output untouched.
        harness+='out.fill(0xa5);assert_eq!(params.build(&mut out[..hw::SIZE-1]),Err(hw::InvalidGeometry));assert!(out.iter().all(|b|*b==0xa5));\n'
        harness+='params.period_ms=0;assert_eq!(params.build(&mut out),Err(hw::InvalidGeometry));assert!(out.iter().all(|b|*b==0xa5));\n'
        harness+='params.period_ms=1;params.cs=&[];assert_eq!(params.build(&mut out),Err(hw::InvalidGeometry));assert!(out.iter().all(|b|*b==0xa5));\n'
        harness+='params.cs=&cs;let duplicate=[(17,wire::MmioRegion::default()),(17,wire::MmioRegion::default())];params.registers=&duplicate;assert_eq!(params.build(&mut out),Err(hw::InvalidGeometry));assert!(out.iter().all(|b|*b==0xa5));\n'
        harness+='let disabled=[(2,wire::MmioRegion::default())];params.registers=&disabled;assert_eq!(params.build(&mut out),Err(hw::InvalidGeometry));assert!(out.iter().all(|b|*b==0xa5));\n'
        harness+='let invalid=[(53,wire::MmioRegion::default())];params.registers=&invalid;assert_eq!(params.build(&mut out),Err(hw::InvalidGeometry));assert!(out.iter().all(|b|*b==0xa5));\n'
        harness+='params.registers=&registers;params.hardware_status+=1;assert_eq!(params.build(&mut out),Err(hw::InvalidGeometry));assert!(out.iter().all(|b|*b==0xa5));\n'
        harness+='}\n'
    harness+='}\n'
    (tmp/'main.rs').write_text(harness)
    subprocess.run([args.rustc,'--edition=2021','-Dwarnings',str(tmp/'main.rs'),*macro_flags,'-o',str(tmp/'gate')],check=True)
    subprocess.run([str(tmp/'gate'),str(tmp/'images')],check=True)
    for name,want in expected.items():
        actual=(tmp/'images'/name).read_bytes()
        assert actual==want,(name,len(actual),len(want),[(hex(i),a,b) for i,(a,b) in enumerate(zip(actual,want)) if a!=b][:16])
    if args.evidence:
        args.evidence.mkdir(parents=True,exist_ok=False)
        for name in expected:
            (args.evidence/name).write_bytes((tmp/'images'/name).read_bytes())
        summary={'independent_RE_assertions':len(assertions),'Max_whole_images':32,'Neo_template_images':1,
                 'native_whole_images':len(args.native_hwdata),'negative_geometry_checks':len(expected)*7-7,'native_hardware_qualification':False,
                 'source_sha256':{str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in
                     [tests.parent/'scheduled_hwdata.rs',tests.parent/'scheduled_initdata.rs',args.inputs,args.g17p_initdata]}}
        (args.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(f'PASS: {len(assertions)} independent RE assertions; 32 complete Max hardware images; {len(args.native_hwdata)} whole native Max DATA images; Neo template; {(32+len(args.native_hwdata))*7} rejection-before-mutation checks')
