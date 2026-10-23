// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Byte serializers from m1n1's g17p_initdata.py.
//!
//! All addresses are supplied by the object owner. These functions only fill
//! unpublished storage; they never update firmware-owned state or publish a
//! producer. In particular, unaligned qwords are serialized as bytes.

use super::scheduled_initdata as wire;
pub(crate) use wire::FirmwareRole;
pub(crate) const G17P_ROLES: &[FirmwareRole] = wire::G17P_ROLES;

pub(crate) const ROOT_SIZE: usize = wire::InitRootG17P::SIZE;
pub(crate) const ROOT_SECONDARY_SIZE: usize = wire::SecondaryInitRootG17P::SIZE;
pub(crate) const MAIN_SIZE: usize = wire::MainConfigG17P::SIZE;
pub(crate) const CHANNELS: usize = 17;
pub(crate) const HWDATA_SIZE: usize = 0x3db4;
pub(crate) const REGION_C_SIZE: usize = super::scheduled_boot_records::RegionCG17P::SIZE;

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
    pub(crate) fn size_for_role(role: FirmwareRole) -> usize {
        match role {
            FirmwareRole::Primary => ROOT_SIZE,
            FirmwareRole::Secondary => ROOT_SECONDARY_SIZE,
        }
    }

    pub(crate) fn size(&self) -> usize {
        if self.secondary_extra == [0; 2] {
            ROOT_SIZE
        } else {
            ROOT_SECONDARY_SIZE
        }
    }

    pub(crate) fn build(&self, out: &mut [u8]) -> Result<(), InvalidSize> {
        self.build_image(out, self.size())
    }

    /// Real instance construction selects the wire form by role, independently
    /// of whether a pointer happens to be zero while staging is populated.
    pub(crate) fn build_for_role(
        &self,
        role: FirmwareRole,
        out: &mut [u8],
    ) -> Result<(), InvalidSize> {
        let kind = match role {
            FirmwareRole::Primary => 0,
            FirmwareRole::Secondary => 1,
        };
        if self.kind != kind || (role == FirmwareRole::Primary && self.secondary_extra != [0; 2]) {
            return Err(InvalidSize);
        }
        self.build_image(out, Self::size_for_role(role))
    }

    fn build_image(&self, out: &mut [u8], size: usize) -> Result<(), InvalidSize> {
        clear(out, size)?;
        let root = wire::InitRootG17P {
            version: wire::version_words(self.version),
            region_a: self.region_a.to_le_bytes(),
            main: self.main.to_le_bytes(),
            region_c: self.region_c.to_le_bytes(),
            kind: self.kind.to_le_bytes(),
            status_a: self.status[0].to_le_bytes(),
            status_b: self.status[1].to_le_bytes(),
            ..Default::default()
        };
        if size == ROOT_SECONDARY_SIZE {
            let extended = wire::SecondaryInitRootG17P {
                root,
                extra_0: self.secondary_extra[0].to_le_bytes(),
                extra_1: self.secondary_extra[1].to_le_bytes(),
            };
            out.copy_from_slice(extended.as_bytes());
        } else {
            out.copy_from_slice(root.as_bytes());
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
        clear(out, wire::Channel::SIZE)?;
        let channel = wire::Channel {
            states: self.states.map(u64::to_le_bytes),
            ring: self.ring.to_le_bytes(),
        };
        out.copy_from_slice(channel.as_bytes());
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
    pub(crate) role: FirmwareRole,
    pub(crate) secondary_extra: u64,
}

impl MainConfig {
    pub(crate) fn build(&self, out: &mut [u8]) -> Result<(), InvalidSize> {
        clear(out, MAIN_SIZE)?;
        let mut main = wire::MainConfigG17P::for_role(self.role);
        main.hardware = self.hardware.to_le_bytes();
        main.repeated = self.repeated.to_le_bytes();
        main.repeated_2 = self.repeated.to_le_bytes();
        let first = match self.role {
            FirmwareRole::Primary => 0,
            FirmwareRole::Secondary => 12,
        };
        for (index, channel) in self.channels.iter().enumerate().skip(first) {
            main.channels[index] = wire::Channel {
                states: channel.states.map(u64::to_le_bytes),
                ring: channel.ring.to_le_bytes(),
            };
        }
        if self.role == FirmwareRole::Primary {
            main.auxiliary = self.addresses.map(u64::to_le_bytes);
        }
        main.regions = self
            .region_views
            .map(|(address, value)| wire::RegionTriple {
                address: address.to_le_bytes(),
                value: value.unwrap_or(0).to_le_bytes(),
                kind: if value.is_some() {
                    0x70u32.to_le_bytes()
                } else {
                    [0; 4]
                },
            });
        if self.role == FirmwareRole::Secondary {
            main.unaligned = self.secondary_extra.to_le_bytes();
        }
        out.copy_from_slice(main.as_bytes());
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
        let region = wire::MmioRegion {
            physical: self.physical.to_le_bytes(),
            address: self.address.to_le_bytes(),
            size: self.size.to_le_bytes(),
            stride: self.size.to_le_bytes(),
            relative: self.relative.to_le_bytes(),
            flags: self.flags.to_le_bytes(),
            ..Default::default()
        };
        out.copy_from_slice(region.as_bytes());
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
        let region = wire::HardwareRegion {
            lead: self.lead.to_le_bytes(),
            low: self.value.to_le_bytes(),
            high: self.address.to_le_bytes(),
            count: self.size_a.to_le_bytes(),
            stride: self.size_b.to_le_bytes(),
            trail: self.trail.to_le_bytes(),
            ..Default::default()
        };
        out.copy_from_slice(region.as_bytes());
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
        if out.len() < HWDATA_SIZE {
            return Err(InvalidSize);
        }
        out[..HWDATA_SIZE]
            .copy_from_slice(super::scheduled_hwdata::template_g17p_transport_bytes());
        for (slot, register) in self.registers {
            let offset = 0x640 + slot * 0x28;
            register.build(&mut out[offset..offset + 0x28])?;
        }
        for (slot, flags) in self.flags {
            put32(out, 0x640 + slot * 0x28 + 0x20, *flags);
        }
        let perf = self.performance;
        super::scheduled_hwdata::tables_g17p(
            out,
            &perf.freq_a,
            &perf.freq_b,
            &perf.core_voltage,
            &perf.memory_voltage,
        );
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
        for (index, region) in self.regions.iter().enumerate() {
            let offset = 0x2610 + index * 0x40;
            region.build(&mut out[offset..offset + 0x40])?;
        }
        runs(out, self.opaque.unwrap_or(constants::HWDATA_CONSTANTS))
    }
}

pub(crate) fn region_c(out: &mut [u8]) -> Result<(), InvalidSize> {
    if out.len() != REGION_C_SIZE {
        return Err(InvalidSize);
    }
    out.copy_from_slice(super::scheduled_boot_records::RegionCG17P::template_bytes());
    Ok(())
}

// Retained for explicit overlays in ABI regression fixtures. Runtime boot
// construction uses the versioned source template.
#[allow(dead_code)]
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
