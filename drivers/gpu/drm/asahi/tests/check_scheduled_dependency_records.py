#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare shared dependency records to tagged Neo code and native Max DATA.

The tagged constructor is an independent oracle for Neo behavior, including
invalid inputs and preservation of bytes outside the dependency vector. Max
checks use complete output-qualified native records and the ordinary Neo
receiver-last rule with Max's captured timeout/control prefix preserved.
This host gate does not establish GPU execution.
"""

import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess

import host_versions


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1', type=Path)
parser.add_argument('--evidence', required=True, type=Path)
args = parser.parse_args()
tests = Path(__file__).resolve().parent
linux = tests.parents[4]
baseline = 'a6883c3c6e5aa42548543a5a7b82bb6e375d6922'
output = args.evidence.resolve()
output.mkdir(parents=True, exist_ok=False)
legacy = output / 'legacy.rs'
legacy.write_bytes(subprocess.check_output(
    ['git', 'show', baseline + ':drivers/gpu/drm/asahi/g17p_compute.rs'], cwd=linux))
flags = host_versions.build(output)
current = tests.parent / 'g17p_compute.rs'
code = ['#![allow(dead_code)]\n',
        f'#[path={json.dumps(str(legacy))}] mod legacy;\n',
        f'#[path={json.dumps(str(current))}] mod current;\n',
        'use current::work;\nfn main(){\n']
code.append('''
    let mut comparisons=0;
    for own in 0u8..128 {
        let foreign=(own+1)%128;
        let other=(own+2)%128;
        let cases=[vec![],vec![(own,7)],vec![(foreign,1),(own,6)],
            vec![(foreign,3),(other,5),(foreign,9)],
            vec![(foreign,0)],vec![(foreign,1<<30)],vec![(128,1)],
            vec![(foreign,1),(other,2),(own,7),(foreign,3)]];
        for dependencies in cases {
            let mut old=[0xa5u8;512];
            old[0x20..0x28].copy_from_slice(&0xffff082100000001u64.to_le_bytes());
            old[0x28..0x30].copy_from_slice(&((u64::from(own)<<40)|7).to_le_bytes());
            let mut new=old;
            let a=legacy::context_dependencies(&mut old,&dependencies);
            let b=current::context_dependencies(&mut new,&dependencies);
            assert_eq!(format!("{:?}",a),format!("{:?}",b));
            assert_eq!(old,new);comparisons+=1;
        }
        // An invalid receiver must fail without destroying the caller tail.
        let mut old=[0x5au8;512];
        old[0x28..0x30].copy_from_slice(&((255u64<<40)|7).to_le_bytes());
        let mut new=old;
        assert!(legacy::context_dependencies(&mut old,&[(foreign,1)]).is_err());
        assert!(current::context_dependencies(&mut new,&[(foreign,1)]).is_err());
        assert_eq!(old,new);comparisons+=1;
    }
    for length in [0,511,513] {
        let mut old=vec![0xa5;length];let mut new=old.clone();
        for deps in [vec![],vec![(1,1)]] {
            let a=legacy::context_dependencies(&mut old,&deps);
            let b=current::context_dependencies(&mut new,&deps);
            assert_eq!(format!("{:?}",a),format!("{:?}",b));assert_eq!(old,new);
            comparisons+=1;
        }
    }
    // Reverse grid order distinguishes receiver-last from sorting all grids.
    for own in 0u8..4 {
        let mut raw=work::WorkRecordG16C::new(&work::RecordParametersG16C{
            descriptor:0xfffffc20c0018000,queue:0xfffffc20c0000000,
            grid:own,item:7,control:0x0821,retire_control:2,..Default::default()
        }).unwrap();
        let deps=[(((own+1)%4)as u8,9),(((own+2)%4)as u8,8),(own as u8,7)];
        let mut expected=raw.as_bytes().to_vec();
        let prefix=u64::from_le_bytes(expected[0x20..0x28].try_into().unwrap())&!0xffffffff;
        legacy::context_dependencies(&mut expected,&deps).unwrap();
        let mask=u64::from_le_bytes(expected[0x20..0x28].try_into().unwrap())&0xffffffff;
        expected[0x20..0x28].copy_from_slice(&(prefix|mask).to_le_bytes());
        raw.append_dependencies(&deps).unwrap();assert_eq!(raw.as_bytes(),expected);
    }
    println!("TAGGED_NEO_COMPARISONS={}",comparisons);
''')

references = [
    (args.m1n1 / 'build/m4max-gpu/single-user-compute-four-queues-20261010', 4),
    (args.m1n1 / 'build/m4max-gpu/g16c-native-queued-dependencies-20261022/single-user-same-queue-v2', 2),
    (args.m1n1 / 'build/m4max-gpu/g16c-native-queued-dependencies-20261022/single-user-long-pair-v3', 2),
]
native = []
for directory, count in references:
    verification = json.loads((directory / 'native-verification.json').read_text())
    assert verification['pass'] and verification['output_expected'] and verification['output_changed']
    for index in range(count):
        path = directory / f'submission-{index}-record.bin'
        data = path.read_bytes()
        assert len(data) == 512
        qword = lambda at: struct.unpack_from('<Q', data, at)[0]
        header = qword(0)
        dependency_header = qword(0x20)
        mask = dependency_header & 0xffffffff
        assert 1 <= mask.bit_count() <= 4 and mask == (1 << mask.bit_count()) - 1
        points = [qword(0x28 + i * 8) for i in range(mask.bit_count())]
        own_grid = (header >> 42) & 127
        own_count = ((header & 0xfffffffc) >> 2) - 1
        assert points[-1] == own_grid << 40 | own_count
        ready = struct.unpack_from('<I', data, 0x144)[0]
        assert ready in (0, 0x40000000)
        values = dict(descriptor=qword(0x10), queue=qword(0x18), grid=own_grid,
                      item=own_count, primary_count=(qword(0x150) >> 40) // 8,
                      timeout=dependency_header >> 48, ready=str(bool(ready)).lower(),
                      control=(dependency_header >> 32) & 65535,
                      control_130=qword(0x130), retire_control=qword(0x138), flags=header & (1 << 60))
        fields = ','.join(f'{k}:{v}' for k, v in values.items())
        point_args = ','.join(f'({value >> 40},{value & 0xffffffff})' for value in points)
        code.append('let mut raw=work::WorkRecordG16C::new(&work::RecordParametersG16C{'
                    + fields + ',..Default::default()}).unwrap();\n')
        code.append(f'raw.append_dependencies(&[{point_args}]).unwrap();\n')
        code.append('assert_eq!(raw.as_bytes(),&[' + ','.join(map(str, data)) + ']);\n')
        native.append(dict(path=str(path), sha256=hashlib.sha256(data).hexdigest(),
                           bytes=len(data), receiver_last=True))
code.append('println!("PASS native Max whole dependency records");}\n')
harness = output / 'main.rs'
harness.write_text(''.join(code))
binary = output / 'gate'
subprocess.run(['rustc', '--edition=2021', '-Dwarnings', str(harness), *flags,
                '-o', str(binary)], check=True)
result = subprocess.check_output([str(binary)], text=True)
assert 'TAGGED_NEO_COMPARISONS=1158' in result
proof = dict(tagged_Neo=baseline, Neo_comparisons=1158, reverse_grid_Max_cases=4,
             native_Max_records=native, all_bytes_match=True,
             hardware_execution_tested=False,
             source_sha256={str(p): hashlib.sha256(p.read_bytes()).hexdigest()
                            for p in [current, tests.parent / 'scheduled_work.rs',
                                      linux / 'rust/macros/versions.rs']})
(output / 'summary.json').write_text(json.dumps(proof, indent=2) + '\n')
print(result, end='')
