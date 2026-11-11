#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exercise m1n1 reserved-memory publication and the real Rust FDT reader."""
import argparse
import ctypes as C
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

import host_fdt
import host_versions

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1',type=Path)
parser.add_argument('--inputs',type=Path,required=True)
parser.add_argument('--fdt-template',type=Path,required=True)
parser.add_argument('--evidence',type=Path)
args=parser.parse_args()
tests=Path(__file__).resolve().parent
sys.path.insert(0,str(args.m1n1/'proxyclient'))
from m1n1.g16c.platform import Platform
inputs=json.loads(args.inputs.read_text())
props={k:bytes.fromhex(v) for k,v in inputs['properties'].items()}
p=Platform(inputs['sgx_base'],inputs['sgx_span'],inputs['revision'],props,inputs['core'],inputs['sram'])
names=['gpu-region','gfx-shared-region','gfx-shared-l2-region','gfx-handoff','gfx-data']
regions=[[int.from_bytes(props[n+'-base'],'little'),int.from_bytes(props[n+'-size'],'little')] for n in names]
private=[int.from_bytes(props['rtkit-private-vm-region-'+n],'little') for n in ['base','size']]
expected={}
with tempfile.TemporaryDirectory(prefix='scheduled-resources-') as directory:
    tmp=Path(directory);flags=host_versions.build(tmp)
    loader=host_fdt.Loader(args.m1n1,tmp,args.fdt_template)
    def prepare():
        dt,_,_=loader.prepare(p)
        return dt
    def publish(dt,rs=regions,pv=private):
        values=(host_fdt.Region*5)(*[host_fdt.Region(*r) for r in rs])
        return loader.populate_memory(dt,loader.path(dt,b'/gpu'),values,*pv)
    code=host_fdt.rust_prelude()+'\n'
    code+=f'#[path={json.dumps(str(tests.parent/"scheduled_resources.rs"))}] mod resources;\n'
    code+='fn main(){let out=std::path::PathBuf::from(std::env::args().nth(1).unwrap());std::fs::create_dir_all(&out).unwrap();\n'
    negative=0
    for case in range(12):
        rs=[[base+case*0x100000,size] for base,size in regions]
        if case==11:rs[4][0]=(1<<42)-rs[4][1]
        dt=prepare();assert publish(dt,rs)==0
        gpu=loader.path(dt,b'/gpu')
        # Native wrapper subsequently fills typed OPP/configuration values.
        dt_input,input_values,owners=loader.prepare(p)
        assert loader.populate(dt,gpu,C.byref(input_values))==0
        node=loader.snapshot(dt,loader.path(dt,b'/gpu'),True)
        assert node['props']['status']==b'disabled\0'
        assert not any(k.startswith('apple,sgx-') for k in node['props'])
        for i,r in enumerate(rs):
            target=node['links'][f'memory-region#{i}']
            assert target['props']['reg']==struct.pack('>QQ',*r)
            assert target['props']['status']==b'okay\0' and target['props']['no-map']==b''
        code+='{let node='+host_fdt.rust_node(node)+';\n'
        code+='let r=resources::ResourcesG16C::from_fdt(&node).unwrap();let mut bytes=vec![];for p in r.regions{bytes.extend_from_slice(&p.base.to_le_bytes());bytes.extend_from_slice(&p.size.to_le_bytes());}for p in r.private_vm{bytes.extend_from_slice(&p.to_le_bytes());}\n'
        code+=f'std::fs::write(out.join("{case}-resources.bin"),bytes).unwrap();\n'
        code+='assert!(r.boot_table(r.regions[1].base));assert!(r.boot_table(r.regions[2].base));assert!(!r.boot_table(r.regions[1].base+1));assert!(!r.boot_table(r.regions[0].base));\n'
        expected[f'{case}-resources.bin']=b''.join(struct.pack('<QQ',*r) for r in rs)+struct.pack('<QQ',*private)
        if case==0:
            mutations=[
                'bad.props.remove("apple,firmware-build");',
                'bad.props.insert("apple,firmware-build".to_owned(),b"mBoot-18000.161.9\\0".to_vec());',
                'bad.props.insert("memory-region-names".to_owned(),b"ttbs\\0".to_vec());',
                'bad.props.insert("memory-region".to_owned(),vec![0;16]);',
                'bad.props.insert("apple,rtkit-private-vm-region".to_owned(),vec![0;16]);',
                'bad.props.insert("apple,rtkit-private-vm-region".to_owned(),vec![0;15]);']
            for i in range(5):
                key=f'memory-region#{i}'
                for expression in [f'bad.links.remove("{key}");',
                    f'bad.links.get_mut("{key}").unwrap().props.remove("no-map");',
                    f'bad.links.get_mut("{key}").unwrap().props.insert("no-map".to_owned(),vec![0]);',
                    f'bad.links.get_mut("{key}").unwrap().props.insert("status".to_owned(),b"disabled\\0".to_vec());',
                    f'bad.links.get_mut("{key}").unwrap().props.insert("reg".to_owned(),vec![0;15]);']:
                    mutations.append(expression)
                for base,size in [(0,0x4000),(1,0x4000),(0x100000,0),(0x100000,1),
                                  (1<<42,0x4000),((1<<64)-0x4000,0x8000),tuple(rs[(i+1)%5])]:
                    data=list(struct.pack('>QQ',base,size))
                    mutations.append(f'bad.links.get_mut("{key}").unwrap().props.insert("reg".to_owned(),vec!{data});')
            for mutation in mutations:
                code+='{let mut bad=node.clone();'+mutation+'assert!(resources::ResourcesG16C::from_fdt(&bad).is_err());}\n'
            negative=len(mutations)
        code+='}\n'
    code+='}\n'
    (tmp/'main.rs').write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'main.rs'),*flags,'-o',str(tmp/'gate')],check=True)
    subprocess.run([str(tmp/'gate'),str(tmp/'images')],check=True)
    for name,want in expected.items():assert (tmp/'images'/name).read_bytes()==want
    loader_negative=0
    for i in range(5):
        for replacement in [(0,0x4000),(1,0x4000),(0x100000,0),(0x100000,1),
                            (1<<42,0x4000),((1<<64)-0x4000,0x8000),tuple(regions[(i+1)%5])]:
            rs=[r[:] for r in regions];rs[i]=replacement
            dt=prepare();before=dt.raw;assert publish(dt,rs)!=0;assert dt.raw==before;loader_negative+=1
    for pv in [[0,private[1]],[private[0],0]]:
        dt=prepare();before=dt.raw;assert publish(dt,pv=pv)!=0;assert dt.raw==before;loader_negative+=1
    for mutation in ['missing-no-map','bad-reg-size','bad-handles','duplicate-handle','bad-names','bad-parent-cells']:
        dt=prepare();gpu=loader.path(dt,b'/gpu');handles=loader.property(dt,gpu,'memory-region');target=loader.phandle(dt,struct.unpack_from('>I',handles)[0])
        if mutation=='missing-no-map':assert loader.delprop(dt,target,b'no-map')==0
        if mutation=='bad-reg-size':assert loader.set(dt,target,b'reg',bytes(15),15)==0
        if mutation=='bad-handles':assert loader.set(dt,gpu,b'memory-region',bytes(16),16)==0
        if mutation=='duplicate-handle':assert loader.set(dt,gpu,b'memory-region',handles[:4]*5,20)==0
        if mutation=='bad-names':assert loader.set(dt,gpu,b'memory-region-names',b'x\0',2)==0
        if mutation=='bad-parent-cells':assert loader.set(dt,loader.path(dt,b'/reserved-memory'),b'#size-cells',struct.pack('>I',1),4)==0
        before=dt.raw;assert publish(dt)!=0;assert dt.raw==before;loader_negative+=1
    summary={'typed_FDT_resource_variants':12,'invalid_Rust_resources':negative,
             'C_loader_rejections_before_mutation':loader_negative,
             'native_Max_startup_qualified':False,'no_hardware_access':True,
             'images':{k:{'bytes':len(v),'sha256':hashlib.sha256(v).hexdigest()} for k,v in expected.items()}}
    if args.evidence:
        args.evidence.mkdir(parents=True,exist_ok=False)
        for k,v in expected.items():(args.evidence/k).write_bytes(v)
        (args.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(f'PASS: 12 typed resource layouts; {negative} invalid Rust resources; {loader_negative} C loader rejections before DT mutation')
