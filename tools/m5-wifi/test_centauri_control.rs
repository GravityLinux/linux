// SPDX-License-Identifier: GPL-2.0-only
//! Independent firmware-side descriptor model for the real kernel IPC code.
extern crate self as kernel;
#[macro_export]
macro_rules! pr_err { ($($args:tt)*) => { eprintln!($($args)*); }; }
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
    #[derive(Clone, Copy, Debug)]
    pub struct Error(i32);
    impl Error {
        pub fn to_errno(self) -> i32 {
            self.0
        }
    }
    pub type Result<T = ()> = core::result::Result<T, Error>;
    pub const EINVAL: Error = Error(-22);
    pub const EBUSY: Error = Error(-16);
    pub const EIO: Error = Error(-5);
    pub const ETIMEDOUT: Error = Error(-110);
}
#[path = "../../drivers/net/wireless/apple/centauri/centauri_control.rs"]
mod control;
use core::{ffi::c_void, ptr};
use std::sync::atomic::{fence, Ordering};

#[repr(align(16384))]
struct Page([u8; 0x4000]);
struct Bus {
    page: Box<Page>,
    arena: Vec<u8>,
    dma: u64,
    arena_dma: u64,
    ipc: u32,
    image: u32,
    address: u64,
    published: usize,
    requests: Vec<Vec<u8>>,
    posts: Vec<(usize, u32)>,
    writes: Vec<(usize, u32)>,
    corrupt_cookie: bool,
    complete: bool,
    handshake: bool,
    fault: bool,
}
impl Bus {
    fn new() -> Self {
        Self {
            page: Box::new(Page([0xa5; 0x4000])),
            arena: vec![0xa5; 0x200000],
            dma: 0x112340000,
            arena_dma: 0x123000000,
            ipc: 0,
            image: 2,
            address: 0,
            published: 0,
            requests: vec![],
            posts: vec![],
            writes: vec![],
            corrupt_cookie: false,
            complete: true,
            handshake: true,
            fault: false,
        }
    }
    fn start(&mut self) -> i32 {
        let context = self.page.0.as_mut_ptr();
        let arena = self.arena.as_mut_ptr();
        let owner = ptr::from_mut(self).cast::<c_void>();
        // SAFETY: These distinct test allocations/mappings live through start.
        unsafe {
            control::centauri_control_start(
                owner,
                owner,
                context,
                0x4000,
                self.dma,
                arena,
                0x200000,
                self.arena_dma,
            )
        }
    }
    fn transport(&mut self) -> control::Control {
        // SAFETY: Test retains both allocations, serializes every submission.
        unsafe {
            control::Control::from_raw(
                ptr::from_mut(self).cast(),
                self.page.0.as_mut_ptr(),
                self.dma,
            )
        }
    }
    fn service(&mut self) -> Service {
        let bar = ptr::from_mut(self).cast();
        // SAFETY: Start succeeded, resources remain live through this guard.
        let state = unsafe {
            control::centauri_control_service_create(
                bar, self.page.0.as_mut_ptr(), self.dma,
                self.arena.as_mut_ptr(), self.arena_dma,
            )
        };
        assert!(!state.is_null());
        Service(state)
    }
    fn receive(&mut self, queue: usize, count: usize) {
        let (entries, ring) = match queue {
            8 => (4, 0),
            2 => (16, 0x4000),
            _ => panic!("unexpected receive queue"),
        };
        let index = 0x7a + queue * 2;
        let mut consumer = read16(&self.page.0, index) as usize;
        let producer = read16(&self.page.0, 0x96 + queue * 2) as usize;
        for _ in 0..count {
            assert_ne!(consumer, producer, "firmware exhausted posted buffers");
            let descriptor = ring + consumer * 16;
            assert_eq!(read32(&self.arena, descriptor) & 255, 1);
            // Runtime completion tags need not equal the original slot.
            write32(&mut self.arena, descriptor, 64 << 8);
            write32(&mut self.arena, descriptor + 12, 0x04000000 | consumer as u32);
            consumer = (consumer + 1) % entries;
        }
        write16(&mut self.page.0, index, consumer as u16);
    }
    fn complete_request(&mut self, producer: u32) {
        let page = &mut self.page.0;
        let old = read16(page, 0x7a) as usize;
        let entries = read16(page, 0x46) as usize;
        let submission = (read64(page, 0x3c) - self.dma) as usize;
        let completion = (read64(page, 0x34) - self.dma) as usize;
        let record = submission + old * 16;
        let format = read32(page, record);
        let pointer = read64(page, record + 4);
        let cookie = read32(page, record + 12);
        let length = (format >> 8) as usize;
        let Some(offset) = pointer.checked_sub(self.dma).map(|n| n as usize) else {
            self.fault = true;
            return;
        };
        if format & 255 != 1
            || !matches!(length, 4 | 52)
            || offset + length > page.len()
            || cookie != old as u32
            || producer as usize != (old + 1) % entries
            || read16(page, 0x96) != producer as u16
        {
            self.fault = true;
            return;
        }
        self.requests.push(page[offset..offset + length].to_vec());
        if !self.complete {
            return;
        }
        write32(page, completion + old * 16, 0x401);
        write32(
            page,
            completion + old * 16 + 4,
            (length as u32) << 16 | cookie ^ u32::from(self.corrupt_cookie),
        );
        write32(page, completion + old * 16 + 8, 0);
        write32(page, completion + old * 16 + 12, 0);
        write16(page, 0x78, producer as u16);
        write16(page, 0x7a, producer as u16);
    }
}
struct Service(*mut c_void);
impl Service {
    fn poll(&mut self) -> i32 {
        unsafe { control::centauri_control_service_poll(self.0) }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        unsafe { control::centauri_control_service_destroy(self.0) };
    }
}
fn read16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn read32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn read64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn write16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn write32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
#[no_mangle]
unsafe extern "C" fn centauri_boot_read(bar: *mut c_void, offset: usize) -> u32 {
    // SAFETY: Only a live test-owned Bus is passed as BAR.
    let bus = unsafe { &mut *bar.cast::<Bus>() };
    match offset {
        0x8000 => bus.image,
        0x8050 => bus.ipc,
        _ => {
            bus.fault = true;
            0
        }
    }
}
#[no_mangle]
unsafe extern "C" fn centauri_boot_write(bar: *mut c_void, offset: usize, value: u32) {
    // SAFETY: Same test-owned Bus.
    let bus = unsafe { &mut *bar.cast::<Bus>() };
    bus.writes.push((offset, value));
    match offset {
        0x907c => {
            if bus.handshake {
                bus.ipc = value
            }
        }
        0x8054 => bus.address = (bus.address & 0xffffffff00000000) | u64::from(value),
        0x8058 => bus.address = (bus.address & 0xffffffff) | u64::from(value) << 32,
        0x9000 => bus.complete_request(value),
        0x9008 | 0x9010 | 0x9020 => bus.posts.push((offset, value)),
        _ => bus.fault = true,
    }
}
#[no_mangle]
unsafe extern "C" fn centauri_boot_publish_dma(owner: *mut c_void) {
    // SAFETY: Same test-owned Bus.
    unsafe { &mut *owner.cast::<Bus>() }.published += 1;
}
#[no_mangle]
extern "C" fn centauri_boot_sleep() {}
#[no_mangle]
extern "C" fn centauri_control_dma_wmb() {
    fence(Ordering::SeqCst);
}
#[no_mangle]
extern "C" fn centauri_control_dma_rmb() {
    fence(Ordering::SeqCst);
}

#[test]
fn matches_python_context_and_queue_frames_above_four_gib() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    assert!(!bus.fault);
    assert_eq!(bus.address, bus.dma);
    assert_eq!(bus.published, 1);
    let mut expected = include_bytes!(env!("CENTAURI_CONTROL_CONTEXT")).to_vec();
    // Device ownership changes only the four modeled command indices/rings.
    write16(&mut expected, 0x78, 4);
    write16(&mut expected, 0x7a, 4);
    write16(&mut expected, 0x94, 3);
    write16(&mut expected, 0x96, 4);
    write16(&mut expected, 0xa6, 3);
    write16(&mut expected, 0x9a, 15);
    write16(&mut expected, 0x9e, 7);
    assert_eq!(&bus.page.0[..0xb0], &expected[..0xb0]);
    let frames = include_bytes!(env!("CENTAURI_CONTROL_FRAMES"));
    assert_eq!(bus.requests.len(), 4);
    for (index, frame) in bus.requests.iter().enumerate() {
        assert_eq!(frame, &frames[index * 52..(index + 1) * 52]);
    }
    assert_eq!(bus.posts, [(0x9020, 3), (0x9008, 15), (0x9010, 7)]);
    assert_eq!(read64(&bus.arena, 4), bus.arena_dma + 0x18000);
    assert_eq!(read32(&bus.arena, 0), 0x100001);
    assert_eq!(read64(&bus.arena, 0x4004), bus.arena_dma + 0x20000);
    assert_eq!(read32(&bus.arena, 0x4000), 0x800001);
    assert_eq!(read64(&bus.arena, 0x8004), u64::MAX);
    assert_eq!(read32(&bus.arena, 0x8000), 0x40000);
}

#[test]
fn shared_receive_replenishes_partial_batches_and_wraps() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut service = bus.service();
    assert_eq!(service.poll(), 0);
    bus.receive(8, 3);
    bus.receive(2, 4);
    assert_eq!(service.poll(), 7);
    assert_eq!(read16(&bus.page.0, 0xa6), 2);
    assert_eq!(read16(&bus.page.0, 0x9a), 3);
    // Four previously consumed queue2 buffers are reposted from producer15;
    // slots4..14 were still device-owned and must remain untouched.
    assert_eq!(read32(&bus.arena, 0x4000 + 4 * 16), 0x800001);
    assert_eq!(read64(&bus.arena, 0x4000 + 15 * 16 + 4),
               bus.arena_dma + 0x20000 + 15 * 0x8000);
    for _ in 0..20 {
        bus.receive(8, 3);
        bus.receive(2, 15);
        assert_eq!(service.poll(), 18);
        assert_eq!(service.poll(), 0);
    }
    assert!(!bus.fault);
}

#[test]
fn shared_receive_rejects_invalid_cursors_and_changed_host_ownership() {
    for (offset, value) in [(0x8a, 4), (0x7e, 16), (0xa6, 1), (0x9a, 1)] {
        let mut bus = Bus::new();
        assert_eq!(bus.start(), 0);
        let mut service = bus.service();
        write16(&mut bus.page.0, offset, value);
        let before = bus.posts.len();
        assert_eq!(service.poll(), -5);
        assert_eq!(bus.posts.len(), before);
    }
}

#[test]
fn shared_receive_validates_whole_batch_before_returning_buffers() {
    for (field, value) in [(0, 0x8001 << 8), (4, 0x1234), (8, 0x1234)] {
        let mut bus = Bus::new();
        assert_eq!(bus.start(), 0);
        let mut service = bus.service();
        bus.receive(2, 2);
        write32(&mut bus.arena, 0x4000 + 16 + field, value);
        let before = bus.arena.clone();
        let posts = bus.posts.len();
        assert_eq!(service.poll(), -5);
        assert_eq!(bus.posts.len(), posts);
        assert_eq!(bus.arena, before);
        assert_eq!(read16(&bus.page.0, 0x9a), 15);
    }
}

#[test]
fn shared_receive_preserves_inline_controlapp_responses() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let mut service = bus.service();
    write16(&mut bus.page.0, 0x82, 1);
    bus.arena[0x8010..0x8018].copy_from_slice(b"RESPONSE");
    let before = bus.arena.clone();
    let posts = bus.posts.len();
    assert_eq!(service.poll(), 0);
    assert_eq!(bus.posts.len(), posts);
    assert_eq!(bus.arena, before);
    assert_eq!(read16(&bus.page.0, 0x9e), 7);
}
#[test]
fn command_wrap_and_exact_completion_ownership() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    let transport = bus.transport();
    for _ in 0..20 {
        assert!(transport.submit(&0x30003u32.to_le_bytes()).is_ok());
    }
    assert!(!bus.fault);
    assert_eq!(bus.requests.len(), 24);
    assert_eq!(read16(&bus.page.0, 0x78), 8);
    assert_eq!(read16(&bus.page.0, 0x96), 8);
    assert_eq!(read16(&bus.page.0, 0x94), 7);
}
#[test]
fn incomplete_transfer_cannot_be_overwritten() {
    let mut bus = Bus::new();
    assert_eq!(bus.start(), 0);
    bus.complete = false;
    let transport = bus.transport();
    assert_eq!(
        transport.submit(&[3, 0, 3, 0]).unwrap_err().to_errno(),
        -110
    );
    let before = bus.page.0;
    let writes = bus.writes.len();
    assert_eq!(transport.submit(&[3, 0, 4, 0]).unwrap_err().to_errno(), -16);
    assert_eq!(bus.page.0, before);
    assert_eq!(bus.writes.len(), writes);
}
#[test]
fn bad_cookie_stops_before_posting_receive_buffers() {
    let mut bus = Bus::new();
    bus.corrupt_cookie = true;
    assert_eq!(bus.start(), -5);
    assert_eq!(bus.requests.len(), 1);
    assert!(bus.posts.is_empty());
}
#[test]
fn stale_context_rejected_without_dma_or_memory_changes() {
    let mut bus = Bus::new();
    bus.ipc = 2;
    assert_eq!(bus.start(), -16);
    assert_eq!(bus.published, 0);
    assert!(bus.writes.is_empty());
    assert!(bus.page.0.iter().all(|b| *b == 0xa5));
    assert!(bus.arena.iter().all(|b| *b == 0xa5));
}
#[test]
fn handshake_timeout_never_publishes_context_address() {
    let mut bus = Bus::new();
    bus.handshake = false;
    assert_eq!(bus.start(), -110);
    assert_eq!(bus.writes, [(0x907c, 1)]);
    assert!(bus.posts.is_empty());
}
#[test]
fn invalid_dma_extents_do_not_enable_mastering() {
    for dma in [1, 1 << 36, (1 << 36) - 0x2000] {
        let mut bus = Bus::new();
        bus.dma = dma;
        assert_eq!(bus.start(), -22);
        assert_eq!(bus.published, 0);
    }
    let mut bus = Bus::new();
    bus.arena_dma = bus.dma;
    assert_eq!(bus.start(), -22);
    assert_eq!(bus.published, 0);
}
