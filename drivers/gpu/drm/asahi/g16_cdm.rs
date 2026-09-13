// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! Bounded Apple9 CDM walker and resource-table selection.
//! Reads happen after scheduler dependencies complete, because earlier GPU
//! commands can produce a later command stream or its resource table.

use kernel::prelude::*;

pub(crate) trait Memory {
    fn read(&self, address: u64, out: &mut [u8]) -> Result;
    fn cover(&self, address: u64, size: u64, permissions: u32) -> Result;
}

pub(crate) fn resource(memory: &impl Memory, base: u64, end: u64) -> Result<u64> {
    if base >= end || (base | end) & 3 != 0 {
        return Err(EINVAL);
    }
    memory.cover(base, end - base, 2)?;
    let mut cursor = base;
    let mut stack = [0u64; 32];
    let mut depth = 0;
    let mut selected = None;
    // A fixed block budget also bounds cycles. No recursion or allocations
    // proportional to the user-supplied virtual span are needed.
    for _ in 0..4096 {
        if cursor < base || cursor.checked_add(4).ok_or(EINVAL)? > end || cursor & 3 != 0 {
            return Err(EINVAL);
        }
        let mut body = [0u8; 0x28];
        memory.read(cursor, &mut body[..4])?;
        let header = u32::from_le_bytes(body[..4].try_into().map_err(|_| EINVAL)?);
        let kind = header >> 29;
        let mode = (header >> 27) & 3;
        let size = match kind {
            0 => match mode {
                0 => 0x28,
                1 => 0x24,
                2 => 0x18,
                _ => return Err(EINVAL),
            },
            1 => 8,
            2..=4 => 4,
            _ => return Err(EINVAL),
        };
        let next = cursor.checked_add(size).ok_or(EINVAL)?;
        if next > end {
            return Err(EINVAL);
        }
        // Keep the header already decoded, even if userspace concurrently
        // edits its BO. Such edits cannot expand our validated bounds.
        memory.read(cursor + 4, &mut body[4..size as usize])?;
        let word = |off| u32::from_le_bytes(body[off..off + 4].try_into().unwrap());
        match kind {
            0 => {
                if word(12) & 0x4000_0000 == 0 {
                    return Err(EINVAL);
                }
                let pipeline = (u64::from(word(12) & 255) << 40) | (u64::from(word(8)) << 6);
                let resource = (pipeline & !0x7fff).checked_add(0x50000).ok_or(EINVAL)?;
                let aperture = 0x100_0000_0000..0x101_0000_0000;
                if !aperture.contains(&pipeline) || !aperture.contains(&resource) {
                    return Err(EINVAL);
                }
                if selected.is_some_and(|old| old != resource) {
                    return Err(ENOTSUPP);
                }
                selected = Some(resource);
                if mode != 0 {
                    let indirect = (u64::from(word(0x10)) << 32) | u64::from(word(0x14));
                    // Local-indirect adds three local dimensions to the three
                    // global thread dimensions. Validate the complete readable
                    // object, but leave GPU-produced values to the hardware.
                    let geometry_size = if mode == 2 { 24 } else { 12 };
                    memory.cover(indirect, geometry_size, 2)?;
                }
                cursor = next;
            }
            1 => {
                if header & (1 << 28) != 0 {
                    if depth == stack.len() {
                        return Err(EINVAL);
                    }
                    stack[depth] = next;
                    depth += 1;
                }
                cursor = (u64::from(header & 255) << 32) | u64::from(word(4));
            }
            2 => {
                if depth != 0 || next != end {
                    return Err(EINVAL);
                }
                let resource = selected.ok_or(ENOTSUPP)?;
                memory.cover(resource, 0x4000, 2)?;
                return Ok(resource);
            }
            3 => cursor = next,
            4 => {
                if depth == 0 {
                    return Err(EINVAL);
                }
                depth -= 1;
                cursor = stack[depth];
            }
            _ => return Err(EINVAL),
        }
    }
    Err(EINVAL)
}
