// SPDX-License-Identifier: GPL-2.0-only OR MIT
//! Execute the kernel VM constructor against bounded RAM, with no hardware.
#![allow(dead_code)]
extern crate self as kernel;

#[macro_export]
macro_rules! dev_info { ($dev:expr, $($arg:tt)*) => {{ let _ = $dev; let _ = format_args!($($arg)*); }}; }
pub mod device {
    pub struct Device;
}
pub mod prelude {
    pub type Result<T = ()> = std::result::Result<T, i32>;
    pub const EINVAL: i32 = 22;
    pub const EEXIST: i32 = 17;
    pub const EBUSY: i32 = 16;
    pub const EIO: i32 = 5;
    pub const GFP_KERNEL: u32 = 0;
    pub struct KVec<T>(Vec<T>);
    impl<T> KVec<T> {
        pub fn new() -> Self {
            Self(Vec::new())
        }
        pub fn push(&mut self, v: T, _: u32) -> Result {
            self.0.push(v);
            Ok(())
        }
    }
    impl<T> std::ops::Deref for KVec<T> {
        type Target = [T];
        fn deref(&self) -> &[T] {
            &self.0
        }
    }
    impl<'a, T> IntoIterator for &'a KVec<T> {
        type Item = &'a T;
        type IntoIter = std::slice::Iter<'a, T>;
        fn into_iter(self) -> Self::IntoIter {
            self.0.iter()
        }
    }
}
#[path = "../g17p_abi.rs"]
mod g17p_abi;
#[path = "../g17p_initgraph.rs"]
mod g17p_initgraph;
#[path = "../g17p_layout.rs"]
mod g17p_layout;
#[path = "../g17p_opening.rs"]
mod g17p_opening;
#[path = "../g17p_topology.rs"]
mod g17p_topology;
#[path = "../g17p_vm.rs"]
mod g17p_vm;

mod g17p_platform {
    pub struct Region {
        pub base: u64,
    }
    pub struct Platform {
        pub regions: [Region; 2],
    }
}
mod g17p_image {
    use super::*;
    pub struct Image {
        pub graph: g17p_initgraph::Graph,
        pub buffers: Vec<Vec<u8>>,
    }
    pub struct Storage(pub Vec<Vec<u8>>);
    impl g17p_initgraph::Storage for Storage {
        fn object(&mut self, index: usize) -> Result<&mut [u8], g17p_abi::InvalidSize> {
            Ok(&mut self.0[index])
        }
    }
    impl Image {
        pub fn object(&self, index: usize) -> prelude::Result<&[u8]> {
            Ok(&self.buffers[index])
        }
    }
}
mod g17p_memory {
    pub fn sync() {}
    use super::*;
    use prelude::*;
    use std::collections::BTreeMap;
    pub struct Memory {
        pub pages: BTreeMap<u64, Vec<u8>>,
        next: u64,
    }
    impl Memory {
        pub fn new() -> Self {
            let mut m = Self {
                pages: BTreeMap::new(),
                next: 0x10080000000,
            };
            for &(base, size) in g17p_topology::RESERVATIONS
                .iter()
                .chain([(0x101fff30000, 0xcc000)].iter())
            {
                for pa in (base..base + size).step_by(0x4000) {
                    m.pages.insert(pa, vec![0; 0x4000]);
                }
            }
            m
        }
        pub fn allocate(&mut self, size: usize) -> Result<u64> {
            let pa = self.next;
            let size = size.next_power_of_two();
            for at in (pa..pa + size as u64).step_by(0x4000) {
                assert!(self.pages.insert(at, vec![0; 0x4000]).is_none());
            }
            self.next += size as u64;
            Ok(pa)
        }
        pub fn write(&mut self, at: u64, bytes: &[u8]) -> Result {
            for (index, byte) in bytes.iter().enumerate() {
                let at = at + index as u64;
                self.pages.get_mut(&(at & !0x3fff)).ok_or(EINVAL)?[(at & 0x3fff) as usize] = *byte;
            }
            Ok(())
        }
        pub fn zero(&mut self, at: u64, size: usize) -> Result {
            self.write(at, &vec![0; size])
        }
        pub fn write64(&mut self, at: u64, value: u64) -> Result {
            self.write(at, &value.to_le_bytes())
        }
        pub fn read64(&self, at: u64) -> Result<u64> {
            let page = self.pages.get(&(at & !0x3fff)).ok_or(EINVAL)?;
            let offset = (at & 0x3fff) as usize;
            Ok(u64::from_le_bytes(
                page[offset..offset + 8].try_into().unwrap(),
            ))
        }
        pub fn clean(&self, _: u64, _: usize) -> Result {
            Ok(())
        }
        pub fn invalidate(&self, _: u64, _: usize) -> Result {
            Ok(())
        }
        pub fn walk(&self, root: u64, va: u64) -> u64 {
            let l2 = self.read64(root + ((va >> 36) & 63) * 8).unwrap();
            assert_eq!(l2 & 3, 3);
            let l3 = self
                .read64((l2 & 0x0000ffffffffc000) + ((va >> 25) & 2047) * 8)
                .unwrap();
            assert_eq!(l3 & 3, 3);
            self.read64((l3 & 0x0000ffffffffc000) + ((va >> 14) & 2047) * 8)
                .unwrap()
        }
    }
}

fn main() {
    use g17p_initgraph::OBJECT_LAYOUT;
    use g17p_platform::{Platform, Region};
    const MASK: u64 = 0x0000ffffffffc000;
    let platform = Platform {
        regions: [
            Region {
                base: 0x101fffb8000,
            },
            Region {
                base: 0x101fff38000,
            },
        ],
    };
    let mut storage = g17p_image::Storage(OBJECT_LAYOUT.iter().map(|o| vec![0; o.size]).collect());
    let graph =
        g17p_initgraph::build(&mut storage, 0xfffffc2000000000, &g17p_layout::PERFORMANCE).unwrap();
    let mut image = g17p_image::Image {
        graph,
        buffers: storage.0,
    };
    let mut memory = g17p_memory::Memory::new();
    let originals = [
        [0x101fff3c403, 0x04200101fff484cb, 0x00200101fff344cb],
        [0x101fff7c403, 0x04200101fff884cb, 0x00200101fff344cb],
    ];
    for (peer, values) in originals.iter().enumerate() {
        for (index, value) in values.iter().enumerate() {
            memory
                .write64(
                    platform.regions[1].base + peer as u64 * 0x40000 + index as u64 * 8,
                    *value,
                )
                .unwrap();
        }
    }
    // A populated firmware-owned L2 slot must stop the constructor before any
    // host object/table publication instead of overwriting unknown mappings.
    memory.write64(g17p_topology::SHARED_L2, 3).unwrap();
    assert!(matches!(
        g17p_vm::Vm::build(&device::Device, &mut memory, &platform, &image),
        Err(prelude::EBUSY)
    ));
    assert_eq!(memory.read64(platform.regions[0].base).unwrap(), 0);
    memory.write64(g17p_topology::SHARED_L2, 0).unwrap();
    let mut vm = g17p_vm::Vm::build(&device::Device, &mut memory, &platform, &image).unwrap();
    // Keep the independent initdata byte check, allowing only the intentional
    // opening control ring/counter writes to the original generated image.
    for (slot, channels) in image.graph.channels.iter().enumerate() {
        let control = channels[12];
        for (va, bytes) in
            std::iter::once((control.ring, g17p_opening::message(slot == 1).to_vec())).chain(
                control
                    .states
                    .into_iter()
                    .map(|va| (va, 1u32.to_le_bytes().to_vec())),
            )
        {
            let index = OBJECT_LAYOUT
                .iter()
                .enumerate()
                .position(|(i, o)| {
                    va >= image.graph.addresses[i]
                        && va + bytes.len() as u64 <= image.graph.addresses[i] + o.size as u64
                })
                .unwrap();
            let offset = (va - image.graph.addresses[index]) as usize;
            image.buffers[index][offset..offset + bytes.len()].copy_from_slice(&bytes);
        }
    }
    let high_root = 0x10021598000;
    let low_root = 0x10034bcc000;
    for (peer, values) in originals.iter().enumerate() {
        for (index, value) in values.iter().enumerate() {
            assert_eq!(
                memory
                    .read64(high_root + peer as u64 * 0x40000 + index as u64 * 8)
                    .unwrap(),
                *value
            );
        }
    }
    let mut checked = 0;
    for &(first, count, expected) in g17p_topology::FIRMWARE_RUNS {
        for index in 0..count {
            let va = first + index as u64 * 0x4000;
            let pte = memory.walk(high_root, va);
            assert_eq!(pte & !MASK, expected, "attributes at {va:x}");
            assert!(
                memory.pages.contains_key(&(pte & MASK)),
                "unowned RAM leaf {va:x}"
            );
            checked += 1;
        }
    }
    for (index, object) in OBJECT_LAYOUT.iter().enumerate() {
        let va = image.graph.addresses[index];
        for offset in (0..object.size).step_by(0x4000) {
            let pte = memory.walk(high_root, va + offset as u64);
            let mut expected = image.buffers[index][offset..offset + 0x4000].to_vec();
            // Cold opening records are installed after the initdata image.
            // Their constructors are independently compared with Python by
            // check_g17p_opening.py; check their final physical placement here.
            let page = va + offset as u64;
            for (address, body) in [
                (0xfffffc20015e0000, g17p_opening::resource_record().to_vec()),
                (
                    0xfffffc20c07d0000,
                    g17p_opening::current_jobs(false).to_vec(),
                ),
                (
                    0xfffffc20c07f8000,
                    g17p_opening::current_jobs(true).to_vec(),
                ),
                (0xfffffc20015e8000, g17p_opening::dispatch_record().to_vec()),
            ] {
                if address >= page && address < page + 0x4000 {
                    let at = (address - page) as usize;
                    expected[at..at + body.len()].copy_from_slice(&body);
                }
            }
            assert!(
                memory.pages[&(pte & MASK)].as_slice() == expected,
                "initdata object {index} page {page:x} differs after cold opening overlay"
            );
        }
    }
    for &(first, count, high) in g17p_topology::CONTEXT0_PEERS {
        for index in 0..count {
            let va = first + index as u64 * 0x4000;
            let pte = memory.walk(low_root, va);
            let expected = g17p_topology::CONTEXT0_RUNS
                .iter()
                .find(|(start, count, _)| va >= *start && va < *start + *count as u64 * 0x4000)
                .unwrap()
                .2;
            assert_eq!(pte & !MASK, expected);
            assert_eq!(
                pte & MASK,
                memory.walk(high_root, high + index as u64 * 0x4000) & MASK
            );
            checked += 1;
        }
    }
    for (_, register) in g17p_layout::REGISTERS {
        for offset in (0..(register.address & 0x3fff) + register.size as u64).step_by(0x4000) {
            let pte = memory.walk(high_root, (register.address & !0x3fff) + offset);
            assert_eq!(
                pte,
                ((register.physical & !0x3fff) + offset) | 0x00c0000000000447
            );
            checked += 1;
        }
    }
    for (slot, tag, low) in [(0, 0, low_root), (1, 1, 0x10057ba0000)] {
        assert_eq!(
            memory.read64(platform.regions[0].base + slot * 16).unwrap(),
            tag << 48 | low | 1
        );
        assert_eq!(
            memory
                .read64(platform.regions[0].base + slot * 16 + 8)
                .unwrap(),
            tag << 48 | high_root | 1
        );
    }
    for slot in 2..64 {
        assert_eq!(
            memory.read64(platform.regions[0].base + slot * 16).unwrap(),
            0
        );
        assert_eq!(
            memory
                .read64(platform.regions[0].base + slot * 16 + 8)
                .unwrap(),
            0
        );
    }
    for &(first, count, flags) in g17p_topology::RENDER_RUNS {
        for index in 0..count {
            let va = first + index as u64 * 0x4000;
            let pte = memory.walk(0x10057ba0000, va);
            assert_eq!(pte & !MASK, flags, "render flags at {va:x}");
            assert!(memory.pages[&(pte & MASK)].iter().all(|b| *b == 0));
            checked += 1;
        }
    }
    for (high, low) in g17p_opening::CONTEXTS {
        for index in 0..8 {
            let delta = index * 0x4000;
            let hp = memory.walk(high_root, high + delta) & MASK;
            let cp = memory.walk(low_root, low + delta) & MASK;
            let rp = memory.walk(0x10057ba0000, low + delta) & MASK;
            assert_eq!(hp, cp);
            assert_ne!(cp, rp);
            assert!(memory.pages[&rp].iter().all(|b| *b == 0));
        }
    }
    // Runtime extension must preserve existing backing/content, including an
    // unaligned request spanning multiple pages. First-work aliases replace
    // only the explicitly requested context-0 pages.
    let address = 0xfffffc20c1f00010;
    vm.ensure_firmware(&mut memory, address, 0x9000).unwrap();
    let pa = vm.physical(&memory, 2, address).unwrap();
    vm.write(&mut memory, 2, address, &[0x55, 0x66, 0x77])
        .unwrap();
    vm.ensure_firmware(&mut memory, address, 0x9000).unwrap();
    assert_eq!(vm.physical(&memory, 2, address).unwrap(), pa);
    assert_eq!(&memory.pages[&(pa & MASK)][0x10..0x13], &[0x55, 0x66, 0x77]);
    vm.alias_firmware(&mut memory, address, 0x7000900010, 0x9000)
        .unwrap();
    for offset in (0..0xc000).step_by(0x4000) {
        let high = memory.walk(high_root, (address & !0x3fff) + offset);
        let low = memory.walk(low_root, 0x7000900000 + offset);
        assert_eq!(high & !MASK, 0x00c0000000000443);
        assert_eq!(low, (high & MASK) | 0x0080000000000c8b);
    }
    assert!(vm
        .alias_firmware(&mut memory, address, 0x7000900000, 0x9000)
        .is_err());
    // External timestamp pages keep exact caller backing and Shared/AP=1
    // attributes. Neither collisions nor aperture/alignment errors replace it.
    let stamp = g17p_vm::TIMESTAMP_BASE;
    let stamp_pa = memory.allocate(0x4000).unwrap();
    vm.timestamp_page(&mut memory, stamp, stamp_pa).unwrap();
    assert_eq!(memory.walk(high_root, stamp), stamp_pa | 0x00c000000000044b);
    vm.timestamp_page(&mut memory, stamp, stamp_pa).unwrap();
    assert_eq!(
        vm.timestamp_page(&mut memory, stamp, stamp_pa + 0x4000),
        Err(17)
    );
    for va in [stamp - 0x4000, stamp + 1, stamp + g17p_vm::TIMESTAMP_SIZE] {
        assert_eq!(vm.timestamp_page(&mut memory, va, stamp_pa), Err(22));
    }
    assert_eq!(
        vm.timestamp_page(&mut memory, stamp + 0x4000, stamp_pa + 1),
        Err(22)
    );
    assert_eq!(memory.walk(high_root, stamp), stamp_pa | 0x00c000000000044b);
    vm.timestamp_page(
        &mut memory,
        stamp + g17p_vm::TIMESTAMP_SIZE - 0x4000,
        stamp_pa,
    )
    .unwrap();
    println!("PASS: {checked} firmware/alias/MMIO leaves, all object bytes, private roots, context tags, L2 collision rejection, runtime extension/alias preservation, timestamp aliases/attributes/bounds");
}
