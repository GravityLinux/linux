// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! Diagnostics: backpressure and hold reports, publication traces, hang
//! reports, Work reuse history, stale event tags and A4 wedge detection.

use super::*;

impl Bootstrap {
    /// A Work held on a stamp waits for a completion to advance it. With no
    /// Work in flight anywhere nothing can write that stamp, so the hold
    /// would never clear and the pump would keep the job forever with an
    /// idle GPU. If the same wait stays unreached with nothing in flight
    /// for a second (the pump re-polls every 256 ms), fail just that
    /// command and say why. A recorded hold that later resolves is counted
    /// and logged as a near miss.
    pub(super) fn hold_check(&self, kind: &str, dep: &Stamp, now: u32, done: bool) -> Result {
        let key = dep.address ^ (u64::from(dep.value) << 32).rotate_left(16);
        let recorded = self.diagnostics.hold_key.load(Ordering::Relaxed);
        if done {
            if recorded == key {
                self.diagnostics.hold_key.store(0, Ordering::Relaxed);
                let misses = self.diagnostics.hold_near_misses.fetch_add(1, Ordering::Relaxed) + 1;
                if misses <= 16 {
                    dev_info!(self.dev.as_ref(),
                              "G16: {} wait on stamp {:#x} want {:#x} resolved after being unreachable with nothing in flight (near miss #{})\n",
                              kind, dep.address, dep.value, misses);
                }
            }
            return Ok(());
        }
        if !self.flights.is_empty() {
            if recorded == key {
                self.diagnostics.hold_key.store(0, Ordering::Relaxed);
            }
            return Ok(());
        }
        let now_ns = monotonic_ns();
        if recorded != key {
            self.diagnostics.hold_key.store(key, Ordering::Relaxed);
            self.diagnostics.hold_since_ns.store(now_ns, Ordering::Relaxed);
            return Ok(());
        }
        if now_ns - self.diagnostics.hold_since_ns.load(Ordering::Relaxed) < 1_000_000_000 {
            return Ok(());
        }
        self.diagnostics.hold_key.store(0, Ordering::Relaxed);
        dev_err!(
            self.dev.as_ref(),
            "G16: {} wait on stamp {:#x} want {:#x} (now {:#x}, event {}) unreachable with nothing in flight for 1 s; failing the command\n",
            kind, dep.address, dep.value, now, dep.event
        );
        Err(EDEADLK)
    }

    /// Backpressure bits recorded since the previous call (timeline=1).
    pub(crate) fn take_pressure(&self) -> u64 {
        self.diagnostics.pressure_seen.swap(0, Ordering::Relaxed)
    }

    /// Report each exhausted resource once, without logging every retry or
    /// enabling firmware KTrace (which changes command execution timing).
    pub(super) fn report_pressure(&self, bit: u64, resource: &str) {
        self.diagnostics.pressure_seen.fetch_or(bit, Ordering::Relaxed);
        if let Some(count) = crate::g16_timeline::DEFERRED_BY.get(bit.trailing_zeros() as usize) {
            count.fetch_add(1, Ordering::Relaxed);
        }
        // Every resource here is released when work retires. Backpressure with
        // nothing in flight can therefore never clear: it means an accounting
        // leak, and submission stalls indefinitely with an idle GPU. Always
        // report that, and event/ASID exhaustion, once per boot; transient
        // ring and scene pressure under load stays behind fw_trace & 2.
        let persistent = bit & (1 | 8) != 0 || self.flights.is_empty();
        if (persistent || crate::module_parameters::fw_trace.value() & 2 != 0)
            && self.diagnostics.pressure_reported.fetch_or(bit, Ordering::Relaxed) & bit == 0
        {
            let asids = (1..64).filter(|&i| self.context_users[i] != 0).count();
            dev_info!(
                self.dev.as_ref(),
                "G16: backpressure {} outstanding={} queue_events={}/32 asids={}/63\n",
                resource,
                self.flights.len(),
                (self.queue_events.load(Ordering::Acquire) & 0xffff_ffff).count_ones(),
                asids
            );
        }
    }

    pub(super) fn trace_publication(&self, kind: &str, id: u64) -> Result {
        if crate::module_parameters::fw_trace.value() & 1 != 0 {
            let mut unretired = 0;
            for flight in &self.flights {
                if !self.stamps_done(flight).unwrap_or(false) {
                    unretired += 1;
                }
            }
            let flight = self.flights.iter().find(|f| f.id == id).ok_or(EIO)?;
            dev_info!(
                self.dev.as_ref(),
                "G16: publish {} {:#x} outstanding={} unfinished_stamps={} queues={:#x}/{:#x} events={}/{}\n",
                kind,
                id,
                self.flights.len(),
                unretired,
                flight.queues[0].0.saturating_sub(0x4000),
                flight.queues[1].0.saturating_sub(0x4000),
                flight.stamps[0].map_or(u32::MAX, |s| s.event),
                flight.stamps[1].map_or(u32::MAX, |s| s.event)
            );
        }
        Ok(())
    }

    /// Optional qualification trace. Readiness is observed only for diagnostics;
    /// it does not decide whether a dependent Work can enter the firmware ring.
    pub(super) fn trace_dependencies(
        &self,
        kind: &str,
        id: u64,
        context: u32,
        dependencies: &[Option<Stamp>; 2],
    ) -> Result {
        if crate::module_parameters::fw_trace.value() & 8 != 0 {
            let fw = self._firmware_space.as_ref().ok_or(EIO)?;
            let mut unfinished = 0;
            for dep in dependencies.iter().flatten() {
                unfinished += usize::from(!g16_stamp::reached(fw.read_u32(dep.address)?, dep.value));
            }
            let flight = self.flights.iter().find(|flight| flight.id == id).ok_or(EIO)?;
            let mut driver_values = [0; 2];
            for (index, stamp) in flight.driver_stamps.iter().enumerate() {
                if let Some(stamp) = stamp {
                    driver_values[index] = fw.read_u32(stamp.address)?;
                }
            }
            dev_info!(
                self.dev.as_ref(),
                "G16: staged {} {:#x} context={} dependencies={} unfinished={} stamps={:#x}/{:#x} driver_stamps={:#x}/{:#x}\n",
                kind,
                id,
                context,
                dependencies.iter().flatten().count(),
                unfinished,
                flight.stamps[0].map_or(0, |stamp| stamp.value),
                flight.stamps[1].map_or(0, |stamp| stamp.value),
                driver_values[0],
                driver_values[1]
            );
        }
        Ok(())
    }

    /// Preserve the event and unfinished queue identities before poisoning the
    /// device. Firmware timeout events name a stamp slot, which can be shared
    /// by several in-flight Works from the same userspace queue.
    /// Hang diagnostics: GPU fault registers, every pending flight's
    /// firmware and driver stamps against their targets, and the newest
    /// firmware receive-ring entries.
    pub(super) fn dump_hang_state(&self) -> Result {
        // Device-wide transports: whether firmware consumed each channel's
        // messages (read/done) up to what the driver published (tail).
        {
            let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
            if let Some(guard) = self._sgx.try_access() {
                for (index, route) in config.channels.iter().enumerate().take(12) {
                    let read = guard
                        .try_read32(0xd64000 + (route.state[0] - 0xffff_fc20_0002_8000) as usize)
                        .unwrap_or(u32::MAX);
                    let done = guard
                        .try_read32(0xd64000 + (route.state[1] - 0xffff_fc20_0002_8000) as usize)
                        .unwrap_or(u32::MAX);
                    let tail = guard
                        .try_read32(0xd60000 + (route.state[2] - 0xffff_fc20_0002_0000) as usize)
                        .unwrap_or(u32::MAX);
                    dev_err!(
                        self.dev.as_ref(),
                        "G16: hang channel {} (priority {} stage {}) read={} done={} tail={}\n",
                        index,
                        index / 3,
                        index % 3,
                        read,
                        done,
                        tail
                    );
                }
            }
        }
        if let Some(guard) = self._sgx.try_access() {
            let read64 = |offset: usize| -> u64 {
                let lo = guard.try_read32(offset).unwrap_or(0xdeadbeef);
                let hi = guard.try_read32(offset + 4).unwrap_or(0xdeadbeef);
                u64::from(lo) | (u64::from(hi) << 32)
            };
            let info = read64(0xd8c0);
            let addr = read64(0xd8c8);
            dev_err!(self.dev.as_ref(), "G16: fault info d8c0={:#018x} d8c8={:#018x} 17030={:#018x}\n",
                     info, addr, read64(0x17030));
            // Field layout as on G14X (regs.rs get_fault_info); not yet
            // validated on G16 beyond the VM slot matching the hung context.
            if info & 1 != 0 {
                let access = if info & (1 << 4) != 0 { "read" } else { "write" };
                dev_err!(self.dev.as_ref(),
                         "G16: fault addr={:#x} {} reason={} unit={:#04x} vm_slot={} level={} sideband={:#04x}\n",
                         addr << 6, access,
                         (info >> 1) & 7, (info >> 9) & 0xff, (info >> 17) & 0x3f,
                         (info >> 7) & 3, (info >> 23) & 0x7f);
            }
        }
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        for status in [0xffff_fc20_0003_f180u64, 0xffff_fc20_0003_0000] {
            dev_err!(self.dev.as_ref(), "G16: status {:#x}: {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x}\n",
                     status, fw.read_u64(status)?, fw.read_u64(status + 8)?, fw.read_u64(status + 0x10)?,
                     fw.read_u64(status + 0x18)?, fw.read_u64(status + 0x20)?, fw.read_u64(status + 0x28)?,
                     fw.read_u64(status + 0x30)?, fw.read_u64(status + 0x38)?);
        }
        // Never let one unreadable word end the report early.
        let word = |address: u64| fw.read_u32(address).unwrap_or(0xdead_beef);
        // The oldest publication; `flights` is unordered.
        let now = Instant::now();
        if let Some(flight) = self.flights.iter().max_by_key(|f| now - f.started) {
            for (stage, (pointers, head)) in flight.queues.iter().enumerate() {
                if *pointers == 0 {
                    continue;
                }
                // Ring words +0x0/+0x30/+0x40; on G13 these are the GPU done
                // pointer, GPU read pointer and CPU write pointer.
                dev_err!(self.dev.as_ref(), "G16: hang queue stage {} ring+0={} ring+0x30={} ring+0x40={} flight head={}\n",
                         stage, word(*pointers), word(*pointers + 0x30),
                         word(*pointers + 0x40), head);
            }
            if let Some(render) = &flight.render {
                let a = &render.addresses;
                dev_err!(self.dev.as_ref(), "G16: hang render TA={:#x} 3D={:#x} scene={} context={}\n",
                         a.tiling_work, a.fragment_work, render.scene, a.context_id);
                let mut line = KVec::new();
                for i in 0..0x600u64 / 12 {
                    let at = a.fragment_work + 0x80 + i * 12;
                    let number = fw.read_u32(at)?;
                    let value = u64::from(fw.read_u32(at + 4)?) | (u64::from(fw.read_u32(at + 8)?) << 32);
                    if number == 0 && value == 0 {
                        break;
                    }
                    line.push((number, value), GFP_KERNEL)?;
                    if line.len() == 6 {
                        dev_err!(self.dev.as_ref(), "G16: hang 3D regs {:x?}\n", &line[..]);
                        line.clear();
                    }
                }
                if !line.is_empty() {
                    dev_err!(self.dev.as_ref(), "G16: hang 3D regs {:x?}\n", &line[..]);
                }
            }
        }
        self.print_reuse_history();
        self.print_lane_events();
        for flight in self.flights.iter().take(24) {
            // blocked: bit 1 notifier, 2 ring, 3 channel, 4 control reply.
            dev_err!(self.dev.as_ref(), "G16: hang flight Work={:#x} context={} age_ms={} render={} stream={:#x} ordinal={} blocked={:#x} notifier={:?} tgid={} comm={}\n",
                     flight.id, flight.context, self.flight_ms(flight, Instant::now()),
                     flight.render.is_some(), flight.stream, flight.ordinal, flight.blocked,
                     flight.notifier_count.map(|(addr, total)| (word(addr), total)), flight.tgid,
                     comm_str(&flight.comm));
            for (stage, stamp) in flight.stamps.iter().enumerate() {
                if let Some(stamp) = stamp {
                    dev_err!(self.dev.as_ref(), "G16:   stage {} event {} fw stamp {:#x}={:#x} want {:#x}\n",
                             stage, stamp.event, stamp.address, word(stamp.address), stamp.value);
                }
            }
            for stamp in flight.driver_stamps.iter().flatten() {
                dev_err!(self.dev.as_ref(), "G16:   driver stamp {:#x}={:#x} want {:#x}\n",
                         stamp.address, word(stamp.address), stamp.value);
            }
            for &(pointers, head) in flight.queues.iter().filter(|q| q.0 != 0) {
                dev_err!(self.dev.as_ref(), "G16:   ring {:#x} +0={} +0x30={} +0x40={} head={}\n",
                         pointers, word(pointers), word(pointers + 0x30),
                         word(pointers + 0x40), head);
            }
            for &(channel, target) in flight.channels.iter().filter(|c| c.0 != usize::MAX) {
                dev_err!(self.dev.as_ref(), "G16:   channel {} target {}\n", channel, target);
            }
            for (kind, dep) in flight.dependencies.iter().enumerate() {
                if let Some(dep) = dep {
                    let stage = if kind == 0 { "render" } else { "compute" };
                    dev_err!(self.dev.as_ref(), "G16:   waits on {} stamp {:#x}={:#x} want {:#x} event {}\n",
                             stage, dep.address, word(dep.address), dep.value, dep.event);
                }
            }
        }
        // Newest firmware trace-ring entries (512 x 0x48 at 0xe5c40).
        let write = fw.read_u32(0xffff_fc20_0003_f3c0 + 0x20)?;
        for back in 1..=6u32 {
            let index = (write + 512 - back) % 512;
            let base = 0xffff_fc20_000e_5c40 + u64::from(index) * 0x48;
            dev_err!(self.dev.as_ref(), "G16: trace[{}] {:#018x} {:#018x} {:#018x} {:#018x} {:#018x} {:#018x}\n",
                     index, fw.read_u64(base)?, fw.read_u64(base + 8)?, fw.read_u64(base + 0x10)?,
                     fw.read_u64(base + 0x18)?, fw.read_u64(base + 0x20)?, fw.read_u64(base + 0x28)?);
        }
        Ok(())
    }

    pub(super) fn report_failed_event(&self, base: u64, kind: u32, cursor: u32) -> Result {
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        dev_err!(self.dev.as_ref(), "G16: failed Event kind={} cursor={} flights={}\n",
                 kind, cursor, self.flights.len());
        for offset in (0..0x48).step_by(8) {
            dev_err!(self.dev.as_ref(), "G16: Event +{:#x}={:#018x}\n",
                     offset, fw.read_u64(base + offset)?);
        }
        let _ = self.dump_hang_state();
        Ok(())
    }
}

impl Bootstrap {
    /// fw_trace & 256: a detached VM's stale-event summary.
    pub(crate) fn report_vm_detach(&mut self, vm_root: u64) {
        if crate::module_parameters::fw_trace.value() & 256 != 0 {
            let now = monotonic_ns();
            pr_info!(
                "G16: event summary at={}.{:06} (VM {:#x} detached): {} stale ({} live, {} live on own predecessor) of {} waits since boot\n",
                now / 1_000_000_000, (now % 1_000_000_000) / 1000, vm_root,
                self.diagnostics.stale_event_tags, self.diagnostics.stale_live_tags, self.diagnostics.stale_live_own_tags, self.diagnostics.dependency_waits
            );
        }
    }
}

impl Bootstrap {
    /// Hang report: for each stuck render, when its context slot last changed
    /// hands and what the previous occupant last retired there, and who used
    /// its Work memory before it. Negative times are after the stuck submit.
    pub(super) fn print_reuse_history(&self) {
        for flight in self.flights.iter().filter(|f| f.render.is_some()).take(8) {
            let slot = flight.context as usize;
            let root = flight.render.as_ref().map_or(0, |r| r.root);
            match self.diagnostics.handovers.get(slot).copied().flatten() {
                Some(h) => {
                    let other = if h.next == root { "" } else { " (not to this Work's VM)" };
                    dev_err!(self.dev.as_ref(),
                        "G16: hang history Work={:#x} slot {}: rebound {:#x} -> {:#x} {} us before submit{}\n",
                        flight.id, slot, h.previous, h.next, micros(flight.started, h.at),
                        other);
                    if let Some(r) = self.diagnostics.retire_log.iter()
                        .filter(|r| r.context as usize == slot && r.root == h.previous)
                        .max_by_key(|r| micros(r.retired, flight.started))
                    {
                        dev_err!(self.dev.as_ref(),
                            "G16: hang history   previous occupant tgid={} comm={} ordinal={} render={} retired {} us before the rebind, {} us before submit\n",
                            r.tgid, comm_str(&r.comm), r.ordinal, r.render,
                            micros(h.at, r.retired), micros(flight.started, r.retired));
                    }
                }
                None => dev_err!(self.dev.as_ref(),
                    "G16: hang history Work={:#x} slot {}: no rebind recorded\n", flight.id, slot),
            }
            match self.diagnostics.retire_log.iter().filter(|r| r.work == flight.id).max_by_key(|r| micros(r.retired, flight.started)) {
                Some(r) => dev_err!(self.dev.as_ref(),
                    "G16: hang history   Work memory last used by tgid={} comm={} slot {} ordinal={} vm {:#x}: submitted {} us and retired {} us before this submit\n",
                    r.tgid, comm_str(&r.comm), r.context, r.ordinal, r.root,
                    micros(flight.started, r.submitted), micros(flight.started, r.retired)),
                None => dev_err!(self.dev.as_ref(),
                    "G16: hang history   Work memory: no earlier use in the last {} retirements\n", RETIRE_LOG),
            }
        }
    }
}

impl Bootstrap {
    /// Diagnostics: the event a lane's queue signals now, by the lane's
    /// firmware stamp address (the address cross-queue waits name).
    pub(super) fn set_lane_event(&mut self, stamp: u64, event: u32) {
        if let Some(entry) = self.diagnostics.lane_events.iter_mut().find(|e| e.0 == stamp) {
            entry.1 = event;
        } else {
            let _ = self.diagnostics.lane_events.push((stamp, event), GFP_KERNEL);
        }
    }

    /// Diagnostics: count waits whose event tag differs from the event the
    /// producing lane's queue currently holds ("stale tag at publish").
    #[inline(never)]
    pub(super) fn note_dependency_events(&mut self, dependencies: &[Option<Stamp>; 2], own: [u64; 3]) {
        for dep in dependencies.iter().flatten() {
            let Some(&(_, current)) = self.diagnostics.lane_events.iter().find(|e| e.0 == dep.address) else {
                continue;
            };
            self.diagnostics.dependency_waits = self.diagnostics.dependency_waits.wrapping_add(1);
            if current != dep.event {
                self.diagnostics.stale_event_tags = self.diagnostics.stale_event_tags.wrapping_add(1);
                // Is the stale tag's index live for some other queue right
                // now (its event fires for them), or free?
                let index = dep.event.wrapping_sub(32) / 3;
                let live = index < 32
                    && self.queue_events.load(Ordering::Acquire) & (1 << index) != 0;
                if live {
                    self.diagnostics.stale_live_tags = self.diagnostics.stale_live_tags.wrapping_add(1);
                    // A wait on the publishing queue's own lane: its
                    // predecessor Work, published before an idle gap.
                    if own.contains(&dep.address) {
                        self.diagnostics.stale_live_own_tags = self.diagnostics.stale_live_own_tags.wrapping_add(1);
                    }
                }
                let slot = self.diagnostics.stale_next % self.diagnostics.stale_recent.len();
                self.diagnostics.stale_recent[slot] = (monotonic_ns(), dep.address, dep.value, dep.event, current, live);
                self.diagnostics.stale_next = self.diagnostics.stale_next.wrapping_add(1);
                if crate::module_parameters::fw_trace.value() & 256 != 0 && self.diagnostics.stale_event_tags <= 64 {
                    pr_info!(
                        "G16: stale event tag: wait on {:#x} want {:#x} names event {}, its queue now holds {}{}\n",
                        dep.address, dep.value, dep.event, current,
                        if live { " (index live for another queue)" } else { "" }
                    );
                }
            }
        }
    }

    /// Hang report: stale-tag totals and the event each known lane holds.
    pub(super) fn print_lane_events(&self) {
        dev_err!(self.dev.as_ref(), "G16: hang events: {} stale ({} on an index live for another queue, {} of those on the queue's own predecessor) of {} waits since boot; events in use {:#010x}\n",
                 self.diagnostics.stale_event_tags, self.diagnostics.stale_live_tags, self.diagnostics.stale_live_own_tags, self.diagnostics.dependency_waits,
                 self.queue_events.load(Ordering::Acquire) & 0xffff_ffff);
        // The most recent stale tags, oldest first.
        let len = self.diagnostics.stale_recent.len();
        let count = self.diagnostics.stale_next.min(len);
        for i in 0..count {
            let (at, stamp, value, tag, current, live) = self.diagnostics.stale_recent[(self.diagnostics.stale_next - count + i) % len];
            dev_err!(self.dev.as_ref(), "G16: hang stale tag at={}.{:06} wait on {:#x} want {:#x} names event {}, queue holds {}{}\n",
                     at / 1_000_000_000, (at % 1_000_000_000) / 1000, stamp, value, tag, current,
                     if live { " (live for another queue)" } else { "" });
        }
        let mut line = KVec::new();
        for &(stamp, event) in self.diagnostics.lane_events.iter().take(96) {
            if line.push((stamp, event), GFP_KERNEL).is_err() {
                break;
            }
            if line.len() == 6 {
                dev_err!(self.dev.as_ref(), "G16: hang lane events (stamp, event) {:x?}\n", &line[..]);
                line.clear();
            }
        }
        if !line.is_empty() {
            dev_err!(self.dev.as_ref(), "G16: hang lane events (stamp, event) {:x?}\n", &line[..]);
        }
    }
}

impl Bootstrap {
    /// A4 wedge (see tmp/kernel-73-validate/a4-recovery-design.md): a render
    /// whose TA is done and whose waits are all satisfied, while its 3D never
    /// starts, every channel is drained and no fragment stamp on any lane
    /// moves for wedge_ms. Firmware reports nothing for this state, so the
    /// kernel detects it and logs it once; the 10 s watchdog handles it.
    pub(super) fn check_wedge(&mut self) -> Result {
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        let mut stuck = None;
        let mut signature = 0u64;
        for (index, flight) in self.flights.iter().enumerate() {
            let (Some(_), Some(tiling), Some(fragment)) = (&flight.render, flight.stamps[0], flight.stamps[1]) else {
                continue;
            };
            let ta = fw.read_u32(tiling.address)?;
            let frag = fw.read_u32(fragment.address)?;
            signature = signature.wrapping_mul(0x100_0000_01b3).wrapping_add(u64::from(frag));
            if stuck.is_none()
                && g16_stamp::reached(ta, tiling.value)
                && !g16_stamp::reached(frag, fragment.value)
                && flight.dependencies.iter().flatten().all(|d| {
                    fw.read_u32(d.address).is_ok_and(|v| g16_stamp::reached(v, d.value))
                })
            {
                stuck = Some(index);
            }
        }
        let drained = {
            let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
            let guard = self._sgx.try_access().ok_or(ENODEV)?;
            config.channels.iter().take(12).all(|route| {
                let read = guard.try_read32(0xd64000 + (route.state[0] - 0xffff_fc20_0002_8000) as usize);
                let done = guard.try_read32(0xd64000 + (route.state[1] - 0xffff_fc20_0002_8000) as usize);
                let tail = guard.try_read32(0xd60000 + (route.state[2] - 0xffff_fc20_0002_0000) as usize);
                matches!((read, done, tail), (Ok(r), Ok(d), Ok(t)) if r == d && d == t)
            })
        };
        let Some(index) = stuck.filter(|_| drained) else {
            self.diagnostics.wedge_since = None;
            self.diagnostics.wedge_reported = false;
            return Ok(());
        };
        let now = Instant::now();
        match self.diagnostics.wedge_since {
            Some(since) if signature == self.diagnostics.wedge_signature => {
                let wedge_ms = u64::from(crate::module_parameters::wedge_ms.value().max(100));
                if (now - since).as_millis() < wedge_ms as i64 {
                    return Ok(());
                }
            }
            _ => {
                // First sighting, or some fragment stamp moved: restart.
                self.diagnostics.wedge_since = Some(now);
                self.diagnostics.wedge_signature = signature;
                return Ok(());
            }
        }
        if !self.diagnostics.wedge_reported {
            let flight = &self.flights[index];
            dev_err!(self.dev.as_ref(),
                "G16: A4 wedge: Work {:#x} context {} stream {:#x} ordinal {} age {} ms, no fragment progress for {} ms; detection only\n",
                flight.id, flight.context, flight.stream, flight.ordinal,
                self.flight_ms(flight, now), (now - self.diagnostics.wedge_since.unwrap_or(now)).as_millis());
            self.diagnostics.wedge_reported = true;
        }
        Ok(())
    }
}
