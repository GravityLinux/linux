// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! Live platform inputs and initialization builders from g16g_platform.py and
//! g16g_initdata.py. No archived performance or register-window profile.

use kernel::{c_str, device, of, prelude::*};

const PAGE: u64 = 0x4000;
const REGISTER_BASE: u64 = 0xffff_fc21_8020_0000;
const REGISTER_LIMIT: u64 = 0xffff_fc21_8100_0000;
const MCC_BASE: u64 = 0x2201c4000;
const MCC_SIZE: u32 = 0x18000;

pub(crate) fn put32(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn put64(out: &mut [u8], offset: usize, value: u64) {
    out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn zeroed(size: usize) -> Result<KVec<u8>> {
    let mut out = KVec::new();
    out.extend_with(size, 0, GFP_KERNEL)?;
    Ok(out)
}

/// The boot boundary preserves original ADT bytes, including float bit patterns.
pub(crate) fn adt_word(node: &of::Node, name: &CStr, default: Option<u32>) -> Result<u32> {
    let bytes: Option<KVec<u8>> = node.get_opt_property(name)?;
    match bytes {
        Some(bytes) => Ok(u32::from_le_bytes(
            bytes.as_slice().try_into().map_err(|_| EINVAL)?,
        )),
        None => default.ok_or(ENOENT),
    }
}

fn ladder(node: &of::Node, name: &CStr) -> Result<[u32; 11]> {
    let values: KVec<u32> = node.get_property(name)?;
    values.as_slice().try_into().map_err(|_| EINVAL)
}

pub(crate) struct Register {
    pub(crate) slot: usize,
    pub(crate) physical: u64,
    pub(crate) address: u64,
    pub(crate) size: u32,
    pub(crate) stride: u32,
    pub(crate) relative: u64,
    pub(crate) flags: u32,
}

pub(crate) struct Platform {
    pub(crate) node: of::Node,
    pub(crate) freq_a: [u32; 11],
    pub(crate) freq_b: [u32; 11],
    core_voltage: [u32; 11],
    memory_voltage: [u32; 11],
    index_a: [u32; 11],
    index_b: [u32; 11],
    relative_a: [u32; 11],
    relative_b: [u32; 11],
    pub(crate) maximum_power: u32,
    pub(crate) core_mask: u32,
    revision: u32,
    pub(crate) period_ms: u32,
    pub(crate) period_clocks: u32,
    pub(crate) registers: KVec<Register>,
    pub(crate) mcc: KVec<u64>,
    gpc: bool,
}

impl Platform {
    pub(crate) fn new(dev: &device::Device) -> Result<Self> {
        let node = dev.of_node().ok_or(ENODEV)?;
        let power = ladder(&node, c_str!("apple,m4-power-mw"))?;
        let freq_a = ladder(&node, c_str!("apple,m4-freq-a"))?;
        let freq_b = ladder(&node, c_str!("apple,m4-freq-b"))?;
        if power[0] != 0
            || power[10] == 0
            || power[10] > i32::MAX as u32
            || power.windows(2).any(|w| w[0] > w[1])
            || freq_a[0] != 0
            || freq_a.windows(2).any(|w| w[0] >= w[1])
            || freq_b[0] != 0
            || freq_b.windows(2).any(|w| w[0] >= w[1])
        {
            return Err(EINVAL);
        }
        let period_clocks = match adt_word(
            &node,
            c_str!("apple,sgx-gpu-pwr-sample-period-aic-clks"),
            None,
        ) {
            Ok(value) => value,
            Err(ENOENT) => adt_word(&node, c_str!("apple,sgx-gpu-power-sample-period"), None)?
                .checked_mul(24000)
                .ok_or(EINVAL)?,
            Err(error) => return Err(error),
        };
        let period_ms = period_clocks / 24000;
        if period_ms == 0 {
            return Err(EINVAL);
        }
        let core_mask: u32 = node.get_property(c_str!("apple,m4-core-mask"))?;
        if core_mask == 0 || core_mask & !0x3ff != 0 {
            return Err(EINVAL);
        }
        let mut this = Self {
            core_voltage: ladder(&node, c_str!("apple,m4-core-voltage"))?,
            memory_voltage: ladder(&node, c_str!("apple,m4-memory-voltage"))?,
            index_a: ladder(&node, c_str!("apple,m4-index-a"))?,
            index_b: ladder(&node, c_str!("apple,m4-index-b"))?,
            relative_a: ladder(&node, c_str!("apple,m4-relative-a"))?,
            relative_b: ladder(&node, c_str!("apple,m4-relative-b"))?,
            revision: node.get_property(c_str!("apple,chip-revision"))?,
            gpc: adt_word(&node, c_str!("apple,sgx-no-gpc"), Some(0))? == 0,
            node,
            freq_a,
            freq_b,
            maximum_power: power[10],
            core_mask,
            period_ms,
            period_clocks,
            registers: KVec::new(),
            mcc: KVec::new(),
        };
        this.build_registers()?;
        Ok(this)
    }

    fn build_registers(&mut self) -> Result {
        let reg: KVec<u32> = self.node.get_property(c_str!("reg"))?;
        if reg.len() != 8 {
            return Err(EINVAL);
        }
        let base = (u64::from(reg[4]) << 32) | u64::from(reg[5]);
        let span = (u64::from(reg[6]) << 32) | u64::from(reg[7]);
        let irq: KVec<u8> = self
            .node
            .get_property(c_str!("apple,sgx-meta-sw-interrupt"))?;
        if irq.len() != 20 && irq.len() != 40 {
            return Err(EINVAL);
        }
        let irq_address = u64::from_le_bytes(
            irq[irq.len() - 20..irq.len() - 12]
                .try_into()
                .map_err(|_| EINVAL)?,
        );
        if irq_address < PAGE || irq_address >= 1 << 48 {
            return Err(EINVAL);
        }
        let mask = adt_word(
            &self.node,
            c_str!("apple,sgx-active-mcc-bit-vector"),
            Some(3),
        )?;
        if mask == 0 {
            return Err(EINVAL);
        }
        for bit in 0..32 {
            if mask & (1 << bit) != 0 {
                self.mcc.push(MCC_BASE + (bit << 25), GFP_KERNEL)?;
            }
        }
        self.registers.push(
            Register {
                slot: 0,
                physical: irq_address & !(PAGE - 1),
                address: 0,
                size: PAGE as u32,
                stride: PAGE as u32,
                relative: 0,
                flags: 2,
            },
            GFP_KERNEL,
        )?;
        self.registers.push(
            Register {
                slot: 3,
                physical: self.mcc[0],
                address: 0,
                size: self.mcc.len() as u32 * MCC_SIZE,
                stride: MCC_SIZE,
                relative: 0,
                flags: 2,
            },
            GFP_KERNEL,
        )?;
        // Native HAL declarations and the three platform-specific windows.
        for (slot, offset, size, relative) in [
            (17, 0, 0x20000, true),
            (26, 0xd04000, 0x8000, true),
            (27, 0xd0d000, 0x1000, true),
            (28, 0xd58000, 0x8000, true),
            (29, 0xd10000, 0x4000, true),
            (31, 0xd40000, 0x4000, true),
            (32, 0xd60000, 0x8000, true),
            (40, 0xe1c000, 0x4000, true),
            (22, 0x1000000, 0x8000, false),
            (39, 0xe08000, 0x8000, false),
            (41, 0xe1f800, 0x4000, false),
        ] {
            if offset + u64::from(size) > span {
                return Err(EINVAL);
            }
            self.registers.push(
                Register {
                    slot,
                    physical: base.checked_add(offset).ok_or(EINVAL)?,
                    address: 0,
                    size,
                    stride: size,
                    relative: if relative { offset } else { 0 },
                    flags: 2,
                },
                GFP_KERNEL,
            )?;
        }
        for (slot, physical, size, flags) in [
            (9, 0x3803d0000, 0x1000, 2),
            (10, 0x3803c0000, 0x2000, 0),
            (12, 0x50165c000, 0x4000, 2),
            (14, 0x380280000, 0x8000, 0),
            (15, 0x38c840000, 0x24000, 0),
        ] {
            self.registers.push(
                Register {
                    slot,
                    physical,
                    address: 0,
                    size,
                    stride: size,
                    relative: 0,
                    flags,
                },
                GFP_KERNEL,
            )?;
        }
        self.registers.sort_unstable_by_key(|r| r.slot);
        let mut cursor = REGISTER_BASE;
        for r in &mut self.registers {
            let offset = r.physical & (PAGE - 1);
            r.address = cursor + offset;
            cursor += (offset + u64::from(r.size) + PAGE - 1) & !(PAGE - 1);
            cursor += PAGE;
            if cursor > REGISTER_LIMIT {
                return Err(ENOSPC);
            }
        }
        Ok(())
    }

    pub(crate) fn hwdata(&self) -> Result<KVec<u8>> {
        let mut out = zeroed(0x3db4)?;
        for r in &self.registers {
            let off = 0x640 + r.slot * 0x28;
            put64(&mut out, off, r.physical);
            put64(&mut out, off + 8, r.address);
            put32(&mut out, off + 0x10, r.size);
            put32(&mut out, off + 0x14, r.stride);
            put64(&mut out, off + 0x18, r.relative);
            put32(&mut out, off + 0x20, r.flags);
        }
        for (base, repeats, frequencies) in [(0xfc8, 16, &self.freq_a), (0x1cdc, 1, &self.freq_b)] {
            for state in 0..11 {
                put32(&mut out, base + state * 4, frequencies[state]);
                for word in 0..repeats {
                    put32(
                        &mut out,
                        base + 0x40 + state * 0x40 + word * 4,
                        self.core_voltage[state],
                    );
                    put32(
                        &mut out,
                        base + 0x440 + state * 0x40 + word * 4,
                        self.memory_voltage[state],
                    );
                }
            }
        }
        for (off, values) in [
            (0x1808, &self.freq_b),
            (0x18c8, &self.relative_a),
            (0x1908, &self.relative_b),
            (0x19c8, &self.index_a),
            (0x1a08, &self.index_b),
        ] {
            for (i, value) in values.iter().enumerate() {
                put32(&mut out, off + i * 4, *value);
            }
        }
        for i in 0..11 {
            put32(&mut out, 0x1848 + i * 4, 1.02f32.to_bits());
        }
        for (off, value) in [
            (0xe90, 0x8132),
            (0xe94, self.revision >> 4),
            (0xe98, self.revision & 7),
            (0xea0, u32::from(self.gpc)),
            (0xeb8, 1),
            (0xec8, 1),
            (0xed0, 24000),
            (0xed8, self.period_ms),
            (0xee0, 1),
            (0xee4, 1),
            (0xee8, 1),
            (0xfc4, 10),
            (0x1cd8, 10),
            (0x2604, 1),
        ] {
            put32(&mut out, off, value);
        }
        out[0x258e..0x2590].copy_from_slice(&22u16.to_le_bytes());
        for group in 0..2 {
            for word in 1..4 {
                put32(&mut out, 0x2590 + group * 16 + word * 4, 1);
            }
        }
        for word in 0..12 {
            put32(&mut out, 0x25b4 + word * 4, u32::MAX);
        }
        Ok(out)
    }

    pub(crate) fn region_c(&self) -> Result<KVec<u8>> {
        let mut out = zeroed(PAGE as usize)?;
        for (off, value) in [
            (0x78, 1),
            (0x7ec, 1),
            (0x7f0, 1),
            (0xe28, 3),
            (0xe2c, 1),
            (0x24, 4096),
            (0x998, 32),
            (0x99c, 8),
            (0x9a0, 256),
            (0x9a4, 1),
            (0x9c8, 64),
            (0x9cc, 64),
            (0x9d0, 2),
            (0x84, self.maximum_power),
            (0x88, 1000),
            (0x8c, 1000),
            (0x98, 1000),
        ] {
            put32(&mut out, off, value);
        }
        for (off, value) in [(0x54, u16::MAX), (0x56, 40), (0x58, u16::MAX)] {
            out[off..off + 2].copy_from_slice(&value.to_le_bytes());
        }
        Ok(out)
    }
}
