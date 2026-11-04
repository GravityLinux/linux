// SPDX-License-Identifier: GPL-2.0-only

//! Generated PF0 control IPC and shared service queues.
//!
//! C owns coherent DMA buffers and the live PCI function. No reserved-memory
//! handoff, firmware-owned DART root or captured context bytes are consumed.

use core::{ffi::c_void, ptr, slice};
use kernel::prelude::*;

pub(crate) const CONTEXT_BYTES: usize = 0x4000;
pub(crate) const ARENA_BYTES: usize = 0x200000;
const HEADER_BYTES: usize = 0x68;
const CONTEXT_FORMAT: u16 = 0x0300;
const STATE: usize = HEADER_BYTES;
const INITIAL_HOST_STATE: u32 = 2;
const COMPLETION_QUEUES: usize = 1;
const SOURCE_QUEUES: usize = 13;
const INDICES: usize = COMPLETION_QUEUES + SOURCE_QUEUES;
const DEVICE_INDICES: usize = STATE + 0x10;
const HOST_INDICES: usize = DEVICE_INDICES + INDICES * 2;
const SUBMISSION_RING: usize = HOST_INDICES + INDICES * 2;
const RING_ENTRIES: u16 = 16;
const DESCRIPTOR_BYTES: usize = 16;
const COMPLETION_RING: usize = SUBMISSION_RING + RING_ENTRIES as usize * DESCRIPTOR_BYTES;
const AUXILIARY: usize = COMPLETION_RING + RING_ENTRIES as usize * DESCRIPTOR_BYTES;
const FRAME_AREA: usize = 0x1000;
const FRAME_STRIDE: usize = 0x80;
const REGISTER_BYTES: usize = 52;
const REGISTER_RING: u32 = 1;
const IN_PLACE_COMPLETIONS: u32 = 1 << 22;
const EXTERNAL_DESCRIPTOR: u32 = 1;
const TRANSPORT_COMPLETION: u32 = 0x401;
const NO_ADDRESS: u64 = u64::MAX;
const NO_QUEUE: u16 = u16::MAX;
const BANK: usize = 0x8000;
const IMAGE_STATE: usize = BANK;
const IPC_STATE: usize = BANK + 0x50;
const IPC_ADDRESS_LOW: usize = BANK + 0x54;
const IPC_ADDRESS_HIGH: usize = BANK + 0x58;
const IPC_HANDSHAKE: usize = BANK + 0x107c;
const QUEUE_DOORBELLS: usize = BANK + 0x1000;
const WAIT_ATTEMPTS: usize = 600;

unsafe extern "C" {
    fn centauri_boot_read(bar: *mut c_void, offset: usize) -> u32;
    fn centauri_boot_write(bar: *mut c_void, offset: usize, value: u32);
    fn centauri_boot_publish_dma(owner: *mut c_void);
    fn centauri_boot_sleep();
    fn centauri_control_dma_wmb();
    fn centauri_control_dma_rmb();
}

fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn valid_dma(address: u64, bytes: usize) -> bool {
    address & (CONTEXT_BYTES as u64 - 1) == 0
        && address
            .checked_add(bytes as u64)
            .is_some_and(|end| end <= 1 << 36)
}

pub(crate) fn make_context(bytes: &mut [u8], dma: u64) -> Result {
    if bytes.len() != CONTEXT_BYTES || !valid_dma(dma, CONTEXT_BYTES) {
        return Err(EINVAL);
    }
    bytes.fill(0);
    put16(bytes, 0, CONTEXT_FORMAT);
    put16(bytes, 2, HEADER_BYTES as u16);
    for (field, offset) in [
        (0x08, STATE),
        (0x10, DEVICE_INDICES),
        (0x18, DEVICE_INDICES + 2),
        (0x20, HOST_INDICES),
        (0x28, HOST_INDICES + 2),
        (0x34, COMPLETION_RING),
        (0x3c, SUBMISSION_RING),
        (0x58, AUXILIARY),
    ] {
        // These two ring pointer fields are unaligned u64 wire values.
        put64(bytes, field, dma + offset as u64);
    }
    put16(bytes, 0x30, COMPLETION_QUEUES as u16);
    put16(bytes, 0x32, SOURCE_QUEUES as u16);
    put16(bytes, 0x44, RING_ENTRIES);
    put16(bytes, 0x46, RING_ENTRIES);
    // Independent222 qualifies zero at +0x48 through Wi-Fi management and
    // a Bluetooth connection/role/disconnect. No unknown profile is retained.
    put32(bytes, STATE, INITIAL_HOST_STATE);
    put32(bytes, STATE + 4, INITIAL_HOST_STATE);
    Ok(())
}

#[derive(Clone, Copy)]
struct Queue {
    number: u16,
    offset: usize,
    entries: u16,
    capacity: u32,
    inline: bool,
    receive_target: u16,
    pool: usize,
}

const QUEUES: [Queue; 4] = [
    Queue {
        number: 8,
        offset: 0,
        entries: 4,
        capacity: 0x1000,
        inline: false,
        receive_target: 3,
        pool: 0x18000,
    },
    Queue {
        number: 2,
        offset: 0x4000,
        entries: 16,
        capacity: 0x8000,
        inline: false,
        receive_target: 15,
        pool: 0x20000,
    },
    Queue {
        number: 4,
        offset: 0x8000,
        entries: 16,
        capacity: 0x400,
        inline: true,
        receive_target: 7,
        pool: 0,
    },
    Queue {
        number: 3,
        offset: 0x10000,
        entries: 16,
        capacity: 0x400,
        inline: true,
        receive_target: 0,
        pool: 0,
    },
];

impl Queue {
    fn stride(self) -> usize {
        DESCRIPTOR_BYTES
            + if self.inline {
                self.capacity as usize
            } else {
                0
            }
    }

    fn registration(self, arena_dma: u64) -> [u8; REGISTER_BYTES] {
        let mut frame = [0; REGISTER_BYTES];
        // Qualified 1024-byte inline capacity encoding: mantissa16 * 4 *
        // 2^4 bytes. External buffers instead carry capacity per descriptor.
        let capacity = if self.inline {
            4 << 28 | (self.capacity / (4 << 4)) << 16
        } else {
            0
        };
        put32(&mut frame, 0, capacity | REGISTER_RING);
        put32(
            &mut frame,
            4,
            u32::from(self.number) << 16 | u32::from(self.number),
        );
        put64(&mut frame, 8, arena_dma + self.offset as u64);
        put64(&mut frame, 16, NO_ADDRESS);
        put16(&mut frame, 24, self.entries);
        put16(&mut frame, 26, NO_QUEUE);
        put32(
            &mut frame,
            28,
            IN_PLACE_COMPLETIONS | u32::from(self.number),
        );
        frame
    }
}

// Raw pointers deliberately outlive no exclusive Rust reference: after
// publication firmware owns the device indices/completions and RX buffers.
pub(crate) struct Control {
    bar: *mut c_void,
    context: *mut u8,
    dma: u64,
}

impl Control {
    /// Borrow a published control context for serialized submissions.
    ///
    /// # Safety
    /// The validated BAR, aligned coherent context and DMA mapping must stay
    /// live for every use of the returned object. No other CPU may submit or
    /// mutate host cursors concurrently. Firmware owns device cursors/RX data.
    pub(crate) unsafe fn from_raw(bar: *mut c_void, context: *mut u8, dma: u64) -> Self {
        Self { bar, context, dma }
    }

    fn read16(&self, offset: usize) -> u16 {
        // SAFETY: Only the fixed aligned cursor offsets in our live context.
        u16::from_le(unsafe { ptr::read_volatile(self.context.add(offset).cast::<u16>()) })
    }

    fn write16(&self, offset: usize, value: u16) {
        // SAFETY: Only fixed aligned host-owned cursor offsets are passed.
        unsafe { ptr::write_volatile(self.context.add(offset).cast::<u16>(), value.to_le()) };
    }

    fn wait(&self, value: u32) -> Result {
        for _ in 0..WAIT_ATTEMPTS {
            // SAFETY: C retains the validated BAR for the entire probe.
            if unsafe { centauri_boot_read(self.bar, IPC_STATE) } == value {
                return Ok(());
            }
            // SAFETY: Sleepable probe context, live BAR.
            unsafe { centauri_boot_sleep() };
        }
        Err(ETIMEDOUT)
    }

    fn publish(&self, owner: *mut c_void) -> Result {
        // SAFETY: C retains all DMA buffers; our context is fully initialized
        // before the barrier/master enable and context publication sequence.
        unsafe {
            centauri_boot_publish_dma(owner);
            centauri_boot_write(self.bar, IPC_HANDSHAKE, 1);
        }
        self.wait(1)?;
        // SAFETY: These protocol registers are inside the validated BAR.
        unsafe {
            centauri_boot_write(self.bar, IPC_ADDRESS_LOW, self.dma as u32);
            centauri_boot_write(self.bar, IPC_ADDRESS_HIGH, (self.dma >> 32) as u32);
            centauri_boot_write(self.bar, IPC_HANDSHAKE, 2);
        }
        self.wait(2)
    }

    pub(crate) fn submit(&self, frame: &[u8]) -> Result {
        if !matches!(frame.len(), 4 | REGISTER_BYTES) {
            return Err(EINVAL);
        }
        let device_cq = self.read16(DEVICE_INDICES);
        let device_sq = self.read16(DEVICE_INDICES + 2);
        let host_cq = self.read16(HOST_INDICES);
        let producer = self.read16(HOST_INDICES + 2);
        if [device_cq, device_sq, host_cq, producer]
            .iter()
            .any(|v| *v >= RING_ENTRIES)
        {
            return Err(EIO);
        }
        if device_cq != producer || device_sq != producer {
            return Err(EBUSY);
        }
        let mask = RING_ENTRIES - 1;
        let pending = device_cq.wrapping_sub(host_cq) & mask;
        if pending > 1 {
            return Err(EIO);
        }
        if pending != 0 {
            // SAFETY: The last transport completed and the slot is now ours.
            unsafe {
                ptr::write_bytes(
                    self.context
                        .add(COMPLETION_RING + host_cq as usize * DESCRIPTOR_BYTES),
                    0,
                    DESCRIPTOR_BYTES,
                );
            }
        }
        let frame_offset = FRAME_AREA + producer as usize * FRAME_STRIDE;
        let mut descriptor = [0; DESCRIPTOR_BYTES];
        put32(
            &mut descriptor,
            0,
            (frame.len() as u32) << 8 | EXTERNAL_DESCRIPTOR,
        );
        put64(&mut descriptor, 4, self.dma + frame_offset as u64);
        put32(&mut descriptor, 12, u32::from(producer));
        // SAFETY: The prior exact cursor pair completed. Only this probe
        // submits, and each slot has its own bounded frame storage.
        unsafe {
            ptr::copy_nonoverlapping(frame.as_ptr(), self.context.add(frame_offset), frame.len());
            ptr::copy_nonoverlapping(
                descriptor.as_ptr(),
                self.context
                    .add(SUBMISSION_RING + producer as usize * DESCRIPTOR_BYTES),
                descriptor.len(),
            );
            centauri_control_dma_wmb();
        }
        let next = (producer + 1) & mask;
        self.write16(HOST_INDICES, device_cq);
        self.write16(HOST_INDICES + 2, next);
        // SAFETY: Publish initialized descriptor/frame and host cursors before
        // notifying the firmware. writel supplies the MMIO ordering too.
        unsafe {
            centauri_control_dma_wmb();
            centauri_boot_write(self.bar, QUEUE_DOORBELLS, u32::from(next));
        }
        for _ in 0..WAIT_ATTEMPTS {
            if self.read16(DEVICE_INDICES) == next && self.read16(DEVICE_INDICES + 2) == next {
                // SAFETY: Device cursor publication transfers ownership of
                // this aligned completion entry back to the host.
                let (kind, tag) = unsafe {
                    centauri_control_dma_rmb();
                    let entry = self
                        .context
                        .add(COMPLETION_RING + producer as usize * DESCRIPTOR_BYTES)
                        .cast::<u32>();
                    (
                        u32::from_le(ptr::read_volatile(entry)),
                        u32::from_le(ptr::read_volatile(entry.add(1))),
                    )
                };
                if kind != TRANSPORT_COMPLETION
                    || tag != ((frame.len() as u32) << 16 | u32::from(producer))
                {
                    return Err(EIO);
                }
                return Ok(());
            }
            // SAFETY: Sleepable probe context; C retains all DMA resources.
            unsafe { centauri_boot_sleep() };
        }
        Err(ETIMEDOUT)
    }

    fn post_receive(&self, queue: Queue, arena: *mut u8, arena_dma: u64) -> Result {
        let index = HOST_INDICES + 2 + queue.number as usize * 2;
        if self.read16(index) != 0 || queue.receive_target >= queue.entries {
            return Err(EBUSY);
        }
        for slot in 0..queue.receive_target {
            self.write_receive(queue, arena, arena_dma, slot);
        }
        // SAFETY: Publish descriptor contents before the host-owned cursor,
        // including when firmware polls that cursor without an MMIO kick.
        unsafe { centauri_control_dma_wmb() };
        self.write16(index, queue.receive_target);
        // SAFETY: Initialized coherent descriptors precede the queue doorbell.
        unsafe {
            centauri_control_dma_wmb();
            centauri_boot_write(
                self.bar,
                QUEUE_DOORBELLS + queue.number as usize * 4,
                u32::from(queue.receive_target),
            );
        }
        Ok(())
    }

    fn write_receive(&self, queue: Queue, arena: *mut u8, arena_dma: u64, slot: u16) {
        let mut descriptor = [0; DESCRIPTOR_BYTES];
        let pointer = if queue.inline {
            NO_ADDRESS
        } else {
            arena_dma + queue.pool as u64 + u64::from(slot) * u64::from(queue.capacity)
        };
        put32(
            &mut descriptor,
            0,
            queue.capacity << 8 | if queue.inline { 0 } else { EXTERNAL_DESCRIPTOR },
        );
        put64(&mut descriptor, 4, pointer);
        put32(&mut descriptor, 12, u32::from(slot));
        let offset = queue.offset + slot as usize * queue.stride();
        // SAFETY: The fixed profile/slot fits the live arena. Callers have
        // established host ownership before replacing this descriptor.
        unsafe { ptr::copy_nonoverlapping(descriptor.as_ptr(), arena.add(offset), descriptor.len()) };
    }
}

/// Service the two opaque shared event queues. ControlApp replies on inline
/// queue4 remain queued for the later radio initialization state machine.
pub(crate) struct SharedEvents {
    control: Control,
    arena: *mut u8,
    arena_dma: u64,
    cursors: [u16; 2],
    producers: [u16; 2],
}

impl SharedEvents {
    fn drain(&mut self) -> Result<i32> {
        let mut received = 0;
        for (index, queue) in QUEUES[..2].iter().copied().enumerate() {
            let device = self.control.read16(DEVICE_INDICES + 2 + queue.number as usize * 2);
            let host_offset = HOST_INDICES + 2 + queue.number as usize * 2;
            let producer = self.producers[index];
            let cursor = self.cursors[index];
            let outstanding = (producer + queue.entries - cursor) % queue.entries;
            if device >= queue.entries || self.control.read16(host_offset) != producer {
                return Err(EIO);
            }
            let completed = (device + queue.entries - cursor) % queue.entries;
            if completed > outstanding {
                return Err(EIO);
            }
            // Validate the entire completed batch before returning any of its
            // storage to firmware. Payloads remain opaque and are not copied.
            // SAFETY: The sampled device cursor transfers these slots to us.
            unsafe { centauri_control_dma_rmb() };
            for step in 0..completed {
                let slot = (cursor + step) % queue.entries;
                let offset = queue.offset + slot as usize * queue.stride();
                // SAFETY: Aligned u32 accesses to the bounded descriptor. The
                // u64 wire pointer is unaligned, so read its halves separately.
                let (length, pointer) = unsafe {
                    let entry = self.arena.add(offset).cast::<u32>();
                    (
                        u32::from_le(ptr::read_volatile(entry)) >> 8,
                        u64::from(u32::from_le(ptr::read_volatile(entry.add(1))))
                            | u64::from(u32::from_le(ptr::read_volatile(entry.add(2)))) << 32,
                    )
                };
                let expected = self.arena_dma
                    + queue.pool as u64
                    + u64::from(slot) * u64::from(queue.capacity);
                if length > queue.capacity || pointer != expected {
                    return Err(EIO);
                }
            }
            if completed == 0 {
                continue;
            }
            // The remaining posted slots stay firmware-owned. New slots begin
            // at the old producer, never at an outstanding descriptor.
            for step in 0..completed {
                self.control.write_receive(
                    queue,
                    self.arena,
                    self.arena_dma,
                    (producer + step) % queue.entries,
                );
            }
            let next = (producer + completed) % queue.entries;
            // SAFETY: Publish buffers before the cursor, then cursor before
            // MMIO. No exclusive slice of live device memory is constructed.
            unsafe { centauri_control_dma_wmb() };
            self.control.write16(host_offset, next);
            unsafe {
                centauri_control_dma_wmb();
                centauri_boot_write(
                    self.control.bar,
                    QUEUE_DOORBELLS + queue.number as usize * 4,
                    u32::from(next),
                );
            }
            self.cursors[index] = device;
            self.producers[index] = next;
            received += i32::from(completed);
        }
        Ok(received)
    }
}

/// Allocate host bookkeeping after a successful `centauri_control_start`.
///
/// # Safety
/// These are the same live validated buffers passed to start. C serializes
/// service calls and drains IRQ/work callbacks before destroying the result.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_control_service_create(
    bar: *mut c_void,
    context: *mut u8,
    context_dma: u64,
    arena: *mut u8,
    arena_dma: u64,
) -> *mut c_void {
    let state = SharedEvents {
        // SAFETY: Caller retains the successful startup's live resources.
        control: unsafe { Control::from_raw(bar, context, context_dma) },
        arena,
        arena_dma,
        cursors: [0; 2],
        producers: [QUEUES[0].receive_target, QUEUES[1].receive_target],
    };
    match KBox::new(state, GFP_KERNEL) {
        Ok(state) => KBox::into_raw(state).cast(),
        Err(_) => ptr::null_mut(),
    }
}

/// # Safety
/// The create result is live and exclusively owned by the service worker.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_control_service_poll(state: *mut c_void) -> i32 {
    // SAFETY: C serializes access for this live allocation.
    let state = unsafe { &mut *state.cast::<SharedEvents>() };
    match state.drain() {
        Ok(received) => received,
        Err(error) => error.to_errno(),
    }
}

/// # Safety
/// The unique create result is returned exactly once, after draining work.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_control_service_destroy(state: *mut c_void) {
    // SAFETY: C has synchronized IRQ/work users and returns ownership.
    drop(unsafe { KBox::<SharedEvents>::from_raw(state.cast()) });
}

/// Initialize shared control IPC and its four generated service queues.
///
/// # Safety
/// C must exclusively own distinct, aligned coherent context/arena buffers of
/// the exact lengths, their DMA mappings, and the validated PF0 BAR through
/// this call and subsequent device use. There must be no existing IPC owner.
/// On failure C must stop mastering and drain IRQ/DMA before freeing them.
#[no_mangle]
pub(crate) unsafe extern "C" fn centauri_control_start(
    owner: *mut c_void,
    bar: *mut c_void,
    context: *mut u8,
    context_bytes: usize,
    context_dma: u64,
    arena: *mut u8,
    arena_bytes: usize,
    arena_dma: u64,
) -> i32 {
    let mut stage = "preflight";
    let mut queue_number = 0;
    let mut start = || -> Result {
        if owner.is_null()
            || bar.is_null()
            || context.is_null()
            || arena.is_null()
            || context as usize & 3 != 0
            || arena as usize & 3 != 0
            || context_bytes != CONTEXT_BYTES
            || arena_bytes != ARENA_BYTES
            || !valid_dma(context_dma, context_bytes)
            || !valid_dma(arena_dma, arena_bytes)
            || !(context_dma + context_bytes as u64 <= arena_dma
                || arena_dma + arena_bytes as u64 <= context_dma)
        {
            return Err(EINVAL);
        }
        // SAFETY: Both checked register offsets are in the live mapped BAR.
        if unsafe { centauri_boot_read(bar, IMAGE_STATE) } != 2
            || unsafe { centauri_boot_read(bar, IPC_STATE) } != 0
        {
            return Err(EBUSY);
        }
        // SAFETY: No device pointer has been published. End the exclusive
        // slice borrow before firmware can write the context via DMA.
        make_context(
            unsafe { slice::from_raw_parts_mut(context, context_bytes) },
            context_dma,
        )?;
        // SAFETY: Arena is exclusive until we register/post its queue buffers.
        unsafe { ptr::write_bytes(arena, 0, arena_bytes) };
        // SAFETY: This probe exclusively owns the checked live resources.
        let control = unsafe { Control::from_raw(bar, context, context_dma) };
        stage = "context publication";
        control.publish(owner)?;
        for queue in QUEUES {
            stage = "queue registration";
            queue_number = queue.number;
            control.submit(&queue.registration(arena_dma))?;
            if queue.receive_target != 0 {
                stage = "receive publication";
                control.post_receive(queue, arena, arena_dma)?;
            }
        }
        Ok(())
    };
    match start() {
        Ok(()) => 0,
        Err(error) => {
            kernel::pr_err!(
                "control IPC {} (queue {}) failed: {}\n",
                stage,
                queue_number,
                error.to_errno()
            );
            error.to_errno()
        }
    }
}
