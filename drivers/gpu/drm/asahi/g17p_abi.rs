// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Byte serializers from m1n1's g17p_initdata.py.
//!
//! All addresses are supplied by the object owner. These functions only fill
//! unpublished storage; they never update firmware-owned state or publish a
//! producer. In particular, unaligned qwords are serialized as bytes.

pub(crate) const ROOT_SIZE: usize = 0xb8;
pub(crate) const ROOT_SECONDARY_SIZE: usize = 0xc8;
pub(crate) const MAIN_SIZE: usize = 0x600;
pub(crate) const CHANNELS: usize = 17;
pub(crate) const HWDATA_SIZE: usize = 0x3db4;
pub(crate) const REGION_C_SIZE: usize = 0x1000;

#[path = "g17p_constants.rs"]
mod constants;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InvalidSize;

fn clear(out: &mut [u8], size: usize) -> Result<(), InvalidSize> {
    if out.len() != size {
        return Err(InvalidSize);
    }
    out.fill(0);
    Ok(())
}

fn put16(out: &mut [u8], offset: usize, value: u16) {
    out[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(out: &mut [u8], offset: usize, value: u64) {
    out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub(crate) struct Root {
    pub(crate) version: [u16; 4],
    pub(crate) region_a: u64,
    pub(crate) main: u64,
    pub(crate) region_c: u64,
    pub(crate) kind: u32,
    pub(crate) status: [u64; 2],
    pub(crate) secondary_extra: [u64; 2],
}

impl Root {
    pub(crate) fn size(&self) -> usize {
        if self.secondary_extra == [0; 2] {
            ROOT_SIZE
        } else {
            ROOT_SECONDARY_SIZE
        }
    }

    pub(crate) fn build(&self, out: &mut [u8]) -> Result<(), InvalidSize> {
        clear(out, self.size())?;
        for (index, value) in self.version.iter().enumerate() {
            put16(out, index * 2, *value);
        }
        put64(out, 0x08, self.region_a);
        put64(out, 0x18, self.main);
        put64(out, 0x20, self.region_c);
        put32(out, 0x28, self.kind);
        put32(out, 0x2c, 1);
        put16(out, 0x30, 0x4000);
        out[0x32] = 14;
        out[0x33] = 3;
        for (index, (shift, count)) in [(36, 64), (25, 2048), (14, 2048)].iter().enumerate() {
            let offset = 0x34 + index * 0x20;
            out[offset..offset + 4].copy_from_slice(&[8, 14, 14, *shift]);
            put16(out, offset + 4, *count);
            put16(out, offset + 6, 0x4000);
            put64(out, offset + 8, 1);
            put64(out, offset + 0x10, 0x0000_03ff_ffff_c000);
            put64(out, offset + 0x18, (u64::from(*count) - 1) << shift);
        }
        put64(out, 0xa8, self.status[0]);
        put64(out, 0xb0, self.status[1]);
        if self.size() == ROOT_SECONDARY_SIZE {
            put64(out, 0xb8, self.secondary_extra[0]);
            put64(out, 0xc0, self.secondary_extra[1]);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Channel {
    pub(crate) states: [u64; 3],
    pub(crate) ring: u64,
}

impl Channel {
    pub(crate) fn build(&self, out: &mut [u8]) -> Result<(), InvalidSize> {
        clear(out, 0x20)?;
        for (index, address) in self.states.iter().enumerate() {
            put64(out, index * 8, *address);
        }
        put64(out, 0x18, self.ring);
        Ok(())
    }
}

pub(crate) struct MainConfig {
    pub(crate) hardware: u64,
    pub(crate) repeated: u64,
    pub(crate) channels: [Channel; CHANNELS],
    pub(crate) addresses: [u64; 5],
    // Three pairs represent six qwords: high sentinel, two low/high aliases,
    // then a null terminator. None leaves the second qword zero.
    pub(crate) region_views: [(u64, Option<u32>); 3],
    pub(crate) secondary: bool,
    pub(crate) secondary_extra: u64,
}

impl MainConfig {
    pub(crate) fn build(&self, out: &mut [u8]) -> Result<(), InvalidSize> {
        clear(out, MAIN_SIZE)?;
        put64(out, 0, self.hardware);
        put64(out, 8, self.repeated);
        put64(out, 0x10, self.repeated);
        let first = if self.secondary { 12 } else { 0 };
        for (index, channel) in self.channels.iter().enumerate().skip(first) {
            let offset = 0x20 + index * 0x20;
            channel.build(&mut out[offset..offset + 0x20])?;
        }
        if !self.secondary {
            for (index, address) in self.addresses.iter().enumerate() {
                put64(out, 0x254 + index * 8, *address);
            }
        }
        for (index, (address, value)) in self.region_views.iter().enumerate() {
            let offset = 0x2d0 + index * 0x10;
            put64(out, offset, *address);
            if let Some(value) = value {
                put32(out, offset + 8, *value);
                put32(out, offset + 12, 0x70);
            }
        }
        if self.secondary {
            put32(out, 0x300, 4);
            put32(out, 0x4c0, 0x2a);
            put64(out, 0x471, self.secondary_extra);
        } else {
            put32(out, 0x3e0, 0xff);
            put32(out, 0x4c0, 0x16);
        }
        Ok(())
    }
}

pub(crate) struct Register {
    pub(crate) physical: u64,
    pub(crate) address: u64,
    pub(crate) size: u32,
    pub(crate) relative: u64,
    pub(crate) flags: u32,
}

impl Register {
    pub(crate) fn build(&self, out: &mut [u8]) -> Result<(), InvalidSize> {
        clear(out, 0x28)?;
        put64(out, 0, self.physical);
        put64(out, 8, self.address);
        put32(out, 0x10, self.size);
        put32(out, 0x14, self.size);
        put64(out, 0x18, self.relative);
        put32(out, 0x20, self.flags);
        Ok(())
    }
}

pub(crate) struct RegionRecord {
    pub(crate) lead: u32,
    pub(crate) value: u32,
    pub(crate) address: u64,
    pub(crate) size_a: u32,
    pub(crate) size_b: u32,
    pub(crate) trail: u32,
}

impl RegionRecord {
    pub(crate) fn build(&self, out: &mut [u8]) -> Result<(), InvalidSize> {
        clear(out, 0x40)?;
        put32(out, 0, self.lead);
        put32(out, 4, self.value);
        put32(out, 8, 0x70);
        put64(out, 0xc, self.address);
        put32(out, 0x14, self.size_a);
        put32(out, 0x18, self.size_b);
        put32(out, 0x1c, self.trail);
        Ok(())
    }
}

pub(crate) fn compute_dispatch(out: &mut [u8]) -> Result<(), InvalidSize> {
    clear(out, 0x20)?;
    for (index, value) in [0xe0000000, 0x08000000, 0, 0x2a00, 0x1500]
        .iter()
        .enumerate()
    {
        put32(out, index * 4, *value);
    }
    Ok(())
}

pub(crate) fn status(out: &mut [u8], acknowledged: bool) -> Result<(), InvalidSize> {
    clear(out, 0x80)?;
    put32(out, 4, 1);
    if acknowledged {
        put32(out, 0x10, 1);
        put32(out, 0x14, 1);
    }
    Ok(())
}

pub(crate) struct Performance {
    pub(crate) freq_a: [u32; 11],
    pub(crate) freq_b: [u32; 11],
    pub(crate) core_voltage: [u32; 11],
    pub(crate) memory_voltage: [u32; 11],
    pub(crate) scale_b: [u32; 11],
    pub(crate) relative_a: [u32; 11],
    pub(crate) relative_b: [u32; 11],
    pub(crate) index_a: [u32; 11],
    pub(crate) index_b: [u32; 11],
}

fn runs(out: &mut [u8], runs: &[(usize, &[u8])]) -> Result<(), InvalidSize> {
    for (offset, bytes) in runs {
        let end = offset.checked_add(bytes.len()).ok_or(InvalidSize)?;
        out.get_mut(*offset..end)
            .ok_or(InvalidSize)?
            .copy_from_slice(bytes);
    }
    Ok(())
}

pub(crate) struct HardwareData<'a> {
    pub(crate) registers: &'a [(usize, Register)],
    pub(crate) flags: &'a [(usize, u32)],
    pub(crate) performance: &'a Performance,
    pub(crate) chip: Option<u32>,
    pub(crate) regions: &'a [RegionRecord],
    pub(crate) opaque: Option<&'a [(usize, &'a [u8])]>,
}

impl HardwareData<'_> {
    pub(crate) fn build(&self, out: &mut [u8]) -> Result<(), InvalidSize> {
        if self.registers.iter().any(|(slot, _)| *slot >= 53)
            || self.flags.iter().any(|(slot, _)| *slot >= 53)
            || self.regions.len() > (HWDATA_SIZE - 0x2610) / 0x40
        {
            return Err(InvalidSize);
        }
        clear(out, HWDATA_SIZE)?;
        for (slot, register) in self.registers {
            let offset = 0x640 + slot * 0x28;
            register.build(&mut out[offset..offset + 0x28])?;
        }
        for (slot, flags) in self.flags {
            put32(out, 0x640 + slot * 0x28 + 0x20, *flags);
        }
        let perf = self.performance;
        for (base, repeat, frequency) in [(0xfc8, 16, &perf.freq_a), (0x1cdc, 1, &perf.freq_b)] {
            for state in 0..11 {
                put32(out, base + state * 4, frequency[state]);
                for word in 0..repeat {
                    put32(
                        out,
                        base + 0x40 + state * 0x40 + word * 4,
                        perf.core_voltage[state],
                    );
                    put32(
                        out,
                        base + 0x440 + state * 0x40 + word * 4,
                        perf.memory_voltage[state],
                    );
                }
            }
        }
        for (base, ladder) in [
            (0x1808, &perf.freq_b),
            (0x1848, &perf.scale_b),
            (0x18c8, &perf.relative_a),
            (0x1908, &perf.relative_b),
            (0x19c8, &perf.index_a),
            (0x1a08, &perf.index_b),
        ] {
            for (index, value) in ladder.iter().enumerate() {
                put32(out, base + index * 4, *value);
            }
        }
        if let Some(chip) = self.chip {
            put32(out, 0xe90, chip);
        }
        put16(out, 0x258e, 12);
        for group in 0..2 {
            for (index, value) in [0, 1, 1, 1].iter().enumerate() {
                put32(out, 0x2590 + group * 16 + index * 4, *value);
            }
        }
        for channel in 0..12 {
            put32(out, 0x25b4 + channel * 4, u32::MAX);
        }
        for offset in [0x25f4, 0x2600, 0x2608] {
            put32(out, offset, 1);
        }
        for (index, region) in self.regions.iter().enumerate() {
            let offset = 0x2610 + index * 0x40;
            region.build(&mut out[offset..offset + 0x40])?;
        }
        runs(out, self.opaque.unwrap_or(constants::HWDATA_CONSTANTS))
    }
}

pub(crate) fn region_c(out: &mut [u8]) -> Result<(), InvalidSize> {
    clear(out, REGION_C_SIZE)?;
    runs(out, constants::REGION_C_CONSTANTS)
}

pub(crate) fn primary_status(
    out: &mut [u8],
    fwctl_state: u64,
    fwctl_ring: u64,
    config_header: usize,
    config_offset: usize,
    config_runs: &[(usize, &[u8])],
) -> Result<(), InvalidSize> {
    let header_end = config_header.checked_add(12).ok_or(InvalidSize)?;
    if out.len() < 0x48f0 || header_end > out.len() || config_offset > out.len() {
        return Err(InvalidSize);
    }
    out.fill(0);
    status(&mut out[..0x80], false)?;
    put64(out, 0x48e0, fwctl_state);
    put64(out, 0x48e8, fwctl_ring);
    put32(out, config_header, 1);
    out[config_offset..].fill(0);
    runs(&mut out[config_offset..], config_runs)
}
