#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare real Max startup DATA transactions with actual Python methods.

Uses the common production UAT walker; ASID invalidation is recorded by a host
CPU primitive. Firmware responses are simulated, so this is not GPU execution.
"""
import argparse
import ctypes as C
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
from types import SimpleNamespace as NS

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
from m1n1.g16c.startup import Bootstrap
from m1n1.g16c.events import EventService
inputs=json.loads(args.inputs.read_text())
props={k:bytes.fromhex(v) for k,v in inputs['properties'].items()}
p=Platform(inputs['sgx_base'],inputs['sgx_span'],inputs['revision'],props,inputs['core'],inputs['sram'])
b,ram=reference.build(args.m1n1,p,0)
b.ttbs=ram.memalign(0x4000,0x4000);b.spaces={0:b.client};b.borrowed_contexts={0:(0,0)}
instructions=[];ram.inst=lambda *args,**kwargs:instructions.append(args)
messages=[];b.asc.send_endpoint=lambda ep,msg:messages.append((ep,msg))
def received(predicate,timeout,description):
    if 'acceptance' in description:b.write32(b.root.status_a+0x14,1)
    else:
        target=7 if 'maintenance' in description else 1
        for va in (b.main.channels[12].read,b.main.channels[12].write,b.main.channels[12].completion):b.write32(va,target)
    assert predicate(),description
b.wait_received=received
Bootstrap.publish(b);Bootstrap.open_control(b)
spans=reference.spans(b,ram)
expected={f'opened-{i}.bin':b.firmware.read(address,size) for i,(address,size,_) in enumerate(spans)}
assert messages==[(0x20,0x00810c2000100000),(0x21,0x0084000000000011),(0x21,0x0084000000000011)]
assert any(op[0]==0xd5088140 for op in instructions)
# Ring wrap, trace and auxiliary cursor handling by the real source service.
base=0xfffffc2000000000
for state,consumer,producer in [(0x3f1c0,255,1),(0x3f3c0,511,1),(0x3f400,3,9)]:
    b.write32(base+state,consumer);b.write32(base+state+0x20,producer)
for slot in [255,0]:b.write32(base+0x3f440+slot*0x48,1)
EventService(b.firmware).drain()
expected.update({f'drained-{i}.bin':b.firmware.read(address,size) for i,(address,size,_) in enumerate(spans)})
service=EventService(b.firmware)
receipt=service.expect_flist(7,0xfffffc20c3000000,0x7001000000,5,previous=23,reply_value=0)
reply=struct.pack('<IIIQQQI',13,7,0,0xfffffc20c3000000,0x7001000000,0x7001000000+23*64,40)+bytes(32)
b.write32(base+0x3f1c0,0);b.write32(base+0x3f1e0,1);b.firmware.write(base+0x3f440,reply)
service.drain();assert receipt.received
expected.update({f'admitted-{i}.bin':b.firmware.read(address,size) for i,(address,size,_) in enumerate(spans)})
b.failed=False;b.published=True
with tempfile.TemporaryDirectory() as artifact:
    b.asc.crash_directory=Path(artifact)
    Bootstrap.activate_render_status(b)
expected.update({f'render-active-{i}.bin':b.firmware.read(address,size) for i,(address,size,_) in enumerate(spans)})
with tempfile.TemporaryDirectory(prefix='scheduled-bootstrap-') as directory:
    tmp=Path(directory);flags=host_versions.build(tmp)
    loader=host_fdt.Loader(args.m1n1,tmp,args.fdt_template)
    dt,incoming,owners=loader.prepare(p)
    regions=[(int.from_bytes(props[n+'-base'],'little'),int.from_bytes(props[n+'-size'],'little')) for n in
             ['gpu-region','gfx-shared-region','gfx-shared-l2-region','gfx-handoff','gfx-data']]
    values=(host_fdt.Region*5)(*[host_fdt.Region(*r) for r in regions])
    assert loader.populate_memory(dt,loader.path(dt,b'/gpu'),values,0xfffffc0000000000,0x2000000000)==0
    assert loader.populate(dt,loader.path(dt,b'/gpu'),C.byref(incoming))==0
    node=loader.snapshot(dt,loader.path(dt,b'/gpu'),True)
    code=host_vm.prelude()+'''
mod g17p_user_vm {
    pub static INVALIDATED:std::sync::Mutex<Vec<u16>>=std::sync::Mutex::new(Vec::new());
    pub struct UserVm;
    impl UserVm {pub fn invalidate(slot:u16){INVALIDATED.lock().unwrap().push(slot);}}
}
'''
    code+='mod g17p_render { #[derive(Debug)] pub enum Error {Invalid,Overflow}\n'
    code+=f'#[path={json.dumps(str(tests.parent/"scheduled_render_admission.rs"))}] pub mod admission; }}\n'
    for name in ['float','scheduled_initdata','scheduled_boot_records','scheduled_hwdata',
                 'scheduled_board','scheduled_power','scheduled_initgraph','scheduled_mmio',
                 'scheduled_resources','scheduled_image','g17p_abi','g17p_layout',
                 'g17p_initgraph','g17p_opening','g17p_topology','g17p_vm','scheduled_vm_g16c','scheduled_bootstrap']:
        code+=f'#[path={json.dumps(str(tests.parent/(name+".rs")))}] mod {name};\n'
    code+='''
use prelude::*;use g17p_memory::Memory;use g17p_vm::Vm;use scheduled_bootstrap as boot;
const BASE:u64=scheduled_initgraph::BASE;
fn write32(m:&mut Memory,v:&Vm,at:u64,value:u32)->Result{v.write(m,2,at,&value.to_le_bytes())}
fn dump(m:&Memory,v:&Vm,out:&std::path::Path,prefix:&str)->Result {
    for (i,o) in scheduled_initgraph::OBJECT_LAYOUT.iter().enumerate(){
        let mut bytes=vec![];for offset in (0..o.size).step_by(0x4000){bytes.extend_from_slice(&m.read(v.physical(m,2,BASE+o.offset as u64+offset as u64)?,0x4000)?);}
        std::fs::write(out.join(format!("{prefix}-{i}.bin")),bytes).unwrap();
    }Ok(())
}
fn main()->Result {
    let out=std::path::PathBuf::from(std::env::args().nth(1).unwrap());std::fs::create_dir_all(&out).unwrap();
'''
    code+='let node='+host_fdt.rust_node(node)+';\n'
    code+='''
    let r=scheduled_resources::ResourcesG16C::from_fdt(&node)?;
    let mut m=Memory::new();for x in &r.regions[..4]{m.reserve(x.base,x.size);}
    m.write64(r.regions[1].base+16,r.regions[2].base|3)?;
    let image=scheduled_image::ImageG16C::new(&node,0)?;
    let mut v=Vm::build_g16c(&mut m,&r,&image)?;
    assert_eq!(boot::ROOT_MESSAGE,0x00810c2000100000);assert_eq!(boot::CONTROL_MESSAGE,0x0084000000000011);
    // A stale context must fail before exposing producer 1 or changing TTBS.
    m.write64(r.regions[0].base,0x12345678)?;
    assert_eq!(boot::prepare_publication(&mut m,&v,r.regions[0].base),Err(EBUSY));
    assert_eq!(m.read64(r.regions[0].base)?,0x12345678);
    assert!(boot::control_retired(&m,&v,0)?);
    m.write64(r.regions[0].base,0)?;
    assert!(!boot::accepted(&m,&v)?);boot::prepare_publication(&mut m,&v,r.regions[0].base)?;
    let roots=v.boot_context_roots(0)?;assert_eq!(m.read64(r.regions[0].base)?,roots[0]|1);assert_eq!(m.read64(r.regions[0].base+8)?,roots[1]|1);
    assert_eq!(*g17p_user_vm::INVALIDATED.lock().unwrap(),vec![0]);
    assert_eq!(m.read64(r.regions[0].base+16)?,0);assert_eq!(m.read64(r.regions[0].base+24)?,0);
    write32(&mut m,&v,BASE+0x3f194,2)?;assert!(!boot::accepted(&m,&v)?);
    let before=[m.read_firmware32(v.physical(&m,2,BASE+0x3eac0)?)?,m.read_firmware32(v.physical(&m,2,BASE+0x3f19c)?)?];
    assert_eq!(boot::activate_render_status(&mut m,&v),Err(EIO));
    assert_eq!(before,[m.read_firmware32(v.physical(&m,2,BASE+0x3eac0)?)?,m.read_firmware32(v.physical(&m,2,BASE+0x3f19c)?)?]);
    write32(&mut m,&v,BASE+0x3f194,1)?;assert!(boot::accepted(&m,&v)?);
    boot::prepare_control(&mut m,&v)?;
    // A producer alone is not retirement. All independent cursors must agree.
    assert!(!boot::control_retired(&m,&v,1)?);
    for o in [0,0x10]{write32(&mut m,&v,BASE+0x20300+o,1)?;}
    assert!(boot::control_retired(&m,&v,1)?);
    boot::prepare_maintenance(&mut m,&v)?;assert!(!boot::control_retired(&m,&v,7)?);
    for o in [0,0x10]{write32(&mut m,&v,BASE+0x20300+o,7)?;}
    assert!(boot::control_retired(&m,&v,7)?);dump(&m,&v,&out,"opened")?;
    for (state,consumer,producer) in [(0x3f1c0,255,1),(0x3f3c0,511,1),(0x3f400,3,9)] {
        write32(&mut m,&v,BASE+state,consumer)?;write32(&mut m,&v,BASE+state+0x20,producer)?;
    }
    for slot in [255,0]{write32(&mut m,&v,BASE+0x3f440+slot*0x48,1)?;}
    boot::drain(&mut m,&v)?;dump(&m,&v,&out,"drained")?;
    let state=BASE+0x3f1c0;let data=BASE+0x3f440;
    let registration=g17p_render::admission::RegistrationG16C{context:7,support:0xfffffc20c3000000,
        table:0x7001000000,added:5,previous:23,bank:0,tag:19};
    let (_,mut receipt)=registration.prepare().unwrap();
    let mut reply=[0u8;0x48];
    for(at,value)in[(0,13u32),(4,7),(8,0),(36,40)]{reply[at..at+4].copy_from_slice(&value.to_le_bytes());}
    for(at,value)in[(12,0xfffffc20c3000000u64),(20,0x7001000000),(28,0x7001000000+23*64)]{reply[at..at+8].copy_from_slice(&value.to_le_bytes());}
    // Every byte in the owned comparison fields is significant. A mismatch
    // must retain both the pending owner and the RX consumer.
    for at in 0..40usize {
        let (_,mut pending)=registration.prepare().unwrap();let mut bad=reply;bad[at]^=1;
        write32(&mut m,&v,state,0)?;write32(&mut m,&v,state+0x20,1)?;v.write(&mut m,2,data,&bad)?;
        assert_eq!(boot::drain_with_flist(&mut m,&v,Some(&mut pending)),Err(EIO));
        assert!(!pending.received);assert_eq!(m.read_firmware32(v.physical(&m,2,state)?)?,0);
    }
    v.write(&mut m,2,data,&reply)?;
    boot::drain_with_flist(&mut m,&v,Some(&mut receipt))?;assert!(receipt.received);
    dump(&m,&v,&out,"admitted")?;
    boot::activate_render_status(&mut m,&v)?;dump(&m,&v,&out,"render-active")?;
    // Slot 41 straddles UAT leaves: RX DATA must walk each physical page.
    let (_,mut crossing)=registration.prepare().unwrap();
    write32(&mut m,&v,state,41)?;write32(&mut m,&v,state+0x20,42)?;
    v.write(&mut m,2,data+41*0x48,&reply)?;
    boot::drain_with_flist(&mut m,&v,Some(&mut crossing))?;assert!(crossing.received);
    write32(&mut m,&v,state,0)?;
    assert_eq!(boot::drain_with_flist(&mut m,&v,Some(&mut receipt)),Err(EIO));
    assert_eq!(m.read_firmware32(v.physical(&m,2,state)?)?,0);
    // Unknown/unowned events and timeouts stay unconsumed.
    for kind in [0,4,6,7,13,u32::MAX] {
        write32(&mut m,&v,state,0)?;write32(&mut m,&v,state+0x20,1)?;write32(&mut m,&v,data,kind)?;
        assert_eq!(boot::drain(&mut m,&v),Err(if kind==4{ETIMEDOUT}else{EIO}));
        assert_eq!(m.read_firmware32(v.physical(&m,2,state)?)?,0);
    }
    for (consumer,producer) in [(256,0),(0,256),(u32::MAX,1)] {
        write32(&mut m,&v,state,consumer)?;write32(&mut m,&v,state+0x20,producer)?;assert_eq!(boot::drain(&mut m,&v),Err(EIO));
    }
    assert_eq!(boot::publish_context(&mut m,&v,r.regions[0].base,2),Err(EINVAL));
    v.restore_boot_tables(&mut m)?;
    println!("PASS Max bootstrap DATA: full source publication/opening/RX images, context guard, acceptance, independent retirement and unowned-event rejection");Ok(())
}
'''
    (tmp/'main.rs').write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'main.rs'),*flags,'-o',str(tmp/'gate')],check=True)
    subprocess.run([str(tmp/'gate'),str(tmp/'images')],check=True)
    for name,want in expected.items():
        actual=(tmp/'images'/name).read_bytes()
        assert actual==want,(name,[(hex(i),a,b) for i,(a,b) in enumerate(zip(actual,want)) if a!=b][:12])
    summary={'whole_source_RAM_images':len(expected),'source_messages':messages,
             'source_publication_and_control_methods_executed':True,'source_EventService_executed':True,
             'host_ASID_primitive_recorded':True,'simulated_firmware_responses':True,
             'owned_FList_receipt_matching':True,'wrong_owned_reply_bytes_retained':40,
             'duplicate_FList_reply_retained':True,
             'native_GPU_or_barrier_execution_qualified':False,
             'images':{k:{'bytes':len(v),'sha256':hashlib.sha256(v).hexdigest()} for k,v in expected.items()}}
    if args.evidence:
        args.evidence.mkdir(parents=True,exist_ok=False)
        for k,v in expected.items():(args.evidence/k).write_bytes(v)
        (args.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
