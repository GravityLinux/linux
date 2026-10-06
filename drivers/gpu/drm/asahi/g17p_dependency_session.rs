// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! First synchronous native owner, retained inside the dual-ASC Session.
//! Admission currently requires the source's completed compute primer. The
//! backend-owned cold primer and UAPI dispatch are wired separately; a caller
//! command must never be executed twice to synthesize that prerequisite.

use super::super::{
    g17p_dependency as d,
    g17p_dependency_control::{Host, Notifications},
    g17p_dependency_release::{self as release, Control, Target},
    g17p_dependency_retire::{Owner, Reports, Retirement},
    g17p_dependency_runtime::{self as runtime, Prepared},
    g17p_growth_runtime::{Action, Service},
    g17p_memory, g17p_queue as q,
    g17p_render::Parameters,
};
use super::{compute, render, Image, Peer, Phase, Report, Session};
use core::sync::atomic::Ordering;
use kernel::prelude::*;

pub(super) struct State {
    prepared: Prepared,
    pub(super) service: Service,
    retirement: Retirement,
    startup: [Report; 2],
    last_control: Option<Control>,
    limited: bool,
    released: bool,
    release_control: Option<Control>,
    render_notified: bool,
    retained_before: usize,
    started: kernel::time::Instant<kernel::time::Monotonic>,
}
impl State {
    pub(super) fn set_control(&mut self, control: Control) -> Result {
        if !self.complete() {
            return Err(EBUSY);
        }
        self.last_control = Some(control);
        Ok(())
    }
    pub(super) fn complete(&self) -> bool {
        self.retirement.complete()
    }
}

struct Transport<'a> {
    peers: &'a mut [Peer],
    remaining: u32,
}
impl Transport<'_> {
    fn healthy(&self) -> Result {
        if self.peers.len() != 2
            || self
                .peers
                .iter()
                .any(|p| !p.started || p.data.crashed.load(Ordering::Acquire) || p.rtkit.is_none())
        {
            return Err(EIO);
        }
        Ok(())
    }
}
impl Notifications for Transport<'_> {
    fn send(&mut self, message: u64) -> Result {
        self.healthy()?;
        self.peers[0]
            .rtkit
            .as_mut()
            .ok_or(EIO)?
            .as_mut()
            .send_message(0x21, message)
    }
    fn wait_step(&mut self) -> Result {
        self.healthy()?;
        if self.remaining == 0 {
            return Err(ETIMEDOUT);
        }
        self.remaining -= 1;
        kernel::time::delay::fsleep(kernel::time::Delta::from_millis(10));
        self.healthy()
    }
}

fn startup_receipt(report: &Report, before: &Report, index: usize) -> bool {
    index == 0
        && report.host == 0
        && before.host == 0
        && before.firmware == 1
        && before.records.len() == 1
        && report.records[index] == before.records[0]
        && report.records[index][..12] == [13, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]
}
impl Session {
    /// Refresh both views of the same quiescent logical VM. Retain old GEM
    /// references through both root publications; caller-render aliases below
    /// BASE must be reflected in the shared command-buffer execution root.
    pub(super) fn refresh_native_render(
        &mut self,
        replacement: Option<compute::Client>,
        compute_replacement: Option<compute::Client>,
        p: &Parameters,
    ) -> Result {
        if !self.native.as_ref().ok_or(EINVAL)?.complete() {
            return Err(EBUSY);
        }
        let render = self.render.as_mut().ok_or(EINVAL)?;
        let compute = self.compute.as_mut().ok_or(EINVAL)?;
        let retained_before = self.retained_buffers.len();
        for bo in
            core::iter::Iterator::chain(render.client.buffers.iter(), compute.client.buffers.iter())
        {
            self.retained_buffers.push(bo.clone(), GFP_KERNEL)?;
        }
        let mut addresses = KVec::new();
        for &(base, size, _, _) in &render.client.bindings {
            let low = if base < 0x1000000000 {
                base + 0x1000000000
            } else {
                base
            };
            for offset in (0..size).step_by(0x4000) {
                addresses.push(low + offset, GFP_KERNEL)?;
            }
        }
        if let Some(client) = replacement {
            render::validate_client(&client, p)?;
            for &(base, size, _, _) in &client.bindings {
                let low = if base < 0x1000000000 {
                    base + 0x1000000000
                } else {
                    base
                };
                for offset in (0..size).step_by(0x4000) {
                    if !addresses.contains(&(low + offset)) {
                        addresses.push(low + offset, GFP_KERNEL)?;
                    }
                }
            }
            render.client.rebind(client, true)?;
        }
        if let Some(client) = compute_replacement {
            compute.client.rebind(client, false)?;
        }
        let mut changes = KVec::new();
        for address in addresses {
            let old = compute.client.root.pte(address)?;
            let new = render.client.root.pte(address)?;
            if old != new {
                changes.push((address, old, new), GFP_KERNEL)?;
            }
        }
        compute.client.root.rebind(&changes, &[1, 2, 3])?;
        render.client.cache(false)?;
        // On any earlier error the Session keeps the previous caller backing
        // pinned, including aliases left by a partially refreshed root.
        self.retained_buffers.truncate(retained_before);
        Ok(())
    }
    pub(crate) fn native_render(&self) -> bool {
        self.native.is_some()
    }

    pub(super) fn complete_native_render_control(&mut self, image: &Image) -> Result<bool> {
        let native = self.native.as_mut().ok_or(EINVAL)?;
        if !native.complete() {
            return Err(EBUSY);
        }
        let mut host = Host::attach(
            self.memory.as_mut().ok_or(EINVAL)?,
            self.vm.as_ref().ok_or(EINVAL)?,
            &mut self.compute.as_mut().ok_or(EINVAL)?.client.root,
            &mut native.service,
            image.graph.channels[0][12],
            self.ttbs,
            Transport {
                peers: &mut self.peers,
                remaining: 500,
            },
        )?;
        host.notify(release::CONTROL_DOORBELL)?;
        host.poll_control(native.last_control.ok_or(EINVAL)?)
    }

    /// A single already-validated C/R/C owner. Neither this entrypoint nor the
    /// Session admission guards changes default ordinary UAPI dispatch.
    pub(crate) fn submit_native_dependency(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        replacement: Option<compute::Client>,
        render_client: compute::Client,
        inputs: [&compute::Parameters; 2],
        parameters: &Parameters,
    ) -> Result {
        if self.phase == Phase::Prepared {
            if inputs[0].preempt != inputs[1].preempt {
                return Err(EINVAL);
            }
            self.bootstrap_compute(dev, image, render_client.owner, inputs[0].preempt)?;
        }
        if self.phase != Phase::Running
            || !self.bootstrapped
            || self.native.is_some()
            || self.render.is_some()
            || self.dormant_render.is_none()
            || self
                .compute
                .as_ref()
                .is_none_or(|c| c.ordinal != 0 || c.after_render)
        {
            return Err(EBUSY);
        }
        render::validate_client(&render_client, parameters)?;
        let retained_before = self.retained_buffers.len();
        let mut operation = "startup reports";
        let result = (|| {
            let startup = self.report_snapshot(image)?;
            // Existing pre-work records must be known before the mixed reader
            // adopts its initial cursor; never silently skip unknown evidence.
            for report in &startup {
                for (index, body) in report.records.iter().enumerate() {
                    if body[..4] != 1u32.to_le_bytes() && !startup_receipt(report, report, index) {
                        return Err(EIO);
                    }
                }
            }
            compute::idle(
                self.memory.as_ref().ok_or(EINVAL)?,
                self.vm.as_ref().ok_or(EINVAL)?,
                self.compute.as_ref().ok_or(EINVAL)?,
            )?;
            if let Some(client) = replacement {
                operation = "compute caller binding";
                self.compute
                    .as_mut()
                    .ok_or(EINVAL)?
                    .client
                    .rebind(client, false)?;
            }
            // Install the retained owner before adoption can expose references.
            self.render = self.dormant_render.take();
            operation = "render caller adoption";
            self.render.as_mut().ok_or(EINVAL)?.adopt(
                self.memory.as_mut().ok_or(EINVAL)?,
                self.vm.as_ref().ok_or(EINVAL)?,
                render_client,
                parameters,
            )?;
            for bo in core::iter::Iterator::chain(
                self.render.as_ref().ok_or(EINVAL)?.client.buffers.iter(),
                self.compute.as_ref().ok_or(EINVAL)?.client.buffers.iter(),
            ) {
                self.retained_buffers.push(bo.clone(), GFP_KERNEL)?;
            }
            operation = "native graph preparation";
            let prepared = runtime::prepare(
                self.memory.as_mut().ok_or(EINVAL)?,
                self.vm.as_mut().ok_or(EINVAL)?,
                image,
                self.compute.as_mut().ok_or(EINVAL)?,
                self.render.as_mut().ok_or(EINVAL)?,
                inputs,
                parameters,
                1,
            )?;
            let memory = self.memory.as_ref().ok_or(EINVAL)?;
            let vm = self.vm.as_ref().ok_or(EINVAL)?;
            for pair in prepared.compute_timestamps {
                self.timestamps.as_ref().ok_or(EINVAL)?.cache(pair, false)?;
            }
            let ts = prepared.render_timestamps;
            for pair in [[ts[0], ts[1]], [ts[2], ts[3]]] {
                self.timestamps.as_ref().ok_or(EINVAL)?.cache(pair, false)?;
            }
            operation = "native root/report admission";
            let retirement = prepared.retirement(memory, vm)?;
            prepared.activate(
                memory,
                self.ttbs,
                self.render.as_ref().ok_or(EINVAL)?.client.root.root(),
            )?;
            let mut service = Service::new_dependency(
                memory,
                vm,
                self.ttbs,
                &self.compute.as_ref().ok_or(EINVAL)?.client.root,
                image.graph.channels[0][12],
                image.graph.channels[0][13],
            )?;
            super::configure_growth_limits(&mut service)?;
            service.begin_compute(1)?;
            service.begin_compute(2)?;
            service.bind_work(d::RENDER_DESCRIPTORS[1])?;
            self.native = Some(State {
                prepared,
                service,
                retirement,
                startup,
                last_control: None,
                limited: false,
                released: false,
                release_control: None,
                render_notified: false,
                retained_before,
                started: kernel::time::Instant::now(),
            });
            self.inject_owned_render_fault(parameters)?;
            operation = "native release";
            self.release_native(image)?;
            self.pending = Some(super::PendingWork::Native);
            Ok(())
        })();
        if let Err(error) = &result {
            dev_err!(
                dev,
                "G17P: {} failed: {:?}; native resources retained\n",
                operation,
                error
            );
            self.phase = Phase::Failed;
            self.faults.fail();
            if let Some(native) = self.native.as_mut() {
                native.retirement.fail();
            }
            // compute/render/Memory remain Session-owned through shutdown.
        }
        result
    }

    fn release_native(&mut self, image: &Image) -> Result {
        let native = self.native.as_mut().ok_or(EINVAL)?;
        if native.released {
            return Err(EIO);
        }
        let transport = Transport {
            peers: &mut self.peers,
            remaining: 500,
        };
        let mut host = Host::new(
            self.memory.as_mut().ok_or(EINVAL)?,
            self.vm.as_ref().ok_or(EINVAL)?,
            &mut self.compute.as_mut().ok_or(EINVAL)?.client.root,
            &mut native.service,
            image.graph.channels[0][12],
            self.ttbs,
            transport,
        )?;
        let control = release::release_opening(&mut host, &native.prepared.boundary()?).map_err(
            |e| match e {
                release::Error::Access(e) => e,
                _ => EIO,
            },
        )?;
        native.last_control = host.last_control();
        native.limited = host.limit_seen();
        native.release_control = Some(control);
        Ok(())
    }

    fn service_native_reports(&mut self) -> Result {
        let native = self.native.as_mut().ok_or(EINVAL)?;
        let vm = self.vm.as_ref().ok_or(EINVAL)?;
        let memory = self.memory.as_mut().ok_or(EINVAL)?;
        let mut transport = Transport {
            peers: &mut self.peers,
            remaining: 1,
        };
        transport.healthy()?;
        for _ in 0..32 {
            match native.service.step_dependency(
                memory,
                vm,
                &mut self.compute.as_mut().ok_or(EINVAL)?.client.root,
                self.ttbs,
            )? {
                Action::Idle => break,
                Action::Consumed => (),
                Action::Limit => {
                    native.limited = true;
                }
                Action::LimitReply { .. } => {
                    native.limited = true;
                    transport.send(release::CONTROL_DOORBELL)?;
                }
                Action::Reply { .. } => {
                    transport.send(release::CONTROL_DOORBELL)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn poll_native(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
    ) -> Result<bool> {
        if self
            .native
            .as_ref()
            .ok_or(EINVAL)?
            .started
            .elapsed()
            .as_millis()
            >= 5000
        {
            return Err(ETIMEDOUT);
        }
        if !self.native.as_ref().ok_or(EINVAL)?.released {
            self.service_native_reports()?;
            let native = self.native.as_mut().ok_or(EINVAL)?;
            let mut host = Host::attach(
                self.memory.as_mut().ok_or(EINVAL)?,
                self.vm.as_ref().ok_or(EINVAL)?,
                &mut self.compute.as_mut().ok_or(EINVAL)?.client.root,
                &mut native.service,
                image.graph.channels[0][12],
                self.ttbs,
                Transport {
                    peers: &mut self.peers,
                    remaining: 0,
                },
            )?;
            if !host.poll_control(native.release_control.ok_or(EINVAL)?)? {
                return Ok(false);
            }
            release::release_closing(&mut host, &native.prepared.boundary()?).map_err(
                |e| match e {
                    release::Error::Access(e) => e,
                    _ => EIO,
                },
            )?;
            native.last_control = host.last_control();
            native.limited |= host.limit_seen();
            native.released = true;
        }
        self.service_native_reports()?;
        let reports = self.report_snapshot(image)?;
        let native = self.native.as_mut().ok_or(EINVAL)?;
        if native.service.cursor() != reports[0].firmware {
            // A new primary record arrived after service; validate it next
            // iteration rather than acknowledging an unowned report.
            return Ok(false);
        }
        for (index, body) in reports[1].records.iter().enumerate() {
            if body[..4] != 1u32.to_le_bytes()
                && !startup_receipt(&reports[1], &native.startup[1], index)
            {
                return Err(EIO);
            }
        }
        let observed = native.prepared.observe(
            self.memory.as_ref().ok_or(EINVAL)?,
            self.vm.as_ref().ok_or(EINVAL)?,
        )?;
        let owner = native
            .retirement
            .poll(
                &observed,
                Reports {
                    valid: true,
                    render_complete: native.service.terminals() > 0 || native.limited,
                },
            )
            .map_err(|_| EIO)?;
        if let Some(owner) = owner {
            if owner == Owner::Render {
                // The source pumps control-done before its final render
                // snapshot. Await only the owned final control target.
                let native = self.native.as_mut().ok_or(EINVAL)?;
                let control = native.last_control.ok_or(EIO)?;
                let transport = Transport {
                    peers: &mut self.peers,
                    remaining: 500,
                };
                let mut host = Host::attach(
                    self.memory.as_mut().ok_or(EINVAL)?,
                    self.vm.as_ref().ok_or(EINVAL)?,
                    &mut self.compute.as_mut().ok_or(EINVAL)?.client.root,
                    &mut native.service,
                    image.graph.channels[0][12],
                    self.ttbs,
                    transport,
                )?;
                // Native render is global submission ordinal one, so
                // _finish_render uses control_done_count (one), rather
                // than the ordinal-zero first_control_done_count (two).
                if !native.render_notified {
                    host.notify(release::CONTROL_DOORBELL)?;
                    native.render_notified = true;
                }
                if !host.poll_control(control)? {
                    return Ok(false);
                }

                native.limited |= host.limit_seen();
            }
            // Recheck reports/status/crash evidence after that control
            // pump and before any copyback or scheduler reset.
            self.service_native_reports()?;
            let reports = self.report_snapshot(image)?;
            let native = self.native.as_ref().ok_or(EINVAL)?;
            if native.service.cursor() != reports[0].firmware {
                return Ok(false);
            }
            for (index, body) in reports[1].records.iter().enumerate() {
                if body[..4] != 1u32.to_le_bytes()
                    && !startup_receipt(&reports[1], &native.startup[1], index)
                {
                    return Err(EIO);
                }
            }
            let observed = native.prepared.observe(
                self.memory.as_ref().ok_or(EINVAL)?,
                self.vm.as_ref().ok_or(EINVAL)?,
            )?;
            let native = self.native.as_mut().ok_or(EINVAL)?;
            if native
                .retirement
                .poll(
                    &observed,
                    Reports {
                        valid: true,
                        render_complete: native.service.terminals() > 0 || native.limited,
                    },
                )
                .map_err(|_| EIO)?
                != Some(owner)
            {
                return Err(EIO);
            }
            if owner == Owner::Render && native.limited {
                if native.service.limit_report().is_none() {
                    return Err(EIO);
                }
                self.submission_error = Some(ENOMEM);
            }
            let timestamps = match owner {
                Owner::Closing => native.prepared.compute_timestamps[1],
                Owner::Opening => native.prepared.compute_timestamps[0],
                Owner::Render => [0; 2],
            };
            match owner {
                Owner::Closing => self.compute.as_ref().ok_or(EINVAL)?.client.cache(true)?,
                Owner::Render if !native.limited => {
                    let list = d::LAYOUTS[1].job_list;
                    self.vm.as_ref().ok_or(EINVAL)?.write(
                        self.memory.as_mut().ok_or(EINVAL)?,
                        2,
                        list,
                        &q::job_list(list),
                    )?;
                    g17p_memory::sync();
                    let memory = self.memory.as_ref().ok_or(EINVAL)?;
                    let tail = self
                        .vm
                        .as_ref()
                        .ok_or(EINVAL)?
                        .physical(memory, 2, list + 8)?;
                    memory.invalidate(tail, 8)?;
                    if memory.read64(tail)? != list {
                        return Err(EIO);
                    }
                    let ts = native.prepared.render_timestamps;
                    for pair in [[ts[0], ts[1]], [ts[2], ts[3]]] {
                        self.timestamps.as_ref().ok_or(EINVAL)?.cache(pair, true)?;
                    }
                }
                Owner::Opening | Owner::Render => (),
            }
            self.timestamps
                .as_ref()
                .ok_or(EINVAL)?
                .cache(timestamps, true)?;
            self.acknowledge_reports(image, &reports)?;
            let native = self.native.as_mut().ok_or(EINVAL)?;
            if owner != Owner::Render {
                native
                    .service
                    .finish_compute(if owner == Owner::Closing { 2 } else { 1 })?;
            } else {
                native.service.retire_work()?;
            }
            native.retirement.retire(owner).map_err(|_| EIO)?;
            dev_info!(
                dev,
                "G17P: native {:?} completed from own queues/status/reports\n",
                owner
            );
            if native.retirement.complete() {
                // _submit_dependency_wave pulls render writable bindings
                // after all three finishers, including opening CL.
                if !native.limited {
                    self.render.as_ref().ok_or(EINVAL)?.client.cache(true)?;
                }
                self.render.as_mut().ok_or(EINVAL)?.retain_native(
                    [
                        native.prepared.publications[1],
                        native.prepared.publications[2],
                    ],
                    [native.prepared.channels[1], native.prepared.channels[2]],
                    native.prepared.render_timestamps,
                )?;
                // Python retains runtime["queue"] (the direct bootstrap
                // transport) through the two overridden native queues.
                // Its logical ordinal advances by two; never adopt the
                // native closing queue as the ordinary retained transport.
                let work = self.compute.as_mut().ok_or(EINVAL)?;
                work.ordinal = 2;
                work.status = d::Compute::Closing.status();
                work.timestamps = native.prepared.compute_timestamps[1];
                let retained_before = native.retained_before;
                if self.submission_error.is_none() {
                    self.retained_buffers.truncate(retained_before);
                }
                self.completed_owned_render_fault();
                return Ok(true);
            }
        }
        Ok(false)
    }
}
