// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]
#[path = "../g17p_dependency.rs"]
mod d;
#[path = "../g17p_compute.rs"]
mod g17p_compute;
#[path = "../g17p_compute_memory.rs"]
mod g17p_compute_memory;
#[path = "../g17p_queue.rs"]
mod g17p_queue;
#[path = "../g17p_render.rs"]
mod g17p_render;
#[path = "../g17p_render_graph.rs"]
mod g17p_render_graph;
use d as g17p_dependency;
#[path = "../g17p_dependency_release.rs"]
mod release;
use std::{
    collections::BTreeMap,
    io::{self, Read},
};
const CONTROL: u64 = 0xfffffc20c0780020;
const OUTER: [u64; 3] = [0xfffffc20c07800a0, 0xfffffc20c07800b0, 0xfffffc20c07800c0];
struct Target {
    words: BTreeMap<u64, u32>,
    cursor: u8,
    trace: Vec<String>,
    calls: usize,
    fail: usize,
    bad_state: bool,
    dirty: bool,
}
fn hex(body: &[u8]) -> String {
    body.iter().map(|b| format!("{b:02x}")).collect()
}
impl Target {
    fn access(&mut self) -> Result<(), ()> {
        let index = self.calls;
        self.calls += 1;
        if index == self.fail {
            Err(())
        } else {
            Ok(())
        }
    }
    fn write(&mut self, va: u64, word: u32) {
        self.words.insert(va, word);
        self.trace.push(format!("W {va:x} {word:x}"));
        self.dirty = true;
    }
}
impl d::ReleaseWriter for Target {
    type Error = ();
    fn read32(&mut self, va: u64) -> Result<u32, ()> {
        self.access()?;
        Ok(*self.words.get(&va).unwrap_or(&0))
    }
    fn write32(&mut self, va: u64, word: u32) -> Result<(), ()> {
        self.access()?;
        self.write(va, word);
        Ok(())
    }
}
impl release::Target for Target {
    fn stage_control(&mut self, body: &[u8; 0x40], deferred: bool) -> Result<release::Control, ()> {
        self.access()?;
        self.trace
            .push(format!("S {} {}", u8::from(deferred), hex(body)));
        let before = self.cursor;
        self.cursor = self.cursor.wrapping_add(1);
        if !deferred {
            self.write(CONTROL, self.cursor as u32);
            self.dirty = false;
        }
        Ok(release::Control {
            producer: CONTROL,
            target: self.cursor,
            consumers_before: [before; 2],
        })
    }
    fn expect_receipt(&mut self, sequence: u32) -> Result<(), ()> {
        self.access()?;
        self.trace.push(format!("R {sequence}"));
        Ok(())
    }
    fn write_context(&mut self, va: u64, body: &[u8]) -> Result<(), ()> {
        self.access()?;
        self.trace.push(format!("C {va:x} {}", hex(body)));
        self.dirty = true;
        Ok(())
    }
    fn barrier(&mut self) {
        self.dirty = false;
    }
    fn notify(&mut self, message: u64) -> Result<(), ()> {
        self.access()?;
        assert!(!self.dirty, "mailbox exposed unbarriered stores");
        self.trace.push(format!("N {message:x}"));
        Ok(())
    }
    fn await_control(&mut self, control: release::Control) -> Result<(), ()> {
        self.access()?;
        self.trace.push(format!("A {}", control.target));
        if self.bad_state {
            self.words.insert(d::RENDER_INNER, 0);
        }
        Ok(())
    }
}
fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
    let args: Vec<usize> = input
        .split_whitespace()
        .map(|x| x.parse().unwrap())
        .collect();
    let case = args[0];
    let fail = args[1];
    let bad = args[2];
    let mut target = Target {
        words: BTreeMap::new(),
        cursor: 1,
        trace: Vec::new(),
        calls: 0,
        fail,
        bad_state: bad == 1,
        dirty: false,
    };
    for i in 0..8 {
        target.words.insert(
            d::LEAVES[0] + 0x60 + i * 4,
            if case == 15 {
                u32::MAX
            } else {
                case as u32 * 0x100 + i as u32 * 4
            },
        );
    }
    target.words.insert(
        d::LEAVES[1] + 0x30,
        if case == 15 {
            u32::MAX
        } else {
            case as u32 + 10
        },
    );
    target.words.insert(d::LEAVES[1] + 0x38, case as u32 + 20);
    let context = vec![case as u8; 0x200];
    let mut boundary = release::Boundary {
        opening_outer: (OUTER[0], 1),
        render_outer: [(OUTER[1], 1), (OUTER[2], 1)],
        closing_outer: (OUTER[0], 2),
        closing_inner: (d::LAYOUTS[3].pointers + 0x40, 3),
        closing_context: (d::LAYOUTS[3].context_high + 0x200, &context),
    };
    match bad {
        2 => boundary.opening_outer.1 = 2,
        3 => boundary.render_outer[0].1 = 0,
        4 => boundary.closing_outer.1 = 1,
        5 => boundary.closing_inner.1 = 2,
        6 => boundary.render_outer[1].0 = boundary.render_outer[0].0,
        7 => boundary.closing_context.1 = &context[..0x1ff],
        8 => boundary.closing_context.0 += 4,
        9 => boundary.opening_outer.0 = 0,
        10 => boundary.render_outer[0].0 += 1,
        _ => (),
    }
    let result = release::release(&mut target, &boundary);
    for line in target.trace {
        println!("{line}");
    }
    println!(
        "RESULT {} {}",
        match result {
            Ok(()) => "OK",
            Err(release::Error::InvalidBoundary) => "BOUNDARY",
            Err(release::Error::ClassState(_)) => "STATE",
            Err(_) => "ACCESS",
        },
        target.calls
    );
}
