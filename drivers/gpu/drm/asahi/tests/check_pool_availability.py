#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare actual batched pool classification with the original per-pool gate."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence', required=True, type=Path)
a = p.parse_args()
r = Path(__file__).resolve().parents[1]
source = (r / 'g17p_boot.rs').read_text()
wave = (r / 'g17p_render_wave.rs').read_text()


def item(s, marker):
    i = s.index(marker)
    j = s.index('{', i) + 1
    depth = 1
    while depth:
        depth += (s[j] == '{') - (s[j] == '}')
        j += 1
    return s[i:j]


code = r'''
#![allow(dead_code)]
use std::{cell::Cell,sync::{Arc,atomic::{AtomicBool,AtomicI32,Ordering}}};
thread_local!{static QUERIES:Cell<usize>=const{Cell::new(0)};static SERIAL:Cell<bool>=const{Cell::new(false)};}
mod g17p_render_lifecycle{POOL_CONSTANT}
mod render{use super::*;#[derive(Clone,Copy,PartialEq,Eq)]pub enum PreparationKey{Pool(u32),Storage(u32)}
 pub fn serialized_cache_enabled()->bool{SERIAL.with(Cell::get)}}
struct Fence(AtomicI32);impl Fence{fn raw(&self)->*const AtomicI32{&self.0}}
mod kernel{pub mod bindings{pub unsafe fn dma_fence_get_status(p:*const super::super::AtomicI32)->i32{
 super::super::QUERIES.with(|n|n.set(n.get()+1));unsafe{(*p).load(super::super::Ordering::Acquire)}}}}
struct Frame(u32);impl Frame{fn pool(&self)->u32{self.0}}
struct Wave{frames:Vec<Frame>,next:usize}
impl Wave{ BUSY }
enum PendingWork{Render(Wave),WrappedRender(Wave),Control}
mod production{use super::*;
struct Session{automatic_render_parallel:bool,pending:Option<PendingWork>,
 render_cpu_leases:Vec<(u32,Fence)>,render_prepare_leases:Vec<(render::PreparationKey,Arc<AtomicBool>)>}
impl Session{
 fn live_render_count(&self)->usize{match &self.pending{Some(PendingWork::Render(w)|PendingWork::WrappedRender(w))=>w.frames.len()-w.next,_=>0}}
 ACTIVE
 PREPARING
 IDLE
 ORIGINAL
 BATCH
}
pub fn run(){
 let mut rng=91u64;let mut random=||{rng=rng.wrapping_mul(6364136223846793005).wrapping_add(1);rng};
 for case in 0..20000{
  SERIAL.with(|s|s.set(case&1!=0));
  let frames=(0..random()%24).map(|_|Frame((random()%(g17p_render_lifecycle::POOL_SLOTS as u64+4)) as u32)).collect::<Vec<_>>();
  let next=(random()as usize)%(frames.len()+1);let w=Wave{frames,next};
  let pending=match random()%4{0=>Some(PendingWork::Render(w)),1=>Some(PendingWork::WrappedRender(w)),2=>Some(PendingWork::Control),_=>None};
  let leases=(0..random()%40).map(|_|((random()%(g17p_render_lifecycle::POOL_SLOTS as u64+4))as u32,Fence(AtomicI32::new(match random()%4{0=>0,1=>1,_=>-5})))).collect();
  let preparations=(0..random()%40).map(|_|{let pool=(random()%(g17p_render_lifecycle::POOL_SLOTS as u64+4))as u32;let key=if random()&1!=0{render::PreparationKey::Pool(pool)}else{render::PreparationKey::Storage(pool)};(key,Arc::new(AtomicBool::new(random()&1!=0)))}).collect();
  let s=Session{automatic_render_parallel:case&2!=0,pending,render_cpu_leases:leases,render_prepare_leases:preparations};
  let expected=std::array::from_fn(|i|s.render_pool_available(i as u32));
  QUERIES.with(|n|n.set(0));let got=s.render_pool_availability();assert_eq!(got,expected,"case {case}");
  QUERIES.with(|n|assert!(n.get()<=s.render_cpu_leases.len(),"each CPU lease may be sampled at most once per selection"));
  // Off-lock completion/release can only improve availability. Cached
  // positives remain safe; negatives are refreshed by the next selection.
  for (_,f) in &s.render_cpu_leases{f.0.store(1,Ordering::Release);}
  for (_,active) in &s.render_prepare_leases{active.store(false,Ordering::Release);}
  for (pool,available) in got.iter().enumerate(){if *available{assert!(s.render_pool_available(pool as u32));}}
  assert_eq!(s.render_pool_availability(),std::array::from_fn(|i|s.render_pool_available(i as u32)));
 }
 println!("PASS 20000 actual classification/original-gate comparisons: Neo/Max waves, retired prefixes, pending/error/signalled fences, pool/storage preparations, serialized/parallel modes and monotonic off-lock completion");
}
}
fn main(){production::run();}
'''
for key, marker in [('ACTIVE', '    fn render_preparation_active('),
                    ('PREPARING', '    fn render_preparing('),
                    ('IDLE', '    fn retained_render_idle('),
                    ('ORIGINAL', '    fn render_pool_available('),
                    ('BATCH', '    fn render_pool_availability(')]:
    code = code.replace(key, item(source, marker))
code = code.replace('POOL_CONSTANT', next(line for line in (r / 'g17p_render_lifecycle.rs').read_text().splitlines() if line.startswith('pub(crate) const POOL_SLOTS:')))
code = code.replace('BUSY', item(wave, '    pub(super) fn pool_busy(').replace('pub(super) ', ''))
a.evidence.mkdir(parents=True, exist_ok=False)
(a.evidence / 'test.rs').write_text(code)
subprocess.run(['rustc', '--edition=2021', '-O', str(a.evidence / 'test.rs'),
                '-o', str(a.evidence / 'gate')], check=True)
out = subprocess.check_output([str(a.evidence / 'gate')], text=True)
(a.evidence / 'output.txt').write_text(out)
(a.evidence / 'source.json').write_text(json.dumps({
    'boot_sha256': hashlib.sha256(source.encode()).hexdigest(),
    'wave_sha256': hashlib.sha256(wave.encode()).hexdigest(),
    'GPU_execution': False,
}, indent=2) + '\n')
print(out, end='')
