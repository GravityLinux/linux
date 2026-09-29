// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Retained synchronous growth service. Report pointers never select storage.

use super::{
    g17p_abi::Channel,
    g17p_growth as g,
    g17p_memory::{self, Memory},
    g17p_user_vm::UserVm,
    g17p_vm::Vm,
};
use kernel::prelude::*;
const STATE: u64 = 0xfffffc2001610000;
const LIST: u64 = 0xfffffc20c0838000;
const CAPACITY: u32 = 0x4000 / 8;

pub(crate) enum Action {
    Idle,
    Consumed,
    Reply {
        counter: u32,
        old: u32,
        new: u32,
        refused: bool,
    },
    Limit,
}
pub(crate) struct Service {
    command: Channel,
    report: Channel,
    root: u64,
    cursor: u32,
    counter: u32,
    refused: bool,
    limited: bool,
    terminals: u32,
}
fn word(memory: &Memory, vm: &Vm, va: u64) -> Result<u32> {
    memory.read_firmware32(vm.physical(memory, 2, va)?)
}
fn read(memory: &Memory, vm: &Vm, va: u64, body: &mut [u8]) -> Result {
    if va & 7 != 0 || body.len() % 8 != 0 {
        return Err(EINVAL);
    }
    for (i, qword) in body.chunks_exact_mut(8).enumerate() {
        let pa = vm.physical(memory, 2, va + i as u64 * 8)?;
        memory.invalidate(pa, 8)?;
        qword.copy_from_slice(&memory.read64(pa)?.to_le_bytes());
    }
    Ok(())
}
fn verify_root(memory: &Memory, ttbs: u64, root: u64) -> Result {
    memory.invalidate(ttbs + 16, 8)?;
    if memory.read64(ttbs + 16)? != ((1 << 48) | root | 1) {
        return Err(EIO);
    }
    Ok(())
}
impl Service {
    pub(crate) fn cursor(&self) -> u32 {
        self.cursor
    }
    pub(crate) fn new(
        memory: &Memory,
        vm: &Vm,
        ttbs: u64,
        root: u64,
        command: Channel,
        report: Channel,
    ) -> Result<Self> {
        verify_root(memory, ttbs, root)?;
        if report.states[1].checked_add(256 * 0x48) != Some(report.ring)
            || word(memory, vm, STATE)? != 8
            || word(memory, vm, STATE + 4)? != 8
        {
            return Err(EIO);
        }
        // Skip the already-observed opening receipt, as GrowthService does.
        let cursor = word(memory, vm, report.states[0] + 0x20)?;
        if cursor >= 256 {
            return Err(EIO);
        }
        Ok(Self {
            command,
            report,
            root,
            cursor,
            counter: 0,
            refused: false,
            limited: false,
            terminals: 0,
        })
    }
    fn consume(&mut self, memory: &mut Memory, vm: &Vm, next: u32) -> Result {
        vm.write(memory, 2, self.report.states[0], &next.to_le_bytes())?;
        g17p_memory::sync();
        self.cursor = next;
        Ok(())
    }
    fn allocate(&self, memory: &mut Memory, vm: &Vm, root: &mut UserVm, old: u32) -> Result<u32> {
        if old > CAPACITY {
            return Err(EIO);
        }
        let new = old.checked_add(g::INCREMENT as u32).ok_or(EIO)?;
        if new > CAPACITY {
            return Err(ENOMEM);
        }
        let addresses = g::block_addresses(self.counter).ok_or(EIO)?;
        let mut prior = KVec::with_capacity(old as usize, GFP_KERNEL)?;
        for i in 0..old {
            let mut body = [0; 8];
            read(memory, vm, LIST + i as u64 * 8, &mut body)?;
            let id = u64::from_le_bytes(body);
            let address = id
                .checked_mul(g::UNIT)
                .and_then(|a| a.checked_add(g::CONTEXT_BASE))
                .ok_or(EIO)?;
            prior.push(address, GFP_KERNEL)?;
        }
        if !g::block_list(&prior, &addresses) {
            return Err(EIO);
        }
        let size = g::INCREMENT * g::BLOCK as usize;
        let physical = memory.allocate(size)?;
        if physical % g::UNIT != 0 {
            return Err(EIO);
        }
        memory.clean(physical, size)?;
        let mut pages = KVec::with_capacity(size / g::PAGE as usize, GFP_KERNEL)?;
        for (i, va) in addresses.into_iter().enumerate() {
            for offset in (0..g::BLOCK).step_by(g::PAGE as usize) {
                pages.push(
                    (va + offset, physical + i as u64 * g::BLOCK + offset),
                    GFP_KERNEL,
                )?;
            }
        }
        // grow() allocates/preflights all table paths before any live store.
        // Backing stays owned by Memory even if preparation/publication fails.
        root.grow(&pages)?;
        let mut ids = [0; g::INCREMENT * 8];
        for (i, address) in addresses.into_iter().enumerate() {
            ids[i * 8..i * 8 + 8].copy_from_slice(&g::block_id(address).ok_or(EIO)?.to_le_bytes());
        }
        vm.write(memory, 2, LIST + old as u64 * 8, &ids)?;
        g17p_memory::sync();
        let mut counts = [0; 8];
        counts[..4].copy_from_slice(&new.to_le_bytes());
        counts[4..].copy_from_slice(&new.to_le_bytes());
        vm.write(memory, 2, STATE, &counts)?;
        g17p_memory::sync();
        Ok(new)
    }
    pub(crate) fn step(
        &mut self,
        memory: &mut Memory,
        vm: &Vm,
        root: &mut UserVm,
        ttbs: u64,
    ) -> Result<Action> {
        let tail = word(memory, vm, self.report.states[0] + 0x20)?;
        if tail >= 256 {
            return Err(EIO);
        }
        if tail == self.cursor {
            return Ok(Action::Idle);
        }
        let mut body = [0; 0x48];
        read(
            memory,
            vm,
            self.report.states[1] + self.cursor as u64 * 0x48,
            &mut body,
        )?;
        let opcode = u32::from_le_bytes(body[..4].try_into().unwrap());
        let next = (self.cursor + 1) & 255;
        let owner = g::Owner { vm: 1, pool: 0 };
        if opcode == 7 {
            if !self.refused
                || self.limited
                || owner
                    .limit(
                        &body,
                        &[0xfffffc2000000100],
                        super::g17p_render_runtime::DESCRIPTORS[1],
                        1,
                    )
                    .is_none()
            {
                return Err(EIO);
            }
            // Qualified consume-only closure: no guessed command/doorbell.
            self.limited = true;
            self.consume(memory, vm, next)?;
            return Ok(Action::Limit);
        }
        if opcode == 4 {
            // The ABI has no fault address/owner. Retain this report and all
            // active backing; the sole live render fails rather than guessing.
            let _payload_free = g::fatal(&body);
            return Err(EIO);
        }
        if opcode == 1 {
            if body[4..16] != [3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] {
                return Err(EIO);
            }
            self.terminals = self.terminals.checked_add(1).ok_or(EIO)?;
            self.consume(memory, vm, next)?;
            return Ok(Action::Consumed);
        }
        if !owner.request(&body, self.counter) || self.counter >= g::REQUEST_LIMIT || self.limited {
            return Err(EIO);
        }
        verify_root(memory, ttbs, self.root)?;
        if root.root() != self.root {
            return Err(EIO);
        }
        let head = word(memory, vm, self.command.states[0])?;
        let slot = word(memory, vm, self.command.states[2])?;
        if head >= 256 || slot >= 256 || (slot + 1) & 255 == head {
            return Err(EIO);
        }
        let old = word(memory, vm, STATE)?;
        if old != word(memory, vm, STATE + 4)? {
            return Err(EIO);
        }
        let (new, refused) = match self.allocate(memory, vm, root, old) {
            Ok(new) => (new, false),
            Err(e) if e == ENOMEM => (old, true),
            Err(e) => return Err(e),
        };
        let command = owner.reply(&body, self.counter, !refused).ok_or(EIO)?;
        vm.write(memory, 2, self.command.ring + slot as u64 * 0x40, &command)?;
        // Command body -> report credit -> barrier -> command producer ->
        // barrier -> caller's doorbell. Match the source service ordering.
        self.consume(memory, vm, next)?;
        vm.write(
            memory,
            2,
            self.command.states[2],
            &((slot + 1) & 255).to_le_bytes(),
        )?;
        g17p_memory::sync();
        let counter = self.counter;
        self.counter += 1;
        self.refused = refused;
        Ok(Action::Reply {
            counter,
            old,
            new,
            refused,
        })
    }
}
