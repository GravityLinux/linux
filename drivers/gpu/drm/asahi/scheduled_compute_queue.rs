// SPDX-License-Identifier: GPL-2.0-only

//! G16C wire geometry for the common retained compute generation.
//! Selection, leases, root replacement and transport rotation stay in Queues.

use super::{Client, Layout, Memory, Parameters, Vm, WrappedSeed};
use super::super::{
    g17p_compute_runtime::client_storage,
    g17p_queue as q,
    scheduled_dispatch::DispatchStateG16C,
    scheduled_operands as operands,
};
use kernel::prelude::*;
const PAGE:u64=0x4000;
const DISPATCH:u64=0xfffffc2001550000;
const SAVE_STRIDE:u64=0x78000;

#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) struct PrivateLayout {
    pub(crate) base:u64,
    pub(crate) table:u64,
    pub(crate) state:u64,
    pub(crate) buffers:u64,
    pub(crate) save:u64,
    pub(crate) robustness:u64,
    pub(crate) end:u64,
}
impl PrivateLayout {
    pub(crate) fn new(p:&Parameters)->Result<Self> {
        let base=p.preempt;
        let table=base.checked_add(operands::DIRECTORY_SIZE+2*PAGE).ok_or(EOVERFLOW)?;
        let state=table.checked_add(operands::TABLE_SIZE+2*PAGE).ok_or(EOVERFLOW)?;
        let buffers=state.checked_add(0x80000).ok_or(EOVERFLOW)?;
        let save=buffers.checked_add(u64::from(operands::INITIAL_BUFFERS)*operands::BUFFER_STRIDE+0x80000).ok_or(EOVERFLOW)?;
        let robustness=save.checked_add(256*SAVE_STRIDE).ok_or(EOVERFLOW)?;
        let end=robustness.checked_add(PAGE).ok_or(EOVERFLOW)?;
        if base<PAGE || base&(PAGE-1)!=0 || end>p.kernel_end || p.kernel_end>1<<42 {return Err(EINVAL);}
        Ok(Self {base,table,state,buffers,save,robustness,end})
    }
    pub(crate) fn offsets(&self)->Result<KVec<u64>> {
        let mut offsets=KVec::new();
        for (address,size) in [(self.base,operands::DIRECTORY_SIZE),(self.table,operands::TABLE_SIZE),
            (self.state,0x7c000),(self.robustness,PAGE)] {
            for offset in (0..size).step_by(PAGE as usize) {offsets.push(address-self.base+offset,GFP_KERNEL)?;}
        }
        for i in 0..u64::from(operands::INITIAL_BUFFERS) {
            for offset in (0..operands::BUFFER_SIZE).step_by(PAGE as usize) {
                offsets.push(self.buffers-self.base+i*operands::BUFFER_STRIDE+offset,GFP_KERNEL)?;
            }
        }
        for slot in 0..256 {
            for offset in [0,PAGE,2*PAGE] {offsets.push(self.save-self.base+slot*SAVE_STRIDE+offset,GFP_KERNEL)?;}
        }
        Ok(offsets)
    }
    pub(crate) fn build(&self,memory:&mut Memory,client:&mut Client)->Result {
        for offset in (0..operands::DIRECTORY_SIZE).step_by(0x200000) {
            client_storage(memory,&mut client.root,self.base+offset,0x200000)?;
        }
        let table=client_storage(memory,&mut client.root,self.table,operands::TABLE_SIZE as usize)?;
        client_storage(memory,&mut client.root,self.state,0x7c000)?;
        let mut entries=KVVec::with_capacity(operands::TABLE_SIZE as usize,GFP_KERNEL)?;
        entries.resize(operands::TABLE_SIZE as usize,0,GFP_KERNEL)?;
        let mut list=KVVec::with_capacity(PAGE as usize,GFP_KERNEL)?;
        list.resize(PAGE as usize,0,GFP_KERNEL)?;
        for i in 0..u64::from(operands::INITIAL_BUFFERS) {
            let address=self.buffers+i*operands::BUFFER_STRIDE;
            for offset in (0..operands::BUFFER_SIZE).step_by(0x200000) {
                client_storage(memory,&mut client.root,address+offset,0x200000)?;
            }
            for page in 0..2048 {
                list[page*8..page*8+8].copy_from_slice(&(address+page as u64*0x1000).to_le_bytes());
            }
            let pa=client.root.pte(self.base+i*PAGE)?&0x000003ffffffc000;
            memory.write(pa,&list)?;memory.clean(pa,PAGE as usize)?;
            entries[i as usize*64..i as usize*64+8].copy_from_slice(&(address|1<<63).to_le_bytes());
        }
        memory.write(table,&entries)?;memory.clean(table,entries.len())?;
        for slot in 0..256 {client_storage(memory,&mut client.root,self.save+slot*SAVE_STRIDE,0xc000)?;}
        client_storage(memory,&mut client.root,self.robustness,PAGE as usize)?;
        Ok(())
    }
}

pub(crate) struct WrappedQueue {
    pub(crate) private:PrivateLayout,
    pub(crate) registration:u32,
    pub(crate) bound:bool,
}
impl WrappedQueue {
    pub(crate) fn initialize(memory:&mut Memory,vm:&Vm,l:Layout,p:&Parameters,priority:u32)->Result<Self> {
        let private=PrivateLayout::new(p)?;
        vm.write(memory,2,l.support(),operands::ComputeSupportG16C::new(private.base,private.table,
            l.support_state(),operands::INITIAL_BUFFERS).ok_or(EINVAL)?.as_bytes())?;
        vm.write(memory,2,l.support_state(),&1u32.to_le_bytes())?;
        Self::registration(memory,vm,l,l.pointers(),l.ring(),priority)?;
        Ok(Self {private,registration:0,bound:false})
    }
    pub(crate) fn registration(memory:&mut Memory,vm:&Vm,l:Layout,pointers:u64,ring:u64,priority:u32)->Result {
        vm.write(memory,2,l.control(),q::wire::QueueContextG16C::default().as_bytes())?;
        vm.write(memory,2,l.job_list(),&q::job_list(l.job_list()))?;
        vm.write(memory,2,pointers,q::wire::QueuePointersG16C::with_write(0).as_bytes())?;
        let words=q::priority_profile(priority).map_err(|_|EINVAL)?;
        vm.write(memory,2,l.queue(),q::wire::QueueRecordG16C::new(q::wire::QueueParameters {
            pointers,ring,jobs:l.job_list(),context:l.control(),uuid:0x200+u32::from(l.grid),
            priority,unk_2c:priority,unk_30:u64::from_le_bytes(words[8..16].try_into().unwrap()),
            unk_38:u32::from_le_bytes(words[16..20].try_into().unwrap()),
            prio5:u32::from_le_bytes(words[24..28].try_into().unwrap()),unk_94:0,
        }).as_bytes())
    }
    pub(crate) fn seed(l:Layout,p:&Parameters,sequence:u32,registration:u32)->Result<WrappedSeed> {
        let private=PrivateLayout::new(p)?;let slot=sequence%256;
        Ok(WrappedSeed {context:l.asid,root:0,queue:l.queue(),queue_control:l.control(),
            uuid:(0x200+u32::from(l.grid))as u16,queue_ordinal:registration,
            support_ordinal:sequence,support:l.support(),save:private.save+u64::from(slot)*SAVE_STRIDE,
            robustness:private.robustness+u64::from(slot)*64,operand_state:private.state,save_slot:slot as usize})
    }
    pub(crate) fn dispatch(memory:&mut Memory,vm:&Vm,l:Layout,p:&Parameters,initial:bool)->Result {
        let private=PrivateLayout::new(p)?;
        let offset=DispatchStateG16C::slot_offset(l.asid).ok_or(EINVAL)?;
        let address=vm.physical(memory,2,DISPATCH+offset as u64)?;
        memory.invalidate(address,32)?;
        let data=DispatchStateG16C::initial(private.base,operands::INITIAL_BUFFERS*2048).ok_or(EINVAL)?;
        // An idle rebind relocates the same private pages; preserve all GPU
        // cursor/count bytes and change only the directory address.
        memory.write(address,if initial {data.as_bytes()} else {&data.as_bytes()[..8]})?;
        memory.clean(address,if initial {32} else {8})
    }
    pub(crate) fn update_private(&mut self,memory:&mut Memory,vm:&Vm,l:Layout,p:&Parameters)->Result {
        let private=PrivateLayout::new(p)?;
        vm.write(memory,2,l.support()+0x14,&private.base.to_le_bytes())?;
        vm.write(memory,2,l.support()+0x30,&private.table.to_le_bytes())?;
        Self::dispatch(memory,vm,l,p,false)?;
        self.private=private;Ok(())
    }
    pub(crate) fn clear_command(&self,memory:&mut Memory,root:&super::super::g17p_user_vm::UserVm,sequence:u32)->Result {
        let slot=u64::from(sequence%256);
        for offset in [0,PAGE,2*PAGE] {
            let pa=root.pte(self.private.save+slot*SAVE_STRIDE+offset)?&0x000003ffffffc000;
            if pa==0 {return Err(EIO);}memory.zero(pa,PAGE as usize)?;memory.clean(pa,PAGE as usize)?;
        }
        let address=self.private.robustness+slot*64;
        let pa=(root.pte(address&!(PAGE-1))?&0x000003ffffffc000)+(address&(PAGE-1));
        memory.zero(pa,64)?;memory.clean(pa,64)
    }
}

pub(crate) fn cleanup_message(step:u32,control:u64,live:&[u8;0x40])->Result<[u8;0x40]> {
    use super::super::scheduled_lifetime::{ContextCloseG16C,LifetimeTickG16C,MaintenanceG16C};
    Ok(match step {
        0=>*MaintenanceG16C::new(0,1).ok_or(EINVAL)?.bytes(),
        1=>*LifetimeTickG16C::default().bytes(),
        2=>*MaintenanceG16C::new(0,2).ok_or(EINVAL)?.bytes(),
        3=>*MaintenanceG16C::new(1,1).ok_or(EINVAL)?.bytes(),
        4=>*MaintenanceG16C::new(1,2).ok_or(EINVAL)?.bytes(),
        5=>*MaintenanceG16C::new(2,1).ok_or(EINVAL)?.bytes(),
        6=>*MaintenanceG16C::new(2,2).ok_or(EINVAL)?.bytes(),
        7=>*ContextCloseG16C::new(control,live[..4].try_into().unwrap()).ok_or(EINVAL)?.bytes(),
        _=>return Err(EINVAL),
    })
}
