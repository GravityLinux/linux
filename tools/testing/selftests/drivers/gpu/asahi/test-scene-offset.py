#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Check the actual BufferThing serializer, register builders and scene leases."""
from pathlib import Path
import os
import re
import subprocess
import tempfile

root = next(p for p in Path(__file__).resolve().parents
            if (p / 'drivers/gpu/drm/asahi/g16_render.rs').exists())
driver = root / 'drivers/gpu/drm/asahi'
tvb = (driver / 'g16_tvb.rs').read_text()
runtime = ''.join(p.read_text() for p in sorted(driver.glob('g16_runtime*.rs')))
util = (driver / 'm4_util.rs').read_text()


def item(source, pattern):
    start = re.search(pattern, source).start()
    brace = source.index('{', start)
    depth, end = 1, brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


fixture = r'''
#![allow(dead_code, non_upper_case_globals)]
#[derive(Debug, PartialEq)] struct Error;
const EBUSY: Error = Error;
type Result<T> = std::result::Result<T, Error>;
mod module_parameters {
    pub struct Parameter(u32);
    impl Parameter { pub fn value(&self) -> u32 { self.0 } }
    pub static notifier_flush: Parameter = Parameter(0);
}
// Only fragment_work uses this shim; the test calls the register builders.
mod float {
    pub struct F32(f32);
    impl F32 {
        pub fn from_bits(bits: u32) -> Self { Self(f32::from_bits(bits)) }
        pub fn to_bits(self) -> u32 { self.0.to_bits() }
    }
    impl From<u32> for F32 { fn from(v: u32) -> Self { Self(v as f32) } }
    impl std::ops::Div for F32 {
        type Output = Self;
        fn div(self, rhs: Self) -> Self { Self(self.0 / rhs.0) }
    }
}
'''
fixture += 'mod util {\n'
for name in ('put32', 'put64'):
    fixture += item(util, r'pub\(crate\) fn ' + name + r'\(') + '\n'
fixture += '}\n'
for name in ('g16_render', 'g16_stamp', 'g16_mesh'):
    fixture += f'#[path = "{driver / (name + ".rs")}"] mod {name};\n'
fixture += 'use g16_render as render;\n'
fixture += re.search(r'const SCENES: u32 = .*?;', tvb).group() + '\n'
fixture += '#[allow(dead_code)]\n' + item(tvb, r'pub\(crate\) enum Refusal') + '\n'
fixture += '#[derive(Default)]\n' + item(tvb, r'pub\(crate\) struct Tvb')
fixture += '\nimpl Tvb {\n'
for name in ('reserve_scene', 'release_scene', 'work_addresses'):
    fixture += item(tvb, r'pub\(crate\) fn ' + name + r'\(') + '\n'
fixture += '}\nfn select(params: &mut render::Parameters, scene: u32) {\n'
fixture += re.search(r'params\.scene_slot = [^;]+;\s*params\.cycle = [^;]+;',
                     runtime).group() + '\n}\n'
fixture += r'''
fn read64(bytes: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap())
}
fn scene_offset(pool: &Tvb, page: &[u8], scene: u32) -> (u64, u64) {
    let mut work = render::Addresses::bootstrap();
    pool.work_addresses(&mut work, scene);
    let record = (work.buffer_thing - (pool.addresses.buffer_thing & !0x3fff)) as usize;
    let mut p = render::Parameters::default();
    p.set_private(0x10e0000000, 0x1100000000).unwrap();
    p.width = 64; p.height = 64; p.layers = 1; p.samples = 1;
    p.utile_width = 32; p.utile_height = 32;
    p.encoder = p.context_base + 0x10000;
    select(&mut p, scene);
    let ta = p.tiling_registers().unwrap();
    let frag = p.fragment_registers().unwrap();
    for (registers, number) in [(&ta[..], 0x1ca30), (&ta[..], 0x16c39), (&frag[..], 0x1ca28)] {
        assert_eq!(registers.iter().find(|(reg, _)| *reg == number).unwrap().1, p.cycle);
    }
    let cursor_reg = ta.iter().find(|(reg, _)| *reg == 0x1c910).unwrap().1;
    assert_eq!(read64(page, record), p.context_base + cursor_reg - 1);
    assert_eq!(read64(page, record + 8), pool.addresses.buffer_manager_scene_list + u64::from(scene) * 4);
    let offset = read64(page, record + 0x28);
    // The fixed alias maps one 16-KiB scene page. All records must fit.
    assert!((0x240000..=0x244000 - 48).contains(&offset));
    (offset, p.cycle)
}
fn main() {
    for base in [0xfffffc20c0600000, 0xfffffc20c3804000] {
        let mut addresses = render::Addresses::bootstrap();
        addresses.buffer_thing = base + 0x80;
        let mut pool = Tvb { addresses, ..Default::default() };
        let mut page = [0u8; 0x4000];
        addresses.private_page(base, &mut page).unwrap();
        for _ in 0..3 {
            let mut live = std::collections::BTreeSet::new();
            let mut mismatches = Vec::new();
            for expected in 0..SCENES {
                let scene = pool.reserve_scene().unwrap();
                assert_eq!(scene, expected);
                let (offset, register) = scene_offset(&pool, &page, scene);
                if offset != register { mismatches.push((scene, offset, register)); }
                live.insert(offset);
            }
            assert!(mismatches.is_empty(), "{} mismatches; first={:?}",
                    mismatches.len(), mismatches.first());
            assert_eq!(live.len(), SCENES as usize, "live scenes must not alias");
            assert_eq!(pool.reserve_scene(), Err(EBUSY));
            // Keep odd leases alive while reusing the retired even leases.
            for scene in (0..SCENES).step_by(2) { pool.release_scene(scene); }
            for expected in (0..SCENES).step_by(2) {
                let scene = pool.reserve_scene().unwrap();
                assert_eq!(scene, expected);
                let (offset, register) = scene_offset(&pool, &page, scene);
                assert_eq!(offset, register);
            }
            assert_eq!(pool.reserve_scene(), Err(EBUSY));
            for scene in 0..SCENES { pool.release_scene(scene); }
            // Leave the next full cycle starting at zero after the sparse reuse.
            let last = pool.reserve_scene().unwrap();
            assert_eq!(last, SCENES - 1);
            pool.release_scene(last);
        }
    }
    println!("PASS: 80 unique scene offsets, TA/3D and cursor agreement, exhaustion and reuse; 2 placements x 3 cycles");
}
'''
with tempfile.TemporaryDirectory(prefix='asahi-scene-', dir=os.getenv('TMPDIR')) as temp:
    path = Path(temp)
    (path / 'test.rs').write_text(fixture)
    subprocess.run(['rustc', '--edition=2021', '-C', 'opt-level=1',
                    str(path / 'test.rs'), '-o', str(path / 'test')], check=True)
    subprocess.run([str(path / 'test')], check=True)
