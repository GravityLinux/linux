// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Synchronous Asahi submission dependencies and transactional output syncs.

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

impl Plan {
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
        let unique = dma_fence::FenceContexts::new(1, c_str!("asahi_neo"), FENCE_KEY)?
            .new_fence(0, Completion)?;
        Ok(Self {
            dependencies,
            outputs,
            completion: dma_fence::Fence::from_fence(&unique),
        })
    }

    /// Caller must hold neither file state nor the GPU runtime lock: the
    /// producer of an imported dependency can need either to make progress.
    pub(crate) fn wait_inputs(&self) -> Result {
        for fence in &self.dependencies {
            // SAFETY: The plan owns this reference throughout the wait. Match
            // the Python synchronous adapter's bounded two-second wait.
            let waited = unsafe {
                bindings::dma_fence_wait_timeout(
                    fence.raw(),
                    true,
                    kernel::time::msecs_to_jiffies(2000) as _,
                )
            };
            if waited < 0 {
                return Err(Error::from_errno(waited as i32));
            }
            if waited == 0 {
                return Err(ETIMEDOUT);
            }
            // Chain wrappers do not propagate the contained point's error.
            // Wait the whole chain, then inspect this point's actual fence.
            // SAFETY: The owned chain retains its immutable contained fence
            // (or returns itself), and get_status locks the live object.
            let status = unsafe {
                bindings::dma_fence_get_status(bindings::dma_fence_chain_contained(fence.raw()))
            };
            if status < 0 {
                return Err(Error::from_errno(status));
            }
        }
        Ok(())
    }

    /// Called only after the synchronous backend verified completion and
    /// made caller writes CPU-visible. Failed admission leaves outputs alone.
    pub(crate) fn complete(self) {
        self.completion.signal();
        for output in self.outputs {
            if let Some(chain) = output.chain {
                output
                    .object
                    .add_point(chain, &self.completion, output.point);
            } else {
                output.object.replace_fence(Some(&self.completion));
            }
        }
    }
}
