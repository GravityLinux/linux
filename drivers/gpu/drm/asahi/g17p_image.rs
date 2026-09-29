// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Owned, unpublished firmware objects for the synchronous G17P port.
//!
//! These buffers are staging storage, not DMA allocations. A later mapping
//! stage must supply the prescribed GPU addresses and physical topology before
//! either firmware instance can see them.

use super::{g17p_abi, g17p_initgraph as graph, g17p_platform};
use kernel::{device, prelude::*};

struct Buffers([KVVec<u8>; graph::OBJECTS]);

impl graph::Storage for Buffers {
    fn object(&mut self, index: usize) -> core::result::Result<&mut [u8], g17p_abi::InvalidSize> {
        self.0
            .get_mut(index)
            .map(|bytes| bytes.as_mut_slice())
            .ok_or(g17p_abi::InvalidSize)
    }
}

pub(crate) struct Image {
    buffers: Buffers,
    pub(crate) graph: graph::Graph,
}

impl Image {
    pub(crate) fn new(dev: &device::Device, platform: &g17p_platform::Platform) -> Result<Self> {
        let performance = platform.firmware_performance().inspect_err(|_| {
            dev_err!(
                dev,
                "G17P: live performance tables differ from the qualified Python profile\n"
            );
        })?;
        let base = platform.private_vm[0]
            .checked_add(platform.private_vm[1])
            .ok_or(EINVAL)?;
        let mut buffers = Buffers(core::array::from_fn(|_| KVVec::new()));
        for (bytes, object) in buffers.0.iter_mut().zip(graph::OBJECT_LAYOUT.iter()) {
            bytes.extend_with(object.size, 0, GFP_KERNEL)?;
        }
        let graph = graph::build(&mut buffers, base, &performance).map_err(|_| EINVAL)?;
        let bytes: usize = buffers.0.iter().map(|bytes| bytes.len()).sum();
        dev_info!(dev,
            "G17P: built {} unpublished initdata objects ({} bytes), roots {:#x}/{:#x}, {} channels per instance\n",
            graph::OBJECTS, bytes, graph.roots()[0], graph.roots()[1], g17p_abi::CHANNELS);
        Ok(Self { buffers, graph })
    }

    pub(crate) fn object(&self, index: usize) -> Result<&[u8]> {
        self.buffers
            .0
            .get(index)
            .map(|v| v.as_slice())
            .ok_or(EINVAL)
    }
}
