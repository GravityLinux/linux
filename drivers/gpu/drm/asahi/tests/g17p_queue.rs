// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]
#[path = "../g17p_queue.rs"]
mod queue;
use queue::*;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
struct Writes(Vec<(u64, Vec<u8>)>);
impl Writer for Writes {
    type Error = ();
    fn write(&mut self, at: u64, data: &[u8]) -> Result<(), ()> {
        self.0.push((at, data.to_vec()));
        Ok(())
    }
}
fn hash(h: &mut u64, value: u8) {
    *h = (*h ^ value as u64).wrapping_mul(0x100000001b3);
}
fn main() {
    println!("compute-mailbox {COMPUTE_DOORBELL:x}");
    println!("pointer-offsets {POINTER_DONE:x} {POINTER_READ:x} {POINTER_WRITE:x}");
    for priority in 0..4 {
        println!(
            "priority {priority} {}",
            hex(&priority_profile(priority).unwrap())
        );
    }
    assert!(priority_profile(4).is_err());
    for case in 0u32..192 {
        let kind = match case % 3 {
            0 => Kind::Tiling,
            1 => Kind::Fragment,
            _ => Kind::Compute,
        };
        let producer = [0, 1, 254, 255][case as usize % 4];
        let count = [1, 3, 9][case as usize % 3];
        let items: Vec<u64> = (0..count)
            .map(|i| 0xfffffc20c1000000 + i as u64 * 0x4000)
            .collect();
        let stage = Stage {
            queue: 0xfffffc20c0000000,
            pointers: 0xfffffc2000400000,
            item_ring: 0xfffffc20c0040000,
            item_capacity: 64,
            write_index: 12,
            channel_ring: 0xfffffc20c0060000,
            channel_producer: 0xfffffc2000410020,
            counters: Counters::new([producer, producer, producer]).unwrap(),
            slot: None,
            items: &items,
            group: case * 129,
            grid: case % 12,
            kind,
            first: case & 1 != 0,
            in_place: case & 16 != 0,
            announce: case & 32 != 0,
            defer_inner: case & 2 != 0,
            defer_outer: case & 4 != 0,
            event_subtype: (case & 64 != 0).then_some(0x1001a),
            event_counter: (case % 11 == 0).then_some(0xfedcba98),
            event_counter_low: case & 7,
        };
        let mut writes = Writes(Vec::new());
        let publication = stage.publish(&mut writes).unwrap();
        println!("case {case}");
        for (at, data) in writes.0 {
            println!("write {at:x} {}", hex(&data));
        }
        for (name, pair) in [
            ("inner", publication.deferred_inner),
            ("outer", publication.deferred_outer),
        ] {
            if let Some((at, value)) = pair {
                println!("{name} {at:x} {}", hex(&value.to_le_bytes()));
            }
        }
        let record = Record {
            pointers: stage.pointers,
            ring: stage.item_ring,
            job_list: 0xfffffc2000000000,
            context: 0xfffffc20c07b8000,
            uuid: case * 913,
            priority: case % 5,
            prio5: case % 3,
            unk_2c: case << 16,
            unk_38: case & 1,
            unk_30: (case & 8 != 0).then_some(0xfffe0000deadbeef),
            unk_94: case * 734,
            sentinel_size: case as usize % 7,
        };
        println!("record {}", hex(&record.build().unwrap()));
        println!("pointers {}", hex(&pointers(case.wrapping_sub(1))));
        println!(
            "jobs {}",
            hex(&job_list(0xfffffc2000000000 + case as u64 * 0x4000))
        );
        println!(
            "meta {} {} {}",
            publication.slot, publication.producer, publication.write_after
        );
        assert!(!publication.completed(
            publication.write_after - 1,
            Counters([publication.producer; 3])
        ));
        assert!(!publication.completed(
            publication.write_after,
            Counters([producer as u8, publication.producer, publication.producer])
        ));
        assert!(publication.accepted(publication.write_after));
        assert!(publication.completed(publication.write_after, Counters([publication.producer; 3])));
    }
    let mut digest = 0xcbf29ce484222325;
    for producer in [0, 1, 127, 128, 254, 255] {
        for a in 0..=255 {
            for b in 0..=255 {
                let counters = Counters::new([a, b, producer]).unwrap();
                hash(&mut digest, counters.available());
                hash(&mut digest, if counters.slot().is_ok() { 1 } else { 0 });
                for distance in [1u8, 17, 255] {
                    hash(
                        &mut digest,
                        reached(
                            producer as u8,
                            a as u8,
                            (producer as u8).wrapping_add(distance),
                        ) as u8,
                    );
                }
            }
        }
    }
    println!("digest {digest:x}");
    assert!(Counters::new([256, 0, 0]).is_err());
    assert!(event(0x1000000, 0, Kind::Tiling, None, None, 0).is_err());
    // Invalid extent/overflow must be rejected before any publication store.
    let items = [0x1000];
    let mut stage = Stage {
        queue: 0,
        pointers: 0,
        item_ring: 0,
        item_capacity: 1,
        write_index: 1,
        channel_ring: 0,
        channel_producer: 0,
        counters: Counters([0; 3]),
        slot: None,
        items: &items,
        group: 0,
        grid: 0,
        kind: Kind::Tiling,
        first: true,
        in_place: false,
        announce: false,
        defer_inner: false,
        defer_outer: false,
        event_subtype: None,
        event_counter: None,
        event_counter_low: 0,
    };
    let mut writer = Writes(Vec::new());
    assert!(matches!(
        stage.publish(&mut writer),
        Err(StageError::Protocol(Error::Full))
    ));
    assert!(writer.0.is_empty());
    stage.item_capacity = 4;
    stage.item_ring = u64::MAX;
    assert!(matches!(
        stage.publish(&mut writer),
        Err(StageError::Protocol(Error::Overflow))
    ));
    assert!(writer.0.is_empty());
}
