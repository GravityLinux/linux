// SPDX-License-Identifier: GPL-2.0-only OR MIT
#![allow(dead_code)]
#[path = "../g17p_dependency_vm.rs"]
mod vm;
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
};
struct Space {
    roots: [BTreeMap<u64, u64>; 2],
    inventory: Vec<u64>,
    bindings: Vec<(u64, u64)>,
    next: u64,
    zeroed: Vec<u64>,
}
impl vm::Space for Space {
    type Error = ();
    fn pte(&self, root: vm::Root, va: u64) -> Result<u64, ()> {
        Ok(*self.roots[match root {
            vm::Root::Compute => 0,
            vm::Root::Render => 1,
        }]
        .get(&va)
        .unwrap_or(&0))
    }
    fn caller_overlap(&self, va: u64) -> bool {
        self.bindings
            .iter()
            .any(|&(start, len)| start < va + 0x4000 && va < start + len)
    }
    fn replace(&mut self, va: u64, pte: u64) -> Result<(), ()> {
        self.roots[0].insert(va, pte);
        Ok(())
    }
    fn zero_page(&mut self) -> Result<u64, ()> {
        let pa = self.next;
        self.next += 0x4000;
        self.zeroed.push(pa);
        Ok(pa)
    }
}
fn main() {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input).unwrap();
    let words: Vec<u64> = input
        .chunks_exact(8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let mut at = 0;
    let mut next = || {
        let word = words[at];
        at += 1;
        word
    };
    let cdms = [next(), next()];
    let mut space = Space {
        roots: [BTreeMap::new(), BTreeMap::new()],
        inventory: Vec::new(),
        bindings: Vec::new(),
        next: 0x10080000000,
        zeroed: Vec::new(),
    };
    for root in &mut space.roots {
        let count = next();
        for _ in 0..count {
            root.insert(next(), next());
        }
    }
    let count = next();
    for _ in 0..count {
        space.inventory.push(next());
    }
    let count = next();
    for _ in 0..count {
        space.bindings.push((next(), next()));
    }
    assert_eq!(at, words.len());
    let inventory = space.inventory.clone();
    let result =
        vm::join_render(&mut space, &inventory).and_then(|_| vm::join_compute(&mut space, cdms));
    let mut output = io::stdout().lock();
    let mut put = |v: u64| output.write_all(&v.to_le_bytes()).unwrap();
    match result {
        Ok(robustness) => {
            put(0);
            put(space.roots[0].len() as u64);
            for (&va, &pte) in &space.roots[0] {
                put(va);
                put(pte);
            }
            for pa in robustness {
                put(pa);
            }
            put(space.zeroed.len() as u64);
            for pa in space.zeroed {
                put(pa);
            }
        }
        Err(error) => {
            put(match error {
                vm::Error::Overlap(_) => 1,
                vm::Error::Unmapped(_) => 2,
                _ => 3,
            });
        }
    }
}
