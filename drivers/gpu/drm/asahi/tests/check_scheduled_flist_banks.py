#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Check actual FList/dispatch constructors against complete native DATA.

The input must have passed native output checks and four-phase opaque replay.
No Apple instructions are read or decoded; this test never opens a device.
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
p.add_argument('--capture', type=Path, required=True)
p.add_argument('--replay', type=Path, required=True)
p.add_argument('--evidence', type=Path, required=True)
a = p.parse_args()
sys.path.insert(0, str(a.m1n1 / 'proxyclient/experiments'))
from agx_g16c_reference_image import ReferenceImage

tests = Path(__file__).resolve().parent
replay = json.loads(a.replay.read_text())
assert replay['pass_'] and not replay['actual_result']['hypervisor_used']
assert replay['actual_result']['all6outputs_correct']
assert replay['actual_result']['all18stage_retirements']
seen, rows, lines = set(), [], []
for phase in ('warm-A', 'warm-B', 'warm-C', 'triple'):
    directory = a.capture / phase
    assert json.loads((directory / 'full-capture-validation-v2.json').read_text())['pass']
    assert json.loads((directory / 'native-verification.json').read_text())['pass']
    plan = json.loads((directory / 'uat/replay-plan.json').read_text())
    image = ReferenceImage(directory / 'uat')
    route = struct.unpack_from('<4Q', image.virtual(-1, plan['main'], 0x500), 0x20 + 13 * 32)
    events = image.virtual(-1, route[1], 256 * 72)
    history = json.loads((directory / 'startup-history.json').read_text())
    for entry in history['controls']:
        if entry['kind'] != 0x20 or entry['serial'] in seen:
            continue
        seen.add(entry['serial'])
        raw = bytes.fromhex(entry['message'])
        context, tag, support, table, end, added, bank = (
            struct.unpack_from(fmt, raw, at)[0]
            for fmt, at in (('<I', 4), ('<I', 12), ('<Q', 20), ('<Q', 28),
                            ('<Q', 36), ('<I', 44), ('<I', 48)))
        assert (end - table) % 64 == 0 and added % 8 == 0
        matches = [events[i:i + 72] for i in range(0, len(events), 72)
                   if struct.unpack_from('<2I', events, i) == (13, context)
                   and struct.unpack_from('<Q', events, i + 12)[0] == support]
        assert len(matches) == 1
        reply = matches[0]
        assert struct.unpack_from('<I', reply, 8)[0] == bank
        body = image.virtual(-1, support, 0x64)
        assert struct.unpack_from('<I', body, 8)[0] == bank
        assert struct.unpack_from('<I', body, 0x60)[0] == 4
        directory_va = struct.unpack_from('<Q', body, 0x14)[0]
        dispatch = image.virtual(-1, 0xfffffc2001550000 + bank * 32, 32)
        pages = struct.unpack_from('<I', dispatch, 16)[0]
        assert struct.unpack_from('<Q', dispatch)[0] == 0x4000000000000000 | (directory_va >> 7)
        assert pages == ((end - table) // 64 + added // 8) * 2048
        fields = (f'context:{context},support:{support},table:{table},added:{added // 8},'
                  f'previous:{(end - table) // 64},bank:{bank},tag:{tag}')
        array = lambda data: '[' + ','.join(map(str, data)) + ']'
        lines.append('let r=admission::RegistrationG16C{' + fields + '};'
                     'let(request,mut receipt)=r.prepare().unwrap_or_else(|e|panic!("native bank {} rejected: {e:?}",r.bank));'
                     f'assert_eq!(*request.bytes(),{array(raw)});'
                     f'let reply={array(reply)};let mut wrong=reply;wrong[8]^=1;'
                     'assert!(!receipt.consume(&wrong));assert!(receipt.consume(&reply));assert!(!receipt.consume(&reply));'
                     f'assert_eq!(*dispatch::DispatchStateG16C::initial({directory_va},{pages}).unwrap().as_bytes(),{array(dispatch)});')
        rows.append(dict(phase=phase, context=context, bank=bank, support=support,
                         directory=directory_va, pages=pages, request_sha256=hashlib.sha256(raw).hexdigest(),
                         reply_sha256=hashlib.sha256(reply).hexdigest(), dispatch_sha256=hashlib.sha256(dispatch).hexdigest()))
assert len(rows) == 6 and {row['bank'] for row in rows} == {0, 1, 3, 4, 5}
code = '#![allow(dead_code)]\n#[derive(Debug)]pub enum Error{Invalid,Overflow}\n'
code += f'#[path={json.dumps(str(tests.parent / "scheduled_render_admission.rs"))}]mod admission;\n'
code += f'#[path={json.dumps(str(tests.parent / "scheduled_dispatch.rs"))}]mod dispatch;\n'
code += 'fn main(){' + ''.join(lines) + 'println!("PASS six native FList requests/replies and six generated dispatch records");}\n'
with tempfile.TemporaryDirectory(prefix='native-flist-bank-') as tmp:
    tmp = Path(tmp)
    flags = host_versions.build(tmp)
    (tmp / 'main.rs').write_text(code)
    subprocess.run(['rustc', '--edition=2021', *flags, str(tmp / 'main.rs'), '-o', str(tmp / 'gate')], check=True)
    result = subprocess.check_output([str(tmp / 'gate')], text=True)
    a.evidence.mkdir(parents=True, exist_ok=False)
    (a.evidence / 'harness.rs').write_text(code)
    (a.evidence / 'summary.json').write_text(json.dumps(dict(pass_=True, rows=rows,
        complete_requests_exact=6, complete_dispatch_records_exact=6,
        actual_production_versions_macro=True, wrong_bank_and_duplicate_reply_rejected=True,
        native_opaque_replay_receipt_sha256=hashlib.sha256(a.replay.read_bytes()).hexdigest(),
        source_sha256={name:hashlib.sha256((tests.parent / name).read_bytes()).hexdigest()
                       for name in ('scheduled_render_admission.rs', 'scheduled_dispatch.rs')},
        device_access=False, GPU_execution=False), indent=2) + '\n')
    print(result, end='')
