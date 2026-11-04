// SPDX-License-Identifier: GPL-2.0-only
//! Port of the Python-tested one-page Bluetooth transport (see bluetooth-protocol.rst).
//! Cold Linux-owned DMA/context startup; opaque shared firmware comes from PF0.
//! No vendor event body inspection, auxiliary queues or firmware parsing occurs here.
use super::centauri_bluetooth_cold::Owned;
use super::{le16, le32, le64, Buffer};
use core::{ffi::c_void, ptr, slice};
use kernel::prelude::*;

unsafe extern "C" {
    fn centauri_bt_queue_rx(glue: *mut c_void, packet_type: u8, frame: *const u8, length: usize);
    fn centauri_bt_now_ms() -> u64;
    fn centauri_boot_sleep();
}

const BT_DOORBELLS: usize = 0x1000;
const INLINE_HEADER: u64 = 16;
const INLINE_ADDRESS: u64 = u64::MAX;
const INLINE_TX: u32 = 2;
const HCI_COMMAND: u8 = 1;
const HCI_ACL: u8 = 2;
const HCI_EVENT: u8 = 4;
const HCI_RESET: u16 = 0x0c03;
const HCI_CLASSIC_CREATE_INHIBIT: u16 = 0xff77;
const HCI_COMMAND_COMPLETE: u8 = 0x0e;
const HCI_COMMAND_STATUS: u8 = 0x0f;
const HCI_VENDOR_EVENT: u8 = 0xff;
const HCI_CONNECTION_COMPLETE: u8 = 3;
const HCI_LINK_ACL: u8 = 1;
const N1_LINK_ACL_F0: u8 = 0xf0;
const N1_LINK_ACL_F1: u8 = 0xf1;
const COMMAND_QUEUE: u16 = 1;
const ACL_TX_QUEUE: u16 = 2;
const EVENT_QUEUE: u16 = 7;
const ACL_RX_QUEUE: u16 = 8;
// Standard HCI header plus the maximum255-byte parameter field, padded to u32.
const COMMAND_CAPACITY: usize = (3 + 255 + 3) & !3;
// Host allocation choices: one reusable admin frame ends below0x1000;
// each subsequent two-slot ring is placed at the next64-byte boundary.
const COMMAND_RING_OFFSET: u64 = 0x1000;
const EVENT_RING_OFFSET: u64 =
    (COMMAND_RING_OFFSET + 2 * (INLINE_HEADER + COMMAND_CAPACITY as u64) + 63) & !63;
const ACL_TX_RING_OFFSET: u64 =
    (EVENT_RING_OFFSET + 2 * (INLINE_HEADER + COMMAND_CAPACITY as u64) + 63) & !63;
const HCI_SET_FACTORY_ADDRESS: u16 = 0xfc06;
const HCI_RADIO_INITIALIZE: u16 = 0xff0b;
const HCI_READ_BUFFER_SIZE: u16 = 0x1005;
const SETUP_TIMEOUT_MS: u64 = 3000;

fn now_ms() -> u64 {
    // SAFETY: Generic monotonic clock helper; no device or shared state access.
    unsafe { centauri_bt_now_ms() }
}

#[derive(Clone, Copy)]
struct InlineQueue {
    number: u16,
    ring: u64,
    capacity: usize,
    cursor: u16,
}
impl InlineQueue {
    fn slot(&self, slot: u16) -> u64 {
        self.ring + u64::from(slot) * (INLINE_HEADER + self.capacity as u64)
    }
    // Exactly two slots, reserving one credit to distinguish empty from full.
    fn next(&self, slot: u16) -> u16 {
        (slot + 1) % 2
    }
}
struct ResetSetup {
    reply: [u8; 6],
    sent: bool,
    started: u64,
}
struct Bluetooth {
    glue: *mut c_void,
    memory: Owned,
    initializing: bool,
    bootstrap_mtu: Option<u16>,
    device_indices: u64,
    host_indices: u64,
    queues: KVec<InlineQueue>,
    reset_setup: Option<ResetSetup>,
    commands: u64,
    acl_tx: u64,
    events: u64,
    acl_rx: u64,
    skipped_vendor: u64,
}
impl Bluetooth {
    unsafe fn create(
        glue: *mut c_void,
        bar: *mut c_void,
        cpu: *mut u8,
        dma: u64,
        bytes: usize,
        resource_dma: u64,
        resource_bytes: usize,
    ) -> Result<Self> {
        let memory = unsafe { Owned::new(bar, cpu, dma, bytes) }?;
        memory.start(glue, resource_dma, resource_bytes)?;
        let mut queues = KVec::with_capacity(4, GFP_KERNEL)?;
        for (number, offset) in [
            (COMMAND_QUEUE, COMMAND_RING_OFFSET),
            (EVENT_QUEUE, EVENT_RING_OFFSET),
        ] {
            queues.push(
                InlineQueue {
                    number,
                    ring: dma + offset,
                    capacity: COMMAND_CAPACITY,
                    cursor: 0,
                },
                GFP_KERNEL,
            )?;
        }
        let mut state = Self {
            glue,
            memory,
            initializing: true,
            bootstrap_mtu: None,
            device_indices: dma + 0x7a,
            host_indices: dma + 0xa8,
            queues,
            reset_setup: None,
            commands: 0,
            acl_tx: 0,
            events: 0,
            acl_rx: 0,
            skipped_vendor: 0,
        };
        for i in 0..2 {
            let queue = state.queues[i];
            state
                .memory
                .register(queue.number, (queue.ring - dma) as usize, queue.capacity)?;
            if queue.number == EVENT_QUEUE {
                state.post_rx(&queue, 0)?;
                state.publish(&queue, 1)?;
            }
        }
        state.submit(COMMAND_QUEUE, &[0x05, 0x10, 0])?;
        for _ in 0..600 {
            state.poll(64)?;
            if state.bootstrap_mtu.is_some() {
                break;
            }
            unsafe { centauri_boot_sleep() };
        }
        let mtu = usize::from(state.bootstrap_mtu.ok_or(ETIMEDOUT)?);
        if !(1..=4092).contains(&mtu) {
            return Err(EIO);
        }
        let capacity = (mtu + 4 + 3) & !3;
        let rx_offset = (ACL_TX_RING_OFFSET as usize + 2 * (16 + capacity) + 63) & !63;
        for (number, offset) in [
            (ACL_TX_QUEUE, ACL_TX_RING_OFFSET as usize),
            (ACL_RX_QUEUE, rx_offset),
        ] {
            state.memory.register(number, offset, capacity)?;
            let queue = InlineQueue {
                number,
                ring: dma + offset as u64,
                capacity,
                cursor: 0,
            };
            if number == ACL_RX_QUEUE {
                state.post_rx(&queue, 0)?;
                state.publish(&queue, 1)?;
            }
            state.queues.push(queue, GFP_KERNEL)?;
        }
        state
            .memory
            .publish_resource(glue, resource_dma, resource_bytes)?;
        state.initializing = false;
        pr_info!("Bluetooth generated four two-slot rings; ACL MTU={}\n", mtu);
        Ok(state)
    }
    fn ready(&self) -> Result {
        self.memory.ready()
    }
    fn index(&self, vector: u64, number: u16) -> Result<u16> {
        let index = self.memory.index(vector, 0, number)?;
        if index >= 2 {
            return Err(EIO);
        }
        Ok(index)
    }
    fn publish(&self, queue: &InlineQueue, producer: u16) -> Result {
        self.memory
            .set_index(self.host_indices, 0, queue.number, producer)?;
        self.memory.write_register(
            BT_DOORBELLS + queue.number as usize * 4,
            u32::from(producer),
        );
        Ok(())
    }
    fn post_rx(&self, queue: &InlineQueue, slot: u16) -> Result {
        self.memory.write(
            queue.slot(slot),
            &mut Buffer {
                flags: (queue.capacity as u32) << 8,
                address: INLINE_ADDRESS,
                tag: u32::from(slot),
            }
            .bytes(),
        )
    }
    fn submit(&mut self, number: u16, frame: &[u8]) -> Result {
        let queue = self
            .queues
            .iter()
            .find(|q| q.number == number)
            .ok_or(EINVAL)?;
        if frame.is_empty() || frame.len() > queue.capacity {
            return Err(EMSGSIZE);
        }
        let posted = self.index(self.host_indices, number)?;
        let consumed = self.index(self.device_indices, number)?;
        if queue.next(posted) == consumed {
            return Err(EAGAIN);
        }
        self.memory.write(
            queue.slot(posted),
            &mut Buffer {
                flags: (frame.len() as u32) << 8 | INLINE_TX,
                address: INLINE_ADDRESS,
                tag: u32::from(posted),
            }
            .bytes(),
        )?;
        self.memory
            .write(queue.slot(posted) + INLINE_HEADER, frame)?;
        self.publish(queue, queue.next(posted))?;
        if number == COMMAND_QUEUE {
            self.commands += 1;
        } else {
            self.acl_tx += 1;
        }
        Ok(())
    }
    fn send(&mut self, packet_type: u8, frame: &[u8], setup: bool) -> Result {
        self.ready()?;
        if self.reset_setup.is_some() {
            return Err(EAGAIN);
        }
        let number = match packet_type {
            HCI_COMMAND => {
                if frame.len() < 3 || usize::from(frame[2]) + 3 != frame.len() {
                    return Err(EINVAL);
                }
                // Only the driver's setup callback may send these own-device operations.
                let opcode = le16(frame, 0);
                if opcode >> 10 == 0x3f
                    && (!setup
                        || !matches!(
                            (opcode, frame.len()),
                            (HCI_SET_FACTORY_ADDRESS, 9) | (HCI_RADIO_INITIALIZE, 3)
                        ))
                {
                    return Err(EOPNOTSUPP);
                }
                COMMAND_QUEUE
            }
            HCI_ACL => {
                if frame.len() < 4 || usize::from(le16(frame, 2)) + 4 != frame.len() {
                    return Err(EINVAL);
                }
                ACL_TX_QUEUE
            }
            _ => return Err(EOPNOTSUPP),
        };
        self.submit(number, frame)
    }
    fn setup_submit(&mut self) -> Result {
        let Some(setup) = &self.reset_setup else {
            return Ok(());
        };
        if now_ms().saturating_sub(setup.started) > SETUP_TIMEOUT_MS {
            return Err(ETIMEDOUT);
        }
        if setup.sent {
            return Ok(());
        }
        let [low, high] = HCI_CLASSIC_CREATE_INHIBIT.to_le_bytes();
        match self.submit(COMMAND_QUEUE, &[low, high, 1, 0]) {
            Ok(()) => {
                self.reset_setup.as_mut().ok_or(EIO)?.sent = true;
                Ok(())
            }
            Err(error) if error == EAGAIN => Ok(()),
            Err(error) => Err(error),
        }
    }
    fn receive(&mut self, kind: u8, packet: &mut [u8]) -> Result {
        if self.initializing {
            if kind == HCI_EVENT
                && packet[0] == HCI_COMMAND_COMPLETE
                && packet.len() == 13
                && le16(packet, 3) == HCI_READ_BUFFER_SIZE
                && packet[5] == 0
            {
                self.bootstrap_mtu = Some(le16(packet, 6));
                return Ok(());
            }
            if kind == HCI_EVENT
                && packet[0] == HCI_COMMAND_STATUS
                && packet.len() == 6
                && le16(packet, 4) == HCI_READ_BUFFER_SIZE
                && packet[2] == 0
            {
                return Ok(());
            }
            return Err(EIO);
        }
        if kind == HCI_EVENT {
            if packet[0] == HCI_COMMAND_COMPLETE && packet.len() >= 6 {
                let opcode = le16(packet, 3);
                if opcode == HCI_RESET && packet[5] == 0 {
                    if packet.len() != 6 || self.reset_setup.is_some() {
                        return Err(EIO);
                    }
                    self.reset_setup = Some(ResetSetup {
                        reply: packet.try_into().map_err(|_| EINVAL)?,
                        sent: false,
                        started: now_ms(),
                    });
                    return Ok(());
                }
                if opcode == HCI_CLASSIC_CREATE_INHIBIT && self.reset_setup.is_some() {
                    let setup = self.reset_setup.take().ok_or(EIO)?;
                    if !setup.sent || packet[5] != 0 {
                        return Err(EIO);
                    }
                    // SAFETY: Copies the original Reset Complete synchronously.
                    unsafe {
                        centauri_bt_queue_rx(
                            self.glue,
                            HCI_EVENT,
                            setup.reply.as_ptr(),
                            setup.reply.len(),
                        )
                    };
                    return Ok(());
                }
            }
            if packet[0] == HCI_COMMAND_STATUS
                && packet.len() == 6
                && le16(packet, 4) == HCI_CLASSIC_CREATE_INHIBIT
                && self.reset_setup.is_some()
            {
                if packet[2] != 0 || !self.reset_setup.as_ref().ok_or(EIO)?.sent {
                    return Err(EIO);
                }
                return Ok(());
            }
            if packet[0] == HCI_CONNECTION_COMPLETE && packet.len() == 13 && packet[2] == 0 {
                // Both codes were tested as ACL; neither encodes direction/role.
                match packet[11] {
                    N1_LINK_ACL_F0 | N1_LINK_ACL_F1 => packet[11] = HCI_LINK_ACL,
                    0..=2 => (),
                    _ => return Err(EIO),
                }
            }
        }
        // SAFETY: C synchronously copies a validated standard HCI/ACL frame.
        // No key, address, or packet contents are logged by either component.
        unsafe { centauri_bt_queue_rx(self.glue, kind, packet.as_ptr(), packet.len()) };
        Ok(())
    }
    fn poll(&mut self, budget: usize) -> Result<usize> {
        self.ready()?;
        self.setup_submit()?;
        let mut done = 0;
        for index in 0..self.queues.len() {
            let queue = self.queues[index];
            let kind = match queue.number {
                EVENT_QUEUE => HCI_EVENT,
                ACL_RX_QUEUE => HCI_ACL,
                _ => continue,
            };
            let consumer = self.index(self.device_indices, queue.number)?;
            let mut posted = self.index(self.host_indices, queue.number)?;
            let mut cursor = queue.cursor;
            while cursor != consumer && done < budget {
                let header = self.memory.read::<16>(queue.slot(cursor))?;
                let length = (le32(&header, 0) >> 8) as usize;
                if length < 2 || length > queue.capacity || le64(&header, 4) != INLINE_ADDRESS {
                    return Err(EIO);
                }
                let address = queue.slot(cursor) + INLINE_HEADER;
                let prefix = self.memory.read::<2>(address)?;
                if kind == HCI_EVENT && prefix[0] == HCI_VENDOR_EVENT {
                    // Leave every vendor event body byte unread. Only its
                    // standard two-byte event header was needed to identify it.
                    self.skipped_vendor += 1;
                } else {
                    let mut packet = KVec::from_elem(0, queue.capacity, GFP_KERNEL)?;
                    if kind == HCI_EVENT {
                        if usize::from(prefix[1]) + 2 != length {
                            return Err(EIO);
                        }
                        self.events += 1;
                    } else {
                        if length < 4 {
                            return Err(EIO);
                        }
                        let acl_header = self.memory.read::<4>(address)?;
                        if usize::from(le16(&acl_header, 2)) + 4 != length {
                            return Err(EIO);
                        }
                        self.acl_rx += 1;
                    }
                    self.memory.copy(address, &mut packet[..length], false)?;
                    self.receive(kind, &mut packet[..length])?;
                }
                if queue.next(posted) == consumer {
                    return Err(EIO);
                }
                self.post_rx(&queue, posted)?;
                posted = queue.next(posted);
                cursor = queue.next(cursor);
                done += 1;
            }
            if cursor != queue.cursor {
                self.publish(&queue, posted)?;
            }
            self.queues[index].cursor = cursor;
        }
        self.setup_submit()?;
        Ok(done)
    }
}

/// # Safety
/// C owns its coherent allocation, live PF2 BAR and writable output pointer.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_bt_rust_create(
    glue: *mut c_void,
    bar: *mut c_void,
    cpu: *mut u8,
    dma: u64,
    bytes: usize,
    resource_dma: u64,
    resource_bytes: usize,
    output: *mut *mut c_void,
) -> i32 {
    if output.is_null() {
        return EINVAL.to_errno();
    }
    unsafe { output.write(ptr::null_mut()) };
    let result =
        unsafe { Bluetooth::create(glue, bar, cpu, dma, bytes, resource_dma, resource_bytes) }
            .and_then(|state| KBox::new(state, GFP_KERNEL).map_err(Error::from));
    match result {
        Ok(state) => {
            unsafe { output.write(KBox::into_raw(state).cast()) };
            0
        }
        Err(error) => {
            pr_err!(
                "Bluetooth cold initialization failed: {}\n",
                error.to_errno()
            );
            error.to_errno()
        }
    }
}
/// # Safety
/// State is the unique create result, after every HCI callback has stopped.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_bt_rust_destroy(state: *mut c_void) {
    // SAFETY: C returns unique ownership exactly once after draining callbacks.
    let state = unsafe { KBox::<Bluetooth>::from_raw(state.cast()) };
    pr_info!(
        "Rust Bluetooth commands={} ACL-TX={} events={} ACL-RX={} vendor-skipped={}\n",
        state.commands,
        state.acl_tx,
        state.events,
        state.acl_rx,
        state.skipped_vendor
    );
    drop(state);
}
/// # Safety
/// State is exclusively locked; frame is a live linear skb of this length.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_bt_rust_send(
    state: *mut c_void,
    packet_type: u8,
    frame: *const u8,
    length: usize,
    setup: bool,
) -> i32 {
    // SAFETY: C holds the protocol lock and supplies the valid skb slice.
    let state = unsafe { &mut *state.cast::<Bluetooth>() };
    // SAFETY: Same exclusive callback and skb guarantees.
    let frame = unsafe { slice::from_raw_parts(frame, length) };
    state
        .send(packet_type, frame, setup)
        .map_or_else(|e| e.to_errno(), |_| 0)
}
/// # Safety
/// State is live and exclusively locked throughout this callback.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_bt_rust_poll(state: *mut c_void, budget: i32) -> i32 {
    // SAFETY: C serializes every protocol callback.
    let state = unsafe { &mut *state.cast::<Bluetooth>() };
    state
        .poll(budget.max(0) as usize)
        .map_or_else(|e| e.to_errno(), |n| n as i32)
}
