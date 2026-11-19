// SPDX-License-Identifier: GPL-2.0-only
//! Independent model of fresh Bluetooth context, registrations and HCI ownership.
extern crate self as kernel;
#[macro_export]
macro_rules! pr_err { ($($args:tt)*) => { eprintln!($($args)*); }; }
#[macro_export]
macro_rules! pr_info { ($($args:tt)*) => { eprintln!($($args)*); }; }
pub mod prelude {
    pub const GFP_KERNEL: () = ();
    pub struct KBox<T>(Box<T>);
    impl<T> KBox<T> {
        pub fn new(value: T, _: ()) -> Result<Self> {
            Ok(Self(Box::new(value)))
        }
        pub fn into_raw(value: Self) -> *mut T {
            Box::into_raw(value.0)
        }
        pub unsafe fn from_raw(value: *mut T) -> Self {
            Self(unsafe { Box::from_raw(value) })
        }
    }
    impl<T> std::ops::Deref for KBox<T> {
        type Target = T;
        fn deref(&self) -> &T {
            &self.0
        }
    }
    impl<T> std::ops::DerefMut for KBox<T> {
        fn deref_mut(&mut self) -> &mut T {
            &mut self.0
        }
    }
    pub struct KVec<T>(Vec<T>);
    impl<T> KVec<T> {
        pub fn with_capacity(n: usize, _: ()) -> Result<Self> {
            Ok(Self(Vec::with_capacity(n)))
        }
        pub fn push(&mut self, v: T, _: ()) -> Result {
            self.0.push(v);
            Ok(())
        }
    }
    impl<T: Clone> KVec<T> {
        pub fn from_elem(value: T, count: usize, _: ()) -> Result<Self> {
            Ok(Self(vec![value; count]))
        }
    }
    impl<T> std::ops::Deref for KVec<T> {
        type Target = [T];
        fn deref(&self) -> &[T] {
            &self.0
        }
    }
    impl<T> std::ops::DerefMut for KVec<T> {
        fn deref_mut(&mut self) -> &mut [T] {
            &mut self.0
        }
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Error(i32);
    impl Error {
        pub fn from_errno(value: i32) -> Self {
            Self(value)
        }
        pub fn to_errno(self) -> i32 {
            self.0
        }
    }
    pub type Result<T = ()> = core::result::Result<T, Error>;
    pub const EINVAL: Error = Error(-22);
    pub const EBUSY: Error = Error(-16);
    pub const ETIMEDOUT: Error = Error(-110);
    pub const EIO: Error = Error(-5);
    pub const EMSGSIZE: Error = Error(-90);
    pub const EAGAIN: Error = Error(-11);
    pub const EOPNOTSUPP: Error = Error(-95);
}

use core::{ffi::c_void, ptr};
use std::cell::Cell;
use std::collections::BTreeMap;
#[path = "../../drivers/net/wireless/apple/centauri/centauri_bluetooth.rs"]
mod centauri_bluetooth;
#[path = "../../drivers/net/wireless/apple/centauri/centauri_bluetooth_cold.rs"]
mod centauri_bluetooth_cold;
fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
struct Buffer {
    flags: u32,
    address: u64,
    tag: u32,
}
impl Buffer {
    fn bytes(&self) -> [u8; 16] {
        let mut b = [0; 16];
        b[..4].copy_from_slice(&self.flags.to_le_bytes());
        b[4..12].copy_from_slice(&self.address.to_le_bytes());
        b[12..].copy_from_slice(&self.tag.to_le_bytes());
        b
    }
}
thread_local! { static NOW:Cell<u64>=const{Cell::new(0)}; }
const DMA: u64 = 0x100000000;
const RESOURCE: u64 = 0xffc00000;
struct Bus {
    page: Vec<u8>,
    registers: BTreeMap<usize, u32>,
    config: BTreeMap<u16, u32>,
    frames: Vec<Vec<u8>>,
    rings: BTreeMap<u16, (usize, usize)>,
    commands: Vec<Vec<u8>>,
    received: Vec<(u8, Vec<u8>)>,
    master: bool,
    mtu: u16,
    corrupt_cq: bool,
    drop_buffer: bool,
}
impl Bus {
    fn new() -> Self {
        NOW.with(|v| v.set(0));
        Self {
            page: vec![0xa5; 0x4000],
            registers: BTreeMap::from([(0, 0), (0x18, 2)]),
            config: BTreeMap::new(),
            frames: vec![],
            rings: BTreeMap::new(),
            commands: vec![],
            received: vec![],
            master: false,
            mtu: 2586,
            corrupt_cq: false,
            drop_buffer: false,
        }
    }
    fn u16(&self, at: usize) -> u16 {
        le16(&self.page, at)
    }
    fn put16(&mut self, at: usize, v: u16) {
        self.page[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn inject(&mut self, number: u16, packet: &[u8]) {
        let (ring, capacity) = self.rings[&number];
        let slot = self.u16(0x7a + number as usize * 2);
        assert_ne!(slot, self.u16(0xa8 + number as usize * 2));
        let offset = ring + slot as usize * (16 + capacity);
        assert_eq!(le64(&self.page, offset + 4), u64::MAX);
        assert!(packet.len() <= capacity);
        self.page[offset..offset + 16].copy_from_slice(
            &Buffer {
                flags: (packet.len() as u32) << 8,
                address: u64::MAX,
                tag: u32::from(slot) | (1 << 26),
            }
            .bytes(),
        );
        self.page[offset + 16..offset + 16 + packet.len()].copy_from_slice(packet);
        self.put16(0x7a + number as usize * 2, (slot + 1) % 2);
    }
    fn notify(&mut self, offset: usize, value: u32) {
        if offset == 0x1000 {
            let slot = self.u16(0x7a);
            let at = 0xd4 + slot as usize * 16;
            assert_eq!(le32(&self.page, at), 52 << 8 | 1);
            assert_eq!(le32(&self.page, at + 12), u32::from(slot));
            assert_eq!(le64(&self.page, at + 4), DMA + 0xf80);
            let frame = self.page[0xf80..0xfb4].to_vec();
            let number = le16(&frame, 4);
            let pointer = le64(&frame, 8);
            assert_eq!(le16(&frame, 6), number);
            assert_eq!(le16(&frame, 24), 2);
            assert_eq!(le16(&frame, 26), u16::MAX);
            assert_eq!(le32(&frame, 28), (1 << 22) | u32::from(number));
            let mode = le32(&frame, 0);
            let capacity = (((mode >> 16) & 255) as usize * 4) << ((mode >> 28) & 15);
            let ring = (pointer - DMA) as usize;
            assert!(ring >= 0x1000 && ring + 2 * (16 + capacity) <= 0x4000);
            self.rings.insert(number, (ring, capacity));
            self.frames.push(frame);
            let cq = 0x1d4 + slot as usize * 32;
            self.page[cq..cq + 4].copy_from_slice(&0x401u32.to_le_bytes());
            let cookie = if self.corrupt_cq {
                u32::from(slot) + 1
            } else {
                u32::from(slot)
            };
            self.page[cq + 4..cq + 8].copy_from_slice(&(52u32 << 16 | cookie).to_le_bytes());
            self.put16(0x78, value as u16);
            self.put16(0x7a, value as u16);
        } else if offset == 0x1004 {
            let (ring, capacity) = self.rings[&1];
            let slot = self.u16(0x7c);
            let at = ring + slot as usize * (16 + capacity);
            let flags = le32(&self.page, at);
            assert_eq!(flags & 255, 2);
            assert_eq!(le64(&self.page, at + 4), u64::MAX);
            let bytes = (flags >> 8) as usize;
            let command = self.page[at + 16..at + 16 + bytes].to_vec();
            self.commands.push(command.clone());
            self.put16(0x7c, value as u16);
            let opcode = le16(&command, 0);
            if opcode == 0x1005 && !self.drop_buffer {
                let mut event = vec![0x0e, 11, 1, 5, 0x10, 0];
                event.extend_from_slice(&self.mtu.to_le_bytes());
                event.extend_from_slice(&[0, 4, 0, 0, 0]);
                self.inject(7, &event);
            } else if opcode == 0x0c03 || opcode == 0xff77 {
                self.inject(7, &[0x0e, 4, 1, command[0], command[1], 0]);
            }
        } else if offset == 0x1008 {
            self.put16(0x7e, value as u16);
        }
    }
    fn create(&mut self) -> Result<*mut c_void, i32> {
        let owner = (self as *mut Self).cast();
        let mut output = ptr::null_mut();
        let cpu = self.page.as_mut_ptr();
        let error = unsafe {
            centauri_bluetooth::centauri_bt_rust_create(
                owner,
                owner,
                cpu,
                DMA,
                0x4000,
                RESOURCE,
                0x4000,
                &mut output,
            )
        };
        if error == 0 {
            Ok(output)
        } else {
            assert!(output.is_null());
            Err(error)
        }
    }
}
#[no_mangle]
unsafe extern "C" fn centauri_boot_read(bar: *mut c_void, offset: usize) -> u32 {
    let b = unsafe { &*(bar as *mut Bus) };
    *b.registers.get(&offset).unwrap_or(&0)
}
#[no_mangle]
unsafe extern "C" fn centauri_boot_write(bar: *mut c_void, offset: usize, value: u32) {
    let b = unsafe { &mut *(bar as *mut Bus) };
    b.registers.insert(offset, value);
    if offset == 0x107c {
        assert!(b.master);
        if value == 1 {
            assert_eq!(&b.page[..0xd4], include_bytes!(env!("CENTAURI_BT_CONTEXT")));
        }
        b.registers.insert(0, value);
    } else if (0x1000..0x1058).contains(&offset) {
        b.notify(offset, value);
    }
}
#[no_mangle]
unsafe extern "C" fn centauri_bt_enable_dma(owner: *mut c_void) {
    unsafe {
        (*(owner as *mut Bus)).master = true;
    }
}
#[no_mangle]
unsafe extern "C" fn centauri_bt_config_read(
    owner: *mut c_void,
    offset: u16,
    value: *mut u32,
) -> i32 {
    let b = unsafe { &*(owner as *mut Bus) };
    unsafe { value.write(*b.config.get(&offset).unwrap_or(&0)) };
    0
}
#[no_mangle]
unsafe extern "C" fn centauri_bt_config_write(owner: *mut c_void, offset: u16, value: u32) -> i32 {
    let b = unsafe { &mut *(owner as *mut Bus) };
    assert_eq!(b.frames.len(), 4);
    b.config.insert(offset, value);
    0
}
#[no_mangle]
extern "C" fn centauri_boot_sleep() {
    NOW.with(|v| v.set(v.get() + 5));
}
#[no_mangle]
extern "C" fn centauri_bt_now_ms() -> u64 {
    NOW.with(|v| v.get())
}
#[no_mangle]
extern "C" fn centauri_boot_time_us() -> u64 {
    1793000000000000
}
#[no_mangle]
extern "C" fn centauri_control_dma_wmb() {
    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
}
#[no_mangle]
extern "C" fn centauri_control_dma_rmb() {
    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
}
#[no_mangle]
unsafe extern "C" fn centauri_bt_queue_rx(
    owner: *mut c_void,
    kind: u8,
    packet: *const u8,
    bytes: usize,
) {
    let b = unsafe { &mut *(owner as *mut Bus) };
    b.received.push((
        kind,
        unsafe { std::slice::from_raw_parts(packet, bytes) }.to_vec(),
    ));
}

#[test]
fn generated_context_registrations_and_reported_mtu_match_python() {
    let mut b = Bus::new();
    let state = b.create().unwrap();
    assert_eq!(
        b.frames.concat(),
        include_bytes!(env!("CENTAURI_BT_FRAMES"))
    );
    assert_eq!(b.commands, vec![vec![5, 0x10, 0]]);
    assert_eq!(b.rings[&2].1, 2592);
    assert_eq!(b.rings[&8].0, 0x2900);
    assert!(b.received.is_empty());
    assert_eq!(b.config[&0xf90], 0x4000);
    unsafe { centauri_bluetooth::centauri_bt_rust_destroy(state) };
}
#[test]
fn controller_mtu_changes_both_acl_buffers_without_a_device_identity_gate() {
    let mut b = Bus::new();
    b.mtu = 2044;
    let state = b.create().unwrap();
    assert_eq!(b.rings[&2].1, 2048);
    assert_eq!(b.rings[&8].1, 2048);
    assert_ne!(b.rings[&8].0, 0x2900);
    unsafe { centauri_bluetooth::centauri_bt_rust_destroy(state) };
}
#[test]
fn used_state_bad_registration_and_missing_mtu_fail_before_resource_publication() {
    for mode in 0..3 {
        let mut b = Bus::new();
        if mode == 0 {
            b.registers.insert(0, 2);
        }
        if mode == 1 {
            b.corrupt_cq = true;
        }
        if mode == 2 {
            b.drop_buffer = true;
        }
        assert!(b.create().is_err());
        assert_eq!(*b.config.get(&0xf90).unwrap_or(&0), 0);
        if mode == 0 {
            assert!(!b.master);
            assert!(b.page.iter().all(|&v| v == 0xa5));
        }
    }
}
#[test]
fn reset_completion_is_released_only_after_own_ff77_and_standard_acl_wraps() {
    let mut b = Bus::new();
    let state = b.create().unwrap();
    unsafe {
        assert_eq!(
            centauri_bluetooth::centauri_bt_rust_send(state, 1, [3u8, 0x0c, 0].as_ptr(), 3, false),
            0
        );
        assert!(centauri_bluetooth::centauri_bt_rust_poll(state, 64) >= 0);
    }
    assert!(b.received.is_empty());
    assert_eq!(b.commands.last().unwrap(), &[0x77, 0xff, 1, 0]);
    unsafe {
        assert!(centauri_bluetooth::centauri_bt_rust_poll(state, 64) >= 0);
    }
    assert_eq!(b.received, vec![(4, vec![0x0e, 4, 1, 3, 0x0c, 0])]);
    b.received.clear();
    for n in 0..100u8 {
        let mut packet = vec![1, 0x20, 12, 0];
        packet.extend((0..12u8).map(|v| v ^ n));
        unsafe {
            assert_eq!(
                centauri_bluetooth::centauri_bt_rust_send(
                    state,
                    2,
                    packet.as_ptr(),
                    packet.len(),
                    false
                ),
                0
            );
        }
        b.inject(8, &packet);
        unsafe {
            assert!(centauri_bluetooth::centauri_bt_rust_poll(state, 64) >= 0);
        }
        assert_eq!(b.received.pop(), Some((2, packet)));
    }
    unsafe {
        assert_eq!(
            centauri_bluetooth::centauri_bt_rust_send(
                state,
                1,
                [6u8, 0xfc, 6, 1, 2, 3, 4, 5, 6].as_ptr(),
                9,
                false
            ),
            -95
        );
        centauri_bluetooth::centauri_bt_rust_destroy(state);
    }
}
