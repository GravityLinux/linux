// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Source adapter's owned render fault arm/inject/restore lifecycle.
//! Diagnostics use caller GEMs and retain their original backing and leaves.

use super::super::{g17p_drm::Object, g17p_render::Parameters};
use super::{Phase, Session};
use kernel::{prelude::*, sync::aref::ARef, uapi};
const PAGE: u64 = 0x4000;
const ADDRESS: u64 = 0x000003ffffffc000;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    ShaderStore,
    CommandFetch,
}
#[derive(Clone, Copy)]
struct Armed {
    owner: (u64, u32),
    address: u64,
    kind: Kind,
}
pub(super) struct Record {
    pub(super) kind: Kind,
    pub(super) owner: (u64, u32),
    pub(super) address: u64,
    pub(super) physical: u64,
    pub(super) ordinal: u32,
    pub(super) restored: bool,
    pub(super) quiescent: bool,
    pub(super) failed: bool,
    render_pte: u64,
    compute_pte: Option<u64>,
    root: u64,
    slot: u16,
    _bo: ARef<Object>,
    // Raw owned caller data is retained instead of relying on a Python-only
    // hashlib digest. It never reads a firmware binary or a code capture.
    pub(super) backing: KVVec<u8>,
}
pub(super) struct State {
    pending: Option<Armed>,
    active: Option<usize>,
    used: bool,
    records: KVec<Record>,
}
impl State {
    pub(super) fn new() -> Self {
        Self {
            pending: None,
            active: None,
            used: false,
            records: KVec::new(),
        }
    }
    pub(super) fn fail(&mut self) {
        if let Some(index) = self.active.take() {
            self.records[index].failed = true;
        }
    }
}
impl Session {
    pub(super) fn arm_owned_render_fault(
        &mut self,
        owner: (u64, u32),
        address: u64,
        kind: Kind,
    ) -> Result {
        if self.faults.pending.is_some() || self.faults.active.is_some() {
            return Err(EBUSY);
        }
        if self.phase == Phase::Failed || address & (PAGE - 1) != 0 {
            return Err(EINVAL);
        }
        let client = &self.render.as_ref().ok_or(EINVAL)?.client;
        if client.owner != owner
            || !client.bindings.iter().any(|&(base, size, _, _)| {
                address >= base
                    && address
                        .checked_add(PAGE)
                        .is_some_and(|end| end <= base + size)
            })
        {
            return Err(EINVAL);
        }
        self.faults.pending = Some(Armed {
            owner,
            address,
            kind,
        });
        Ok(())
    }
    pub(super) fn inject_owned_render_fault(&mut self, parameters: &Parameters) -> Result {
        let work = self.render.as_ref().ok_or(EINVAL)?;
        let kind = *crate::module_parameters::owned_render_fault_kind.value();
        if !self.faults.used
            && kind != 0
            && work.ordinal == *crate::module_parameters::owned_render_fault_ordinal.value()
        {
            let kind = match kind {
                1 => Kind::ShaderStore,
                2 => Kind::CommandFetch,
                _ => return Err(EINVAL),
            };
            self.arm_owned_render_fault(
                work.client.owner,
                *crate::module_parameters::owned_render_fault_address.value(),
                kind,
            )?;
            self.faults.used = true;
        }
        let Some(armed) = self.faults.pending.take() else {
            return Ok(());
        };
        let work = self.render.as_ref().ok_or(EINVAL)?;
        let dva = if armed.address < 0x1000000000 {
            armed.address + 0x1000000000
        } else {
            armed.address
        };
        if armed.owner != work.client.owner {
            return Err(EINVAL);
        }
        match armed.kind {
            Kind::ShaderStore => {
                if !parameters.fragment_attachments[..parameters.fragment_attachment_count]
                    .iter()
                    .any(|&[base, size]| {
                        armed.address >= base
                            && armed
                                .address
                                .checked_add(PAGE)
                                .is_some_and(|end| end <= base + size)
                    })
                {
                    return Err(EINVAL);
                }
            }
            Kind::CommandFetch => {
                if dva != parameters.encoder & !(PAGE - 1) {
                    return Err(EINVAL);
                }
            }
        }
        let index = work
            .client
            .bindings
            .iter()
            .position(|&(base, size, _, _)| {
                armed.address >= base && armed.address + PAGE <= base + size
            })
            .ok_or(EINVAL)?;
        let (base, _, offset, flags) = work.client.bindings[index];
        let offset = offset
            + if flags & uapi::drm_asahi_bind_flags_DRM_ASAHI_BIND_SINGLE_PAGE != 0 {
                0
            } else {
                armed.address - base
            };
        let bo = work.client.buffers[index].clone();
        let render_pte = work.client.root.pte(dva)?;
        if render_pte & 3 != 3 {
            return Err(EIO);
        }
        let physical = render_pte & ADDRESS;
        let mut cursor = 0;
        let mut expected = None;
        for entry in bo.sg_table()?.iter() {
            let length = entry.dma_len() as u64;
            if cursor <= offset && offset + PAGE <= cursor + length {
                expected = Some(entry.dma_address() + offset - cursor);
                break;
            }
            cursor += length;
        }
        if expected != Some(physical) {
            return Err(EIO);
        }
        let compute_pte = if self.native.is_some() {
            let pte = self.compute.as_ref().ok_or(EINVAL)?.client.root.pte(dva)?;
            if pte & ADDRESS != physical || pte & 3 != 3 {
                return Err(EIO);
            }
            Some(pte)
        } else {
            None
        };
        let map = bo.vmap::<u8>()?;
        if map.is_iomem() {
            return Err(EINVAL);
        }
        let mut backing = KVVec::with_capacity(PAGE as usize, GFP_KERNEL)?;
        for index in offset as usize..(offset + PAGE) as usize {
            let pointer = map.ptr_from_index(index)?;
            // SAFETY: Source binding/SG bounds retain this complete caller page.
            backing.push(unsafe { pointer.read_volatile() }, GFP_KERNEL)?;
        }
        drop(map);
        let slot = work.layout.context as u16;
        self.faults.records.reserve(1, GFP_KERNEL)?;
        let record = self.faults.records.len();
        self.faults.records.push(
            Record {
                kind: armed.kind,
                owner: armed.owner,
                address: dva,
                physical,
                ordinal: work.ordinal,
                restored: false,
                quiescent: false,
                failed: false,
                render_pte,
                compute_pte,
                root: work.client.root.root(),
                slot,
                _bo: bo,
                backing,
            },
            GFP_KERNEL,
        )?;
        self.faults.active = Some(record);
        // Retain the complete record before breaking either owned alias.
        self.render
            .as_mut()
            .ok_or(EINVAL)?
            .client
            .root
            .rebind(&[(dva, render_pte, 0)], &[slot])?;
        if let Some(pte) = compute_pte {
            self.compute
                .as_mut()
                .ok_or(EINVAL)?
                .client
                .root
                .rebind(&[(dva, pte, 0)], &[1, 2, 3])?;
        }
        if self.render.as_ref().ok_or(EINVAL)?.client.root.pte(dva)? != 0
            || compute_pte.is_some()
                && self.compute.as_ref().ok_or(EINVAL)?.client.root.pte(dva)? != 0
        {
            return Err(EIO);
        }
        Ok(())
    }
    pub(super) fn completed_owned_render_fault(&mut self) {
        if let Some(index) = self.faults.active {
            self.faults.records[index].quiescent = true;
        }
    }
    pub(super) fn render_fault_evidence(&self) -> Option<&Record> {
        self.faults.records.last()
    }
    pub(super) fn restore_owned_render_fault(&mut self) -> Result {
        let Some(index) = self.faults.active else {
            return Ok(());
        };
        let record = &self.faults.records[index];
        if self.phase != Phase::Running
            || record.kind != Kind::ShaderStore
            || record.restored
            || record.failed
            || !record.quiescent
        {
            return Err(EIO);
        }
        let work = self.render.as_mut().ok_or(EINVAL)?;
        if work.client.owner != record.owner || work.client.root.root() != record.root {
            return Err(EIO);
        }
        work.client
            .root
            .rebind(&[(record.address, 0, record.render_pte)], &[record.slot])?;
        if let Some(pte) = record.compute_pte {
            self.compute
                .as_mut()
                .ok_or(EINVAL)?
                .client
                .root
                .rebind(&[(record.address, 0, pte)], &[1, 2, 3])?;
        }
        if work.client.root.pte(record.address)? != record.render_pte {
            return Err(EIO);
        }
        if let Some(pte) = record.compute_pte {
            if self
                .compute
                .as_ref()
                .ok_or(EINVAL)?
                .client
                .root
                .pte(record.address)?
                != pte
            {
                return Err(EIO);
            }
        }
        self.faults.records[index].restored = true;
        self.faults.active = None;
        Ok(())
    }
}
