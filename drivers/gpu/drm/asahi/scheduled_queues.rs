// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! G16C queue storage built through the common Memory/UAT owners.
//! No producers or context roots are published here. Detached preparation
//! releases failed backing attempts before any resource leaf is installed.
//! After adoption, Memory retains backing until stopped cleanup. Session
//! invalidates UAT before returning live storage.

use super::{g17p_memory::Memory, g17p_queue as q, g17p_vm::Vm};
use kernel::prelude::*;

const PAGE: u64 = 0x4000;
pub(crate) const START: u64 = 0xfffffc2208000000;
pub(crate) const END: u64 = 0xfffffc220f000000;
pub(crate) const STRIDE: u64 = 0x2c000;
pub(crate) const SIZE: u64 = STRIDE * 3;
const PRIVATE: u64 = 0x00c0000000000443;
const SHARED: u64 = 0x00c000000000044b;
const GPU_READ: u64 = 0x0080000000000c8b;

/// Session-wide engine arrays outlive every queue/context close. In particular
/// recycling a queue must never zero the SKSM state still bound to an engine.
pub(crate) struct EngineScratchG16C {
    pub(crate) high: [u64; 4],
    pub(crate) low: [u64; 4],
}
pub(crate) struct PreparedEngineScratchG16C {
    value: EngineScratchG16C,
    backing: [u64; 4],
}
impl PreparedEngineScratchG16C {
    pub(crate) fn install(self, memory: &mut Memory, vm: &mut Vm) -> Result<EngineScratchG16C> {
        for i in 0..4 {
            vm.span(memory, 2, self.value.high[i], self.backing[i], 0x20000, SHARED)?;
            vm.span(memory, 0, self.value.low[i], self.backing[i], 0x20000, GPU_READ)?;
        }
        vm.flush_tables(memory)?;
        Ok(self.value)
    }
}
impl EngineScratchG16C {
    pub(crate) fn prepare(memory: &mut Memory, vm: &mut Vm) -> Result<(Memory, PreparedEngineScratchG16C)> {
        let high = core::array::from_fn(|i| 0xfffffc2206000000 + i as u64 * 0x20000);
        let low = high.map(|address| 0x7400000000 + address - 0xfffffc2200000000);
        for i in 0..4 {
            empty(memory, vm, 2, high[i], 0x20000)?;
            empty(memory, vm, 0, low[i], 0x20000)?;
            vm.prepare_g16c_empty(memory, 2, high[i], 0x20000)?;
            vm.prepare_g16c_empty(memory, 0, low[i], 0x20000)?;
        }
        let mut staged = Memory::detached();
        let mut backing = [0; 4];
        for physical in &mut backing {
            *physical = staged.allocate(0x20000)?;
            staged.clean(*physical, 0x20000)?;
        }
        Ok((staged, PreparedEngineScratchG16C { value: Self { high, low }, backing }))
    }
    #[allow(dead_code)]
    pub(crate) fn new(memory: &mut Memory, vm: &mut Vm) -> Result<Self> {
        let (staged, prepared) = Self::prepare(memory, vm)?;
        memory.absorb(staged)?;
        prepared.install(memory, vm)
    }
}

pub(crate) struct LaneG16C {
    pub(crate) queue: u64,
    pub(crate) pointers: u64,
    pub(crate) ring: u64,
    pub(crate) scratch_high: u64,
    pub(crate) scratch_low: u64,
    pub(crate) capacity: u32,
    pub(crate) head: u32,
    pub(crate) first: bool,
    pub(crate) priority: u8,
    pub(crate) stage: u8,
}

pub(crate) struct QueueSetG16C {
    pub(crate) base: u64,
    pub(crate) lanes: [LaneG16C; 3],
    pub(crate) jobs: u64,
    pub(crate) context: u64,
    pub(crate) uuid: u16,
    backing: [[u64; 4]; 3],
    aliases: [u64; 3],
}

fn empty(memory: &Memory, vm: &Vm, group: usize, start: u64, size: u64) -> Result {
    for offset in (0..size).step_by(PAGE as usize) {
        if vm.pte(memory, group, start + offset)? != 0 {
            return Err(EBUSY);
        }
    }
    Ok(())
}

impl QueueSetG16C {
    pub(crate) fn prepare(
        memory: &mut Memory,
        vm: &mut Vm,
        base: u64,
        priority: u8,
        uuid: u16,
        scratch_aliases: Option<[u64; 3]>,
    ) -> Result<(Memory, Self)> {
        vm.require_g16c_graph()?;
        if priority >= 4 || base < START || base > END - SIZE || base & (PAGE - 1) != 0 {
            return Err(EINVAL);
        }
        let lows = scratch_aliases.unwrap_or_else(|| {
            core::array::from_fn(|i| 0x7400000000 + (base - 0xfffffc2200000000) + i as u64 * STRIDE)
        });
        for (i, &low) in lows.iter().enumerate() {
            let end = low.checked_add(0x20000).ok_or(EINVAL)?;
            if low & (PAGE - 1) != 0
                || end > 1 << 42
                || lows[..i]
                    .iter()
                    .any(|&other| low < other + 0x20000 && other < end)
            {
                return Err(EINVAL);
            }
        }
        // Validate every high mapping and every context-zero alias before
        // allocating backing or changing a PTE. Application roots are untouched.
        empty(memory, vm, 2, base, SIZE)?;
        for low in lows {
            empty(memory, vm, 0, low, 0x20000)?;
        }
        let jobs = base + 0x4100;
        let context = base + 0x140;
        let lanes = core::array::from_fn(|i| {
            let at = base + i as u64 * STRIDE;
            LaneG16C {
                queue: at,
                pointers: at + 0x4000,
                ring: at + 0x8000,
                scratch_high: at + 0xc000,
                scratch_low: lows[i],
                capacity: 0x500,
                head: 0,
                first: true,
                priority,
                stage: i as u8,
            }
        });
        vm.prepare_g16c_empty(memory, 2, base, SIZE as usize)?;
        for low in lows { vm.prepare_g16c_empty(memory, 0, low, 0x20000)?; }
        let mut staged = Memory::detached();
        let mut backing = [[0; 4]; 3];
        for (i, _) in lanes.iter().enumerate() {
            for (j, (_, size, _)) in [
                (0, PAGE, PRIVATE),
                (0x4000, PAGE, SHARED),
                (0x8000, PAGE, PRIVATE),
                (0xc000, 0x20000, SHARED),
            ].into_iter().enumerate() {
                let physical = staged.allocate(size as usize)?;
                backing[i][j] = physical;
                staged.clean(physical, size as usize)?;
            }
        }
        Ok((staged, Self { base, lanes, jobs, context, uuid, backing, aliases: lows }))
    }

    pub(crate) fn install(mut self, memory: &mut Memory, vm: &mut Vm, priority: u8) -> Result<Self> {
        for (i, lane) in self.lanes.iter().enumerate() {
            for (j, (offset, size, flags)) in [
                (0, PAGE, PRIVATE), (0x4000, PAGE, SHARED),
                (0x8000, PAGE, PRIVATE), (0xc000, 0x20000, SHARED),
            ].into_iter().enumerate() {
                vm.span(memory, 2, lane.queue + offset, self.backing[i][j], size as usize, flags)?;
                if offset == 0xc000 {
                    vm.span(memory, 0, lane.scratch_low, self.backing[i][j], size as usize, GPU_READ)?;
                }
            }
        }
        self.initialize(memory, vm, priority)?;
        vm.flush_tables(memory)?;
        Ok(self)
    }

    #[allow(dead_code)]
    pub(crate) fn new(memory: &mut Memory, vm: &mut Vm, base: u64, priority: u8,
        uuid: u16, scratch_aliases: Option<[u64; 3]>) -> Result<Self> {
        let (staged, prepared) = Self::prepare(memory, vm, base, priority, uuid, scratch_aliases)?;
        memory.absorb(staged)?;
        prepared.install(memory, vm, priority)
    }

    /// Caller proves GPU/CPU retirement and the context-close/TTBR receipt.
    /// Check every retained mapping before zeroing any allocation. Canonical
    /// engine arrays are separate; these are only the queue's own allocations.
    pub(crate) fn reset(&mut self, memory: &mut Memory, vm: &Vm, priority: u8) -> Result {
        if priority >= 4 { return Err(EINVAL); }
        for i in 0..3 {
            for (j, (offset, size, flags)) in [(0, PAGE, PRIVATE),
                (0x4000, PAGE, SHARED), (0x8000, PAGE, PRIVATE),
                (0xc000, 0x20000, SHARED)].into_iter().enumerate() {
                for page in (0..size).step_by(PAGE as usize) {
                    let pa = self.backing[i][j] + page;
                    if vm.pte(memory, 2, self.base + i as u64 * STRIDE + offset + page)? != pa | flags {
                        return Err(EIO);
                    }
                    if j == 3 && vm.pte(memory, 0, self.aliases[i] + page)? != pa | GPU_READ {
                        return Err(EIO);
                    }
                }
            }
        }
        for i in 0..3 {
            for (j, size) in [PAGE, PAGE, PAGE, 0x20000].into_iter().enumerate() {
                memory.zero(self.backing[i][j], size as usize)?;
                memory.clean(self.backing[i][j], size as usize)?;
            }
        }
        self.initialize(memory, vm, priority)
    }

    fn initialize(&mut self, memory: &mut Memory, vm: &Vm, priority: u8) -> Result {
        let priority_words = q::priority_profile(priority as u32).map_err(|_| EINVAL)?;
        for lane in &mut self.lanes {
            lane.head = 0;
            lane.first = true;
            lane.priority = priority;
            let record = q::wire::QueueRecordG16C::new(q::wire::QueueParameters {
                pointers: lane.pointers,
                ring: lane.ring,
                jobs: self.jobs,
                context: self.context,
                uuid: self.uuid as u32,
                priority: priority as u32,
                unk_2c: priority as u32,
                unk_30: u64::from_le_bytes(priority_words[8..16].try_into().unwrap()),
                unk_38: u32::from_le_bytes(priority_words[16..20].try_into().unwrap()),
                prio5: u32::from_le_bytes(priority_words[24..28].try_into().unwrap()),
                unk_94: 0,
            });
            vm.write(memory, 2, lane.queue, record.as_bytes())?;
            let pointers = q::wire::QueuePointersG16C::with_write(0);
            vm.write(memory, 2, lane.pointers, pointers.as_bytes())?;
        }
        vm.write(memory, 2, self.jobs, &q::job_list(self.jobs))?;
        vm.write(
            memory,
            2,
            self.context,
            q::wire::QueueContextG16C::default().as_bytes(),
        )?;
        Ok(())
    }
}
