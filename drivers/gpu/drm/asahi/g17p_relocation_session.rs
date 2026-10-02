// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Owned kernel RAM/table adapters for g17p_lifecycle.py migration diagnostics.

use super::super::{
    g17p_context::NativeComputeContexts,
    g17p_lifecycle::{self as life, AliasMove, RelocationHost},
    g17p_memory::{Memory, Word64},
    g17p_user_vm::TableWord,
    g17p_vm::Vm,
};
use super::{compute, render, Phase, Session};
use kernel::prelude::*;
const PAGE: u64 = 0x4000;
const ADDRESS: u64 = 0x000003ffffffc000;
enum Word<'a> {
    Memory(Word64<'a>),
    User(TableWord<'a>),
}
impl Word<'_> {
    fn load(&self) -> u64 {
        match self {
            Self::Memory(word) => word.load(),
            Self::User(word) => word.load(),
        }
    }
    fn store(&self, value: u64) {
        match self {
            Self::Memory(word) => word.store(value),
            Self::User(word) => word.store(value),
        }
    }
}
struct Migration<'a> {
    memory: &'a mut Memory,
    vm: &'a Vm,
    compute: &'a mut compute::Submission,
    render: Option<&'a render::Submission>,
    dormant: Option<&'a render::Submission>,
    render_clients: &'a [compute::Client],
    contexts: Option<&'a NativeComputeContexts>,
    ttbs: u64,
}
impl Migration<'_> {
    fn word(&self, address: u64) -> Result<Word<'_>> {
        if let Ok(word) = self.memory.word64(address) {
            return Ok(Word::Memory(word));
        }
        if let Ok(word) = self.compute.client.root.table_word(address) {
            return Ok(Word::User(word));
        }
        for render in [self.render, self.dormant].into_iter().flatten() {
            if let Ok(word) = render.client.root.table_word(address) {
                return Ok(Word::User(word));
            }
        }
        for client in self.render_clients {
            if let Ok(word) = client.root.table_word(address) {
                return Ok(Word::User(word));
            }
        }
        if let Some(contexts) = self.contexts {
            for context in &contexts.contexts {
                if let Ok(word) = context
                    .retained_root(&self.compute.client.root)?
                    .table_word(address)
                {
                    return Ok(Word::User(word));
                }
            }
        }
        Err(EINVAL)
    }
}
impl RelocationHost for Migration<'_> {
    fn firmware_root(&self) -> u64 {
        self.vm.firmware_root()
    }
    fn compute_root(&self) -> u64 {
        self.compute.client.root.root()
    }
    fn ttbs(&self) -> Result<[u64; 128]> {
        let mut values = [0; 128];
        for (index, value) in values.iter_mut().enumerate() {
            *value = self.memory.word64(self.ttbs + index as u64 * 8)?.load();
        }
        Ok(values)
    }
    fn leaf(&self, group: usize, va: u64) -> Result<(u64, u64)> {
        self.vm.leaf_record(self.memory, group, va)
    }
    fn table_word(&self, address: u64) -> Result<u64> {
        Ok(self.word(address)?.load())
    }
    fn copy_page(&mut self, pa: u64) -> Result<u64> {
        if pa == 0 || pa & (PAGE - 1) != 0 {
            return Err(EINVAL);
        }
        let mut body = KVVec::with_capacity(PAGE as usize, GFP_KERNEL)?;
        body.resize(PAGE as usize, 0, GFP_KERNEL)?;
        for (index, word) in body.chunks_exact_mut(8).enumerate() {
            word.copy_from_slice(
                &self
                    .memory
                    .word64(pa + index as u64 * 8)?
                    .load()
                    .to_le_bytes(),
            );
        }
        let copy = self.memory.allocate(PAGE as usize)?;
        if copy == pa || copy == 0 || copy & !ADDRESS != 0 {
            return Err(EIO);
        }
        self.memory.write(copy, &body)?;
        self.memory.clean(copy, PAGE as usize)?;
        Ok(copy)
    }
    fn publish_aliases(&mut self, aliases: &[AliasMove]) -> Result {
        let mut words = KVec::with_capacity(aliases.len(), GFP_KERNEL)?;
        for (index, alias) in aliases.iter().enumerate() {
            if aliases[..index]
                .iter()
                .any(|row| row.address == alias.address)
            {
                return Err(EINVAL);
            }
            let word = self.word(alias.address)?;
            if word.load() != alias.before {
                return Err(EIO);
            }
            words.push((word, alias.after), GFP_KERNEL)?;
        }
        // Every alias owner is checked and pinned before the first live store.
        for (word, value) in &words {
            word.store(*value);
        }
        Vm::invalidate_gpu();
        for (word, value) in &words {
            if word.load() != *value {
                return Err(EIO);
            }
        }
        Ok(())
    }
    fn publish_compute_root(&mut self, before: &[u64; 128]) -> Result<u64> {
        if self.contexts.is_some() || self.ttbs()? != *before {
            return Err(EBUSY);
        }
        let low2 = self.memory.word64(self.ttbs + 32)?;
        let low3 = self.memory.word64(self.ttbs + 48)?;
        let (index, copy) = self.compute.client.root.clone_root_page()?;
        // No fallible allocation or owner lookup remains after the first TTB
        // store. The root owner retains the old page and every child table.
        low2.store((before[4] & !ADDRESS) | copy);
        low3.store((before[6] & !ADDRESS) | copy);
        self.compute.client.root.publish_root_clone(index);
        Vm::invalidate_gpu();
        Ok(copy)
    }
}
impl Session {
    pub(super) fn run_cleanup_diagnostics(&mut self, index: usize) -> Result {
        let mask = *crate::module_parameters::cleanup_diagnostics.value();
        if self.phase != Phase::Running || mask & !15 != 0 {
            return Err(EINVAL);
        }
        let receipt = self.cleanup.cleanup_receipt(index)?;
        let mut host = Migration {
            memory: self.memory.as_mut().ok_or(EINVAL)?,
            vm: self.vm.as_ref().ok_or(EINVAL)?,
            compute: self.compute.as_mut().ok_or(EINVAL)?,
            render: self.render.as_ref(),
            dormant: self.dormant_render.as_ref(),
            render_clients: &self.render_clients,
            contexts: self.compute_contexts.as_ref(),
            ttbs: self.ttbs,
        };
        let result = (|| {
            if mask & 1 != 0 {
                life::relocate_completed_compute_descriptor(
                    &mut host,
                    receipt,
                    0xfffffc20c0358000,
                    0x7000340000,
                )?;
            }
            if mask & 2 != 0 {
                life::relocate_completed_compute_page(
                    &mut host,
                    receipt,
                    0xfffffc2000278000,
                    0x70004d8000,
                )?;
            }
            if mask & 4 != 0 {
                life::relocate_completed_transport_page(&mut host, receipt, 0xfffffc2001658000)?;
            }
            if mask & 8 != 0 {
                life::relocate_completed_compute_root(&mut host, receipt)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result
    }
}
