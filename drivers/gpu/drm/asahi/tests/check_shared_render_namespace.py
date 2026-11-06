#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Run actual Neo render serializers against the pre-split byte baseline."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import host_versions

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence', type=Path, required=True)
a = p.parse_args()
a.evidence.mkdir(parents=True, exist_ok=False)
tests = Path(__file__).resolve().parent
root = tests.parent
baseline = '70790a6c02ac'
old = subprocess.check_output(['git', 'show', baseline + ':drivers/gpu/drm/asahi/g17p_render_lifecycle.rs'],
                              cwd=root, text=True)
old_path = a.evidence / 'baseline-lifecycle.rs'
old_path.write_text(old)
runner = (tests / 'g17p_render_lifecycle.rs').read_text()
runner = re.sub(r'#\[path = "\.\./([^"]+)"\]',
                lambda m: '#[path = ' + json.dumps(str(root / m[1])) + ']', runner)
extra = r'''
fn check_namespace() {
    for pool in 0..15u32 {
        let layout=if pool==0 {life::ORDINARY} else if pool==1 {life::SECOND}
            else {life::extra_pool(pool).unwrap()};
        assert_eq!(u32::from(layout.event_slot),pool);
        let old=life::Item::retained(3,7,layout).unwrap();
        let new=life::Item {layout:life::Layout {event_slot:((pool+1)%15)as u8,..layout},..old};
        let p=Parameters {width:64,height:32,context_base:0x1000000000,
            tilemap:0x10001b0000,tpc:0x10001d8000,heapmeta:0x10001b1000,
            deflake_1:0x10000682a0,deflake_2:0x1000068020,deflake_3:0x1000068000,
            ta_status:0x1000078000,fragment_status:0x10001a8000,aux_fb:0x10000300000,
            ..Default::default()};
        assert_eq!(old.parameters(&p).tvb_pool_id,new.parameters(&p).tvb_pool_id);
        for kind in [Kind::Tiling,Kind::Fragment] {
            let mut before=[0u8;0x180];let mut after=before;
            old.context(kind,&p,&mut before).unwrap();new.context(kind,&p,&mut after).unwrap();
            assert_eq!(before[0x24],layout.event_slot);
            before[0x24]=new.layout.event_slot;
            assert_eq!(before,after,"only the queue event namespace changes: pool={pool}");
        }
    }
}
'''
flags = host_versions.build(a.evidence)
outputs = []
for name in ['baseline', 'current']:
    code = runner
    if name == 'baseline':
        code = code.replace(json.dumps(str(root / 'g17p_render_lifecycle.rs')), json.dumps(str(old_path)))
    else:
        code = code.replace('fn main() {', 'fn main() { check_namespace();', 1) + extra
    source = a.evidence / (name + '.rs')
    source.write_text(code)
    binary = a.evidence / name
    subprocess.run(['rustc', '--edition=2021', '-Dwarnings', *flags, str(source), '-o', str(binary)], check=True)
    outputs.append(subprocess.check_output([str(binary)]))
assert outputs[0] == outputs[1], 'Existing Neo wire output changed'
(a.evidence / 'summary.json').write_text(json.dumps(dict(
    baseline=baseline, bytes_compared=len(outputs[0]),
    sha256=hashlib.sha256(outputs[0]).hexdigest(),
    independent_event_pool_cases=30, GPU_execution=False), indent=2) + '\n')
print('PASS: existing Neo render output byte-identical; 30 event/pool separation cases')
