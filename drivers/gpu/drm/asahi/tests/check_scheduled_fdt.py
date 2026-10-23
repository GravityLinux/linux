#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Populate Max FDT in m1n1 and consume typed OPP/config nodes in Rust.

Compare all derived fields and whole hardware images with the qualified source
builder. Does not connect to hardware or inspect executable bytes.
"""
import argparse
import copy
from dataclasses import replace
import hashlib
import json
from pathlib import Path
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
parser.add_argument('--native-hwdata',type=Path,action='append',default=[])
parser.add_argument('--evidence',type=Path)
parser.add_argument('--rustc',default='rustc')
args=parser.parse_args()
tests=Path(__file__).resolve().parent
sys.path.insert(0,str(args.m1n1.resolve()/'proxyclient'))
from m1n1.g16c.platform import Platform
from m1n1.g16c.abi_defaults import apply
from m1n1.g16c.firmware import HardwareRegion,hardware_regions

inputs=json.loads(args.inputs.read_text())
properties={k:bytes.fromhex(v) for k,v in inputs['properties'].items()}
assert properties['perf-states']==b''.join(struct.pack('<II',*r) for r in inputs['core'])
assert properties['perf-states-sram']==b''.join(struct.pack('<II',*r) for r in inputs['sram'])


def register(r):
    fields={'physical':(r.physical,64),'address':(r.address,64),'size':(r.size,32),
            'stride':(r.stride,32),'relative':(r.relative,64),'flags':(r.flags,32)}
    return f'({r.slot}, wire::MmioRegion{{'+','.join(f'{k}:{v}u{bits}.to_le_bytes()' for k,(v,bits) in fields.items())+',..Default::default()})'

expected={}
with tempfile.TemporaryDirectory(prefix='scheduled-board-') as temporary:
    tmp=Path(temporary)
    flags=host_versions.build(tmp,args.rustc)
    loader=host_fdt.Loader(args.m1n1,tmp,args.fdt_template)
    loader_rejections=loader.rejection_checks(Platform(inputs['sgx_base'],inputs['sgx_span'],inputs['revision'],properties,inputs['core'],inputs['sram']))
    code=host_fdt.rust_prelude()+'use std::{fs,path::Path};\n'
    for name in ['scheduled_initdata','scheduled_hwdata','scheduled_board']:
        code+=f'#[path={json.dumps(str(tests.parent/(name+".rs")))}] mod {name};\n'
    code+='use scheduled_initdata as wire;use scheduled_board::HardwarePointers;\n'
    code+='fn main(){let root=std::env::args().nth(1).unwrap();let root=Path::new(&root);fs::create_dir_all(root).unwrap();\n'
    negative_checks=0
    for case,native_path in enumerate([None]*32+args.native_hwdata):
        props=dict(properties)
        core=[list(row) for row in inputs['core']]
        memory=[list(row) for row in inputs['sram']]
        revision=inputs['revision']
        if case and native_path is None:
            for rows,voltage_step in [(core,17),(memory,13)]:
                for i,row in enumerate(rows):
                    if row[0]:row[0]+=case*1000000
                    row[1]+=case*voltage_step
            count=(case-1)%16+1
            props['cs-perf-states']=struct.pack('<QQ',1,count)+b''.join(
                struct.pack('<QQ',600000+i*10000,(200+i*7)*1000000) for i in range(count))+struct.pack('<Q',650000)
            props['gpu-power-sample-period']=struct.pack('<I',case+1)
            if case%2:props.pop('gpu-pwr-sample-period-aic-clks',None)
            else:props['gpu-pwr-sample-period-aic-clks']=struct.pack('<I',(case+1)*24000+8000)
            props['no-gpc']=struct.pack('<I',case&1)
            revision+=case
        props['perf-states']=b''.join(struct.pack('<II',*r) for r in core)
        props['perf-states-sram']=b''.join(struct.pack('<II',*r) for r in memory)
        p=Platform(inputs['sgx_base'],inputs['sgx_span'],revision,props,core,memory)
        native=native_path.read_bytes() if native_path else None
        timestamp=0xfffffc2181400000+case*0x10000
        status=0xfffffc218100c000+case*0x4000
        computed=0xfffffc2000120000+case*0x8000
        regions=[]
        if native is not None:
            assert len(native)==0x3db4
            updated=[]
            for r in p.registers:
                physical,address,size,stride,relative,access=struct.unpack_from('<QQIIQI',native,0x640+r.slot*0x28)
                assert (physical,size,stride,relative,access)==(r.physical,r.size,r.stride,r.relative,r.flags)
                updated.append(replace(r,address=address))
            p.registers=tuple(updated)
            timestamp=struct.unpack_from('<Q',native,0x28)[0]
            status=struct.unpack_from('<Q',native,0xe88)[0]
            computed=struct.unpack_from('<Q',native,0x26b8)[0]
            for at in (0x261c,0x265c):
                lead,low,tag,high,count,stride,trail=struct.unpack_from('<IIIQIII',native,at)
                assert tag==0x70
                region=HardwareRegion(lead,0x7000000000+low,high,count,stride,trail)
                assert region.encode()==native[at:at+0x40]
                regions.append(region)
        image=bytearray(apply(p.hwdata(),hardware_status_address=status))
        struct.pack_into('<Q',image,0x28,timestamp)
        struct.pack_into('<Q',image,0x26b8,computed)
        if regions:image=hardware_regions(image,regions)
        if native is not None:assert bytes(image)==native
        expected[f'hardware-{case}.bin']=bytes(image)
        cs=props['cs-perf-states'];_,count=struct.unpack_from('<QQ',cs)
        minimum=struct.unpack_from('<Q',cs,16+count*16)[0]//1000
        cs_states=[]
        for i in range(count):
            voltage,frequency=struct.unpack_from('<QQ',cs,16+i*16)
            cs_states.extend([frequency//1000000,voltage//1000,max(voltage//1000,minimum)])
        cs_states += [0]*((16-count)*3)
        values=[]
        for name in ['freq_a','freq_b','core_voltage','memory_voltage','cluster_core_voltage','cluster_memory_voltage','index_a','index_b']:
            values.extend(getattr(p,name))
        values+=cs_states+[count,p.revision,p.period_clocks,p.period_ms,int(p.gpc),p.maximum_power]
        values+=list(p.relative_a)+list(p.relative_b)
        expected[f'fields-{case}.bin']=struct.pack('<'+'I'*len(values),*values)
        code+=' {\n'
        dt,node=loader.build(p,cs_count=count if count!=8 else None)
        assert node['props']['status']==b'disabled\0'
        assert not any(k.startswith('apple,sgx-') for k in node['props'])
        code+='let node='+host_fdt.rust_node(node)+';\n'
        code+='let board=scheduled_board::BoardG16C::from_fdt(&node).unwrap();let mut fields=vec![];\n'

        for field in ['freq_a','freq_b','core_voltage','memory_voltage']:
            code+=f'for v in board.{field} {{fields.extend_from_slice(&v.to_le_bytes());}}\n'
        for field in ['cluster_core','cluster_memory']:
            code+=f'for row in board.{field} {{for v in row {{fields.extend_from_slice(&v.to_le_bytes());}}}}\n'
        for field in ['index_a','index_b']:
            code+=f'for v in board.{field} {{fields.extend_from_slice(&v.to_le_bytes());}}\n'
        code+='for state in board.cs {for v in [state.frequency,state.core_voltage,state.memory_voltage]{fields.extend_from_slice(&v.to_le_bytes());}}\n'
        code+='for v in [board.cs_count as u32,board.revision,board.period_clocks,board.period_ms,u32::from(board.gpc),board.maximum_power]{fields.extend_from_slice(&v.to_le_bytes());}\n'
        code+='for v in board.relative_a.into_iter().chain(board.relative_b){fields.extend_from_slice(&v.to_le_bytes());}\n'
        code+=f'fs::write(root.join("fields-{case}.bin"),fields).unwrap();\n'
        code+='let registers=['+','.join(register(r) for r in p.registers)+'];\n'
        region_source='None'
        if regions:
            code+='let regions=['+','.join('wire::HardwareRegion{'+f'lead:{r.lead}u32.to_le_bytes(),low:{r.low-0x7000000000}u32.to_le_bytes(),high:{r.high}u64.to_le_bytes(),count:{r.count}u32.to_le_bytes(),stride:{r.stride}u32.to_le_bytes(),trail:{r.trail}u32.to_le_bytes(),..Default::default()'+'}' for r in regions)+'];\n'
            region_source='Some(&regions)'
        code+=f'let params=board.hardware(&registers,HardwarePointers{{status:{status},computed:{computed},timestamp:{timestamp}}},{region_source});\n'
        code+='let mut out=vec![0xa5;scheduled_hwdata::SIZE+16];params.build(&mut out).unwrap();assert_eq!(&out[scheduled_hwdata::SIZE..],&[0xa5;16]);\n'
        code+=f'fs::write(root.join("hardware-{case}.bin"),&out[..scheduled_hwdata::SIZE]).unwrap();\n'
        # Corrupt typed FDT cells/OPP topology, without involving raw ADT in
        # Linux. All mutations are isolated clones of the populated tree.
        mutations=[
          'bad.props.remove("apple,chip-revision");',
          'bad.props.insert("apple,power-sample-period".to_owned(),0u32.to_be_bytes().to_vec());',
          'bad.props.insert("apple,pwr-sample-period-aic-clks".to_owned(),1u32.to_be_bytes().to_vec());',
          'bad.props.insert("apple,max-power-milliwatt".to_owned(),0u32.to_be_bytes().to_vec());',
          'bad.props.insert("apple,gpc-enabled".to_owned(),vec![1,2,3]);',
          'bad.links.remove("operating-points-v2");',
          'bad.links.get_mut("operating-points-v2").unwrap().nodes.pop();',
          'bad.links.get_mut("apple,cs-opp").unwrap().nodes.clear();',
          'bad.links.get_mut("operating-points-v2").unwrap().nodes[2].props.insert("opp-microvolt".to_owned(),vec![0;12]);',
          'bad.links.get_mut("operating-points-v2").unwrap().nodes[2].props.insert("opp-hz".to_owned(),vec![0;4]);',
          'bad.links.get_mut("operating-points-v2").unwrap().nodes[2].props.insert("apple,perf-index".to_owned(),vec![0xff;8]);',
          'bad.links.get_mut("operating-points-v2").unwrap().nodes[2].props.insert("apple,relative-power".to_owned(),vec![0xff;8]);',
          'bad.links.get_mut("apple,cs-opp").unwrap().nodes[0].props.insert("opp-microvolt".to_owned(),vec![0;8]);',
        ]
        for mutation in mutations:
            code+='{let mut bad=node.clone();'+mutation+'assert!(scheduled_board::BoardG16C::from_fdt(&bad).is_err());}\n'
            negative_checks+=1
        code+='}\n'
    code+='}\n'
    (tmp/'main.rs').write_text(code)
    subprocess.run([args.rustc,'--edition=2021','-Dwarnings',str(tmp/'main.rs'),*flags,'-o',str(tmp/'gate')],check=True)
    subprocess.run([str(tmp/'gate'),str(tmp/'images')],check=True)
    for name,want in expected.items():
        actual=(tmp/'images'/name).read_bytes()
        assert actual==want,(name,len(actual),len(want),[(hex(i),a,b) for i,(a,b) in enumerate(zip(actual,want)) if a!=b][:16])
    if args.evidence:
        args.evidence.mkdir(parents=True,exist_ok=False)
        for name in expected:(args.evidence/name).write_bytes((tmp/'images'/name).read_bytes())
        summary={'m1n1_populated_FDT_variants':32,'whole_native_Max_DATA_images':len(args.native_hwdata),
                 'compared_images':len(expected),'invalid_input_checks':negative_checks,'C_loader_rejections_before_mutation':loader_rejections,
                 'hardware_access':False,'driver_raw_ADT_parsing':False,'native_driver_startup_qualified':False,
                 'source_sha256':{str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in
                     [tests.parent/'scheduled_board.rs',tests.parent/'scheduled_hwdata.rs',tests.parent/'scheduled_initdata.rs',args.inputs]}}
        (args.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(f'PASS: 32 m1n1-populated FDT variants, {len(args.native_hwdata)} whole native DATA images, {len(expected)} complete derived/config images, {negative_checks} invalid FDT inputs; {loader_rejections} C loader rejections')
