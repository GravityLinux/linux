// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Owned RAM/control/report adapter for the native synchronous release.
//! The Session supplies its primary RTKit transport and bounded event wait.
//! No TTBR switch, hardware discovery, or resource lifetime is hidden here.

use super::{
    g17p_abi::Channel,
    g17p_dependency as d, g17p_dependency_release as release,
    g17p_growth_runtime::{Action, Service},
    g17p_memory::{self, Memory},
    g17p_queue as q,
    g17p_user_vm::UserVm,
    g17p_vm::Vm,
};
use kernel::prelude::*;

pub(crate) trait Notifications {
    /// Send through the already-owned primary RTKit endpoint 0x21. The
    /// implementation must reject a crashed/unavailable transport.
    fn send(&mut self, message: u64) -> Result;
    /// Bounded event step while awaiting control consumption. Session owns
    /// timeout policy and checks both firmware crash flags on every step.
    fn wait_step(&mut self) -> Result;
}

pub(crate) struct Host<'a, N> {
    memory: &'a mut Memory,
    vm: &'a Vm,
    root: &'a mut UserVm,
    service: &'a mut Service,
    control: Channel,
    ttbs: u64,
    notifications: N,
    deferred: Option<(u64, u32)>,
    limit_seen: bool,
}
impl<'a, N: Notifications> Host<'a, N> {
    /// Called after all preparation and the explicit Session root switch. The
    /// mixed service verifies that root again before any growth publication.
    /// Reserve admission for all seven records before the first release kick;
    /// this first native source lifetime uses finite control-ring backing.
    pub(crate) fn new(
        memory: &'a mut Memory,
        vm: &'a Vm,
        root: &'a mut UserVm,
        service: &'a mut Service,
        control: Channel,
        ttbs: u64,
        notifications: N,
    ) -> Result<Self> {
        service.require_dependency(memory, ttbs, root)?;
        let host = Self {
            memory,
            vm,
            root,
            service,
            control,
            ttbs,
            notifications,
            deferred: None,
            limit_seen: false,
        };
        let counters = host.counters()?;
        if counters.available() < 7 || counters.0[2] > 248 {
            return Err(EBUSY);
        }
        // Bounds/ownership validation, not report consumption or GPU work.
        for slot in counters.0[2] as usize..counters.0[2] as usize + 7 {
            for offset in (0..0x40).step_by(8) {
                let va = control
                    .ring
                    .checked_add((slot * 0x40 + offset) as u64)
                    .ok_or(EINVAL)?;
                host.memory.word64(host.vm.physical(host.memory, 2, va)?)?;
            }
        }
        for va in control.states {
            if va == 0 || va & 3 != 0 {
                return Err(EINVAL);
            }
            host.memory
                .word64(host.vm.physical(host.memory, 2, va)? & !7)?;
        }
        Ok(host)
    }
    fn counters(&self) -> Result<q::Counters> {
        let mut values = [0; 3];
        for (value, va) in values.iter_mut().zip(self.control.states) {
            *value = self
                .memory
                .read_firmware32(self.vm.physical(self.memory, 2, va)?)?;
        }
        q::Counters::new(values).map_err(|_| EIO)
    }
    pub(crate) fn pump_reports(&mut self) -> Result {
        for _ in 0..32 {
            match self
                .service
                .step_dependency(self.memory, self.vm, self.root, self.ttbs)?
            {
                Action::Idle => break,
                Action::Consumed => (),
                Action::Limit => {
                    self.limit_seen = true;
                }
                Action::Reply { .. } => {
                    self.notifications.send(release::CONTROL_DOORBELL)?;
                }
            }
        }
        Ok(())
    }
    pub(crate) fn limit_seen(&self) -> bool {
        self.limit_seen
    }
}
impl<N: Notifications> d::ReleaseWriter for Host<'_, N> {
    type Error = Error;
    fn write32(&mut self, address: u64, value: u32) -> Result {
        if let Some((producer, target)) = self.deferred {
            if address == producer {
                if value != target {
                    return Err(EIO);
                }
                self.vm
                    .write(self.memory, 2, address, &value.to_le_bytes())?;
                self.deferred = None;
                return Ok(());
            }
        }
        self.vm.write(self.memory, 2, address, &value.to_le_bytes())
    }
    fn read32(&mut self, address: u64) -> Result<u32> {
        self.memory
            .read_firmware32(self.vm.physical(self.memory, 2, address)?)
    }
}
impl<N: Notifications> release::Target for Host<'_, N> {
    fn stage_control(&mut self, body: &[u8; 0x40], deferred: bool) -> Result<release::Control> {
        if self.deferred.is_some() {
            return Err(EBUSY);
        }
        let counters = self.counters()?;
        let slot = counters.slot().map_err(|_| EBUSY)?;
        if slot == 255 {
            return Err(EBUSY);
        }
        let target = slot + 1;
        self.vm
            .write(self.memory, 2, self.control.ring + slot as u64 * 0x40, body)?;
        if deferred {
            self.deferred = Some((self.control.states[2], target as u32));
        } else {
            self.vm.write(
                self.memory,
                2,
                self.control.states[2],
                &(target as u32).to_le_bytes(),
            )?;
        }
        g17p_memory::sync();
        Ok(release::Control {
            producer: self.control.states[2],
            target,
            consumers_before: [counters.0[0], counters.0[1]],
        })
    }
    fn expect_receipt(&mut self, sequence: u32) -> Result {
        self.service.expect_dependency_receipt(sequence)
    }
    fn write_context(&mut self, address: u64, body: &[u8]) -> Result {
        self.vm.write(self.memory, 2, address, body)
    }
    fn barrier(&mut self) {
        g17p_memory::sync();
    }
    fn notify(&mut self, message: u64) -> Result {
        self.notifications.send(message)
    }
    fn await_control(&mut self, control: release::Control) -> Result {
        if self.deferred.is_some() || control.producer != self.control.states[2] {
            return Err(EIO);
        }
        loop {
            // Also validate firmware health when consumers are already done.
            self.notifications.wait_step()?;
            self.pump_reports()?;
            let counters = self.counters()?;
            if (0..2)
                .all(|i| q::reached(control.consumers_before[i], counters.0[i], control.target))
            {
                return Ok(());
            }
        }
    }
}
