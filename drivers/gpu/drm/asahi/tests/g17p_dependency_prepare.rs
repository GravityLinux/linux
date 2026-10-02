// SPDX-License-Identifier: GPL-2.0-only OR MIT
//! Execute production preparation/report logic with owned synthetic RAM.
//! The models below replace kernel allocation/UAT/cache APIs, never protocol
//! builders. This does not qualify cache instructions or execution on a GPU.
#![allow(dead_code)]
extern crate self as kernel;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    rc::Rc,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error(i32);
pub type Result<T = ()> = core::result::Result<T, Error>;
pub const EINVAL: Error = Error(22);
pub const EIO: Error = Error(5);
pub const EBUSY: Error = Error(16);
pub const ENOMEM: Error = Error(12);
pub const EEXIST: Error = Error(17);
pub const GFP_KERNEL: u32 = 0;
pub struct KVec<T>(Vec<T>);
pub type KVVec<T> = KVec<T>;
impl<T> KVec<T> {
    pub fn new() -> Self {
        Self(Vec::new())
    }
    pub fn with_capacity(n: usize, _: u32) -> Result<Self> {
        Ok(Self(Vec::with_capacity(n)))
    }
    pub fn push(&mut self, v: T, _: u32) -> Result {
        self.0.push(v);
        Ok(())
    }
    pub fn reserve(&mut self, n: usize, _: u32) -> Result {
        self.0.reserve(n);
        Ok(())
    }
    pub fn remove(&mut self, i: usize) -> Result<T> {
        Ok(self.0.remove(i))
    }
}
impl<T: Clone> KVec<T> {
    pub fn resize(&mut self, n: usize, v: T, _: u32) -> Result {
        self.0.resize(n, v);
        Ok(())
    }
    pub fn extend_from_slice(&mut self, s: &[T], _: u32) -> Result {
        self.0.extend_from_slice(s);
        Ok(())
    }
}
impl<T> Deref for KVec<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}
impl<T> DerefMut for KVec<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}
impl<T> IntoIterator for KVec<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
impl<'a, T> IntoIterator for &'a KVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
impl<'a, T> IntoIterator for &'a mut KVec<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
    }
}
pub mod prelude {
    pub use crate::{
        dev_info, pr_err, Error, KVVec, KVec, Result, EBUSY, EEXIST, EINVAL, EIO, ENOMEM,
        GFP_KERNEL,
    };
}
pub mod device {
    pub struct Device;
}
#[macro_export]
macro_rules! dev_info {($dev:expr,$($arg:tt)*) => {{let _=$dev;let _=format_args!($($arg)*);}}}
#[macro_export]
macro_rules! pr_err {($($arg:tt)*) => {{eprint!($($arg)*);}}}

#[path = "../g17p_abi.rs"]
mod g17p_abi;
#[path = "../g17p_compute.rs"]
mod g17p_compute;
#[path = "../g17p_compute_memory.rs"]
mod g17p_compute_memory;
#[path = "../g17p_dependency.rs"]
mod g17p_dependency;
#[path = "../g17p_dependency_control.rs"]
mod g17p_dependency_control;
#[path = "../g17p_dependency_release.rs"]
mod g17p_dependency_release;
#[path = "../g17p_dependency_retire.rs"]
mod g17p_dependency_retire;
#[path = "../g17p_dependency_runtime.rs"]
mod g17p_dependency_runtime;
#[path = "../g17p_dependency_vm.rs"]
mod g17p_dependency_vm;
#[path = "../g17p_growth.rs"]
mod g17p_growth;
#[path = "../g17p_growth_runtime.rs"]
mod g17p_growth_runtime;
#[path = "../g17p_initgraph.rs"]
mod g17p_initgraph;
#[path = "../g17p_layout.rs"]
mod g17p_layout;
#[path = "../g17p_opening.rs"]
mod g17p_opening;
#[path = "../g17p_queue.rs"]
mod g17p_queue;
#[path = "../g17p_render.rs"]
mod g17p_render;
#[path = "../g17p_render_graph.rs"]
mod g17p_render_graph;
#[path = "../g17p_resource_record.rs"]
mod g17p_resource_record;
#[path = "../g17p_partial_runtime.rs"]
mod g17p_partial_runtime;
#[path = "../g17p_render_lifecycle.rs"]
mod g17p_render_lifecycle;
#[path = "../g17p_topology.rs"]
mod g17p_topology;
// Compile the production Vm replacements against the same Memory API too.
// Never execute this type's privileged target cache/TLBI instructions here.
#[path = "../g17p_vm.rs"]
mod checked_vm;

mod g17p_memory {
    use super::*;
    pub struct Memory {
        pub pages: Rc<RefCell<BTreeMap<u64, Box<[u8; 0x4000]>>>>,
        next: u64,
        pub fail_next_allocation: bool,
    }
    pub struct Word64<'a> {
        owner: &'a Memory,
        address: u64,
    }
    impl Word64<'_> {
        pub fn load(&self) -> u64 {
            self.owner.read64(self.address).unwrap()
        }
        pub fn store(&self, value: u64) {
            self.owner.put(self.address, &value.to_le_bytes()).unwrap();
        }
    }
    pub fn sync() {}
    impl Memory {
        pub fn new() -> Self {
            Self {
                pages: Rc::new(RefCell::new(BTreeMap::new())),
                next: 0x10090000000,
                fail_next_allocation: false,
            }
        }
        pub fn allocate(&mut self, size: usize) -> Result<u64> {
            if core::mem::take(&mut self.fail_next_allocation) {
                return Err(ENOMEM);
            }
            let size = (size + 0x7fff) & !0x7fff;
            let pa = self.next;
            self.next += size as u64;
            for off in (0..size).step_by(0x4000) {
                self.pages
                    .borrow_mut()
                    .insert(pa + off as u64, Box::new([0; 0x4000]));
            }
            Ok(pa)
        }
        fn put(&self, address: u64, body: &[u8]) -> Result {
            for (i, &v) in body.iter().enumerate() {
                let at = address + i as u64;
                let mut pages = self.pages.borrow_mut();
                let page = pages.get_mut(&(at & !0x3fff)).ok_or(EINVAL)?;
                page[(at & 0x3fff) as usize] = v;
            }
            Ok(())
        }
        pub fn bytes(&self, address: u64, size: usize) -> Result<Vec<u8>> {
            (0..size)
                .map(|i| {
                    let at = address + i as u64;
                    self.pages
                        .borrow()
                        .get(&(at & !0x3fff))
                        .map(|p| p[(at & 0x3fff) as usize])
                        .ok_or(EINVAL)
                })
                .collect()
        }
        pub fn write(&mut self, address: u64, body: &[u8]) -> Result {
            self.put(address, body)
        }
        pub fn write32(&mut self, address: u64, word: u32) -> Result {
            self.put(address, &word.to_le_bytes())
        }
        pub fn write64(&mut self, address: u64, word: u64) -> Result {
            self.put(address, &word.to_le_bytes())
        }
        pub fn zero(&mut self, address: u64, size: usize) -> Result {
            self.put(address, &vec![0; size])
        }
        pub fn clean(&self, address: u64, size: usize) -> Result {
            self.bytes(address, size).map(|_| ())
        }
        pub fn invalidate(&self, address: u64, size: usize) -> Result {
            self.clean(address, size)
        }
        pub fn read_firmware32(&self, address: u64) -> Result<u32> {
            Ok(u32::from_le_bytes(
                self.bytes(address, 4)?.try_into().unwrap(),
            ))
        }
        pub fn read64(&self, address: u64) -> Result<u64> {
            Ok(u64::from_le_bytes(
                self.bytes(address, 8)?.try_into().unwrap(),
            ))
        }
        pub fn word64(&self, address: u64) -> Result<Word64<'_>> {
            self.bytes(address, 8)?;
            Ok(Word64 {
                owner: self,
                address,
            })
        }
    }
}
mod g17p_user_vm {
    use super::*;
    pub struct UserVm {
        pub leaves: BTreeMap<u64, u64>,
        pub root: u64,
    }
    impl UserVm {
        pub fn new(root: u64) -> Self {
            Self {
                leaves: BTreeMap::new(),
                root,
            }
        }
        pub fn root(&self) -> u64 {
            self.root
        }
        pub fn invalidate(context: u16) {
            assert!((1..=3).contains(&context));
        }
        pub fn pte(&self, va: u64) -> Result<u64> {
            Ok(*self.leaves.get(&va).unwrap_or(&0))
        }
        pub fn rebind(&mut self, changes: &[(u64, u64, u64)], contexts: &[u16]) -> Result {
            assert!(!contexts.is_empty());
            for &(va, old, _) in changes {
                assert_eq!(self.pte(va)?, old);
            }
            for &(va, _, new) in changes {
                self.leaves.insert(va, new);
            }
            Ok(())
        }
        pub fn grow(&mut self, pages: &[(u64, u64)]) -> Result {
            for &(va, _) in pages {
                if self.pte(va)? != 0 {
                    return Err(EINVAL);
                }
            }
            for &(va, pa) in pages {
                self.leaves.insert(va, pa | 0xc0000000000c8b);
            }
            Ok(())
        }
    }
}
mod g17p_vm {
    use super::*;
    pub struct Vm {
        pub leaves: BTreeMap<(usize, u64), u64>,
        pub fresh: Vec<(u64, u64, u64)>,
    }
    impl Vm {
        pub fn invalidate_gpu() {}
        pub fn new() -> Self {
            Self {
                leaves: BTreeMap::new(),
                fresh: Vec::new(),
            }
        }
        pub fn pte(&self, _: &g17p_memory::Memory, group: usize, va: u64) -> Result<u64> {
            Ok(*self.leaves.get(&(group, va)).unwrap_or(&0))
        }
        pub fn physical(&self, m: &g17p_memory::Memory, g: usize, va: u64) -> Result<u64> {
            let leaf = self.pte(m, g, va & !0x3fff)?;
            if leaf == 0 {
                return Err(EINVAL);
            }
            Ok((leaf & 0x3ffffffc000) + (va & 0x3fff))
        }
        pub fn write(&self, m: &mut g17p_memory::Memory, g: usize, va: u64, body: &[u8]) -> Result {
            for (i, b) in body.iter().enumerate() {
                m.write(self.physical(m, g, va + i as u64)?, &[*b])?;
            }
            Ok(())
        }
        pub fn ensure_firmware(
            &mut self,
            m: &mut g17p_memory::Memory,
            va: u64,
            size: usize,
        ) -> Result {
            let start = va & !0x3fff;
            let end = (va + size as u64 + 0x3fff) & !0x3fff;
            for at in (start..end).step_by(0x4000) {
                if self.pte(m, 2, at)? == 0 {
                    let pa = m.allocate(0x4000)?;
                    self.leaves.insert(
                        (2, at),
                        pa | if at >= 0xfffffc20c0000000 {
                            0xc0000000000443
                        } else {
                            0xc000000000044b
                        },
                    );
                }
            }
            Ok(())
        }
        pub fn rebind_pages(
            &self,
            m: &g17p_memory::Memory,
            changes: &[(usize, u64, u64, u64)],
        ) -> Result {
            // Preparation only reclassifies existing high leaves in this model;
            // fixtures already carry exactly the admitted Shared/AP1 flags.
            for &(g, va, old, new) in changes {
                assert_eq!(self.pte(m, g, va)?, old);
                assert_eq!(old, new);
            }
            Ok(())
        }
        pub fn alias_firmware(
            &mut self,
            m: &mut g17p_memory::Memory,
            high: u64,
            low: u64,
            size: usize,
        ) -> Result {
            self.ensure_firmware(m, high, size)?;
            let first = high & !0x3fff;
            let end = (high + size as u64 + 0x3fff) & !0x3fff;
            for at in (first..end).step_by(0x4000) {
                self.leaves.insert(
                    (0, (low & !0x3fff) + at - first),
                    self.physical(m, 2, at)? | 0x80000000000c8b,
                );
            }
            Ok(())
        }
        pub fn freshen_firmware(&mut self, m: &mut g17p_memory::Memory, pages: &[u64]) -> Result {
            for &va in pages {
                let old = self.pte(m, 2, va)?;
                assert_ne!(old, 0);
                let pa = m.allocate(0x4000)?;
                let new = pa | (old & !0x3ffffffc000);
                self.leaves.insert((2, va), new);
                self.fresh.push((va, old, new));
            }
            Ok(())
        }
        pub fn flush_tables(&self, _: &g17p_memory::Memory) -> Result {
            Ok(())
        }
        pub fn bytes(&self, m: &g17p_memory::Memory, g: usize, va: u64, size: usize) -> Vec<u8> {
            (0..size)
                .map(|i| {
                    m.bytes(self.physical(m, g, va + i as u64).unwrap(), 1)
                        .unwrap()[0]
                })
                .collect()
        }
    }
}
mod g17p_compute_runtime {
    use super::*;
    pub struct Client {
        pub root: g17p_user_vm::UserVm,
        pub buffers: KVec<()>,
        pub bindings: KVec<(u64, u64, u64, u32)>,
        pub owner: (u64, u32),
    }
    impl Client {
        pub fn cache(&self, _: bool) -> Result {
            Ok(())
        }
    }
    pub struct Parameters {
        pub cdm: u64,
        pub end: u64,
        pub sampler: u64,
        pub sampler_count: u32,
        pub timestamps: [u64; 2],
    }
    pub struct Submission {
        pub client: Client,
        pub channel: g17p_abi::Channel,
        pub ordinal: u32,
        pub after_render: bool,
    }
    pub fn idle(_: &g17p_memory::Memory, _: &g17p_vm::Vm, work: &Submission) -> Result {
        if work.ordinal != 0 {
            return Err(EBUSY);
        }
        Ok(())
    }
}
mod g17p_render_runtime {
    use super::*;
    pub(crate) use g17p_render_lifecycle::POINTERS;
    pub struct Submission {
        pub client: g17p_compute_runtime::Client,
        pub channels: [g17p_abi::Channel; 2],
        pub ordinal: u32,
        pub growth: Option<()>,
    }
    pub fn validate_client(
        _: &g17p_compute_runtime::Client,
        p: &g17p_render::Parameters,
    ) -> Result {
        p.validate().map_err(|_| EINVAL)
    }
}
mod g17p_platform {
    pub struct Region {
        pub base: u64,
        pub size: u64,
    }
    pub struct Platform {
        pub regions: [Region; 4],
    }
}
mod g17p_image {
    use super::*;
    pub struct Graph {
        pub addresses: [u64; 12],
        pub channels: [[g17p_abi::Channel; g17p_abi::CHANNELS]; 2],
    }
    impl Graph {
        pub fn primary_aliases(&self) -> [(u64, u64); 0] {
            []
        }
    }
    pub struct Image {
        pub graph: Graph,
    }
    impl Image {
        pub fn object(&self, _: usize) -> Result<&[u8]> {
            Ok(&[])
        }
    }
}

use g17p_compute_runtime as c;
use g17p_dependency as d;
use g17p_memory::Memory;
use g17p_vm::Vm;
fn client(root: u64) -> c::Client {
    c::Client {
        root: g17p_user_vm::UserVm::new(root),
        buffers: KVec::new(),
        bindings: KVec::new(),
        owner: (1, 1),
    }
}
fn partial_owner_graph() -> Result {
    let mut memory = g17p_memory::Memory::new();
    let mut vm = g17p_vm::Vm::new();
    let mut root = g17p_user_vm::UserVm::new(0x10098000000);
    let primary = g17p_render_lifecycle::ORDINARY;
    let control = g17p_render_lifecycle::SECOND.support;
    vm.ensure_firmware(&mut memory, control, 0x4000)?;
    vm.write(&mut memory, 2, control + 0x4c, &0xfffffc2000004000u64.to_le_bytes())?;
    g17p_partial_runtime::build_graph(&mut memory, &mut vm, &mut root, primary, control)?;
    let graph = g17p_partial_runtime::GRAPH;
    let shared = vm.physical(&memory, 2, graph[8].1)?;
    assert_eq!(memory.read_firmware32(shared + 0xc)?, 1);
    assert_eq!(memory.read_firmware32(shared + 0x3c)?, 8);
    assert_eq!(memory.read64(shared + 0x20)?, graph[0].1);
    assert_eq!(memory.read64(shared + 0x28)?, 0x1000340000);
    for offset in (0..0x10000).step_by(0x4000) {
        assert_eq!(root.pte(0x1000340000 + offset)? & 0x3ffffffc000,
                   vm.physical(&memory, 2, graph[0].1 + offset)?);
    }
    let array = vm.physical(&memory, 2, graph[6].1)?;
    for i in 0..35 { assert_eq!(memory.read64(array + i * 0x100)?, graph[2].1 + 4 + i * 4); }
    println!("PASS production second partial owner construction and full four-page index aliases");
    Ok(())
}
fn main() -> Result {
    partial_owner_graph()?;
    let mut memory = Memory::new();
    let mut vm = Vm::new();
    let channel = |index: u64| g17p_abi::Channel {
        states: [
            0xfffffc20c0790000 + index * 0x80,
            0xfffffc20c0790004 + index * 0x80,
            0xfffffc20c0790008 + index * 0x80,
        ],
        ring: 0xfffffc20c0900000 + index * 0x1800,
    };
    let mut image = g17p_image::Image {
        graph: g17p_image::Graph {
            addresses: [0; 12],
            channels: [[g17p_abi::Channel::default(); g17p_abi::CHANNELS]; 2],
        },
    };
    for i in 0..9 {
        image.graph.channels[0][i] = channel(i as u64);
        vm.ensure_firmware(&mut memory, channel(i as u64).states[0], 0x100)?;
        vm.ensure_firmware(&mut memory, channel(i as u64).ring, 0x3000)?;
    }
    let mut compute = c::Submission {
        client: client(0x10081000000),
        channel: channel(8),
        ordinal: 0,
        after_render: false,
    };
    let mut render = g17p_render_runtime::Submission {
        client: client(0x10082000000),
        channels: [channel(6), channel(7)],
        ordinal: 0,
        growth: None,
    };
    render.client.buffers.push((), GFP_KERNEL)?;
    for &(base, count, flags) in g17p_topology::RENDER_RUNS {
        for off in (0..count * 0x4000).step_by(0x4000) {
            let va = base + off as u64;
            let pa = memory.allocate(0x4000)?;
            render.client.root.leaves.insert(va, pa | flags);
        }
    }
    for va in g17p_opening::EXTRA_RENDER {
        let pa = memory.allocate(0x4000)?;
        render.client.root.leaves.insert(va, pa | 0xc0000000000c8b);
    }
    // All active render sources are independently retained, while compute's
    // operand tranches collide with them before the production join.
    for &va in render
        .client
        .root
        .leaves
        .keys()
        .filter(|&&va| (0x7000000000..0x7003000000).contains(&va))
    {
        let pa = memory.allocate(0x4000)?;
        compute.client.root.leaves.insert(va, pa | 0xc0000000000c8b);
    }
    for va in [0x1000080000, 0x1000198000, 0x10001a8000] {
        let pa = memory.allocate(0x4000)?;
        compute.client.root.leaves.insert(va, pa | 0xc0000000000c8b);
    }
    for base in [0x7000220000, 0x70017e0000] {
        for off in (0..0x14000).step_by(0x4000) {
            let pa = memory.allocate(0x4000)?;
            compute
                .client
                .root
                .leaves
                .insert(base + off, pa | 0xc0000000000c8b);
        }
    }
    let cdms = [0x10000400000, 0x10000500000];
    for cdm in cdms {
        for off in [0, 0x30000] {
            let pa = memory.allocate(0x4000)?;
            compute
                .client
                .root
                .leaves
                .insert(cdm + off, pa | 0xc0000000000c8b);
        }
        compute
            .client
            .bindings
            .push((cdm, 0x40000, 0, 0), GFP_KERNEL)?;
    }
    let inputs = cdms.map(|cdm| c::Parameters {
        cdm,
        end: cdm + 0x34,
        sampler: 0,
        sampler_count: 0,
        timestamps: [0, 0],
    });
    // The source high extent and ordinary render pointer blocks are present.
    for &(va, count, _) in g17p_topology::FIRMWARE_RUNS {
        vm.ensure_firmware(&mut memory, va, count * 0x4000)?;
    }
    let p = g17p_render::Parameters {
        width: 32,
        height: 32,
        context_base: 0x1000000000,
        tilemap: 0x10001b0000,
        heapmeta: 0x10001b1000,
        tpc: 0x10001d8000,
        ta_status: 0x1000078000,
        fragment_status: 0x10001a8000,
        deflake_1: 0x10000682a0,
        deflake_2: 0x1000068020,
        deflake_3: 0x1000068000,
        encoder: 0x1000000000,
        aux_fb: 0x10000300000,
        emit_uapi_fields: true,
        reactive_tvb_growth: true,
        ..Default::default()
    };
    let prepared = g17p_dependency_runtime::prepare(
        &mut memory,
        &mut vm,
        &image,
        &mut compute,
        &mut render,
        [&inputs[0], &inputs[1]],
        &p,
        1,
    )?;
    assert_eq!(prepared.root, compute.client.root.root());
    assert!(prepared.boundary()?.valid());
    let mut retirement = prepared.retirement(&memory, &vm)?;
    assert_ne!(prepared.robustness[0], prepared.robustness[1]);
    assert_eq!(vm.fresh.len(), 4);
    for &(va, old, new) in &vm.fresh {
        assert!(d::FRESH_SCHEDULER_PAGES.contains(&va));
        assert_ne!(old & 0x3ffffffc000, new & 0x3ffffffc000);
        assert_eq!(old & !0x3ffffffc000, new & !0x3ffffffc000);
    }
    for i in 0..4 {
        let layout = d::LAYOUTS[i];
        let publication = prepared.publications[i];
        assert_eq!(publication.write_after, 3);
        assert_eq!(
            publication.deferred_outer,
            Some((prepared.channels[i].states[2], if i == 3 { 2 } else { 1 }))
        );
        assert_eq!(
            memory.read_firmware32(vm.physical(&memory, 2, prepared.channels[i].states[2])?)?,
            0
        );
        let head = memory.read_firmware32(vm.physical(&memory, 2, layout.pointers + 0x40)?)?;
        assert_eq!(head, if i == 3 { 0 } else { 3 });
        let descriptor = if i == 0 {
            d::Compute::Opening.descriptor()
        } else if i == 3 {
            d::Compute::Closing.descriptor()
        } else {
            d::RENDER_DESCRIPTORS[i - 1]
        };
        assert_eq!(
            memory.read64(vm.physical(&memory, 2, layout.ring)?)?,
            descriptor
        );
        assert_eq!(vm.bytes(&memory, 2, layout.queue, 0xc0), layout.record());
    }
    assert_eq!(
        vm.bytes(&memory, 2, d::LAYOUTS[3].context_high + 0x200, 0x200),
        vec![0; 0x200]
    );
    assert_eq!(prepared.closing_context.len(), 0x200);
    for (i, (record, slot, _)) in d::SCHEDULERS.into_iter().enumerate() {
        assert_eq!(
            vm.bytes(&memory, 2, record, 0x100),
            d::scheduler(i).unwrap()
        );
        assert_eq!(
            memory.read_firmware32(vm.physical(&memory, 2, slot)?)?,
            if i == 1 { 2 } else { 1 }
        );
    }
    assert!(prepared.joined.active_replaced >= 20);
    for (low, high) in [
        (0x1000080000, d::RENDER_STATUS[0]),
        (0x1000198000, d::LEAVES[0]),
        (0x10001a8000, d::RENDER_STATUS[1]),
    ] {
        assert_eq!(
            compute.client.root.pte(low)? & 0x3ffffffc000,
            vm.physical(&memory, 2, high)?
        );
    }
    for command in [d::Compute::Opening, d::Compute::Closing] {
        let terminator = memory.read64(vm.physical(&memory, 2, command.descriptor() + 0xee0)?)?;
        assert_eq!(
            terminator,
            if command.index() == 0 {
                0x100000b0030
            } else {
                cdms[1] + 0x30
            }
        );
    }
    assert_eq!(prepared.late_aliases, 3);
    let unknown = 0x1000128000;
    let pa = memory.allocate(0x4000)?;
    compute
        .client
        .root
        .leaves
        .insert(unknown, pa | 0xc0000000000c8b);
    // A packed pointer outside the allowed low dependencies cannot silently
    // replace an existing compute aperture owner. No repair happens on error.
    let before = compute.client.root.leaves.clone();
    vm.write(&mut memory, 2, d::SHARED[0] + 0x08, &unknown.to_le_bytes())?;
    assert!(g17p_dependency_runtime::join_late_references(
        &memory,
        &vm,
        &mut compute.client,
        &render.client
    )
    .is_err());
    assert_eq!(before, compute.client.root.leaves);
    println!("PASS: production dependency preparation: four held outer producers, hidden closing inner/context, live joins, compact descriptors and four fresh scheduler owners");
    control_checks(&mut memory, &mut vm, &mut compute, &prepared)?;
    report_checks(&mut memory, &mut vm, &mut compute,
        std::env::args().any(|arg| arg == "--pool-limit"))?;
    use g17p_dependency_retire::{Owner, Reports};
    let reports = |render_complete| Reports {
        valid: true,
        render_complete,
    };
    for i in 0..4 {
        vm.write(&mut memory, 2, d::LAYOUTS[i].pointers, &3u32.to_le_bytes())?;
        for &va in &prepared.channels[i].states {
            vm.write(&mut memory, 2, va, &2u32.to_le_bytes())?;
        }
    }
    let mut poll = |memory: &Memory, render_complete| {
        retirement
            .poll(
                &prepared.observe(memory, &vm).unwrap(),
                reports(render_complete),
            )
            .unwrap()
    };
    assert_eq!(poll(&memory, true), None); // transport alone never completes
    vm.write(
        &mut memory,
        2,
        d::Compute::Opening.status()[1],
        &1u64.to_le_bytes(),
    )?;
    assert_eq!(poll(&memory, true), None); // opening cannot retire first
    vm.write(
        &mut memory,
        2,
        d::Compute::Closing.status()[0],
        &1u64.to_le_bytes(),
    )?;
    assert_eq!(poll(&memory, true), None); // status_a cannot substitute for status_b
    vm.write(
        &mut memory,
        2,
        d::Compute::Closing.status()[1],
        &1u64.to_le_bytes(),
    )?;
    assert_eq!(poll(&memory, false), Some(Owner::Closing));
    retirement.retire(Owner::Closing).unwrap();
    vm.write(&mut memory, 2, d::RENDER_STATUS[0], &1u64.to_le_bytes())?;
    assert_eq!(
        retirement
            .poll(&prepared.observe(&memory, &vm)?, reports(true))
            .unwrap(),
        None
    );
    vm.write(
        &mut memory,
        2,
        d::RENDER_STATUS[1] + 0x38,
        &1u64.to_le_bytes(),
    )?;
    assert_eq!(
        retirement
            .poll(&prepared.observe(&memory, &vm)?, reports(false))
            .unwrap(),
        None
    );
    assert_eq!(
        retirement
            .poll(&prepared.observe(&memory, &vm)?, reports(true))
            .unwrap(),
        Some(Owner::Render)
    );
    retirement.retire(Owner::Render).unwrap();
    // Source transport completion is permanent once observed, even if a later
    // backing generation no longer contains these physical cursor values.
    for i in 0..4 {
        vm.write(&mut memory, 2, d::LAYOUTS[i].pointers, &0u32.to_le_bytes())?;
        for &va in &prepared.channels[i].states {
            vm.write(&mut memory, 2, va, &0u32.to_le_bytes())?;
        }
    }
    assert_eq!(
        retirement
            .poll(&prepared.observe(&memory, &vm)?, reports(true))
            .unwrap(),
        Some(Owner::Opening)
    );
    retirement.retire(Owner::Opening).unwrap();
    assert!(retirement.complete());
    println!("PASS: production snapshot/retirement: closing CL status_b, both render status records and owned growth closure, opening CL; transport completion latched independently");
    Ok(())
}

fn control_checks(
    memory: &mut Memory,
    vm: &mut Vm,
    compute: &mut c::Submission,
    prepared: &g17p_dependency_runtime::Prepared,
) -> Result {
    use g17p_dependency_control::{Host, Notifications};
    struct FirmwareModel {
        pages: Rc<RefCell<BTreeMap<u64, Box<[u8; 0x4000]>>>>,
        counters: [u64; 3],
        messages: Rc<RefCell<Vec<u64>>>,
        waits: Rc<RefCell<usize>>,
    }
    impl Notifications for FirmwareModel {
        fn send(&mut self, message: u64) -> Result {
            self.messages.borrow_mut().push(message);
            Ok(())
        }
        fn wait_step(&mut self) -> Result {
            let step = *self.waits.borrow();
            if step >= 2 {
                return Err(EIO);
            }
            *self.waits.borrow_mut() += 1;
            let mut pages = self.pages.borrow_mut();
            let producer = self.counters[2];
            let value: [u8; 4] = pages[&(producer & !0x3fff)]
                [(producer & 0x3fff) as usize..(producer & 0x3fff) as usize + 4]
                .try_into()
                .unwrap();
            // Separate observations: only one consumer progresses per step.
            let consumer = self.counters[step];
            pages.get_mut(&(consumer & !0x3fff)).unwrap()
                [(consumer & 0x3fff) as usize..(consumer & 0x3fff) as usize + 4]
                .copy_from_slice(&value);
            Ok(())
        }
    }
    let ttbs = memory.allocate(0x4000)?;
    memory.write64(ttbs + 16, (1 << 48) | compute.client.root.root() | 1)?;
    let control = g17p_abi::Channel {
        states: [0xfffffc20c0a00000, 0xfffffc20c0a00004, 0xfffffc20c0a00008],
        ring: 0xfffffc20c0a04000,
    };
    let command = g17p_abi::Channel {
        states: [0xfffffc20c0a08000, 0xfffffc20c0a08004, 0xfffffc20c0a08008],
        ring: 0xfffffc20c0a0c000,
    };
    let report = g17p_abi::Channel {
        states: [0xfffffc20c0a10000, 0xfffffc20c0a14000, 0],
        ring: 0xfffffc20c0a14000 + 256 * 0x48,
    };
    vm.ensure_firmware(memory, control.states[0], 0x4000)?;
    vm.ensure_firmware(memory, control.ring, 0x4000)?;
    vm.ensure_firmware(memory, command.states[0], 0x8000)?;
    vm.ensure_firmware(memory, report.states[0], 0x14000)?;
    let mut service = g17p_growth_runtime::Service::new_dependency(
        memory,
        vm,
        ttbs,
        &compute.client.root,
        command,
        report,
    )?;
    service.begin_compute(1)?;
    service.begin_compute(2)?;
    service.bind_work(d::RENDER_DESCRIPTORS[1])?;
    let counters = control.states.map(|va| vm.physical(memory, 2, va).unwrap());
    let messages = Rc::new(RefCell::new(Vec::new()));
    let waits = Rc::new(RefCell::new(0));
    let model = || FirmwareModel {
        pages: memory.pages.clone(),
        counters,
        messages: messages.clone(),
        waits: waits.clone(),
    };
    let io = model();
    // A full control window rejects before the first byte is published.
    vm.write(memory, 2, control.states[2], &249u32.to_le_bytes())?;
    assert!(Host::new(
        memory,
        vm,
        &mut compute.client.root,
        &mut service,
        control,
        ttbs,
        io
    )
    .is_err());
    assert!(messages.borrow().is_empty());
    vm.write(memory, 2, control.states[2], &0u32.to_le_bytes())?;
    // A stale execution root cannot publish even the opening control body.
    memory.write64(ttbs + 16, 0)?;
    let io = FirmwareModel {
        pages: memory.pages.clone(),
        counters,
        messages: messages.clone(),
        waits: waits.clone(),
    };
    assert!(Host::new(
        memory,
        vm,
        &mut compute.client.root,
        &mut service,
        control,
        ttbs,
        io
    )
    .is_err());
    assert!(messages.borrow().is_empty());
    assert_eq!(vm.bytes(memory, 2, control.ring, 0x40), [0; 0x40]);
    memory.write64(ttbs + 16, (1 << 48) | compute.client.root.root() | 1)?;
    let io = FirmwareModel {
        pages: memory.pages.clone(),
        counters,
        messages: messages.clone(),
        waits: waits.clone(),
    };
    let mut host = Host::new(
        memory,
        vm,
        &mut compute.client.root,
        &mut service,
        control,
        ttbs,
        io,
    )?;
    g17p_dependency_release::release(&mut host, &prepared.boundary()?).unwrap();
    assert!(!host.limit_seen());
    drop(host);
    assert_eq!(*waits.borrow(), 2);
    use g17p_dependency_release::{CONTROL_DOORBELL as CTL, RENDER_DOORBELL as R};
    use g17p_queue::COMPUTE_DOORBELL as C;
    assert_eq!(
        *messages.borrow(),
        [C, CTL, CTL, R, CTL, C, CTL, CTL, CTL, CTL]
    );
    let bodies = [
        d::tick(0).unwrap(),
        d::render_registration(1),
        d::tick(1).unwrap(),
        d::tick(2).unwrap(),
        d::engine_owner(2).unwrap(),
        d::engine_owner(1).unwrap(),
        d::engine_owner(0).unwrap(),
    ];
    for (slot, body) in bodies.iter().enumerate() {
        assert_eq!(
            vm.bytes(memory, 2, control.ring + slot as u64 * 0x40, 0x40),
            body
        );
    }
    assert_eq!(
        memory.read_firmware32(vm.physical(memory, 2, control.states[2])?)?,
        7
    );
    assert_eq!(
        memory.read_firmware32(vm.physical(memory, 2, prepared.channels[3].states[2])?)?,
        2
    );
    assert_eq!(
        memory.read_firmware32(vm.physical(memory, 2, d::LAYOUTS[3].pointers + 0x40)?)?,
        3
    );
    assert_eq!(
        vm.bytes(memory, 2, d::LAYOUTS[3].context_high + 0x200, 0x200),
        &*prepared.closing_context
    );
    println!("PASS: production owned control/report adapter: seven exact control bodies, ten ordered notifications, independently advancing consumers, admission rejection and deferred closing publication");
    Ok(())
}

fn report_checks(memory: &mut Memory, vm: &mut Vm, compute: &mut c::Submission, bounded: bool) -> Result {
    let ttbs = memory.allocate(0x4000)?;
    memory.write64(ttbs + 16, (1 << 48) | compute.client.root.root() | 1)?;
    let command = g17p_abi::Channel {
        states: [0xfffffc20c0940000, 0xfffffc20c0940004, 0xfffffc20c0940008],
        ring: 0xfffffc20c0944000,
    };
    let base = 0xfffffc20c0950000;
    let report = g17p_abi::Channel {
        states: [base, base + 0x4000, 0],
        ring: base + 0x4000 + 256 * 0x48,
    };
    vm.ensure_firmware(memory, command.states[0], 0x8000)?;
    vm.ensure_firmware(memory, base, 0x14000)?;
    let mut service = g17p_growth_runtime::Service::new_dependency(
        memory,
        vm,
        ttbs,
        &compute.client.root,
        command,
        report,
    )?;
    if bounded {
        service.set_source_pool_limits(18, [u32::MAX; 2])?;
    }
    service.expect_dependency_receipt(1)?;
    service.begin_compute(1)?;
    service.begin_compute(2)?;
    service.bind_work(d::RENDER_DESCRIPTORS[1])?;
    let mut cl = [0u8; 0x48];
    cl[..4].copy_from_slice(&1u32.to_le_bytes());
    cl[4..8].copy_from_slice(&1u32.to_le_bytes());
    let mut growth = [0u8; 0x48];
    growth[..4].copy_from_slice(&6u32.to_le_bytes());
    growth[4..8].copy_from_slice(&1u32.to_le_bytes());
    growth[56..64].copy_from_slice(&1u64.to_le_bytes());
    growth[64..].copy_from_slice(&1u64.to_le_bytes());
    let mut receipt = [0u8; 0x48];
    receipt[..0x28].copy_from_slice(&d::render_receipt(1));
    let mut render = [0u8; 0x48];
    render[..4].copy_from_slice(&1u32.to_le_bytes());
    render[4..8].copy_from_slice(&6u32.to_le_bytes());
    let mut closing = cl;
    closing[4..8].copy_from_slice(&8u32.to_le_bytes());
    for (i, body) in [cl, growth, receipt, render, closing].iter().enumerate() {
        vm.write(memory, 2, report.states[1] + i as u64 * 0x48, body)?;
    }
    vm.write(memory, 2, report.states[0] + 0x20, &5u32.to_le_bytes())?;
    for i in 0..5 {
        let action = service.step_dependency(memory, vm, &mut compute.client.root, ttbs)?;
        if i == 1 {
            assert!(matches!(
                action,
                g17p_growth_runtime::Action::Reply {
                    old: 8,
                    new: 18,
                    refused: false,
                    ..
                }
            ));
        } else {
            assert!(matches!(action, g17p_growth_runtime::Action::Consumed));
        }
    }
    assert_eq!(service.compute_terminals(), 2);
    assert_eq!(service.terminals(), 1);
    assert_eq!(service.cursor(), 5);
    // Run both the real allocator-error path and the source bounded-policy
    // path. Only this graph's fragment
    // event 2 can close it; event 1 belongs to the ordinary graph and must keep
    // its report credit. Context event_slot remains 1 in both graphs.
    let mut next_growth = growth;
    next_growth[12..16].copy_from_slice(&1u32.to_le_bytes());
    vm.write(memory, 2, report.states[1] + 5 * 0x48, &next_growth)?;
    vm.write(memory, 2, report.states[0] + 0x20, &6u32.to_le_bytes())?;
    memory.fail_next_allocation = !bounded;
    let allocations_before = memory.pages.borrow().len();
    assert!(matches!(
        service.step_dependency(memory, vm, &mut compute.client.root, ttbs)?,
        g17p_growth_runtime::Action::Reply {
            old: 18,
            new: 18,
            refused: true,
            ..
        }
    ));
    assert_eq!(memory.pages.borrow().len(), allocations_before);
    let limit = |event: u32| {
        let mut body = [0; 0x48];
        for (offset, word) in [(0, 7u32), (8, 1), (12, event)] {
            body[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        }
        for (offset, word) in [
            (16, 0xfffffc2000000100u64),
            (24, 1),
            (40, d::RENDER_DESCRIPTORS[1]),
            (48, event as u64),
            (56, 1),
        ] {
            body[offset..offset + 8].copy_from_slice(&word.to_le_bytes());
        }
        body
    };
    // Identity rejection is checked without reviving a quarantined reader.
    // Production dispatch failure permanently freezes that reader, which the
    // duplicate-report cases below exercise independently of this valid wave.
    assert!(g17p_growth::Owner { vm: 1, pool: 0 }
        .limit(
            &limit(1),
            &[0xfffffc2000000100, 0xfffffc2000000200],
            d::RENDER_DESCRIPTORS[1],
            2
        )
        .is_none());
    assert_eq!(service.cursor(), 6);
    vm.write(memory, 2, report.states[1] + 6 * 0x48, &limit(2))?;
    vm.write(memory, 2, report.states[0] + 0x20, &7u32.to_le_bytes())?;
    assert!(matches!(
        service.step_dependency(memory, vm, &mut compute.client.root, ttbs)?,
        g17p_growth_runtime::Action::Limit
    ));
    assert_eq!(service.cursor(), 7);
    vm.write(memory, 2, report.states[1] + 7 * 0x48, &limit(2))?;
    vm.write(memory, 2, report.states[0] + 0x20, &8u32.to_le_bytes())?;
    assert!(service
        .step_dependency(memory, vm, &mut compute.client.root, ttbs)
        .is_err());
    assert_eq!(service.cursor(), 7);
    assert_eq!(
        compute
            .client
            .root
            .leaves
            .keys()
            .filter(|&&va| va >= g17p_growth::GROWTH_BASE && va < g17p_growth::GROWTH_END)
            .count(),
        80
    );
    // Repeated class receipt and a terminal without an owned CL queue must
    // fail without advancing the report credit or satisfying another owner.
    for body in [receipt, cl] {
        vm.write(memory, 2, report.states[1] + 7 * 0x48, &body)?;
        vm.write(memory, 2, report.states[0] + 0x20, &8u32.to_le_bytes())?;
        assert!(service
            .step_dependency(memory, vm, &mut compute.client.root, ttbs)
            .is_err());
        assert_eq!(service.cursor(), 7);
    }
    assert!(service
        .step(memory, vm, &mut compute.client.root, ttbs, Some(1))
        .is_err());
    // Preserve standalone routing: a compute-only wait cannot approve render
    // growth, and a render-only service cannot consume a non-render terminal.
    for va in [0xfffffc2001610000, 0xfffffc2001610004] {
        vm.write(memory, 2, va, &8u32.to_le_bytes())?;
    }
    vm.write(memory, 2, report.states[0] + 0x20, &0u32.to_le_bytes())?;
    // The standalone profile has its own source list, with retained TVB
    // leaves already admitted by production preparation. Copy the eight
    // initial source IDs, not allocator or validation substitutes.
    let mut initial = [0; 64];
    for (index, word) in initial.chunks_exact_mut(8).enumerate() {
        word.copy_from_slice(
            &memory
                .read64(vm.physical(memory, 2, d::LEAVES[1] + index as u64 * 8)?)?
                .to_le_bytes(),
        );
    }
    vm.write(memory, 2, 0xfffffc20c0838000, &initial)?;
    let mut ordinary =
        g17p_growth_runtime::Service::new(memory, vm, ttbs, &compute.client.root, command, report)?;
    ordinary.pools[0].retired = true;
    let mut logical = g17p_user_vm::UserVm::new(0x10099000000);
    let sentinel = (0x11000000000, 0x10099080000 | 0xc0000000000c8b);
    logical.leaves.insert(sentinel.0, sentinel.1);
    let mut expected = BTreeMap::from([sentinel]);
    for id in initial.chunks_exact(8) {
        let base = 0x1000000000 + u64::from_le_bytes(id.try_into().unwrap()) * 0x8000;
        for offset in (0..0x20000).step_by(0x4000) {
            let pte = compute.client.root.pte(base + offset)?;
            // The source initial tranche is sparse. Absent leaves are not
            // owned backing and must remain absent in the other logical VM.
            if pte != 0 && memory.word64(pte & 0x3ffffffc000).is_ok() {
                expected.insert(base + offset, pte);
            }
        }
    }
    ordinary.mirror_retained_mappings(&mut logical)?;
    assert_eq!(logical.leaves, expected);
    assert_eq!(logical.pte(sentinel.0)?, sentinel.1);
    let retained = logical.leaves.clone();
    ordinary.mirror_retained_mappings(&mut logical)?;
    assert_eq!(logical.leaves, retained);
    let address = *logical.leaves.keys().find(|&&va| va != sentinel.0).unwrap();
    logical.leaves.insert(address, logical.pte(address)? ^ 0x4000);
    let conflict = logical.leaves.clone();
    assert_eq!(ordinary.mirror_retained_mappings(&mut logical), Err(EIO));
    assert_eq!(logical.leaves, conflict);
    ordinary.pools[0].retired = false;
    assert_eq!(ordinary.mirror_retained_mappings(&mut logical), Err(EBUSY));
    assert_eq!(logical.leaves, conflict);
    println!("PASS retained growth mappings into logical roots: idempotent owned leaves, caller sentinel preserved, conflicting owner and pending work rejected before changes");
    assert!(ordinary.expect_dependency_receipt(1).is_err());
    assert!(ordinary
        .step_dependency(memory, vm, &mut compute.client.root, ttbs)
        .is_err());
    ordinary.begin_compute(9)?;
    vm.write(memory, 2, report.states[1], &growth)?;
    vm.write(memory, 2, report.states[0] + 0x20, &1u32.to_le_bytes())?;
    assert!(ordinary
        .step(memory, vm, &mut compute.client.root, ttbs, Some(9))
        .is_err());
    assert_eq!(ordinary.cursor(), 0);
    vm.write(memory, 2, report.states[1], &cl)?;
    assert!(ordinary
        .step(memory, vm, &mut compute.client.root, ttbs, None)
        .is_err());
    assert_eq!(ordinary.cursor(), 0);
    // A failed report service remains quarantined. Admit the valid case
    // through a fresh reader/owner instead of reviving the failed service.
    vm.write(memory, 2, report.states[0] + 0x20, &0u32.to_le_bytes())?;
    let mut ordinary =
        g17p_growth_runtime::Service::new(memory, vm, ttbs, &compute.client.root, command, report)?;
    ordinary.begin_compute(9)?;
    vm.write(memory, 2, report.states[0] + 0x20, &1u32.to_le_bytes())?;
    assert!(matches!(
        ordinary.step(memory, vm, &mut compute.client.root, ttbs, Some(9))?,
        g17p_growth_runtime::Action::Consumed
    ));
    assert_eq!(ordinary.compute_terminals(), 1);
    assert_eq!(ordinary.terminals(), 0);
    println!("PASS: mixed native report FIFO: CL/growth/owned receipt/render/CL; duplicate receipt/unowned CL retain credits; allocation refusal requires owned event-2 limit, wrong event/duplicate retain credits");
    Ok(())
}
