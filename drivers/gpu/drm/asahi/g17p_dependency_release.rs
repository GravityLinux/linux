// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Native first-wave release from _submit_dependency_wave and the two source
//! target-side release primitives. Every object is owned and resident first.
//! A control-consumption observation is never a GPU completion observation.

use super::{g17p_dependency as d, g17p_queue as q};

pub(crate) const CONTROL_DOORBELL: u64 = 0x0084000000000011;
pub(crate) const RENDER_DOORBELL: u64 = 0x0083000000000008;

#[derive(Clone, Copy)]
pub(crate) struct Control {
    pub(crate) producer: u64,
    pub(crate) target: u8,
    pub(crate) consumers_before: [u8; 2],
}
pub(crate) trait Target: d::ReleaseWriter {
    /// Reserve a credited slot, write/clean its body, then expose its producer
    /// unless deferred. Both consumers constrain reuse, including across wrap.
    fn stage_control(
        &mut self,
        body: &[u8; 0x40],
        deferred: bool,
    ) -> core::result::Result<Control, Self::Error>;
    /// Bind the exact expected primary receipt before the first work kick.
    fn expect_receipt(&mut self, sequence: u32) -> core::result::Result<(), Self::Error>;
    fn write_context(&mut self, address: u64, body: &[u8])
        -> core::result::Result<(), Self::Error>;
    /// Cache cleaning occurs in writes. This orders all earlier stores before
    /// a producer/mailbox operation. There is no GPU wait inside either window.
    fn barrier(&mut self);
    fn notify(&mut self, message: u64) -> core::result::Result<(), Self::Error>;
    /// Pump owned reports/events while awaiting only this control's consumers.
    fn await_control(&mut self, control: Control) -> core::result::Result<(), Self::Error>;
}

pub(crate) struct Boundary<'a> {
    pub(crate) opening_outer: (u64, u32),
    pub(crate) render_outer: [(u64, u32); 2],
    pub(crate) closing_outer: (u64, u32),
    pub(crate) closing_inner: (u64, u32),
    pub(crate) closing_context: (u64, &'a [u8]),
}
impl Boundary<'_> {
    pub(crate) fn valid(&self) -> bool {
        let producers = [
            self.opening_outer.0,
            self.render_outer[0].0,
            self.render_outer[1].0,
            self.closing_inner.0,
        ];
        self.opening_outer.1 == 1
            && self.render_outer.iter().all(|r| r.1 == 1)
            && self.closing_outer == (self.opening_outer.0, 2)
            && self.closing_inner == (d::LAYOUTS[3].pointers + q::POINTER_WRITE, 3)
            && self.closing_context.0 == d::LAYOUTS[3].context_high + 0x200
            && self.closing_context.1.len() == 0x200
            && producers
                .iter()
                .enumerate()
                .all(|(i, &va)| va != 0 && va & 3 == 0 && !producers[..i].contains(&va))
    }
}
#[derive(Debug)]
pub(crate) enum Error<E> {
    InvalidBoundary,
    InvalidControl,
    ClassState(u32),
    Access(E),
}

/// CL/tick0/class/render; observe class consumption; tick1/closing CL;
/// tick2/owners2,1,0. Retirement is a separate, closing-first operation.
/// On any failure the caller must retain all GPU-reachable owners and fail the
/// Session. A published prefix cannot be retried or rolled back as rejection.
pub(crate) fn release_opening<T: Target>(
    target: &mut T,
    b: &Boundary<'_>,
) -> core::result::Result<Control, Error<T::Error>> {
    if !b.valid() {
        return Err(Error::InvalidBoundary);
    }
    target
        .stage_control(&d::tick(0).unwrap(), false)
        .map_err(Error::Access)?;
    let class = target
        .stage_control(&d::render_registration(1), true)
        .map_err(Error::Access)?;
    if class.producer == 0 || class.producer & 3 != 0 {
        return Err(Error::InvalidControl);
    }
    target.expect_receipt(1).map_err(Error::Access)?;

    target
        .write32(b.opening_outer.0, b.opening_outer.1)
        .map_err(Error::Access)?;
    target.barrier();
    target.notify(q::COMPUTE_DOORBELL).map_err(Error::Access)?;
    target.notify(CONTROL_DOORBELL).map_err(Error::Access)?;
    target
        .write32(class.producer, class.target as u32)
        .map_err(Error::Access)?;
    target.barrier();
    target.notify(CONTROL_DOORBELL).map_err(Error::Access)?;
    d::render_transition(target).map_err(Error::Access)?;
    for &(address, value) in &b.render_outer {
        target.write32(address, value).map_err(Error::Access)?;
    }
    target.barrier();
    target.notify(RENDER_DOORBELL).map_err(Error::Access)?;

    Ok(class)
}

pub(crate) fn release_closing<T: Target>(
    target: &mut T,
    b: &Boundary<'_>,
) -> core::result::Result<(), Error<T::Error>> {
    let state = target.read32(d::RENDER_INNER).map_err(Error::Access)?;
    if state != 2 {
        return Err(Error::ClassState(state));
    }
    target
        .stage_control(&d::tick(1).unwrap(), false)
        .map_err(Error::Access)?;
    target
        .write_context(b.closing_context.0, b.closing_context.1)
        .map_err(Error::Access)?;
    target
        .write32(b.closing_inner.0, b.closing_inner.1)
        .map_err(Error::Access)?;
    target.barrier();
    target.notify(CONTROL_DOORBELL).map_err(Error::Access)?;
    target.barrier();
    target
        .write32(b.closing_outer.0, b.closing_outer.1)
        .map_err(Error::Access)?;
    target.barrier();
    target.notify(q::COMPUTE_DOORBELL).map_err(Error::Access)?;

    target
        .stage_control(&d::tick(2).unwrap(), false)
        .map_err(Error::Access)?;
    target.notify(CONTROL_DOORBELL).map_err(Error::Access)?;
    for engine in [2, 1, 0] {
        target
            .stage_control(&d::engine_owner(engine).unwrap(), false)
            .map_err(Error::Access)?;
        target.notify(CONTROL_DOORBELL).map_err(Error::Access)?;
    }
    Ok(())
}
