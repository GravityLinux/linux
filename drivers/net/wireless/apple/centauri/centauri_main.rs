// SPDX-License-Identifier: GPL-2.0-only

//! Centauri Wi-Fi queue driver reconstructed from runtime hardware observations.
//!
//! The cold path owns firmware loading and newly generated radio/DMA state.
//! An explicitly selected legacy test transport accepts typed address/geometry
//! metadata. C supplies Linux net_device/NAPI/HCI glue; protocol lives here.

use core::{ptr, slice};
use kernel::prelude::*;

mod centauri_bluetooth;
mod centauri_bluetooth_cold;
mod centauri_firmware;
mod centauri_bootstrap;
mod centauri_control;
mod centauri_radio;
mod centauri_wifi_queues;
mod centauri_wifi_scan;
mod centauri_wifi_connection;
mod centauri_wifi_data;

module! {
    type: CentauriModule,
    name: "centauri",
    authors: ["Gravity Linux Contributors"],
    description: "Apple Centauri Wi-Fi and Bluetooth Rust queue driver",
    license: "GPL",
}

unsafe extern "C" {
    fn centauri_register() -> i32;
    fn centauri_unregister();
    fn centauri_bt_register() -> i32;
    fn centauri_bt_unregister();
    fn centauri_bootstrap_register() -> i32;
    fn centauri_bootstrap_unregister();
    fn centauri_radio_register() -> i32;
    fn centauri_radio_unregister();
    fn centauri_map_ram(physical: u64, length: usize) -> *mut c_void;
    fn centauri_unmap_ram(address: *mut c_void);
    fn centauri_map_io(physical: u64, length: usize) -> *mut c_void;
    fn centauri_unmap_io(address: *mut c_void);
    fn centauri_barrier();
    fn centauri_queue_rx(glue: *mut c_void, frame: *const u8, length: usize);
    fn centauri_tx_metadata() -> u32;
}

struct CentauriModule;
impl kernel::Module for CentauriModule {
    fn init(_: &'static ThisModule) -> Result<Self> {
        // SAFETY: Module initialization exclusively registers the C adapter.
        let result = unsafe { centauri_register() };
        if result < 0 {
            return Err(Error::from_errno(result));
        }
        // SAFETY: Same module init ownership; Bluetooth has independent queues.
        let result = unsafe { centauri_bt_register() };
        if result < 0 {
            // SAFETY: Roll back the successful network registration.
            unsafe { centauri_unregister() };
            return Err(Error::from_errno(result));
        }
        // SAFETY: Register cold radio consumers before bootstrap may rescan
        // the runtime PCI functions. Legacy packet adapters match other DTs.
        let result = unsafe { centauri_radio_register() };
        if result < 0 {
            unsafe { centauri_bt_unregister(); centauri_unregister() };
            return Err(Error::from_errno(result));
        }
        // SAFETY: The bootstrap PCI function has a distinct identity and
        // explicit DT opt-in. Roll back registrations if its driver fails.
        let result = unsafe { centauri_bootstrap_register() };
        if result < 0 {
            // SAFETY: Both independent adapters were registered above.
            unsafe { centauri_radio_unregister(); centauri_bt_unregister(); centauri_unregister() };
            return Err(Error::from_errno(result));
        }
        Ok(Self)
    }
}
impl Drop for CentauriModule {
    fn drop(&mut self) {
        // SAFETY: Registration succeeded; unregister drains NAPI and callbacks.
        unsafe {
            centauri_radio_unregister();
            centauri_bootstrap_unregister();
            centauri_bt_unregister();
            centauri_unregister();
        }
    }
}

// PF1 BAR0 registers, proven by owned host transfers and RF packet tests.
const FW_STATUS: usize = 0x08;
const IPC_STATE: usize = 0x0c;
const CONTEXT_LOW: usize = 0x10;
const CONTEXT_HIGH: usize = 0x14;
const DMA_WINDOW_LOW: usize = 0x18;
const DMA_WINDOW_HIGH: usize = 0x1c;
const DMA_WINDOW_LENGTH: usize = 0x20;
const TX_NOTIFY: usize = 0x1030;
const RX_NOTIFY: usize = 0x1050;
const DART_ERROR_STATUS: usize = 0x100;

// Context cursor arrays contain little-endian u16 indices.
const DEVICE_COMPLETION: u64 = 0x78;
const DEVICE_SUBMISSION: u64 = 0x24c;
const HOST_COMPLETION: u64 = 0x3cc;
const HOST_SUBMISSION: u64 = 0x5a0;
const COMPLETION_INDICES: u16 = 234;
const SUBMISSION_INDICES: u16 = 192;
const SOURCE_ENTRIES: u16 = 2048;
const RX_COMPLETION_ENTRIES: u16 = 4096;
const TX_COMPLETION_ENTRIES: u16 = 8192;
const RX_DESCRIPTOR_BYTES: u64 = 16;
const DATA_DESCRIPTOR_BYTES: u64 = 32;
const COMPLETION_BYTES: u64 = 32;
const DATA_FORMAT: u32 = 3;
const RX_COMPLETION_FORMAT: u16 = 0x0403;
const TX_COMPLETION_FORMAT: u16 = 0x0502;
const RX_ALIGNMENT_BYTES: usize = 2;
const PACKET_CAPACITY: usize = 2048;
const TX_AREA: u64 = 0x20000;
const TX_SLOT_BYTES: u64 = 0x1000;
const TX_SLOTS: u16 = 64;
const RX_QUEUE_MODE: u32 = 0x0b;
const TX_QUEUE_MODE: u32 = 0x12;
const EXTENDED_SUBMISSION: u32 = 0x40001;
const EXTENDED_COMPLETION: u32 = 0x40002;

fn fence() {
    // SAFETY: Unconditional arm64 dsb sy; no device memory dereference.
    unsafe { centauri_barrier() };
}
fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}
fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn le64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

// C resolves these from the PCI BAR and the device's firmware-DART phandle.
#[repr(C)]
#[derive(Clone, Copy)]
struct HardwareResources {
    bar: u64,
    bar_length: u64,
    dart: u64,
    dart_length: u64,
}
impl HardwareResources {
    fn io(&self, recorded_bar: u64, recorded_dart: u64) -> Result<(Io, Io)> {
        if recorded_bar != self.bar || recorded_dart != self.dart || self.bar_length < 0x1054 || self.dart_length < 0x104 {
            return Err(EINVAL);
        }
        Ok((Io::new(self.bar, usize::try_from(self.bar_length).map_err(|_| EINVAL)?)?,
            Io::new(self.dart, usize::try_from(self.dart_length).map_err(|_| EINVAL)?)?))
    }
}

struct Reader<'a> { data: &'a [u8], position: usize }
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self.position.checked_add(count).ok_or(EINVAL)?;
        let value = self.data.get(self.position..end).ok_or(EINVAL)?;
        self.position = end;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32> { Ok(le32(self.take(4)?, 0)) }
    fn u64(&mut self) -> Result<u64> { Ok(le64(self.take(8)?, 0)) }
}

struct Io { address: *mut u8, length: usize }
impl Io {
    fn new(physical: u64, length: usize) -> Result<Self> {
        // SAFETY: Fixed-size resources from validated device-specific metadata.
        let address = unsafe { centauri_map_io(physical, length) }.cast::<u8>();
        if address.is_null() { return Err(ENOMEM); }
        Ok(Self { address, length })
    }
    fn read(&self, offset: usize) -> Result<u32> {
        if offset & 3 != 0 || offset.checked_add(4).ok_or(EINVAL)? > self.length {
            return Err(EINVAL);
        }
        fence();
        // SAFETY: The checked offset is aligned and inside our live IO mapping.
        let value = unsafe { ptr::read_volatile(self.address.add(offset).cast::<u32>()) };
        fence();
        Ok(u32::from_le(value))
    }
    fn write(&self, offset: usize, value: u32) -> Result {
        if offset & 3 != 0 || offset.checked_add(4).ok_or(EINVAL)? > self.length {
            return Err(EINVAL);
        }
        fence();
        // SAFETY: Same mapping/bounds guarantees as read; this is an owned register.
        unsafe { ptr::write_volatile(self.address.add(offset).cast::<u32>(), value.to_le()) };
        fence();
        Ok(())
    }
}
impl Drop for Io {
    fn drop(&mut self) {
        // SAFETY: Created by map_io and uniquely owned until this destructor.
        unsafe { centauri_unmap_io(self.address.cast()) };
    }
}

struct Region { iova: u64, physical: u64, length: usize, address: *mut u8 }
impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY: Created by map_ram; underlying reserved pages remain retained.
        unsafe { centauri_unmap_ram(self.address.cast()) };
    }
}
struct Memory { regions: KVec<Region>, window: u64, end: u64 }
impl Memory {
    fn copy(&self, mut iova: u64, data: &mut [u8], write: bool) -> Result {
        let end = iova.checked_add(data.len() as u64).ok_or(EINVAL)?;
        if data.is_empty() || data.len() > 0x10000 || iova < self.window || end > self.end {
            return Err(EINVAL);
        }
        fence();
        let mut done = 0;
        while done < data.len() {
            let region = self.regions.iter().find(|r| r.iova <= iova && iova < r.iova + r.length as u64).ok_or(EFAULT)?;
            let offset = (iova - region.iova) as usize;
            let count = (region.length - offset).min(data.len() - done);
            for n in 0..count {
                // SAFETY: The matched region bounds each byte. The reserved
                // Normal-NC mapping remains live; callbacks hold the driver lock.
                unsafe {
                    let address = region.address.add(offset + n);
                    if write { ptr::write_volatile(address, data[done + n]); }
                    else { data[done + n] = ptr::read_volatile(address); }
                }
            }
            iova += count as u64;
            done += count;
        }
        fence();
        Ok(())
    }
    fn read<const N: usize>(&self, iova: u64) -> Result<[u8; N]> {
        let mut result = [0; N];
        self.copy(iova, &mut result, false)?;
        Ok(result)
    }
    fn write(&self, iova: u64, data: &mut [u8]) -> Result { self.copy(iova, data, true) }
    fn index(&self, context: u64, array: u64, index: u16) -> Result<u16> {
        Ok(le16(&self.read::<2>(context + array + u64::from(index) * 2)?, 0))
    }
    fn set_index(&self, context: u64, array: u64, index: u16, value: u16) -> Result {
        self.write(context + array + u64::from(index) * 2, &mut value.to_le_bytes())
    }
    fn physical_read<const N: usize>(&self, physical: u64) -> Result<[u8; N]> {
        let r = self.regions.iter().find(|r| r.physical <= physical && physical + N as u64 <= r.physical + r.length as u64).ok_or(EFAULT)?;
        self.read(r.iova + physical - r.physical)
    }
}

struct Queue { kind: u32, number: u16, words: [u32; 15] }
impl Queue {
    fn ring(&self) -> u64 { u64::from(self.words[2]) | (u64::from(self.words[3]) << 32) }
}
#[derive(Clone, Copy, Default)]
struct Buffer { flags: u32, address: u64, tag: u32 }
impl Buffer {
    fn bytes(&self) -> [u8; 16] {
        let mut result = [0; 16];
        result[..4].copy_from_slice(&self.flags.to_le_bytes());
        result[4..12].copy_from_slice(&self.address.to_le_bytes());
        result[12..].copy_from_slice(&self.tag.to_le_bytes());
        result
    }
}
struct RxSource { number: u16, ring: u64, buffers: KVec<Buffer> }
struct RxCompletion { ring: u64, index: u16, source: usize, ethernet: bool }
struct Core {
    glue: *mut c_void,
    memory: Memory,
    bank: Io,
    dart: Io,
    context: u64,
    arena: u64,
    mac: [u8; 6],
    tx_source: u16,
    tx_ring: u64,
    tx_completion: u64,
    tx_index: u16,
    tx_pending: Option<u16>,
    sources: KVec<RxSource>,
    completions: KVec<RxCompletion>,
    rx_packets: u64,
    rx_recycled: u64,
    tx_packets: u64,
}

impl Core {
    fn create(glue: *mut c_void, bytes: &[u8], resources: &HardwareResources) -> Result<Self> {
        let mut input = Reader { data: bytes, position: 0 };
        if input.take(8)? != b"CENH0001" || input.u32()? as usize != bytes.len() { return Err(EINVAL); }
        let map_count = input.u32()? as usize;
        let queue_count = input.u32()? as usize;
        input.u32()?; // Legacy serial-length field is ignored.
        if map_count == 0 || map_count > 8192 || queue_count > 426 { return Err(EINVAL); }
        let context = input.u64()?;
        let context_physical = input.u64()?;
        let bank_physical = input.u64()?;
        let dart_physical = input.u64()?;
        let (bank, dart) = resources.io(bank_physical, dart_physical)?;
        let arena = input.u64()?;
        let arena_physical = input.u64()?;
        let arena_length = input.u64()?;
        let nonce_physical = input.u64()?;
        let dram = input.u64()?;
        let dram_end = dram.checked_add(input.u64()?).ok_or(EINVAL)?;
        let mac: [u8; 6] = input.take(6)?.try_into().map_err(|_| EINVAL)?;
        if mac[0] & 1 != 0 || input.take(2)? != [0, 0] { return Err(EINVAL); }
        let nonce = input.take(32)?;
        // Keep the existing handoff layout; serial metadata is informational.
        input.take(32)?;
        if arena_length < TX_AREA + u64::from(TX_SLOTS) * TX_SLOT_BYTES { return Err(EINVAL); }
        let window = u64::from(bank.read(DMA_WINDOW_LOW)?) | u64::from(bank.read(DMA_WINDOW_HIGH)?) << 32;
        let end = window.checked_add(u64::from(bank.read(DMA_WINDOW_LENGTH)?)).ok_or(EINVAL)?;
        let mut regions = KVec::with_capacity(map_count, GFP_KERNEL)?;
        let mut previous_end = 0;
        for _ in 0..map_count {
            let iova = input.u64()?;
            let physical = input.u64()?;
            let length = input.u64()?;
            if iova < previous_end || length == 0 || length > 0x4000000 || physical < dram || physical.checked_add(length).ok_or(EINVAL)? > dram_end || (physical | length) & 4095 != 0 {
                return Err(EINVAL);
            }
            // SAFETY: All pages are checked reserved by the C mapping helper.
            let address = unsafe { centauri_map_ram(physical, length as usize) }.cast::<u8>();
            if address.is_null() { return Err(ENOMEM); }
            regions.push(Region { iova, physical, length: length as usize, address }, GFP_KERNEL)?;
            previous_end = iova.checked_add(length).ok_or(EINVAL)?;
        }
        let memory = Memory { regions, window, end };
        if memory.physical_read::<32>(nonce_physical)? != nonce || memory.physical_read::<4>(context_physical)? != memory.read::<4>(context)? {
            return Err(EINVAL);
        }
        // Check the declared transmit arena is the same retained SID-2 mapping.
        let arena_region = memory.regions.iter().find(|r| r.iova <= arena && arena + arena_length <= r.iova + r.length as u64).ok_or(EINVAL)?;
        if arena_region.physical + arena - arena_region.iova != arena_physical { return Err(EINVAL); }
        let current_context = u64::from(bank.read(CONTEXT_LOW)?) | u64::from(bank.read(CONTEXT_HIGH)?) << 32;
        if context != current_context { return Err(EINVAL); }
        let mut queues = KVec::with_capacity(queue_count, GFP_KERNEL)?;
        for _ in 0..queue_count {
            let kind = input.u32()?;
            let number = input.u32()?;
            let count = input.u32()? as usize;
            if number > u16::MAX as u32 || count > 15 { return Err(EINVAL); }
            let mut words = [0; 15];
            for word in &mut words { *word = input.u32()?; }
            queues.push(Queue { kind, number: number as u16, words }, GFP_KERNEL)?;
        }
        if input.position != bytes.len() { return Err(EINVAL); }
        let mut sources = KVec::<RxSource>::new();
        let mut completions = KVec::new();
        for cq in queues.iter().filter(|q| q.kind == 2 && q.words[0] == EXTENDED_COMPLETION && q.words[5] == RX_QUEUE_MODE) {
            let source_id = (cq.words[4] >> 16) as u16;
            let cursor = (cq.words[1] >> 16) as u16;
            if cq.words[4] as u16 != RX_COMPLETION_ENTRIES || cursor >= COMPLETION_INDICES { return Err(EINVAL); }
            let source_index = match sources.iter().position(|s| s.number == source_id) {
                Some(index) => index,
                None => {
                    let sq = queues.iter().find(|q| q.kind == 1 && q.number == source_id).ok_or(EINVAL)?;
                    if source_id >= SUBMISSION_INDICES || sq.words[6] as u16 != SOURCE_ENTRIES { return Err(EINVAL); }
                    let mut buffers = KVec::from_elem(Buffer::default(), SOURCE_ENTRIES as usize, GFP_KERNEL)?;
                    for slot in 0..SOURCE_ENTRIES {
                        let raw = memory.read::<16>(sq.ring() + u64::from(slot) * RX_DESCRIPTOR_BYTES)?;
                        if raw == [0; 16] { continue; }
                        let record = Buffer { flags: le32(&raw, 0), address: le64(&raw, 4), tag: le32(&raw, 12) };
                        let cookie = record.tag as u16;
                        if record.flags & 255 != DATA_FORMAT || record.flags >> 8 > PACKET_CAPACITY as u32 || record.flags >> 8 < RX_ALIGNMENT_BYTES as u32 || cookie >= SOURCE_ENTRIES { return Err(EINVAL); }
                        let old = &mut buffers[cookie as usize];
                        if old.address != 0 && old.bytes() != raw { return Err(EINVAL); }
                        *old = record;
                    }
                    sources.push(RxSource { number: source_id, ring: sq.ring(), buffers }, GFP_KERNEL)?;
                    sources.len() - 1
                }
            };
            completions.push(RxCompletion { ring: cq.ring(), index: cursor, source: source_index, ethernet: cq.words[11] == 0 }, GFP_KERNEL)?;
        }
        if completions.is_empty() { return Err(EINVAL); }
        let mut candidates = queues.iter().filter(|q| q.kind == 1 && q.words[0] == EXTENDED_SUBMISSION && q.words[7] & 0xffff == TX_QUEUE_MODE && q.words[13..] == [0, 0]);
        let tx = candidates.next().ok_or(EINVAL)?;
        if candidates.next().is_some() || tx.number >= SUBMISSION_INDICES || tx.words[6] as u16 != SOURCE_ENTRIES { return Err(EINVAL); }
        let tx_cq_id = (tx.words[6] >> 16) as u16;
        let tx_cq = queues.iter().find(|q| q.kind == 2 && q.number == tx_cq_id).ok_or(EINVAL)?;
        let tx_index = (tx_cq.words[1] >> 16) as u16;
        if tx_cq.words[4] as u16 != TX_COMPLETION_ENTRIES || tx_cq.words[5] != TX_QUEUE_MODE || tx_index >= COMPLETION_INDICES { return Err(EINVAL); }
        let result = Self { glue, memory, bank, dart, context, arena, mac,
            tx_source: tx.number, tx_ring: tx.ring(), tx_completion: tx_cq.ring(), tx_index,
            tx_pending: None, sources, completions, rx_packets: 0, rx_recycled: 0, tx_packets: 0 };
        result.ready()?;
        pr_info!("centauri: Rust owns TX source {} / completion cursor {}, {} receive classes\n", result.tx_source, result.tx_index, result.completions.len());
        Ok(result)
    }
    fn ready(&self) -> Result {
        if self.bank.read(FW_STATUS)? != 2 || self.bank.read(IPC_STATE)? != 2 || self.dart.read(DART_ERROR_STATUS)? != 0 { return Err(EIO); }
        Ok(())
    }
    fn poll_tx(&mut self) -> Result<bool> {
        let producer = self.memory.index(self.context, DEVICE_COMPLETION, self.tx_index)?;
        let mut cursor = self.memory.index(self.context, HOST_COMPLETION, self.tx_index)?;
        if producer >= TX_COMPLETION_ENTRIES || cursor >= TX_COMPLETION_ENTRIES { return Err(EIO); }
        let mut completed = false;
        while cursor != producer {
            let raw = self.memory.read::<32>(self.tx_completion + u64::from(cursor) * COMPLETION_BYTES)?;
            if le16(&raw, 0) != TX_COMPLETION_FORMAT { return Err(EIO); }
            if le16(&raw, 2) == self.tx_source && self.tx_pending == Some(le16(&raw, 4)) {
                self.tx_pending = None;
                completed = true;
                self.tx_packets += 1;
            }
            cursor = (cursor + 1) % TX_COMPLETION_ENTRIES;
        }
        self.memory.set_index(self.context, HOST_COMPLETION, self.tx_index, cursor)?;
        Ok(completed)
    }
    fn xmit(&mut self, frame: &[u8]) -> Result {
        self.ready()?;
        self.poll_tx()?;
        if !(14..=PACKET_CAPACITY).contains(&frame.len()) { return Err(EINVAL); }
        let slot = self.memory.index(self.context, HOST_SUBMISSION, self.tx_source)?;
        let consumer = self.memory.index(self.context, DEVICE_SUBMISSION, self.tx_source)?;
        if self.tx_pending.is_some() || slot != consumer { return Err(EAGAIN); }
        if slot >= SOURCE_ENTRIES { return Err(EIO); }
        let address = self.arena + TX_AREA + u64::from(slot % TX_SLOTS) * TX_SLOT_BYTES;
        let mut packet = [0; PACKET_CAPACITY];
        packet[..frame.len()].copy_from_slice(frame);
        self.memory.write(address, &mut packet[..frame.len()])?;
        let mut descriptor = [0; 32];
        descriptor[..4].copy_from_slice(&((frame.len() as u32) << 8 | DATA_FORMAT).to_le_bytes());
        descriptor[4..12].copy_from_slice(&address.to_le_bytes());
        descriptor[12..16].copy_from_slice(&u32::from(slot).to_le_bytes());
        // SAFETY: Generic scalar module parameter read, no memory dereference.
        descriptor[16..20].copy_from_slice(&unsafe { centauri_tx_metadata() }.to_le_bytes());
        self.memory.write(self.tx_ring + u64::from(slot) * DATA_DESCRIPTOR_BYTES, &mut descriptor)?;
        self.tx_pending = Some(slot);
        self.memory.set_index(self.context, HOST_SUBMISSION, self.tx_source, (slot + 1) % SOURCE_ENTRIES)?;
        self.bank.write(TX_NOTIFY, 1)
    }
    fn poll(&mut self, budget: usize) -> Result<(usize, bool)> {
        self.ready()?;
        let tx_done = self.poll_tx()?;
        let mut done = 0;
        let mut recycled = false;
        for cq in &self.completions {
            let source = &self.sources[cq.source];
            let producer = self.memory.index(self.context, DEVICE_COMPLETION, cq.index)?;
            let mut cursor = self.memory.index(self.context, HOST_COMPLETION, cq.index)?;
            if producer >= RX_COMPLETION_ENTRIES || cursor >= RX_COMPLETION_ENTRIES { return Err(EIO); }
            while cursor != producer && done < budget {
                let raw = self.memory.read::<32>(cq.ring + u64::from(cursor) * COMPLETION_BYTES)?;
                let cookie = le16(&raw, 4);
                let length = le16(&raw, 6) as usize;
                if le16(&raw, 0) != RX_COMPLETION_FORMAT || le16(&raw, 2) != source.number || cookie >= SOURCE_ENTRIES || !(RX_ALIGNMENT_BYTES..=PACKET_CAPACITY).contains(&length) { return Err(EIO); }
                let buffer = source.buffers[cookie as usize];
                if buffer.address == 0 || buffer.flags >> 8 < length as u32 { return Err(EIO); }
                if cq.ethernet && length >= RX_ALIGNMENT_BYTES + 14 {
                    let mut frame = [0; PACKET_CAPACITY];
                    let count = length - RX_ALIGNMENT_BYTES;
                    self.memory.copy(buffer.address + RX_ALIGNMENT_BYTES as u64, &mut frame[..count], false)?;
                    if frame[0] & 1 != 0 || frame[..6] == self.mac {
                        // SAFETY: The C helper immediately copies this bounded
                        // stack buffer into an skb for delivery after unlocking.
                        unsafe { centauri_queue_rx(self.glue, frame.as_ptr(), count) };
                        self.rx_packets += 1;
                    }
                }
                cursor = (cursor + 1) % RX_COMPLETION_ENTRIES;
                self.memory.set_index(self.context, HOST_COMPLETION, cq.index, cursor)?;
                let posted = self.memory.index(self.context, HOST_SUBMISSION, source.number)?;
                let consumed = self.memory.index(self.context, DEVICE_SUBMISSION, source.number)?;
                if posted >= SOURCE_ENTRIES || consumed >= SOURCE_ENTRIES || (posted + 1) % SOURCE_ENTRIES == consumed { return Err(EIO); }
                self.memory.write(source.ring + u64::from(posted) * RX_DESCRIPTOR_BYTES, &mut buffer.bytes())?;
                self.memory.set_index(self.context, HOST_SUBMISSION, source.number, (posted + 1) % SOURCE_ENTRIES)?;
                self.rx_recycled += 1;
                recycled = true;
                done += 1;
            }
        }
        if recycled { self.bank.write(RX_NOTIFY, 1)?; }
        Ok((done, tx_done))
    }
}

/// Create the Rust protocol state from our typed, current-boot metadata.
///
/// # Safety
/// C owns `glue`, the valid input/resources and both writable output pointers.
/// The returned allocation is used only under C's driver lock until destroy.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_rust_create(glue: *mut c_void, metadata: *const u8, length: usize, output: *mut *mut c_void, mac: *mut u8, resources: *const HardwareResources) -> i32 {
    // SAFETY: C keeps the reserved host handoff mapped throughout this call.
    let bytes = unsafe { slice::from_raw_parts(metadata, length) };
    // SAFETY: C supplies live PCI/DT resources for this initialization call.
    let resources = unsafe { &*resources };
    let result = Core::create(glue, bytes, resources).and_then(|state| KBox::new(state, GFP_KERNEL).map_err(Error::from));
    match result {
        Ok(state) => {
            // SAFETY: C provided a six-byte MAC output and a state-pointer slot.
            unsafe {
                ptr::copy_nonoverlapping(state.mac.as_ptr(), mac, 6);
                output.write(KBox::into_raw(state).cast());
            }
            0
        }
        Err(error) => { pr_err!("centauri: Rust initialization failed: {}\n", error.to_errno()); error.to_errno() }
    }
}

/// # Safety
/// `state` is the unique create result after all callbacks have been drained.
#[no_mangle]
pub unsafe extern "C" fn centauri_rust_destroy(state: *mut c_void) {
    // SAFETY: Ownership returns from C exactly once after unregistering netdev.
    let state = unsafe { KBox::<Core>::from_raw(state.cast()) };
    pr_info!("centauri: Rust counters TX={} RX={} recycled={}\n", state.tx_packets, state.rx_packets, state.rx_recycled);
    drop(state);
}

/// # Safety
/// The state is live and exclusively locked; the frame slice is valid for length.
#[no_mangle]
pub unsafe extern "C" fn centauri_rust_xmit(state: *mut c_void, frame: *const u8, length: usize) -> i32 {
    // SAFETY: C serializes every protocol callback with the driver lock.
    let state = unsafe { &mut *state.cast::<Core>() };
    // SAFETY: The netdev callback supplies a linear skb of exactly this size.
    let frame = unsafe { slice::from_raw_parts(frame, length) };
    state.xmit(frame).map_or_else(|error| error.to_errno(), |_| 0)
}

/// # Safety
/// State is live/exclusively locked, and tx_done points to a writable u32.
#[no_mangle]
pub unsafe extern "C" fn centauri_rust_poll(state: *mut c_void, budget: i32, tx_done: *mut u32) -> i32 {
    // SAFETY: Serialized by the C driver lock for the whole call.
    let state = unsafe { &mut *state.cast::<Core>() };
    match state.poll(budget.max(0) as usize) {
        Ok((count, completed)) => {
            // SAFETY: The C NAPI callback supplied this writable output.
            unsafe { tx_done.write(u32::from(completed)) };
            count as i32
        }
        Err(error) => error.to_errno(),
    }
}
