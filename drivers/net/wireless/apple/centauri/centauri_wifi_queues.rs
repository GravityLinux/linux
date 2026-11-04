// SPDX-License-Identifier: GPL-2.0-only

//! Generated cold Wi-Fi queues, solicited management and bounded scan events.
//! C retains the coherent arena, PCI BAR and serialized protocol state.

use super::centauri_radio::{
    ADMIN_COMPLETION, ADMIN_SUBMISSION, DEVICE_COMPLETIONS, DEVICE_SUBMISSIONS, HOST_COMPLETIONS,
    HOST_SUBMISSIONS, WIFI_ARENA_BYTES, WIFI_POOL_BYTES,
};
use super::centauri_wifi_connection as connection;
use super::centauri_wifi_scan as scan;
use core::{ffi::c_void, ptr};
use kernel::prelude::*;

const DESCRIPTOR_BYTES: usize = 16;
const ADMIN_ENTRIES: u16 = 16;
const ADMIN_FRAME_AREA: usize = 0x10000;
const ADMIN_FRAME_STRIDE: usize = 0x80;
const NOTIFY: usize = 0x1004;
const FIRMWARE_STATE: usize = 8;
const IPC_STATE: usize = 12;
const CONTEXT_LOW: usize = 0x10;
const CONTEXT_HIGH: usize = 0x14;
const RING_AREA: usize = 0x20000;
const RING_BYTES: usize = 0x8000;
const COMPLETION_KIND: u32 = 0x401;
const EXTERNAL_DESCRIPTOR: u32 = 1;
const MANAGEMENT_HEADER_BYTES: usize = 24;
const MANAGEMENT_REPLY_BYTES: usize = 4096;
const WAIT_ATTEMPTS: usize = 1000;
const QUERY_INITIAL_PARAMETERS: u32 = 8;
const QUERY_COUNTRY: u32 = 10;
const QUERY_CHANNELS: u32 = 47;
const SET_ADDRESS_TEMPLATE: u32 = 1;
const ACTIVATE_INTERFACE: u32 = 2;
const DEACTIVATE_INTERFACE: u32 = 3;
const QUERY_INTERFACE_INFO: u32 = 20;
const INTERFACE_INACTIVE: u8 = 0;
const INTERFACE_ACTIVE: u8 = 2;
// Qualified wire form: selector byte0 followed by a zero u32 options word.
// An empty activation body is invalid; the option bits remain unassigned.
const INTERFACE_OPERATION: [u8; 5] = [0; 5];

unsafe extern "C" {
    fn centauri_boot_read(bar: *mut c_void, offset: usize) -> u32;
    fn centauri_boot_write(bar: *mut c_void, offset: usize, value: u32);
    fn centauri_boot_sleep();
    fn centauri_control_dma_wmb();
    fn centauri_control_dma_rmb();
    fn centauri_radio_scan_frame(
        owner: *mut c_void,
        frequency: u16,
        signal: i8,
        frame: *const u8,
        bytes: usize,
    );
    fn centauri_radio_scan_done(owner: *mut c_void, aborted: bool);
    fn centauri_radio_connect_done(owner: *mut c_void, bssid: *const u8, status: u16);

}

fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[derive(Clone, Copy)]
struct Profile {
    number: u16,
    submissions: u16,
    completions: u16,
    capacity: usize,
    receive_target: u16,
}

const PROFILES: [Profile; 6] = [
    Profile {
        number: 6,
        submissions: 128,
        completions: 1024,
        capacity: 8192,
        receive_target: 32,
    },
    Profile {
        number: 5,
        submissions: 16,
        completions: 16,
        capacity: 2048,
        receive_target: 8,
    },
    Profile {
        number: 2,
        submissions: 154,
        completions: 154,
        capacity: 4096,
        receive_target: 153,
    },
    Profile {
        number: 3,
        submissions: 96,
        completions: 96,
        capacity: 4096,
        receive_target: 95,
    },
    Profile {
        number: 4,
        submissions: 512,
        completions: 512,
        capacity: 2560,
        receive_target: 511,
    },
    Profile {
        number: 1,
        submissions: 128,
        completions: 128,
        capacity: 8192,
        receive_target: 0,
    },
];
const SETUP_ORDER: [(u32, u16); 12] = [
    (2, 6),
    (2, 5),
    (1, 6),
    (1, 5),
    (2, 1),
    (1, 1),
    (2, 2),
    (1, 2),
    (2, 3),
    (1, 3),
    (2, 4),
    (1, 4),
];
const POLL_ORDER: [u16; 6] = [6, 5, 1, 2, 3, 4];

struct Queue {
    profile: Profile,
    submission: usize,
    completion: usize,
    pool: usize,
    producer: u16,
    cursor: u16,
    active: u16,
    // A consumed SQ descriptor is not necessarily a completed buffer. Keep
    // ownership until its matching CQ cookie returns, including out of order.
    outstanding: [u64; 8],
}

impl Queue {
    fn new(profile: Profile) -> Self {
        Self {
            profile,
            submission: 0,
            completion: 0,
            pool: 0,
            producer: 0,
            cursor: 0,
            active: 0,
            outstanding: [0; 8],
        }
    }

    fn owned_by_firmware(&self, cookie: u16) -> bool {
        self.outstanding[cookie as usize / 64] & (1 << (cookie % 64)) != 0
    }

    fn set_owner(&mut self, cookie: u16, firmware: bool) {
        let bit = 1 << (cookie % 64);
        let word = &mut self.outstanding[cookie as usize / 64];
        if firmware {
            *word |= bit;
        } else {
            *word &= !bit;
        }
    }

    fn registration(&self, kind: u32, dma: u64) -> ([u8; 52], usize) {
        let mut frame = [0; 52];
        let number = u32::from(self.profile.number);
        put32(&mut frame, 0, kind);
        put32(&mut frame, 4, number << 16 | number);
        if kind == 2 {
            put64(&mut frame, 8, dma + self.completion as u64);
            put32(
                &mut frame,
                16,
                0xffff0000 | u32::from(self.profile.completions),
            );
            put32(&mut frame, 20, 0xffff);
            // Qualified basic completion profile. Remaining flag roles are
            // not inferred from success of the registration transport.
            put32(&mut frame, 28, 2);
            (frame, 44)
        } else {
            put64(&mut frame, 8, dma + self.submission as u64);
            put64(&mut frame, 16, u64::MAX);
            put32(
                &mut frame,
                24,
                number << 16 | u32::from(self.profile.submissions),
            );
            // Qualified basic submission profile; individual bits stay open.
            put32(&mut frame, 28, 0x80000);
            put32(&mut frame, 36, 2);
            (frame, 52)
        }
    }
}

#[derive(Clone, Copy)]
struct Awaiting {
    transaction: u16,
    command: u32,
    reply_command: u32,
}

struct Scan {
    owner: *mut c_void,
    cookie: u8,
    reports: u16,
    invalid: u16,
    complete: Option<scan::Completion>,
}

struct Connection {
    owner: *mut c_void,
    transaction: u16,
    ssid: [u8; 32],
    ssid_bytes: usize,
    bssid: [u8; 6],
    outcome: Option<u16>,
    authentication: Option<SaeWatch>,
}

#[derive(Clone, Copy)]
struct SaeWatch {
    bssid: [u8; 6],
    h2e_only: bool,
}

pub(crate) struct WifiQueues {
    bar: *mut c_void,
    arena: *mut u8,
    dma: u64,
    queues: [Queue; 6],
    next_transaction: u16,
    awaiting: Option<Awaiting>,
    reply: KVec<u8>,
    reply_bytes: usize,
    reply_word3: u32,
    reply_ready: bool,
    received: u64,
    transmitted: u64,
    interface_active: bool,
    interface_address: [u8; 6],
    scan: Option<Scan>,
    scan_buffer: KVec<u8>,
    connection: Option<Connection>,
    secret_tx_cookie: Option<u16>,
    associated: bool,
    disconnecting: Option<(u16, bool)>,
    sae_watch: Option<SaeWatch>,
}

impl WifiQueues {
    /// # Safety
    /// C retains the enabled BAR and coherent allocation for every state use.
    pub(crate) unsafe fn new(
        bar: *mut c_void,
        arena: *mut u8,
        dma: u64,
        bytes: usize,
    ) -> Result<Self> {
        if bar.is_null()
            || arena.is_null()
            || arena as usize & 3 != 0
            || bytes != WIFI_ARENA_BYTES + WIFI_POOL_BYTES
            || dma & 0x3fff != 0
            || dma
                .checked_add(bytes as u64)
                .is_none_or(|end| end > 1 << 36)
        {
            return Err(EINVAL);
        }
        let mut queues = PROFILES.map(Queue::new);
        let mut pool = WIFI_ARENA_BYTES;
        for queue in &mut queues {
            queue.pool = pool;
            pool += usize::from(queue.profile.submissions) * queue.profile.capacity;
            pool = (pool + 0x3fff) & !0x3fff;
        }
        if pool > bytes || RING_AREA + SETUP_ORDER.len() * RING_BYTES > WIFI_ARENA_BYTES {
            return Err(EINVAL);
        }
        for (index, (kind, number)) in SETUP_ORDER.into_iter().enumerate() {
            let queue = queues
                .iter_mut()
                .find(|q| q.profile.number == number)
                .ok_or(EINVAL)?;
            let offset = RING_AREA + index * RING_BYTES;
            if kind == 2 {
                queue.completion = offset;
            } else {
                queue.submission = offset;
            }
        }
        let state = Self {
            bar,
            arena,
            dma,
            queues,
            next_transaction: 1,
            awaiting: None,
            reply: KVec::from_elem(0, MANAGEMENT_REPLY_BYTES, GFP_KERNEL)?,
            reply_bytes: 0,
            reply_word3: 0,
            reply_ready: false,
            received: 0,
            transmitted: 0,
            interface_active: false,
            interface_address: [0; 6],
            scan: None,
            scan_buffer: KVec::from_elem(0, 2560, GFP_KERNEL)?,
            connection: None,
            secret_tx_cookie: None,
            associated: false,
            disconnecting: None,
            sae_watch: None,
        };
        state.ready()?;
        // SAFETY: C retains the validated enabled BAR; this is its own context.
        let context = unsafe {
            u64::from(centauri_boot_read(bar, CONTEXT_LOW))
                | u64::from(centauri_boot_read(bar, CONTEXT_HIGH)) << 32
        };
        if context != dma {
            return Err(EINVAL);
        }
        Ok(state)
    }

    fn ready(&self) -> Result {
        // SAFETY: The enabled BAR outlives the serialized protocol state.
        if unsafe { centauri_boot_read(self.bar, FIRMWARE_STATE) } != 2
            || unsafe { centauri_boot_read(self.bar, IPC_STATE) } != 2
        {
            return Err(EIO);
        }
        Ok(())
    }

    fn read<const N: usize>(&self, offset: usize) -> Result<[u8; N]> {
        if offset
            .checked_add(N)
            .is_none_or(|end| end > WIFI_ARENA_BYTES + WIFI_POOL_BYTES)
        {
            return Err(EINVAL);
        }
        let mut bytes = [0; N];
        // SAFETY: The checked range is live coherent RAM. Volatile byte
        // access also handles unaligned u64 descriptor pointers without UB.
        unsafe {
            for (index, value) in bytes.iter_mut().enumerate() {
                *value = ptr::read_volatile(self.arena.add(offset + index));
            }
        }
        Ok(bytes)
    }

    fn write(&self, offset: usize, bytes: &[u8]) -> Result {
        if offset
            .checked_add(bytes.len())
            .is_none_or(|end| end > WIFI_ARENA_BYTES + WIFI_POOL_BYTES)
        {
            return Err(EINVAL);
        }
        // SAFETY: Private callers establish host ownership before replacing
        // a bounded descriptor, frame, cursor or completed CQ entry.
        unsafe {
            for (index, byte) in bytes.iter().enumerate() {
                ptr::write_volatile(self.arena.add(offset + index), *byte);
            }
        }
        Ok(())
    }

    fn index(&self, base: usize, number: u16, entries: u16) -> Result<u16> {
        let value = le16(&self.read::<2>(base + usize::from(number) * 2)?, 0);
        if value >= entries {
            return Err(EIO);
        }
        Ok(value)
    }

    fn publish(&self, base: usize, number: u16, value: u16) -> Result {
        // SAFETY: Publish initialized descriptors or consumed completions
        // before the corresponding host-owned cursor becomes observable.
        unsafe { centauri_control_dma_wmb() };
        self.write(base + usize::from(number) * 2, &value.to_le_bytes())?;
        // SAFETY: Cursor publication precedes the generic queue notification.
        unsafe {
            centauri_control_dma_wmb();
            centauri_boot_write(self.bar, NOTIFY, 1);
        }
        Ok(())
    }

    pub(crate) fn admin(&self, frame: &[u8]) -> Result {
        self.ready()?;
        if !matches!(frame.len(), 44 | 52 | 54 | 62) {
            return Err(EINVAL);
        }
        let device_cq = self.index(DEVICE_COMPLETIONS, 0, ADMIN_ENTRIES)?;
        let device_sq = self.index(DEVICE_SUBMISSIONS, 0, ADMIN_ENTRIES)?;
        let host_cq = self.index(HOST_COMPLETIONS, 0, ADMIN_ENTRIES)?;
        let producer = self.index(HOST_SUBMISSIONS, 0, ADMIN_ENTRIES)?;
        if device_cq != producer || device_sq != producer {
            return Err(EBUSY);
        }
        let pending = (device_cq + ADMIN_ENTRIES - host_cq) % ADMIN_ENTRIES;
        if pending > 1 {
            return Err(EIO);
        }
        if pending != 0 {
            self.write(
                ADMIN_COMPLETION + usize::from(host_cq) * DESCRIPTOR_BYTES,
                &[0; 16],
            )?;
        }
        let offset = ADMIN_FRAME_AREA + usize::from(producer) * ADMIN_FRAME_STRIDE;
        self.write(offset, frame)?;
        let mut descriptor = [0; DESCRIPTOR_BYTES];
        put32(
            &mut descriptor,
            0,
            (frame.len() as u32) << 8 | EXTERNAL_DESCRIPTOR,
        );
        put64(&mut descriptor, 4, self.dma + offset as u64);
        put32(&mut descriptor, 12, u32::from(producer));
        self.write(
            ADMIN_SUBMISSION + usize::from(producer) * DESCRIPTOR_BYTES,
            &descriptor,
        )?;
        // Defer notification until both admin cursors are ready together.
        unsafe { centauri_control_dma_wmb() };
        self.write(HOST_COMPLETIONS, &device_cq.to_le_bytes())?;
        let next = (producer + 1) % ADMIN_ENTRIES;
        self.publish(HOST_SUBMISSIONS, 0, next)?;
        for _ in 0..WAIT_ATTEMPTS {
            self.ready()?;
            if self.index(DEVICE_COMPLETIONS, 0, ADMIN_ENTRIES)? == next
                && self.index(DEVICE_SUBMISSIONS, 0, ADMIN_ENTRIES)? == next
            {
                // SAFETY: Completed device cursors transfer this CQ slot.
                unsafe { centauri_control_dma_rmb() };
                let entry =
                    self.read::<16>(ADMIN_COMPLETION + usize::from(producer) * DESCRIPTOR_BYTES)?;
                if le32(&entry, 0) != COMPLETION_KIND
                    || le32(&entry, 4) != (frame.len() as u32) << 16 | u32::from(producer)
                {
                    return Err(EIO);
                }
                return Ok(());
            }
            unsafe { centauri_boot_sleep() };
        }
        Err(ETIMEDOUT)
    }

    fn queue_index(&self, number: u16) -> Result<usize> {
        self.queues
            .iter()
            .position(|q| q.profile.number == number)
            .ok_or(EINVAL)
    }

    fn replenish(&mut self, index: usize) -> Result {
        let queue = &self.queues[index];
        let profile = queue.profile;
        let consumed = self.index(DEVICE_SUBMISSIONS, profile.number, profile.submissions)?;
        let mut producer = queue.producer;
        let mut added = 0;
        while queue.active + added < profile.receive_target {
            if (producer + 1) % profile.submissions == consumed || queue.owned_by_firmware(producer)
            {
                break;
            }
            let mut descriptor = [0; DESCRIPTOR_BYTES];
            put32(
                &mut descriptor,
                0,
                (profile.capacity as u32) << 8 | EXTERNAL_DESCRIPTOR,
            );
            put64(
                &mut descriptor,
                4,
                self.dma + (queue.pool + usize::from(producer) * profile.capacity) as u64,
            );
            put32(&mut descriptor, 12, u32::from(producer));
            self.write(
                queue.submission + usize::from(producer) * DESCRIPTOR_BYTES,
                &descriptor,
            )?;
            producer = (producer + 1) % profile.submissions;
            added += 1;
        }
        if added == 0 {
            return Ok(());
        }
        let queue = &mut self.queues[index];
        for step in 0..added {
            queue.set_owner((queue.producer + step) % profile.submissions, true);
        }
        queue.active += added;
        queue.producer = producer;
        self.publish(HOST_SUBMISSIONS, profile.number, producer)
    }

    pub(crate) fn configure(&mut self) -> Result {
        for base in [
            DEVICE_COMPLETIONS,
            DEVICE_SUBMISSIONS,
            HOST_COMPLETIONS,
            HOST_SUBMISSIONS,
        ] {
            if self.index(base, 0, ADMIN_ENTRIES)? != 0 {
                return Err(EBUSY);
            }
            for queue in &self.queues {
                let entries = if matches!(base, DEVICE_COMPLETIONS | HOST_COMPLETIONS) {
                    queue.profile.completions
                } else {
                    queue.profile.submissions
                };
                if self.index(base, queue.profile.number, entries)? != 0 {
                    return Err(EBUSY);
                }
            }
        }
        for (kind, number) in SETUP_ORDER {
            let index = self.queue_index(number)?;
            let (frame, bytes) = self.queues[index].registration(kind, self.dma);
            self.admin(&frame[..bytes])?;
            if kind == 1 && matches!(number, 6 | 5) {
                self.replenish(index)?;
            }
        }
        for number in [3, 4, 2] {
            self.replenish(self.queue_index(number)?)?;
        }
        let completed = self.index(DEVICE_COMPLETIONS, 0, ADMIN_ENTRIES)?;
        self.publish(HOST_COMPLETIONS, 0, completed)
    }

    fn management_reply(&mut self, offset: usize, length: usize) -> Result {
        if length < MANAGEMENT_HEADER_BYTES {
            return Err(EIO);
        }
        let header = self.read::<MANAGEMENT_HEADER_BYTES>(offset)?;
        if usize::from(le16(&header, 16)) != length {
            return Err(EIO);
        }
        if le32(&header, 20) == scan::COMPLETE && self.scan.is_some() {
            if length != MANAGEMENT_HEADER_BYTES + 9 {
                return Err(EIO);
            }
            let body = self.read::<9>(offset + MANAGEMENT_HEADER_BYTES)?;
            let complete = scan::completion(&body)?;
            let pending = self.scan.as_mut().ok_or(EIO)?;
            if complete.cookie == pending.cookie {
                pending.complete = Some(complete);
            }
            return Ok(());
        }
        if le32(&header, 20) == connection::DISCONNECT_COMPLETE {
            if let Some((transaction, complete)) = &mut self.disconnecting {
                if le16(&header, 18) == *transaction {
                    if length != MANAGEMENT_HEADER_BYTES || le32(&header, 12) != 0 {
                        return Err(EIO);
                    }
                    *complete = true;
                }
            }
            return Ok(());
        }
        if le32(&header, 20) == connection::COMPLETE {
            if let Some(pending) = &self.connection {
                if le16(&header, 18) == pending.transaction {
                    if length != MANAGEMENT_HEADER_BYTES + connection::RESULT_BYTES {
                        return Err(EIO);
                    }
                    let body = self.read::<44>(offset + MANAGEMENT_HEADER_BYTES)?;
                    let outcome = connection::outcome(&body)?;
                    if outcome.status != 0
                        || (outcome.ssid == &pending.ssid[..pending.ssid_bytes]
                            && outcome.bssid == pending.bssid)
                    {
                        self.connection.as_mut().ok_or(EIO)?.outcome = Some(outcome.status);
                    }
                }
            }
            return Ok(());
        }
        let Some(awaiting) = self.awaiting else {
            return Ok(());
        };
        let command = le32(&header, 20);
        let word3 = le32(&header, 12);
        if le16(&header, 18) != awaiting.transaction
            || !(command == awaiting.reply_command || command == awaiting.command && word3 != 0)
        {
            return Ok(());
        }
        if self.reply_ready {
            return Err(EIO);
        }
        let bytes = length - MANAGEMENT_HEADER_BYTES;
        if bytes > self.reply.len() {
            return Err(EMSGSIZE);
        }
        // SAFETY: Only a solicited completed management body is copied, before
        // its CQ/cookie is recycled. No firmware pointer is followed.
        unsafe {
            for index in 0..bytes {
                self.reply[index] =
                    ptr::read_volatile(self.arena.add(offset + MANAGEMENT_HEADER_BYTES + index));
            }
        }
        self.reply_bytes = bytes;
        self.reply_word3 = word3;
        self.reply_ready = true;
        Ok(())
    }

    pub(crate) fn poll(&mut self, budget: usize) -> Result<usize> {
        self.ready()?;
        let mut done = 0;
        for number in POLL_ORDER {
            let index = self.queue_index(number)?;
            let profile = self.queues[index].profile;
            let device = self.index(DEVICE_COMPLETIONS, number, profile.completions)?;
            // SAFETY: This cursor publishes completed CQ records and buffers.
            unsafe { centauri_control_dma_rmb() };
            let mut cursor = self.queues[index].cursor;
            while cursor != device && done < budget {
                let offset = self.queues[index].completion + usize::from(cursor) * DESCRIPTOR_BYTES;
                let completion = self.read::<16>(offset)?;
                let flags = le32(&completion, 0);
                let transfer = le32(&completion, 4);
                let cookie = transfer as u16;
                let length = (transfer >> 16) as usize;
                if flags != u32::from(number) << 16 | COMPLETION_KIND
                    || cookie >= profile.submissions
                    || length > profile.capacity
                    || !self.queues[index].owned_by_firmware(cookie)
                {
                    return Err(EIO);
                }
                if number == 2 && length >= MANAGEMENT_HEADER_BYTES {
                    let payload = self.queues[index].pool + usize::from(cookie) * profile.capacity;
                    self.management_reply(payload, length)?;
                }
                if number == 4 && length >= MANAGEMENT_HEADER_BYTES {
                    let payload = self.queues[index].pool + usize::from(cookie) * profile.capacity;
                    if self.scan.is_some() {
                        self.scan_receive(payload, length)?;
                    }
                    self.authentication_receive(payload, length)?;
                }
                self.write(offset, &[0; 16])?;
                self.queues[index].set_owner(cookie, false);
                self.queues[index].active -= 1;
                cursor = (cursor + 1) % profile.completions;
                done += 1;
                if number == 1 {
                    if self.secret_tx_cookie == Some(cookie) {
                        self.wipe_control_slot(cookie)?;
                        self.secret_tx_cookie = None;
                    }
                    self.transmitted += 1;
                } else {
                    self.received += 1;
                }
            }
            if cursor != self.queues[index].cursor {
                self.queues[index].cursor = cursor;
                self.publish(HOST_COMPLETIONS, number, cursor)?;
            }
            self.replenish(index)?;
        }
        self.finish_scan();
        self.finish_connection();
        Ok(done)
    }

    fn scan_receive(&mut self, offset: usize, length: usize) -> Result {
        let header = self.read::<MANAGEMENT_HEADER_BYTES>(offset)?;
        let pending = self.scan.as_mut().ok_or(EIO)?;
        if le32(&header, 20) != 0 || le16(&header, 18) != u16::from(pending.cookie) {
            return Ok(());
        }
        pending.reports = pending.reports.saturating_add(1);
        if usize::from(le16(&header, 16)) != length {
            pending.invalid = pending.invalid.saturating_add(1);
            return Ok(());
        }
        let bytes = length - MANAGEMENT_HEADER_BYTES;
        if bytes > self.scan_buffer.len() {
            return Err(EMSGSIZE);
        }
        // SAFETY: The completed queue4 buffer remains host-owned through
        // callback return. Copy a bounded report; never follow a device pointer.
        unsafe {
            for index in 0..bytes {
                self.scan_buffer[index] =
                    ptr::read_volatile(self.arena.add(offset + MANAGEMENT_HEADER_BYTES + index));
            }
        }
        match scan::frame(&self.scan_buffer[..bytes]) {
            Ok(frame) if !pending.owner.is_null() => unsafe {
                centauri_radio_scan_frame(
                    pending.owner,
                    frame.frequency,
                    frame.signal_dbm,
                    frame.bytes.as_ptr(),
                    frame.bytes.len(),
                );
            },
            Ok(_) => (),
            Err(_) => pending.invalid = pending.invalid.saturating_add(1),
        }
        Ok(())
    }

    fn authentication_receive(&mut self, offset: usize, length: usize) -> Result {
        let watch = self
            .connection
            .as_ref()
            .and_then(|pending| pending.authentication)
            .or(self.sae_watch);
        let Some(watch) = watch else { return Ok(()) };
        let header = self.read::<MANAGEMENT_HEADER_BYTES>(offset)?;
        if usize::from(le16(&header, 16)) != length || le32(&header, 20) != 0 {
            return Ok(());
        }
        let bytes = length - MANAGEMENT_HEADER_BYTES;
        if bytes > self.scan_buffer.len() {
            return Err(EMSGSIZE);
        }
        // SAFETY: This completed queue4 buffer remains owned until recycle.
        // Only the bounded public metadata is retained after parsing.
        unsafe {
            for index in 0..bytes {
                self.scan_buffer[index] =
                    ptr::read_volatile(self.arena.add(offset + MANAGEMENT_HEADER_BYTES + index));
            }
        }
        let metadata = scan::sae_authentication(
            &self.scan_buffer[..bytes],
            &self.interface_address,
            &watch.bssid,
        );
        self.scan_buffer[..bytes].fill(0);
        if let Ok(authentication) = metadata {
            if let (1, Some(group)) = (authentication.transaction, authentication.group) {
                let h2e = authentication.status == 126;
                kernel::pr_info!(
                    "Wi-Fi SAE commit H2E={} group={} required={}\n",
                    h2e,
                    group,
                    watch.h2e_only
                );
                if watch.h2e_only && !h2e {
                    kernel::pr_err!("Wi-Fi SAE H2E-only policy violation\n");
                    return Err(EPROTO);
                }
            }
        }
        Ok(())
    }

    fn finish_scan(&mut self) {
        let Some(pending) = &self.scan else { return };
        let Some(complete) = pending.complete else {
            return;
        };
        // Queue2's completion can precede queue4 service. Do not discard the
        // final batch just because the control event was drained first.
        if complete.status == 0 && pending.reports < complete.frames {
            return;
        }
        let pending = self.scan.take().unwrap();
        let aborted = complete.status != 0 || pending.reports != complete.frames;
        kernel::pr_info!(
            "Wi-Fi scan cookie={} status={} reports={}/{} invalid={} elapsed_ms={}\n",
            pending.cookie,
            complete.status,
            pending.reports,
            complete.frames,
            pending.invalid,
            complete.elapsed_ms
        );
        // SAFETY: C retains the owner and cfg80211 request through completion.
        if !pending.owner.is_null() {
            unsafe { centauri_radio_scan_done(pending.owner, aborted) };
        }
    }

    pub(crate) fn start_scan(
        &mut self,
        owner: *mut c_void,
        cookie: u8,
        channels: &[u32],
    ) -> Result {
        if !self.interface_active || self.scan.is_some() || self.connection.is_some() {
            return Err(EBUSY);
        }
        let mut body = [0; scan::MAX_REQUEST];
        let bytes = scan::request(cookie, channels, &mut body)?;
        self.scan = Some(Scan {
            owner,
            cookie,
            reports: 0,
            invalid: 0,
            complete: None,
        });
        if let Err(error) = self.query(scan::START, &body[..bytes], scan::START_REPLY) {
            self.scan = None;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn abort_scan(&mut self) {
        // Firmware cancellation has not been qualified. C keeps this scan
        // fenced after timeout until its real completion or full teardown.
        if let Some(pending) = &mut self.scan {
            pending.owner = ptr::null_mut();
        }
    }

    fn wipe_control_slot(&self, cookie: u16) -> Result {
        let queue = &self.queues[self.queue_index(1)?];
        let offset = queue.pool + usize::from(cookie) * queue.profile.capacity;
        // SAFETY: Caller owns this returned CQ cookie, or has stopped all
        // DMA before destruction. The whole slot fits the validated pool.
        unsafe {
            for index in 0..queue.profile.capacity {
                ptr::write_volatile(self.arena.add(offset + index), 0);
            }
        }
        Ok(())
    }

    pub(crate) fn start_connection(
        &mut self,
        owner: *mut c_void,
        input: &connection::Parameters,
    ) -> Result {
        self.poll(128)?;
        if !self.interface_active
            || self.scan.is_some()
            || self.connection.is_some()
            || self.associated
            || self.disconnecting.is_some()
            || self.secret_tx_cookie.is_some()
        {
            return Err(EBUSY);
        }
        let request = connection::Request::new(input)?;
        self.connection = Some(Connection {
            owner,
            transaction: self.next_transaction,
            ssid: input.ssid,
            ssid_bytes: input.ssid_bytes as usize,
            bssid: input.bssid,
            outcome: None,
            authentication: (input.security == connection::SECURITY_SAE).then_some(SaeWatch {
                bssid: input.bssid,
                h2e_only: input.h2e_only != 0,
            }),
        });
        if let Err(error) = self.query(connection::START, request.bytes(), connection::START_REPLY)
        {
            self.connection = None;
            return Err(error);
        }
        self.finish_connection();
        Ok(())
    }

    fn finish_connection(&mut self) {
        // A result can precede the ack in the same batch. Publish it only
        // after the solicited command also completed successfully.
        if self.awaiting.is_some() {
            return;
        }
        let Some(pending) = &self.connection else {
            return;
        };
        let Some(status) = pending.outcome else {
            return;
        };
        let pending = self.connection.take().unwrap();
        self.associated = status == 0;
        self.sae_watch = if status == 0 {
            pending.authentication
        } else {
            None
        };
        kernel::pr_info!(
            "Wi-Fi connection outcome status={} target readback verified={}\n",
            status,
            (status == 0)
        );
        if !pending.owner.is_null() {
            unsafe { centauri_radio_connect_done(pending.owner, pending.bssid.as_ptr(), status) };
        }
    }

    pub(crate) fn detach_connection(&mut self) {
        self.sae_watch = None;
        if let Some(pending) = &mut self.connection {
            pending.owner = ptr::null_mut();
        }
    }

    fn send_control(&mut self, command: u32, body: &[u8], reply_command: u32) -> Result {
        if self.awaiting.is_some() {
            return Err(EBUSY);
        }
        self.ready()?;
        let index = self.queue_index(1)?;
        let queue = &self.queues[index];
        let profile = queue.profile;
        let slot = queue.producer;
        let consumed = self.index(DEVICE_SUBMISSIONS, 1, profile.submissions)?;
        let length = body
            .len()
            .checked_add(MANAGEMENT_HEADER_BYTES)
            .ok_or(EMSGSIZE)?;
        if length > profile.capacity {
            return Err(EMSGSIZE);
        }
        if (slot + 1) % profile.submissions == consumed || queue.owned_by_firmware(slot) {
            return Err(EAGAIN);
        }
        let offset = queue.pool + usize::from(slot) * profile.capacity;
        let mut header = [0; MANAGEMENT_HEADER_BYTES];
        header[16..18].copy_from_slice(&(length as u16).to_le_bytes());
        header[18..20].copy_from_slice(&self.next_transaction.to_le_bytes());
        put32(&mut header, 20, command);
        self.write(offset, &header)?;
        self.write(offset + MANAGEMENT_HEADER_BYTES, body)?;
        let mut descriptor = [0; DESCRIPTOR_BYTES];
        put32(
            &mut descriptor,
            0,
            (length as u32) << 8 | EXTERNAL_DESCRIPTOR,
        );
        put64(&mut descriptor, 4, self.dma + offset as u64);
        put32(&mut descriptor, 12, u32::from(slot));
        self.write(
            queue.submission + usize::from(slot) * DESCRIPTOR_BYTES,
            &descriptor,
        )?;
        self.awaiting = Some(Awaiting {
            transaction: self.next_transaction,
            command,
            reply_command,
        });
        if command == connection::START {
            self.secret_tx_cookie = Some(slot);
        }
        self.next_transaction = self.next_transaction.wrapping_add(1);
        self.reply_bytes = 0;
        self.reply_word3 = 0;
        self.reply_ready = false;
        let queue = &mut self.queues[index];
        queue.set_owner(slot, true);
        queue.active += 1;
        queue.producer = (slot + 1) % profile.submissions;
        let producer = queue.producer;
        self.publish(HOST_SUBMISSIONS, 1, producer)
    }

    pub(crate) fn query(&mut self, command: u32, body: &[u8], reply_command: u32) -> Result<&[u8]> {
        self.poll(128)?;
        self.send_control(command, body, reply_command)?;
        for _ in 0..WAIT_ATTEMPTS {
            self.poll(128)?;
            if self.reply_ready {
                self.awaiting = None;
                // Word3 is command-specific. Inactive interface replies can
                // carry nonzero values with a valid state0 body (172/246).
                // Only explicit asynchronous reply aliases have a qualified
                // nonzero-error contract; normal callers verify their bodies.
                if self.reply_word3 != 0 {
                    kernel::pr_info!(
                        "Wi-Fi reply command={:#x} word3={:#x}\n",
                        command,
                        self.reply_word3
                    );
                    if reply_command != command {
                        return Err(EIO);
                    }
                }
                return Ok(&self.reply[..self.reply_bytes]);
            }
            // SAFETY: Serialized sleepable request; C retains all resources.
            unsafe { centauri_boot_sleep() };
        }
        // Retain the outstanding request on timeout. A caller must not reuse
        // its transaction/response storage until recovery or device teardown.
        Err(ETIMEDOUT)
    }

    fn probe_info(&mut self) -> Result {
        let body = self.query(QUERY_INITIAL_PARAMETERS, &[], QUERY_INITIAL_PARAMETERS)?;
        let bytes = body.len();
        let mut offset = 0;
        let mut records = 0;
        while offset < bytes {
            if bytes - offset < 4 {
                return Err(EIO);
            }
            let length = usize::from(le16(body, offset + 2));
            offset = offset.checked_add(4 + length).ok_or(EIO)?;
            if offset > bytes {
                return Err(EIO);
            }
            records += 1;
        }
        let body = self.query(QUERY_COUNTRY, &[], QUERY_COUNTRY)?;
        if body.len() != 3 {
            return Err(EIO);
        }
        let country = [body[0], body[1]];
        let body = self.query(QUERY_CHANNELS, &[1], QUERY_CHANNELS)?;
        if body.is_empty() || 1 + usize::from(body[0]) * 4 > body.len() {
            return Err(EIO);
        }
        kernel::pr_info!("Wi-Fi basic queues ready: parameter fields={}, country bytes={:02x}/{:02x}, channels={}\n",
            records, country[0], country[1], body[0]);
        Ok(())
    }

    pub(crate) fn start_interface(&mut self, address: &[u8; 6]) -> Result {
        if address.iter().all(|byte| *byte == 0) || address[0] & 1 != 0 {
            return Err(EINVAL);
        }
        self.disconnect()?;
        // Follow the independent175 sequence: stop, verify inactive, set the
        // next interface's address, activate with the qualified zero options.
        self.query(
            DEACTIVATE_INTERFACE,
            &INTERFACE_OPERATION,
            DEACTIVATE_INTERFACE,
        )?;
        let info = self.query(QUERY_INTERFACE_INFO, &[], QUERY_INTERFACE_INFO)?;
        if info.len() < 8 || info[1] != INTERFACE_INACTIVE {
            return Err(EIO);
        }
        self.interface_active = false;
        self.query(SET_ADDRESS_TEMPLATE, address, SET_ADDRESS_TEMPLATE)?;
        self.query(ACTIVATE_INTERFACE, &INTERFACE_OPERATION, ACTIVATE_INTERFACE)?;
        self.interface_active = true;
        self.interface_address = *address;
        let info = self.query(QUERY_INTERFACE_INFO, &[], QUERY_INTERFACE_INFO)?;
        if info.len() < 8 || info[1] != INTERFACE_ACTIVE || info[2..8] != address[..] {
            return Err(EIO);
        }
        kernel::pr_info!("Wi-Fi firmware interface active; address readback verified\n");
        Ok(())
    }

    pub(crate) fn disconnect(&mut self) -> Result {
        if !self.associated && self.connection.is_none() && self.disconnecting.is_none() {
            return Ok(());
        }
        self.detach_connection();
        if self.disconnecting.is_none() {
            self.disconnecting = Some((self.next_transaction, false));
            if let Err(error) = self.query(
                connection::DISCONNECT,
                &connection::DISCONNECT_BODY,
                connection::DISCONNECT_REPLY,
            ) {
                // A timed-out solicited request remains fenced by awaiting.
                if self.awaiting.is_none() {
                    self.disconnecting = None;
                }
                return Err(error);
            }
        }
        for _ in 0..WAIT_ATTEMPTS {
            self.poll(128)?;
            if self.disconnecting.is_some_and(|(_, complete)| complete) {
                self.associated = false;
                self.sae_watch = None;
                self.connection = None;
                self.disconnecting = None;
                kernel::pr_info!("Wi-Fi disconnection completion verified\n");
                return Ok(());
            }
            unsafe { centauri_boot_sleep() };
        }
        Err(ETIMEDOUT)
    }

    pub(crate) fn stop_interface(&mut self) -> Result {
        if !self.interface_active {
            return Ok(());
        }
        self.disconnect()?;
        self.query(
            DEACTIVATE_INTERFACE,
            &INTERFACE_OPERATION,
            DEACTIVATE_INTERFACE,
        )?;
        let info = self.query(QUERY_INTERFACE_INFO, &[], QUERY_INTERFACE_INFO)?;
        if info.len() < 8 || info[1] != INTERFACE_INACTIVE {
            return Err(EIO);
        }
        self.interface_active = false;
        self.connection = None;
        kernel::pr_info!("Wi-Fi firmware interface inactive\n");
        Ok(())
    }
}

/// # Safety
/// C retains the successful cold startup's enabled BAR and coherent buffers,
/// and exclusively serializes all later state calls until destroy.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_queues_create(
    bar: *mut c_void,
    arena: *mut u8,
    dma: u64,
    bytes: usize,
    output: *mut *mut c_void,
) -> i32 {
    if output.is_null() {
        return EINVAL.to_errno();
    }
    // SAFETY: Caller supplies the unique output slot, including on failure.
    unsafe { output.write(ptr::null_mut()) };
    let create = || -> Result<KBox<WifiQueues>> {
        // SAFETY: C retains the successful cold startup's live resources.
        let initial = unsafe { WifiQueues::new(bar, arena, dma, bytes) }?;
        let mut state = KBox::new(initial, GFP_KERNEL)?;
        state.configure()?;
        state.probe_info()?;
        Ok(state)
    };
    match create() {
        Ok(state) => {
            unsafe { output.write(KBox::into_raw(state).cast()) };
            0
        }
        Err(error) => {
            kernel::pr_err!(
                "Wi-Fi basic queue initialization failed: {}\n",
                error.to_errno()
            );
            error.to_errno()
        }
    }
}

/// # Safety
/// State is the live create result; C serializes service and management calls.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_queues_poll(state: *mut c_void, budget: u32) -> i32 {
    let state = unsafe { &mut *state.cast::<WifiQueues>() };
    state
        .poll((budget as usize).min(128))
        .map_or_else(|e| e.to_errno(), |n| n as i32)
}

/// # Safety
/// State is the live create result under C's lock; address points to six
/// immutable bytes for this call. No worker may access the state concurrently.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_interface_start(
    state: *mut c_void,
    address: *const u8,
) -> i32 {
    if address.is_null() {
        return EINVAL.to_errno();
    }
    // SAFETY: C retains and exclusively locks this state and the six bytes.
    let state = unsafe { &mut *state.cast::<WifiQueues>() };
    let address = unsafe { &*address.cast::<[u8; 6]>() };
    state
        .start_interface(address)
        .map_or_else(|e| e.to_errno(), |_| 0)
}

/// # Safety
/// State and DMA/PCI resources remain live and mastering remains enabled.
/// C serializes this final request before stopping DMA and draining work.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_interface_stop(state: *mut c_void) -> i32 {
    // SAFETY: The caller retains the live exclusively locked create result.
    let state = unsafe { &mut *state.cast::<WifiQueues>() };
    state.stop_interface().map_or_else(|e| e.to_errno(), |_| 0)
}

/// # Safety
/// The unique create result is returned once after IRQ/work users are drained.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_queues_destroy(state: *mut c_void) {
    let state = unsafe { KBox::<WifiQueues>::from_raw(state.cast()) };
    kernel::pr_info!(
        "Wi-Fi basic completions: TX={} RX={}\n",
        state.transmitted,
        state.received
    );
    if let Some(cookie) = state.secret_tx_cookie {
        let _ = state.wipe_control_slot(cookie);
    }
    drop(state);
}

/// # Safety
/// C holds the state lock and retains owner plus the channel array. Only
/// bounded 2.4GHz channel words from cfg80211 are accepted.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_scan_start(
    state: *mut c_void,
    owner: *mut c_void,
    cookie: u8,
    channels: *const u32,
    count: usize,
) -> i32 {
    if channels.is_null() || count == 0 || count > scan::MAX_CHANNELS || owner.is_null() {
        return EINVAL.to_errno();
    }
    let state = unsafe { &mut *state.cast::<WifiQueues>() };
    let channels = unsafe { core::slice::from_raw_parts(channels, count) };
    state
        .start_scan(owner, cookie, channels)
        .map_or_else(|e| e.to_errno(), |_| 0)
}

/// # Safety
/// C holds the live state lock before reporting an aborted cfg80211 request.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_scan_detach(state: *mut c_void) {
    unsafe { &mut *state.cast::<WifiQueues>() }.abort_scan();
}

/// # Safety
/// C serializes this management command with all queue state access.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_set_country(
    state: *mut c_void,
    country: *const u8,
) -> i32 {
    if country.is_null() {
        return EINVAL.to_errno();
    }
    let country = unsafe { &*country.cast::<[u8; 2]>() };
    if country.iter().any(|byte| !byte.is_ascii_uppercase()) {
        return EINVAL.to_errno();
    }
    let state = unsafe { &mut *state.cast::<WifiQueues>() };
    // The trailing byte1 is a qualified profile value, individually unassigned.
    let body = [country[0], country[1], 1];
    match state.query(9, &body, 9) {
        Err(error) => error.to_errno(),
        Ok(_) => match state.query(QUERY_COUNTRY, &[], QUERY_COUNTRY) {
            Ok(reply) if reply == body => 0,
            Ok(_) => EIO.to_errno(),
            Err(error) => error.to_errno(),
        },
    }
}

/// # Safety
/// C retains the owner and immutable credential-bearing parameter record under its state
/// lock for this call, and retains the callback owner until completion/detach.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_connect_start(
    state: *mut c_void,
    owner: *mut c_void,
    input: *const connection::Parameters,
) -> i32 {
    if owner.is_null() || input.is_null() {
        return EINVAL.to_errno();
    }
    let state = unsafe { &mut *state.cast::<WifiQueues>() };
    let input = unsafe { &*input };
    state
        .start_connection(owner, input)
        .map_or_else(|e| e.to_errno(), |_| 0)
}

/// # Safety
/// C holds the live state lock before retiring a cfg80211 connection request.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_connect_detach(state: *mut c_void) {
    unsafe { &mut *state.cast::<WifiQueues>() }.detach_connection();
}
