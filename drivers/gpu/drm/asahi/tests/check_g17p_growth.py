#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Differential protocol checks against source g17p_growth.py, no hardware."""
import argparse
import importlib.util
from pathlib import Path
import struct
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1', type=Path)
parser.add_argument('--rustc', default='rustc')
args = parser.parse_args()
spec = importlib.util.spec_from_file_location('source_growth', args.m1n1 / 'proxyclient/m1n1/agx/g17p_growth.py')
source = importlib.util.module_from_spec(spec)
spec.loader.exec_module(source)
inputs = bytearray()
expected = bytearray()
cases = 0

def emit(op, vm, pool, counter, body, result):
    global cases
    inputs.extend(struct.pack('<5I', op, vm, pool, counter, len(body)) + body)
    expected.extend(result)
    cases += 1

def request(vm, pool, counter, body):
    try:
        a = source.growth_ack(body, counter, pool_id=pool, vm_id=vm)
        b = source.growth_failure_ack(body, counter, pool_id=pool, vm_id=vm)
        result = b'\1' + a + b
    except ValueError:
        result = bytes(129)
    emit(0, vm, pool, counter, body, result)

for vm, pool in ((1, 0), (1, 1), (2, 1), (0, 0), (2, 0), (1, 2)):
    for counter in (0, 1, 31, 32, 512, 0xffffffff):
        for slot, mask in ((0, 1), (2, 4), (1, 1), (0, 4)):
            body = struct.pack('<5I', 6, vm, pool, counter, slot) + bytes(36) + struct.pack('<2Q', 1, mask)
            request(vm, pool, counter, body)
            for bit in range(72 * 8):
                bad = bytearray(body)
                bad[bit // 8] ^= 1 << (bit % 8)
                request(vm, pool, counter, bad)
            for length in (0, 4, 16, 56, 64, 71, 73, 80):
                request(vm, pool, counter, (body + bytes(8))[:length])

work = (0xfffffc2000000100, 0xfffffc2000000120)
fragment = 0xfffffc20c00b0000
for vm, pool in ((1, 0), (1, 1), (2, 1), (0, 0), (2, 0)):
    for node in (*work, 0xfffffc2000000140):
        body = struct.pack('<4I7Q', 7, 0, 1, 1, node, 0x1122334455667788, 0, fragment, 1, vm, pool)
        variants = [body]
        for bit in range(72 * 8):
            bad = bytearray(body); bad[bit // 8] ^= 1 << (bit % 8); variants.append(bad)
        for variant in variants:
            try:
                row = source.growth_limit_report(variant, work_dva=work, fragment_dva=fragment, event_slot=1, vm_id=vm, pool_id=pool)
                result = struct.pack('<BQ', 1, row['callback_cookie'])
            except ValueError:
                result = bytes(9)
            emit(1, vm, pool, 0, variant, result)

fatal = struct.pack('<I', 4) + bytes(68)
for bit in range(-1, 72 * 8):
    body = bytearray(fatal)
    if bit >= 0: body[bit // 8] ^= 1 << (bit % 8)
    try:
        source.fatal_fault_report(body); result = b'\1'
    except ValueError:
        result = b'\0'
    emit(2, 0, 0, 0, body, result)

for old_count in (0, 8, 18, 328):
    old = tuple(source.BASE + (0x11 + i * 5) * source.UNIT for i in range(old_count))
    for new in ((0x1002000000,), (0x1002000000, 0x1002028000), (), old[:1], (0, 1),
                (source.BASE + 1,), (1 << 42,), ((1 << 42) - source.BLOCK,),
                (0x1002000000, 0x1002010000)):
        try:
            # Each list is independently valid, then the combined list must
            # reject overlaps with all retained blocks as the source does.
            source.backing_payload(old_count, new)
            source.backing_payload(0, old + new)
            result = b'\1'
        except ValueError:
            result = b'\0'
        emit(3, old_count, len(new), 0, struct.pack('<%dQ' % (len(old) + len(new)), *old, *new), result)

candidate = Path(__file__).resolve().parents[1] / 'g17p_growth.rs'
rust = '''#![allow(dead_code)]
use std::io::{Read, Write};
#[path="CANDIDATE"] mod g;
fn u32_at(b:&[u8],i:usize)->u32 {u32::from_le_bytes(b[i..i+4].try_into().unwrap())}
fn main(){
 let mut input=Vec::new(); std::io::stdin().read_to_end(&mut input).unwrap();
 let mut out=Vec::new(); let mut at=0;
 while at<input.len(){
  let h=&input[at..at+20];let op=u32_at(h,0);let vm=u32_at(h,4);let pool=u32_at(h,8);let counter=u32_at(h,12);let len=u32_at(h,16) as usize;
  at+=20;let body=&input[at..at+len];at+=len;let owner=g::Owner{vm,pool};
  match op {
   0=>{out.push(u8::from(owner.request(body,counter)));out.extend_from_slice(&owner.reply(body,counter,true).unwrap_or([0;64]));out.extend_from_slice(&owner.reply(body,counter,false).unwrap_or([0;64]));},
   1=>{let r=owner.limit(body,&[0xfffffc2000000100,0xfffffc2000000120],0xfffffc20c00b0000,1);out.push(u8::from(r.is_some()));out.extend_from_slice(&r.unwrap_or(0).to_le_bytes());},
   2=>out.push(u8::from(g::fatal(body))),
   3=>{let list:Vec<u64>=body.chunks_exact(8).map(|v|u64::from_le_bytes(v.try_into().unwrap())).collect();out.push(u8::from(g::block_list(&list[..vm as usize],&list[vm as usize..])));},
   _=>panic!("bad op")
  }
 }
 for counter in 0..32 {let a=g::block_addresses(counter).unwrap();for (i,&va) in a.iter().enumerate(){assert_eq!(va,0x1002000000+(counter as u64*10+i as u64)*0x28000);assert!(g::block_id(va).is_some());}assert!(g::block_list(&[],&a));}
 assert!(g::block_addresses(32).is_none());
 std::io::stdout().write_all(&out).unwrap();
}
'''.replace('CANDIDATE', str(candidate))
with tempfile.TemporaryDirectory() as tmp:
    path = Path(tmp)
    (path / 'check.rs').write_text(rust)
    subprocess.run([args.rustc, '--edition=2021', '-Dwarnings', str(path / 'check.rs'), '-o', str(path / 'check')], check=True)
    actual = subprocess.run([str(path / 'check')], input=inputs, stdout=subprocess.PIPE, check=True).stdout
assert actual == expected, (len(actual), len(expected), next((i for i, (a, b) in enumerate(zip(actual, expected)) if a != b), None))
print(f'PASS: {cases} growth identities, replies, limit/fatal reports and retained block lists; {len(expected)} compared bytes, all request/report bit mutations, malformed lengths and 32 bounded allocations')
