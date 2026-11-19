// SPDX-License-Identifier: GPL-2.0-only
//! Host contract tests for the kernel Rust transfer using modeled MMIO/DMA.

extern crate self as kernel;
#[macro_export]
macro_rules! pr_err {
    ($($argument:tt)*) => { eprintln!($($argument)*); };
}
pub mod prelude {
    #[derive(Clone, Copy)]
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
    pub const ENODEV: Error = Error(-19);
}

#[path = "../../drivers/net/wireless/apple/centauri/centauri_bootstrap.rs"]
mod bootstrap;
#[path = "../../drivers/net/wireless/apple/centauri/centauri_firmware.rs"]
mod centauri_firmware;

use core::ffi::c_void;
use std::cell::Cell;
const PREPARED: &[u8] = include_bytes!(env!("CENTAURI_FW_TEST_IMAGE"));
thread_local! { static SLEEPS: Cell<usize> = const { Cell::new(0) }; }

struct Bus {
    initial: u32,
    ready: u32,
    reads: usize,
    publish: usize,
    config_error: i32,
    peer_identity: u32,
    submission_initial: u32,
    irq_error: i32,
    irq_count: u32,
    config_writes: Vec<(u8, u16, u32)>,
    writes: Vec<(usize, u32)>,
}
impl Bus {
    fn new(ready: u32) -> Self {
        Self {
            initial: 0,
            ready,
            reads: 0,
            publish: 0,
            config_error: 0,
            peer_identity: 0x1903106b,
            submission_initial: 0,
            irq_error: 0,
            irq_count: 0,
            config_writes: vec![],
            writes: vec![],
        }
    }
}

#[no_mangle]
unsafe extern "C" fn centauri_boot_read(bar: *mut c_void, offset: usize) -> u32 {
    assert!(matches!(offset, 0x8000 | 0x8040));
    // SAFETY: Each test passes its exclusive live Bus as the BAR pointer.
    let bus = unsafe { &mut *bar.cast::<Bus>() };
    bus.reads += 1;
    if offset == 0x8040 {
        return if bus.publish == 0 {
            bus.submission_initial
        } else {
            1
        };
    }
    if bus.reads == 1 {
        bus.initial
    } else {
        bus.ready
    }
}
#[no_mangle]
unsafe extern "C" fn centauri_boot_write(bar: *mut c_void, offset: usize, value: u32) {
    // SAFETY: Same test-owned Bus contract as read.
    unsafe { &mut *bar.cast::<Bus>() }
        .writes
        .push((offset, value));
}
#[no_mangle]
unsafe extern "C" fn centauri_boot_publish_dma(owner: *mut c_void) {
    // SAFETY: Tests pass their live Bus as the opaque DMA owner.
    let bus = unsafe { &mut *owner.cast::<Bus>() };
    bus.publish += 1;
}
#[no_mangle]
extern "C" fn centauri_boot_time_us() -> u64 {
    0x123456789abcdef0
}
#[no_mangle]
unsafe extern "C" fn centauri_boot_enable_irqs(owner: *mut c_void, count: u32) -> i32 {
    // SAFETY: The test owns the live exclusive Bus for this entire call.
    let bus = unsafe { &mut *owner.cast::<Bus>() };
    bus.irq_count = count;
    bus.irq_error
}
#[no_mangle]
unsafe extern "C" fn centauri_boot_config_read(
    owner: *mut c_void,
    function: u8,
    offset: u16,
    value: *mut u32,
) -> i32 {
    assert_eq!((function, offset), (2, 0));
    // SAFETY: The test supplies a live Bus and an exclusive output scalar.
    unsafe {
        *value = (*owner.cast::<Bus>()).peer_identity;
    }
    0
}
#[no_mangle]
unsafe extern "C" fn centauri_boot_config_write(
    owner: *mut c_void,
    function: u8,
    offset: u16,
    value: u32,
) -> i32 {
    // SAFETY: The modeled PCI owner is the live exclusive Bus for this call.
    let bus = unsafe { &mut *owner.cast::<Bus>() };
    bus.config_writes.push((function, offset, value));
    bus.config_error
}
#[no_mangle]
extern "C" fn centauri_boot_sleep() {
    SLEEPS.with(|count| count.set(count.get() + 1));
}

unsafe extern "C" {
    fn centauri_bootstrap_image(
        owner: *mut c_void,
        bar: *mut c_void,
        image: *const u8,
        bytes: usize,
        image_dma: u64,
        record: *mut u8,
        record_dma: u64,
        platform: u32,
        runtime: bool,
        companion_dma: u64,
        companion_bytes: usize,
    ) -> i32;
}

fn transfer(
    bus: &mut Bus,
    image: &[u8],
    record: &mut [u8; 64],
    runtime: bool,
    image_dma: u64,
) -> i32 {
    let owner = core::ptr::from_mut(bus).cast();
    // SAFETY: Image is immutable; record[8..40] is exclusive; neither aliases
    // the live model Bus, which outlives this synchronous call. No device runs.
    unsafe {
        centauri_bootstrap_image(
            owner,
            owner,
            image.as_ptr(),
            image.len(),
            image_dma,
            record.as_mut_ptr().add(8),
            0x230000000,
            0x248142,
            runtime,
            0x340000000,
            0x10000,
        )
    }
}

#[test]
fn initial_stage_preserves_64_bit_dma_addresses_and_record_canaries() {
    let mut bus = Bus::new(4);
    let mut record = [0xa5; 64];
    assert_eq!(
        transfer(&mut bus, PREPARED, &mut record, false, 0x120000000),
        0
    );
    assert_eq!(bus.publish, 1);
    assert_eq!(bus.irq_count, 1);
    assert_eq!(
        bus.config_writes,
        vec![(0, 0xff8, 0x9abcdef0), (0, 0xffc, 0x12345678)]
    );
    assert_eq!(&record[..8], &[0xa5; 8]);
    assert_eq!(&record[40..], &[0xa5; 24]);
    assert_eq!(
        u64::from_le_bytes(record[8..16].try_into().unwrap()),
        0x120000000
    );
    assert_eq!(
        u64::from_le_bytes(record[16..24].try_into().unwrap()),
        PREPARED.len() as u64
    );
    assert_eq!(&record[24..40], &[0; 16]);
    assert_eq!(
        bus.writes,
        vec![
            (0x803c, 0),
            (0x807c, 0x248142),
            (0x8078, 0),
            (0x8044, 0x30000000),
            (0x8048, 2),
            (0x804c, 32),
            (0x9000, 1)
        ]
    );
}

#[test]
fn runtime_stage_uses_distinct_doorbell_and_ready_value() {
    let mut bus = Bus::new(2);
    bus.initial = 1;
    assert_eq!(
        transfer(&mut bus, PREPARED, &mut [0; 64], true, 0x120000000),
        0
    );
    assert_eq!(
        bus.writes,
        vec![(0x8044, 0x30000000), (0x8048, 2), (0x804c, 32), (0x9070, 1)]
    );
    assert_eq!(bus.irq_count, 4);
    assert_eq!(
        bus.config_writes,
        vec![
            (0, 0xff8, 0x9abcdef0),
            (0, 0xffc, 0x12345678),
            (2, 0xff8, 0x9abcdef0),
            (2, 0xffc, 0x12345678),
            (0, 0xf88, 0x40000000),
            (0, 0xf8c, 3),
            (0, 0xf90, 0x10000)
        ]
    );
}

#[test]
fn malformed_firmware_never_enables_bus_mastering_or_touches_mmio() {
    let mut bus = Bus::new(4);
    let mut record = [0xa5; 64];
    assert_eq!(
        transfer(&mut bus, &PREPARED[..48], &mut record, false, 0x120000000),
        -22
    );
    assert_eq!(bus.publish, 0);
    assert_eq!(bus.reads, 0);
    assert!(bus.writes.is_empty());
    assert_eq!(record, [0xa5; 64]);
}

#[test]
fn busy_function_is_not_adopted() {
    let mut bus = Bus::new(4);
    bus.initial = 2;
    assert_eq!(
        transfer(&mut bus, PREPARED, &mut [0; 64], false, 0x120000000),
        -16
    );
    assert_eq!(bus.publish, 0);
    assert!(bus.writes.is_empty());
}

#[test]
fn configuration_failure_does_not_ring_a_doorbell() {
    let mut bus = Bus::new(4);
    bus.config_error = -517;
    assert_eq!(
        transfer(&mut bus, PREPARED, &mut [0; 64], false, 0x120000000),
        -517
    );
    assert_eq!(bus.publish, 1);
    assert!(bus.writes.is_empty());
}

#[test]
fn unexpected_peer_function_prevents_dma_publication() {
    let mut bus = Bus::new(2);
    bus.initial = 1;
    bus.peer_identity = 0xffffffff;
    assert_eq!(
        transfer(&mut bus, PREPARED, &mut [0; 64], true, 0x120000000),
        -19
    );
    assert_eq!(bus.publish, 0);
    assert!(bus.writes.is_empty());
}

#[test]
fn runtime_image_requires_the_qualified_waiting_state_one() {
    let mut bus = Bus::new(2);
    assert_eq!(
        transfer(&mut bus, PREPARED, &mut [0; 64], true, 0x120000000),
        -16
    );
    assert_eq!(bus.publish, 0);
    assert_eq!(bus.irq_count, 0);
    assert!(bus.writes.is_empty());
}

#[test]
fn wrong_stage_ready_value_times_out_with_a_bounded_poll() {
    let mut bus = Bus::new(2);
    SLEEPS.with(|count| count.set(0));
    assert_eq!(
        transfer(&mut bus, PREPARED, &mut [0; 64], false, 0x120000000),
        -110
    );
    assert_eq!(bus.reads, 2004);
    assert_eq!(SLEEPS.with(|count| count.get()), 2000);
}

#[test]
fn pending_submission_prevents_reprobe_dma_even_with_main_status_zero() {
    let mut bus = Bus::new(4);
    bus.submission_initial = 1;
    let mut record = [0xa5; 64];
    assert_eq!(
        transfer(&mut bus, PREPARED, &mut record, false, 0x120000000),
        -16
    );
    assert_eq!(bus.publish, 0);
    assert_eq!(bus.irq_count, 0);
    assert!(bus.writes.is_empty());
    assert_eq!(record, [0xa5; 64]);
}

#[test]
fn msi_allocation_failure_does_not_publish_dma_or_ring_doorbells() {
    let mut bus = Bus::new(4);
    bus.irq_error = -28;
    let mut record = [0xa5; 64];
    assert_eq!(
        transfer(&mut bus, PREPARED, &mut record, false, 0x120000000),
        -28
    );
    assert_eq!(bus.irq_count, 1);
    assert_eq!(bus.publish, 0);
    assert!(bus.writes.is_empty());
    assert_eq!(record, [0xa5; 64]);
}

#[test]
fn overflowing_or_unaddressable_dma_is_rejected_before_publication() {
    for address in [u64::MAX, 1 << 36, (1 << 36) - 1] {
        let mut bus = Bus::new(4);
        assert_eq!(
            transfer(&mut bus, PREPARED, &mut [0; 64], false, address),
            -22
        );
        assert_eq!(bus.publish, 0);
        assert!(bus.writes.is_empty());
    }
}
