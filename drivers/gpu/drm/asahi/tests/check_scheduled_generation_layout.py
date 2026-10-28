#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare production command-storage geometry with the known-good Neo tag.

Also check all Max retained generations for command-region overlap and native
context-zero alias arithmetic. This is host address evidence, not a GPU test.
"""
from pathlib import Path
import argparse, subprocess, json, hashlib
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--evidence',type=Path,required=True)
args=parser.parse_args()
out=args.evidence.resolve();out.mkdir(parents=True,exist_ok=False)
root=Path(__file__).resolve().parent.parents[4];base='a6883c3c6e5aa42548543a5a7b82bb6e375d6922'
old=subprocess.check_output(['git','show',base+':drivers/gpu/drm/asahi/g17p_compute_queues.rs'],cwd=root,text=True)
new=(root/'drivers/gpu/drm/asahi/scheduled_generation.rs').read_text()
old=old[old.index('const FIRST_ASID:'):old.index('/// Immutable physical-owner/ordinal seed.')]
new=new[new.index('const FIRST_ASID:'):]
# Compile the actual host-only layout implementation, including its bounds.
prefix='type Result<T> = core::result::Result<T,i32>; const EBUSY:i32=16;\n'
fields='''fn words(l:Layout,slot:u32)->[u64;20] { [l.high,l.low,l.grid as u64,l.asid as u64,l.queue(),l.pointers(),l.ring(),l.context(),l.context_low(),l.support(),l.support_state(),l.zero(),l.completion_scratch(slot),l.control(),l.job_list(),l.optional(),l.scheduler(slot),l.scheduler_slot(slot),l.status(slot),l.event(slot)] }
pub fn at(index:usize,asid:u16,slot:u32)->[u64;24] {let l=Layout::new(index,asid).unwrap(); let a=words(l,slot);let mut v=[0;24];v[..20].copy_from_slice(&a);v[20..].copy_from_slice(&[l.dispatch(slot)[0],l.dispatch(slot)[1],l.descriptor(slot),l.descriptor_low(slot)]);v}
'''
code='#![allow(dead_code)]\nmod g17p_compute_lifecycle { pub const TRANSPORT_INTERVAL:u32=128; }\nmod tagged {\n'+prefix+old+fields+'}\nmod current {\n'+prefix+new+fields+'''pub fn check_max() -> usize {
let mut checked=0;
for index in 0..217 {
 let l=Layout::g16c(index).unwrap(); let mut spans=Vec::new();
 for slot in 0..240 {spans.push((l.descriptor(slot),0x1040u64)); assert_eq!(l.descriptor_low(slot),0x7000000000+l.descriptor(slot)-0xfffffc20c0018000);}
 for slot in 0..256 {
  for span in [(l.scheduler(slot),0x100),(l.scheduler_slot(slot),64),(l.status(slot),64),(l.dispatch(slot)[0],64),(l.dispatch(slot)[1],64),(l.event(slot),0x400),(l.completion_scratch(slot),64)] {assert_eq!(span.0 & 63,0);spans.push(span);}
 }
 spans.sort(); for pair in spans.windows(2) {assert!(pair[0].0+pair[0].1<=pair[1].0);}
 assert!(spans.last().unwrap().0+spans.last().unwrap().1<=l.high+0x200000);
 checked+=spans.len();
} checked }
}\nfn main(){let mut compared=0;for index in 0..60 { for asid in 0..64 {assert_eq!(tagged::at(index,asid,0),current::at(index,asid,0));compared+=24;} for slot in 0..512 {assert_eq!(tagged::at(index,4,slot),current::at(index,4,slot));compared+=24;}} println!("{} {}",compared,current::check_max());}\n'''
rs=out/'layout-current-and-tag.rs';rs.write_text(code);binary=out/'layout-current-and-tag';subprocess.run(['rustc','--edition=2021','-O',str(rs),'-o',str(binary)],check=True)
a,b=map(int,subprocess.check_output([str(binary)],text=True).split());assert a==829440 and b==440944,(a,b)
receipt=dict(Neo_tag=base,Neo_address_comparisons=a,Max_cache_line_disjoint_intervals=b,Max_retained_queue_generations=217,actual_production_Layout_sha256=hashlib.sha256(new.encode()).hexdigest(),firmware_ABI_structs_changed=False,hardware_tested=False)
(out/'summary.json').write_text(json.dumps(receipt,indent=2)+'\n');print('PASS',json.dumps(receipt))
