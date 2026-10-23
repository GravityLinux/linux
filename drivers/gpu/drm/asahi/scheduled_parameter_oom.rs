// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Owned Max OOM7 -> error9. Reply retirement does not complete the render:
//! normal stamps/queues/channels must independently retire before ENOMEM.

use super::*;
use parameter_error::{ParameterErrorG16C,RenderOwnerG16C};
use super::super::super::g17p_render::wire::FragmentDescriptorG16C;

// Keep pure identity validation tied to the macro-generated actual layout.
const _: () = {
    assert!(core::mem::size_of::<FragmentDescriptorG16C>()==0x2280);
    assert!(core::mem::offset_of!(FragmentDescriptorG16C,opcode)==0);
    assert!(core::mem::offset_of!(FragmentDescriptorG16C,context)==0xc);
    assert!(core::mem::offset_of!(FragmentDescriptorG16C,manager)==0x28);
    assert!(core::mem::offset_of!(FragmentDescriptorG16C,driver_stamp)==0x2140);
    assert!(core::mem::offset_of!(FragmentDescriptorG16C,stamp)==0x2150);
    assert!(core::mem::offset_of!(FragmentDescriptorG16C,event)==0x2154);
};
pub(super) struct AcceptedError {
    id:u64,
    request:ParameterErrorG16C,
    owner:RenderOwnerG16C,
    before_driver:[u32;2],
}
pub(super) struct RepliedError {
    pub(super) owner:RenderOwnerG16C,
    pub(super) before_driver:[u32;2],
}
impl State {
    pub(super) fn accept_parameter_error(&mut self,memory:&Memory,vm:&Vm,ttbs:u64,
        raw:&[u8;0x48])->Result<AcceptedError> {
        let request=ParameterErrorG16C::parse(raw).ok_or(EIO)?;
        if self.pending.len()!=1 {return Err(EBUSY);}
        let p=&self.pending[0];
        if !matches!(p.receipt,ReceiptG16C::Render(_)) || p.parameter_error.is_some()
            || unsafe {bindings::dma_fence_get_status(p.receipt.fence().raw())}!=0 {return Err(EIO);}
        let owner=p.render_work.ok_or(EIO)?;
        let oi=p.render_owner.ok_or(EIO)?;
        let o=self.owners.get(oi).ok_or(EIO)?;
        let r=o.render.as_ref().ok_or(EIO)?;
        if owner.context!=u32::from(o.context) || owner.slot!=r.parameter.slot
            || owner.manager!=r.manager || !r.admitted()
            || !self.queues.iter().any(|q|q.live && q.owner==oi && q.storage.lanes[1].queue==owner.queue) {
            return Err(EIO);
        }
        self.growth_roots(memory,ttbs,oi)?;
        let mut body=bytes(FragmentDescriptorG16C::SIZE)?;
        boot::read_words(memory,vm,owner.descriptor,&mut body)?;
        if !owner.matches(&request,&body) {return Err(EIO);}
        let mut before_driver=[0;2];
        for (value,address)in before_driver.iter_mut().zip(owner.driver_stamps) {
            *value=memory.read_firmware32(vm.physical(memory,2,address)?)?;
        }
        Ok(AcceptedError {id:p.id,request,owner,before_driver})
    }
    pub(super) fn service_parameter_error(&mut self,session:&mut Session,accepted:AcceptedError)->Result {
        let pending=self.pending.first_mut().ok_or(EIO)?;
        if pending.id!=accepted.id || pending.parameter_error.is_some() {return Err(EIO);}
        let reply=accepted.request.reply().ok_or(EIO)?;
        // The dispatcher has ACKed the validated RX slot before reaching here.
        // Any control uncertainty returns an error and retains all owners.
        session.g16c_work_control(reply.bytes())?;
        pending.parameter_error=Some(RepliedError {owner:accepted.owner,before_driver:accepted.before_driver});
        kernel::dev_info!(session.peers[0].data.dev.as_ref(),
            "G16C: owned OOM7 context={} slot={} event={} stamp={} queue={:#x} error9 retired; waiting independent work retirement\n",
            accepted.owner.context,accepted.owner.slot,accepted.owner.event,accepted.owner.stamp,accepted.owner.queue);
        Ok(())
    }
}
