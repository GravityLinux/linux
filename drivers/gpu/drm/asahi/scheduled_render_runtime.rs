// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Max render resources within the existing Session/Memory/UAT ownership.
//! Allocation does not publish roots or work. Admission retains the root and
//! checks its installed words before exposing the FList control producer.

use super::super::super::{
    g17p_render::{admission::RegistrationG16C, resources as raw, growth},
    g17p_user_vm::RootLease,
    scheduled_initgraph,
};
use super::{
    bytes, map_private, map_prepared_private, operands, private_span, write_blocks, write_low, Memory, PrivateBlock, Session,
    UserVm, Vm, ADDRESS, GPU_READ, PAGE, PRIVATE, SHARED,
};
use kernel::prelude::*;

const INITIAL_BUFFERS: u32 = 28;
const REGISTERED: u32 = 23;
const GPU_WRITE: u64 = GPU_READ | (1 << 54);

pub(super) struct GrowthSnapshot {
    manager: [u8;0x88],
    control: [u8;0x80],
    counter: u32,
    region: [u8;16],
    pages: KVVec<u8>,
}

pub(super) struct PreparedGeometryG16C {
    blocks: KVec<PrivateBlock>,
    geometry: [(u64, u64); 3],
    next: u64,
    root: RootLease,
}

pub(super) struct ResourcesG16C {
    pub(super) parameter: raw::ParameterBufferG16C,
    pub(super) support: u64,
    pub(super) auxiliary: u64,
    pub(super) manager: u64,
    pub(super) scene: u64,
    pub(super) empty_scene: u64,
    pub(super) private_end: u64,
    pub(super) directory: u64,
    table: u64,
    root: Option<RootLease>,
    private: KVec<PrivateBlock>,
    compact: KVec<PrivateBlock>,
    metadata: KVec<(u64, u64, u64, u64)>,
    admitted: bool,
    geometry: [(u64, u64); 3],
    geometry_next: u64,
    block_page_ids: KVec<u32>,
    parameter_growth_next: u64,
    pub(super) sequence: u32,
    pub(super) sequence_base: u32,
}

fn metadata(
    memory: &mut Memory,
    records: &mut KVec<(u64, u64, u64, u64)>,
    address: u64,
    data: &[u8],
    flags: u64,
) -> Result<u64> {
    records.reserve(1, GFP_KERNEL)?;
    let pa = memory.allocate(data.len())?;
    records.push((address, pa, data.len() as u64, flags), GFP_KERNEL)?;
    memory.write(pa, data)?;
    memory.clean(pa, data.len())?;
    Ok(pa)
}
fn alias(root: &mut UserVm, address: u64, physical: u64, size: u64) -> Result {
    root.prepare(address, size)?;
    for offset in (0..size).step_by(PAGE as usize) {
        root.map_page(address + offset, physical + offset, true)?;
    }
    Ok(())
}
impl ResourcesG16C {
    // Conservatively retain a 32-KiB guard after every compact chunk. This
    // also covers the smaller geometry guard and all retired geometry extents.
    fn compact_conflict(&self, address: u64, size: u64) -> Option<u64> {
        self.compact.iter().filter_map(|block| {
            let start=raw::CONTEXT_BASE+block.offset;
            let end=start+block.size+0x8000;
            (start < address+size && address < end).then_some(end)
        }).max()
    }

    pub(super) fn growth_snapshot(&self, memory:&Memory, vm:&Vm) -> Result<GrowthSnapshot> {
        if !self.admitted || self.root.is_none() { return Err(EIO); }
        let mut snapshot=GrowthSnapshot { manager:[0;0x88], control:[0;0x80], counter:0,
            region:[0;16], pages:bytes(raw::PAGE_LIST_SIZE)? };
        let read=super::boot::read_words;
        read(memory,vm,self.manager,&mut snapshot.manager)?;
        read(memory,vm,self.parameter.block_control,&mut snapshot.control)?;
        snapshot.counter=memory.read_firmware32(vm.physical(memory,2,self.parameter.counter)?)?;
        read(memory,vm,scheduled_initgraph::BASE+0x1548000+u64::from(self.parameter.slot)*16,&mut snapshot.region)?;
        read(memory,vm,self.parameter.page_list,&mut snapshot.pages)?;
        let mut blocks=bytes(self.parameter.blocks as usize*8)?;
        read(memory,vm,self.parameter.block_list,&mut blocks)?;
        if !growth::metadata_owned(&self.parameter,&self.block_page_ids,&snapshot.manager,
            &snapshot.control,snapshot.counter,&snapshot.region,&snapshot.pages,&blocks) {
            kernel::pr_err!("G16C: PB metadata mismatch blocks={} generation={} manager={:02x?} control={:02x?} region={:02x?}\n",self.parameter.blocks,snapshot.counter,snapshot.manager,snapshot.control,snapshot.region);
            return Err(EIO);
        }
        Ok(snapshot)
    }

    pub(super) fn growth_generation(&self,memory:&Memory,vm:&Vm)->Result<u32> {
        Ok(self.growth_snapshot(memory,vm)?.counter)
    }

    /// Called only after an owned request has been ACKed and maintenance has
    /// retired. Append fresh backing; never rebuild live firmware free lists.
    pub(super) fn grow(&mut self,memory:&mut Memory,vm:&Vm,root:&mut UserVm,
        context:u16,generation:u32)->Result<(u32,u32)> {
        let before=self.growth_snapshot(memory,vm)?;
        if before.counter!=generation || self.root.as_ref().is_none_or(|r|r.root()!=root.root()) {
            return Err(EIO);
        }
        let old=self.parameter.blocks;
        let count=(old+10).min(raw::MAX_BLOCKS);
        if count==old { return Err(ENOMEM); }
        let additional=(count-old)as usize;
        self.compact.reserve(additional,GFP_KERNEL)?;
        self.block_page_ids.reserve(additional,GFP_KERNEL)?;
        let mut entries=bytes(additional*8)?;
        let mut cursor=self.parameter_growth_next;
        let mut first=self.compact.len();
        for index in 0..additional {
            let mut address=(cursor+0x7fff)&!0x7fff;
            loop {
                if address.checked_add(0x28000).ok_or(EOVERFLOW)?>0x1100000000 {return Err(ENOSPC);}
                if let Some(end)=self.compact_conflict(address,0x28000) {
                    address=(end+0x7fff)&!0x7fff;
                    continue;
                }
                let mut conflict=None;
                for offset in (0..0x28000).step_by(PAGE as usize) {
                    if root.pte(address+offset)?!=0 {conflict=Some(address+offset+PAGE);break;}
                }
                if let Some(end)=conflict {address=(end+0x7fff)&!0x7fff;continue;}
                break;
            }
            root.prepare(address,0x28000)?;
            // Retain allocation bookkeeping BEFORE publishing any leaf.
            private_span(memory,&mut self.compact,address-raw::CONTEXT_BASE,0x20000)?;
            map_private(memory,root,raw::CONTEXT_BASE,&self.compact[first..])?;
            first=self.compact.len();
            let id=u32::try_from((address-raw::CONTEXT_BASE)/0x8000).map_err(|_|EOVERFLOW)?;
            entries[index*8..index*8+4].copy_from_slice(&id.to_le_bytes());
            cursor=address+0x28000;
        }
        self.root.as_mut().ok_or(EIO)?.refresh(root)?;
        UserVm::invalidate(context);
        let unchanged=self.growth_snapshot(memory,vm)?;
        if before.manager!=unchanged.manager || before.control!=unchanged.control
            || before.counter!=unchanged.counter || before.region!=unchanged.region
            || before.pages!=unchanged.pages {return Err(EBUSY);}
        vm.write(memory,2,self.parameter.block_list+u64::from(old)*8,&entries)?;
        let mut counts=[0u8;8];
        counts[..4].copy_from_slice(&count.to_le_bytes());
        counts[4..].copy_from_slice(&count.to_le_bytes());
        vm.write(memory,2,self.parameter.block_control,&counts)?;
        super::super::super::g17p_memory::sync();
        // Deliberately exclude control counts and appended entries, the ONLY
        // host-owned DATA changed before sending the reply.
        let mut manager=[0;0x88];let mut region=[0;16];let mut pages=bytes(raw::PAGE_LIST_SIZE)?;
        let read=super::boot::read_words;
        read(memory,vm,self.manager,&mut manager)?;
        read(memory,vm,scheduled_initgraph::BASE+0x1548000+u64::from(self.parameter.slot)*16,&mut region)?;
        read(memory,vm,self.parameter.page_list,&mut pages)?;
        if before.manager!=manager || before.region!=region || before.pages!=pages
            || generation!=memory.read_firmware32(vm.physical(memory,2,self.parameter.counter)?)? {return Err(EIO);}
        for entry in entries.chunks_exact(8) {
            self.block_page_ids.push(u32::from_le_bytes(entry[..4].try_into().unwrap()),GFP_KERNEL)?;
        }
        self.parameter.blocks=count;
        self.parameter_growth_next=cursor;
        Ok((old,count))
    }

    pub(super) fn growth_applied(&self,memory:&Memory,vm:&Vm,old:u32)->Result<bool> {
        let mut manager=[0;0x88];super::boot::read_words(memory,vm,self.manager,&mut manager)?;
        let word=|at|u32::from_le_bytes(manager[at..at+4].try_into().unwrap());
        let pages=word(0x34);let blocks=word(0x3c);
        if pages==self.parameter.blocks*4 && blocks==self.parameter.blocks {return Ok(true);}
        if ![old*4,self.parameter.blocks*4].contains(&pages) || ![old,self.parameter.blocks].contains(&blocks) {
            kernel::pr_err!("G16C: unexpected growth manager pages={} blocks={} expected={}->{} manager={:02x?}\n",pages,blocks,old,self.parameter.blocks,manager);
            return Err(EIO);
        }
        Ok(false)
    }

    pub(super) fn private_end(directory: u64) -> Result<u64> {
        directory.checked_add(operands::DIRECTORY_SIZE + PAGE)
            .and_then(|base| base.checked_add(u64::from(INITIAL_BUFFERS) * operands::BUFFER_STRIDE))
            .ok_or(EOVERFLOW)
    }

    pub(super) fn prepare(
        memory: &mut Memory,
        vm: &mut Vm,
        root: &mut UserVm,
        slot: u32,
        bank: u32,
        directory: u64,
        limit: u64,
    ) -> Result<(Memory, Self)> {
        if slot >= 15 || bank >= 15 || directory & (PAGE - 1) != 0 || directory == 0 {
            return Err(EINVAL);
        }
        let start = 0xfffffc20c3000000 + u64::from(bank) * 0x200000;
        let support = start;
        let auxiliary = start + PAGE;
        let manager = start + 0x100000;
        let scenes = start + 0x108000;
        let parameter = raw::ParameterBufferG16C {
            slot,
            blocks: REGISTERED,
            page_list: start + 0x150000,
            block_list: start + 0x120000,
            block_control: start + 0x110000,
            counter: start + 0x118000,
            scene_list: start + 0x114000,
        };
        parameter.validate().map_err(|_| EINVAL)?;
        let table = directory
            .checked_add(operands::DIRECTORY_SIZE)
            .ok_or(EOVERFLOW)?;
        let base = table.checked_add(PAGE).ok_or(EOVERFLOW)?;
        let private_end = Self::private_end(directory)?;
        if private_end > limit || limit > 1 << 42 {
            return Err(ENOSPC);
        }
        // Reserve firmware and application paths before writing metadata. A
        // collision retains existing leaves; never replace caller mappings.
        for (va, size) in [
            (support, PAGE),
            (auxiliary, PAGE),
            (manager, PAGE),
            (scenes, PAGE),
            (parameter.block_control, PAGE),
            (parameter.counter, PAGE),
            (parameter.scene_list, PAGE),
            (parameter.block_list, raw::BLOCK_LIST_SIZE as u64),
            (parameter.page_list, raw::PAGE_LIST_SIZE as u64),
        ] {
            vm.prepare_g16c_empty(memory, 2, va, size as usize)?;
        }
        let mut staged = Memory::detached();
        let mut private = KVec::new();
        let mut compact = KVec::new();
        let mut records = KVec::new();
        private_span(&mut staged, &mut private, 0, operands::DIRECTORY_SIZE)?;
        private_span(&mut staged, &mut private, table - directory, PAGE)?;
        for i in 0..INITIAL_BUFFERS {
            private_span(
                &mut staged,
                &mut private,
                base - directory + u64::from(i) * operands::BUFFER_STRIDE,
                operands::BUFFER_SIZE,
            )?;
        }
        for (va, size) in [
            (0x1000068000, 0x10000),
            (0x1000078000, PAGE),
            (0x1000490000, PAGE),
            (0x1000420000, PAGE),
            (0x1000428000, PAGE),
            (0x10004d0000, PAGE),
        ] {
            private_span(&mut staged, &mut compact, va - raw::CONTEXT_BASE, size)?;
        }
        for (va, size) in parameter.backing_ranges().map_err(|_| EINVAL)? {
            private_span(&mut staged, &mut compact, va - raw::CONTEXT_BASE, size)?;
        }
        // Preflight both sets and the aliases as one owned mapping plan.
        for block in &private {
            root.prepare(directory + block.offset, block.size)?;
        }
        for block in &compact {
            root.prepare(raw::CONTEXT_BASE + block.offset, block.size)?;
        }
        for i in 0..INITIAL_BUFFERS {
            root.prepare(
                base + u64::from(i) * operands::BUFFER_STRIDE + operands::BUFFER_SIZE,
                0x8000,
            )?;
        }
        for (va, size) in parameter.backing_ranges().map_err(|_| EINVAL)? {
            root.prepare(va + size, 0x8000)?;
        }
        root.prepare(raw::GPU_PAGE_LIST, raw::PAGE_LIST_SIZE as u64)?;
        root.prepare(0x1000080000, PAGE)?;
        let mut entries = bytes(PAGE as usize)?;
        let mut directory_page = bytes(PAGE as usize)?;
        for i in 0..INITIAL_BUFFERS as usize {
            let buffer = base + i as u64 * operands::BUFFER_STRIDE;
            for page in 0..2048usize {
                directory_page[page * 8..page * 8 + 8]
                    .copy_from_slice(&(buffer + page as u64 * 0x1000).to_le_bytes());
            }
            write_blocks(&mut staged, &private, i as u64 * PAGE, &directory_page)?;
            entries[i * 64..i * 64 + 8].copy_from_slice(&(buffer | 1 << 63).to_le_bytes());
        }
        write_blocks(&mut staged, &private, table - directory, &entries)?;
        let mut page = bytes(PAGE as usize)?;
        let record = operands::OperandSupportG16C::for_render(
            directory,
            table,
            auxiliary,
            INITIAL_BUFFERS,
            REGISTERED,
            REGISTERED,
        )
        .ok_or(EINVAL)?;
        page[..record.as_bytes().len()].copy_from_slice(record.as_bytes());
        metadata(&mut staged, &mut records, support, &page, PRIVATE)?;
        page.fill(0);
        metadata(&mut staged, &mut records, auxiliary, &page, SHARED)?;
        raw::ParameterManagerG16C::write(&mut page, &parameter).map_err(|_| EINVAL)?;
        metadata(&mut staged, &mut records, manager, &page, PRIVATE)?;
        parameter
            .scenes(&mut page, 0x1000080000)
            .map_err(|_| EINVAL)?;
        metadata(&mut staged, &mut records, scenes, &page, PRIVATE)?;
        parameter.control(&mut page).map_err(|_| EINVAL)?;
        metadata(
            &mut staged,
            &mut records,
            parameter.block_control,
            &page,
            SHARED,
        )?;
        parameter.counter(&mut page).map_err(|_| EINVAL)?;
        metadata(&mut staged, &mut records, parameter.counter, &page, SHARED)?;
        page.fill(0);
        metadata(
            &mut staged,
            &mut records,
            parameter.scene_list,
            &page,
            SHARED,
        )?;
        let mut block_list = bytes(raw::BLOCK_LIST_SIZE)?;
        parameter.block_ids(&mut block_list).map_err(|_| EINVAL)?;
        metadata(
            &mut staged,
            &mut records,
            parameter.block_list,
            &block_list,
            PRIVATE,
        )?;
        let mut pages = bytes(raw::PAGE_LIST_SIZE)?;
        parameter.pages(&mut pages).map_err(|_| EINVAL)?;
        metadata(
            &mut staged,
            &mut records,
            parameter.page_list,
            &pages,
            PRIVATE,
        )?;
        let mut block_page_ids = KVec::new();
        for i in 0..REGISTERED { block_page_ids.push(0x11+i*5,GFP_KERNEL)?; }
        let root = root.lease()?;
        for block in Iterator::chain(private.iter(), &compact) {
            staged.clean(block.physical, block.size as usize)?;
        }
        Ok((staged, Self {
            parameter,
            support,
            auxiliary,
            manager,
            scene: scenes + 0x80,
            empty_scene: scenes + 0x2800,
            private_end,
            directory,
            table,
            root: Some(root),
            private,
            compact,
            metadata: records,
            admitted: false,
            geometry: [(0, 0); 3],
            geometry_next: 0x1001000000,
            block_page_ids,
            parameter_growth_next: 0x1008000000,
            sequence: 0,
            sequence_base: 0,
        }))
    }

    pub(super) fn install(&self, memory: &mut Memory, vm: &mut Vm, root: &mut UserVm) -> Result {
        map_prepared_private(root, self.directory, &self.private)?;
        map_prepared_private(root, raw::CONTEXT_BASE, &self.compact)?;
        for &(address, physical, size, flags) in &self.metadata {
            vm.span(memory, 2, address, physical, size as usize, flags)?;
        }
        let scene = self.metadata.iter().find(|r| r.0 == self.parameter.scene_list).ok_or(EIO)?.1;
        let pages = self.metadata.iter().find(|r| r.0 == self.parameter.page_list).ok_or(EIO)?.1;
        alias(root, 0x1000080000, scene, PAGE)?;
        alias(root, raw::GPU_PAGE_LIST, pages, raw::PAGE_LIST_SIZE as u64)?;
        let descriptor = raw::ParameterDescriptorG16C::new(raw::GPU_PAGE_LIST, REGISTERED * 4)
            .map_err(|_| EINVAL)?;
        vm.write(
            memory,
            2,
            scheduled_initgraph::BASE + 0x1548000 + u64::from(self.parameter.slot) * 16,
            descriptor.bytes(),
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub(super) fn allocate(memory: &mut Memory, vm: &mut Vm, root: &mut UserVm,
        slot: u32, bank: u32, directory: u64, limit: u64) -> Result<Self> {
        let (staged, prepared) = Self::prepare(memory, vm, root, slot, bank, directory, limit)?;
        memory.absorb(staged)?;
        prepared.install(memory, vm, root)?;
        Ok(prepared)
    }

    /// The caller has retired all work/controls and invalidated both owned
    /// TTBRs. Logical PB metadata survives; no executable VM root may stay pinned.
    pub(super) fn detach(&mut self) -> Result {
        self.sequence_base = self
            .sequence_base
            .checked_add(self.sequence)
            .ok_or(EOVERFLOW)?;
        self.sequence = 0;
        self.admitted = false;
        self.root = None;
        Ok(())
    }

    pub(super) fn admitted(&self)->bool {self.admitted && self.root.is_some()}

    pub(super) fn attached(&self) -> bool {
        self.root.is_some()
    }

    /// No application mapping/lease or in-flight FList admission may move.
    pub(super) fn dormant(&self) -> bool {
        self.root.is_none() && !self.admitted && self.sequence == 0
    }

    /// Reattach retired backing to a fresh caller root. Metadata pages contain
    /// our firmware-updated PB state and must not be zeroed or reconstructed.
    pub(super) fn rebind(
        &mut self,
        memory: &mut Memory,
        vm: &mut Vm,
        root: &mut UserVm,
        directory: u64,
        limit: u64,
    ) -> Result {
        if self.root.is_some() || self.admitted || directory == 0 || directory & (PAGE - 1) != 0 {
            return Err(EINVAL);
        }
        let span = self.private_end.checked_sub(self.directory).ok_or(EIO)?;
        let end = directory.checked_add(span).ok_or(EOVERFLOW)?;
        if end > limit || limit > 1 << 42 {
            return Err(ENOSPC);
        }
        // Validate EVERY retained firmware mapping and fresh application path
        // before the first clear. A foreign PTE never becomes ours by reuse.
        for &(va, physical, size, flags) in &self.metadata {
            for offset in (0..size).step_by(PAGE as usize) {
                if vm.pte(memory, 2, va + offset)? != physical + offset | flags {
                    return Err(EIO);
                }
            }
        }
        for (base, blocks) in [
            (directory, &self.private),
            (raw::CONTEXT_BASE, &self.compact),
        ] {
            for block in blocks {
                root.prepare(base + block.offset, block.size)?;
            }
        }
        // Geometry blocks may be chunked; internal chunk ends are already
        // covered by the following block. The final end retains its guard.
        for block in &self.compact {
            if raw::CONTEXT_BASE + block.offset >= 0x1001000000 {
                root.prepare(raw::CONTEXT_BASE + block.offset + block.size, PAGE)?;
            }
        }
        let table = directory + operands::DIRECTORY_SIZE;
        let base = table + PAGE;
        for i in 0..INITIAL_BUFFERS {
            root.prepare(
                base + u64::from(i) * operands::BUFFER_STRIDE + operands::BUFFER_SIZE,
                0x8000,
            )?;
        }
        for id in &self.block_page_ids {
            root.prepare(raw::CONTEXT_BASE + u64::from(*id)*0x8000 + 0x20000, 0x8000)?;
        }
        for &(va, size) in &[
            (raw::GPU_PAGE_LIST, raw::PAGE_LIST_SIZE as u64),
            (0x1000080000, PAGE),
        ] {
            root.prepare(va, size)?;
        }
        map_private(memory, root, directory, &self.private)?;
        map_private(memory, root, raw::CONTEXT_BASE, &self.compact)?;
        let mut entries = bytes(PAGE as usize)?;
        let mut page = bytes(PAGE as usize)?;
        for i in 0..INITIAL_BUFFERS as usize {
            let buffer = base + i as u64 * operands::BUFFER_STRIDE;
            for j in 0..2048usize {
                page[j * 8..j * 8 + 8].copy_from_slice(&(buffer + j as u64 * 0x1000).to_le_bytes());
            }
            write_low(memory, root, directory + i as u64 * PAGE, &page)?;
            entries[i * 64..i * 64 + 8].copy_from_slice(&(buffer | 1 << 63).to_le_bytes());
        }
        write_low(memory, root, table, &entries)?;
        let support = operands::OperandSupportG16C::for_render(
            directory,
            table,
            self.auxiliary,
            INITIAL_BUFFERS,
            REGISTERED,
            REGISTERED,
        )
        .ok_or(EINVAL)?;
        page.fill(0);
        page[..support.as_bytes().len()].copy_from_slice(support.as_bytes());
        vm.write(memory, 2, self.support, &page)?;
        page.fill(0);
        vm.write(memory, 2, self.auxiliary, &page)?;
        for (alias_va, original, size) in [
            (
                raw::GPU_PAGE_LIST,
                self.parameter.page_list,
                raw::PAGE_LIST_SIZE as u64,
            ),
            (0x1000080000, self.parameter.scene_list, PAGE),
        ] {
            for offset in (0..size).step_by(PAGE as usize) {
                alias(
                    root,
                    alias_va + offset,
                    vm.pte(memory, 2, original + offset)? & ADDRESS,
                    PAGE,
                )?;
            }
        }
        self.directory = directory;
        self.table = table;
        self.private_end = end;
        self.root = Some(root.lease()?);
        Ok(())
    }

    pub(super) fn prepare_geometry(
        &mut self,
        root: &mut UserVm,
        sizes: [u64; 3],
    ) -> Result<Option<(Memory, PreparedGeometryG16C)>> {
        if self.root.as_ref().is_none_or(|r| r.root() != root.root()) { return Err(EIO); }
        let mut staged = Memory::detached();
        let mut geometry = self.geometry;
        let mut cursor = self.geometry_next;
        let mut blocks = KVec::new();
        for (i, size) in sizes.into_iter().enumerate() {
            if size == 0 {
                return Err(EINVAL);
            }
            if geometry[i].1 >= size {
                continue;
            }
            let capacity = size
                .max(PAGE)
                .checked_next_power_of_two()
                .ok_or(EOVERFLOW)?;
            let span = capacity.checked_add(PAGE).ok_or(EOVERFLOW)?;
            let mut address = cursor;
            loop {
                if address.checked_add(span).ok_or(EOVERFLOW)? > 0x1100000000 {
                    return Err(ENOSPC);
                }
                let mut conflict = self.compact_conflict(address,span);
                for offset in (0..span).step_by(PAGE as usize) {
                    if conflict.is_some() { break; }
                    if root.pte(address + offset)? != 0 {
                        conflict = Some(address + offset + PAGE);
                        break;
                    }
                }
                if let Some(next) = conflict {
                    address = next;
                    continue;
                }
                break;
            }
            root.prepare(address, span)?;
            private_span(&mut staged, &mut blocks, address - raw::CONTEXT_BASE, capacity)?;
            geometry[i] = (address, capacity);
            cursor = address + span;
        }
        if blocks.is_empty() {
            self.root.as_mut().ok_or(EIO)?.refresh(root)?;
            return Ok(None);
        }
        self.compact.reserve(blocks.len(), GFP_KERNEL)?;
        let lease = root.lease()?;
        for block in &blocks { staged.clean(block.physical, block.size as usize)?; }
        Ok(Some((staged, PreparedGeometryG16C { blocks, geometry, next: cursor, root: lease })))
    }

    pub(super) fn geometry_addresses(&self) -> [u64; 3] { self.geometry.map(|(address, _)| address) }

    pub(super) fn install_geometry(&mut self, root: &mut UserVm,
        prepared: PreparedGeometryG16C) -> Result<[u64; 3]> {
        map_prepared_private(root, raw::CONTEXT_BASE, &prepared.blocks)?;
        for block in prepared.blocks {
            self.compact.push(block, GFP_KERNEL).expect("reserved prepared geometry backing");
        }
        self.geometry = prepared.geometry;
        self.geometry_next = prepared.next;
        self.root = Some(prepared.root);
        Ok(self.geometry_addresses())
    }

    #[allow(dead_code)]
    pub(super) fn fit_geometry(&mut self, memory: &mut Memory, root: &mut UserVm,
        sizes: [u64; 3]) -> Result<[u64; 3]> {
        let Some((staged, prepared)) = self.prepare_geometry(root, sizes)? else { return Ok(self.geometry_addresses()); };
        memory.absorb(staged)?;
        self.install_geometry(root, prepared)
    }

    pub(super) fn admit(
        &mut self,
        session: &mut Session,
        root: &UserVm,
        context: u16,
        high_root: u64,
        tag: u32,
    ) -> Result {
        if self.admitted {
            return Ok(());
        }
        if !(1..64).contains(&context)
            || self
                .root
                .as_ref()
                .is_none_or(|lease| root.root() != lease.root())
            || high_root == 0
            || high_root & (PAGE - 1) != 0
            || high_root & !ADDRESS != 0
        {
            return Err(EINVAL);
        }
        let memory = session.memory.as_ref().ok_or(EIO)?;
        let vm = session.vm.as_ref().ok_or(EIO)?;
        let at = session.ttbs + u64::from(context) * 16;
        memory.invalidate(at, 16)?;
        if [memory.read64(at)?, memory.read64(at + 8)?]
            != [root.root(), high_root].map(|p| p | u64::from(context) << 48 | 1)
        {
            return Err(EIO);
        }
        for &(va, physical, size, flags) in &self.metadata {
            for offset in (0..size).step_by(PAGE as usize) {
                if vm.pte(memory, 2, va + offset)? != physical + offset | flags {
                    return Err(EIO);
                }
            }
        }
        for (base, blocks) in [
            (self.directory, &self.private),
            (raw::CONTEXT_BASE, &self.compact),
        ] {
            for block in blocks {
                for offset in (0..block.size).step_by(PAGE as usize) {
                    if root.pte(base + block.offset + offset)?
                        != block.physical + offset | GPU_WRITE
                    {
                        return Err(EIO);
                    }
                }
            }
        }
        for (alias, original, size) in [
            (
                raw::GPU_PAGE_LIST,
                self.parameter.page_list,
                raw::PAGE_LIST_SIZE as u64,
            ),
            (0x1000080000, self.parameter.scene_list, PAGE),
        ] {
            for offset in (0..size).step_by(PAGE as usize) {
                let expected = vm.pte(memory, 2, original + offset)? & ADDRESS;
                if root.pte(alias + offset)? != expected | GPU_WRITE {
                    return Err(EIO);
                }
            }
        }
        self.root.as_mut().ok_or(EIO)?.refresh(root)?;
        vm.flush_tables(memory)?;
        Vm::invalidate_gpu();
        session.g16c_register_flist(&RegistrationG16C {
            context: u32::from(context),
            support: self.support,
            table: self.table,
            added: INITIAL_BUFFERS - REGISTERED,
            previous: REGISTERED,
            mode: 0,
            tag,
            reply_value: 0,
        })?;
        self.admitted = true;
        Ok(())
    }
}
