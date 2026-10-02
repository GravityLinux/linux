// SPDX-License-Identifier: GPL-2.0-only OR MIT

//! Source-owned direct compute bootstrap from DRMAsahiShim.g17p_cold_boot.
//! This is the authored add3 workload used by the Python bootstrap, never a
//! caller command. Firmware graph construction stays in compute::build.

use super::super::{g17p_compute as c, g17p_queue as q, g17p_user_vm::UserVm};
use super::{compute, g17p_memory, Image, Phase, Session};
use kernel::prelude::*;

const PAGE: usize = 0x4000;
const CODE: u64 = c::USC_EXEC_BASE;
const SHADER: u64 = 0x100000a8000;
const CDM: u64 = 0x100000c8000;
const RESOURCE: u64 = 0x100000f8000;
const INPUT_A: u64 = 0x10000030000;
const INPUT_B: u64 = 0x10000038000;
const OUTPUT: u64 = 0x10000040000;
// Compiler output for our own a[i] + b[i] program. The enclosing code image,
// resource table and dispatch records below are constructed field by field.
const CONSTANT_PROGRAM: &[u8] = &[
    0x03, 0x00, 0x07, 0x00, 0x02, 0x00, 0x00, 0x00, 0x60, 0x00, 0x0e, 0x00, 0x00, 0x00, 0x06, 0x00,
    0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00,
    0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00,
    0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00, 0x06, 0x00,
];
const MAIN_PROGRAM: &[u8] = &[
    0x1c, 0xa0, 0x10, 0x06, 0x67, 0x10, 0x54, 0x00, 0x00, 0x01, 0x20, 0x00, 0x51, 0x01, 0x00, 0x40,
    0x46, 0x00, 0x67, 0x00, 0x44, 0x04, 0x01, 0x01, 0x20, 0x00, 0x51, 0x01, 0x00, 0x40, 0x46, 0x00,
    0x09, 0x05, 0x1c, 0x01, 0x00, 0xc0, 0xe7, 0x00, 0x54, 0x00, 0x02, 0x01, 0x21, 0x00, 0x11, 0x00,
    0x00, 0x90, 0x11, 0x00, 0x0e, 0x00, 0x00, 0x00,
];
const LAUNCH_HEADER: &[u8] = &[
    0x2c, 0xa0, 0x02, 0x00, 0x12, 0x08, 0x7c, 0x00, 0x3c, 0x80, 0x02, 0x00, 0x04, 0x00, 0x00, 0x00,
    0x8c, 0xa0, 0x42, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x9c, 0x80, 0x42, 0x00, 0x04, 0x00, 0x00, 0x00,
    0x67, 0x00, 0x54, 0x2c, 0x02, 0x00, 0x00, 0x00, 0x59, 0x00, 0x02, 0x40, 0x26, 0x00, 0x67, 0x00,
    0x54, 0x24, 0x02, 0x00, 0x00, 0x00, 0x57, 0x00, 0x00, 0x40, 0x26, 0x00, 0x67, 0x00, 0x54, 0x30,
    0x18, 0x00, 0x00, 0x00, 0x59, 0x04, 0x00, 0x40, 0x26, 0x00, 0x77, 0x00, 0x2a, 0x41, 0x00, 0x00,
    0x00, 0x00, 0x77, 0x01, 0xaa, 0x07, 0x00, 0x00, 0x00, 0x02, 0x04, 0x00, 0xf7, 0x00, 0x2a, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1c, 0x80, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x14, 0x81,
    0x11, 0x06, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x80, 0x02, 0x00, 0x04, 0x00, 0x00, 0x00, 0x9f, 0x11,
    0x54, 0x00, 0x02, 0x00, 0x08, 0xa8, 0x10, 0x05, 0x1c, 0x80, 0x02, 0x00, 0x04, 0x00, 0x00, 0x00,
    0x0f, 0x12, 0x54, 0x00, 0x4c, 0x00, 0x4b, 0x2c, 0x09, 0x44, 0x5b, 0x2e, 0x09, 0x04, 0x0b, 0x24,
    0x09, 0x04, 0x1b, 0x26, 0x09, 0x04, 0x2b, 0x28, 0x09, 0x04, 0x3b, 0x2a, 0x09, 0x04, 0x6b, 0x30,
    0x09, 0x04, 0x7b, 0x32, 0x09, 0x04, 0x03, 0x00, 0x07, 0x00, 0x02, 0x00, 0x00, 0x00, 0x60, 0x00,
    0x0e, 0x00, 0x00, 0x00,
];

// IEEE-754 encodings of 1000 + index and 1000.5 + index, without issuing
// floating-point instructions in kernel context. The exponent changes at 24.
fn vector_word(index: u32, half: bool) -> u32 {
    if index < 24 {
        0x447a0000 + index * 0x4000 + if half { 0x2000 } else { 0 }
    } else {
        0x44800000 + (index - 24) * 0x2000 + if half { 0x1000 } else { 0 }
    }
}

fn code_image(out: &mut [u8]) {
    out.fill(0);
    c::u32_at(out, 0, 0x340);
    for offset in (0x40..0x340).step_by(2) {
        out[offset..offset + 2].copy_from_slice(&6u16.to_le_bytes());
    }
    for base in [0x100, 0x200] {
        for (index, opcode) in [0x60, 0x50, 0x40, 0x30, 0x20, 0x10].into_iter().enumerate() {
            let at = base + index * 0x10;
            out[at..at + 0x10]
                .copy_from_slice(&[0x0f, 0, 0x54, opcode, 0, 0, 0, 0, 0, 0, 6, 0, 6, 0, 6, 0]);
        }
        for offset in [0x60, 0x70] {
            out[base + offset..base + offset + 0x10]
                .copy_from_slice(&[0xf7, 3, 0xaa, 0, 0x8f, 2, 0x54, 1, 6, 0, 6, 0, 6, 0, 6, 0]);
        }
    }
    c::u32_at(out, 0x340, 0xc0);
    out[0x380..0x3c0].copy_from_slice(CONSTANT_PROGRAM);
    out[0x3c0..0x3c0 + MAIN_PROGRAM.len()].copy_from_slice(MAIN_PROGRAM);
}
fn cdm_stream(out: &mut [u8]) {
    out.fill(0);
    c::u32_at(out, 0, 0x80000);
    c::u32_at(out, 4, 0x1000000);
    let encoded = ((SHADER >> 6) & 0xffffffff) | ((0x40000000 | (SHADER >> 40)) << 32);
    c::u64_at(out, 8, encoded);
    for (index, value) in [64, 1, 1, 32, 1, 1].into_iter().enumerate() {
        c::u32_at(out, 0x10 + index * 4, value);
    }
    c::u32_at(out, 0x28, 0x60000160);
    c::u32_at(out, 0x2c, 0x40000000);
}
impl Session {
    /// Complete the backend-owned primer before mirroring any caller BOs.
    /// A failure retains the graph in Session and prevents caller publication.
    pub(super) fn bootstrap_compute(
        &mut self,
        dev: &kernel::device::Device,
        image: &Image,
        owner: (u64, u32),
        preempt: u64,
    ) -> Result {
        if self.phase != Phase::Prepared || self.compute.is_some() || self.render.is_some() {
            return Err(EBUSY);
        }
        let result = (|| {
            let mut client = compute::Client {
                root: UserVm::new()?,
                buffers: KVec::new(),
                bindings: KVec::new(),
                owner,
                cpu_maps: crate::g17p_compute_runtime::CpuMaps::new()?,
                primer_aliases: Some([(0, 0); 2]),
            };
            let mut body = KVVec::with_capacity(PAGE, GFP_KERNEL)?;
            body.resize(PAGE, 0, GFP_KERNEL)?;
            let mut output = 0;
            let memory = self.memory.as_mut().ok_or(EINVAL)?;
            for (address, writable) in [
                (CODE, false),
                // Source native_shader_attributes=True maps the launch
                // header with UXN=1 (GPU resource class), unlike code/CDM.
                (SHADER, true),
                (CDM, false),
                (RESOURCE, true),
                (INPUT_A, true),
                (INPUT_B, true),
                (OUTPUT, true),
            ] {
                body.fill(0);
                match address {
                    CODE => code_image(&mut body),
                    SHADER => body[..LAUNCH_HEADER.len()].copy_from_slice(LAUNCH_HEADER),
                    CDM => cdm_stream(&mut body),
                    RESOURCE => {
                        for (index, buffer) in [INPUT_A, INPUT_B, OUTPUT].into_iter().enumerate() {
                            c::u64_at(&mut body, 0x14a0 + index * 8, buffer);
                        }
                    }
                    INPUT_A => {
                        for i in 0..64 {
                            c::u32_at(&mut body, i * 4, vector_word(i as u32, false));
                        }
                    }
                    INPUT_B => {
                        for i in 0..64 {
                            c::u32_at(&mut body, i * 4, 0x3f000000);
                        }
                    }
                    OUTPUT => body.fill(0xa5),
                    _ => return Err(EINVAL),
                }
                let size = match address {
                    SHADER | CDM => 2 * PAGE,
                    RESOURCE => 3 * PAGE,
                    _ => PAGE,
                };
                let pa = memory.allocate(size)?;
                memory.write(pa, &body)?;
                memory.clean(pa, size)?;
                client.root.prepare(address, size as u64)?;
                for offset in (0..size).step_by(PAGE) {
                    client
                        .root
                        .map_page(address + offset as u64, pa + offset as u64, writable)?;
                }
                client.bindings.push(
                    (address, size as u64, 0, if writable { 6 } else { 2 }),
                    GFP_KERNEL,
                )?;
                if address == OUTPUT {
                    output = pa;
                }
            }
            let parameters = compute::Parameters {
                preempt,
                cdm: CDM,
                end: CDM + 0x30,
                sampler: 0,
                sampler_count: 0,
                timestamps: [0; 2],
            };
            self.prepare_compute(dev, image, client, &parameters)?;
            // Program::preempt is the driver-owned Tier-2 table. Populate it
            // after build allocated the private resource/preemption pages.
            let memory = self.memory.as_mut().ok_or(EINVAL)?;
            let root = &self.compute.as_ref().ok_or(EINVAL)?.client.root;
            let pa = root.pte(preempt)? & 0x3ffffffc000;
            if pa == 0 {
                return Err(EIO);
            }
            for (index, address) in [INPUT_A, INPUT_B, OUTPUT].into_iter().enumerate() {
                memory.write64(pa + 0x14a0 + index as u64 * 8, address)?;
            }
            memory.clean(pa, PAGE)?;
            self.start(dev, image)?;
            self.dormant_render
                .as_ref()
                .ok_or(EINVAL)?
                .initialize_operands(
                    self.memory.as_mut().ok_or(EINVAL)?,
                    self.vm.as_ref().ok_or(EINVAL)?,
                )?;
            let startup = self.report_snapshot(image)?;
            let frame = self.compute.as_ref().ok_or(EINVAL)?.pending()?;
            let (address, value) = frame.publication.deferred_outer.ok_or(EIO)?;
            self.vm.as_ref().ok_or(EINVAL)?.write(
                self.memory.as_mut().ok_or(EINVAL)?,
                2,
                address,
                &value.to_le_bytes(),
            )?;
            g17p_memory::sync();
            self.peers[0]
                .rtkit
                .as_mut()
                .ok_or(EIO)?
                .as_mut()
                .send_message(0x21, q::COMPUTE_DOORBELL)?;
            self.finish_compute(dev, image, &frame, &startup, false, true)?;
            let memory = self.memory.as_ref().ok_or(EINVAL)?;
            memory.invalidate(output, PAGE)?;
            for i in 0..32 {
                let actual = memory.read64(output + i * 8)?;
                let expected = vector_word((2 * i) as u32, true) as u64
                    | (vector_word((2 * i + 1) as u32, true) as u64) << 32;
                if actual != expected {
                    dev_err!(
                        dev,
                        "G17P: source bootstrap output {} mismatch: {:#018x}, expected {:#018x}\n",
                        i,
                        actual,
                        expected
                    );
                    return Err(EIO);
                }
            }
            for offset in (256..PAGE).step_by(8) {
                if memory.read64(output + offset as u64)? != 0xa5a5a5a5a5a5a5a5 {
                    return Err(EIO);
                }
            }
            self.bootstrapped = true;
            dev_info!(dev, "G17P: source-owned direct bootstrap complete; 64 add3 outputs and guards verified\n");
            Ok(())
        })();
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result
    }
}
