#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Compare generated dispatch layouts with whole Max DATA pages and tagged Neo.

This gate uses the production versions macro. Captures supply independent
expected bytes and the two geometry scalars; no captured payload is a template.
It does not contact hardware or qualify execution.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

import host_versions

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1', type=Path)
p.add_argument('--capture', type=Path, action='append', required=True)
p.add_argument('--artifact', type=Path, required=True)
p.add_argument('--baseline', default='neo-known-good-2026-10-21')
a = p.parse_args()
root = Path(__file__).resolve().parents[1]
linux = root.parents[3]
sys.path.insert(0, str(a.m1n1.resolve() / 'proxyclient/experiments'))
from agx_g16c_reference_image import ReferenceImage

cases = []
for capture in a.capture:
    assert json.loads((capture/'capture-validation.json').read_text())['pass']
    image = ReferenceImage(capture/'uat')
    expected = image.virtual(0, 0x7001840000, 0x4000)
    plan = json.loads((capture/'uat/replay-plan.json').read_text())
    contexts = sorted({row['context'] for row in plan['submissions']})
    entries = []
    for context in contexts:
        assert 1 <= context < 64
        offset = (context-1)*32
        tagged, = struct.unpack_from('<Q', expected, offset)
        pages, = struct.unpack_from('<I', expected, offset+16)
        assert tagged >> 56 == 0x40
        directory = (tagged & ~(0x40 << 56)) << 7
        entries.append(dict(context=context, directory=directory, pages=pages))
    cases.append(dict(capture=str(capture), entries=entries, expected=expected))

opening = subprocess.check_output(
    ['git', 'show', a.baseline+':drivers/gpu/drm/asahi/g17p_opening.rs'], cwd=linux)
with tempfile.TemporaryDirectory(prefix='dispatch-layouts-') as tmp:
    tmp = Path(tmp)
    flags = host_versions.build(tmp)
    (tmp/'opening.rs').write_bytes(opening)
    code = '#![allow(dead_code)]\nuse std::io::Write;\n'
    code += '#[path='+json.dumps(str(root/'scheduled_dispatch.rs'))+']mod wire;\n'
    code += '#[path="opening.rs"]mod tagged_neo;\nfn main(){\n'
    code += 'let neo=wire::DispatchStateG17P::default();assert_eq!(*neo.as_bytes(),tagged_neo::dispatch_record());\n'
    code += 'std::io::stdout().write_all(neo.as_bytes()).unwrap();\n'
    code += 'std::io::stdout().write_all(wire::DispatchStateG16C::default().as_bytes()).unwrap();\n'
    for case in cases:
        code += 'let mut page=[0u8;0x4000];\n'
        for entry in case['entries']:
            code += ('let state=wire::DispatchStateG16C::initial('
                     f'{entry["directory"]},{entry["pages"]}).unwrap();'
                     f'let offset=wire::DispatchStateG16C::slot_offset({entry["context"]}).unwrap();'
                     'page[offset..offset+32].copy_from_slice(state.as_bytes());\n')
        code += 'std::io::stdout().write_all(&page).unwrap();\n'
    invalid = [(0, 1), (1, 1), (0x7000000001, 1), (1 << 42, 1),
               (0x7000000000, 0), (0x7000000000, 1 << 31),
               (0x7000000000, (1 << 32)-1)]
    for directory, pages in invalid:
        code += f'assert!(wire::DispatchStateG16C::initial({directory},{pages}).is_none());\n'
        code += f'assert!(wire::DispatchStateG17P::initial({directory},{pages}).is_none());\n'
    code += 'for context in [0,64,65535]{assert!(wire::DispatchStateG16C::slot_offset(context).is_none());}\n'
    code += 'assert_eq!(wire::DispatchStateG16C::slot_offset(63),Some(62*32));\n'
    code += '}\n'
    (tmp/'main.rs').write_text(code)
    subprocess.run(['rustc', '--edition=2021', '-Dwarnings', str(tmp/'main.rs'),
                    *flags, '-o', str(tmp/'test')], check=True)
    raw = subprocess.check_output([str(tmp/'test')])
assert len(raw) == 64+len(cases)*0x4000
assert raw[32:64] == struct.pack('<Q6I', 0x40000000e0000000, 0, 20*4096, 20*2048, 0, 0, 0)
a.artifact.mkdir(parents=True, exist_ok=False)
rows = []
for index, case in enumerate(cases):
    data = raw[64+index*0x4000:64+(index+1)*0x4000]
    name = f'dispatch-{index}.bin'
    (a.artifact/name).write_bytes(data)
    assert data == case['expected'], (case['capture'], 'whole dispatch page mismatch')
    rows.append(dict(capture=case['capture'], entries=case['entries'], file=name, bytes=len(data),
                     sha256=hashlib.sha256(data).hexdigest()))
(a.artifact/'generated-main.rs').write_text(code)
(a.artifact/'Neo-default.bin').write_bytes(raw[:32])
report = dict(Max_whole_pages_exact=len(cases), Neo_tagged_default_exact=True,
              generated_size_alignment_all_field_offsets_asserted=True,
              malformed_inputs_rejected_per_variant=len(invalid),
              source_payload_bytes_copied=False, GPU_execution_tested=False,
              source_sha256=hashlib.sha256((root/'scheduled_dispatch.rs').read_bytes()).hexdigest(),
              versions_macro_sha256=hashlib.sha256((linux/'rust/macros/versions.rs').read_bytes()).hexdigest(),
              cases=rows)
(a.artifact/'summary.json').write_text(json.dumps(report, indent=2)+'\n')
print('PASS:', len(cases), 'whole Max DATA pages; tagged Neo default; both macro layouts and malformed inputs')
