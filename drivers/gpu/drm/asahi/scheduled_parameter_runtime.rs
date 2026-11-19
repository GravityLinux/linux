// SPDX-License-Identifier: GPL-2.0-only

//! Owned active Max PB growth. The receive dispatcher validates ownership,
//! ACKs once, then returns here; control waits never recursively service RX.

use super::*;
use super::super::super::g17p_render::growth::GrowthRequestG16C;

pub(super) struct AcceptedGrowth {
    request: GrowthRequestG16C,
    owner: usize,
    generation: u32,
    first: bool,
}
impl State {
    pub(super) fn growth_roots(&self,memory:&Memory,ttbs:u64,owner:usize)->Result {
        let o=self.owners.get(owner).ok_or(EIO)?;
        let root=&o.client.as_ref().ok_or(EIO)?.root;
        if !o.published {return Err(EIO);}
        let at=ttbs+u64::from(o.context)*16;
        memory.invalidate(at,16)?;
        if [memory.read64(at)?,memory.read64(at+8)?]
            !=[root.root(),o.high_root].map(|p|p|u64::from(o.context)<<48|1) {return Err(EIO);}
        Ok(())
    }

    pub(super) fn accept_growth(&mut self,memory:&Memory,vm:&Vm,ttbs:u64,
        pending:&mut [RenderTicket],raw:&[u8;0x48])->Result<AcceptedGrowth> {
        let request=GrowthRequestG16C::parse(raw).ok_or(EIO)?;
        // The growth protocol names a retained context/pool. Never choose
        // another flight merely because it is first in the retirement wave.
        let index=RenderWave::<RenderTicket>::find_owner(pending,|p|
            p.render_work.context==request.context() && p.render_work.slot==request.slot())?;
        let p=&pending[index];
        if p.parameter_error.is_some() || p.growth_tokens.contains(&request.token()) {
            return Err(EIO);
        }
        let owner=p.render_owner;
        let o=self.owners.get(owner).ok_or(EIO)?;
        let r=o.render.as_ref().ok_or(EIO)?;
        if request.context()!=u32::from(o.context) || request.slot()!=r.parameter.slot {
            return Err(EIO);
        }
        self.growth_roots(memory,ttbs,owner)?;
        let generation=r.growth_generation(memory,vm).map_err(|error|{
            kernel::pr_err!("G16C: PB request metadata rejected context={} slot={} token={} blocks={}\n",request.context(),request.slot(),request.token(),r.parameter.blocks);error
        })?;
        if r.parameter.blocks==super::super::super::g17p_render::resources::MAX_BLOCKS {
            return Err(ENOMEM);
        }
        let first=p.growth_tokens.is_empty();
        // Reserve/retain before ACK. Tokens cannot be replayed in this flight.
        pending[index].growth_tokens.push(request.token(),GFP_KERNEL)?;
        Ok(AcceptedGrowth {request,owner,generation,first})
    }

    pub(super) fn service_growth(&mut self,session:&mut Session,accepted:AcceptedGrowth)->Result {
        if accepted.first {
            for engine in 0..3 {for phase in 1..=2 {
                session.g16c_work_control(MaintenanceG16C::new(engine,phase).ok_or(EIO)?.bytes())?;
            }}
        }
        self.growth_roots(session.memory.as_ref().ok_or(EIO)?,session.ttbs,accepted.owner)?;
        let o=&mut self.owners[accepted.owner];
        let r=o.render.as_mut().ok_or(EIO)?;
        let (old,count)=r.grow(session.memory.as_mut().ok_or(EIO)?,session.vm.as_ref().ok_or(EIO)?,
            &mut o.client.as_mut().ok_or(EIO)?.root,o.context,accepted.generation)?;
        let reply=accepted.request.reply(accepted.generation).ok_or(EIO)?;
        session.g16c_work_control(reply.bytes())?;
        // Control retirement is independent of the firmware manager stores.
        // Require the exact applied counts, bounded; defer any next request.
        session.g16c_wait_growth_condition(|memory,vm|r.growth_applied(memory,vm,old))?;
        // Growth is progress, not a submission deadline extension. The
        // original pending.started bounds the entire render including growth.
        kernel::dev_info!(session.peers[0].data.dev.as_ref(),
            "G16C: owned PB growth context={} slot={} token={} generation={} blocks={}->{} preserved firmware DATA, reply retired\n",
            o.context,r.parameter.slot,accepted.request.token(),accepted.generation,old,count);
        Ok(())
    }
}
impl Session {
    pub(super) fn g16c_work_control(&mut self,record:&[u8;64])->Result {
        let target=boot::publish_control(self.memory.as_mut().ok_or(EIO)?,
            self.vm.as_ref().ok_or(EIO)?,record)?;
        self.g16c_send(0x21,boot::CONTROL_MESSAGE)?;
        self.g16c_send(0x21,0x0083000000000010)?;
        self.g16c_wait_work_control(target)
    }
}
