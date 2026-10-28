// SPDX-License-Identifier: GPL-2.0-only

//! Source-owned Max render pair lowering in the common scheduler/runtime.

use super::super::super::g17p_render as command;
use super::*;
const FLAGS: [u64; 7] = [
    PRIVATE,
    PRIVATE,
    PRIVATE,
    PRIVATE,
    0x008000000000044b,
    0x008000000000044b,
    SHARED,
];

impl RenderTemplateSeedG16C {
    pub(crate) fn prepare(self)->Result<PreparedRenderTemplateG16C> {
        let (memory,resource)=render::ResourcesG16C::build(self.pool,self.pool,self.directory,self.limit)?;
        if crate::module_parameters::submission_log.value()>=3 {
            kernel::pr_info!("G16C: RENDER_TEMPLATE_PREPARED pool={} context={} outside_runtime_lock\n",self.pool,self.context);
        }
        Ok(PreparedRenderTemplateG16C {seed:self,memory,resource})
    }
}
fn geometry_sizes(p:&command::Parameters)->Result<[u64;3]> {
    let (_,stride,tpc)=p.scratch_layout().map_err(|_|EINVAL)?;
    Ok([stride.checked_mul(u64::from(super::super::super::g17p_render_lifecycle::SCRATCH_SLOTS)).ok_or(EOVERFLOW)?,
        tpc.checked_mul(4).ok_or(EOVERFLOW)?,0x200+if p.layers>1 {0x100}else{0}])
}
impl State {
    pub(in super::super) fn render_scratch_seed(&self,session:&Session,incoming:&compute::Client,p:&command::Parameters)
        ->Result<Option<GeometrySeedG16C>> {
        let Some(qi)=self.retained_render_index(session,incoming,p) else {return Ok(None);};
        let owner=self.queues[qi].owner;let o=&self.owners[owner];
        let client=o.client.as_ref().ok_or(EIO)?;
        // Geometry addresses must see the admitted caller's mappings. The
        // common rebind transaction runs before normal geometry staging;
        // defer this seed until that exact caller/window has been installed.
        if !same(client,incoming) || o.directory!=p.kernel_start || o.private_limit!=p.kernel_end {return Ok(None);}
        let r=o.render.as_ref().ok_or(EIO)?;let root=&client.root;
        let Some(plan)=r.geometry_plan(root,geometry_sizes(p)?)? else {return Ok(None);};
        Ok(Some(GeometrySeedG16C {owner,pool:o.pool,context:o.context,plan,root:root.lease()?}))
    }
    pub(super) fn commit_render_scratch(&mut self,session:&mut Session,mut prepared:PreparedScratchG16C)->StageResult<bool> {
        if !session.render_pool_available(prepared.pool) {return Ok(false);}
        let Some(o)=self.owners.get_mut(prepared.owner) else {return Ok(false);};
        if o.pool!=prepared.pool || o.context!=prepared.context {return Ok(false);}
        let Some(client)=o.client.as_mut() else {return Ok(false);};
        let Some(r)=o.render.as_mut() else {return Ok(false);};
        if !r.preflight_geometry(&mut client.root,&prepared.plan,&mut prepared.geometry).map_err(StageError::allocation)? {return Ok(false);}
        initial_allocation_checkpoint(7,&mut self.initial_allocation_faulted,&prepared.memory)?;
        session.memory.as_mut().ok_or(EIO)?.absorb(prepared.memory).map_err(StageError::allocation)?;
        r.install_geometry(&mut client.root,prepared.geometry)?;
        Ok(true)
    }
}

impl State {
    fn cold_render_owner(&self,session:&Session,incoming:&compute::Client,p:&command::Parameters)->Option<usize> {
        let pool=self.render_pool(session,incoming,p)?;
        let oi=self.owners.iter().position(|o|o.pool==pool && o.render.is_none()
            && o.client.as_ref().is_some_and(|c|same(c,incoming)))?;
        let o=&self.owners[oi];
        // Preserve Neo-style retained reuse: an existing dormant graph is
        // rebound by normal admission and needs no speculative replacement.
        if self.owners.iter().any(|old|old.pool==pool && old.render.as_ref().is_some_and(|r|r.dormant()
            && r.private_end.checked_sub(r.directory).and_then(|span|o.private_next.checked_add(span))
                .is_some_and(|end|end<=o.private_limit))) {return None;}
        Some(oi)
    }
    pub(in super::super) fn render_template_size(&self,session:&Session,incoming:&compute::Client,p:&command::Parameters)
        ->Result<Option<usize>> {
        Ok(self.cold_render_owner(session,incoming,p).map(|i|self.owners[i].client.as_ref().unwrap().root.table_count()))
    }
    pub(in super::super) fn capture_render_template(&self,session:&Session,incoming:&compute::Client,p:&command::Parameters,
        storage:super::super::super::g17p_user_vm::TableStorage)->Result<Option<RenderTemplateSeedG16C>> {
        let Some(owner)=self.cold_render_owner(session,incoming,p) else {return Ok(None);};
        let o=&self.owners[owner];let root=&o.client.as_ref().ok_or(EIO)?.root;
        let Some(snapshot)=root.capture_tables(storage)? else {return Ok(None);};
        Ok(Some(RenderTemplateSeedG16C {owner,pool:o.pool,context:o.context,directory:o.private_next,
            limit:o.private_limit,snapshot,_root:root.lease()?}))
    }
    pub(super) fn commit_render_template(&mut self,session:&mut Session,incoming:&compute::Client,p:&command::Parameters,
        mut prepared:PreparedRenderTemplateG16C)->StageResult<bool> {
        let seed=&prepared.seed;
        if self.cold_render_owner(session,incoming,p)!=Some(seed.owner) {return Ok(false);}
        let o=&mut self.owners[seed.owner];let root=&mut o.client.as_mut().ok_or(EIO)?.root;
        if o.pool!=seed.pool || o.context!=seed.context || o.private_next!=seed.directory
            || o.private_limit!=seed.limit || !seed.snapshot.matches(root) {return Ok(false);}
        if prepared.resource.private_end>o.private_limit {return Err(StageError::Rejected(ENOSPC));}
        let memory=session.memory.as_mut().ok_or(EIO)?;let vm=session.vm.as_mut().ok_or(EIO)?;
        prepared.resource.preflight(memory,vm,root).map_err(StageError::allocation)?;
        initial_allocation_checkpoint(4,&mut self.initial_allocation_faulted,&prepared.memory)?;
        memory.absorb(prepared.memory).map_err(StageError::allocation)?;
        prepared.resource.install(memory,vm,root)?;
        o.private_next=prepared.resource.private_end;o.render=Some(prepared.resource);
        kernel::dev_info!(session.peers[0].data.dev.as_ref(),"G16C: PB {} allocated for context {} common off-lock template commit\n",o.pool,o.context);
        Ok(true)
    }
}

impl State {
    pub(in super::super) fn render_needs_private(&self,session:&Session,incoming:&compute::Client,p:&command::Parameters)->bool {
        // A retained physical root already owns its private backing. The
        // common PTE rebind plan changes callers/windows without replacing it.
        self.retained_render_index(session,incoming,p).is_none()
            && self.render_pool(session,incoming,p).is_some_and(|pool|
                self.owner_private_required(pool,incoming,p.kernel_start,p.kernel_end))
    }
    pub(crate) fn render_pool(&self,session:&Session,incoming:&compute::Client,p:&command::Parameters)->Option<u32> {
        super::super::super::g17p_render_lifecycle::select_pool(
            self.render_ordinal,super::super::super::g17p_render_runtime::pool_count() as u32,
            |pool| {
                if !session.render_pool_available(*pool) {return false;}
                let Some(o)=self.owners.iter().find(|o|o.pool==*pool) else {
                    return super::super::super::g17p_render_lifecycle::render_asid(*pool,session.independent_compute.asid_mask()).is_some();
                };
                o.private_next.checked_sub(o.directory).and_then(|span|p.kernel_start.checked_add(span))
                    .is_some_and(|end|end<=p.kernel_end)
                    && self.queues.iter().filter(|q|q.owner<self.owners.len() && self.owners[q.owner].pool==*pool)
                        .all(|q|q.generation.idle())
            },
            |pool|self.owners.iter().any(|o|o.pool==*pool&&o.client.as_ref().is_some_and(|c|same(c,incoming))))
    }
    fn retained_render_index(&self,session:&Session,incoming:&compute::Client,p:&command::Parameters)->Option<usize> {
        let key=p.queue_owner?;p.firmware_priority?;
        let pool=self.render_pool(session,incoming,p)?;
        let owns_pool=|q:&&Queue|q.live&&q.generation.idle()&&self.owners[q.owner].pool==pool
            && self.owners[q.owner].client.is_some()
            && self.owners[q.owner].render.as_ref().is_some_and(|r|r.attached());
        self.queues.iter().enumerate().find(|(_,q)|owns_pool(q)&&q.key==key)
            .or_else(||self.queues.iter().enumerate().find(|(_,q)|owns_pool(q))).map(|(index,_)|index)
    }
    pub(crate) fn selected_render_client(&self,session:&Session,incoming:&compute::Client,p:&command::Parameters)->Option<&compute::Client> {
        let qi=self.retained_render_index(session,incoming,p)?;
        self.owners[self.queues[qi].owner].client.as_ref()
    }
    pub(crate) fn render_rebind_size(&self,session:&Session,incoming:&compute::Client,p:&command::Parameters)
        ->Result<Option<usize>> {
        let Some(qi)=self.retained_render_index(session,incoming,p) else {return Ok(None);};
        let o=&self.owners[self.queues[qi].owner];let installed=o.client.as_ref().ok_or(EIO)?;
        Ok((!same(installed,incoming)||o.directory!=p.kernel_start||o.private_limit!=p.kernel_end)
            .then(||installed.root.table_count()))
    }
    pub(crate) fn capture_render_rebind(&self,session:&Session,incoming:&compute::Client,p:&command::Parameters,
        storage:Option<super::super::super::g17p_user_vm::TableStorage>)
        ->Result<Option<Option<super::super::super::g17p_boot::PoolRebindSeed>>> {
        let Some(qi)=self.retained_render_index(session,incoming,p) else {return Ok(Some(None));};
        let o=&self.owners[self.queues[qi].owner];let client=o.client.as_ref().ok_or(EIO)?;
        let moving=o.directory!=p.kernel_start||o.private_limit!=p.kernel_end;
        if same(client,incoming)&&!moving {return Ok(Some(None));}
        let Some(storage)=storage else {return Ok(None);};
        let seed=super::super::super::g17p_boot::PoolRebindSeed::capture(
            o.render.as_ref().ok_or(EIO)?.parameter.slot,o.context,client,storage,
            Arc::new(KVec::new(),GFP_KERNEL)?,KVec::new(),KVec::new())?;
        let Some(seed)=seed else {return Ok(None);};
        let seed=if moving {
            let mut blocks=KVec::with_capacity(o.private.len(),GFP_KERNEL)?;
            for b in &o.private {blocks.push((b.offset,b.physical,b.size),GFP_KERNEL)?;}
            o.render.as_ref().ok_or(EIO)?.append_private_blocks(o.directory,&mut blocks)?;
            seed.relocate_private([o.directory,o.private_next],[p.kernel_start,p.kernel_end],blocks)
        } else {seed};
        Ok(Some(Some(seed)))
    }
    fn rebind_render_owner(&mut self,session:&mut Session,client:&mut Option<compute::Client>,p:&command::Parameters,
        plan:Option<&mut super::super::super::g17p_boot::PreparedPoolRebind>)->Result<bool> {
        let Some(incoming)=client.as_ref() else {return Ok(true);};
        let Some(qi)=self.retained_render_index(session,incoming,p) else {return Ok(true);};
        let oi=self.queues[qi].owner;
        let o=&self.owners[oi];let installed=o.client.as_ref().ok_or(EIO)?;
        let moving=o.directory!=p.kernel_start||o.private_limit!=p.kernel_end;
        let private=moving.then_some(([o.directory,o.private_next],[p.kernel_start,p.kernel_end]));
        if !same(installed,incoming)||moving {
            let Some(plan)=plan else {return Ok(false);};
            if !plan.matches_pool(o.render.as_ref().ok_or(EIO)?.parameter.slot,o.context)
                || !plan.matches(installed,incoming)||!plan.matches_private(private) {return Ok(false);}
            let memory=session.memory.as_ref().ok_or(EIO)?;
            let at=session.ttbs+u64::from(o.context)*16;
            memory.invalidate(at,16)?;
            let expected=if o.published {
                [installed.root.root(),o.high_root].map(|root|root|(u64::from(o.context)<<48)|1)
            } else {o.invalid_roots};
            if [memory.read64(at)?,memory.read64(at+8)?]!=expected {return Err(EIO);}
            if moving {
                let old=o.directory;let root=installed.root.root();let context=o.context;
                let end=p.kernel_start+(o.private_next-old);
                let render_directory=p.kernel_start+(o.render.as_ref().ok_or(EIO)?.directory-old);
                // Every allocation for host directory serialization precedes
                // the existing firmware retirement/TTBR transaction.
                let mut directory=bytes(PAGE as usize)?;
                let mut entries=bytes(operands::TABLE_SIZE as usize)?;
                self.retire_owner(session,oi,true)?;
                let o=&mut self.owners[oi];let installed=o.client.as_mut().ok_or(EIO)?;
                plan.commit(installed)?;
                if installed.root.root()!=root {return Err(EIO);}
                let memory=session.memory.as_mut().ok_or(EIO)?;
                let vm=session.vm.as_mut().ok_or(EIO)?;
                reset_mapped_private(memory,&installed.root,p.kernel_start,&o.private)?;
                for physical in [o.support_pages[0],o.support_pages[1],o.high_root] {
                    memory.zero(physical,PAGE as usize)?;memory.clean(physical,PAGE as usize)?;
                }
                o.dispatch_initial=initialize_operand_data(memory,vm,&installed.root,p.kernel_start,o.support,
                    &o.operand_buffers,&mut directory,&mut entries)?;
                o.render.as_mut().ok_or(EIO)?.rebind_mapped(memory,vm,&mut installed.root,render_directory,p.kernel_end)?;
                o.operand_state=o.operand_state.map(|address|p.kernel_start+(address-old));
                o.directory=p.kernel_start;o.private_limit=p.kernel_end;o.private_next=end;
                o.operand_probe_seeded=false;o.operand_probe_pending=false;
                let q=&mut self.queues[qi];q.storage.reset_retained(memory,vm,p.firmware_priority.ok_or(EINVAL)?.try_into()?)?;
                q.live=true;q.render_count=0;q.generation.window=0;
                kernel::dev_info!(session.peers[0].data.dev.as_ref(),
                    "G16C: retained render private window moved context={} root={:#x} pool={} old={:#x} new={:#x} common prepared PTE commit; physical root and ASID unchanged\n",
                    context,root,o.render.as_ref().ok_or(EIO)?.parameter.slot,old,p.kernel_start);
            } else {
                let o=&mut self.owners[oi];let installed=o.client.as_mut().ok_or(EIO)?;
                plan.commit(installed)?;
                o.render.as_mut().ok_or(EIO)?.refresh_root(&installed.root)?;
                kernel::dev_info!(session.peers[0].data.dev.as_ref(),"G16C: retained render root rebound context={} root={:#x} pool={} through common prepared PTE commit\n",
                    o.context,installed.root.root(),o.render.as_ref().ok_or(EIO)?.parameter.slot);
            }
        }
        let priority=p.firmware_priority.ok_or(EINVAL)?;
        let q=&mut self.queues[qi];let words=super::super::super::g17p_queue::priority_profile(priority).map_err(|_|EINVAL)?;
        if u32::from(q.storage.lanes[0].priority)!=priority {
            for lane in &mut q.storage.lanes {
                session.vm.as_ref().ok_or(EIO)?.write(session.memory.as_mut().ok_or(EIO)?,2,lane.queue+0x28,&words)?;
                lane.priority=priority.try_into()?;
            }
        }
        q.key=p.queue_owner.ok_or(EINVAL)?;self.owners[oi].close_requested=false;
        drop(client.take());Ok(true)
    }
    fn configured_render(input:&command::Parameters,n:u32,sequence:u32,geo:[u64;3])->Result<command::Parameters> {
        let mut p=*input;
        p.context_base=0x1000000000;p.deflake_1=0x1000068da0;p.deflake_2=0x1000068120;p.deflake_3=0x1000068000;
        p.ta_status=0x1000078000;p.fragment_status=0x1000490000;p.aux_fb=0x10004d0000;
        p.reactive_tvb_growth=true;
        if crate::module_parameters::g16c_parameter_oom_test.value()==n {
            p.reactive_tvb_growth=false;p.fragment_sync_grow=Some(true);
        }
        p.aux_fb_flags=0xc000|(p.aux_fb_flags&(1<<18));
        // Same eight-block tilemap/heap ring as the Neo retained lifecycle.
        let (heap,stride,_)=p.scratch_layout().map_err(|_|EINVAL)?;
        p.tilemap=geo[0].checked_add(super::super::super::g17p_render_lifecycle::scratch_offset(sequence,stride)).ok_or(EOVERFLOW)?;
        p.tpc=geo[1];p.heapmeta=p.tilemap.checked_add(heap).ok_or(EOVERFLOW)?
            + if p.layers>1 {0x100}else{0};Ok(p)
    }
    fn render_seed(&self,qi:usize,storage:u32,input:&command::Parameters)
        ->Result<(command::Parameters,super::super::super::g17p_render_runtime::WrappedSeed)> {
        use super::super::super::g17p_render_runtime::{WrappedSeed,WrappedLane};
        let q=&self.queues[qi];let o=&self.owners[q.owner];let r=o.render.as_ref().ok_or(EIO)?;
        let geo=r.geometry_addresses();let p=Self::configured_render(input,self.render_ordinal,r.sequence,geo)?;
        Ok((p,WrappedSeed {base:super::super::super::g17p_render_runtime::wrapped_storage_base(storage)?,storage,ordinal:self.render_ordinal,local:q.render_count,
            sequence:r.sequence,milestone:r.sequence_base.checked_add(r.sequence).ok_or(EOVERFLOW)?,grids:super::super::super::g17p_render_lifecycle::pool_grids(o.pool).map_err(|_|EINVAL)?.map(|grid|grid as u8),context:o.context,slot:r.parameter.slot,support:r.support,
            manager:r.manager,scene:r.scene+u64::from(r.sequence%79)*0x80,empty:r.empty_scene,
            layermeta:p.heapmeta-if p.layers>1 {0x100}else{0},queue_control:q.storage.context,uuid:q.storage.uuid,
            lanes:core::array::from_fn(|i| {let l=&q.storage.lanes[i];
                WrappedLane{queue:l.queue,scratch_low:l.scratch_low,scratch_high:l.scratch_high,priority:input.firmware_priority.unwrap_or(u32::from(l.priority)) as u8}})}))
    }
    pub(crate) fn render_preparation_seed(&self,session:&Session,input:&command::Parameters,incoming:&compute::Client)
        ->Result<Option<KBox<super::super::super::g17p_render_runtime::PreparationSeed>>> {
        let Some(qi)=self.retained_render_index(session,incoming,input) else {return Ok(None);};
        let o=&self.owners[self.queues[qi].owner];
        let installed=o.client.as_ref().ok_or(EIO)?;
        // Rebind the retained root/window before selecting or clearing its
        // scratch. An incoming larger geometry cannot use the old pool's
        // tilemap span while that root transaction is still pending.
        if !same(installed,incoming) || o.directory!=input.kernel_start
            || o.private_limit!=input.kernel_end {return Ok(None);}
        if o.render.as_ref().ok_or(EIO)?.geometry_plan(&installed.root,geometry_sizes(input)?)?.is_some() {
            return Ok(None);
        }
        let Some(storage)=session.next_wrapped_render_storage(self.render_ordinal) else {return Ok(None);};
        let (p,wire)=self.render_seed(qi,storage,input)?;
        // A cold template installs backing before admission fits geometry.
        // Let normal staging perform that step before serializing descriptors.
        if [p.tilemap,p.tpc,p.heapmeta].contains(&0) {return Ok(None);}
        let stamps=session.timestamp_cache_seed(&[p.ta_user_timestamp_start,p.ta_user_timestamp_end,p.fragment_user_timestamp_start,p.fragment_user_timestamp_end])?;
        super::super::super::g17p_render_runtime::PreparationSeed::wrapped(wire,p,incoming,stamps).map(Some)
    }

    /// The selected slot is retired and held by the common preparation lease.
    /// Each span pins its physical allocation while CPU clearing runs off-lock.
    pub(in super::super) fn render_clearing_seed(&self,memory:&Memory,seed:&super::super::super::g17p_render_runtime::PreparationSeed)
        ->Result<KVec<super::super::super::g17p_memory::ZeroSpan>> {
        let storage=seed.wrapped_storage().ok_or(EIO)?;
        let mut spans=KVec::new();
        let o=self.owners.iter().find(|o|o.pool==seed.pair() && o.client.is_some()).ok_or(EIO)?;
        let r=o.render.as_ref().ok_or(EIO)?;let root=&o.client.as_ref().ok_or(EIO)?.root;
        let (address,size)=seed.wrapped_tilemap_span()?;
        let (base,capacity)=r.tilemap_span();
        if address<base || address.checked_add(size).ok_or(EOVERFLOW)?
            >base.checked_add(capacity).ok_or(EOVERFLOW)? {return Err(EIO);}
        // The pool and work-slot leases pin all retired backing during this
        // common off-lock clear. Fresh slots remain zero; wrapped slots lose
        // only their old tilemap/heap contents, preserving the persistent TPC.
        let mut offset=0;
        while offset<size {
            let at=address+offset;let pa=(root.pte(at&!(PAGE-1))?&ADDRESS)+(at&(PAGE-1));
            let count=(size-offset).min(PAGE-(at&(PAGE-1)))as usize;
            spans.push(memory.zero_span(pa,count)?,GFP_KERNEL)?;offset+=count as u64;
        }
        if let Some((_,pages))=self.render_work_pages.iter().find(|(slot,_)|*slot==storage) {
            spans.reserve(pages.len(),GFP_KERNEL)?;
            for pa in pages {spans.push(memory.zero_span(*pa,PAGE as usize)?,GFP_KERNEL)?;}
        }
        // Fresh WorkBackingG16C pages are already zeroed/cleaned off-lock.
        Ok(spans)
    }
    fn render_storage(&mut self, memory: &mut Memory, vm: &mut Vm, storage:u32,backing:&mut Option<WorkBackingG16C>,cleared:bool) -> StageResult<()> {
        if !cleared {return Err(EIO.into());}

        let base=super::super::super::g17p_render_runtime::wrapped_storage_base(storage)?;
        let alias = 0x7000000000 + base - work::DESCRIPTOR_BASE_G16C;
        if !self.render_work_pages.iter().any(|(slot,_)|*slot==storage) {
            self.render_work_pages.reserve(1,GFP_KERNEL).map_err(StageError::allocation)?;
            prepare_work_mappings(memory,vm,base,FLAGS,&[0,2]).map_err(StageError::allocation)?;
            let WorkBackingG16C {memory:staged,pages}=backing.take().ok_or(EIO)?;
            initial_allocation_checkpoint(6, &mut self.initial_allocation_faulted, &staged)?;
            memory.absorb(staged).map_err(StageError::allocation)?;
            install_work_pages(memory, vm, base, FLAGS, &[0, 2], pages)?;
            self.render_work_pages.push((storage,pages),GFP_KERNEL).expect("reserved render storage");
        }
        let pages = self.render_work_pages.iter().find(|(slot,_)|*slot==storage).ok_or(EIO)?.1;
        for (i, flags) in FLAGS.into_iter().enumerate() {
            if vm.pte(memory, 2, base + i as u64 * PAGE)? != pages[i] | flags {
                return Err(EIO.into());
            }
        }
        for i in [0, 2] {
            if vm.pte(memory, 0, alias + i as u64 * PAGE)? != pages[i] | GPU_READ {
                return Err(EIO.into());
            }
        }
        Ok(())
    }
    fn stage_render(
        &mut self,
        session: &mut Session,
        key: Key,
        client: &mut Option<compute::Client>,
        input: &command::Parameters,
        priority: u32,
        kernel_start: u64,
        kernel_end: u64,
        private:&mut Option<OwnerPrivateG16C>,
        transport:&mut Option<scheduled_queues::QueueBackingG16C>,
        work_backing:&mut Option<WorkBackingG16C>,
        prepared:Option<KBox<super::super::super::g17p_render_runtime::PreparedAppend>>,
        rebind:Option<&mut super::super::super::g17p_boot::PreparedPoolRebind>,
    ) -> StageResult<Option<Arc<RenderReceipt>>> {
        let caller=client.as_ref().or_else(||self.client(key)).ok_or(EIO)?;
        let Some(pool)=self.render_pool(session,caller,input) else {return Ok(None);};
        if self.render_needs_private(session,caller,input)
            && !private.as_ref().is_some_and(|plan|plan.matches(caller,kernel_start,kernel_end)) {return Ok(None);}

        if let Some(caller)=client.as_ref().or_else(||self.client(key)) {
            for bo in &caller.buffers {session.retain_source_backing(bo)?;}
        }
        if !self.rebind_render_owner(session,client,input,rebind)? {return Ok(None);}
        self.compute_mode=false;
        self.leave_compute_mode();
        self.release_requested(session)?;
        let Some(qi) = self.prepare_queue(session, key, client, kernel_start, kernel_end, priority, pool,private,transport)? else {return Ok(None);};
        self.queues[qi].generation.leases.prepare().map_err(StageError::allocation)?;
        let oi = self.queues[qi].owner;
        // Vertex scratch uses the owned queue operand directory at the VM's
        // requested origin. Admission protects this pool's GPU/CPU leases;
        // the render resource constructor owns all 28 backing tranches.
        if self.owners[oi].directory != kernel_start {
            return Err(EIO.into());
        }
        if self.owners[oi].render.is_none() {
            // PB identities belong to the render cache, not an idle compute
            // owner's ASID. Detached resources have completed owned-root and
            // control retirement; all GPU/CPU leases are quiescent above.
            if let Some(from) = self
                .owners
                .iter()
                .position(|o| o.pool==pool && o.render.as_ref().is_some_and(|r| r.dormant()
                    && r.private_end.checked_sub(r.directory)
                        .and_then(|span| self.owners[oi].private_next.checked_add(span))
                        .is_some_and(|end| end <= self.owners[oi].private_limit)))
            {
                let slot = self.owners[from].render.as_ref().ok_or(EIO)?.parameter.slot;
                self.owners[oi].render = self.owners[from].render.take();
                kernel::dev_info!(session.peers[0].data.dev.as_ref(),
                    "G16C: dormant PB {} transferred context {} -> {} with no retained render root\n",
                    slot, self.owners[from].context, self.owners[oi].context);
            }
        }
        // The existing common template phase builds unpublished PB backing
        // off-lock. Retry this node once its owner exists; never allocate a
        // second bulk resource graph while holding the publication mutex.
        if self.owners[oi].render.is_none() {return Ok(None);}
        if !self.owners[oi].render.as_ref().ok_or(EIO)?.attached() {
            let o = &mut self.owners[oi];
            let r = o.render.as_mut().ok_or(EIO)?;
            let span = r.private_end.checked_sub(r.directory).ok_or(EIO)?;
            if o.private_next.checked_add(span).ok_or(EOVERFLOW)? > o.private_limit {
                return Err(StageError::Rejected(ENOSPC));
            }
            r.rebind(
                session.memory.as_mut().ok_or(EIO)?,
                session.vm.as_mut().ok_or(EIO)?,
                &mut o.client.as_mut().ok_or(EIO)?.root,
                o.private_next,
                o.private_limit,
            )?;
            o.private_next = r.private_end;
        }
        let n = self.render_ordinal;
        let first_event = super::super::super::g17p_render_lifecycle::pool_grids(pool).map_err(|_|EINVAL)?[0] as u8;
        if n >= 0x3fffffff {
            return Err(EOVERFLOW.into());
        }
        let mut p = *input;
        p.context_base = 0x1000000000;
        p.deflake_1 = 0x1000068da0;
        p.deflake_2 = 0x1000068120;
        p.deflake_3 = 0x1000068000;
        p.ta_status = 0x1000078000;
        p.fragment_status = 0x1000490000;
        p.aux_fb = 0x10004d0000;
        // Owned kind-6 requests are serviced by the common runtime before polling retirement.
        p.reactive_tvb_growth = true;
        if crate::module_parameters::g16c_parameter_oom_test.value()==n {
            // Explicit one-shot qualification matches the source OOM7 gate.
            p.reactive_tvb_growth=false;
            p.fragment_sync_grow=Some(true);
            kernel::dev_info!(session.peers[0].data.dev.as_ref(),"G16C: test-only parameter OOM policy at render ordinal={} TA892=0 FG2100=1\n",n);
        }
        p.aux_fb_flags = 0xc000 | (p.aux_fb_flags & (1 << 18));
        let o = &mut self.owners[oi];
        let r = o.render.as_mut().ok_or(EIO)?;
        let root=&mut o.client.as_mut().ok_or(EIO)?.root;
        if r.geometry_plan(root,geometry_sizes(&p)?)?.is_some() {return Ok(None);}
        r.refresh_root(root)?;
        let geo=r.geometry_addresses();
        p=Self::configured_render(input,n,r.sequence,geo)?;
        let Some(storage)=session.next_wrapped_render_storage(n) else {return Ok(None);};
        if !self.render_work_pages.iter().any(|(old,_)|*old==storage) && work_backing.is_none() {return Ok(None);}
        let (configured,wire)=self.render_seed(qi,storage,input)?;
        let base=wire.base;
        let Some(prepared)=prepared.filter(|plan|plan.matches_wrapped(wire,&configured,
            self.owners[oi].client.as_ref().unwrap())) else {return Ok(None);};
        let o=&mut self.owners[oi];let r=o.render.as_mut().ok_or(EIO)?;
        let receipt = RenderReceipt::new(n).map_err(StageError::allocation)?;
        let addresses = [
            p.ta_user_timestamp_start,
            p.ta_user_timestamp_end,
            p.fragment_user_timestamp_start,
            p.fragment_user_timestamp_end,
        ];
        let completion = Completion {
            client: o.client.as_ref().ok_or(EIO)?.lease().map_err(StageError::allocation)?,
            stamps: session
                .timestamps
                .as_ref()
                .ok_or(EIO)?
                .cache_seed(&addresses).map_err(StageError::allocation)?,
            fence: receipt.fence.clone(),
            grid: first_event + 1,
            error: None,
        };
        let mut leases = KVec::new();
        leases.push(o.client.as_ref().ok_or(EIO)?.lease().map_err(StageError::allocation)?, GFP_KERNEL).map_err(StageError::allocation)?;
        let slot = r.parameter.slot;
        let manager = r.manager;
        let auxiliary = r.auxiliary;
        let resource_count = (r.sequence_base.checked_add(r.sequence).ok_or(EOVERFLOW)? + 1)
            .checked_mul(2)
            .ok_or(EOVERFLOW)?;
        session.reserve_wrapped_render().map_err(StageError::allocation)?;
        self.render_storage(
            session.memory.as_mut().ok_or(EIO)?,
            session.vm.as_mut().ok_or(EIO)?,storage,work_backing,prepared.wrapped_storage_cleared(),
        )?;
        let stamp = wire.local.wrapping_add(1).wrapping_shl(8);
        let mut drivers = KVec::new();
        for stage in 0..2 {
            drivers.push(
                Stamp {
                    address: base + 0x18000 + stage * 0x40,
                    value: stamp,
                },
                GFP_KERNEL,
            ).map_err(StageError::allocation)?;
        }
        let mut counters = KVec::new();
        counters.push(
            Stamp {
                address: base + 0x20c,
                value: 2,
            },
            GFP_KERNEL,
        ).map_err(StageError::allocation)?;
        let next_local = self.queues[qi].render_count.checked_add(1).ok_or(EOVERFLOW)?;
        flight_allocation_checkpoint(&mut self.initial_allocation_faulted)?;
        let memory = session.memory.as_mut().ok_or(EIO)?;
        let vm = session.vm.as_mut().ok_or(EIO)?;
        prepared.write_wrapped(memory,vm)?;
        let mut event = [0u8; 0x28];
        event[..8].copy_from_slice(&(base + 0x18200).to_le_bytes());
        event[0x10..0x14].copy_from_slice(&0x50u32.to_le_bytes());
        vm.write(memory, 2, base + 0x200, &event)?;
        vm.write(memory, 2, base + 0x18200, &2u32.to_le_bytes())?;
        vm.write(memory, 2, auxiliary, &resource_count.to_le_bytes())?;
        let [ta, fr, _] = &mut self.queues[qi].storage.lanes;
        let prepared = session.prepare_g16c_admission(
            &mut [
                Request {
                    lane: ta,
                    works: &[base + 0x4000, base + 0x40c0],
                    event: first_event,
                    tag: 0,
                },
                Request {
                    lane: fr,
                    works: &[base + 0xc000, base + 0xc0c0],
                    event: first_event + 1,
                    tag: 0,
                },
            ],
            Requirements {
                firmware: KVec::new(),
                driver: drivers,
                notifiers: KVec::new(),
                drains: KVec::new(),
                counters,
            },
            leases,
        )?;
        self.activate(session, oi)?;
        if !self.render_status {
            boot::activate_render_status(
                session.memory.as_mut().ok_or(EIO)?,
                session.vm.as_ref().ok_or(EIO)?,
            )?;
            self.render_status = true;
        }
        let o = &mut self.owners[oi];
        o.render.as_mut().ok_or(EIO)?.admit(
            session,
            &o.client.as_ref().ok_or(EIO)?.root,
            o.context,
            o.high_root,
            0,
        )?;
        session.configure_g16c_engines(
            &BindingG16C::render(
                first_event,
                self.queues[qi].storage.lanes[0].scratch_low,
                self.queues[qi].storage.lanes[1].scratch_low,
            )
            .ok_or(EINVAL)?, false, true,
        )?;
        let [ta, fr, _] = &mut self.queues[qi].storage.lanes;
        let id = session.publish_g16c_submission(&mut [
            Request { lane: ta, works: &[base + 0x4000, base + 0x40c0], event: first_event, tag: 0 },
            Request { lane: fr, works: &[base + 0xc000, base + 0xc0c0], event: first_event + 1, tag: 0 },
        ], prepared)?;
        if crate::module_parameters::submission_log.value()!=0 {
            kernel::dev_info!(session.peers[0].data.dev.as_ref(),
                "G16C: RENDER_PUBLISHED pool={} context={} root={:#x} grids={},{} milestone={} ordinal={} scratch={:#x},{:#x}\n",
                pool,self.owners[oi].context,self.owners[oi].client.as_ref().ok_or(EIO)?.root.root(),
                first_event,first_event+1,wire.milestone+1,n,wire.lanes[0].scratch_high,wire.lanes[1].scratch_high);
        }
        self.queues[qi].generation.leases.published(receipt.fence.clone())?;
        receipt.retained_pool.store(slot,Ordering::Relaxed);
        for (stored,grid)in receipt.milestones.iter().zip([first_event,first_event+1]) {
            stored.store((u64::from(grid)<<32)|u64::from(wire.milestone+1),Ordering::Relaxed);
        }
        receipt.publication_ready.store(true, Ordering::Release);
        session.push_wrapped_render(
            RenderTicket {
                id,
                receipt: receipt.clone(),
                render_owner: oi,storage,
                growth_tokens: KVec::new(),
                render_work: parameter_error::RenderOwnerG16C {
                    context:u32::from(self.owners[oi].context),slot,event:u32::from(first_event)+1,stamp,
                    descriptor:base+0x9b40,manager,queue:self.queues[qi].storage.lanes[1].queue,
                    driver_stamps:[base+0x18000,base+0x18040],
                },
                parameter_error: None,
                completion,
                started: kernel::time::Instant::now(),
            },
        )?;
        self.queues[qi].render_count = next_local;
        self.owners[oi].render.as_mut().ok_or(EIO)?.sequence += 1;
        self.render_ordinal += 1;
        Ok(Some(receipt))
    }
}
impl Session {
    pub(crate) fn stage_g16c_render(
        &mut self,
        key: Key,
        client: &mut Option<compute::Client>,
        p: &command::Parameters,
        priority: u32,
        kernel_start: u64,
        kernel_end: u64,
        private:&mut Option<OwnerPrivateG16C>,
        transport:&mut Option<scheduled_queues::QueueBackingG16C>,
        work_backing:&mut Option<WorkBackingG16C>,
        prepared:Option<KBox<super::super::super::g17p_render_runtime::PreparedAppend>>,
        rebind:Option<&mut super::super::super::g17p_boot::PreparedPoolRebind>,
    ) -> Result<Option<Arc<RenderReceipt>>> {
        if self.phase != Phase::Running
            || self
                .peers
                .iter()
                .any(|p| p.data.crashed.load(Ordering::Acquire))
        {
            return Err(EIO);
        }
        let mut state = self.g16c_compute.take().ok_or(EIO)?;
        let result = state.stage_render(self, key, client, p, priority, kernel_start, kernel_end,private,transport,work_backing,prepared,rebind);
        self.g16c_compute = Some(state);
        self.finish_g16c_admission(result)
    }
}
