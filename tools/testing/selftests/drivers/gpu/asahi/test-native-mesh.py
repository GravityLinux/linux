#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Host tests of the real mesh parser, serializers, support and scratch setup.

The ordinary-render byte comparison uses g16_render.rs from the commit before
native mesh support was added (the parent of the commit that added
g16_mesh.rs), found in this checkout's git history; production #214 in the
original history. Without git history the test is skipped. No device, kernel
module, or external package is needed; rustc must be on PATH.
"""
from pathlib import Path
import os
import re
import subprocess
import sys
import tempfile

root = next(p for p in Path(__file__).resolve().parents
            if (p / 'drivers/gpu/drm/asahi/g16_mesh.rs').exists())
driver = root / 'drivers/gpu/drm/asahi'


def item(source, pattern):
    start = re.search(pattern, source).start()
    brace = source.index('{', start)
    depth, end = 1, brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


# Check that validation, support initialization and submission use the same
# tested mode mapping, and that submission passes that mode into the decoder.
runtime = (driver / 'g16_runtime.rs').read_text()
bootstrap = (driver / 'g16.rs').read_text()
mode_expr = r'g16_mesh::Mode::from_parameter\(crate::module_parameters::native_mesh.value\(\)\)\s*\.ok_or\(EINVAL\)\?'
# The boot value is validated once; support and submission read the runtime
# copy (debugfs asahi_m4/native_mesh) through the same mapping.
runtime_expr = r'g16_mesh::Mode::from_parameter\(g16_mesh::mode\(\)\)\s*\.ok_or\(EINVAL\)\?'
assert re.search(mode_expr, bootstrap)
assert re.search(r'let native =\s*' + runtime_expr + r'\s*\.enabled\(\);', runtime)
assert re.search(r'if lease\.native\s*\{\s*render::initialize_native_support', runtime)
assert re.search(r'let mesh_mode =\s*' + runtime_expr + r';\s*let params = if mesh_mode.enabled\(\)', runtime)
assert re.search(r'params\.mesh\.is_some\(\) && !support\.native', runtime)
assert 'native_mesh.value()' not in runtime
mesh = (driver / 'g16_mesh.rs').read_text()
assert 'pub(crate) const MAX_MODE: u32 = 2;' in mesh
assert 'g16_mesh::decode(params.encoder, mesh_mode,' in runtime
assert 'native_mesh.value() == 1' not in runtime
params = (driver / 'asahi_m4.rs').read_text()
assert re.search(r'native_mesh:\s*u32\s*\{\s*default: 0,', params)

fixture = r'''
#![allow(dead_code, non_upper_case_globals, unused_imports)]
mod module_parameters {
    pub struct Parameter(u32);
    impl Parameter { pub fn value(&self) -> u32 { self.0 } }
    pub static micro_layout: Parameter = Parameter(0);
}
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
mod mem { pub fn sync() {} }
mod util {
'''
util = (driver / 'm4_util.rs').read_text()
for name in ('put32', 'put64'):
    fixture += item(util, r'pub\(crate\) fn ' + name + r'\(') + '\n'
fixture += '}\n'
fixture += f'#[path = "{Path(__file__).resolve().with_name("native-support.rs")}"] mod native_support;\n'
for name in ('g16_mesh', 'g16_render', 'g16_stamp'):
    fixture += f'#[path = "{driver / (name + ".rs")}"] mod {name};\n'

# Extract the production scratch method; the small double records allocations
# and writes so the test checks its actual ownership and clearing behavior.
vm = (driver / 'g16_vm.rs').read_text()
tests = Path(__file__).with_name('native-mesh.rs').read_text()
fixture += tests
fixture = fixture.replace('/* PREPARE_RENDER */', item(vm, r'    pub\(crate\) fn prepare_render\('))

def baseline_render():
    """g16_render.rs as of the parent of the commit that added g16_mesh.rs."""
    try:
        added = subprocess.run(
            ['git', 'log', '--diff-filter=A', '--format=%H', 'HEAD', '--',
             'drivers/gpu/drm/asahi/g16_mesh.rs'],
            cwd=root, text=True, capture_output=True, check=True).stdout.split()
        if not added:
            return None
        return subprocess.run(
            ['git', 'show', added[-1] + '^:drivers/gpu/drm/asahi/g16_render.rs'],
            cwd=root, text=True, capture_output=True, check=True).stdout
    except (OSError, subprocess.CalledProcessError):
        return None


baseline = baseline_render()
if baseline is None:
    print('SKIP: no git history with the commit that added g16_mesh.rs; '
          'the ordinary-render baseline is unavailable')
    sys.exit(4)

with tempfile.TemporaryDirectory(prefix='asahi-native-mesh-') as temp:
    temp = Path(temp)
    (temp / 'baseline.rs').write_text(baseline)
    fixture += f'#[path = "{temp / "baseline.rs"}"] mod baseline;\n'
    (temp / 'test.rs').write_text(fixture)
    subprocess.run([os.environ.get('RUSTC', 'rustc'), '--edition=2021', '--test',
                    '-Dwarnings', str(temp / 'test.rs'), '-o', str(temp / 'test')], check=True)
    subprocess.run([str(temp / 'test'), '--nocapture'], check=True)
