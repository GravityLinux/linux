// SPDX-License-Identifier: GPL-2.0-only

//! Generated PF2 IPC and inline registration in caller-owned coherent memory.
//! Python's owned Bluetooth path qualifies this page layout and wire encoding.

use core::{ffi::c_void, ptr};
use kernel::prelude::*;

pub(super) const PAGE_BYTES: usize = 0x4000;
const HEADER_BYTES: usize = 0x68;
const DEVICE_CQ: usize = 0x78;
const DEVICE_SQ: usize = 0x7a;
const HOST_CQ: usize = 0xa6;
const HOST_SQ: usize = 0xa8;
const ADMIN_SQ: usize = 0xd4;
const ADMIN_CQ: usize = 0x1d4;
const ADMIN_ENTRIES: u16 = 16;
const ADMIN_FRAME: usize = 0xf80;
const AUXILIARY: usize = 0x3d8;
const COMPLETION_BYTES: usize = 32;
const INLINE_COMPLETIONS: u32 = 1 << 22;
const EXTENDED_COMPLETIONS: u32 = 1 << 26;
const IPC: usize = 0;
const CONTEXT_LOW: usize = 4;
const CONTEXT_HIGH: usize = 8;
const WINDOW_LOW: usize = 0xc;
const WINDOW_HIGH: usize = 0x10;
const WINDOW_LENGTH: usize = 0x14;
const PCI_RESOURCE_LOW: u16 = 0xf88;
const PCI_RESOURCE_HIGH: u16 = 0xf8c;
const PCI_RESOURCE_BYTES: u16 = 0xf90;
const PCI_TIME_LOW: u16 = 0xff8;
const PCI_TIME_HIGH: u16 = 0xffc;
const INLINE_ENTRIES: u16 = 2;
// Lower format profile remains qualified rather than assigned a version name.
const CONTEXT_FORMAT_PROFILE: u64 = 0x0300;
const FIRMWARE: usize = 0x18;
const HANDSHAKE: usize = 0x107c;
const WAIT_ATTEMPTS: usize = 600;

unsafe extern "C" {
    fn centauri_boot_read(bar: *mut c_void, offset: usize) -> u32;
    fn centauri_boot_write(bar: *mut c_void, offset: usize, value: u32);
    fn centauri_boot_sleep();
    fn centauri_control_dma_wmb();
    fn centauri_control_dma_rmb();
    fn centauri_bt_enable_dma(owner: *mut c_void);
    fn centauri_bt_config_read(owner: *mut c_void, offset: u16, value: *mut u32) -> i32;
    fn centauri_bt_config_write(owner: *mut c_void, offset: u16, value: u32) -> i32;
    fn centauri_boot_time_us() -> u64;
}

pub(super) struct Owned {
    bar: *mut c_void,
    cpu: *mut u8,
    pub(super) dma: u64,
    admin: u16,
}
impl Owned {
    /// # Safety
    /// C retains a mapped PF2 BAR and its own PAGE_BYTES coherent allocation.
    /// All calls are serialized; DMA is disabled/drained before release.
    pub(super) unsafe fn new(
        bar: *mut c_void,
        cpu: *mut u8,
        dma: u64,
        bytes: usize,
    ) -> Result<Self> {
        if bar.is_null()
            || cpu.is_null()
            || cpu as usize & 3 != 0
            || bytes != PAGE_BYTES
            || dma & (PAGE_BYTES as u64 - 1) != 0
            || dma
                .checked_add(bytes as u64)
                .is_none_or(|end| end > 1 << 36)
        {
            return Err(EINVAL);
        }
        Ok(Self {
            bar,
            cpu,
            dma,
            admin: 0,
        })
    }
    pub(super) fn read_register(&self, offset: usize) -> u32 {
        unsafe { centauri_boot_read(self.bar, offset) }
    }
    pub(super) fn write_register(&self, offset: usize, value: u32) {
        unsafe { centauri_boot_write(self.bar, offset, value) };
    }
    pub(super) fn ready(&self) -> Result {
        if self.read_register(IPC) != 2
            || self.read_register(FIRMWARE) != 2
            || (u64::from(self.read_register(CONTEXT_LOW))
                | u64::from(self.read_register(CONTEXT_HIGH)) << 32)
                != self.dma
        {
            return Err(EIO);
        }
        Ok(())
    }
    fn offset(&self, address: u64, bytes: usize) -> Result<usize> {
        let offset =
            usize::try_from(address.checked_sub(self.dma).ok_or(EINVAL)?).map_err(|_| EINVAL)?;
        if offset.checked_add(bytes).is_none_or(|end| end > PAGE_BYTES) {
            return Err(EINVAL);
        }
        Ok(offset)
    }
    pub(super) fn read<const N: usize>(&self, address: u64) -> Result<[u8; N]> {
        let mut result = [0; N];
        self.copy(address, &mut result, false)?;
        Ok(result)
    }
    pub(super) fn copy(&self, address: u64, bytes: &mut [u8], write: bool) -> Result {
        let offset = self.offset(address, bytes.len())?;
        if !write {
            unsafe { centauri_control_dma_rmb() };
        }
        for (i, byte) in bytes.iter_mut().enumerate() {
            unsafe {
                if write {
                    ptr::write_volatile(self.cpu.add(offset + i), *byte);
                } else {
                    *byte = ptr::read_volatile(self.cpu.add(offset + i));
                }
            }
        }
        Ok(())
    }
    pub(super) fn write(&self, address: u64, bytes: &[u8]) -> Result {
        let offset = self.offset(address, bytes.len())?;
        for (i, byte) in bytes.iter().enumerate() {
            unsafe { ptr::write_volatile(self.cpu.add(offset + i), *byte) };
        }
        Ok(())
    }
    pub(super) fn index(&self, vector: u64, array: u64, number: u16) -> Result<u16> {
        let offset = self.offset(
            vector
                .checked_add(array)
                .and_then(|v| v.checked_add(u64::from(number) * 2))
                .ok_or(EINVAL)?,
            2,
        )?;
        if offset & 1 != 0 {
            return Err(EINVAL);
        }
        let result =
            unsafe { u16::from_le(ptr::read_volatile(self.cpu.add(offset).cast::<u16>())) };
        unsafe { centauri_control_dma_rmb() };
        Ok(result)
    }
    pub(super) fn set_index(&self, vector: u64, array: u64, number: u16, value: u16) -> Result {
        let offset = self.offset(
            vector
                .checked_add(array)
                .and_then(|v| v.checked_add(u64::from(number) * 2))
                .ok_or(EINVAL)?,
            2,
        )?;
        if offset & 1 != 0 {
            return Err(EINVAL);
        }
        unsafe {
            centauri_control_dma_wmb();
            ptr::write_volatile(self.cpu.add(offset).cast::<u16>(), value.to_le());
            centauri_control_dma_wmb();
        }
        Ok(())
    }
    fn wait(&self, offset: usize, wanted: u32) -> Result {
        for _ in 0..WAIT_ATTEMPTS {
            if self.read_register(offset) == wanted {
                return Ok(());
            }
            unsafe { centauri_boot_sleep() };
        }
        Err(ETIMEDOUT)
    }
    pub(super) fn start(
        &self,
        owner: *mut c_void,
        resource_dma: u64,
        resource_bytes: usize,
    ) -> Result {
        let end = resource_dma
            .checked_add(resource_bytes as u64)
            .ok_or(EINVAL)?;
        let page_end = self.dma + PAGE_BYTES as u64;
        if owner.is_null()
            || resource_dma & 0x3fff != 0
            || resource_bytes == 0
            || resource_bytes & 0x3fff != 0
            || end > 1 << 32
            || !(end <= self.dma || page_end <= resource_dma)
        {
            return Err(EINVAL);
        }
        let window = resource_dma.min(self.dma);
        let window_bytes = u32::try_from(end.max(page_end) - window).map_err(|_| EINVAL)?;
        let mut prior = 0;
        let error = unsafe { centauri_bt_config_read(owner, PCI_RESOURCE_BYTES, &mut prior) };
        if error < 0 {
            return Err(Error::from_errno(error));
        }
        if prior != 0 || self.read_register(IPC) != 0 {
            return Err(EBUSY);
        }
        self.wait(FIRMWARE, 2)?;
        unsafe { ptr::write_bytes(self.cpu, 0, PAGE_BYTES) };
        let mut header = [0; ADMIN_SQ];
        header[..8]
            .copy_from_slice(&((HEADER_BYTES as u64) << 16 | CONTEXT_FORMAT_PROFILE).to_le_bytes());
        for (field, target) in [
            (8, HEADER_BYTES),
            (0x10, DEVICE_CQ),
            (0x18, DEVICE_SQ),
            (0x20, HOST_CQ),
            (0x28, HOST_SQ),
            (0x34, ADMIN_CQ),
            (0x3c, ADMIN_SQ),
            (0x58, AUXILIARY),
        ] {
            header[field..field + 8].copy_from_slice(&(self.dma + target as u64).to_le_bytes());
        }
        header[0x30..0x34].copy_from_slice(&(22u32 << 16 | 1).to_le_bytes());
        header[0x44..0x48].copy_from_slice(
            &(u32::from(ADMIN_ENTRIES) << 16 | u32::from(ADMIN_ENTRIES)).to_le_bytes(),
        );
        header[0x50..0x54].copy_from_slice(&EXTENDED_COMPLETIONS.to_le_bytes());
        for offset in [HEADER_BYTES, HEADER_BYTES + 4] {
            header[offset..offset + 4].copy_from_slice(&2u32.to_le_bytes());
        }
        self.write(self.dma, &header)?;
        unsafe { centauri_bt_enable_dma(owner) };
        self.write_register(HANDSHAKE, 1);
        self.wait(IPC, 1)?;
        for (offset, value) in [
            (WINDOW_LOW, window as u32),
            (WINDOW_HIGH, (window >> 32) as u32),
            (WINDOW_LENGTH, window_bytes),
            (CONTEXT_LOW, self.dma as u32),
            (CONTEXT_HIGH, (self.dma >> 32) as u32),
            (HANDSHAKE, 2),
        ] {
            self.write_register(offset, value);
        }
        self.wait(IPC, 2)
    }
    pub(super) fn publish_resource(&self, owner: *mut c_void, dma: u64, bytes: usize) -> Result {
        let time = unsafe { centauri_boot_time_us() };
        for (offset, value) in [
            (PCI_TIME_LOW, time as u32),
            (PCI_TIME_HIGH, (time >> 32) as u32),
            (PCI_RESOURCE_LOW, dma as u32),
            (PCI_RESOURCE_HIGH, (dma >> 32) as u32),
            (PCI_RESOURCE_BYTES, bytes as u32),
        ] {
            let error = unsafe { centauri_bt_config_write(owner, offset, value) };
            if error < 0 {
                return Err(Error::from_errno(error));
            }
        }
        self.ready()
    }
    pub(super) fn register(&mut self, number: u16, offset: usize, capacity: usize) -> Result {
        if !(1..22).contains(&number)
            || capacity == 0
            || capacity > 4096
            || offset < 0x1000
            || offset & 63 != 0
            || offset
                .checked_add(2 * (16 + capacity))
                .is_none_or(|end| end > PAGE_BYTES)
        {
            return Err(EINVAL);
        }
        let mut mode = None;
        for exponent in 0..=4 {
            let unit = 4usize << exponent;
            if capacity % unit == 0 && capacity / unit <= 255 {
                mode = Some((exponent as u32) << 28 | ((capacity / unit) as u32) << 16 | 1);
                break;
            }
        }
        let mode = mode.ok_or(EINVAL)?;
        let ring = self.dma + offset as u64;
        let words = [
            mode,
            u32::from(number) << 16 | u32::from(number),
            ring as u32,
            (ring >> 32) as u32,
            u32::MAX,
            u32::MAX,
            u32::from(u16::MAX) << 16 | u32::from(INLINE_ENTRIES),
            INLINE_COMPLETIONS | u32::from(number),
            0,
            0,
            0,
            0,
            0,
        ];
        let mut frame = [0; 52];
        for (i, word) in words.iter().enumerate() {
            frame[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        self.ready()?;
        let slot = self.admin;
        for cursor in [DEVICE_CQ, DEVICE_SQ] {
            if self.index(self.dma + cursor as u64, 0, 0)? != slot {
                return Err(EBUSY);
            }
        }
        self.write(self.dma + ADMIN_FRAME as u64, &frame)?;
        let mut descriptor = [0; 16];
        descriptor[..4].copy_from_slice(&(52u32 << 8 | 1).to_le_bytes());
        descriptor[4..12].copy_from_slice(&(self.dma + ADMIN_FRAME as u64).to_le_bytes());
        descriptor[12..16].copy_from_slice(&u32::from(slot).to_le_bytes());
        self.write(
            self.dma + ADMIN_SQ as u64 + u64::from(slot) * 16,
            &descriptor,
        )?;
        let next = (slot + 1) % ADMIN_ENTRIES;
        self.set_index(self.dma + HOST_CQ as u64, 0, 0, slot)?;
        self.set_index(self.dma + HOST_SQ as u64, 0, 0, next)?;
        self.write_register(0x1000, u32::from(next));
        for _ in 0..WAIT_ATTEMPTS {
            self.ready()?;
            if self.index(self.dma + DEVICE_CQ as u64, 0, 0)? == next
                && self.index(self.dma + DEVICE_SQ as u64, 0, 0)? == next
            {
                let cq = self.read::<16>(
                    self.dma + ADMIN_CQ as u64 + u64::from(slot) * COMPLETION_BYTES as u64,
                )?;
                if u32::from_le_bytes(cq[..4].try_into().unwrap()) != 0x401
                    || u32::from_le_bytes(cq[4..8].try_into().unwrap())
                        != 52u32 << 16 | u32::from(slot)
                {
                    return Err(EIO);
                }
                self.admin = next;
                return Ok(());
            }
            unsafe { centauri_boot_sleep() };
        }
        Err(ETIMEDOUT)
    }
}
