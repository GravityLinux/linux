#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Whole scheduled compute pages vs qualified Max source and tagged Neo Rust.

Only descriptor/register DATA is read from native captures. No shader or Apple
firmware contents are imported. Host comparisons do not qualify GPU execution.
"""
import argparse
from dataclasses import replace
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

import host_versions

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1', type=Path)
parser.add_argument('--evidence', type=Path, required=True)
parser.add_argument('--native-pair', type=Path, help='additional output-qualified two-publication descriptor DATA')
args = parser.parse_args()
tests = Path(__file__).resolve().parent
linux = tests.parents[4]
baseline = 'a6883c3c6e5aa42548543a5a7b82bb6e375d6922'
sys.path.insert(0, str(args.m1n1 / 'proxyclient'))
from m1n1.g16c import compute as ref

cases = []
original_registers = ref.registers

def emit(name, p, s, pairs=None, expected=None):
    pairs = original_registers(p) if pairs is None else pairs
    ref.registers = lambda _: pairs
    try:
        encoded = ref.descriptor(p, s)
    finally:
        ref.registers = original_registers
    if expected is not None:
        assert encoded[:len(expected)] == expected, name
    words = [s.submit_sequence, p.context, s.scheduler, s.alias, p.cdm_terminator,
             p.sampler_array, p.sampler_count, s.driver_stamp, s.firmware_stamp,
             s.grid, s.queue_submission, s.queue_ordinal, s.submission_index,
             s.work_ordinal, s.timestamp_start, s.timestamp_end,
             s.user_timestamp_start, s.user_timestamp_end, s.support,
             s.support_control, s.support_flags, s.support_kind,
             s.support_phase, s.zero_page, p.save_area, p.cdm, p.dispatch_identity]
    wire = struct.pack('<27QI', *words, len(pairs))
    wire += b''.join(struct.pack('<IQ', *v) for v in pairs)
    cases.append((name, wire, encoded))

for i in range(96):
    n = (0, 1, 3, 255, 256, 1024, 0xffffffff)[i % 7]
    p = ref.Parameters(0x10000200000 + i * 0x4000,
        0x10003000000 + i * 0x8000, 0x10003000100 + i * 0x8000,
        0x0200034503000346 + i, i % 63 + 1, i % 256,
        0x10004000000, 0x7000220000, execution_gate=i % 3,
        sampler_array=0 if i % 2 == 0 else 0x10000800000 + i * 8,
        sampler_count=0 if i % 2 == 0 else (1, 65535, 65537, 500000)[i % 4],
        usc_exec_base=None if i % 3 == 0 else 1 << 40,
        register_a599=0x13200400020 + i)
    s = ref.State(0xfffffc20c0900100, 0x7002000000 + i * 0x4000,
        i << 40 | i, i % 128, 0xfffffc20001c8028 + i * 8,
        0xfffffc20c07c0028 + i * 8, 0xfffffc2000024c68 + i * 8,
        0xfffffc2000024c70 + i * 8, 0xfffffc20c0998000,
        0xfffffc2001710000 + (i % 256), 0x21000001 + i, i % 4,
        i * 257, i % 256, i + 1, n, i * 3 + 1, n,
        i * 0x4000, 0x7000500000 + i * 8)
    emit(f'Max-{i}', p, s)
    # Explicitly exercise Max's LAST duplicate policy and fixed control word.
    pairs = original_registers(p) + ((0x1a540, 0xdeadbeef00000000 + i),
        (0x1a440, 0xbad0), (0x10099, 0x7000000000 + i))
    emit(f'Max-duplicates-{i}', p, s, pairs)
    if i == 0:
        # The 24-bit queue submission rolls over independently of the full
        # descriptor submission identity; do not introduce an artificial cap.
        emit('Max-queue-submission-wrap', p, replace(s,
             queue_submission=0, submission_index=1 << 24))

# Native ready protocol DATA gives an independent full-byte layout gate.
native_dir = args.m1n1 / 'build/m4max-gpu/uat-metal-before-producer'
native_paths = [native_dir / 'compute-2-descriptor.bin',
    args.m1n1 / 'build/m4max-gpu/python-native-compute-inputs/item-1-descriptor.bin']
if args.native_pair:
    native=json.loads((args.native_pair/'native-verification.json').read_text())
    validation=json.loads((args.native_pair/'capture-validation.json').read_text())
    assert native['pass'] and native['output_expected'] and native['output_changed'] and validation['pass']
    native_paths.extend(args.native_pair/f'submission-{i}-descriptor.bin' for i in range(2))
native_hashes = {}
for path in native_paths:
    body = path.read_bytes()
    q = lambda at: struct.unpack_from('<Q', body, at)[0]
    w = lambda at: struct.unpack_from('<I', body, at)[0]
    count = struct.unpack_from('<H', body, 0x1e)[0]
    pairs = tuple(struct.unpack_from('<IQ', body, 0x40 + i * 12) for i in range(count))
    values = dict(pairs)
    p = ref.Parameters(values[0x1a510], values[0x1a420], q(0xee0),
        values[0x1a540], w(0xc), values[0x10201] & 255,
        values[0x14070] & ~1, values[0xa5c1] - 5,
        execution_gate=values[0x14028], sampler_array=q(0xf2c),
        sampler_count=w(0xf34), register_a599=next(v for n, v in pairs if n == 0xa599),
        usc_exec_base=values.get(0x10071), helper_binary=values.get(0x11841,0),
        helper_data=values.get(0x11849,0), helper_config=values.get(0x11f81,0))
    assert original_registers(p) == pairs
    s = ref.State(q(0x10), q(0x740) - 0x40, q(4), w(0xf54),
        q(0xf40), q(0xf48), q(0xf7c), q(0xf84), q(0xfb2), q(0xfcb),
        w(0xfba), w(0xfbe), struct.unpack_from('<H', body, 0xfc5)[0],
        body[0xfd3], w(0xf50) >> 8, w(0xf58), w(0xf60), w(0xf70),
        q(0xf8c), q(0xf94))
    emit('native-' + path.parent.name + '-' + path.stem, p, s, expected=body)
    native_hashes[str(path)] = hashlib.sha256(body).hexdigest()

with tempfile.TemporaryDirectory(prefix='scheduled-compute-') as directory:
    tmp = Path(directory)
    flags = host_versions.build(tmp)
    legacy = tmp / 'legacy.rs'
    legacy.write_bytes(subprocess.check_output(['git', 'show',
        baseline + ':drivers/gpu/drm/asahi/g17p_compute.rs'], cwd=linux))
    harness = tmp / 'main.rs'
    harness.write_text('''#![allow(dead_code)]
use std::io::{Read,Write};
#[path=LEGACY] mod legacy;
#[path=CURRENT] mod current;
use current::wire::{ComputeDescriptorG16C as Max,ComputeProgramG16C as MaxProgram,ProgramParameters,Parameters,Error};
fn main() {
    // Original tagged Neo builders are compiled independently in this crate.
    for i in 0u32..256 {
        let mut regs=legacy::Program {preempt:0x10000200000+i as u64*0x4000,
            cdm:0x10003000000,identity:0x0200034503000346+i as u64,
            context:i%64,ordinal:i*1234567,robustness:0x10004000000,
            operand_state:0x7000220000,usc_exec_base:1<<40,
            helper_binary:5,helper_data:0x10000300000,helper_cfg:(i as u64)<<16,
            execution_gate:i as u64%3}.build().unwrap().to_vec();
        let program=current::Program {preempt:0x10000200000+i as u64*0x4000,
            cdm:0x10003000000,identity:0x0200034503000346+i as u64,
            context:i%64,ordinal:i*1234567,robustness:0x10004000000,
            operand_state:0x7000220000,usc_exec_base:1<<40,
            helper_binary:5,helper_data:0x10000300000,helper_cfg:(i as u64)<<16,
            execution_gate:i as u64%3};
        assert_eq!(&program.build().unwrap(),regs.as_slice());
        let mut invalid=program;invalid.usc_exec_base=0;
        assert_eq!(invalid.build(),Err(current::Error::UnsupportedExecBase));
        invalid.usc_exec_base=1<<40;invalid.preempt=u64::MAX;
        assert_eq!(invalid.build(),Err(current::Error::Overflow));
        if i%3==1 { regs.extend([(0x1a440,0xdeadbeef),(0x1a540,0xfeedface)]); }
        if i%3==2 { regs.resize(128,(0xa5c1,0x1234)); }
        let old=legacy::Descriptor {scheduler:0xfffffc20c0900100,
            low_alias:0x7002000000+i as u64*0x4000,cdm_terminator:0x10003000100,
            sequence:(i as u64)<<40|i as u64,context:i%64,grid:i%12,
            dispatch:[0xfffffc20001c8028,0xfffffc20c07c0028],
            status:[0xfffffc2000024c68,0xfffffc2000024c70],
            timestamps:[i as u64*0x4000,0x7000500000+i as u64*8],
            shared_control:0xfffffc20c0998000,zero_page:0xfffffc2001710000+i as u64,
            support_control:0x21000001+i,support_flags:i%4,ordinal:i*1234567,
            queue_submission:i+1,queue_ordinal:i*1234567,submission_index:i*3+1,
            sampler_array:if i%2==0 {0} else {0x10000800000+i as u64*8},
            sampler_count:if i%2==0 {0} else {65537+i}};
        let new=current::Descriptor {scheduler:old.scheduler,low_alias:old.low_alias,
            cdm_terminator:old.cdm_terminator,sequence:old.sequence,context:old.context,
            grid:old.grid,dispatch:old.dispatch,status:old.status,timestamps:old.timestamps,
            shared_control:old.shared_control,zero_page:old.zero_page,
            support_control:old.support_control,support_flags:old.support_flags,
            ordinal:old.ordinal,queue_submission:old.queue_submission,
            queue_ordinal:old.queue_ordinal,submission_index:old.submission_index,
            sampler_array:old.sampler_array,sampler_count:old.sampler_count};
        let mut a=vec![0xa5;0x4000];let mut b=vec![0x5a;0x4000];
        old.build(&mut a,&regs).unwrap();new.build(&mut b,&regs).unwrap();assert_eq!(a,b);
        std::io::stdout().write_all(&b).unwrap();
        let mut invalid=new;invalid.low_alias=u64::MAX;
        assert_eq!(invalid.build(&mut b,&regs),Err(current::Error::Overflow));assert_eq!(a,b);
        invalid.low_alias=old.low_alias;
        for (queue_submission,sampler_array,sampler_count,submission_index) in [
            (0,0,0,1),(1<<24,0,0,1),(1,0,u32::MAX,1),(1,8,0,1),
            (1,0,1,1),(1,9,1,1),(1,0,0,0)] {
            invalid.queue_submission=queue_submission;invalid.sampler_array=sampler_array;
            invalid.sampler_count=sampler_count;invalid.submission_index=submission_index;
            assert_eq!(invalid.build(&mut b,&regs),Err(current::Error::Invalid));assert_eq!(a,b);
        }
        invalid.queue_submission=1;invalid.sampler_array=0;invalid.sampler_count=0;invalid.submission_index=1;
        assert_eq!(invalid.build(&mut b,&regs[..5]),Err(current::Error::MissingRegister));assert_eq!(a,b);
    }
    let mut input=Vec::new();std::io::stdin().read_to_end(&mut input).unwrap();
    let mut input=input.as_slice();
    while !input.is_empty() {
        let mut words=[0u64;27];for v in &mut words {
            *v=u64::from_le_bytes(input[..8].try_into().unwrap());input=&input[8..];
        }
        let n=u32::from_le_bytes(input[..4].try_into().unwrap()) as usize;input=&input[4..];
        let mut regs=Vec::new();for _ in 0..n {
            regs.push((u32::from_le_bytes(input[..4].try_into().unwrap()),
                u64::from_le_bytes(input[4..12].try_into().unwrap())));input=&input[12..];
        }
        let first=|number| regs.iter().find(|(n,_)|*n==number).map(|(_,v)|*v);
        let program=ProgramParameters {save_area:words[24],cdm:words[25],identity:words[26],
            context:words[1] as u32,ordinal:(first(0x10201).unwrap()&255) as u32,
            robustness:first(0x14070).unwrap()&!1,operand_state:first(0xa5c1).unwrap()-5,
            usc_exec_base:first(0x10071),helper_binary:first(0x11841).unwrap_or(0),
            helper_data:first(0x11849).unwrap_or(0),helper_config:first(0x11f81).unwrap_or(0),
            execution_gate:first(0x14028).unwrap(),register_a599:first(0xa599).unwrap()};
        let encoded=MaxProgram::new(&program).unwrap();
        assert_eq!(encoded.registers(),&regs[..encoded.registers().len()]);
        let mut invalid=program;invalid.usc_exec_base=Some((1<<40)+0x4000);
        assert!(matches!(MaxProgram::new(&invalid),Err(Error::UnsupportedExecBase)));
        invalid.usc_exec_base=None;invalid.helper_binary=1;
        assert!(matches!(MaxProgram::new(&invalid),Err(Error::Invalid)));
        let p=Parameters {save_area:words[24],cdm:words[25],identity:words[26],sequence:words[0],context:words[1] as u32,scheduler:words[2],
            alias:words[3],terminator:words[4],sampler_array:words[5],sampler_count:words[6] as u32,
            dispatch:[words[7],words[8]],grid:words[9] as u32,queue_submission:words[10] as u32,
            queue_ordinal:words[11] as u32,submission_index:words[12] as u32,ordinal:words[13] as u32,
            status:[words[14],words[15]],timestamps:[words[16],words[17]],support:words[18],
            support_control:words[19] as u32,support_flags:words[20] as u32,
            support_kind:words[21] as u16,support_phase:words[22] as u8,zero_page:words[23]};
        let mut guarded=vec![0xa5;0x4020];
        Max::write(&mut guarded[16..0x4010],&p,&regs).unwrap();
        assert_eq!(&guarded[..16],&[0xa5;16]);assert_eq!(&guarded[0x4010..],&[0xa5;16]);
        std::io::stdout().write_all(&guarded[16..0x4010]).unwrap();
        let saved=guarded.clone();
        assert_eq!(Max::write(&mut guarded[16..0x4010],&p,&[]),Err(Error::Invalid));assert_eq!(saved,guarded);
        // Mutate one input at a time; every failure must preserve all staging.
        let mut invalid=p;let good=invalid.context;invalid.context=64;
        assert_eq!(Max::write(&mut guarded[16..0x4010],&invalid,&regs),Err(Error::Invalid));assert_eq!(saved,guarded);
        invalid.context=good;
        let good=invalid.alias;invalid.alias=u64::MAX-7;
        assert_eq!(Max::write(&mut guarded[16..0x4010],&invalid,&regs),Err(Error::Overflow));assert_eq!(saved,guarded);
        invalid.alias=good;
        let good=invalid.submission_index;invalid.submission_index=0;
        assert_eq!(Max::write(&mut guarded[16..0x4010],&invalid,&regs),Err(Error::Invalid));assert_eq!(saved,guarded);
        invalid.submission_index=good;
        let good=invalid.sampler_count;invalid.sampler_count=u32::MAX;
        assert_eq!(Max::write(&mut guarded[16..0x4010],&invalid,&regs),Err(Error::Invalid));assert_eq!(saved,guarded);
        invalid.sampler_count=good;
        let good=invalid.grid;invalid.grid=128;
        assert_eq!(Max::write(&mut guarded[16..0x4010],&invalid,&regs),Err(Error::Invalid));assert_eq!(saved,guarded);
        invalid.grid=good;
        let good=invalid.terminator;invalid.terminator=invalid.cdm-4;
        assert_eq!(Max::write(&mut guarded[16..0x4010],&invalid,&regs),Err(Error::Invalid));assert_eq!(saved,guarded);
        invalid.terminator=good;
        assert_eq!(Max::write(&mut guarded[16..0x4010],&invalid,&regs[..5]),Err(Error::MissingRegister));assert_eq!(saved,guarded);
        let many=vec![(0,0);129];
        assert_eq!(Max::write(&mut guarded[16..0x4010],&invalid,&many),Err(Error::Invalid));assert_eq!(saved,guarded);
        assert_eq!(Max::write(&mut guarded[16..0x100f],&invalid,&regs),Err(Error::Invalid));assert_eq!(saved,guarded);
        // The same generated layout also writes an owned packed 4 KiB object.
        let mut packed=vec![0xa5;0x1000];Max::write(&mut packed,&invalid,&regs).unwrap();
        assert_eq!(&packed,&guarded[16..0x1010]);
    }
}
'''.replace('LEGACY', json.dumps(str(legacy))).replace('CURRENT',
        json.dumps(str(tests.parent / 'g17p_compute.rs'))))
    binary = tmp / 'gate'
    subprocess.run(['rustc', '--edition=2021', '-Dwarnings', str(harness),
                    *flags, '-o', str(binary)], check=True)
    actual = subprocess.check_output([str(binary)], input=b''.join(c[1] for c in cases))
    neo = actual[:256 * 0x4000]
    actual = actual[len(neo):]
    assert len(actual) == len(cases) * 0x4000
    for i, (name, _, expected) in enumerate(cases):
        got = actual[i * 0x4000:(i + 1) * 0x4000]
        assert got == expected, (name, [hex(j) for j, (a,b) in enumerate(zip(got, expected)) if a != b][:16])
    args.evidence.mkdir(parents=True, exist_ok=False)
    summary = dict(baseline=baseline, Neo_identical_whole_pages=256,
        Max_whole_source_pages=len(cases), Max_programs_compared=len(cases), Neo_programs_compared=256, native_Max_DATA_images=len(native_paths),
        native_DATA_sha256=native_hashes, wire_bytes=0x1000, standalone_allocation_bytes=0x4000,
        packed_overlap_preserved=True, duplicate_selection_policies_preserved=True,
        GPU_execution_tested=False, Neo_sha256=hashlib.sha256(neo).hexdigest(),
        Max_sha256=hashlib.sha256(actual).hexdigest(),
        source_sha256={str(p):hashlib.sha256(p.read_bytes()).hexdigest()
            for p in [tests.parent/'scheduled_compute.rs',tests.parent/'g17p_compute.rs',
                      args.m1n1/'proxyclient/m1n1/g16c/compute.py']})
    (args.evidence / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print(f'PASS: {len(cases)} whole Max pages including {len(native_hashes)} native DATA images; 256 whole Neo pages equal original tag')
