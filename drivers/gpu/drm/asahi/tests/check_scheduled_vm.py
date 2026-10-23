#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Run Max's actual common UAT walker over owned RAM and verify all mappings."""
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
import host_vm
import g16c_graph_reference as reference

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
regions=[(int.from_bytes(props[n+'-base'],'little'),int.from_bytes(props[n+'-size'],'little')) for n in
         ['gpu-region','gfx-shared-region','gfx-shared-l2-region','gfx-handoff','gfx-data']]
with tempfile.TemporaryDirectory(prefix='scheduled-vm-') as directory:
    tmp=Path(directory);flags=host_versions.build(tmp)
    loader=host_fdt.Loader(args.m1n1,tmp,args.fdt_template)
    dt,incoming,owners=loader.prepare(p)
    values=(host_fdt.Region*5)(*[host_fdt.Region(*r) for r in regions])
    assert loader.populate_memory(dt,loader.path(dt,b'/gpu'),values,0xfffffc0000000000,0x2000000000)==0
    assert loader.populate(dt,loader.path(dt,b'/gpu'),C.byref(incoming))==0
    node=loader.snapshot(dt,loader.path(dt,b'/gpu'),True)
    b,ram=reference.build(args.m1n1,p,0x60412662)
    spans=reference.spans(b,ram)
    expected={f'{i}.bin':b.firmware.read(address,size) for i,(address,size,_) in enumerate(spans)}
    code=host_vm.prelude()
    for name in ['float','scheduled_initdata','scheduled_boot_records','scheduled_hwdata',
                 'scheduled_board','scheduled_power','scheduled_initgraph','scheduled_mmio',
                 'scheduled_resources','scheduled_image','g17p_abi','g17p_layout',
                 'g17p_initgraph','g17p_opening','g17p_topology','g17p_vm','scheduled_vm_g16c']:
        code+=f'#[path={json.dumps(str(tests.parent/(name+".rs")))}] mod {name};\n'
    code+='''
use prelude::*;
use g17p_memory::Memory;
use g17p_vm::Vm;
use scheduled_initgraph as graph;
use scheduled_resources::ResourcesG16C;
const ADDRESS:u64=0x0000ffffffffc000;
fn boot(r:&ResourcesG16C)->Memory {
    let mut m=Memory::new();for p in &r.regions[..4]{m.reserve(p.base,p.size);}
    let root=r.regions[1].base;let l2=r.regions[2].base;
    // Valid inherited firmware-code trees. Their data pages are deliberately
    // absent from the CPU RAM mock, so accidental executable reads will fail.
    for branch in 0..2u64 {
        let middle=root+(branch*2+1)*0x4000;let leaf=middle+0x4000;
        m.write64(root+branch*8,middle|3).unwrap();m.write64(middle,leaf|3).unwrap();
        m.write64(leaf,(r.regions[4].base+branch*0x4000)|0x0080000000000443).unwrap();
    }
    m.write64(root+16,l2|3).unwrap();
    m.write(r.regions[0].base,&vec![0xa5;0x4000]).unwrap();m
}
fn ranges(r:&ResourcesG16C)->Vec<(u64,u64)>{r.regions[..4].iter().map(|v|(v.base,v.size)).collect()}
fn main()->Result {
    let directory=std::path::PathBuf::from(std::env::args().nth(1).unwrap());std::fs::create_dir_all(&directory).unwrap();
'''
    code+='let node='+host_fdt.rust_node(node)+';\n'
    code+='''
    let r=ResourcesG16C::from_fdt(&node)?;
    let image=scheduled_image::ImageG16C::new(&node,0x60412662)?;
    let mut m=boot(&r);let before=m.snapshot(&ranges(&r));
    let mut vm=Vm::build_g16c(&mut m,&r,&image)?;
    let calls=m.calls;
    assert_eq!(vm.firmware_root(),r.regions[1].base);
    let c0=vm.boot_context_roots(0)?;let c1=vm.boot_context_roots(1)?;
    assert!(vm.boot_context_roots(2).is_err());
    let roots=[c0[0],c0[1],c1[0],c1[1],vm.firmware_root()];
    for (i,p) in roots.iter().enumerate(){assert!(!roots[..i].contains(p));}
    for p in [c0[1],c1[0],c1[1]]{assert!(m.read(p,0x4000)?.iter().all(|b|*b==0));}
    assert_eq!(m.read(r.regions[0].base,0x4000)?,before[0]);
    assert_eq!(m.read(r.regions[1].base,5*0x4000)?,before[1][..5*0x4000]);
    assert_eq!(m.read(r.regions[3].base,0x4000)?,before[3]);
    assert!(vm.render_fallback_leaves(&m)?.is_empty());
    let count=m.calls;assert!(vm.populate_g16c(&mut m,&image).is_err());assert_eq!(m.calls,count);
    let mut metadata=vec![];
    for (i,o) in graph::OBJECT_LAYOUT.iter().enumerate(){
        let va=graph::BASE+o.offset as u64;let mut bytes=vec![];
        for offset in (0..o.size).step_by(0x4000){
            let at=va+offset as u64;let pte=vm.pte(&m,2,at)?;
            assert_eq!(pte&!ADDRESS,o.pte_flags);metadata.extend_from_slice(&at.to_le_bytes());metadata.extend_from_slice(&(pte&!ADDRESS).to_le_bytes());
            bytes.extend_from_slice(&m.read(pte&ADDRESS,0x4000)?);
        }
        std::fs::write(directory.join(format!("{i}.bin")),bytes).unwrap();
    }
    for a in graph::ALIASES {for offset in (0..a.size).step_by(0x4000){
        let lo=vm.pte(&m,0,a.low+offset as u64)?;let hi=vm.pte(&m,2,a.high+offset as u64)?;
        assert_eq!(lo&ADDRESS,hi&ADDRESS);assert_eq!(lo&!ADDRESS,a.pte_flags);
    }}
    for map in image.mmio.mappings(){for offset in (0..map.size).step_by(0x4000){
        assert_eq!(vm.pte(&m,2,map.address+offset as u64)?,(map.physical+offset as u64)|map.pte_flags);
    }}
    for offset in [0x24000,0x104000,0xc00ac000,0xc04bc000,0xc4020000] {assert_eq!(vm.pte(&m,2,graph::BASE+offset)?,0);}
    // A host write through one real translated view reaches the other view.
    let a=graph::ALIASES[4];let old=m.read(vm.physical(&m,0,a.low)?,8)?;
    vm.write(&mut m,0,a.low,&0x1234567812345678u64.to_le_bytes())?;
    assert_eq!(m.read64(vm.physical(&m,2,a.high)?)?,0x1234567812345678);
    vm.write(&mut m,0,a.low,&old)?;
    // A successful but unpublished construction can also be aborted before
    // Memory releases backing. No inherited page retains a dangling link.
    vm.restore_boot_tables(&mut m)?;assert_eq!(before,m.snapshot(&ranges(&r)));
    assert_eq!(vm.firmware_root(),0);assert!(vm.pte(&m,2,graph::BASE+0x20000).is_err());
    assert!(vm.boot_context_roots(0).is_err());assert!(vm.boot_context_roots(1).is_err());
    vm.restore_boot_tables(&mut m)?;
    // Management-phase ownership keeps partial mappings until shutdown.
    // Simulate context publication, then exercise the stopped cleanup path.
    let mut partial=boot(&r);partial.zero(r.regions[0].base,64*16)?;
    let original=partial.snapshot(&ranges(&r));let mut pending=Vm::prepare_g16c(&mut partial,&r)?;
    partial.fail_after=Some(calls/2);assert!(pending.populate_g16c(&mut partial,&image).is_err());
    assert_ne!(original,partial.snapshot(&ranges(&r)));
    let roots=pending.boot_context_roots(0)?;
    partial.write64(r.regions[0].base,roots[0]|1)?;partial.write64(r.regions[0].base+8,roots[1]|1)?;
    assert!(pending.invalidate_g16c_boot_contexts(&mut partial,r.regions[0].base)?);
    pending.restore_boot_tables(&mut partial)?;assert_eq!(original,partial.snapshot(&ranges(&r)));
    // Observed on the real Max: management boot connects the shared L2
    // after the cold inventory. Refresh traverses old root snapshots again,
    // while rollback must still restore the original unlinked root bytes.
    for kind in 0..3 {
        let mut live=boot(&r);let root=r.regions[1].base;
        live.write64(root+16,0)?;
        let original=live.snapshot(&ranges(&r));
        let mut owner=Vm::prepare_g16c(&mut live,&r)?;
        let linked=match kind {0=>r.regions[2].base,1=>r.regions[3].base,_=>root};
        live.write64(root+16,linked|3)?;
        let result=owner.refresh_g16c_boot_tables(&live,&r);
        if kind==0 {
            result?;owner.refresh_g16c_boot_tables(&live,&r)?;
            owner.populate_g16c(&mut live,&image)?;
            assert!(owner.physical(&live,2,graph::BASE+0x20000).is_ok());
            assert!(owner.refresh_g16c_boot_tables(&live,&r).is_err());
        } else {assert!(result.is_err());}
        owner.restore_boot_tables(&mut live)?;
        assert_eq!(original,live.snapshot(&ranges(&r)));
    }
    // RTKit can allocate shared buffers after prepare and before graphics
    // population. They must survive graph construction with the same backing.
    let mut managed=boot(&r);let original=managed.snapshot(&ranges(&r));
    let mut owner=Vm::prepare_g16c(&mut managed,&r)?;let rtkit=0xfffffc2380000000;
    owner.ensure_firmware(&mut managed,rtkit,0x8000)?;
    owner.write(&mut managed,2,rtkit,&vec![0x5a;0x8000])?;
    let physical=owner.physical(&managed,2,rtkit)?;
    owner.populate_g16c(&mut managed,&image)?;
    assert_eq!(owner.physical(&managed,2,rtkit)?,physical);
    assert_eq!(managed.read(physical,0x4000)?,vec![0x5a;0x4000]);
    let second=owner.physical(&managed,2,rtkit+0x4000)?;assert_eq!(managed.read(second,0x4000)?,vec![0x5a;0x4000]);
    owner.restore_boot_tables(&mut managed)?;assert_eq!(original,managed.snapshot(&ranges(&r)));
    // Representative early, middle and final allocation failures. Every
    // borrowed table and TTBS byte must be restored before backing can drop.
    let limits=[0,3,4,8,calls/2,calls-1];
    for limit in limits {
        let mut m=boot(&r);let before=m.snapshot(&ranges(&r));m.fail_after=Some(limit);
        assert!(Vm::build_g16c(&mut m,&r,&image).is_err());assert_eq!(before,m.snapshot(&ranges(&r)));
    }
    // An inherited collision after many successful mappings also rolls back.
    let mut bad=boot(&r);let va=graph::BASE+graph::BUNDLE_OFFSET as u64;let leaf=r.regions[1].base+5*0x4000;
    bad.write64(r.regions[2].base+((va>>25)&2047)*8,leaf|3)?;
    bad.write64(leaf+((va>>14)&2047)*8,r.regions[4].base|0x00c0000000000443)?;
    let before=bad.snapshot(&ranges(&r));assert!(Vm::build_g16c(&mut bad,&r,&image).is_err());assert_eq!(before,bad.snapshot(&ranges(&r)));
    // Invalid descriptor/table ownership/cycles are rejected without changes.
    for target in [r.regions[1].base,r.regions[3].base,r.regions[4].base,0x12340000] {
        let mut bad=boot(&r);bad.write64(r.regions[1].base+24,target|3)?;
        let before=bad.snapshot(&ranges(&r));assert!(Vm::build_g16c(&mut bad,&r,&image).is_err());assert_eq!(before,bad.snapshot(&ranges(&r)));
    }
    std::fs::write(directory.join("metadata.bin"),metadata).unwrap();
    std::fs::write(directory.join("allocations.txt"),format!("{calls}\\n")).unwrap();
    println!("PASS actual common UAT walker: owned Max graph, aliases, MMIO, preserved boot trees, isolated roots, RTKit buffer interleaving, successful abort, 12 failure paths and stopped-context cleanup");Ok(())
}
'''
    (tmp/'main.rs').write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'main.rs'),*flags,'-o',str(tmp/'gate')],check=True)
    subprocess.run([str(tmp/'gate'),str(tmp/'images')],check=True)
    expected['metadata.bin']=b''.join(struct.pack('<QQ',address+off,flags) for address,size,flags in spans for off in range(0,size,0x4000))
    for name,want in expected.items():assert (tmp/'images'/name).read_bytes()==want,name
    summary={'complete_RAM_images':36,'owned_RAM_bytes':sum(s[1] for s in spans),
             'alias_spans':8,'alias_pages':283,'rollback_or_untouched_rejections':12,'successful_cold_abort':True,
             'management_failure_retains_tables_until_stopped':True,'stopped_context_clearing':True,
             'RTKit_buffers_before_graph_population':True,
             'post_management_table_link_refresh_cases':3,
             'pre_management_root_snapshot_preserved':True,
             'allocation_calls':int((tmp/'images/allocations.txt').read_text()),
             'native_execution_qualified':False,'no_hardware_access':True,
             'images':{k:{'bytes':len(v),'sha256':hashlib.sha256(v).hexdigest()} for k,v in expected.items()}}
    if args.evidence:
        args.evidence.mkdir(parents=True,exist_ok=False)
        for k,v in expected.items():(args.evidence/k).write_bytes(v)
        (args.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
