// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Asahi submission dependencies and transactional output sync publication.

use kernel::{
    bindings, c_str,
    dma_fence::{self, RawDmaFence},
    drm,
    prelude::*,
    uaccess::{UserPtr, UserSlice},
    uapi,
};

static FENCE_KEY: Pin<&kernel::sync::LockClassKey> = kernel::static_lock_class!();

struct Completion;
#[vtable]
impl dma_fence::FenceOps for Completion {
    fn get_driver_name<'a>(self: &'a dma_fence::FenceObject<Self>) -> &'a kernel::str::CStr {
        c_str!("asahi")
    }
    fn get_timeline_name<'a>(self: &'a dma_fence::FenceObject<Self>) -> &'a kernel::str::CStr {
        c_str!("neo-execution")
    }
}

/// Backend-owned per-command gate used by the native context lifetime maps.
/// Allocate before publication, just as the Python shim creates its work fence.
pub(crate) fn work_fence() -> Result<dma_fence::Fence> {
    let unique = dma_fence::FenceContexts::new(1, c_str!("asahi_neo"), FENCE_KEY)?
        .new_fence(0, Completion)?;
    Ok(dma_fence::Fence::from_fence(&unique))
}

struct Output {
    object: drm::syncobj::SyncObj,
    chain: Option<dma_fence::FenceChain>,
    point: u64,
}

pub(crate) struct Plan {
    dependencies: KVec<dma_fence::Fence>,
    outputs: KVec<Output>,
    completion: dma_fence::Fence,
}

// SAFETY: A plan uniquely owns its unpublished FenceChains. They are only
// consumed during publish_outputs on the submitting thread, before the plan
// moves to a worker. Fence and SyncObj references are independently refcounted.
// No shared access or concurrent mutation of the raw chain pointer is exposed.
unsafe impl Send for Plan {}

impl Plan {
    pub(crate) fn input_error(&self) -> Result {
        for fence in &self.dependencies {
            // SAFETY: The plan owns the chain and contained fence references.
            let status = unsafe {
                bindings::dma_fence_get_status(bindings::dma_fence_chain_contained(fence.raw()))
            };
            if status < 0 {
                return Err(Error::from_errno(status));
            }
        }
        Ok(())
    }
    pub(crate) fn fence(&self) -> dma_fence::Fence {
        self.completion.clone()
    }
    /// Admission failed before publication. Release only the internal lease;
    /// leave userspace's output sync objects untouched.
    pub(crate) fn cancel(self) {
        self.completion.signal();
    }
    pub(crate) fn read<T: drm::file::DriverFile>(
        file: &drm::File<T>,
        data: &uapi::drm_asahi_submit,
    ) -> Result<Self> {
        if data.in_sync_count > 4096 || data.out_sync_count > 4096 {
            return Err(EINVAL);
        }
        let count = data.in_sync_count as usize + data.out_sync_count as usize;
        let mut bytes = KVec::new();
        UserSlice::new(UserPtr::from_addr(data.syncs.try_into()?), count * 16)
            .reader()
            .read_all(&mut bytes, GFP_KERNEL)?;
        let mut dependencies = KVec::new();
        let mut outputs = KVec::new();
        for (index, entry) in bytes.chunks_exact(16).enumerate() {
            let kind = u32::from_le_bytes(entry[..4].try_into().unwrap());
            let handle = u32::from_le_bytes(entry[4..8].try_into().unwrap());
            let point = u64::from_le_bytes(entry[8..16].try_into().unwrap());
            if kind > 1 || (kind == 0 && point != 0) {
                return Err(EINVAL);
            }
            let object = drm::syncobj::SyncObj::lookup_handle(file, handle)?;
            if index < data.in_sync_count as usize {
                let fence = object.fence_get().ok_or(EINVAL)?;
                let fence = if kind == 1 {
                    fence.chain_find_seqno(point)?
                } else {
                    Some(fence)
                };
                if let Some(fence) = fence {
                    dependencies.push(fence, GFP_KERNEL)?;
                }
            } else {
                outputs.push(
                    Output {
                        object,
                        point,
                        chain: if kind == 1 {
                            Some(dma_fence::FenceChain::new()?)
                        } else {
                            None
                        },
                    },
                    GFP_KERNEL,
                )?;
            }
        }
        // Allocate before any GPU publication. A separate context prevents
        // unrelated submissions from implicitly satisfying each other's wait.
        Ok(Self {
            dependencies,
            outputs,
            completion: work_fence()?,
        })
    }

    pub(crate) fn inputs(&self) -> &[dma_fence::Fence] {
        &self.dependencies
    }

    pub(crate) fn inputs_ready(&self) -> Result<bool> {
        for fence in &self.dependencies {
            // SAFETY: Owned immutable chain and contained fence references.
            let chain = unsafe { bindings::dma_fence_get_status(fence.raw()) };
            let status = unsafe {
                bindings::dma_fence_get_status(bindings::dma_fence_chain_contained(fence.raw()))
            };
            if status < 0 {
                return Err(Error::from_errno(status));
            }
            if chain == 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Publish the accepted job's unsignaled fence before returning from
    /// submit. No fallible allocations occur after this transaction commits.
    pub(crate) fn publish_outputs(&mut self) {
        for output in self.outputs.drain(..) {
            if let Some(chain) = output.chain {
                output
                    .object
                    .add_point(chain, &self.completion, output.point);
            } else {
                output.object.replace_fence(Some(&self.completion));
            }
        }
    }

    /// Called only after the synchronous backend verified completion and
    /// made caller writes CPU-visible. Failed admission leaves outputs alone.
    pub(crate) fn complete(self, error: Option<Error>) -> dma_fence::Fence {
        // A fatal report may already have failed this retained aggregate via
        // its timestamp aliases. Complete it once; set_error after signaling
        // violates the DMA-fence contract and emits a kernel warning.
        // SAFETY: This plan owns the reference and Runtime serializes failure.
        if unsafe { bindings::dma_fence_get_status(self.completion.raw()) } == 0 {
            if let Some(error) = error {
                self.completion.set_error(error);
            }
            self.completion.signal();
        }
        for output in self.outputs {
            if let Some(chain) = output.chain {
                output
                    .object
                    .add_point(chain, &self.completion, output.point);
            } else {
                output.object.replace_fence(Some(&self.completion));
            }
        }
        self.completion
    }
}
