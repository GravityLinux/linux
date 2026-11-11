#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare the complete Neo VM RAM image with the known-good tagged walker."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

import host_versions
import host_vm

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--evidence',type=Path)
args=parser.parse_args()
tests=Path(__file__).resolve().parent
linux=tests.parents[4]
baseline='a6883c3c6e5aa42548543a5a7b82bb6e375d6922'
with tempfile.TemporaryDirectory(prefix='scheduled-neo-vm-') as directory:
    tmp=Path(directory);flags=host_versions.build(tmp)
    old=tmp/'baseline_vm.rs'
    old.write_bytes(subprocess.check_output(['git','show',baseline+':drivers/gpu/drm/asahi/g17p_vm.rs'],cwd=linux))
    main='''
use g17p_platform::{Platform,Region};
struct Buffers(Vec<Vec<u8>>);
impl g17p_initgraph::Storage for Buffers {
    fn object(&mut self,i:usize)->Result<&mut [u8],g17p_abi::InvalidSize>{Ok(&mut self.0[i])}
}
fn main(){
    let platform=Platform{regions:[Region{base:0x101fffb8000},Region{base:0x101fff38000}]};
    let mut buffers=Buffers(g17p_initgraph::OBJECT_LAYOUT.iter().map(|o|vec![0;o.size]).collect());
    let graph=g17p_initgraph::build(&mut buffers,0xfffffc2000000000,&g17p_layout::PERFORMANCE).unwrap();
    let image=g17p_image::Image{graph,buffers:buffers.0};
    let mut memory=g17p_memory::Memory::new();
    for &(base,size) in g17p_topology::RESERVATIONS.iter().chain([(0x101fff30000,0xcc000)].iter()){memory.reserve(base,size);}
    let originals=[[0x101fff3c403u64,0x04200101fff484cb,0x00200101fff344cb],
                   [0x101fff7c403,0x04200101fff884cb,0x00200101fff344cb]];
    for (peer,values) in originals.iter().enumerate(){for (i,value) in values.iter().enumerate(){memory.write64(platform.regions[1].base+peer as u64*0x40000+i as u64*8,*value).unwrap();}}
    let vm=g17p_vm::Vm::build(&device::Device,&mut memory,&platform,&image).unwrap();
    assert_eq!(vm.firmware_root(),0x10021598000);
    let directory=std::path::PathBuf::from(std::env::args().nth(1).unwrap());std::fs::create_dir_all(&directory).unwrap();
    memory.dump(&directory.join("ram.bin"));std::fs::write(directory.join("calls.txt"),memory.calls.to_string()).unwrap();
}
'''
    for profile,source in [('baseline',old),('current',tests.parent/'g17p_vm.rs')]:
        code=host_vm.prelude()
        for name in ['scheduled_initdata','scheduled_boot_records','scheduled_hwdata',
                     'g17p_abi','g17p_layout','g17p_initgraph','g17p_opening','g17p_topology']:
            code+=f'#[path={json.dumps(str(tests.parent/(name+".rs")))}] mod {name};\n'
        code+=f'#[path={json.dumps(str(source))}] mod g17p_vm;\n'+main
        (tmp/'main.rs').write_text(code)
        subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'main.rs'),*flags,'-o',str(tmp/'gate')],check=True)
        subprocess.run([str(tmp/'gate'),str(tmp/profile)],check=True)
    a=tmp/'baseline/ram.bin';b=tmp/'current/ram.bin'
    assert a.stat().st_size==b.stat().st_size
    with a.open('rb') as af,b.open('rb') as bf:
        while True:
            data=af.read(1024*1024);assert data==bf.read(len(data))
            if not data:break
    calls=(tmp/'baseline/calls.txt').read_text()
    assert calls==(tmp/'current/calls.txt').read_text()
    with a.open('rb') as f:digest=hashlib.file_digest(f,'sha256').hexdigest()
    summary={'baseline':baseline,'baseline_vm_source_sha256':hashlib.sha256(old.read_bytes()).hexdigest(),
             'current_vm_source_sha256':hashlib.sha256((tests.parent/'g17p_vm.rs').read_bytes()).hexdigest(),
             'identical_whole_RAM_bytes_including_addresses':a.stat().st_size,
             'whole_RAM_sha256':digest,'identical_allocation_calls':int(calls),'native_hardware_tested':False}
    if args.evidence:
        args.evidence.mkdir(parents=True,exist_ok=False)
        (args.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(f'PASS: tagged Neo VM and common-profile walker produce identical {summary["identical_whole_RAM_bytes_including_addresses"]}-byte complete RAM image and {calls} allocations')
