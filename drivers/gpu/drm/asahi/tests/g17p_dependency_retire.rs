// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]
#[path = "../g17p_dependency_retire.rs"]
mod g17p_dependency_retire;
#[path = "../g17p_queue.rs"]
mod g17p_queue;
use g17p_dependency_retire::{Error, Observation, Owner, Reports, Retirement};
use std::io::{self, BufRead};
fn publication(i: usize, start: u8) -> g17p_queue::Publication {
    g17p_queue::Publication {
        slot: start,
        producer: start.wrapping_add(if i == 3 { 2 } else { 1 }),
        consumers_before: [start; 2],
        write_before: 0,
        write_after: 3,
        deferred_inner: None,
        deferred_outer: None,
    }
}
fn fixture(mask: u32, start: u8) -> ([g17p_queue::Publication; 4], [Observation; 4]) {
    let pubs = core::array::from_fn(|i| publication(i, start));
    let samples = core::array::from_fn(|i| {
        let target = pubs[i].producer as u32;
        let mut status = [0; 0x40];
        if mask & (1 << (12 + i)) != 0 {
            status[if i == 1 || i == 2 { 0x3f } else { 7 }] = 1;
        }
        Observation {
            done: if mask & (1 << i) != 0 { 3 } else { 2 },
            counters: [
                if mask & (1 << (4 + i)) != 0 {
                    target
                } else {
                    start as u32
                },
                if mask & (1 << (8 + i)) != 0 {
                    target
                } else {
                    start as u32
                },
                target,
            ],
            status,
        }
    });
    (pubs, samples)
}
fn main() {
    // Failures are terminal even when all other owners are demonstrably done.
    let (pubs, samples) = fixture(0xffff, 0);
    let good = || Reports {
        valid: true,
        render_complete: true,
    };
    let mut r = Retirement::new(pubs, [[0; 0x40]; 4]);
    assert_eq!(r.retire(Owner::Closing), Err(Error::Order));
    assert_eq!(r.poll(&samples, good()), Err(Error::Failed));
    let mut r = Retirement::new(pubs, [[0; 0x40]; 4]);
    assert_eq!(
        r.poll(
            &samples,
            Reports {
                valid: false,
                render_complete: true
            }
        ),
        Err(Error::Reports)
    );
    assert_eq!(r.poll(&samples, good()), Err(Error::Failed));
    let mut invalid = samples;
    invalid[2].counters[1] = 256;
    let mut r = Retirement::new(pubs, [[0; 0x40]; 4]);
    assert_eq!(r.poll(&invalid, good()), Err(Error::InvalidCounters));
    assert_eq!(r.poll(&samples, good()), Err(Error::Failed));
    let mut r = Retirement::new(pubs, [[0; 0x40]; 4]);
    assert_eq!(r.poll(&samples, good()).unwrap(), Some(Owner::Closing));
    assert_eq!(r.poll(&samples, good()).unwrap(), Some(Owner::Closing));
    r.fail();
    assert_eq!(r.retire(Owner::Closing), Err(Error::Failed));
    assert!(!r.complete());
    for line in io::stdin().lock().lines() {
        let values: Vec<u32> = line
            .unwrap()
            .split_whitespace()
            .map(|v| v.parse().unwrap())
            .collect();
        let (pubs, samples) = fixture(values[0], values[1] as u8);
        let mut r = Retirement::new(pubs, [[0; 0x40]; 4]);
        let mut names = Vec::new();
        while let Some(owner) = r
            .poll(
                &samples,
                Reports {
                    valid: true,
                    render_complete: values[2] != 0,
                },
            )
            .unwrap()
        {
            names.push(match owner {
                Owner::Closing => "C",
                Owner::Render => "R",
                Owner::Opening => "O",
            });
            r.retire(owner).unwrap();
        }
        println!(
            "{}",
            if names.is_empty() {
                "-".to_owned()
            } else {
                names.join("")
            }
        );
    }
}
