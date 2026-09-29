// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! CPU access to the validated, permanently reserved GPU boot RAM.
//! Does not treat firmware physical addresses as kernel direct-map pointers.

use super::{g17p_platform::Platform, g17p_topology};
use kernel::{c_str, device, io::mem, io::resource::Resource, prelude::*};

struct Mapping {
    base: u64,
    memory: mem::Mem,
}

pub(crate) struct Memory {
    mappings: KVec<Mapping>,
}

// SAFETY: These mappings are not thread-local. Only this owner accesses them;
// writes require exclusive access. No reference into shared firmware RAM escapes.
unsafe impl Send for Memory {}

impl Memory {
    pub(crate) fn new(dev: &device::Device, platform: &Platform) -> Result<Self> {
        let node = dev.of_node().ok_or(ENODEV)?;
        let mut memory = Self {
            mappings: KVec::new(),
        };
        for (index, name) in [
            c_str!("ttbs"),
            c_str!("pagetables"),
            c_str!("l2"),
            c_str!("handoff"),
        ]
        .iter()
        .enumerate()
        {
            let res = node.reserved_mem_region_to_resource_byname(name)?;
            let expected = platform.regions[index];
            memory.map(res, expected.base, expected.size)?;
        }
        let source = node
            .parse_phandle(c_str!("memory-region"), 6)
            .ok_or(ENODEV)?;
        for (index, &(base, size)) in g17p_topology::RESERVATIONS.iter().enumerate() {
            memory.map(source.address_to_resource(index)?, base, size)?;
        }
        dev_info!(
            dev,
            "G17P: mapped {} reserved RAM ranges for bounded CPU access\n",
            memory.mappings.len()
        );
        Ok(memory)
    }

    fn map(&mut self, resource: Resource, base: u64, size: u64) -> Result {
        if resource.start() as u64 != base || resource.size() as u64 != size {
            return Err(EINVAL);
        }
        // SAFETY: Platform validated this no-map RAM reservation. It is not
        // MMIO or Linux allocator memory. Mapping alone initiates no DMA, and
        // it creates no CPU alias with different cache attributes.
        let memory = unsafe { mem::Mem::try_new(resource, mem::MemFlag::WB.into())? };
        self.mappings.push(Mapping { base, memory }, GFP_KERNEL)?;
        Ok(())
    }

    fn pointer(&self, address: u64, size: usize) -> Result<*mut u8> {
        let end = address.checked_add(size as u64).ok_or(EINVAL)?;
        let mapping = self
            .mappings
            .iter()
            .find(|mapping| {
                address >= mapping.base && end <= mapping.base + mapping.memory.size() as u64
            })
            .ok_or(EINVAL)?;
        // SAFETY: The range above is entirely within this live mapping.
        Ok(unsafe { mapping.memory.ptr().add((address - mapping.base) as usize) })
    }

    pub(crate) fn read64(&self, address: u64) -> Result<u64> {
        if address & 7 != 0 {
            return Err(EINVAL);
        }
        // SAFETY: The checked address names aligned ordinary RAM. Volatile
        // access avoids assuming the firmware leaves the value unchanged.
        Ok(u64::from_le(unsafe {
            self.pointer(address, 8)?.cast::<u64>().read_volatile()
        }))
    }

    pub(crate) fn write64(&mut self, address: u64, value: u64) -> Result {
        if address & 7 != 0 {
            return Err(EINVAL);
        }
        // SAFETY: Bounds/alignment checked. Only host-owned protocol fields
        // and unpublished tables are written through this interface.
        unsafe {
            self.pointer(address, 8)?
                .cast::<u64>()
                .write_volatile(value.to_le())
        };
        Ok(())
    }

    pub(crate) fn write32(&mut self, address: u64, value: u32) -> Result {
        if address & 3 != 0 {
            return Err(EINVAL);
        }
        // SAFETY: Same ownership and range checks as write64.
        unsafe {
            self.pointer(address, 4)?
                .cast::<u32>()
                .write_volatile(value.to_le())
        };
        Ok(())
    }

    pub(crate) fn write8(&mut self, address: u64, value: u8) -> Result {
        // SAFETY: Checked one-byte host-owned field in ordinary RAM.
        unsafe { self.pointer(address, 1)?.write_volatile(value) };
        Ok(())
    }

    pub(crate) fn zero(&mut self, address: u64, size: usize) -> Result {
        // SAFETY: The caller exclusively owns the unpublished range. Bounds
        // are checked against the reservation before any writes take place.
        unsafe { self.pointer(address, size)?.write_bytes(0, size) };
        Ok(())
    }

    pub(crate) fn clean(&self, address: u64, size: usize) -> Result {
        let pointer = self.pointer(address, size)?;
        let start = (pointer as usize) & !63;
        let end = (pointer as usize).checked_add(size).ok_or(EINVAL)?;
        for address in (start..end).step_by(64) {
            // SAFETY: The live RAM mappings are page aligned, so rounding
            // down to a cache line remains within the same mapped page.
            unsafe {
                core::arch::asm!("dc cvac, {address}", address = in(reg) address, options(nostack, preserves_flags))
            };
        }
        sync();
        Ok(())
    }

    /// Cold partial-world preparation, matching the Python path's ordering.
    /// Both ASC CPUs must still be stopped and no host tables published.
    pub(crate) fn prepare_cold(&mut self, platform: &Platform) -> Result {
        let low_root = g17p_topology::TABLE_TARGETS
            .iter()
            .find(|(group, path, _)| *group == 1 && path.is_empty())
            .map(|(_, _, address)| *address)
            .ok_or(EINVAL)?;
        self.zero(low_root, 0x4000)?;
        self.clean(low_root, 0x4000)?;
        let contexts = platform.regions[0].base;
        // The render-capable Python entrypoint preserves slots 0/1/2 during
        // the first management boot. Other stale table pointers must go.
        for slot in 3..64 {
            self.write64(contexts + slot * 16, 0)?;
            self.write64(contexts + slot * 16 + 8, 0)?;
        }
        self.clean(contexts, 64 * 16)
    }

    pub(crate) fn write_handoff(&mut self, platform: &Platform) -> Result {
        let handoff = platform.regions[3].base;
        // Use the existing gravity-m4 host handoff initialization.
        self.write64(handoff, 0x4b1d000000000002)?;
        self.write32(handoff + 0x18, u32::MAX)?;
        self.write64(handoff + 0x640, 0)?;
        self.clean(handoff, 0x4000)
    }
}

pub(crate) fn sync() {
    // SAFETY: Complete CPU stores/cache maintenance before mailbox publication.
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}
