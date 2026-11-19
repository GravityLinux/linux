#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute production Max resource allocation against real source allocation.

The test models RAM and low/root PTE APIs; neither CPU cache operations nor GPU
execution is simulated as proof. All virtual DATA bytes and attributes compare.
"""
import argparse,ctypes as C,hashlib,json,struct,subprocess,sys,tempfile
from pathlib import Path
from types import SimpleNamespace as NS
import host_fdt,host_vm,host_versions,g16c_graph_reference as reference
p=argparse.ArgumentParser(description=__doc__);p.add_argument('m1n1',type=Path);p.add_argument('--inputs',type=Path,required=True);p.add_argument('--evidence',type=Path,required=True);p.add_argument('--recycle',action='store_true')
a=p.parse_args();tests=Path(__file__).resolve().parent;sys.path.insert(0,str(a.m1n1/'proxyclient'))
from m1n1.g16c.platform import Platform
from m1n1.g16c.memory import AddressSpace,PAGE_SIZE
from m1n1.g16c.arena import Arena
from m1n1.g16c.render_runtime import RenderResources
from m1n1.g16c.render import Parameters
j=json.loads(a.inputs.read_text());props={k:bytes.fromhex(v)for k,v in j['properties'].items()}
platform=Platform(j['sgx_base'],j['sgx_span'],j['revision'],props,j['core'],j['sram'])
b,ram=reference.build(a.m1n1,platform,0)
ram.data.extend(bytes(448*1024*1024));ram.ba.mem_size_actual=len(ram.data)
space=AddressSpace(ram,1,ram.memalign(PAGE_SIZE,PAGE_SIZE),ram.memalign(PAGE_SIZE,PAGE_SIZE))
vm=NS(identifier=1,space=space)
kernel=Arena(space,0x7000000000,0x7020000000)
device=NS(bootstrap=b,vm_arenas={1:kernel},render_resources={},render_parameter_owners={},
    unpublished_builds=[],render_parameter_sequences={},progress=lambda *args:None)
source=RenderResources.allocate(device,NS(vm=vm))
device.drain=lambda vm:True
geometry=source.fit_geometry(Parameters(width=1,height=1,context_base=0x1000000000,
    tilemap=0x1000000000,heapmeta=0x1000000000,tpc=0x1000000000,
    deflake_1=0x1000000000,deflake_2=0x1000000000,deflake_3=0x1000000000,encoder=0x1000000000),device)
assert geometry.tpc_size==8192
expected=[]
for context,memory,allocations in [(1,space,None),(2,b.firmware,source.metadata.allocations)]:
    owners=space.mappings.values() if allocations is None else allocations
    for owner in owners:
        for va in range(owner.address,owner.address+owner.size,PAGE_SIZE):
            expected.append((context,va,owner.attributes,memory))
expected.sort(key=lambda row:row[:2])
private_source=(tests.parent/'scheduled_compute_runtime.rs').read_text()
def item(marker,private_source=private_source):
    start=private_source.index(marker);brace=private_source.index('{',start);at=brace+1;depth=1
    while depth:
        if private_source[at]=='{':depth+=1
        elif private_source[at]=='}':depth-=1
        at+=1
    return ('#[derive(Clone,Copy)]\n' if marker=='struct PrivateBlock' else '')+private_source[start:at]
with tempfile.TemporaryDirectory()as directory:
    tmp=Path(directory);flags=host_versions.build(tmp)
    code=host_vm.prelude().replace('pub const EEXIST:','pub const EOVERFLOW:Error=Error(75);pub const ENOSPC:Error=Error(28);pub const EEXIST:')
    code+='mod g17p_initgraph{pub struct Graph;}\nmod scheduled_initgraph{pub const BASE:u64=0xfffffc2000000000;}\n'
    code+='mod g17p_render{#[derive(Debug)]pub enum Error{Invalid,Overflow}\n'
    for name,file in [('resources','scheduled_render_resources.rs'),('admission','scheduled_render_admission.rs'),('growth','scheduled_parameter_growth.rs')]:
        code+=f'#[path={json.dumps(str(tests.parent/file))}]pub mod {name};\n'
    code+='}\n'+f'#[path={json.dumps(str(tests.parent/"scheduled_operands.rs"))}]mod scheduled_operands;\n'
    code+=f'#[path={json.dumps(str(tests.parent/"scheduled_dispatch.rs"))}]mod scheduled_dispatch;\n'
    code+='''
mod g17p_user_vm {
    use super::prelude::*;use std::collections::BTreeMap;
    pub struct UserVm{pub maps:BTreeMap<u64,u64>,pub identity:u64}
    pub struct RootLease{root:u64}
    impl RootLease{pub fn root(&self)->u64{self.root}pub fn refresh(&mut self,r:&UserVm)->Result{if r.root()!=self.root{return Err(EIO);}Ok(())}}
    impl UserVm{
        pub fn root(&self)->u64{self.identity}pub fn invalidate(_:u16){}
        pub fn lease(&self)->Result<RootLease>{Ok(RootLease{root:self.root()})}
        pub fn prepare(&mut self,va:u64,size:u64)->Result{for off in (0..size).step_by(0x4000){if self.pte(va+off)?!=0{return Err(EBUSY);}}Ok(())}
        pub fn map_page(&mut self,va:u64,pa:u64,write:bool)->Result{if self.pte(va)?!=0{return Err(EBUSY);}self.maps.insert(va,pa|0x0080000000000c8b|if write{1<<54}else{0});Ok(())}
        pub fn pte(&self,va:u64)->Result<u64>{Ok(self.maps.get(&(va&!0x3fff)).copied().unwrap_or(0))}
    }
}
mod g17p_vm{
    use super::{prelude::*,g17p_memory::Memory};use std::collections::BTreeMap;
    pub struct Vm{pub maps:BTreeMap<u64,u64>}
    impl Vm{
        pub fn physical(&self,m:&Memory,g:usize,va:u64)->Result<u64>{let p=self.pte(m,g,va)?;if p==0{return Err(EIO);}Ok((p&0x000003ffffffc000)+(va&0x3fff))}
        pub fn pte(&self,_:&Memory,_:usize,va:u64)->Result<u64>{Ok(self.maps.get(&(va&!0x3fff)).copied().unwrap_or(0))}
        pub fn prepare_g16c_empty(&mut self,m:&mut Memory,g:usize,va:u64,n:usize)->Result{for o in (0..n).step_by(0x4000){if self.pte(m,g,va+o as u64)?!=0{return Err(EBUSY);}}Ok(())}
        pub fn span(&mut self,_:&mut Memory,_:usize,va:u64,pa:u64,n:usize,flags:u64)->Result{for o in (0..n).step_by(0x4000){if self.maps.contains_key(&(va+o as u64)){return Err(EBUSY);}self.maps.insert(va+o as u64,pa+o as u64|flags);}Ok(())}
        pub fn write(&self,m:&mut Memory,_:usize,va:u64,b:&[u8])->Result{let mut at=0;while at<b.len(){let v=va+at as u64;let n=(0x4000-(v&0x3fff)as usize).min(b.len()-at);let pte=self.pte(m,2,v)?;if pte==0{return Err(EIO);}let pa=(pte&0x000003ffffffc000)+(v&0x3fff);m.write(pa,&b[at..at+n])?;at+=n;}Ok(())}
        pub fn flush_tables(&self,_:&Memory)->Result{Ok(())}pub fn invalidate_gpu(){}
    }
}
mod g17p_boot{
    use super::{prelude::*,g17p_memory::Memory,g17p_vm::Vm,g17p_render::admission::RegistrationG16C};
    pub struct Session{pub memory:Option<Memory>,pub vm:Option<Vm>,pub ttbs:u64,pub requests:Vec<Vec<u8>>}
    impl Session{pub fn g16c_register_flist(&mut self,r:&RegistrationG16C)->Result{let(req,mut receipt)=r.prepare().unwrap();let mut reply=[0u8;72];for(at,value)in[(0,13u32),(4,r.context),(8,r.bank),(36,r.added*8)]{reply[at..at+4].copy_from_slice(&value.to_le_bytes());}for(at,value)in[(12,r.support),(20,r.table),(28,r.table+r.previous as u64*64)]{reply[at..at+8].copy_from_slice(&value.to_le_bytes());}assert!(receipt.consume(&reply));self.requests.push(req.bytes().to_vec());Ok(())}}
    pub mod g16c_compute{
        use crate::{prelude::*,g17p_memory::Memory,g17p_vm::Vm,g17p_user_vm::UserVm,scheduled_operands as operands};use super::Session;
        mod boot{use super::*;pub fn read_words(m:&Memory,v:&Vm,va:u64,out:&mut[u8])->Result{let mut off=0;while off<out.len(){let at=va+off as u64;let n=(out.len()-off).min(0x4000-(at&0x3fff)as usize);m.read_firmware_words(v.physical(m,2,at)?,&mut out[off..off+n])?;off+=n;}Ok(())}}
        use crate::scheduled_dispatch::DispatchStateG16C;
        mod compute {pub struct Client {pub root:crate::g17p_user_vm::UserVm}}
        const PAGE:u64=0x4000;const PRIVATE:u64=0x00c0000000000443;const SHARED:u64=0x00c000000000044b;
        const GPU_READ:u64=0x0080000000000c8b;const ADDRESS:u64=0x000003ffffffc000;
'''
    for marker in ['struct PrivateBlock','fn bytes(','fn private_span(','fn map_private(','fn map_prepared_private(','fn validate_mapped_private(','fn reset_mapped_private(','fn write_blocks(','fn write_low(','fn fw_span(','pub(crate) struct OwnerPrivateG16C','impl OwnerPrivateG16C','pub(crate) struct WorkBackingG16C','impl WorkBackingG16C','fn owner_layout(','fn encode_operand_directory(','fn initialize_operand_data(','fn initialize_operand_support(']:code+=item(marker)+'\n'
    owner_reference_revision='d138dcd9c'
    old_owner=subprocess.check_output(['git','show',owner_reference_revision+':drivers/gpu/drm/asahi/scheduled_compute_runtime.rs'],cwd=tests.parent,text=True)
    code+=item('fn initialize_operand_data(',old_owner).replace('fn initialize_operand_data(', 'fn original_initialize_operand_data(')+'\n'
    code+=f'#[path={json.dumps(str(tests.parent/"scheduled_render_runtime.rs"))}]mod render;\n'
    code+='''
        fn owner_private_gate()->Result {
            for base in [0x7000000000u64,0x10e0000000] {
                let limit=base+0x20000000;
                let mut caller=compute::Client{root:UserVm{maps:std::collections::BTreeMap::new(),identity:0x21000000000}};
                let mut plan=OwnerPrivateG16C::build(&mut caller,base,limit)?;
                assert!(plan.matches(&caller,base,limit));assert!(caller.root.maps.is_empty());
                assert!(!plan.matches(&caller,base+0x4000,limit));assert!(!plan.matches(&caller,base,limit-0x4000));
                caller.root.identity+=0x4000;assert!(!plan.matches(&caller,base,limit));caller.root.identity-=0x4000;
                let calls=plan.memory.calls;
                for failed in [0,1,8,calls-1] {
                    crate::g17p_memory::DETACHED_FAIL_AFTER.store(failed,std::sync::atomic::Ordering::Relaxed);
                    assert_eq!(OwnerPrivateG16C::build(&mut caller,base,limit).err(),Some(ENOMEM));
                    assert!(caller.root.maps.is_empty());
                }
                crate::g17p_memory::DETACHED_FAIL_AFTER.store(usize::MAX,std::sync::atomic::Ordering::Relaxed);
                let mut reference=Memory::new();
                for block in &plan.private {reference.reserve(block.physical,block.size);}
                for physical in [plan.support_pages[0],plan.support_pages[1],plan.high_root] {reference.reserve(physical,PAGE);}
                map_prepared_private(&mut caller.root,base,&plan.private)?;
                let mut vm=Vm{maps:std::collections::BTreeMap::new()};let support=0xfffffc2207000000;
                for (i,physical)in plan.support_pages.iter().enumerate() {vm.span(&mut plan.memory,2,support+i as u64*PAGE,*physical,PAGE as usize,PRIVATE)?;}
                let mut directory=bytes(PAGE as usize)?;let mut entries=bytes(operands::TABLE_SIZE as usize)?;
                let expected=original_initialize_operand_data(&mut reference,&vm,&caller.root,base,support,&plan.operand_buffers,&mut directory,&mut entries)?;
                let actual=initialize_operand_support(&mut plan.memory,&vm,base,support,plan.operand_buffers.len())?;
                assert_eq!(actual,expected);
                for block in &plan.private {for offset in (0..block.size).step_by(PAGE as usize) {
                    assert_eq!(plan.memory.read(block.physical+offset,PAGE as usize)?,reference.read(block.physical+offset,PAGE as usize)?,"owner private base={base:#x} offset={:#x}",block.offset+offset);
                }}
                for physical in [plan.support_pages[0],plan.support_pages[1],plan.high_root] {assert_eq!(plan.memory.read(physical,PAGE as usize)?,reference.read(physical,PAGE as usize)?);}
            }
            return Ok(());
        }
        pub fn run()->Result{
            owner_private_gate()?;
            let work=WorkBackingG16C::new()?;
            assert_eq!(work.memory.calls,7);
            for &page in &work.pages {assert_eq!(work.memory.read(page,PAGE as usize)?,vec![0;PAGE as usize]);}
            drop(work);
            for failed in 0..7 {
                crate::g17p_memory::DETACHED_FAIL_AFTER.store(failed,std::sync::atomic::Ordering::Relaxed);
                assert_eq!(WorkBackingG16C::new().err(),Some(ENOMEM));
            }
            crate::g17p_memory::DETACHED_FAIL_AFTER.store(usize::MAX,std::sync::atomic::Ordering::Relaxed);
            let mut m=Memory::new();let mut v=Vm{maps:std::collections::BTreeMap::new()};
            let mut root=UserVm{maps:std::collections::BTreeMap::new(),identity:0x20000000000};
            let pa=m.allocate(0x4000)?;v.span(&mut m,2,crate::scheduled_initgraph::BASE+0x1548000,pa,0x4000,PRIVATE)?;
            let before_vm=v.maps.clone();let before_root=root.maps.clone();let before_calls=m.calls;
            let (staged,mut probe)=render::ResourcesG16C::build(0,0,0x7000000000,0x7020000000)?;
            assert_eq!(v.maps,before_vm);assert_eq!(root.maps,before_root);assert_eq!(m.calls,before_calls);
            probe.preflight(&mut m,&mut v,&mut root)?;
            let complete_calls=staged.calls;drop(staged);drop(probe);
            let saved_maps=v.maps.clone();let saved_calls=m.calls;let seed=m.read(pa,0x4000)?;
            for failed in [0,1,8,complete_calls-1] {
                crate::g17p_memory::DETACHED_FAIL_AFTER.store(failed,std::sync::atomic::Ordering::Relaxed);
                assert_eq!(render::ResourcesG16C::prepare(&mut m,&mut v,&mut root,0,0,0x7000000000,0x7020000000).err(),Some(ENOMEM));
                assert_eq!(v.maps,saved_maps);assert!(root.maps.is_empty());assert_eq!(m.calls,saved_calls);assert_eq!(m.read(pa,0x4000)?,seed);
            }
            crate::g17p_memory::DETACHED_FAIL_AFTER.store(usize::MAX,std::sync::atomic::Ordering::Relaxed);
            let mut r=render::ResourcesG16C::allocate(&mut m,&mut v,&mut root,0,0,0x7000000000,0x7020000000)?;
            let geometry_maps=root.maps.clone();let geometry_calls=m.calls;
            for failed in 0..3 {
                crate::g17p_memory::DETACHED_FAIL_AFTER.store(failed,std::sync::atomic::Ordering::Relaxed);
                assert_eq!(r.prepare_geometry(&mut root,[1280,8192,512]).err(),Some(ENOMEM));
                assert_eq!(root.maps,geometry_maps);assert_eq!(m.calls,geometry_calls);assert_eq!(r.geometry_addresses(),[0;3]);
            }
            crate::g17p_memory::DETACHED_FAIL_AFTER.store(usize::MAX,std::sync::atomic::Ordering::Relaxed);
            let plan=r.geometry_plan(&root,[1280,8192,512])?.unwrap();
            let before=root.maps.clone();let before_calls=m.calls;
            let (unpublished,mut ready)=plan.prepare(root.lease()?)?;
            assert_eq!(root.maps,before);assert_eq!(m.calls,before_calls);
            let guard=0x1001004000;assert!(root.maps.insert(guard,1).is_none());
            assert!(!r.preflight_geometry(&mut root,&plan,&mut ready)?);assert_eq!(root.maps.remove(&guard),Some(1));assert_eq!(root.maps,before);
            let mut foreign=UserVm{maps:root.maps.clone(),identity:root.identity+0x4000};
            assert!(!r.preflight_geometry(&mut foreign,&plan,&mut ready)?);assert_eq!(foreign.maps,before);
            let geometry=r.fit_geometry(&mut m,&mut root,[1280,8192,512])?;
            let installed=root.maps.clone();let installed_calls=m.calls;
            assert!(!r.preflight_geometry(&mut root,&plan,&mut ready)?);assert_eq!(root.maps,installed);assert_eq!(m.calls,installed_calls);
            drop(unpublished);drop(ready);
            assert_eq!(geometry,[0x1001000000,0x1001008000,0x1001010000]);
            let allocations=m.calls;assert_eq!(r.fit_geometry(&mut m,&mut root,[1280,8192,512])?,geometry);assert_eq!(m.calls,allocations);
            let ttbs=m.allocate(0x4000)?;let high=m.allocate(0x4000)?;
            m.write64(ttbs+16,root.root()|(1<<48)|1)?;m.write64(ttbs+24,high|(1<<48)|1)?;
            let mut session=Session{memory:Some(m),vm:Some(v),ttbs,requests:vec![]};
            // Last alias mismatch and a foreign installed root fail BEFORE
            // the one modeled firmware registration, retaining all owners.
            let alias=crate::g17p_render::resources::GPU_PAGE_LIST+0x4c000;
            let old=root.maps.insert(alias,1).unwrap();assert_eq!(r.admit(&mut session,&root,1,high,0),Err(EIO));assert!(session.requests.is_empty());root.maps.insert(alias,old);
            session.memory.as_mut().unwrap().write64(ttbs+16,0)?;assert_eq!(r.admit(&mut session,&root,1,high,0),Err(EIO));assert!(session.requests.is_empty());
            session.memory.as_mut().unwrap().write64(ttbs+16,root.root()|(1<<48)|1)?;
            r.admit(&mut session,&root,1,high,0)?;r.admit(&mut session,&root,1,high,0)?;assert_eq!(session.requests.len(),1);
            let m=session.memory.as_ref().unwrap();let v=session.vm.as_ref().unwrap();
            use std::io::Write;let mut stdout=std::io::BufWriter::new(std::io::stdout().lock());
            for(context,map)in[(1u32,&root.maps),(2,&v.maps)]{for(&va,&pte)in map{
                if context==2&&!(0xfffffc20c3000000..0xfffffc20c3200000).contains(&va){continue;}
                stdout.write_all(&context.to_le_bytes()).unwrap();stdout.write_all(&va.to_le_bytes()).unwrap();stdout.write_all(&(pte&!ADDRESS).to_le_bytes()).unwrap();
                stdout.write_all(&m.read(pte&ADDRESS,0x4000)?).unwrap();
            }}
            stdout.write_all(&session.requests[0]).unwrap();
            let mut descriptor=[0u8;16];let va=crate::scheduled_initgraph::BASE+0x1548000;let pa=v.pte(m,2,va)?&ADDRESS;descriptor.copy_from_slice(&m.read(pa,16)?);stdout.write_all(&descriptor).unwrap();stdout.flush().unwrap();
            Ok(())
        }
    }
}
fn main(){g17p_boot::g16c_compute::run().unwrap();}
'''
    if a.recycle:
        code=code.replace('            Ok(())\n        }',r"""
            drop(stdout);
            let calls=session.memory.as_ref().unwrap().calls;
            let oldmaps=root.maps.clone();
            let mut fresh=UserVm{maps:std::collections::BTreeMap::new(),identity:0x20000004000};
            assert_eq!(r.rebind(session.memory.as_mut().unwrap(),session.vm.as_mut().unwrap(),&mut fresh,0x7100000000,0x7120000000),Err(EINVAL));
            for va in [r.manager+0x200,r.parameter.counter+0x20,r.scene+0x100,r.parameter.page_list+0x20] {
                session.vm.as_ref().unwrap().write(session.memory.as_mut().unwrap(),2,va,&0x123456789abcdef0u64.to_le_bytes())?;
            }
            r.sequence=2;r.detach()?;assert!(!r.attached());assert_eq!(r.sequence_base,2);assert_eq!(r.sequence,0);
            let first=root.pte(0x7000000000)?&ADDRESS;session.memory.as_mut().unwrap().write(first,&[0x5a;16])?;
            let bad=r.parameter.page_list+0x4c000;let prior=session.vm.as_mut().unwrap().maps.insert(bad,1).unwrap();
            assert_eq!(r.rebind(session.memory.as_mut().unwrap(),session.vm.as_mut().unwrap(),&mut fresh,0x7100000000,0x7120000000),Err(EIO));
            assert_eq!(session.memory.as_ref().unwrap().read(first,16)?,[0x5a;16]);
            session.vm.as_mut().unwrap().maps.insert(bad,prior);
            fresh.maps.insert(0x1001004000,1);
            assert_eq!(r.rebind(session.memory.as_mut().unwrap(),session.vm.as_mut().unwrap(),&mut fresh,0x7100000000,0x7120000000),Err(EBUSY));
            assert_eq!(session.memory.as_ref().unwrap().read(first,16)?,[0x5a;16]);fresh.maps.clear();
            r.rebind(session.memory.as_mut().unwrap(),session.vm.as_mut().unwrap(),&mut fresh,0x7100000000,0x7120000000)?;
            assert_eq!(fresh.maps.len(),oldmaps.len());
            for(&va,&pte)in &oldmaps {
                let next=if(0x7000000000..0x7020000000).contains(&va){va+0x100000000}else{va};
                assert_eq!(fresh.pte(next)?,pte);
            }
            assert_eq!(r.fit_geometry(session.memory.as_mut().unwrap(),&mut fresh,[1280,8192,512])?,geometry);
            session.memory.as_mut().unwrap().write64(ttbs+16,fresh.root()|(1<<48)|1)?;
            r.admit(&mut session,&fresh,1,high,0)?;r.admit(&mut session,&fresh,1,high,0)?;
            assert_eq!(session.requests.len(),2);assert_eq!(session.memory.as_ref().unwrap().calls,calls);
            // Reconstruct identical retired DATA through the already-mapped
            // path, without allocating backing or changing the physical root.
            r.detach()?;let identity=fresh.root();let mappings=fresh.maps.clone();
            r.rebind_mapped(session.memory.as_mut().unwrap(),session.vm.as_mut().unwrap(),&mut fresh,0x7100000000,0x7120000000)?;
            assert_eq!(fresh.root(),identity);assert_eq!(fresh.maps,mappings);
            session.requests.pop();r.admit(&mut session,&fresh,1,high,0)?;
            assert_eq!(session.memory.as_ref().unwrap().calls,calls);
            let m=session.memory.as_ref().unwrap();let v=session.vm.as_ref().unwrap();
            let mut stdout=std::io::BufWriter::new(std::io::stdout().lock());
            for(context,map)in[(1u32,&fresh.maps),(2,&v.maps)]{for(&va,&pte)in map{
                if context==2&&!(0xfffffc20c3000000..0xfffffc20c3200000).contains(&va){continue;}
                stdout.write_all(&context.to_le_bytes()).unwrap();stdout.write_all(&va.to_le_bytes()).unwrap();stdout.write_all(&(pte&!ADDRESS).to_le_bytes()).unwrap();
                stdout.write_all(&m.read(pte&ADDRESS,0x4000)?).unwrap();
            }}
            stdout.write_all(&session.requests[1]).unwrap();stdout.flush().unwrap();
            Ok(())
        }""")
    (tmp/'main.rs').write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'main.rs'),*flags,'-o',str(tmp/'gate')],check=True)
    proc=subprocess.Popen([str(tmp/'gate')],stdout=subprocess.PIPE)
    digest=hashlib.sha256();total=0
    for context,va,flags,memory in expected:
        head=proc.stdout.read(20);assert head==struct.pack('<IQQ',context,va,flags),(context,hex(va),head.hex())
        actual=proc.stdout.read(PAGE_SIZE);assert len(actual)==PAGE_SIZE
        want=memory.read(va,PAGE_SIZE)
        assert actual==want,(context,hex(va),[hex(i)for i,(a,b)in enumerate(zip(actual,want))if a!=b][:16])
        digest.update(head+actual);total+=len(head)+len(actual)
    from m1n1.g16c.firmware import register_flist,parameter_buffer_descriptor
    expected_request=register_flist(1,source.links.support,source.pool.table,5,previous=23)
    actual=proc.stdout.read(64);assert actual==expected_request
    expected_descriptor=parameter_buffer_descriptor(source.parameter.gpu_page_list,23*4)
    assert proc.stdout.read(16)==expected_descriptor
    if a.recycle:
        from m1n1.g16c.pools import OperandPool,DIRECTORY_SIZE,BUFFER_STRIDE
        delta=0x100000000
        for va in [source.links.manager+0x200,source.parameter.counter+0x20,source.links.scene+0x100,source.parameter.page_list+0x20]:
            b.firmware.write(va,struct.pack('<Q',0x123456789abcdef0))
        pool=OperandPool(source.pool.directory+delta,source.pool.table+delta,tuple(x+delta for x in source.pool.buffers))
        support=pool.support(source.links.auxiliary,header=1,active=0,registered=23,resource_class=2)
        for context,va,flags,memory in expected:
            newva=va+delta if context==1 and 0x7000000000<=va<0x7020000000 else va
            head=proc.stdout.read(20);assert head==struct.pack('<IQQ',context,newva,flags),(context,hex(newva),head.hex())
            want=bytearray(memory.read(va,PAGE_SIZE))
            if context==1 and (source.pool.directory<=va<source.pool.directory+DIRECTORY_SIZE or va==source.pool.table):
                for i in range(0,len(want),8):
                    value=struct.unpack_from('<Q',want,i)[0]
                    if value:struct.pack_into('<Q',want,i,value+delta)
            if context==2 and va==source.links.support:want=bytearray(support)
            actual=proc.stdout.read(PAGE_SIZE);assert actual==want,(context,hex(newva),[hex(i)for i,(x,y)in enumerate(zip(actual,want))if x!=y][:16])
            digest.update(head+actual);total+=20+len(actual)
        assert proc.stdout.read(64)==register_flist(1,source.links.support,pool.table,5,previous=23)
    assert proc.stdout.read(1)==b'';assert proc.wait()==0
summary=dict(source_RenderResources_allocate_executed=True,detached_build_without_live_vm_or_root=True,detached_work_page_zero_images=7,detached_work_allocation_failure_boundaries=7,owner_private_whole_image_cases=2,owner_private_original_initializer_revision=owner_reference_revision,owner_private_allocation_failure_cases=8,owner_private_exact_root_extent_rejects=6,
        detached_allocation_failure_cases=4,
        detached_geometry_allocation_failure_boundaries=3,geometry_detached_plan_allocation_without_live_root=True,geometry_stale_or_foreign_plan_rejects=3,
        geometry_failure_preserves_all_leaves_and_addresses=True,
        failure_installs_no_resource_leaves_or_descriptor=True,
        same_reservation_retry_after_failures=True,production_Rust_allocator_executed=True,
    complete_virtual_pages=len(expected)*(2 if a.recycle else 1),relocated_rebind_and_physical_reuse=a.recycle,mapped_rebind_preserves_physical_root_and_all_PTEs=a.recycle,PB_metadata_mutations_preserved=a.recycle,zero_new_backing_allocations_on_rebind=a.recycle,geometry_guard_and_last_metadata_foreign_mapping_rejected_before_clear=a.recycle,all_virtual_DATA_bytes_and_flags_match=True,whole_bytes=total,
    whole_sha256=digest.hexdigest(),owned_FList_request_matches=True,one_idempotent_admission=True,admission_lifetimes=2 if a.recycle else 1,
    last_alias_and_installed_root_mismatches_reject_before_control=True,
    models='RAM, low/root PTE APIs, receipt reply and cache primitives; not native CPU/GPU',native_GPU_execution=False,
    source_sha256={name:hashlib.sha256((tests.parent/name).read_bytes()).hexdigest()for name in
        ('scheduled_render_runtime.rs','scheduled_compute_runtime.rs','scheduled_render_resources.rs','scheduled_render_admission.rs','scheduled_operands.rs')})
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print('PASS',summary['complete_virtual_pages'],'complete virtual pages/',total,'bytes, all source allocation DATA/flags; owned admission preflight')
