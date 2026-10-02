// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Immutable caller-only admission trees. They never become executable roots:
//! the backend receives an owned clone only when retained mapping identity
//! changes. Every accepted job retains its exact ordered snapshot and GEMs.

use super::super::{g17p_compute_runtime::Client, g17p_user_vm::UserVm};
use super::{Binding, Object};
use kernel::{
    prelude::*,
    sync::{aref::ARef, Arc},
};

pub(super) struct Snapshot {
    generation: u64,
    render: bool,
    client: Client,
}

impl Snapshot {
    pub(super) fn new(generation: u64, render: bool, client: Client) -> Result<Arc<Self>> {
        Ok(Arc::new(
            Self {
                generation,
                render,
                client,
            },
            GFP_KERNEL,
        )?)
    }

    pub(super) fn client(&self) -> &Client { &self.client }

    pub(super) fn same_client(&self, old: Option<&Client>) -> bool {
        old.is_some_and(|old| old.owner == self.client.owner
            && old.bindings == self.client.bindings
            && old.buffers.len() == self.client.buffers.len()
            && old.buffers.iter().zip(&self.client.buffers)
                .all(|(a,b)| core::ptr::eq(&**a,&**b)))
    }

    pub(super) fn matches(
        &self,
        owner: (u64, u32),
        generation: u64,
        render: bool,
        bindings: &[Binding],
    ) -> bool {
        self.generation == generation
            && self.render == render
            && self.client.owner == owner
            && self.client.bindings.len() == bindings.len()
            && self.client.buffers.len() == bindings.len()
            && bindings.iter().enumerate().all(|(index, binding)| {
                self.client.bindings[index]
                    == (binding.start, binding.size, binding.offset, binding.flags)
                    && core::ptr::eq(&*self.client.buffers[index], &*binding.bo)
            })
    }

    /// Metadata is sufficient for frontend range validation and backend
    /// retained-owner comparison. The empty root must be materialized before
    /// this Client reaches any root/private mapping or publication operation.
    pub(super) fn deferred_client(&self) -> Result<Client> {
        let mut buffers: KVec<ARef<Object>> = KVec::new();
        buffers.reserve(self.client.buffers.len(), GFP_KERNEL)?;
        for buffer in &self.client.buffers {
            buffers.push(buffer.clone(), GFP_KERNEL)?;
        }
        let mut bindings = KVec::new();
        bindings.extend_from_slice(&self.client.bindings, GFP_KERNEL)?;
        Ok(Client {
            root: UserVm::new()?,
            buffers,
            bindings,
            owner: self.client.owner,
            cpu_maps: self.client.cpu_maps.clone(),
            primer_aliases: None,
        })
    }

    pub(super) fn materialize(&self, client: &mut Client, render: bool) -> Result {
        // Generation was checked at admission and travels in the accepted
        // job's immutable Arc; Client itself does not carry a generation.
        // Check the paired metadata and engine view before copying any root.
        if self.render != render
            || client.owner != self.client.owner
            || client.bindings != self.client.bindings
            || client.buffers.len() != self.client.buffers.len()
            || client.primer_aliases.is_some()
            || !client
                .buffers
                .iter()
                .zip(&self.client.buffers)
                .all(|(left, right)| core::ptr::eq(&**left, &**right))
        {
            return Err(EIO);
        }
        // Only the privately owned copy can acquire backend/private/growth
        // mappings. Cached tables and snapshots held by other jobs stay frozen.
        client.root = self.client.root.clone_admission_tree()?;
        Ok(())
    }
}
