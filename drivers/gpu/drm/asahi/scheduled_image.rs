// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Own the G16C graph until its RAM mappings can be installed and published.
//! Allocation failure drops only unpublished CPU buffers, without starting ASC.

use super::{
    scheduled_board::BoardG16C, scheduled_initgraph as graph, scheduled_mmio::MmioG16C,
    scheduled_power::SettingsG16C,
};
use kernel::{of, prelude::*};

struct Buffers([KVVec<u8>; graph::OBJECTS]);

impl graph::Storage for Buffers {
    fn object(&mut self, index: usize) -> core::result::Result<&mut [u8], graph::InvalidGraph> {
        self.0
            .get_mut(index)
            .map(|b| b.as_mut_slice())
            .ok_or(graph::InvalidGraph)
    }
}

pub(crate) struct ImageG16C {
    buffers: Buffers,
    pub(crate) graph: graph::Graph,
    pub(crate) mmio: KBox<MmioG16C>,
}

impl ImageG16C {
    pub(crate) fn new(node: &of::Node, entropy: u32) -> Result<Self> {
        let board = BoardG16C::from_fdt(node)?;
        let settings = SettingsG16C::from_fdt(node)?;
        let mmio = MmioG16C::from_fdt_boxed(node)?;
        let mut buffers = Buffers(core::array::from_fn(|_| KVVec::new()));
        for (buffer, object) in buffers.0.iter_mut().zip(graph::OBJECT_LAYOUT.iter()) {
            buffer.extend_with(object.size, 0, GFP_KERNEL)?;
        }
        let graph = graph::build(&mut buffers, &board, mmio.registers(), &settings, entropy)
            .map_err(|_| EINVAL)?;
        Ok(Self {
            buffers,
            graph,
            mmio,
        })
    }

    pub(crate) fn object(&self, index: usize) -> Result<&[u8]> {
        self.buffers
            .0
            .get(index)
            .map(|b| b.as_slice())
            .ok_or(EINVAL)
    }
}
