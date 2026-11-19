// SPDX-License-Identifier: GPL-2.0-only

//! Fresh Ethernet queues, ported from the independently qualified Python path.
//! C retains the coherent allocation and serializes these methods with setup.

use super::centauri_radio::{
    DEVICE_COMPLETIONS, DEVICE_SUBMISSIONS, HOST_COMPLETIONS, HOST_SUBMISSIONS, WIFI_ARENA_BYTES,
    WIFI_POOL_BYTES,
};
use super::centauri_wifi_queues::WifiQueues;
use core::{ffi::c_void, ptr};
use kernel::prelude::*;

const SOURCE_ENTRIES: u16 = 2048;
const TX_ENTRIES: u16 = 8192;
const RX_ENTRIES: u16 = 4096;
const TX_SOURCE: u16 = 25;
const TX_COMPLETION: u16 = 76;
const TX_CURSOR: u16 = 192;
const RX_SOURCE: u16 = 12;
const RX_COMPLETION: u16 = 15;
const RX_CURSOR: u16 = 207;
const COMPLETION_BYTES: usize = 32;
const TX_DESCRIPTOR_BYTES: usize = 32;
const RX_DESCRIPTOR_BYTES: usize = 16;
const CAPACITY: usize = 2048;
const RX_PADDING: usize = 2;
const TX_SLOTS: usize = 64;
const TX_STRIDE: usize = 4096;
const DMA_ALIGNMENT: usize = 0x4000;
const TX_CQ: usize = 0;
const TX_SQ: usize = TX_CQ + TX_ENTRIES as usize * COMPLETION_BYTES;
const RX_CQ: usize = TX_SQ + SOURCE_ENTRIES as usize * TX_DESCRIPTOR_BYTES;
const RX_SQ: usize = RX_CQ + RX_ENTRIES as usize * COMPLETION_BYTES;
const RX_POOL: usize = RX_SQ + SOURCE_ENTRIES as usize * RX_DESCRIPTOR_BYTES;
const TX_POOL: usize = RX_POOL + SOURCE_ENTRIES as usize * CAPACITY;
pub(crate) const DATA_BYTES: usize = TX_POOL + TX_SLOTS * TX_STRIDE;
const TX_NOTIFY: usize = 0x1030;
const RX_NOTIFY: usize = 0x1050;
const EXTENDED_FORMAT: u32 = 4 << 16;
const TX_CQ_KIND: u16 = 0x502;
const RX_CQ_KIND: u16 = 0x403;
// The functional effect of bit17 is qualified by alternating ARP probes187.
// Its individual meaning remains unassigned; zero prevents matching ARP RX.
const TX_PROFILE_BIT17: u32 = 1 << 17;

unsafe extern "C" {
    fn centauri_boot_read(bar: *mut c_void, offset: usize) -> u32;
    fn centauri_boot_write(bar: *mut c_void, offset: usize, value: u32);
    fn centauri_control_dma_wmb();
    fn centauri_control_dma_rmb();
    fn centauri_radio_receive(owner: *mut c_void, frame: *const u8, bytes: usize);
}
fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn cq_frame(
    queue: u16,
    cursor: u16,
    dma: u64,
    entries: u16,
    source: u16,
    profile: u32,
    service: u32,
) -> [u8; 54] {
    let mut frame = [0; 54];
    put32(&mut frame, 0, EXTENDED_FORMAT | 2);
    put32(&mut frame, 4, u32::from(cursor) << 16 | u32::from(queue));
    put64(&mut frame, 8, dma);
    put32(&mut frame, 16, u32::from(source) << 16 | u32::from(entries));
    put32(&mut frame, 20, profile);
    put32(&mut frame, 28, service);
    frame
}
fn sq_frame(queue: u16, dma: u64, receive: bool, completion: u16, service: u32) -> [u8; 62] {
    let mut frame = [0; 62];
    put32(&mut frame, 0, if receive { 1 } else { EXTENDED_FORMAT | 1 });
    put32(&mut frame, 4, u32::from(queue) << 16 | u32::from(queue));
    put64(&mut frame, 8, dma);
    put64(&mut frame, 16, u64::MAX);
    put32(
        &mut frame,
        24,
        u32::from(completion) << 16 | u32::from(SOURCE_ENTRIES),
    );
    // Qualified profiles; remaining individual flag roles are unassigned.
    put32(
        &mut frame,
        28,
        if receive { 0x0228000b } else { 0x00080012 },
    );
    put32(&mut frame, 36, service);
    frame
}

pub(crate) struct DataQueues {
    owner: *mut c_void,
    bar: *mut c_void,
    context: *mut u8,
    context_dma: u64,
    data: *mut u8,
    dma: u64,
    tx_producer: u16,
    tx_cursor: u16,
    tx_pending: Option<u16>,
    rx_producer: u16,
    rx_cursor: u16,
    rx_owned: [u64; 32],
    pending: KVec<u16>,
    pending_head: u16,
    pending_count: u16,
    frame: KVec<u8>,
    transmitted: u64,
    received: u64,
    recycled: u64,
}
impl DataQueues {
    /// # Safety
    /// Both CPU ranges and the BAR remain live until destroy. C serializes
    /// this packet state and basic setup, including the shared context.
    pub(crate) unsafe fn new(
        owner: *mut c_void,
        bar: *mut c_void,
        context: *mut u8,
        context_dma: u64,
        data: *mut u8,
        dma: u64,
        bytes: usize,
    ) -> Result<Self> {
        if owner.is_null()
            || bar.is_null()
            || context.is_null()
            || data.is_null()
            || context as usize & 3 != 0
            || data as usize & 3 != 0
            || bytes != DATA_BYTES
            || dma & (DMA_ALIGNMENT as u64 - 1) != 0
            || context_dma.checked_add((WIFI_ARENA_BYTES + WIFI_POOL_BYTES) as u64) != Some(dma)
            || dma
                .checked_add(bytes as u64)
                .is_none_or(|end| end > 1 << 36)
        {
            return Err(EINVAL);
        }
        let state = Self {
            owner,
            bar,
            context,
            context_dma,
            data,
            dma,
            tx_producer: 0,
            tx_cursor: 0,
            tx_pending: None,
            rx_producer: 0,
            rx_cursor: 0,
            rx_owned: [0; 32],
            pending: KVec::from_elem(0, SOURCE_ENTRIES as usize, GFP_KERNEL)?,
            pending_head: 0,
            pending_count: 0,
            frame: KVec::from_elem(0, CAPACITY, GFP_KERNEL)?,
            transmitted: 0,
            received: 0,
            recycled: 0,
        };
        state.ready()?;
        let window = unsafe {
            u64::from(centauri_boot_read(bar, 0x18))
                | u64::from(centauri_boot_read(bar, 0x1c)) << 32
        };
        let end = window
            .checked_add(unsafe { u64::from(centauri_boot_read(bar, 0x20)) })
            .ok_or(EINVAL)?;
        if window > context_dma || dma + bytes as u64 > end {
            return Err(EINVAL);
        }
        Ok(state)
    }
    fn ready(&self) -> Result {
        unsafe {
            if centauri_boot_read(self.bar, 8) != 2
                || centauri_boot_read(self.bar, 12) != 2
                || (u64::from(centauri_boot_read(self.bar, 0x10))
                    | u64::from(centauri_boot_read(self.bar, 0x14)) << 32)
                    != self.context_dma
            {
                return Err(EIO);
            }
        }
        Ok(())
    }
    fn index(&self, base: usize, number: u16, entries: u16) -> Result<u16> {
        // SAFETY: All private callers use checked queue IDs in the generated
        // context's u16 arrays. CPU base and offsets are naturally aligned.
        let value = unsafe {
            u16::from_le(ptr::read_volatile(
                self.context
                    .add(base + usize::from(number) * 2)
                    .cast::<u16>(),
            ))
        };
        if value >= entries {
            return Err(EIO);
        }
        Ok(value)
    }
    fn publish(&self, base: usize, number: u16, value: u16, notify: Option<usize>) {
        // SAFETY: Host owns these aligned u16 indices; make descriptor/CQ
        // writes visible before the cursor, then before its doorbell.
        unsafe {
            centauri_control_dma_wmb();
            ptr::write_volatile(
                self.context
                    .add(base + usize::from(number) * 2)
                    .cast::<u16>(),
                value.to_le(),
            );
            centauri_control_dma_wmb();
            if let Some(notify) = notify {
                centauri_boot_write(self.bar, notify, 1);
            }
        }
    }
    fn read<const N: usize>(&self, offset: usize) -> Result<[u8; N]> {
        if offset.checked_add(N).is_none_or(|end| end > DATA_BYTES) {
            return Err(EINVAL);
        }
        let mut bytes = [0; N];
        // SAFETY: Completed, bounded coherent DMA range; unaligned fields
        // are copied as bytes without creating exclusive references to DMA.
        unsafe {
            for (i, byte) in bytes.iter_mut().enumerate() {
                *byte = ptr::read_volatile(self.data.add(offset + i));
            }
        }
        Ok(bytes)
    }
    fn write(&self, offset: usize, bytes: &[u8]) -> Result {
        if offset
            .checked_add(bytes.len())
            .is_none_or(|end| end > DATA_BYTES)
        {
            return Err(EINVAL);
        }
        unsafe {
            for (i, byte) in bytes.iter().enumerate() {
                ptr::write_volatile(self.data.add(offset + i), *byte);
            }
        }
        Ok(())
    }
    fn rx_owner(&self, cookie: u16) -> bool {
        self.rx_owned[cookie as usize / 64] & (1 << (cookie % 64)) != 0
    }
    fn set_rx_owner(&mut self, cookie: u16, firmware: bool) {
        let mask = 1 << (cookie % 64);
        let word = &mut self.rx_owned[cookie as usize / 64];
        if firmware {
            *word |= mask;
        } else {
            *word &= !mask;
        }
    }
    fn rx_descriptor(&self, cookie: u16) -> [u8; 16] {
        let mut descriptor = [0; 16];
        put32(&mut descriptor, 0, (CAPACITY as u32) << 8 | 3);
        put64(
            &mut descriptor,
            4,
            self.dma + (RX_POOL + usize::from(cookie) * CAPACITY) as u64,
        );
        put32(&mut descriptor, 12, u32::from(cookie));
        descriptor
    }
    pub(crate) fn configure(&mut self, basic: &mut WifiQueues) -> Result {
        for base in [DEVICE_COMPLETIONS, HOST_COMPLETIONS] {
            if self.index(base, TX_CURSOR, TX_ENTRIES)? != 0
                || self.index(base, RX_CURSOR, RX_ENTRIES)? != 0
            {
                return Err(EBUSY);
            }
        }
        for base in [DEVICE_SUBMISSIONS, HOST_SUBMISSIONS] {
            if self.index(base, TX_SOURCE, SOURCE_ENTRIES)? != 0
                || self.index(base, RX_SOURCE, SOURCE_ENTRIES)? != 0
            {
                return Err(EBUSY);
            }
        }
        // Memory was zeroed before its enlarged window/context publication.
        // Generate all immutable RX allocations; slot and cookie can diverge.
        for cookie in 0..SOURCE_ENTRIES {
            self.write(
                RX_SQ + usize::from(cookie) * RX_DESCRIPTOR_BYTES,
                &self.rx_descriptor(cookie),
            )?;
        }
        basic.admin(&cq_frame(
            TX_COMPLETION,
            TX_CURSOR,
            self.dma + TX_CQ as u64,
            TX_ENTRIES,
            u16::MAX,
            0x12,
            3,
        ))?;
        basic.admin(&sq_frame(
            TX_SOURCE,
            self.dma + TX_SQ as u64,
            false,
            TX_COMPLETION,
            3,
        ))?;
        basic.admin(&cq_frame(
            RX_COMPLETION,
            RX_CURSOR,
            self.dma + RX_CQ as u64,
            RX_ENTRIES,
            RX_SOURCE,
            0xb,
            8,
        ))?;
        basic.admin(&sq_frame(
            RX_SOURCE,
            self.dma + RX_SQ as u64,
            true,
            RX_SOURCE,
            8,
        ))?;
        for cookie in 0..SOURCE_ENTRIES - 1 {
            self.set_rx_owner(cookie, true);
        }
        self.rx_producer = SOURCE_ENTRIES - 1;
        self.publish(
            HOST_SUBMISSIONS,
            RX_SOURCE,
            self.rx_producer,
            Some(RX_NOTIFY),
        );
        kernel::pr_info!("Wi-Fi generated Ethernet queues ready\n");
        Ok(())
    }
    fn replenish(&mut self) -> Result {
        let consumed = self.index(DEVICE_SUBMISSIONS, RX_SOURCE, SOURCE_ENTRIES)?;
        let mut producer = self.rx_producer;
        let mut added = 0;
        while self.pending_count > 0 && (producer + 1) % SOURCE_ENTRIES != consumed {
            let cookie = self.pending[self.pending_head as usize];
            if self.rx_owner(cookie) {
                return Err(EIO);
            }
            self.write(
                RX_SQ + usize::from(producer) * RX_DESCRIPTOR_BYTES,
                &self.rx_descriptor(cookie),
            )?;
            self.set_rx_owner(cookie, true);
            self.pending_head = (self.pending_head + 1) % SOURCE_ENTRIES;
            self.pending_count -= 1;
            producer = (producer + 1) % SOURCE_ENTRIES;
            added += 1;
        }
        if added > 0 {
            self.rx_producer = producer;
            self.recycled += added;
            self.publish(HOST_SUBMISSIONS, RX_SOURCE, producer, Some(RX_NOTIFY));
        }
        Ok(())
    }
    fn poll_tx(&mut self) -> Result<bool> {
        let device = self.index(DEVICE_COMPLETIONS, TX_CURSOR, TX_ENTRIES)?;
        unsafe { centauri_control_dma_rmb() };
        let mut cursor = self.tx_cursor;
        let mut done = false;
        while cursor != device {
            let offset = TX_CQ + usize::from(cursor) * COMPLETION_BYTES;
            let entry = self.read::<32>(offset)?;
            let cookie = le16(&entry, 4);
            if le16(&entry, 0) != TX_CQ_KIND
                || le16(&entry, 2) != TX_SOURCE
                || self.tx_pending != Some(cookie)
            {
                return Err(EIO);
            }
            self.write(offset, &[0; 32])?;
            self.tx_pending = None;
            self.transmitted += 1;
            done = true;
            cursor = (cursor + 1) % TX_ENTRIES;
        }
        if cursor != self.tx_cursor {
            self.tx_cursor = cursor;
            self.publish(HOST_COMPLETIONS, TX_CURSOR, cursor, None);
        }
        Ok(done)
    }
    pub(crate) fn xmit(&mut self, frame: &[u8]) -> Result {
        self.ready()?;
        if !(14..=CAPACITY).contains(&frame.len()) {
            return Err(EMSGSIZE);
        }
        if self.tx_pending.is_some() {
            return Err(EAGAIN);
        }
        let consumed = self.index(DEVICE_SUBMISSIONS, TX_SOURCE, SOURCE_ENTRIES)?;
        if consumed != self.tx_producer {
            return Err(EAGAIN);
        }
        let cookie = self.tx_producer;
        let offset = TX_POOL + usize::from(cookie) % TX_SLOTS * TX_STRIDE;
        self.write(offset, frame)?;
        let mut descriptor = [0; 32];
        put32(&mut descriptor, 0, (frame.len() as u32) << 8 | 3);
        put64(&mut descriptor, 4, self.dma + offset as u64);
        put32(&mut descriptor, 12, u32::from(cookie));
        put32(&mut descriptor, 16, TX_PROFILE_BIT17);
        self.write(
            TX_SQ + usize::from(cookie) * TX_DESCRIPTOR_BYTES,
            &descriptor,
        )?;
        self.tx_pending = Some(cookie);
        self.tx_producer = (cookie + 1) % SOURCE_ENTRIES;
        self.publish(
            HOST_SUBMISSIONS,
            TX_SOURCE,
            self.tx_producer,
            Some(TX_NOTIFY),
        );
        Ok(())
    }
    pub(crate) fn poll(&mut self, budget: usize) -> Result<(usize, bool)> {
        self.ready()?;
        let tx_done = self.poll_tx()?;
        let device = self.index(DEVICE_COMPLETIONS, RX_CURSOR, RX_ENTRIES)?;
        unsafe { centauri_control_dma_rmb() };
        let mut done = 0;
        while self.rx_cursor != device && done < budget {
            let offset = RX_CQ + usize::from(self.rx_cursor) * COMPLETION_BYTES;
            let entry = self.read::<32>(offset)?;
            let cookie = le16(&entry, 4);
            let length = usize::from(le16(&entry, 6));
            if le16(&entry, 0) != RX_CQ_KIND
                || le16(&entry, 2) != RX_SOURCE
                || cookie >= SOURCE_ENTRIES
                || !(RX_PADDING..=CAPACITY).contains(&length)
                || !self.rx_owner(cookie)
                || self.pending_count >= SOURCE_ENTRIES
            {
                return Err(EIO);
            }
            if length >= RX_PADDING + 14 {
                let bytes = length - RX_PADDING;
                let buffer = RX_POOL + usize::from(cookie) * CAPACITY + RX_PADDING;
                unsafe {
                    for i in 0..bytes {
                        self.frame[i] = ptr::read_volatile(self.data.add(buffer + i));
                    }
                }
                // C immediately copies into an skb; no retained DMA reference.
                unsafe { centauri_radio_receive(self.owner, self.frame.as_ptr(), bytes) };
                self.received += 1;
            }
            self.write(offset, &[0; 32])?;
            self.set_rx_owner(cookie, false);
            let tail = (self.pending_head + self.pending_count) % SOURCE_ENTRIES;
            self.pending[tail as usize] = cookie;
            self.pending_count += 1;
            self.rx_cursor = (self.rx_cursor + 1) % RX_ENTRIES;
            done += 1;
        }
        if done > 0 {
            self.publish(HOST_COMPLETIONS, RX_CURSOR, self.rx_cursor, None);
        }
        self.replenish()?;
        Ok((done, tx_done))
    }
}

#[no_mangle]
pub(crate) extern "C" fn centauri_wifi_data_bytes() -> usize {
    DATA_BYTES
}

/// # Safety
/// C retains the basic state, BAR and one coherent allocation for the10MiB
/// context/basic prefix followed by exactly DATA_BYTES. It holds state_lock.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_data_create(
    basic: *mut c_void,
    owner: *mut c_void,
    bar: *mut c_void,
    context: *mut u8,
    context_dma: u64,
    data: *mut u8,
    dma: u64,
    bytes: usize,
    output: *mut *mut c_void,
) -> i32 {
    if output.is_null() || basic.is_null() {
        return EINVAL.to_errno();
    }
    unsafe { output.write(ptr::null_mut()) };
    let create = || -> Result<KBox<DataQueues>> {
        let initial =
            unsafe { DataQueues::new(owner, bar, context, context_dma, data, dma, bytes) }?;
        let mut state = KBox::new(initial, GFP_KERNEL)?;
        state.configure(unsafe { &mut *basic.cast::<WifiQueues>() })?;
        Ok(state)
    };
    match create() {
        Ok(state) => {
            unsafe { output.write(KBox::into_raw(state).cast()) };
            0
        }
        Err(error) => error.to_errno(),
    }
}
/// # Safety
/// State is the live create result, exclusively locked, with an immutable frame.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_data_xmit(
    state: *mut c_void,
    frame: *const u8,
    bytes: usize,
) -> i32 {
    if frame.is_null() || !(14..=CAPACITY).contains(&bytes) {
        return EINVAL.to_errno();
    }
    let state = unsafe { &mut *state.cast::<DataQueues>() };
    state
        .xmit(unsafe { core::slice::from_raw_parts(frame, bytes) })
        .map_or_else(|e| e.to_errno(), |_| 0)
}
/// # Safety
/// C holds state_lock and retains the state plus writable tx_done slot.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_data_poll(
    state: *mut c_void,
    budget: u32,
    tx_done: *mut bool,
) -> i32 {
    if tx_done.is_null() {
        return EINVAL.to_errno();
    }
    unsafe { tx_done.write(false) };
    let state = unsafe { &mut *state.cast::<DataQueues>() };
    match state.poll((budget as usize).min(128)) {
        Ok((count, done)) => {
            unsafe { tx_done.write(done) };
            count as i32
        }
        Err(error) => error.to_errno(),
    }
}
/// # Safety
/// Unique create result, returned after IRQ/work users and DMA have stopped.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_wifi_data_destroy(state: *mut c_void) {
    let state = unsafe { KBox::<DataQueues>::from_raw(state.cast()) };
    kernel::pr_info!(
        "Wi-Fi Ethernet completions: TX={} RX={} recycled={}\n",
        state.transmitted,
        state.received,
        state.recycled
    );
    drop(state);
}
