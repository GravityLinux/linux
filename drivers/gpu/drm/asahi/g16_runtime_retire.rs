// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! Retirement: completion scans of in-flight Works, render events (TVB
//! growth, partial renders) and the pump's watchdog windows.

use super::*;

impl Bootstrap {
    pub(super) fn stamps_done(&self, flight: &Flight) -> Result<bool> {
        let fw = self._firmware_space.as_ref().ok_or(EIO)?;
        for stamp in flight.stamps.iter().flatten() {
            if !g16_stamp::reached(fw.read_u32(stamp.address)?, stamp.value) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Dispatch events by the outstanding Work identity, never by the most
    /// recently submitted job. Completion notifications are hints; stamps and
    /// ring retirement remain authoritative even if event bits coalesce.
    pub(super) fn service_flights(&mut self) -> Result {
        const STATE: u64 = 0xffff_fc20_0003_f1c0;
        const RING: u64 = 0xffff_fc20_0003_f440;
        for _ in 0..256 {
            let fw = self._firmware_space.as_mut().ok_or(EIO)?;
            let cursor = fw.read_u32(STATE)?;
            let end = fw.read_u32(STATE + 0x20)?;
            if cursor >= 256 || end >= 256 {
                return Err(EIO);
            }
            if cursor == end {
                break;
            }
            let base = RING + u64::from(cursor) * 0x48;
            let kind = fw.read_u32(base)?;
            if kind == 1
                || (kind == 13
                    && self.opening_notification_pending
                    && fw.read_u32(base + 4)? == 1
                    && fw.read_u32(base + 8)? == 0
                    && fw.read_u64(base + 12)? == g16_fw::CONTROL_DATA
                    && fw.read_u64(base + 20)? == g16_fw::OPERAND_TABLE
                    && fw.read_u64(base + 28)? == g16_fw::OPERAND_TABLE
                    && fw.read_u32(base + 36)? == 20 * 8)
            {
                if kind == 13 {
                    self.opening_notification_pending = false;
                }
                fw.write_live(STATE, &((cursor + 1) & 255).to_le_bytes())?;
                continue;
            }
            if kind == 0 || kind == 4 {
                // Firmware fault (0) or timeout (4) report, as upstream's
                // EventMsg::Fault/Timeout. Upstream fails the pending work and
                // resumes the halted firmware; the G16 resume handshake is not
                // known yet, so nothing can continue after one of these.
                let what = if kind == 0 { "fault" } else { "timeout" };
                dev_err!(self.dev.as_ref(), "G16: firmware {} event\n", what);
                self.report_failed_event(base, kind, cursor)?;
                return Err(EIO);
            }
            let mut selected = None;
            for (index, flight) in self.flights.iter().enumerate() {
                if let Some(render) = &flight.render {
                    let a = &render.addresses;
                    let matches = match kind {
                        6 => {
                            fw.read_u32(base + 4)? == a.context_id as u32
                                && fw.read_u32(base + 8)? == a.buffer_manager_slot as u32
                                && fw.read_u32(a.fragment_firmware_stamp)?
                                    != a.fragment_stamp as u32
                        }
                        7 => {
                            fw.read_u32(base + 12)? == a.fragment_event as u32
                                && fw.read_u32(base + 16)? == a.fragment_stamp as u32
                        }
                        _ => false,
                    };
                    if matches {
                        selected = Some(index);
                        break;
                    }
                }
            }
            let Some(index) = selected else {
                self.report_failed_event(base, kind, cursor)?;
                return Err(EIO);
            };
            let mut render = self.flights[index].render.take().ok_or(EIO)?;
            let pool_index = self
                .tvb_pools
                .iter()
                .position(|p| p.root == render.root)
                .ok_or(EIO)?;
            let mut pool = self.tvb_pools.swap_remove(pool_index);
            let result = self.service_render_events(
                &mut render.client.lock(),
                &mut pool,
                &render.addresses,
                &mut render.events,
            );
            // Back into the slot swap_remove freed: no allocation.
            self.tvb_pools.push_within_capacity(pool).map_err(|_| EIO)?;
            self.flights[index].render = Some(render);
            result?;
            if self._firmware_space.as_ref().ok_or(EIO)?.read_u32(STATE)? == cursor {
                break;
            }
        }
        self.drain_trace_rx()
    }

    pub(crate) fn retire(&mut self) -> Result<Option<(u64, Result)>> {
        if self.render_failed {
            return Err(EIO);
        }
        // One scan retires every finished flight; later calls return them
        // without rereading firmware state for each one.
        if let Some((id, limited)) = self.retired.pop() {
            return Ok(Some((id, if limited { Err(ENOMEM) } else { Ok(()) })));
        }
        let before = crate::g16_timeline::reads();
        let serviced = self.service_flights();
        crate::g16_timeline::attribute(&crate::g16_timeline::P_RD_EVENTS, before);
        let scan_before = crate::g16_timeline::reads();
        serviced.inspect_err(|error| {
            dev_err!(
                self.dev.as_ref(),
                "G16: firmware event service failed: {:?}\n",
                error
            );
        })?;
        self.collect_destroyed_queues()?;
        let config = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS);
        // Device-wide transport state is the same for every flight: read each
        // register once per scan instead of once per flight.
        let mut channel_state: [Option<[u32; 2]>; 12] = [None; 12];
        let control_replied = {
            let guard = self._sgx.try_access().ok_or(ENODEV)?;
            let tail = guard.try_read32(0xd60060)?;
            guard.try_read32(0xd640c0)? == tail && guard.try_read32(0xd640c8)? == tail
        };
        // Preallocated to MAX_FLIGHTS, which bounds the flights scanned here.
        if self.retired.capacity() < self.flights.len() {
            return Err(EIO);
        }
        // Like upstream's per-queue event check, read each lane's stamp and
        // ring cursors once per scan and judge every flight on that lane
        // against the same snapshot: flights share a lane's stamp address
        // with rising values. A completion after the read is retired by the
        // pass its own notification triggers.
        let mut snapshot = WordSnapshot::default();
        let mut index = self.flights.len();
        while index > 0 {
            index -= 1;
            let flight = &self.flights[index];
            let fw = self._firmware_space.as_ref().ok_or(EIO)?;
            let mut reached_all = true;
            for stamp in flight.stamps.iter().flatten() {
                if !g16_stamp::reached(snapshot.read(fw, stamp.address)?, stamp.value) {
                    reached_all = false;
                    break;
                }
            }
            if !reached_all {
                continue;
            }
            let mut done = true;
            let mut blocked = 0u32;
            let mut notifier_seen = None;
            // The notifier pass drops embedded JobMeta references and cleans
            // their cache lines before publishing these later driver stamps.
            for stamp in flight.driver_stamps.iter().flatten() {
                done &= g16_stamp::reached(snapshot.read(fw, stamp.address)?, stamp.value);
            }
            blocked |= u32::from(!done);
            // A reusable Work has its own notifier and exact submitted count.
            // Removal must be visible before that storage can change queues.
            if done {
                let notified = match flight.notifier_count {
                    // The shared notifier has counted this render's Works.
                    Some((completed, total)) => {
                        let count = snapshot.read(fw, completed)?;
                        notifier_seen = Some((count, total));
                        (count.wrapping_sub(total) as i32) >= 0
                    }
                    None => {
                        let notifier = flight.id
                            + if flight.render.is_some() {
                                0x200
                            } else {
                                0x1400
                            };
                        fw.read_u32(notifier + 0x94)? == 0
                    }
                };
                blocked |= u32::from(!notified) << 1;
                done &= notified;
            }
            if done {
                for &(pointers, head) in &flight.queues {
                    if pointers != 0 {
                        let lane = reached(snapshot.read(fw, pointers)?, head, 0x500)
                            && reached(snapshot.read(fw, pointers + 0x30)?, head, 0x500);
                        blocked |= u32::from(!lane) << 2;
                        done &= lane;
                    }
                }
            }
            if done {
                for &(channel, target) in &flight.channels {
                    if channel == usize::MAX {
                        continue;
                    }
                    let state = match channel_state.get(channel).copied().flatten() {
                        Some(state) => state,
                        None => {
                            let guard = self._sgx.try_access().ok_or(ENODEV)?;
                            let mut state = [0; 2];
                            for (value, address) in
                                state.iter_mut().zip(&config.channels[channel].state[..2])
                            {
                                *value = guard.try_read32(
                                    0xd64000 + (*address - 0xffff_fc20_0002_8000) as usize,
                                )?;
                            }
                            if let Some(slot) = channel_state.get_mut(channel) {
                                *slot = Some(state);
                            }
                            state
                        }
                    };
                    for value in state {
                        let consumed = reached(value, target, 256);
                        blocked |= u32::from(!consumed) << 3;
                        done &= consumed;
                    }
                }
            }
            // Growth replies can refer to a Work after its main stage stamp.
            if flight.render.is_some() {
                blocked |= u32::from(!control_replied) << 4;
                done &= control_replied;
            }
            {
                let flight = &mut self.flights[index];
                if flight.stamps_seen.is_none() {
                    flight.stamps_seen = Some(Instant::now());
                }
                if !done {
                    flight.blocked = blocked;
                }
            }
            if !done {
                continue;
            }
            let flight = &self.flights[index];
            if self.poll_scan {
                // Classify each rescue. Stamps first seen in this very scan
                // (after_stamps_us ~0) mean no scan ran between completion
                // and the timeout: no notification came. Stamps seen earlier
                // with retirement blocked (bit 1: the shared notifier's count
                // short) mean the notification came and the retirement
                // condition held the Work back. Rescues are rare; the limit
                // only bounds a pathological boot.
                use core::sync::atomic::{AtomicU32, Ordering};
                static REPORTED: AtomicU32 = AtomicU32::new(0);
                if REPORTED.fetch_add(1, Ordering::Relaxed) < 256 {
                    let (relation, owner_retired) = match flight.job_notifier {
                        None => ("own", false),
                        Some(owner) => (
                            if owner == flight.id { "job-owner" } else { "job-member" },
                            self.job_notifiers
                                .iter()
                                .find(|j| j.owner == owner)
                                .is_some_and(|j| j.owner_root.is_some()),
                        ),
                    };
                    let (count, total) = notifier_seen.unwrap_or((u32::MAX, u32::MAX));
                    dev_warn!(
                        self.dev.as_ref(),
                        "G16: rescue {:#x} render={} comm={} ordinal={} age_ms={} after_stamps_us={} last_blocked={:#x} notifier={} owner_retired={} count={:#x}/{:#x}\n",
                        flight.id,
                        flight.render.is_some(),
                        core::str::from_utf8(&flight.comm)
                            .unwrap_or("?")
                            .trim_end_matches('\0'),
                        flight.ordinal,
                        flight.started.elapsed().as_millis(),
                        flight.stamps_seen.map_or(0, |t| t.elapsed().as_micros_ceil()),
                        flight.blocked,
                        relation,
                        owner_retired,
                        count,
                        total
                    );
                }
            }
            if crate::module_parameters::fw_trace.value() & 16 != 0 {
                dev_info!(
                    self.dev.as_ref(),
                    "G16: retire {:#x} render={} after_stamps_us={} last_blocked={:#x} flights={}\n",
                    flight.id,
                    flight.render.is_some(),
                    flight.stamps_seen.map_or(0, |t| t.elapsed().as_micros_ceil()),
                    flight.blocked,
                    self.flights.len()
                );
            }
            let limited = flight
                .render
                .as_ref()
                .is_some_and(|r| r.events.memory_limit);
            let id = flight.id;
            let render_work = flight.render.is_some();
            if let Some(render) = &flight.render {
                let pool = self
                    .tvb_pools
                    .iter_mut()
                    .find(|p| p.root == render.root)
                    .ok_or(EIO)?;
                pool.release_scene(render.scene);
            }
            let mut owner = flight.owner.lock();
            owner.pending -= 1;
            if owner.pending == 0 {
                owner.events = None;
            }
            drop(owner);
            self.context_users[flight.context as usize] -= 1;
            // The scan runs downwards: the flight swapped into `index` was
            // already examined. Order carries no meaning (removal shifted
            // every later flight, tens of KB per Work under load).
            let flight = self.flights.swap_remove(index);
            self.ledger.remove(&flight)?;
            let root = match &flight.render {
                Some(render) => render.root,
                None => flight.client.lock().roots().low,
            };
            let entry = Retired {
                work: id,
                context: flight.context,
                root,
                tgid: flight.tgid,
                comm: flight.comm,
                ordinal: flight.ordinal,
                render: render_work,
                submitted: flight.started,
                retired: Instant::now(),
            };
            // Retirement must not allocate: capacity is reserved up front.
            if self.diagnostics.retire_log.len() < RETIRE_LOG {
                let _ = self.diagnostics.retire_log.push_within_capacity(entry);
            } else {
                let slot = self.diagnostics.retire_next % RETIRE_LOG;
                self.diagnostics.retire_log[slot] = entry;
            }
            self.diagnostics.retire_next = self.diagnostics.retire_next.wrapping_add(1);
            flight.client.lock().release_scratch(flight.scratch)?;
            // allocate_work reserved a slot for every outstanding lease. A
            // render whose submission shares its notifier keeps the owner
            // Work's storage leased until the last render using it retires.
            let mut free = [Some((id, root)), None];
            if let Some(owner_va) = flight.job_notifier {
                if let Some(p) = self.job_notifiers.iter().position(|j| j.owner == owner_va) {
                    let entry = &mut self.job_notifiers[p];
                    entry.refs = entry.refs.saturating_sub(1);
                    if id == owner_va {
                        entry.owner_root = Some(root);
                        free[0] = None;
                    }
                    if entry.refs == 0 {
                        if let Some(owner_root) = entry.owner_root {
                            free[1] = Some((owner_va, owner_root));
                            self.job_notifiers.swap_remove(p);
                        }
                    }
                }
            }
            for (va, root) in free.into_iter().flatten() {
                self.free_work[usize::from(render_work)]
                    .push_within_capacity((va, root))
                    .map_err(|_| EIO)?;
            }
            self.retired.push_within_capacity((id, limited)).map_err(|_| ENOMEM)?;
        }
        crate::g16_timeline::attribute(&crate::g16_timeline::P_RD_SCAN, scan_before);
        *self.last_scan = snapshot;
        if !self.retired.is_empty() {
            // Queued commands can legitimately wait longer than one Work's
            // timeout. Watch for a lack of retirement progress, rather than
            // charging every command for all its predecessors' execution.
            // One timestamp for all of them, not a write to every flight.
            self.progress = Some(Instant::now());
            let (id, limited) = self.retired.pop().ok_or(EIO)?;
            return Ok(Some((id, if limited { Err(ENOMEM) } else { Ok(()) })));
        }
        // Wedge detection needs a few samples per wedge_ms window (default
        // 2 s), not one per pump pass: each sample reads two stamps per
        // outstanding render and 36 channel registers.
        let sample = self.diagnostics.wedge_sampled.is_none_or(|t| t.elapsed().as_millis() >= 50);
        if sample {
            self.diagnostics.wedge_sampled = Some(Instant::now());
            let before = crate::g16_timeline::reads();
            let checked = self.check_wedge();
            crate::g16_timeline::attribute(&crate::g16_timeline::P_RD_WEDGE, before);
            checked?;
        }
        let now = Instant::now();
        if let Some(flight) = self
            .flights
            .iter()
            .find(|f| self.flight_ms(f, now) >= 10000)
        {
            dev_err!(
                self.dev.as_ref(),
                "G16: async Work {:#x} context {} timeout without retirement progress\n",
                flight.id,
                flight.context
            );
            let _ = self.dump_hang_state();
            self.render_failed = true;
            return Err(ETIMEDOUT);
        }
        Ok(None)
    }

    /// Notifications do not extend the watchdog. Only retirement progress
    /// resets it, so trace traffic cannot keep a stuck command alive.
    /// Age of the oldest flight since the last retirement progress (ms).
    /// A flight's age since the later of its publication and the latest
    /// retirement progress.
    pub(super) fn flight_ms(&self, flight: &Flight, now: Instant<Monotonic>) -> i64 {
        let age = now - flight.started;
        self.progress.map_or(age, |p| age.min(now - p)).as_millis()
    }

    /// Mark the following retire scans as safety-poll rescues.
    pub(crate) fn set_poll_scan(&mut self, on: bool) {
        self.poll_scan = on;
    }

    pub(crate) fn oldest_flight_ms(&self) -> u64 {
        // One clock read for all flights.
        let now = Instant::now();
        self.flights
            .iter()
            .map(|flight| self.flight_ms(flight, now) as u64)
            .max()
            .unwrap_or(0)
    }

    pub(crate) fn watchdog_remaining_ms(&self) -> u32 {
        let now = Instant::now();
        let watchdog = self
            .flights
            .iter()
            .map(|flight| (10000 - self.flight_ms(flight, now)).clamp(1, 10000) as u32)
            .min()
            .unwrap_or(10000);
        // Wedge detection samples render progress a few times per window.
        // A recorded unreachable hold is re-checked within its grace second.
        let watchdog = if self.flights.iter().any(|f| f.render.is_some()) {
            watchdog.min(250)
        } else if self.diagnostics.hold_key.load(Ordering::Relaxed) != 0 {
            watchdog.min(500)
        } else {
            watchdog
        };
        // With work outstanding, recheck at least this often, so a late or
        // lost completion notification costs milliseconds.
        if !self.flights.is_empty() {
            watchdog.min(crate::g16_timeline::SAFETY_POLL_MS)
        } else {
            watchdog
        }
    }
}

impl Bootstrap {
    /// Consume only events owned by this publication. Growth transfers retained
    /// pages -> block list -> host counts -> reply -> producer, as in the shim.
    pub(super) fn service_render_events(
        &mut self,
        client: &mut g16_vm::AddressSpace,
        pool: &mut crate::g16_tvb::Tvb,
        a: &render::Addresses,
        service: &mut RenderEvents,
    ) -> Result {
        const STATE: u64 = 0xfffffc200003f1c0;
        const RING: u64 = 0xfffffc200003f440;
        let fw = self._firmware_space.as_mut().ok_or(EIO)?;
        let mut cursor = fw.read_u32(STATE)?;
        let end = fw.read_u32(STATE + 0x20)?;
        if cursor >= 256 || end >= 256 {
            return Err(EIO);
        }
        if cursor != end && crate::module_parameters::fw_trace.value() & 64 != 0 {
            // SAFETY: ktime_get() has no preconditions.
            let now = unsafe { kernel::bindings::ktime_get() };
            let base = RING + u64::from(cursor) * 0x48;
            pr_info!(
                "G16: rxring {} event {}..{} first={:#x}/{:#x}\n",
                now, cursor, end, fw.read_u32(base)?, fw.read_u32(base + 4)?
            );
        }
        while cursor != end {
            let base = RING + u64::from(cursor) * 0x48;
            let kind = fw.read_u32(base)?;
            let next = (cursor + 1) & 255;
            if kind == 1 {
                fw.write_live(STATE, &next.to_le_bytes())?;
                cursor = next;
                continue;
            }
            if !matches!(kind, 6 | 7) {
                dev_err!(self.dev.as_ref(), "G16: unexpected render Event {}\n", kind);
                return Err(EIO);
            }
            let guard = self._sgx.try_access().ok_or(ENODEV)?;
            let sgx = &*guard;
            let head = sgx.try_read32(0xd640c0)?;
            let ack = sgx.try_read32(0xd640c8)?;
            let tail = sgx.try_read32(0xd60060)?;
            drop(guard);
            if head >= 256 || ack >= 256 || tail >= 256 {
                return Err(EIO);
            }
            let successor = (tail + 1) & 255;
            if successor == head || successor == ack {
                break;
            }
            let mut reply = [0u8; 64];
            if kind == 6 {
                let subpipe = fw.read_u32(base + 16)?;
                let halt_count = fw.read_u64(base + 20)?;
                if fw.read_u32(base + 4)? != a.context_id as u32
                    || fw.read_u32(base + 8)? != pool.addresses.buffer_manager_slot as u32
                    || fw.read_u32(base + 12)? != pool.counter
                    || subpipe >= 64
                {
                    return Err(EIO);
                }
                let grown = pool.grow(fw, client, a.context_id as u8)?;
                {
                    use crate::g16_timeline as tl;
                    use core::sync::atomic::Ordering::Relaxed;
                    tl::TVB_REQUESTS.fetch_add(1, Relaxed);
                    if grown { &tl::TVB_GROWN } else { &tl::TVB_REFUSED }.fetch_add(1, Relaxed);
                    if !grown {
                        let reason = pool.refusal.unwrap_or(crate::g16_tvb::Refusal::Memory);
                        match reason {
                            crate::g16_tvb::Refusal::Cap => &tl::TVB_REFUSED_CAP,
                            crate::g16_tvb::Refusal::Va => &tl::TVB_REFUSED_VA,
                            crate::g16_tvb::Refusal::Memory => &tl::TVB_REFUSED_MEMORY,
                        }
                        .fetch_add(1, Relaxed);
                        if !pool.warned {
                            pool.warned = true;
                            // Warn for the first refusal of the boot only: CTS
                            // performance cases refuse in every process.
                            if !crate::g16_tvb::REFUSAL_WARNED.swap(true, Relaxed) {
                                dev_warn!(
                                    self.dev.as_ref(),
                                    "G16: TVB slot {} growth refused at {} blocks ({}); its renders throttle until pages free (later pools: info)\n",
                                    pool.addresses.buffer_manager_slot,
                                    pool.blocks,
                                    reason.name()
                                );
                            } else {
                                dev_info!(
                                    self.dev.as_ref(),
                                    "G16: TVB slot {} growth refused at {} blocks ({})\n",
                                    pool.addresses.buffer_manager_slot,
                                    pool.blocks,
                                    reason.name()
                                );
                            }
                        }
                    }
                }
                dev_info!(
                    self.dev.as_ref(),
                    "G16: TVB slot {} blocks {} growth {} subpipe {}\n",
                    pool.addresses.buffer_manager_slot,
                    pool.blocks,
                    grown,
                    subpipe
                );
                for (i, word) in [
                    8,
                    u32::from(grown),
                    pool.addresses.buffer_manager_slot as u32,
                    a.context_id as u32,
                    pool.counter,
                ]
                .iter()
                .enumerate()
                {
                    reply[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
                }
                // Native allocateMemoryEvent echoes the request's subpipe
                // and halt counter. Queued work can request growth on another
                // subpipe before its main stage starts, so this state belongs
                // to the pool, not whichever Work is currently executing.
                reply[20..24].copy_from_slice(&subpipe.to_le_bytes());
                reply[24..32].copy_from_slice(&halt_count.to_le_bytes());
                pool.refused |= !grown;
                pool.counter = pool.counter.checked_add(1).ok_or(EIO)?;
            } else {
                if !pool.refused
                    || service.memory_limit
                    || fw.read_u32(base + 4)? != 0
                    || fw.read_u32(base + 8)? != 1
                    || fw.read_u32(base + 12)? != a.fragment_event as u32
                    || fw.read_u32(base + 16)? != a.fragment_stamp as u32
                {
                    return Err(EIO);
                }
                crate::g16_timeline::TVB_MEMORY_LIMIT
                    .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                reply[..4].copy_from_slice(&9u32.to_le_bytes());
                reply[4..8].copy_from_slice(&1u32.to_le_bytes());
                reply[8..16].copy_from_slice(&a.fragment_queue.to_le_bytes());
                reply[16..20].copy_from_slice(&(a.fragment_stamp as u32).to_le_bytes());
                service.memory_limit = true;
            }
            crate::mem::sync();
            let control = g16_fw::MainConfig::bootstrap(g16_fw::BUNDLE_ADDRESS).channels[12].ring;
            fw.write_live(control + u64::from(tail) * 64, &reply)?;
            crate::mem::sync();
            fw.write_live(STATE, &next.to_le_bytes())?;
            let guard = self._sgx.try_access().ok_or(ENODEV)?;
            guard.try_write32(successor, 0xd60060)?;
            drop(guard);
            crate::mem::sync();
            self._rtkit
                .as_mut()
                .send_message(0x21, 0x0084_0000_0000_0011)?;
            return Ok(());
        }
        Ok(())
    }
}
