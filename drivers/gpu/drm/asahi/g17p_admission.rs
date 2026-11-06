// SPDX-License-Identifier: GPL-2.0-only

//! Immutable caller-only admission trees. They never become executable roots:
//! the backend receives an owned clone only when retained mapping identity
//! changes. Every accepted job retains its exact ordered snapshot and GEMs.

use super::super::{g17p_compute_runtime::Client, g17p_user_vm::UserVm};
use super::Object;
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
    pub(super) fn new(generation: u64, render: bool, mut client: Client) -> Result<Arc<Self>> {
        client.identity = Some(super::super::g17p_compute_runtime::MappingIdentity::new()?);
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
        old.is_some_and(|old| self.client.same_mapping(old))
    }

    pub(super) fn matches(
        &self,
        owner: (u64, u32),
        generation: u64,
        render: bool,
    ) -> bool {
        self.generation == generation
            && self.render == render
            && self.client.owner == owner
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
        Ok(Client { identity: self.client.identity.clone(),
            root: UserVm::new()?,
            buffers,
            bindings,
            owner: self.client.owner,
            cpu_maps: self.client.cpu_maps.clone(),
            primer_aliases: None,
        })
    }

    pub(super) fn materialize(&self, client: &mut Client, render: bool) -> Result {
        self.materialize_view(client,render,false)
    }
    pub(super) fn materialize_view(&self, client: &mut Client, render: bool, readonly: bool) -> Result {
        // The token certifies owner, engine view and immutable binding/BO
        // metadata. It is invalidated by every destructive mapping mutation.
        if self.render != render || !self.client.same_mapping(client)
            || client.primer_aliases.is_some()
        {
            return Err(EIO);
        }
        // Only the privately owned copy can acquire backend/private/growth
        // mappings. Cached tables and snapshots held by other jobs stay frozen.
        client.root = if readonly {self.client.root.clone_admission_view()?}
            else {self.client.root.clone_admission_tree()?};
        Ok(())
    }
}
