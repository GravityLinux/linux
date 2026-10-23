// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Quiescent source-owned compute FList extension. Never rewrite the live
//! directory, dispatch cursors or firmware support counters from the CPU.

use super::*;
use super::super::super::g17p_render::admission::RegistrationG16C;

fn low_bytes(memory:&Memory,root:&UserVm,address:u64,data:&mut [u8])->Result {
    let mut offset=0;
    while offset<data.len() {
        let va=address+offset as u64;
        let pa=root.pte(va & !(PAGE-1))? & ADDRESS;
        if pa==0 {return Err(EIO);}
        let within=(va & (PAGE-1)) as usize;
        let n=(PAGE as usize-within).min(data.len()-offset);
        memory.read_firmware_words(pa+within as u64,&mut data[offset..offset+n])?;
        offset+=n;
    }
    Ok(())
}
impl State {
    /// Read only our owned scratch DATA, with no executable GPU user. Sampling
    /// is opt-in and never writes a live pool or follows firmware pointers.
    fn sample_operands(&self,session:&Session,index:usize,seed:bool)->Result {
        let memory=session.memory.as_ref().ok_or(EIO)?;
        self.growth_roots(memory,session.ttbs,index)?;
        let o=&self.owners[index];
        let root=&o.client.as_ref().ok_or(EIO)?.root;
        let mut data=bytes(PAGE as usize)?;
        let mut changed=0;
        let mut sampled=0;
        for (buffer,base) in o.operand_buffers.iter().enumerate().skip(operands::INITIAL_BUFFERS as usize) {
            for offset in [0,operands::BUFFER_SIZE/2,operands::BUFFER_SIZE-PAGE] {
                let address=o.directory+base+offset;
                low_bytes(memory,root,address,&mut data)?;
                let nonzero=data.iter().filter(|&&v|v!=0).count();
                sampled+=1;
                if seed && nonzero!=0 {return Err(EIO);}
                if nonzero!=0 {
                    changed+=1;
                    let hash=data.iter().fold(0xcbf29ce484222325u64,|h,&v|(h^u64::from(v)).wrapping_mul(0x100000001b3));
                    let mut head=[0u64;8];
                    for (i,word) in head.iter_mut().enumerate() {*word=u64::from_le_bytes(data[i*8..i*8+8].try_into().unwrap());}
                    kernel::dev_info!(session.peers[0].data.dev.as_ref(),
                        "G16C: operand sample context={} buffer={} offset={:#x} nonzero={} fnv={:016x} head={:016x?} address={:#x}\n",
                        o.context,buffer,offset,nonzero,hash,head,address);
                }
            }
        }
        kernel::dev_info!(session.peers[0].data.dev.as_ref(),
            "G16C: operand probe context={} phase={} buffers={} pages={} changed={} GPU_idle=1\n",
            o.context,if seed {"seed"} else {"retired"},o.operand_buffers.len(),sampled,changed);
        Ok(())
    }
    pub(super) fn seed_operand_probe(&mut self,session:&Session,index:usize)->Result {
        if *crate::module_parameters::g16c_operand_probe.value()==0
            || self.owners[index].operand_probe_seeded
            || self.owners[index].operand_buffers.len()<=operands::INITIAL_BUFFERS as usize {return Ok(());}
        if !self.pending.is_empty() {return Err(EBUSY);}
        self.sample_operands(session,index,true)?;
        self.owners[index].operand_probe_seeded=true;
        Ok(())
    }
    pub(super) fn retire_operand_probe(&mut self,session:&Session)->Result {
        if *crate::module_parameters::g16c_operand_probe.value()==0 {return Ok(());}
        let Some(index)=self.selected else {return Ok(());};
        let o=&self.owners[index];
        if !o.operand_probe_pending || !o.operand_probe_seeded {return Ok(());}
        self.sample_operands(session,index,false)?;
        self.owners[index].operand_probe_pending=false;
        Ok(())
    }
    pub(super) fn grow_operands(&mut self,session:&mut Session,index:usize,total:u32)->StageResult<()> {
        if !self.idle() || self.selected!=Some(index) {return Err(EBUSY.into());}
        self.growth_roots(session.memory.as_ref().ok_or(EIO)?,session.ttbs,index)?;
        let o=&mut self.owners[index];
        let previous=u32::try_from(o.operand_buffers.len())?;
        if total<=previous || total>1024 {return Err(EINVAL.into());}
        let added=total-previous;
        let table=o.directory+operands::DIRECTORY_SIZE+2*PAGE;
        let base=o.private_next;
        let end=base.checked_add(u64::from(added)*operands::BUFFER_STRIDE).ok_or(EOVERFLOW)?;
        if end>o.private_limit {return Err(StageError::Rejected(ENOSPC));}
        // Construct the receipt before publication. The support/table/context
        // tuple and previous end own the reply; a generic kind13 is not enough.
        let registration=RegistrationG16C {context:u32::from(o.context),support:o.support,
            table,added,previous,mode:0,tag:1,reply_value:0};
        registration.prepare().map_err(|_|EINVAL)?;
        let memory=session.memory.as_mut().ok_or(EIO)?;
        let vm=session.vm.as_ref().ok_or(EIO)?;
        let cursors=boot::control_cursors(memory,vm)?;
        if cursors!=[cursors[0];3] {return Err(EBUSY.into());}
        for (i,flags) in [PRIVATE,SHARED].into_iter().enumerate() {
            if vm.pte(memory,2,o.support+i as u64*PAGE)?!=o.support_pages[i]|flags {return Err(EIO.into());}
        }
        let mut header=[0u8;0x100];
        memory.read_firmware_words(o.support_pages[0],&mut header)?;
        let word=|at|u32::from_le_bytes(header[at..at+4].try_into().unwrap());
        let ptr=|at|u64::from_le_bytes(header[at..at+8].try_into().unwrap());
        if word(0x10)!=2 || ptr(0x14)!=o.directory || ptr(0x30)!=table || word(0x48)!=previous*8 {
            return Err(EIO.into());
        }
        let root=&mut o.client.as_mut().ok_or(EIO)?.root;
        let mut prefix=bytes(previous as usize*64).map_err(StageError::allocation)?;
        low_bytes(memory,root,table,&mut prefix)?;
        let mut expected=bytes(prefix.len()).map_err(StageError::allocation)?;
        for (i,offset) in o.operand_buffers.iter().enumerate() {
            expected[i*64..i*64+8].copy_from_slice(&(o.directory+offset|1<<63).to_le_bytes());
        }
        if prefix!=expected {return Err(EIO.into());}
        let mut tail=bytes(added as usize*64).map_err(StageError::allocation)?;
        o.operand_buffers.reserve(added as usize,GFP_KERNEL).map_err(StageError::allocation)?;
        // No owner extents, root leaves, table entries or firmware producers
        // change until ALL backing exists. An allocation failure drops this
        // detached heap, including chunks from earlier buffers in this attempt.
        let mut detached=Memory::detached();
        let mut blocks=KVec::new();
        for i in 0..added {
            let fail_after=*crate::module_parameters::g16c_operand_alloc_fail_after.value();
            if !self.operand_allocation_faulted && fail_after==i {
                self.operand_allocation_faulted=true;
                let (count,bytes)=detached.allocation_stats();
                kernel::dev_info!(session.peers[0].data.dev.as_ref(),
                    "G16C: injected unpublished operand ENOMEM staged_buffers={} allocations={} bytes={}\n",
                    i,count,bytes);
                return Err(StageError::Rejected(ENOMEM));
            }
            let address=base+u64::from(i)*operands::BUFFER_STRIDE;
            private_span(&mut detached,&mut blocks,address-o.directory,operands::BUFFER_SIZE)
                .map_err(StageError::allocation)?;
            tail[i as usize*64..i as usize*64+8].copy_from_slice(&(address|1<<63).to_le_bytes());
        }
        o.private.reserve(blocks.len(),GFP_KERNEL).map_err(StageError::allocation)?;
        memory.absorb(detached).map_err(StageError::allocation)?;
        let first=o.private.len();
        for block in blocks {
            o.private.push(block,GFP_KERNEL).expect("reserved unpublished operand backing");
        }
        // From this point, an error is fatal and all backing remains retained.
        // In particular, ENOMEM from root mapping is not a benign rejection.
        map_private(memory,root,o.directory,&o.private[first..])?;
        let mut offset=0;
        while offset<tail.len() {
            let va=table+u64::from(previous)*64+offset as u64;
            let pa=root.pte(va & !(PAGE-1))? & ADDRESS;
            if pa==0 {return Err(EIO.into());}
            let within=(va & (PAGE-1)) as usize;
            let n=(PAGE as usize-within).min(tail.len()-offset);
            memory.write(pa+within as u64,&tail[offset..offset+n])?;
            memory.clean(pa+within as u64,n)?;
            offset+=n;
        }
        let mut unchanged=bytes(prefix.len())?;
        low_bytes(memory,root,table,&mut unchanged)?;
        let mut unchanged_header=[0u8;0x100];
        memory.read_firmware_words(o.support_pages[0],&mut unchanged_header)?;
        if unchanged!=prefix || unchanged_header!=header {return Err(EIO.into());}
        UserVm::invalidate(o.context);
        Vm::invalidate_gpu();
        super::super::super::g17p_memory::sync();
        let support=o.support;let context=o.context;
        self.growth_roots(session.memory.as_ref().ok_or(EIO)?,session.ttbs,index)?;
        session.g16c_register_flist(&registration)?;
        // RX and all control cursors retiring do not prove applied capacity.
        session.g16c_wait(|memory,vm| {
            let count=memory.read_firmware32(vm.physical(memory,2,support+0x48)?)?;
            if count==total*8 {Ok(true)} else if count==previous*8 {Ok(false)} else {Err(EIO)}
        })?;
        let o=&mut self.owners[index];
        for i in 0..added {
            o.operand_buffers.push(base+u64::from(i)*operands::BUFFER_STRIDE-o.directory,GFP_KERNEL)?;
        }
        o.private_next=end;
        o.operand_probe_seeded=false;
        kernel::dev_info!(session.peers[0].data.dev.as_ref(),
            "G16C: owned operand growth context={} buffers={}->{} appended DATA only, FList reply/control/applied count retired\n",
            context,previous,total);
        Ok(())
    }
}
