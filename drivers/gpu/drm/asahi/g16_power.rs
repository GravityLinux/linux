// SPDX-License-Identifier: GPL-2.0-only
// Copyright The Gravity Linux Contributors

//! Power configuration from g16g_platform.py and g16g_initdata.py.
//! ADT values are original platform inputs; every firmware field is built here.

use crate::{
    f32,
    float::F32,
    g16_platform::{adt_word, put32, put64, zeroed, Platform},
};
use kernel::{c_str, prelude::*};

fn putf(out: &mut [u8], offset: usize, value: F32) -> Result {
    if value.to_bits() & 0x7f80_0000 == 0x7f80_0000 {
        return Err(EINVAL);
    }
    put32(out, offset, value.to_bits());
    Ok(())
}

fn coefficients(count: u32) -> Result<(F32, F32)> {
    if count == 0 {
        return Err(EINVAL);
    }
    let count = F32::from(count);
    Ok(((count - f32!(1.0)) / count, f32!(1.0) / count))
}

struct Filter {
    alpha: F32,
    inverse: F32,
    count_x4: u32,
    duration: u32,
    clocks: u64,
}

fn filter(count: u32, period: u32, request: Option<u32>) -> Result<Filter> {
    let (alpha, inverse) = coefficients(count)?;
    let count_x4 = count.checked_mul(4).ok_or(EINVAL)?;
    let duration = count.checked_mul(period).ok_or(EINVAL)?;
    let clocks = u64::from(duration.checked_mul(24000).ok_or(EINVAL)?);
    if let Some(ms) = request {
        if period == 0 || ms / period != count {
            return Err(EINVAL);
        }
    }
    Ok(Filter {
        alpha,
        inverse,
        count_x4,
        duration: request.unwrap_or(duration),
        clocks,
    })
}

impl Platform {
    /// Entire source-built bundle, including the firmware's power controllers.
    pub(crate) fn bundle(&self, entropy: u32) -> Result<KVec<u8>> {
        let mut out = zeroed(crate::g16_fw::BUNDLE_SIZE)?;
        let hwdata = self.hwdata()?;
        out[..hwdata.len()].copy_from_slice(&hwdata);
        let main = crate::g16_fw::MainConfig::bootstrap(crate::g16_fw::BUNDLE_ADDRESS).encode();
        out[crate::g16_fw::MAIN_OFFSET..crate::g16_fw::MAIN_OFFSET + main.len()]
            .copy_from_slice(&main);
        put64(&mut out, 0x8ed0, 0xffff_fc20_000f_3440 + 0x80);
        put64(&mut out, 0xb698, 0xffff_fc20_c050_0000);
        self.power_data(&mut out)?;
        put32(&mut out, 0xb754, entropy);
        Ok(out)
    }

    pub(crate) fn power_data(&self, out: &mut [u8]) -> Result {
        if out.len() < 0xb758 || self.period_ms == 0 {
            return Err(EINVAL);
        }
        // The property spellings and defaults follow the Python reader exactly.
        macro_rules! uint {
            ($name:literal, $default:expr) => {
                adt_word(
                    &self.node,
                    c_str!(concat!("apple,sgx-", $name)),
                    Some($default),
                )?
            };
        }
        macro_rules! optional {
            ($name:literal) => {
                match adt_word(&self.node, c_str!(concat!("apple,sgx-", $name)), None) {
                    Ok(value) => Some(value),
                    Err(ENOENT) => None,
                    Err(e) => return Err(e),
                }
            };
        }
        macro_rules! gain {
            ($name:literal, $default:expr) => {
                F32::from_bits(uint!($name, f32!($default).to_bits()))
            };
        }
        macro_rules! ints {
            ($($offset:expr => $value:expr),* $(,)?) => {
                $(put32(out, $offset, $value);)*
            };
        }
        macro_rules! floats {
            ($($offset:expr => $value:expr),* $(,)?) => {
                $(putf(out, $offset, $value)?;)*
            };
        }
        let max_scaled = 100 * (self.freq_a.len() as u32 - 1);
        let maximum = F32::from(self.maximum_power);
        let duty = uint!("gpu-pwr-min-duty-cycle", 40);
        let minimum = uint!("gpu-pwr-integral-min-clamp", 0);
        if duty > max_scaled || minimum > self.maximum_power {
            return Err(EINVAL);
        }
        let minimum = F32::from(minimum);
        let dt = F32::from(self.period_ms) / f32!(1000.0);
        let micros = self.period_ms.checked_mul(1000).ok_or(EINVAL)?;
        // Sixteen rows of signed 16-bit initial power slots.
        for i in 0..16 * 32 {
            out[0x6f6f + i * 2..0x6f71 + i * 2].copy_from_slice(&i16::MAX.to_le_bytes());
        }
        ints!(0x5384 => self.period_clocks, 0x5388 => self.period_clocks, 0x5d38 => 625);
        for i in 0..11 {
            put32(out, 0x5440 + i * 4, f32!(1.02).to_bits());
        }
        let (alpha, inverse) = coefficients(uint!("gpu-pwr-filter-time-constant", 313))?;
        floats!(0x5d44 => alpha, 0x5d4c => inverse,
            0x5d54 => gain!("gpu-pwr-integral-gain", 0.0202129),
            0x5d5c => minimum, 0x5d60 => maximum,
            0x5d64 => gain!("gpu-pwr-proportional-gain", 5.2831855),
            0x5d6c => -F32::from(max_scaled) / maximum);
        ints!(0x5d70 => duty, 0x5d74 => max_scaled, 0x5d78 => max_scaled, 0x5db8 => max_scaled);
        for off in [0x5d84, 0x7c48, 0x7c4c, 0x7c50] {
            put32(out, off, self.maximum_power);
        }
        floats!(0x7fb0 => f32!(65536.0), 0x7fb4 => f32!(0.0));
        ints!(0x7fc0 => duty, 0x7fc4 => max_scaled);
        floats!(0x7570 => f32!(65536.0), 0x7574 => F32::from(duty), 0x7578 => F32::from(max_scaled),
            0x7598 => f32!(100.0));
        ints!(0x757c => self.freq_a[10], 0x7590 => duty, 0x759c => max_scaled, 0x7bf0 => max_scaled);
        let (alpha, inverse) = coefficients(5)?;
        floats!(0x75a4 => alpha, 0x75a8 => inverse);

        let ppm_ms = uint!("gpu-ppm-filter-time-constant-ms", 16);
        let ppm = filter(ppm_ms / self.period_ms, self.period_ms, Some(ppm_ms))?;
        floats!(0x5dfc => ppm.alpha, 0x5e04 => ppm.inverse,
            0x5e0c => dt * gain!("gpu-ppm-ki", 150.0),
            0x5e14 => minimum, 0x5e18 => f32!(65536.0), 0x5e1c => gain!("gpu-ppm-kp", 0.25));
        ints!(0x5df0 => ppm.count_x4, 0x5e28 => duty, 0x5e2c => max_scaled,
            0x5e3c => self.maximum_power, 0x5e48 => ppm.duration);
        put64(out, 0x5e50, ppm.clocks);

        let base = uint!("gpu-perf-base-pstate", 1);
        let utilization = uint!("gpu-perf-tgt-utilization", 85);
        let dead_zone = uint!("gpu-perf-dz", 0);
        let boost = uint!("gpu-perf-boost-min-util", 95);
        let integral_minimum = uint!("gpu-perf-integral-min-clamp", 0);
        if base == 0
            || base >= 11
            || utilization > 100
            || dead_zone > utilization
            || boost > 100
            || integral_minimum > 95
        {
            return Err(EINVAL);
        }
        let base_scaled = 100 * base;
        let target = utilization - dead_zone;
        for off in [0x5390, 0x53e0] {
            put32(out, off, 4);
            putf(out, off + 4, f32!(1.0))?;
        }
        for off in [0x53ac, 0x53b0, 0x53ec, 0x53f0] {
            put32(out, off, 1);
        }
        for off in [0x53d0, 0x5410] {
            put32(out, off, 100);
        }
        for off in [0x53bc, 0x53fc] {
            for (i, value) in [0, base_scaled, 1, max_scaled].iter().enumerate() {
                put32(out, off + i * 4, *value);
            }
        }
        let first = coefficients(uint!("gpu-perf-filter-time-constant", 5))?;
        let second = coefficients(uint!("gpu-perf-filter-time-constant2", 200))?;
        floats!(0x5ecc => first.0, 0x5ed0 => second.0, 0x5ed4 => first.1, 0x5ed8 => second.1,
            0x5edc => gain!("gpu-perf-integral-gain", 0.7975),
            0x5ee0 => gain!("gpu-perf-integral-gain2", 0.7975),
            0x5ee4 => F32::from(integral_minimum), 0x5ee8 => f32!(95.0),
            0x5eec => gain!("gpu-perf-proportional-gain", 5.4),
            0x5ef0 => gain!("gpu-perf-proportional-gain2", 5.4),
            0x5ef4 => F32::from(max_scaled-base_scaled) / f32!(95.0));
        ints!(0x5ea8 => target, 0x5eb0 => boost,
            0x5eb4 => uint!("gpu-perf-boost-ce-step", 50),
            0x5eb8 => uint!("gpu-perf-reset-iters", 6), 0x5ec0 => 6, 0x5ec4 => 1,
            0x5ec8 => uint!("gpu-perf-filter-drop-threshold", 0),
            0x5ef8 => base_scaled, 0x5efc => max_scaled, 0x5f00 => base_scaled,
            0x5f0c => target, 0x5f14 => dead_zone, 0x5f40 => base_scaled);

        let first = filter(
            uint!("gpu-se-filter-time-constant", 9),
            self.period_ms,
            None,
        )?;
        let second = filter(
            uint!("gpu-se-filter-time-constant-1", 3),
            self.period_ms,
            None,
        )?;
        floats!(0x7ed8 => first.alpha, 0x7edc => second.alpha,
            0x7ee0 => first.inverse, 0x7ee4 => second.inverse,
            0x7ee8 => gain!("gpu-se-ki", -50.0) * dt,
            0x7eec => gain!("gpu-se-ki-1", -100.0) * dt,
            0x7ef0 => f32!(0.0), 0x7ef4 => f32!(65536.0),
            0x7ef8 => gain!("gpu-se-kp", -5.0), 0x7efc => gain!("gpu-se-kp-1", -5.0),
            0x7f40 => f32!(65536.0), 0x7f14 => F32::from(micros));
        ints!(0x7f04 => 100, 0x7f08 => max_scaled, 0x7f0c => base_scaled,
            0x7f24 => first.duration, 0x7f28 => second.duration, 0x7f68 => micros,
            0x7ecc => 50, 0x7ed0 => 1, 0x7f18 => 1400,
            0x7f6c => (33+self.period_ms/2)/self.period_ms,
            0x7f70 => uint!("gpu-se-inactive-threshold", 2500),
            0x7f74 => uint!("gpu-se-engagement-criteria", 600),
            0x7f78 => uint!("gpu-se-engagement-tolerance", 2),
            0x7f7c => uint!("gpu-se-reset-tolerance", 4),
            0x7f80 => uint!("gpu-se-reset-criteria", 50));
        put64(out, 0x7f2c, first.clocks);
        put64(out, 0x7f34, second.clocks);

        let average_target = filter(
            uint!("gpu-avg-power-target-filter-tc", 1),
            self.period_ms,
            None,
        )?;
        floats!(0x7c5c => average_target.alpha, 0x7c60 => average_target.inverse);
        ints!(0x7c64 => average_target.count_x4, 0x7c68 => average_target.duration);
        put64(out, 0x7c6c, average_target.clocks);
        let ki_only = optional!("gpu-avg-power-ki-only");
        let direct_ki = optional!("gpu-avg-power-ki");
        let mut avg_ki = F32::from_bits(ki_only.or(direct_ki).unwrap_or(f32!(16.875).to_bits()));
        if ki_only.is_some() || direct_ki.is_none() {
            avg_ki = avg_ki * dt;
        }
        let avg_duty = uint!("gpu-avg-power-min-duty-cycle", 40);
        if avg_duty > max_scaled {
            return Err(EINVAL);
        }
        floats!(0x7d58 => avg_ki, 0x7d60 => f32!(0.0), 0x7d64 => f32!(65536.0),
            0x7d68 => gain!("gpu-avg-power-kp", 2.42), 0x7d70 => f32!(0.0), 0x7d84 => maximum);
        ints!(0x7d74 => avg_duty, 0x7d78 => max_scaled, 0x7d7c => max_scaled,
            0x7d88 => self.maximum_power, 0x7dbc => max_scaled);
        let request = optional!("gpu-avg-power-filter-tc-ms");
        let count = match request {
            Some(ms) => ms / self.period_ms,
            None => uint!("gpu-avg-power-input-filter-tc", 248 / self.period_ms),
        };
        let average = filter(count, self.period_ms, request)?;
        ints!(0x7d3c => average.count_x4, 0x7d94 => average.duration);
        floats!(0x7d48 => average.alpha, 0x7d50 => average.inverse);
        put64(out, 0x7d9c, average.clocks);

        // Fast-die uses the untruncated AIC interval, unlike the other loops.
        let fast_dt = F32::from(self.period_clocks) / f32!(24000.0) / f32!(1000.0);
        let release = uint!("gpu-fast-die0-release-temp", 80);
        let delta = uint!("gpu-fast-die0-prop-tgt-delta", 0);
        if release > u32::MAX / 100 || delta > u32::MAX / 100 {
            return Err(EINVAL);
        }
        put64(
            out,
            0x75e8,
            (1 << 3) | (1 << 4) | (1 << 6) | (1 << 8) | (1 << 11) | (1 << 13) | (1 << 14),
        );
        ints!(0x75f0 => (F32::from(release)*f32!(100.0)).to_u32_checked().ok_or(EINVAL)?,
            0x75f8 => 4, 0x7630 => duty, 0x7634 => max_scaled, 0x7638 => max_scaled,
            0x763c => (F32::from(delta)*f32!(100.0)).to_u32_checked().ok_or(EINVAL)?,
            0x7644 => 11000, 0x7678 => max_scaled);
        floats!(0x7604 => f32!(0.0), 0x760c => f32!(1.0),
            0x7614 => gain!("gpu-fast-die0-integral-gain", 200.0)*fast_dt,
            0x761c => f32!(0.0), 0x7620 => f32!(65536.0),
            0x7624 => gain!("gpu-fast-die0-proportional-gain", 120.0), 0x762c => f32!(0.0));

        let turbo_input = optional!("gpu-turbo-controller-input");
        let mode = turbo_input.unwrap_or(2);
        let (kp, ki, target, delta, threshold) = if mode == 0 {
            (f32!(300.0), f32!(1.3), 9600, 300, 0)
        } else {
            (
                f32!(250.0),
                f32!(1.15),
                if mode == 1 { 0 } else { 10150 },
                0,
                700,
            )
        };
        let (alpha, inverse) = coefficients(200)?;
        out[0x7898] = u8::from(turbo_input.is_some());
        out[0x7899] = u8::from(mode == 1);
        floats!(0x7aaa => alpha, 0x7ab2 => inverse, 0x7aba => ki, 0x7ac2 => f32!(0.0),
            0x7ac6 => f32!(65536.0), 0x7aca => kp, 0x7ad2 => f32!(-100.0/65536.0),
            0x7ae6 => f32!(3000.0), 0x7b56 => f32!(64.0), 0x7b5a => f32!(65536.0));
        ints!(0x7ad6 => 900, 0x7ada => 1000, 0x7ade => 1000, 0x7aea => target,
            0x7af2 => delta, 0x7b1e => 1000, 0x7b3a => 200, 0x7b52 => threshold);

        let (alpha, inverse) = coefficients(32)?;
        floats!(0x81b8 => alpha, 0x81c0 => inverse, 0x81c8 => f32!(5.0),
            0x81d0 => f32!(0.0), 0x81d4 => f32!(65536.0), 0x81d8 => f32!(15.0));
        ints!(0x81ac => 4, 0x81e4 => max_scaled-100);
        for off in [0x81e8, 0x81ec, 0x81f8, 0x822c] {
            put32(out, off, max_scaled);
        }
        for (off, key) in [
            (0xb6a0, *b"g0TR"),
            (0xb708, *b"g0t0"),
            (0xb70c, *b"g0t1"),
            (0xb710, *b"g0EB"),
            (0xb714, *b"g0EL"),
        ] {
            put32(out, off, u32::from_be_bytes(key));
        }
        Ok(())
    }
}
