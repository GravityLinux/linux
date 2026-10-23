// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Max construction using the common UAT walker and Memory ownership.
//! Establish the cold boot contract before starting RTKit. Its management
//! phase may precede graph population, as in the qualified source startup.
//! Retain the Vm in the session before population; on failure stop RTKit and
//! restore borrowed tables before Memory releases any owned backing.
//! Boot page-table DATA is preserved; firmware executable pages are not read.
//! Context/root publication belongs to startup, after this construction succeeds.

use super::{
    g17p_memory::Memory,
    g17p_vm::{BootTables, Vm},
    scheduled_image::ImageG16C,
    scheduled_initgraph as graph,
    scheduled_resources::ResourcesG16C,
};
use kernel::prelude::*;

const PAGE: usize = 0x4000;
const ADDRESS: u64 = 0x0000_ffff_ffff_c000;

// Only inherited page-table DATA is saved, never firmware executable pages.
// Cold construction may add links to owned tables. Restore the borrowed tree
// on failure before Memory is allowed to drop those newly allocated pages.
type SavedTables = BootTables;

// Only protocol page-table DATA in the declared boot reservations is read.
// Track every child table before the common walker may use an inherited path.
fn adopt(
    vm: &mut Vm,
    memory: &Memory,
    resources: &ResourcesG16C,
    table: u64,
    ancestors: &mut [u64; 3],
    level: usize,
    saved: &mut SavedTables,
) -> Result {
    if !resources.boot_table(table) || ancestors[..level].contains(&table) {
        return Err(EINVAL);
    }
    memory.invalidate(table, PAGE)?;
    if let Some((_, depth, _)) = saved.iter().find(|(address, _, _)| *address == table) {
        if *depth != level {
            return Err(EINVAL);
        }
    } else {
        let mut bytes = KVVec::new();
        bytes.extend_with(PAGE, 0, GFP_KERNEL)?;
        for i in 0..PAGE / 8 {
            bytes[i * 8..i * 8 + 8]
                .copy_from_slice(&memory.read64(table + i as u64 * 8)?.to_le_bytes());
        }
        saved.push((table, level, bytes), GFP_KERNEL)?;
    }
    vm.track_boot_table(table)?;
    ancestors[level] = table;
    if level < 2 {
        for index in 0..if level == 0 { 64 } else { 2048 } {
            let word = memory.read64(table + index * 8)?;
            if word == 0 {
                continue;
            }
            if word & 3 != 3 {
                return Err(EINVAL);
            }
            adopt(
                vm,
                memory,
                resources,
                word & ADDRESS,
                ancestors,
                level + 1,
                saved,
            )?;
        }
    }
    Ok(())
}

impl Vm {
    pub(crate) fn prepare_g16c(memory: &mut Memory, resources: &ResourcesG16C) -> Result<Self> {
        let mut low = [0; 2];
        let mut high = [0; 2];
        for roots in [&mut low, &mut high] {
            for root in roots {
                *root = memory.allocate(PAGE)?;
                memory.zero(*root, PAGE)?;
                memory.clean(*root, PAGE)?;
            }
        }
        let firmware = resources.regions[1].base;
        let mut vm = Self::g16c(firmware, low, high)?;
        let mut saved = SavedTables::new();
        adopt(
            &mut vm,
            memory,
            resources,
            firmware,
            &mut [0; 3],
            0,
            &mut saved,
        )?;
        vm.retain_boot_tables(saved);
        Ok(vm)
    }

    /// RTKit connects the reserved shared L2 during management startup. Walk
    /// the current table DATA before constructing our graph, keeping original
    /// snapshots for every table that existed before firmware started.
    pub(crate) fn refresh_g16c_boot_tables(
        &mut self, memory: &Memory, resources: &ResourcesG16C,
    ) -> Result {
        let mut saved = self.take_g16c_boot_tables()?;
        let result = adopt(self, memory, resources, self.firmware_root(),
            &mut [0; 3], 0, &mut saved);
        // Retain restoration metadata even if a new table is invalid.
        self.retain_boot_tables(saved);
        result
    }

    /// Keep this Vm and Memory in the session through any failure. Restoring
    /// the inherited tree is permitted only after firmware access has stopped.
    pub(crate) fn populate_g16c(&mut self, memory: &mut Memory, image: &ImageG16C) -> Result {
        self.begin_g16c_graph()?;
        for (index, object) in graph::OBJECT_LAYOUT.iter().enumerate() {
            let physical = memory.allocate(object.size)?;
            memory.write(physical, image.object(index)?)?;
            memory.clean(physical, object.size)?;
            self.span(
                memory,
                2,
                graph::BASE + object.offset as u64,
                physical,
                object.size,
                object.pte_flags,
            ).inspect_err(|error| {
                kernel::pr_err!("G16C: RAM object {} VA {:#x} PA {:#x} size {:#x} mapping failed: {:?}\n",
                    index, graph::BASE + object.offset as u64, physical, object.size, error);
            })?;
        }
        for mapping in image.mmio.mappings() {
            // Device PTEs only. MMIO is never CPU-mapped or cleared as RAM.
            self.span(
                memory,
                2,
                mapping.address,
                mapping.physical,
                mapping.size,
                mapping.pte_flags,
            ).inspect_err(|error| {
                kernel::pr_err!("G16C: MMIO VA {:#x} PA {:#x} size {:#x} mapping failed: {:?}\n",
                    mapping.address, mapping.physical, mapping.size, error);
            })?;
        }
        for alias in graph::ALIASES {
            for offset in (0..alias.size).step_by(PAGE) {
                let physical = self.physical(memory, 2, alias.high + offset as u64)?;
                self.span(
                    memory,
                    0,
                    alias.low + offset as u64,
                    physical,
                    PAGE,
                    alias.pte_flags,
                )?;
            }
        }
        self.flush_tables(memory)?;
        Ok(())
    }

    /// Convenience transaction for a stopped ASC, including host validation.
    /// Management-running startup uses prepare/populate with session ownership
    /// so that its error path can stop RTKit before restoring the boot tree.
    pub(crate) fn build_g16c(
        memory: &mut Memory,
        resources: &ResourcesG16C,
        image: &ImageG16C,
    ) -> Result<Self> {
        let mut vm = Self::prepare_g16c(memory, resources)?;
        if let Err(error) = vm.populate_g16c(memory, image) {
            vm.restore_boot_tables(memory)
                .expect("validated cold boot table restore");
            return Err(error);
        }
        Ok(vm)
    }
}
