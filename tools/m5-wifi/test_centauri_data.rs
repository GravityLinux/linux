// SPDX-License-Identifier: GPL-2.0-only
//! Independent firmware-side extended queues and packet ownership tests.
use super::*;
use centauri_wifi_data::DataQueues;

#[derive(Default)]
struct Received {
    frames: Vec<Vec<u8>>,
}
#[no_mangle]
unsafe extern "C" fn centauri_radio_receive(owner: *mut c_void, frame: *const u8, bytes: usize) {
    let received = unsafe { &mut *owner.cast::<Received>() };
    received
        .frames
        .push(unsafe { std::slice::from_raw_parts(frame, bytes) }.to_vec());
}
impl Bus {
    pub(super) fn data_notify(&mut self) {
        let Some((ring, entries)) = self.rings.get(&(1, 25)).copied() else {
            return;
        };
        let mut consumed = load16(&self.arena, 0x24c + 25 * 2);
        let producer = load16(&self.arena, 0x5a0 + 25 * 2);
        while consumed != producer {
            let descriptor = ring + consumed as usize * 32;
            let flags = load32(&self.arena, descriptor);
            let pointer = load64(&self.arena, descriptor + 4);
            let cookie = load32(&self.arena, descriptor + 12);
            assert_eq!(flags & 255, 3);
            assert_eq!(cookie, consumed as u32);
            assert_eq!(load32(&self.arena, descriptor + 16), 1 << 17);
            assert_eq!(&self.arena[descriptor + 20..descriptor + 32], &[0; 12]);
            let offset = (pointer - self.arena_dma) as usize;
            let length = (flags >> 8) as usize;
            assert!(offset >= 0xa00000 + 0x478000 && offset + length <= self.arena.len());
            self.data_complete(192, 8192, 0x502, 25, cookie as u16, 0);
            consumed = (consumed + 1) % entries;
            store16(&mut self.arena, 0x24c + 25 * 2, consumed);
        }
    }
    fn data_complete(
        &mut self,
        cursor: usize,
        entries: u16,
        kind: u16,
        source: u16,
        cookie: u16,
        length: u16,
    ) {
        let queue = if cursor == 192 { 76 } else { 15 };
        let (ring, _) = self.rings[&(2, queue)];
        let producer = load16(&self.arena, 0x78 + cursor * 2);
        let next = (producer + 1) % entries;
        assert_ne!(next, load16(&self.arena, 0x3cc + cursor * 2));
        store32(
            &mut self.arena,
            ring + producer as usize * 32,
            u32::from(source) << 16 | u32::from(kind),
        );
        store32(
            &mut self.arena,
            ring + producer as usize * 32 + 4,
            u32::from(length) << 16 | u32::from(cookie),
        );
        store16(&mut self.arena, 0x78 + cursor * 2, next);
    }
    fn data_receive(&mut self, frame: &[u8], advance_sq: bool) -> u16 {
        let (ring, entries) = self.rings[&(1, 12)];
        let consumed = load16(&self.arena, 0x24c + 12 * 2);
        let producer = load16(&self.arena, 0x5a0 + 12 * 2);
        assert_ne!(consumed, producer);
        let descriptor = ring + consumed as usize * 16;
        assert_eq!(load32(&self.arena, descriptor), 2048 << 8 | 3);
        let pointer = load64(&self.arena, descriptor + 4);
        let cookie = load32(&self.arena, descriptor + 12) as u16;
        let buffer = (pointer - self.arena_dma) as usize;
        self.arena[buffer..buffer + 2].copy_from_slice(&[0xaa, 0x55]);
        self.arena[buffer + 2..buffer + 2 + frame.len()].copy_from_slice(frame);
        if advance_sq {
            store16(&mut self.arena, 0x24c + 12 * 2, (consumed + 1) % entries);
        }
        self.data_complete(207, 4096, 0x403, 12, cookie, (frame.len() + 2) as u16);
        cookie
    }
}
fn setup() -> (
    Box<Bus>,
    centauri_wifi_queues::WifiQueues,
    DataQueues,
    Box<Received>,
) {
    let mut bus = Box::new(Bus::new());
    bus.arena.resize(0xa00000 + 0x4b8000, 0xa5);
    assert_eq!(bus.start(), 0);
    let mut basic = bus.queues();
    basic.configure().unwrap();
    let mut received = Box::<Received>::default();
    let mut data = unsafe {
        DataQueues::new(
            ptr::from_mut(&mut *received).cast(),
            ptr::from_mut(&mut *bus).cast(),
            bus.arena.as_mut_ptr(),
            bus.arena_dma,
            bus.arena.as_mut_ptr().add(0xa00000),
            bus.arena_dma + 0xa00000,
            0x4b8000,
        )
    }
    .unwrap();
    data.configure(&mut basic).unwrap();
    (bus, basic, data, received)
}
#[test]
fn fresh_extended_registration_and_transmit_match_independent_python() {
    let (bus, _basic, mut data, _received) = setup();
    assert_eq!(centauri_wifi_data::centauri_wifi_data_bytes(), 0x4b8000);
    let fixture = include_bytes!(env!("CENTAURI_WIFI_DATA_FRAMES"));
    let mut offset = 0;
    assert_eq!(bus.admin_requests.len(), 16);
    for frame in &bus.admin_requests[12..] {
        let bytes = load32(fixture, offset) as usize;
        assert_eq!(frame, &fixture[offset + 4..offset + 4 + bytes]);
        offset += 4 + bytes;
    }
    assert_eq!(offset, fixture.len());
    assert_eq!(load16(&bus.arena, 0x5a0 + 24), 2047);
    data.xmit(&[7; 64]).unwrap();
    let ring = bus.rings[&(1, 25)].0;
    assert_eq!(
        &bus.arena[ring..ring + 32],
        include_bytes!(env!("CENTAURI_WIFI_DATA_TX"))
    );
    assert_eq!(data.poll(64).unwrap(), (0, true));
    assert_eq!(load16(&bus.arena, 0x3cc + 192 * 2), 1);
}
#[test]
fn changing_transmit_data_wraps_both_source_and_completion_without_early_reuse() {
    let (bus, _basic, mut data, _received) = setup();
    for sequence in 0..8300u32 {
        let mut frame = [0; 64];
        frame[..4].copy_from_slice(&sequence.to_le_bytes());
        frame[4..].fill((sequence % 251) as u8);
        data.xmit(&frame).unwrap();
        assert_eq!(data.xmit(&frame).unwrap_err().to_errno(), -11);
        let source = (sequence as usize % 2048) * 32 + bus.rings[&(1, 25)].0;
        let pointer = load64(&bus.arena, source + 4);
        let buffer = (pointer - bus.arena_dma) as usize;
        assert_eq!(&bus.arena[buffer..buffer + 64], &frame);
        assert_eq!(data.poll(0).unwrap(), (0, true));
    }
    assert_eq!(load16(&bus.arena, 0x5a0 + 25 * 2), 8300 % 2048);
    assert_eq!(load16(&bus.arena, 0x3cc + 192 * 2), 8300 % 8192);
}
#[test]
fn receive_cookie_recycling_and_payloads_survive_both_ring_wraps() {
    let (mut bus, _basic, mut data, received) = setup();
    for sequence in 0..5000u32 {
        let mut frame = [0; 64];
        frame[..4].copy_from_slice(&sequence.to_le_bytes());
        frame[4..].fill((sequence % 253) as u8);
        bus.data_receive(&frame, true);
        assert_eq!(data.poll(1).unwrap(), (1, false));
        assert_eq!(received.frames.last().unwrap(), &frame);
    }
    assert_eq!(received.frames.len(), 5000);
    assert_eq!(load16(&bus.arena, 0x3cc + 207 * 2), 5000 % 4096);
    assert_eq!(load16(&bus.arena, 0x5a0 + 12 * 2), (2047 + 5000) % 2048);
    // The first completion was reposted in slot2047 with cookie0, not cookie2047.
    let ring = bus.rings[&(1, 12)].0;
    assert_ne!(load32(&bus.arena, ring + 2047 * 16 + 12), 2047);
}
#[test]
fn full_receive_sq_defers_returned_cookie_until_the_device_cursor_advances() {
    let (mut bus, _basic, mut data, received) = setup();
    assert_eq!(bus.data_receive(&[9; 64], false), 0);
    assert_eq!(data.poll(8).unwrap(), (1, false));
    assert_eq!(received.frames.len(), 1);
    assert_eq!(load16(&bus.arena, 0x5a0 + 12 * 2), 2047);
    store16(&mut bus.arena, 0x24c + 12 * 2, 1);
    assert_eq!(data.poll(0).unwrap(), (0, false));
    assert_eq!(load16(&bus.arena, 0x5a0 + 12 * 2), 0);
    let ring = bus.rings[&(1, 12)].0;
    assert_eq!(load32(&bus.arena, ring + 2047 * 16 + 12), 0);
}
#[test]
fn malformed_rx_completion_does_not_advance_or_recycle_unowned_memory() {
    for variant in 0..4 {
        let (mut bus, _basic, mut data, received) = setup();
        let (kind, source, cookie, length) = match variant {
            0 => (0x401, 12, 0, 64),
            1 => (0x403, 13, 0, 64),
            2 => (0x403, 12, 2047, 64),
            _ => (0x403, 12, 0, 2049),
        };
        bus.data_complete(207, 4096, kind, source, cookie, length);
        assert_eq!(data.poll(64).unwrap_err().to_errno(), -5);
        assert_eq!(load16(&bus.arena, 0x3cc + 207 * 2), 0);
        assert_eq!(load16(&bus.arena, 0x5a0 + 12 * 2), 2047);
        assert!(received.frames.is_empty());
    }
}
#[test]
fn bad_tx_cookie_keeps_the_original_buffer_owned_and_cursor_unpublished() {
    let (mut bus, _basic, mut data, _received) = setup();
    data.xmit(&[1; 64]).unwrap();
    let ring = bus.rings[&(2, 76)].0;
    store32(&mut bus.arena, ring + 4, 1);
    assert_eq!(data.poll(8).unwrap_err().to_errno(), -5);
    assert_eq!(data.xmit(&[2; 64]).unwrap_err().to_errno(), -11);
    assert_eq!(load16(&bus.arena, 0x3cc + 192 * 2), 0);
}
