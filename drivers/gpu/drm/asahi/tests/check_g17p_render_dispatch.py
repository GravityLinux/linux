#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exercise the production render parser's dispatch flags with real ABI layouts.

Attachment lists describe memory residency. They cannot prove that a pass has
few primitives, especially when a draw has many instances. No such feature is
advertised by this backend, so every attachment count must use general dispatch.
"""
import argparse
import ctypes
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import host_versions

ap = argparse.ArgumentParser(description=__doc__)
ap.add_argument('bootloader', type=Path)
ap.add_argument('--evidence', type=Path, required=True)
ap.add_argument('--baseline', help='Also require the same test to fail on this revision')
a = ap.parse_args()
tests = Path(__file__).resolve().parent
root = tests.parent
sys.path.insert(0, str(a.bootloader / 'proxyclient'))
from m1n1.g16c import uapi

assert ctypes.sizeof(uapi.drm_asahi_cmd_render) == 240


def function(source, marker):
    start = source.index(marker)
    brace = source.index('{', start)
    depth = 1
    end = brace + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


def rust_types(top):
    done, definitions = set(), []
    def emit(kind):
        if kind in done:
            return
        members = []
        for name, field, *_ in kind._fields_:
            if issubclass(field, ctypes.Structure):
                emit(field)
                ty = field.__name__
            else:
                assert issubclass(field, ctypes._SimpleCData)
                ty = {1: 'u8', 2: 'u16', 4: 'u32', 8: 'u64'}[ctypes.sizeof(field)]
            members.append(f'pub {name}:{ty}')
        done.add(kind)
        definitions.append('#[repr(C)]#[derive(Clone,Copy,Default)]pub struct '
                           + kind.__name__ + '{' + ','.join(members) + '}')
        definitions.append(f'const _:()=assert!(core::mem::size_of::<{kind.__name__}>()=={ctypes.sizeof(kind)});')
        for name, *_ in kind._fields_:
            definitions.append(f'const _:()=assert!(core::mem::offset_of!({kind.__name__},{name})=={getattr(kind, name).offset});')
    emit(top)
    return '#[allow(non_snake_case)]mod uapi{' + '\n'.join(definitions) + '}\n'


prefix = '#![allow(dead_code,non_camel_case_types)]\n'
for name in ('g17p_compute', 'g17p_render'):
    prefix += f'#[path={json.dumps(str(root / (name + ".rs")))}]mod {name};\n'
prefix += 'mod g17p_render_lifecycle{pub const POOL_SLOTS:u32=(u32::BITS-2)/2;}\n'
prefix += 'mod g17p_render_runtime{use super::g17p_render::Parameters;'
prefix += function((root / 'g17p_render_runtime.rs').read_text(),
                   'pub(crate) fn first_parameters()') + '}\n'
prefix += rust_types(uapi.drm_asahi_cmd_render)
prefix += '''
#[derive(Debug,PartialEq)]enum KernelError{Invalid,TooBig,NotFound}
const EINVAL:KernelError=KernelError::Invalid;
const E2BIG:KernelError=KernelError::TooBig;
const ENOENT:KernelError=KernelError::NotFound;
const READ:u32=2;const WRITE:u32=4;const PAGE:u64=0x4000;
const USC_EXEC_BASE:u64=1<<40;
type Result<T>=std::result::Result<T,KernelError>;
struct Vm{kernel_start:u64,kernel_end:u64}
impl Vm{fn cover(&self,address:u64,size:u64,_flags:u32)->Result<()>{
 if address==0||size==0{Err(EINVAL)}else{Ok(())}}}
struct Timestamp{id:u32,size:u64,address:u64}
mod parser{use super::*;struct Parser;
impl Parser{
'''
main = '''
}
pub fn run(){
 let vm=Vm{kernel_start:0x10fc000000,kernel_end:0x1100000000};
 let mut command=uapi::drm_asahi_cmd_render::default();
 command.width_px=1;command.height_px=1;command.layers=1;
 command.samples=1;command.utile_width_px=32;command.utile_height_px=32;
 command.sample_size_B=8;command.vdm_ctrl_stream_base=0x1008000000;
 command.isp_scissor_base=0x2ffff96b00;command.ppp_multisamplectl=0x88;
 command.ppp_ctrl=0x202;
 command.bg.usc=0x100;command.eot.usc=0x200;
 command.partial_bg.usc=0x300;command.partial_eot.usc=0x400;
 let flags_allowed=2|(1<<3)|(1<<18)|(1<<29);
 let mut cases=0;
 for attachments in [0,1,2,16] {
  for flags in [0,2,1<<18,2|(1<<18)] {
   command.flags=flags;
   let bytes=unsafe{std::slice::from_raw_parts(
    std::ptr::from_ref(&command).cast::<u8>(),std::mem::size_of_val(&command))};
   let p=Parser::render_parameters(&vm,&[],bytes,attachments,1024,flags_allowed).unwrap();
   assert_eq!(p.aux_fb_flags,0xc000|u64::from(flags&(1<<18)),
    "aux_fb_flags must use general dispatch for attachment count {}",attachments);
   cases+=1;
  }
 }
 for flags in [1<<3,1<<29,(1<<3)|(1<<29)] {
  command.flags=flags;
  let bytes=unsafe{std::slice::from_raw_parts(
   std::ptr::from_ref(&command).cast::<u8>(),std::mem::size_of_val(&command))};
  let p=Parser::render_parameters(&vm,&[],bytes,0,1024,flags_allowed).unwrap();
  assert!(p.vdm_barrier_fragment,"legacy and downstream barrier flags must agree");
  assert_eq!(p.aux_fb_flags,0xc000);
 }
 for flags in [1,4,1<<30] {
  command.flags=flags;
  let bytes=unsafe{std::slice::from_raw_parts(std::ptr::from_ref(&command).cast::<u8>(),std::mem::size_of_val(&command))};
  assert!(matches!(Parser::render_parameters(&vm,&[],bytes,0,1024,flags_allowed),Err(KernelError::Invalid)));
 }
 println!("PASS {} attachment/flag combinations, 3 barrier compatibility cases and 3 unsupported flags; payload {} bytes",cases,std::mem::size_of_val(&command));
}
}
fn main(){parser::run();}
'''
source = (root / 'g17p_drm.rs').read_text()
report = {'source_sha256': hashlib.sha256(source.encode()).hexdigest(),
          'attachment_flag_cases': 16, 'unsupported_flag_cases': 3,
          'fragment_barrier_compatibility_cases': 3,
          'production_parser_executed': True, 'GPU_execution': False}
with tempfile.TemporaryDirectory() as tmp:
    tmp = Path(tmp)
    flags = host_versions.build(tmp)
    for label, text in [('current', source)] + ([('baseline', subprocess.check_output(
            ['git', '-C', str(root), 'show', a.baseline + ':drivers/gpu/drm/asahi/g17p_drm.rs'],
            text=True))] if a.baseline else []):
        code = prefix + function(text, 'fn render_parameters(') + main
        path = tmp / (label + '.rs')
        path.write_text(code)
        binary = tmp / label
        subprocess.run(['rustc', '--edition=2021', '-Dwarnings', *flags,
                        str(path), '-o', str(binary)], check=True)
        run = subprocess.run([str(binary)], capture_output=True, text=True)
        if label == 'baseline':
            assert run.returncode != 0 and 'aux_fb_flags must use general dispatch' in run.stderr
            report['baseline'] = a.baseline
            report['baseline_same_test_failed'] = True
        else:
            assert run.returncode == 0, run.stderr
            print(run.stdout.strip())
            report['result'] = run.stdout.strip()
a.evidence.mkdir(parents=True, exist_ok=False)
(a.evidence / 'summary.json').write_text(json.dumps(report, indent=2) + '\n')
