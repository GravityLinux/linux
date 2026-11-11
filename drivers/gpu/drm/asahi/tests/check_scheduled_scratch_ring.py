#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Check shared scratch rotation against the original Neo and native Max DATA.

Compile the actual lifecycle and Max parameter constructor. Compare complete
Neo descriptors before/after the policy extraction and native Max warm/pair
pointer progression. This does not simulate firmware or prove GPU execution.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import tempfile

import host_versions

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence', type=Path, required=True)
p.add_argument('--native-phases', type=Path, required=True)
a = p.parse_args()
tests = Path(__file__).resolve().parent
root = tests.parents[4]
baseline = 'a6883c3c6e5aa42548543a5a7b82bb6e375d6922'
old = subprocess.check_output([
    'git', 'show', baseline + ':drivers/gpu/drm/asahi/g17p_render_lifecycle.rs'
], cwd=root, text=True)
source = (tests.parent / 'scheduled_render_submit.rs').read_text()
start = source.index('fn geometry_sizes(')
geometry = source[start:source.index('\nimpl State', start)]
start = source.index('    fn configured_render(')
configure = source[start:source.index('    fn render_seed(', start)]
configure = configure.replace('super::super::super::g17p_render_lifecycle', 'g17p_render_lifecycle')
geometry = geometry.replace('super::super::super::g17p_render_lifecycle', 'g17p_render_lifecycle')
captures = {}
native_rows = []
for tag in ('A', 'B'):
    pointers = []
    for phase in ('warm-' + tag, 'pair/' + tag):
        path = a.native_phases / phase / 'stage-1-descriptor.bin'
        data = path.read_bytes()
        assert len(data) >= 0x2240
        pointers.append(struct.unpack_from('<Q', data, 0x40)[0])
        captures[str(path)] = hashlib.sha256(data).hexdigest()
    assert pointers[1] - pointers[0] == 0x1200, pointers
    native_rows.append('(' + ','.join(str(v) for v in pointers) + ')')

with tempfile.TemporaryDirectory(prefix='scheduled-scratch-ring-') as d:
    tmp = Path(d)
    old_path = tmp / 'original_lifecycle.rs'
    old_path.write_text(old)
    code = '#![allow(dead_code)]\n'
    for name in ('g17p_compute', 'g17p_compute_memory', 'g17p_render',
                 'g17p_opening', 'g17p_render_graph', 'g17p_render_lifecycle',
                 'g17p_queue', 'g17p_dependency'):
        code += f'#[path={json.dumps(str(tests.parent / (name + ".rs")))}]mod {name};\n'
    code += f'#[path={json.dumps(str(old_path))}]mod original;\n'
    code += '''
use g17p_render::{self as command,Parameters,Kind};
type Result<T> = core::result::Result<T,i32>;const EINVAL:i32=22;const EOVERFLOW:i32=75;
mod module_parameters {
 pub struct Selector;impl Selector {pub fn value(&self)->u32{u32::MAX}}
 #[allow(non_upper_case_globals)]pub static g16c_parameter_oom_test:Selector=Selector;
}
''' + geometry + configure
    code += '''
fn main(){let mut descriptors=0;let mut rotations=0;
 for (width,height,layers) in [(8,4,1),(128,128,1),(1280,720,1),(37,19,2)] {
  let p=Parameters{width,height,layers,context_base:0x1000000000,tilemap:0x10004a0000,
   heapmeta:0x10004a1000,tpc:0x1000600000,encoder:0x1000200000,
   deflake_1:0x1000068da0,deflake_2:0x1000068120,deflake_3:0x1000068000,
   ta_status:0x1000078000,fragment_status:0x1000490000,aux_fb:0x10004d0000,
   ..Default::default()};
  let (heap,stride,tpc)=p.scratch_layout().unwrap();
  let sizes=geometry_sizes(&p).unwrap();assert_eq!(sizes[0],stride*8);assert_eq!(sizes[1],tpc*4);
  for index in 0..256 {
   let current=g17p_render_lifecycle::Item::retained(index+1,index,g17p_render_lifecycle::ORDINARY).unwrap();
   let saved=original::Item::retained(index+1,index,original::ORDINARY).unwrap();
   assert_eq!(current.tilemap_reset(&p),saved.tilemap_reset(&p));
   let configured=configured_render(&p,index,index,[p.tilemap,p.tpc,0]).unwrap();
   assert_eq!(configured.tilemap,p.tilemap+u64::from(index%8)*stride);
   assert_eq!(configured.heapmeta,configured.tilemap+heap+if layers>1{0x100}else{0});
   assert_eq!(configured.tpc,p.tpc);assert!(configured.heapmeta+0x100<=p.tilemap+sizes[0]);
   for kind in [Kind::Tiling,Kind::Fragment] {
    let mut before=vec![0;kind.size()];let mut after=vec![0;kind.size()];
    saved.descriptor(kind,&p,&mut before).unwrap();current.descriptor(kind,&p,&mut after).unwrap();
    assert_eq!(before,after,"Neo descriptor changed at index {index}");
    if kind==Kind::Fragment{assert_eq!(u64::from_le_bytes(after[0x40..0x48].try_into().unwrap()),configured.tilemap);}
    descriptors+=1;
   }
   rotations+=1;
  }
 }
 for (first,second) in [NATIVE_ROWS] {
  let p=Parameters{width:1,height:1,..Default::default()};
  assert_eq!(configured_render(&p,1,1,[first,0x1000600000,0]).unwrap().tilemap,second);
 }
 println!("{descriptors} {rotations}");
}
'''.replace('NATIVE_ROWS', ','.join(native_rows))
    flags = host_versions.build(tmp)
    main = tmp / 'main.rs'
    main.write_text(code)
    subprocess.run(['rustc', '--edition=2021', *flags, str(main), '-o', str(tmp / 'gate')], check=True)
    result = subprocess.check_output([str(tmp / 'gate')], text=True).strip()
    assert result == '2048 1024', result
    a.evidence.mkdir(parents=True, exist_ok=False)
    (a.evidence / 'harness.rs').write_text(code)
    (a.evidence / 'original_lifecycle.rs').write_text(old)
    (a.evidence / 'summary.json').write_text(json.dumps(dict(
        complete_Neo_descriptor_comparisons=2048, scratch_rotations=1024,
        original_Neo_revision=baseline, native_Max_progressions=2,
        native_reference_sha256=captures, hardware_execution=False,
        production_source_sha256={str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
                                  for path in (tests.parent / 'g17p_render_lifecycle.rs',
                                               tests.parent / 'scheduled_render_submit.rs')},
    ), indent=2) + '\n')
print('PASS 2048 whole Neo descriptors, 1024 scratch selections, 2 native Max progressions')
