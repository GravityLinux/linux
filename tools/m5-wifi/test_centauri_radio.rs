// SPDX-License-Identifier: GPL-2.0-only
//! Firmware-side model of resource publication and fresh Wi-Fi IPC.
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
    #[derive(Clone, Copy, Debug)]
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
    pub const EPROTO: Error = Error(-71);
}
#[path = "../../drivers/net/wireless/apple/centauri/centauri_radio.rs"]
mod centauri_radio;
use centauri_radio as radio;
#[path = "../../drivers/net/wireless/apple/centauri/centauri_wifi_connection.rs"]
mod centauri_wifi_connection;
#[path = "../../drivers/net/wireless/apple/centauri/centauri_wifi_data.rs"]
mod centauri_wifi_data;
#[path = "../../drivers/net/wireless/apple/centauri/centauri_wifi_queues.rs"]
mod centauri_wifi_queues;
#[path = "../../drivers/net/wireless/apple/centauri/centauri_wifi_scan.rs"]
mod centauri_wifi_scan;
#[path = "test_centauri_data.rs"]
mod data_tests;
use core::{ffi::c_void, ptr};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{fence, Ordering};
thread_local! { static SLEEPS: Cell<usize> = const { Cell::new(0) }; }
// Independent Python-qualified register map; never infer this from the Rust
// implementation under test. Wi-Fi and Beta use different handshake offsets.
const HANDSHAKE: usize =
    u32::from_le_bytes(*include_bytes!(env!("CENTAURI_WIFI_HANDSHAKE"))) as usize;
const NOTIFY: usize = u32::from_le_bytes(*include_bytes!(env!("CENTAURI_WIFI_NOTIFY"))) as usize;
struct Bus {
    arena: Vec<u8>,
    arena_dma: u64,
    resource_dma: u64,
    resource_bytes: usize,
    prior_resource: u32,
    ipc: u32,
    fw: u32,
    master: usize,
    vectors: u32,
    irq_error: i32,
    fw_ready: bool,
    handshake: u32,
    config_error: Option<u16>,
    config: Vec<(u16, u32)>,
    writes: Vec<(usize, u32)>,
    context_address: u64,
    rings: BTreeMap<(u32, u16), (usize, u16)>,
    admin_requests: Vec<Vec<u8>>,
    management_requests: Vec<(u32, u16, Vec<u8>)>,
    admin_cookie_error: bool,
    admin_complete: bool,
    drop_reply: bool,
    reply_alias: Option<u32>,
    reply_word3: u32,
    reply_transaction_delta: u16,
    malformed_reply_length: bool,
    interface_template: [u8; 6],
    interface_address: [u8; 6],
    interface_state: u8,
    corrupt_interface_address: bool,
    connection_event_before_ack: bool,
    radio_associated: bool,
    drop_disconnect_event: bool,
}
impl Bus {
    fn new() -> Self {
        SLEEPS.with(|count| count.set(0));
        Self {
            arena: vec![0xa5; 0xa00000],
            arena_dma: 0x100000000,
            resource_dma: 0xffc00000,
            resource_bytes: 0x200000,
            prior_resource: 0,
            ipc: 0,
            fw: 0,
            master: 0,
            vectors: 0,
            irq_error: 0,
            fw_ready: true,
            handshake: 2,
            config_error: None,
            config: vec![],
            writes: vec![],
            context_address: 0,
            rings: BTreeMap::new(),
            admin_requests: vec![],
            management_requests: vec![],
            admin_cookie_error: false,
            admin_complete: true,
            drop_reply: false,
            reply_alias: None,
            reply_word3: 0,
            reply_transaction_delta: 0,
            malformed_reply_length: false,
            interface_template: [0; 6],
            interface_address: [0; 6],
            interface_state: 0,
            corrupt_interface_address: false,
            connection_event_before_ack: false,
            radio_associated: false,
            drop_disconnect_event: false,
        }
    }
    fn start(&mut self) -> i32 {
        let arena = self.arena.as_mut_ptr();
        let owner = ptr::from_mut(self).cast();
        unsafe {
            radio::centauri_wifi_start(
                owner,
                owner,
                self.resource_dma,
                self.resource_bytes,
                arena,
                self.arena_dma,
                self.arena.len(),
            )
        }
    }
    fn queues(&mut self) -> centauri_wifi_queues::WifiQueues {
        let arena = self.arena.as_mut_ptr();
        let bar = ptr::from_mut(self).cast();
        unsafe { centauri_wifi_queues::WifiQueues::new(bar, arena, self.arena_dma, 0xa00000) }
            .unwrap()
    }
    fn notify(&mut self) {
        let mut consumed = load16(&self.arena, 0x24c);
        let producer = load16(&self.arena, 0x5a0);
        while consumed != producer {
            let record = 0x720 + consumed as usize * 16;
            let flags = load32(&self.arena, record);
            let pointer = load64(&self.arena, record + 4);
            let cookie = load32(&self.arena, record + 12);
            let length = (flags >> 8) as usize;
            assert_eq!(flags & 255, 1);
            assert_eq!(cookie, u32::from(consumed));
            assert!(matches!(length, 44 | 52 | 54 | 62));
            let offset = usize::try_from(pointer - self.arena_dma).unwrap();
            let frame = self.arena[offset..offset + length].to_vec();
            let kind = load32(&frame, 0) & 0xffff;
            let queue = load32(&frame, 4) as u16;
            let ring = usize::try_from(load64(&frame, 8) - self.arena_dma).unwrap();
            assert!(ring + 16 <= self.arena.len());
            let entries = load16(&frame, if kind == 2 { 16 } else { 24 });
            self.rings.insert((kind, queue), (ring, entries));
            self.admin_requests.push(frame);
            if !self.admin_complete {
                return;
            }
            store32(&mut self.arena, 0x820 + consumed as usize * 16, 0x401);
            store32(
                &mut self.arena,
                0x820 + consumed as usize * 16 + 4,
                (length as u32) << 16 | cookie ^ u32::from(self.admin_cookie_error),
            );
            consumed = (consumed + 1) % 16;
            store16(&mut self.arena, 0x78, consumed);
            store16(&mut self.arena, 0x24c, consumed);
        }
        if !self.rings.contains_key(&(1, 1)) {
            return;
        }
        let (ring, entries) = self.rings[&(1, 1)];
        let mut consumed = load16(&self.arena, 0x24e);
        let producer = load16(&self.arena, 0x5a2);
        while consumed != producer {
            let descriptor = ring + consumed as usize * 16;
            let flags = load32(&self.arena, descriptor);
            let pointer = load64(&self.arena, descriptor + 4);
            let cookie = load32(&self.arena, descriptor + 12);
            assert_eq!(flags & 255, 1);
            assert_eq!(cookie, u32::from(consumed));
            let offset = usize::try_from(pointer - self.arena_dma).unwrap();
            let length = (flags >> 8) as usize;
            let packet = self.arena[offset..offset + length].to_vec();
            assert!(packet[..16].iter().all(|value| *value == 0));
            assert_eq!(load16(&packet, 16) as usize, length);
            let transaction = load16(&packet, 18);
            let command = load32(&packet, 20);
            self.management_requests
                .push((command, transaction, packet[24..].to_vec()));
            self.complete(1, cookie as u16, length);
            consumed = (consumed + 1) % entries;
            store16(&mut self.arena, 0x24e, consumed);
            if !self.drop_reply {
                let body = match command {
                    1 => {
                        assert_eq!(packet.len(), 30);
                        self.interface_template.copy_from_slice(&packet[24..]);
                        vec![]
                    }
                    2 => {
                        assert_eq!(&packet[24..], &[0; 5]);
                        self.interface_state = 2;
                        self.interface_address = self.interface_template;
                        vec![]
                    }
                    3 => {
                        assert_eq!(&packet[24..], &[0; 5]);
                        if !self.radio_associated {
                            self.interface_state = 0;
                        }
                        vec![]
                    }
                    20 => {
                        let mut info = vec![0, self.interface_state];
                        info.extend_from_slice(&self.interface_address);
                        if self.corrupt_interface_address && self.interface_state == 2 {
                            info[7] ^= 1;
                        }
                        info
                    }
                    0x20000 => vec![],
                    0x20001 => {
                        assert_eq!(&packet[24..], &[0; 8]);
                        self.radio_associated = false;
                        vec![]
                    }
                    _ => synthetic_reply(command).unwrap_or_else(|| packet[24..].to_vec()),
                };
                let mut reply = vec![0; 24];
                store32(&mut reply, 0, 0x12345678); // Opaque prefix is not an opcode.
                store32(&mut reply, 12, self.reply_word3);
                store16(
                    &mut reply,
                    16,
                    (24 + body.len()) as u16 + u16::from(self.malformed_reply_length),
                );
                store16(
                    &mut reply,
                    18,
                    transaction.wrapping_add(self.reply_transaction_delta),
                );
                store32(
                    &mut reply,
                    20,
                    self.reply_alias.unwrap_or(match command {
                        0x1000f => 0x4001000f,
                        0x20000 => 0x40020000,
                        0x20001 => 0x40020001,
                        _ => command,
                    }),
                );
                reply.extend_from_slice(&body);
                if command == 0x20000 && self.connection_event_before_ack {
                    let mut event = vec![0; 90];
                    let ssid_bytes = packet[24 + 159 + 6] as usize;
                    event[..ssid_bytes]
                        .copy_from_slice(&packet[24 + 159 + 7..24 + 159 + 7 + ssid_bytes]);
                    store32(&mut event, 32, ssid_bytes as u32);
                    event[36..42].copy_from_slice(&packet[25..31]);
                    self.deliver(2, &management_event(0x80000000, transaction, &event));
                }
                self.deliver(2, &reply);
                if command == 0x20001 && !self.drop_disconnect_event {
                    self.deliver(2, &management_event(0x80000004, transaction, &[]));
                }
            }
        }
    }
    fn complete(&mut self, number: u16, cookie: u16, length: usize) {
        let (ring, entries) = self.rings[&(2, number)];
        let offset = 0x78 + number as usize * 2;
        let producer = load16(&self.arena, offset);
        let next = (producer + 1) % entries;
        assert_ne!(next, load16(&self.arena, 0x3cc + number as usize * 2));
        store32(
            &mut self.arena,
            ring + producer as usize * 16,
            u32::from(number) << 16 | 0x401,
        );
        store32(
            &mut self.arena,
            ring + producer as usize * 16 + 4,
            (length as u32) << 16 | u32::from(cookie),
        );
        store32(&mut self.arena, ring + producer as usize * 16 + 8, 0);
        store32(&mut self.arena, ring + producer as usize * 16 + 12, 0);
        store16(&mut self.arena, offset, next);
    }
    fn deliver(&mut self, number: u16, packet: &[u8]) {
        let (ring, entries) = self.rings[&(1, number)];
        let consumed = load16(&self.arena, 0x24c + number as usize * 2);
        let producer = load16(&self.arena, 0x5a0 + number as usize * 2);
        assert_ne!(consumed, producer, "firmware exhausted posted RX buffers");
        let descriptor = ring + consumed as usize * 16;
        let pointer = load64(&self.arena, descriptor + 4);
        let cookie = load32(&self.arena, descriptor + 12);
        assert_eq!(cookie, u32::from(consumed));
        assert!(packet.len() <= (load32(&self.arena, descriptor) >> 8) as usize);
        let offset = usize::try_from(pointer - self.arena_dma).unwrap();
        self.arena[offset..offset + packet.len()].copy_from_slice(packet);
        self.complete(number, cookie as u16, packet.len());
        store16(
            &mut self.arena,
            0x24c + number as usize * 2,
            (consumed + 1) % entries,
        );
    }
}
fn load16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}
fn load32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}
fn load64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}
fn store16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn store32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn synthetic_reply(command: u32) -> Option<Vec<u8>> {
    let input = include_bytes!(env!("CENTAURI_WIFI_TEST_REPLIES"));
    let mut offset = 0;
    while offset < input.len() {
        let opcode = load32(input, offset);
        let size = load32(input, offset + 4) as usize;
        if opcode == command {
            return Some(input[offset + 8..offset + 8 + size].to_vec());
        }
        offset += 8 + size;
    }
    None
}
#[no_mangle]
unsafe extern "C" fn centauri_boot_read(bar: *mut c_void, offset: usize) -> u32 {
    let bus = unsafe { &mut *bar.cast::<Bus>() };
    match offset {
        8 => bus.fw,
        12 => bus.ipc,
        16 => bus.context_address as u32,
        20 => (bus.context_address >> 32) as u32,
        0x18 | 0x1c | 0x20 => bus
            .writes
            .iter()
            .rev()
            .find(|(a, _)| *a == offset)
            .map(|(_, v)| *v)
            .unwrap_or(0),
        _ => panic!("unexpected read {offset:x}"),
    }
}
#[no_mangle]
unsafe extern "C" fn centauri_boot_write(bar: *mut c_void, offset: usize, value: u32) {
    let bus = unsafe { &mut *bar.cast::<Bus>() };
    assert!(bus.master > 0);
    bus.writes.push((offset, value));
    if offset == 16 {
        bus.context_address = bus.context_address & !0xffffffff | u64::from(value);
    }
    if offset == 20 {
        bus.context_address = bus.context_address & 0xffffffff | u64::from(value) << 32;
    }
    if offset == NOTIFY {
        bus.notify();
    }
    if offset == 0x1030 {
        bus.data_notify();
    }
    if offset == HANDSHAKE && value <= bus.handshake {
        if value == 2 && bus.vectors == 0 {
            bus.fw = 3;
        } else {
            bus.ipc = value;
        }
    }
}
#[no_mangle]
extern "C" fn centauri_control_dma_wmb() {
    fence(Ordering::SeqCst);
}
#[no_mangle]
extern "C" fn centauri_control_dma_rmb() {
    fence(Ordering::SeqCst);
}
#[no_mangle]
extern "C" fn centauri_boot_sleep() {
    SLEEPS.with(|count| count.set(count.get() + 1));
}
#[no_mangle]
extern "C" fn centauri_boot_time_us() -> u64 {
    0x123456789abcdef0
}
#[no_mangle]
unsafe extern "C" fn centauri_radio_enable_dma(owner: *mut c_void) {
    unsafe { &mut *owner.cast::<Bus>() }.master += 1;
}
#[no_mangle]
unsafe extern "C" fn centauri_radio_enable_irqs(owner: *mut c_void, count: u32) -> i32 {
    let bus = unsafe { &mut *owner.cast::<Bus>() };
    if bus.irq_error < 0 {
        return bus.irq_error;
    }
    bus.vectors = count;
    0
}
#[no_mangle]
unsafe extern "C" fn centauri_radio_config_read(
    owner: *mut c_void,
    offset: u16,
    value: *mut u32,
) -> i32 {
    let bus = unsafe { &mut *owner.cast::<Bus>() };
    assert_eq!(offset, 0xf90);
    unsafe { value.write(bus.prior_resource) };
    0
}
#[no_mangle]
unsafe extern "C" fn centauri_radio_config_write(
    owner: *mut c_void,
    offset: u16,
    value: u32,
) -> i32 {
    let bus = unsafe { &mut *owner.cast::<Bus>() };
    assert!(bus.master > 0);
    if bus.config_error == Some(offset) {
        return -5;
    }
    bus.config.push((offset, value));
    if offset == 0xf90 && bus.fw_ready {
        bus.fw = 2;
    }
    0
}
#[test]
fn above_four_gib_context_matches_python_and_publishes_only_owned_window() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    assert_eq!(bus.master, 1);
    assert_eq!(bus.vectors, 16);
    assert_eq!(bus.ipc, 2);
    assert_eq!(
        &bus.arena[..0x960],
        include_bytes!(env!("CENTAURI_WIFI_CONTEXT"))
    );
    assert!(bus.arena[0x960..].iter().all(|v| *v == 0));
    assert_eq!(
        bus.config,
        [
            (0xff8, 0x9abcdef0),
            (0xffc, 0x12345678),
            (0xf88, 0xffc00000),
            (0xf8c, 0),
            (0xf90, 0x200000)
        ]
    );
    assert_eq!(
        bus.writes,
        [
            (HANDSHAKE, 1),
            (0x18, 0xffc00000),
            (0x1c, 0),
            (0x20, 0xe00000),
            (0x10, 0),
            (0x14, 1),
            (HANDSHAKE, 2)
        ]
    );
}
#[test]
fn stale_resource_or_ipc_is_rejected_without_dma_or_memory_writes() {
    for used in 0..2 {
        let mut bus = Bus::new();
        if used == 0 {
            bus.prior_resource = 1;
        } else {
            bus.ipc = 1;
        }
        assert_eq!(bus.start(), -16);
        assert_eq!(bus.master, 0);
        assert!(bus.config.is_empty() && bus.writes.is_empty());
        assert!(bus.arena.iter().all(|v| *v == 0xa5));
    }
}
#[test]
fn invalid_dma_extent_overlap_and_window_width_are_rejected_before_enable() {
    for variant in 0..7 {
        let mut bus = Bus::new();
        match variant {
            0 => bus.resource_dma += 1,
            1 => bus.resource_bytes = 0,
            2 => bus.arena_dma = bus.resource_dma,
            3 => bus.arena_dma = 1 << 36,
            4 => bus.arena_dma = 0x500000000,
            5 => {
                bus.arena.pop();
            }
            6 => bus.resource_dma = 0x100000000,
            _ => unreachable!(),
        }
        assert_eq!(bus.start(), -22);
        assert_eq!(bus.master, 0);
        assert!(bus.config.is_empty() && bus.writes.is_empty());
        assert!(bus.arena.iter().all(|v| *v == 0xa5));
    }
}
#[test]
fn config_failure_does_not_publish_context_or_claim_ready() {
    let mut bus = Bus::new();
    bus.config_error = Some(0xf8c);
    assert_eq!(bus.start(), -5);
    assert_eq!(bus.master, 1); // The C error path must stop and drain this.
    assert!(bus.writes.is_empty());
    assert_eq!(bus.ipc, 0);
}
#[test]
fn irq_allocation_failure_prevents_dma_and_publication() {
    let mut bus = Bus::new();
    bus.irq_error = -28;
    assert_eq!(bus.start(), -28);
    assert_eq!(bus.master, 0);
    assert!(bus.config.is_empty() && bus.writes.is_empty());
    assert!(bus.arena.iter().all(|v| *v == 0xa5));
}
#[test]
fn firmware_timeout_precedes_context_publication() {
    let mut bus = Bus::new();
    bus.fw_ready = false;
    assert_eq!(bus.start(), -110);
    assert!(bus.writes.is_empty());
    assert_eq!(SLEEPS.with(Cell::get), 600);
}
#[test]
fn failed_handshake_never_claims_success() {
    for handshake in [0, 1] {
        let mut bus = Bus::new();
        bus.handshake = handshake;
        assert_eq!(bus.start(), -110);
        assert_eq!(SLEEPS.with(Cell::get), 600);
        if handshake == 0 {
            assert_eq!(bus.writes, [(HANDSHAKE, 1)]);
        }
        assert_ne!(bus.ipc, 2);
    }
}

struct QueueOwner(*mut c_void);
impl Drop for QueueOwner {
    fn drop(&mut self) {
        unsafe { centauri_wifi_queues::centauri_wifi_queues_destroy(self.0) };
    }
}

#[test]
fn cold_basic_geometry_frames_and_three_queries_match_python() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut output = ptr::null_mut();
    let arena = bus.arena.as_mut_ptr();
    let bar = ptr::from_mut(&mut bus).cast();
    assert_eq!(
        unsafe {
            centauri_wifi_queues::centauri_wifi_queues_create(
                bar,
                arena,
                bus.arena_dma,
                bus.arena.len(),
                &mut output,
            )
        },
        0
    );
    assert!(!output.is_null());
    let _owner = QueueOwner(output);
    let expected = include_bytes!(env!("CENTAURI_WIFI_ADMIN_FRAMES"));
    let mut offset = 0;
    assert_eq!(bus.admin_requests.len(), 12);
    for frame in &bus.admin_requests {
        let bytes = load32(expected, offset) as usize;
        assert_eq!(frame, &expected[offset + 4..offset + 4 + bytes]);
        offset += 4 + bytes;
    }
    assert_eq!(offset, expected.len());
    assert_eq!(
        bus.management_requests,
        [(8, 1, vec![]), (10, 2, vec![]), (47, 3, vec![1])]
    );
    assert_eq!(load16(&bus.arena, 0x78), 12);
    assert_eq!(load16(&bus.arena, 0x3cc), 12);
    let profiles = include_bytes!(env!("CENTAURI_WIFI_PROFILES"));
    for record in profiles.chunks_exact(24) {
        let number = load32(record, 0) as u16;
        let entries = load32(record, 4) as u16;
        let capacity = load32(record, 12);
        let target = load32(record, 16) as u16;
        let pool = load32(record, 20) as u64;
        let (ring, actual_entries) = bus.rings[&(1, number)];
        assert_eq!(actual_entries, entries);
        assert_eq!(bus.rings[&(2, number)].1, load32(record, 8) as u16);
        let consumed = load16(&bus.arena, 0x24c + number as usize * 2);
        let posted = load16(&bus.arena, 0x5a0 + number as usize * 2);
        assert_eq!((posted + entries - consumed) % entries, target);
        if target != 0 {
            // The producer points one past posted entries; inspect the last
            // live descriptor rather than a previously completed payload.
            let slot = (posted + entries - 1) % entries;
            assert_eq!(
                load32(&bus.arena, ring + slot as usize * 16),
                capacity << 8 | 1
            );
            assert_eq!(
                load64(&bus.arena, ring + slot as usize * 16 + 4),
                bus.arena_dma + pool + u64::from(slot) * u64::from(capacity)
            );
        }
    }
}

#[test]
fn management_requests_return_changing_data_through_both_ring_wraps() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    for sequence in 0..300u32 {
        let body = sequence.to_le_bytes();
        assert_eq!(state.query(0x1234, &body, 0x1234).unwrap(), body);
    }
    assert_eq!(bus.management_requests.len(), 300);
    assert_eq!(load16(&bus.arena, 0x24e), 300 % 128);
    assert_eq!(load16(&bus.arena, 0x3ce), 300 % 128);
    assert_eq!(load16(&bus.arena, 0x250), 300 % 154);
    assert_eq!(load16(&bus.arena, 0x3d0), 300 % 154);
    assert_eq!(load16(&bus.arena, 0x5a4), (153 + 300) % 154);
}

#[test]
fn admin_transport_wraps_without_overwriting_an_outstanding_frame() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let state = bus.queues();
    let fixture = include_bytes!(env!("CENTAURI_WIFI_ADMIN_FRAMES"));
    let frame = &fixture[4..4 + load32(fixture, 0) as usize];
    for _ in 0..20 {
        state.admin(frame).unwrap();
    }
    assert_eq!(bus.admin_requests.len(), 20);
    assert_eq!(load16(&bus.arena, 0x78), 4);
    assert_eq!(load16(&bus.arena, 0x24c), 4);
    assert_eq!(load16(&bus.arena, 0x3cc), 3);
    bus.admin_complete = false;
    assert!(state.admin(frame).is_err());
    let before = bus.arena[0x10000..0x10800].to_vec();
    assert_eq!(state.admin(frame).unwrap_err().to_errno(), -16);
    assert_eq!(&bus.arena[0x10000..0x10800], before);
}

#[test]
fn sq_consumption_without_cq_does_not_reuse_firmware_owned_buffers() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    let (ring, _) = bus.rings[&(1, 3)];
    let before = bus.arena[ring..ring + 96 * 16].to_vec();
    store16(&mut bus.arena, 0x24c + 3 * 2, 95);
    assert_eq!(state.poll(128).unwrap(), 0);
    assert_eq!(load16(&bus.arena, 0x5a0 + 3 * 2), 95);
    assert_eq!(&bus.arena[ring..ring + 96 * 16], before);
}

#[test]
fn out_of_order_completions_release_only_their_own_cookies() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    store16(&mut bus.arena, 0x24c + 3 * 2, 95);
    for (cookie, expected_producer) in [(73, 0), (70, 0), (0, 1)] {
        bus.complete(3, cookie, 1);
        assert_eq!(state.poll(128).unwrap(), 1);
        assert_eq!(load16(&bus.arena, 0x5a0 + 3 * 2), expected_producer);
    }
}

#[test]
fn invalid_completion_extent_source_and_ownership_preserve_host_cursors() {
    for variant in 0..4 {
        let mut bus = Bus::new();
        assert_eq!(bus.start(), 0);
        let mut state = bus.queues();
        state.configure().unwrap();
        let cookie = if variant == 0 {
            96
        } else if variant == 1 {
            95
        } else {
            0
        };
        bus.complete(3, cookie, if variant == 2 { 4097 } else { 1 });
        let (ring, _) = bus.rings[&(2, 3)];
        if variant == 3 {
            store32(&mut bus.arena, ring, 0x40401);
        }
        let before = bus.arena[ring..ring + 16].to_vec();
        assert_eq!(state.poll(128).unwrap_err().to_errno(), -5);
        assert_eq!(load16(&bus.arena, 0x3cc + 3 * 2), 0);
        assert_eq!(load16(&bus.arena, 0x5a0 + 3 * 2), 95);
        assert_eq!(&bus.arena[ring..ring + 16], before);
    }
}

#[test]
fn wrong_transaction_times_out_without_replacing_the_outstanding_request() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    bus.reply_transaction_delta = 1;
    assert_eq!(state.query(10, &[], 10).unwrap_err().to_errno(), -110);
    assert_eq!(SLEEPS.with(Cell::get), 1000);
    assert_eq!(state.query(10, &[], 10).unwrap_err().to_errno(), -16);
    assert_eq!(bus.management_requests.len(), 1);
}

#[test]
fn explicit_reply_alias_and_error_status_match_the_same_transaction() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    bus.reply_alias = Some(0x4001000f);
    assert_eq!(
        state.query(0x1000f, b"request", 0x4001000f).unwrap(),
        b"request"
    );
    bus.reply_alias = Some(0x1000f);
    bus.reply_word3 = 1;
    assert_eq!(
        state
            .query(0x1000f, &[], 0x4001000f)
            .unwrap_err()
            .to_errno(),
        -5
    );
}

#[test]
fn corrupt_admin_completion_stops_before_posting_any_receive_buffers() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    bus.admin_cookie_error = true;
    assert_eq!(state.configure().unwrap_err().to_errno(), -5);
    assert_eq!(bus.admin_requests.len(), 1);
    for number in 1..=6 {
        assert_eq!(load16(&bus.arena, 0x5a0 + number * 2), 0);
    }
}

#[test]
fn management_length_must_match_the_completed_extent() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    bus.malformed_reply_length = true;
    assert_eq!(state.query(10, &[], 10).unwrap_err().to_errno(), -5);
    assert_eq!(load16(&bus.arena, 0x3d0), 0);
}

#[test]
fn short_unsolicited_management_buffers_remain_opaque() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    bus.deliver(2, b"abc");
    assert_eq!(state.poll(128).unwrap(), 1);
    assert_eq!(load16(&bus.arena, 0x3d0), 1);
}

#[test]
fn generated_interface_address_activation_and_stop_follow_the_qualified_protocol() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    let address = [2, 0x11, 0x22, 0x33, 0x44, 0x55];
    state.start_interface(&address).unwrap();
    assert_eq!(bus.interface_state, 2);
    assert_eq!(bus.interface_address, address);
    assert_eq!(
        bus.management_requests
            .iter()
            .map(|r| r.0)
            .collect::<Vec<_>>(),
        [3, 20, 1, 2, 20]
    );
    assert_eq!(bus.management_requests[2].2, address);
    state.stop_interface().unwrap();
    assert_eq!(bus.interface_state, 0);
    assert_eq!(
        bus.management_requests
            .iter()
            .map(|r| r.0)
            .collect::<Vec<_>>(),
        [3, 20, 1, 2, 20, 3, 20]
    );
}

#[test]
fn interface_rejects_invalid_addresses_before_sending_any_request() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    for address in [[0; 6], [1, 2, 3, 4, 5, 6], [255; 6]] {
        assert_eq!(state.start_interface(&address).unwrap_err().to_errno(), -22);
    }
    assert!(bus.management_requests.is_empty());
}

#[test]
fn failed_active_address_readback_still_leaves_a_stoppable_interface() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    bus.corrupt_interface_address = true;
    assert_eq!(
        state
            .start_interface(&[2, 1, 2, 3, 4, 5])
            .unwrap_err()
            .to_errno(),
        -5
    );
    assert_eq!(bus.interface_state, 2);
    state.stop_interface().unwrap();
    assert_eq!(bus.interface_state, 0);
}

#[test]
fn a_service_budget_preserves_unread_completions_until_the_next_drain() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    store16(&mut bus.arena, 0x24c + 3 * 2, 95);
    for cookie in 0..95 {
        bus.complete(3, cookie, 1);
    }
    let mut completed = 0;
    while completed < 95 {
        let done = state.poll(16).unwrap();
        assert_eq!(done, 16.min(95 - completed));
        completed += done;
        assert_eq!(load16(&bus.arena, 0x3cc + 3 * 2), completed as u16);
    }
    assert_eq!(state.poll(16).unwrap(), 0);
    assert_eq!(load16(&bus.arena, 0x5a0 + 3 * 2), 94);
}

#[test]
fn normal_interface_replies_preserve_command_specific_nonzero_word3() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    bus.reply_word3 = 0x01430000;
    assert_eq!(state.query(20, &[], 20).unwrap()[1], 0);
    state.start_interface(&[2, 1, 2, 3, 4, 5]).unwrap();
    assert_eq!(bus.interface_state, 2);
    state.stop_interface().unwrap();
    assert_eq!(bus.interface_state, 0);
}

#[derive(Default)]
struct ScanResults {
    frames: Vec<(u16, i8, Vec<u8>)>,
    completions: Vec<bool>,
}
#[no_mangle]
unsafe extern "C" fn centauri_radio_scan_frame(
    owner: *mut c_void,
    frequency: u16,
    signal: i8,
    frame: *const u8,
    bytes: usize,
) {
    let results = unsafe { &mut *owner.cast::<ScanResults>() };
    results.frames.push((
        frequency,
        signal,
        unsafe { std::slice::from_raw_parts(frame, bytes) }.to_vec(),
    ));
}
#[no_mangle]
unsafe extern "C" fn centauri_radio_scan_done(owner: *mut c_void, aborted: bool) {
    unsafe { &mut *owner.cast::<ScanResults>() }
        .completions
        .push(aborted);
}
fn management_event(command: u32, transaction: u16, body: &[u8]) -> Vec<u8> {
    let mut packet = vec![0; 24];
    store16(&mut packet, 16, (24 + body.len()) as u16);
    store16(&mut packet, 18, transaction);
    store32(&mut packet, 20, command);
    packet.extend_from_slice(body);
    packet
}
fn scan_completion_packet(cookie: u8, frames: u16, status: u16) -> Vec<u8> {
    let mut body = [0; 9];
    store16(&mut body, 0, status);
    store16(&mut body, 2, frames);
    store32(&mut body, 4, 57);
    body[8] = cookie;
    management_event(0x80000005, 0x1234, &body)
}
#[test]
fn scan_request_matches_independent_python_and_rejects_unqualified_channel_words() {
    let mut body = [0; centauri_wifi_scan::MAX_REQUEST];
    let length = centauri_wifi_scan::request(9, &[1, 6, 11], &mut body).unwrap();
    assert_eq!(&body[..length], include_bytes!(env!("CENTAURI_WIFI_SCAN")));
    for channels in [&[][..], &[0], &[12], &[0x205], &[1; 12]] {
        assert_eq!(
            centauri_wifi_scan::request(9, channels, &mut body)
                .unwrap_err()
                .to_errno(),
            -22
        );
    }
}
#[test]
fn scan_radio_frame_uses_declared_header_length_and_strips_only_declared_fcs() {
    let body = include_bytes!(env!("CENTAURI_WIFI_SCAN_FRAME"));
    let frame = centauri_wifi_scan::frame(body).unwrap();
    assert_eq!(frame.frequency, 2412);
    assert_eq!(frame.signal_dbm, -59);
    assert_eq!(frame.bytes, &body[96..body.len() - 4]);
    for length in 0..body.len() - 1 {
        // A few prefixes may form a syntactically valid shorter IE list;
        // bounded acceptance is fine, but no prefix may panic.
        let _ = centauri_wifi_scan::frame(&body[..length]);
    }
    let mut bad = body.to_vec();
    bad[16] |= 0x40;
    assert!(centauri_wifi_scan::frame(&bad).is_err());
    let mut bad = body.to_vec();
    store16(&mut bad, 2, 22);
    assert!(centauri_wifi_scan::frame(&bad).is_err());
    let mut bad = body.to_vec();
    bad[96 + 36 + 1] = 33;
    assert!(centauri_wifi_scan::frame(&bad).is_err());
}
#[test]
fn scan_completion_waits_for_queue4_reports_even_when_control_event_arrives_first() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    state.start_interface(&[2, 1, 2, 3, 4, 5]).unwrap();
    let mut results = ScanResults::default();
    let owner = (&mut results as *mut ScanResults).cast();
    state.start_scan(owner, 9, &[1, 6, 11]).unwrap();
    assert_eq!(
        bus.management_requests.last().unwrap().2,
        include_bytes!(env!("CENTAURI_WIFI_SCAN"))
    );
    bus.deliver(2, &scan_completion_packet(9, 2, 0));
    bus.deliver(
        4,
        &management_event(0, 9, include_bytes!(env!("CENTAURI_WIFI_SCAN_FRAME"))),
    );
    assert_eq!(state.poll(1).unwrap(), 1); // completion first; budget excludes reports
    assert!(results.completions.is_empty());
    state.poll(128).unwrap();
    assert_eq!(results.frames.len(), 1);
    assert!(results.completions.is_empty());
    bus.deliver(
        4,
        &management_event(0, 9, include_bytes!(env!("CENTAURI_WIFI_SCAN_FRAME"))),
    );
    state.poll(128).unwrap();
    assert_eq!(results.frames.len(), 2);
    assert_eq!(results.completions, vec![false]);
    state.poll(128).unwrap();
    assert_eq!(results.completions.len(), 1);
}
#[test]
fn wrong_scan_cookie_is_ignored_and_bad_rf_frame_does_not_stop_queue_service() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    state.start_interface(&[2, 1, 2, 3, 4, 5]).unwrap();
    let mut results = ScanResults::default();
    let owner = (&mut results as *mut ScanResults).cast();
    state.start_scan(owner, 7, &[11]).unwrap();
    bus.deliver(2, &scan_completion_packet(6, 0, 0));
    bus.deliver(
        4,
        &management_event(0, 6, include_bytes!(env!("CENTAURI_WIFI_SCAN_FRAME"))),
    );
    bus.deliver(4, &management_event(0, 7, &[0; 10]));
    state.poll(128).unwrap();
    assert!(results.frames.is_empty());
    assert!(results.completions.is_empty());
    bus.deliver(2, &scan_completion_packet(7, 1, 0));
    state.poll(128).unwrap();
    assert_eq!(results.completions, vec![false]);
    assert_eq!(bus.fw, 2);
}
#[test]
fn detached_scan_fences_reuse_until_real_completion_without_late_callbacks() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    state.start_interface(&[2, 1, 2, 3, 4, 5]).unwrap();
    let mut results = ScanResults::default();
    let owner = (&mut results as *mut ScanResults).cast();
    state.start_scan(owner, 1, &[1]).unwrap();
    state.abort_scan();
    assert_eq!(
        state.start_scan(owner, 2, &[1]).unwrap_err().to_errno(),
        -16
    );
    bus.deliver(
        4,
        &management_event(0, 1, include_bytes!(env!("CENTAURI_WIFI_SCAN_FRAME"))),
    );
    bus.deliver(2, &scan_completion_packet(1, 1, 0));
    state.poll(128).unwrap();
    assert!(results.frames.is_empty());
    assert!(results.completions.is_empty());
    state.start_scan(owner, 2, &[1]).unwrap();
    bus.deliver(2, &scan_completion_packet(2, 0, 1));
    state.poll(128).unwrap();
    assert_eq!(results.completions, vec![true]);
}

#[test]
fn country_requests_validate_ascii_and_verify_firmware_readback() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    let raw = ptr::from_mut(&mut state).cast();
    assert_eq!(
        unsafe { centauri_wifi_queues::centauri_wifi_set_country(raw, b"00".as_ptr()) },
        -22
    );
    assert!(bus.management_requests.is_empty());
    assert_eq!(
        unsafe { centauri_wifi_queues::centauri_wifi_set_country(raw, b"US".as_ptr()) },
        0
    );
    assert_eq!(bus.management_requests[0].0, 9);
    assert_eq!(bus.management_requests[0].2, b"US\x01");
    assert_eq!(bus.management_requests[1].0, 10);
    // The independent model's country reply is deliberately fixed to US/1.
    assert_eq!(
        unsafe { centauri_wifi_queues::centauri_wifi_set_country(raw, b"CA".as_ptr()) },
        -5
    );
}
#[test]
fn radio_presence_extensions_preserve_alignment_and_duplicate_ssids_are_rejected() {
    let body = include_bytes!(env!("CENTAURI_WIFI_SCAN_FRAME"));
    let mut extended = body[..8].to_vec();
    let present = load32(body, 4) | (1 << 31);
    store32(&mut extended, 4, present);
    // One extension plus padding keeps TSFT at its required eight-byte boundary.
    extended.extend_from_slice(&[0; 8]);
    extended.extend_from_slice(&body[8..]);
    store16(&mut extended, 2, 104);
    let decoded = centauri_wifi_scan::frame(&extended).unwrap();
    assert_eq!(decoded.frequency, 2412);
    assert_eq!(decoded.signal_dbm, -59);
    assert_eq!(decoded.bytes, &body[96..body.len() - 4]);
    let mut duplicate = body[..body.len() - 4].to_vec();
    duplicate.extend_from_slice(&[0, 0]);
    duplicate.extend_from_slice(&[0; 4]);
    assert!(centauri_wifi_scan::frame(&duplicate).is_err());
}

#[derive(Default)]
struct ConnectionResults {
    outcomes: Vec<([u8; 6], u16)>,
}
#[no_mangle]
unsafe extern "C" fn centauri_radio_connect_done(
    owner: *mut c_void,
    bssid: *const u8,
    status: u16,
) {
    let results = unsafe { &mut *owner.cast::<ConnectionResults>() };
    results
        .outcomes
        .push((unsafe { *bssid.cast::<[u8; 6]>() }, status));
}
fn connection_parameters() -> centauri_wifi_connection::Parameters {
    let mut input = centauri_wifi_connection::Parameters {
        ssid: [0; 32],
        ssid_bytes: 12,
        bssid: [2, 0x11, 0x22, 0x33, 0x44, 0x55],
        channel: 11,
        pmk: [0; 32],
        rsn: [0; 255],
        rsn_bytes: 20,
        security: centauri_wifi_connection::SECURITY_PSK,
        password: [0; 63],
        password_bytes: 0,
        h2e_only: 0,
    };
    input.ssid[..12].copy_from_slice(b"test-network");
    for (i, v) in input.pmk.iter_mut().enumerate() {
        *v = i as u8;
    }
    let fixture = include_bytes!(env!("CENTAURI_WIFI_CONNECTION"));
    input.rsn[..20].copy_from_slice(&fixture[fixture.len() - 20..]);
    input
}
fn sae_parameters() -> centauri_wifi_connection::Parameters {
    let mut input = connection_parameters();
    input.security = centauri_wifi_connection::SECURITY_SAE;
    input.pmk.fill(0);
    let password = b"synthetic-test-password";
    input.password[..password.len()].copy_from_slice(password);
    input.password_bytes = password.len() as u32;
    let fixture = include_bytes!(env!("CENTAURI_WIFI_SAE_CONNECTION"));
    // RSN follows the fixed prefix and single SSID TLV.
    input.rsn[..20].copy_from_slice(&fixture[182..202]);
    input
}
fn connection_event(
    input: &centauri_wifi_connection::Parameters,
    transaction: u16,
    status: u16,
) -> Vec<u8> {
    let mut body = [0; 90];
    body[..input.ssid_bytes as usize].copy_from_slice(&input.ssid[..input.ssid_bytes as usize]);
    store32(&mut body, 32, input.ssid_bytes);
    body[36..42].copy_from_slice(&input.bssid);
    store16(&mut body, 42, status);
    management_event(0x80000000, transaction, &body)
}
#[test]
fn pmk_only_connection_matches_python_without_plaintext_or_opaque_record() {
    assert_eq!(
        std::mem::size_of::<centauri_wifi_connection::Parameters>(),
        412
    );
    let input = connection_parameters();
    let request = centauri_wifi_connection::Request::new(&input).unwrap();
    assert_eq!(
        request.bytes(),
        include_bytes!(env!("CENTAURI_WIFI_CONNECTION"))
    );
    assert_eq!(&request.bytes()[70..76], &[0, 4, 4, 0, 32, 0]);
    assert_eq!(&request.bytes()[76..108], &(0..32u8).collect::<Vec<_>>());
}
#[test]
fn sae_matches_qualified_python_without_a_precomputed_or_retained_pmk() {
    let input = sae_parameters();
    let request = centauri_wifi_connection::Request::new(&input).unwrap();
    assert_eq!(
        request.bytes(),
        include_bytes!(env!("CENTAURI_WIFI_SAE_CONNECTION"))
    );
    assert_eq!(&request.bytes()[70..76], &[3, 8, 4, 0, 32, 0]);
    assert!(request.bytes()[76..108].iter().all(|byte| *byte == 0));
    for length in [0, 64, u32::MAX] {
        let mut invalid = sae_parameters();
        invalid.password_bytes = length;
        assert!(centauri_wifi_connection::Request::new(&invalid).is_err());
    }
    let mut invalid = sae_parameters();
    invalid.pmk[0] = 1;
    assert!(centauri_wifi_connection::Request::new(&invalid).is_err());
    let mut invalid = connection_parameters();
    invalid.password_bytes = 1;
    assert!(centauri_wifi_connection::Request::new(&invalid).is_err());
    let mut invalid = connection_parameters();
    invalid.security = u32::MAX;
    assert!(centauri_wifi_connection::Request::new(&invalid).is_err());
}
#[test]
fn sae_transport_wipes_password_from_the_completed_dma_submission_slot() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    state.start_interface(&[2, 1, 2, 3, 4, 5]).unwrap();
    let input = sae_parameters();
    let mut results = ConnectionResults::default();
    let slot = load16(&bus.arena, 0x5a2);
    state
        .start_connection(ptr::from_mut(&mut results).cast(), &input)
        .unwrap();
    let profiles = include_bytes!(env!("CENTAURI_WIFI_PROFILES"));
    let profile = profiles
        .chunks_exact(24)
        .find(|p| load32(p, 0) == 1)
        .unwrap();
    let tx_pool = load32(profile, 20) as usize;
    assert!(
        bus.arena[tx_pool + slot as usize * 8192..tx_pool + (slot as usize + 1) * 8192]
            .iter()
            .all(|byte| *byte == 0)
    );
    let (_, transaction, _) = bus.management_requests.last().unwrap();
    bus.deliver(2, &connection_event(&input, *transaction, 0));
    state.poll(128).unwrap();
    assert_eq!(results.outcomes, vec![(input.bssid, 0)]);
}
#[test]
fn connection_ack_waits_for_matching_transaction_ssid_and_bssid_and_wipes_returned_key_slot() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    state.start_interface(&[2, 1, 2, 3, 4, 5]).unwrap();
    let input = connection_parameters();
    let mut results = ConnectionResults::default();
    let owner = ptr::from_mut(&mut results).cast();
    let slot = load16(&bus.arena, 0x5a2);
    state.start_connection(owner, &input).unwrap();
    assert!(results.outcomes.is_empty());
    let (_, transaction, _) = bus.management_requests.last().unwrap();
    let transaction = *transaction;
    let profiles = include_bytes!(env!("CENTAURI_WIFI_PROFILES"));
    let profile = profiles
        .chunks_exact(24)
        .find(|p| load32(p, 0) == 1)
        .unwrap();
    let tx_pool = load32(profile, 20) as usize;
    assert!(
        bus.arena[tx_pool + slot as usize * 8192..tx_pool + (slot as usize + 1) * 8192]
            .iter()
            .all(|b| *b == 0)
    );
    bus.deliver(2, &connection_event(&input, transaction + 1, 0));
    let mut wrong = connection_event(&input, transaction, 0);
    wrong[24] ^= 1;
    bus.deliver(2, &wrong);
    let mut wrong = connection_event(&input, transaction, 0);
    wrong[24 + 41] ^= 1;
    bus.deliver(2, &wrong);
    state.poll(128).unwrap();
    assert!(results.outcomes.is_empty());
    bus.deliver(2, &connection_event(&input, transaction, 0));
    state.poll(128).unwrap();
    assert_eq!(results.outcomes, vec![(input.bssid, 0)]);
}
#[test]
fn connection_outcome_before_ack_is_deferred_until_solicited_success() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    state.start_interface(&[2, 1, 2, 3, 4, 5]).unwrap();
    bus.connection_event_before_ack = true;
    let mut results = ConnectionResults::default();
    let input = connection_parameters();
    state
        .start_connection(ptr::from_mut(&mut results).cast(), &input)
        .unwrap();
    assert_eq!(results.outcomes, vec![(input.bssid, 0)]);
}
#[test]
fn malformed_connection_inputs_and_outcomes_cannot_cross_qualified_boundaries() {
    for field in 0..5 {
        let mut input = connection_parameters();
        match field {
            0 => input.ssid_bytes = 33,
            1 => input.rsn_bytes = 256,
            2 => input.bssid = [0; 6],
            3 => input.bssid[0] = 3,
            _ => input.channel = 0x205,
        }
        assert!(centauri_wifi_connection::Request::new(&input).is_err());
    }
    let mut prefix = [0; 44];
    store32(&mut prefix, 32, 33);
    assert!(centauri_wifi_connection::outcome(&prefix).is_err());
}

#[test]
fn a_rejected_ack_cannot_publish_an_early_successful_connection_event() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    state.start_interface(&[2, 1, 2, 3, 4, 5]).unwrap();
    bus.connection_event_before_ack = true;
    bus.reply_word3 = 0x01510000;
    let mut results = ConnectionResults::default();
    let error = state
        .start_connection(ptr::from_mut(&mut results).cast(), &connection_parameters())
        .unwrap_err();
    assert_eq!(error.to_errno(), -5);
    assert!(results.outcomes.is_empty());
}

#[test]
fn detached_connection_fences_reuse_and_does_not_publish_late_outcomes() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    state.start_interface(&[2, 1, 2, 3, 4, 5]).unwrap();
    let input = connection_parameters();
    let mut results = ConnectionResults::default();
    let owner = ptr::from_mut(&mut results).cast();
    state.start_connection(owner, &input).unwrap();
    let transaction = bus.management_requests.last().unwrap().1;
    state.detach_connection();
    assert_eq!(
        state
            .start_connection(owner, &input)
            .unwrap_err()
            .to_errno(),
        -16
    );
    bus.deliver(2, &connection_event(&input, transaction, 0));
    state.poll(128).unwrap();
    assert!(results.outcomes.is_empty());
    state.disconnect().unwrap();
    state.start_connection(owner, &input).unwrap();
    let transaction = bus.management_requests.last().unwrap().1;
    let mut failure = connection_event(&input, transaction, 1);
    failure[24..24 + 42].fill(0); // Failed outcomes need not carry a selected BSS.
    bus.deliver(2, &failure);
    state.poll(128).unwrap();
    assert_eq!(results.outcomes, vec![(input.bssid, 1)]);
}

#[test]
fn joined_interface_requires_disconnect_ack_and_event_before_deactivation() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    state.start_interface(&[2, 1, 2, 3, 4, 5]).unwrap();
    bus.connection_event_before_ack = true;
    let mut results = ConnectionResults::default();
    let input = connection_parameters();
    state
        .start_connection(ptr::from_mut(&mut results).cast(), &input)
        .unwrap();
    bus.radio_associated = true;
    state.stop_interface().unwrap();
    let commands: Vec<_> = bus.management_requests.iter().map(|r| r.0).collect();
    assert!(commands.ends_with(&[0x20000, 0x20001, 3, 20]));
    assert_eq!(
        bus.management_requests
            .iter()
            .find(|r| r.0 == 0x20001)
            .unwrap()
            .2,
        include_bytes!(env!("CENTAURI_WIFI_DISCONNECT"))
    );
    assert_eq!(bus.interface_state, 0);
    assert!(!bus.radio_associated);
    state.start_interface(&[2, 1, 2, 3, 4, 5]).unwrap();
    state
        .start_connection(ptr::from_mut(&mut results).cast(), &input)
        .unwrap();
    assert_eq!(results.outcomes, vec![(input.bssid, 0), (input.bssid, 0)]);
}

#[test]
fn missing_disconnect_event_never_marks_an_active_interface_inactive() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut state = bus.queues();
    state.configure().unwrap();
    state.start_interface(&[2, 1, 2, 3, 4, 5]).unwrap();
    bus.connection_event_before_ack = true;
    let mut results = ConnectionResults::default();
    let input = connection_parameters();
    state
        .start_connection(ptr::from_mut(&mut results).cast(), &input)
        .unwrap();
    bus.drop_disconnect_event = true;
    assert_eq!(state.stop_interface().unwrap_err().to_errno(), -110);
    assert_eq!(bus.interface_state, 2);
    let transaction = bus.management_requests.last().unwrap().1;
    bus.deliver(2, &management_event(0x80000004, transaction + 1, &[]));
    state.poll(128).unwrap();
    assert_eq!(bus.interface_state, 2);
    bus.deliver(2, &management_event(0x80000004, transaction, &[]));
    state.stop_interface().unwrap();
    assert_eq!(bus.interface_state, 0);
    assert_eq!(
        bus.management_requests
            .iter()
            .filter(|r| r.0 == 0x20001)
            .count(),
        1
    );
}

#[test]
fn h2e_request_matches_python_and_validates_peer_capability_extent() {
    let mut input = sae_parameters();
    input.h2e_only = 1;
    let request = centauri_wifi_connection::Request::new(&input).unwrap();
    assert_eq!(
        request.bytes(),
        include_bytes!(env!("CENTAURI_WIFI_H2E_CONNECTION"))
    );
    assert_eq!(
        &request.bytes()[request.bytes().len() - 5..],
        &[244, 0, 1, 0, 32]
    );
    assert!(centauri_wifi_connection::rsnxe_h2e(&[32]));
    assert!(centauri_wifi_connection::rsnxe_h2e(&[33, 0]));
    for invalid in [&[][..], &[0][..], &[64][..], &[33][..], &[32, 0][..]] {
        assert!(!centauri_wifi_connection::rsnxe_h2e(invalid));
    }
    input.h2e_only = 2;
    assert!(centauri_wifi_connection::Request::new(&input).is_err());
    input = connection_parameters();
    input.h2e_only = 1;
    assert!(centauri_wifi_connection::Request::new(&input).is_err());
    assert!(!unsafe { centauri_wifi_connection::centauri_wifi_rsnxe_h2e(ptr::null(), 1) });
}

#[test]
fn h2e_metadata_is_peer_checked_and_a_wrong_method_precedes_success_callback() {
    let station = [2, 0x11, 0x22, 0x33, 0x44, 0x55];
    let peer = [2, 0x66, 0x77, 0x88, 0x99, 0xaa];
    let fixture = include_bytes!(env!("CENTAURI_WIFI_SAE_AUTH"));
    let decoded = centauri_wifi_scan::sae_authentication(fixture, &station, &peer).unwrap();
    assert_eq!(
        (decoded.transaction, decoded.status, decoded.group),
        (1, 126, Some(19))
    );
    for h2e in [true, false] {
        let mut bus = Bus::new();
        assert_eq!(bus.start(), 0);
        let mut state = bus.queues();
        state.configure().unwrap();
        state.start_interface(&station).unwrap();
        let mut input = sae_parameters();
        input.bssid = peer;
        input.h2e_only = 1;
        let mut results = ConnectionResults::default();
        state
            .start_connection(ptr::from_mut(&mut results).cast(), &input)
            .unwrap();
        let (_, transaction, _) = bus.management_requests.last().unwrap();
        let transaction = *transaction;
        let mut frame = fixture.to_vec();
        if !h2e {
            frame[96 + 28..96 + 30].fill(0);
        }
        let mut wrong_peer = frame.clone();
        wrong_peer[96 + 10] ^= 1;
        bus.deliver(4, &management_event(0, 0, &wrong_peer));
        assert!(state.poll(128).is_ok());
        assert!(results.outcomes.is_empty());
        bus.deliver(2, &connection_event(&input, transaction, 0));
        bus.deliver(4, &management_event(0, 0, &frame));
        let result = state.poll(128);
        if h2e {
            assert!(result.is_ok());
            assert_eq!(results.outcomes, vec![(peer, 0)]);
        } else {
            assert_eq!(result.unwrap_err().to_errno(), -71);
            assert!(results.outcomes.is_empty());
        }
    }
}
