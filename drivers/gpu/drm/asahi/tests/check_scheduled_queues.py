#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Whole Max queue storage and common UAT mappings vs actual QueueSet.create.

Uses populated typed FDT nodes and bounded owned RAM. No target access or
privileged UAT invalidation is executed; Session's live invalidation is separate.
"""
import argparse
import ctypes as C
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

import host_fdt
import host_versions
import host_vm
import g16c_graph_reference as reference

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1', type=Path)
p.add_argument('--inputs', type=Path, required=True)
p.add_argument('--fdt-template', type=Path, required=True)
p.add_argument('--evidence', type=Path, required=True)
args = p.parse_args()
tests = Path(__file__).resolve().parent
sys.path.insert(0, str(args.m1n1 / 'proxyclient'))
from m1n1.g16c.platform import Platform
from m1n1.g16c.arena import Arena, QueueSet

inputs = json.loads(args.inputs.read_text())
props = {k:bytes.fromhex(v) for k,v in inputs['properties'].items()}
platform = Platform(inputs['sgx_base'], inputs['sgx_span'], inputs['revision'],
                    props, inputs['core'], inputs['sram'])
regions = [(int.from_bytes(props[n+'-base'], 'little'), int.from_bytes(props[n+'-size'], 'little'))
    for n in ['gpu-region', 'gfx-shared-region', 'gfx-shared-l2-region', 'gfx-handoff', 'gfx-data']]
b, ram = reference.build(args.m1n1, platform, 0)
expected, layouts = {}, []
for i in range(4):
    base = 0xfffffc2208000000 + i * 0x100000
    arena = Arena(b.firmware, base, base + 0x84000)
    lows = None if i % 2 == 0 else tuple(0x7500000000 + i * 0x100000 + j * 0x20000 for j in range(3))
    q = QueueSet.create(arena, i, 0xff00 + i, alias_space=b.client,
        scratch_aliases=lows, private_rings=True)
    assert len(q.allocations) == 12
    layouts.append((base, lows))
    for j, a in enumerate(q.allocations):
        expected[f'{i}-{j}.bin'] = b.firmware.read(a.address, a.size)
    expected[f'{i}-aliases.bin'] = b''.join(b.client.read(low, 0x20000) for low in q.scratch_low)

with tempfile.TemporaryDirectory(prefix='scheduled-queues-') as directory:
    tmp = Path(directory)
    flags = host_versions.build(tmp)
    loader = host_fdt.Loader(args.m1n1, tmp, args.fdt_template)
    dt, incoming, owners = loader.prepare(platform)
    values = (host_fdt.Region * 5)(*[host_fdt.Region(*r) for r in regions])
    assert loader.populate_memory(dt, loader.path(dt, b'/gpu'), values,
                                   0xfffffc0000000000, 0x2000000000) == 0
    assert loader.populate(dt, loader.path(dt, b'/gpu'), C.byref(incoming)) == 0
    node = loader.snapshot(dt, loader.path(dt, b'/gpu'), True)
    code = host_vm.prelude()
    for name in ['float', 'scheduled_initdata', 'scheduled_boot_records', 'scheduled_hwdata',
        'scheduled_board', 'scheduled_power', 'scheduled_initgraph', 'scheduled_mmio',
        'scheduled_resources', 'scheduled_image', 'g17p_abi', 'g17p_layout',
        'g17p_initgraph', 'g17p_opening', 'g17p_topology', 'g17p_vm',
        'scheduled_vm_g16c', 'g17p_queue', 'scheduled_queues']:
        code += f'#[path={json.dumps(str(tests.parent/(name+".rs")))}] mod {name};\n'
    code += '''
use prelude::*;
use g17p_memory::Memory;
use g17p_vm::Vm;
use scheduled_queues::{QueueSetG16C,START,SIZE};
use scheduled_resources::ResourcesG16C;
const PAGE:u64=0x4000; const MASK:u64=0x0000ffffffffc000;
fn boot(r:&ResourcesG16C)->Memory {
    let mut m=Memory::new();for p in &r.regions[..4]{m.reserve(p.base,p.size);}m
}
fn bytes(m:&Memory,vm:&Vm,group:usize,va:u64,size:u64)->Result<Vec<u8>> {
    let mut out=Vec::new();for offset in (0..size).step_by(PAGE as usize){
        out.extend(m.read(vm.physical(m,group,va+offset)?,PAGE as usize)?);
    }Ok(out)
}
fn main()->Result {
    let directory=std::path::PathBuf::from(std::env::args().nth(1).unwrap());std::fs::create_dir_all(&directory).unwrap();
'''
    code += 'let node=' + host_fdt.rust_node(node) + ';\n'
    code += '''
    let r=ResourcesG16C::from_fdt(&node)?;
    let image=scheduled_image::ImageG16C::new(&node,0)?;
    let mut m=boot(&r);let mut vm=Vm::build_g16c(&mut m,&r,&image)?;
    let context1=vm.boot_context_roots(1)?;
    let ttbs=m.read(r.regions[0].base,0x4000)?;
'''
    for i, (base, lows) in enumerate(layouts):
        aliases = 'None' if lows is None else 'Some([' + ','.join(str(v) for v in lows) + '])'
        code += 'let caller_maps=context1;let caller_ttbs=ttbs.clone();let before_calls=m.calls;\n'
        code += 'let mut backing=Some(scheduled_queues::QueueBackingG16C::new()?);assert_eq!(m.calls,before_calls);\n'
        code += f'let (staged,prepared)=QueueSetG16C::prepare_backed(&mut m,&mut vm,{base},{i},{0xff00+i},{aliases},&mut backing)?;assert!(backing.is_none());assert_eq!(staged.calls,12);m.absorb(staged)?;let mut q=prepared.install(&mut m,&mut vm,{i})?;\n'
        code += 'assert_eq!(vm.boot_context_roots(1)?,caller_maps);assert_eq!(m.read(r.regions[0].base,0x4000)?,caller_ttbs);\n'
        code += f'let case={i};\n'
        code += '''
    assert_eq!((q.base,q.jobs,q.context,q.uuid),(q.base,q.base+0x4100,q.base+0x140,0xff00+case as u16));
    let mut aliases=Vec::new();
    for (i,lane) in q.lanes.iter().enumerate() {
        assert_eq!((lane.head,lane.first,lane.capacity,lane.priority,lane.stage),(0,true,1280,case as u8,i as u8));
        assert_eq!((lane.queue,lane.pointers,lane.ring,lane.scratch_high),
            (q.base+i as u64*0x2c000,q.base+i as u64*0x2c000+0x4000,
             q.base+i as u64*0x2c000+0x8000,q.base+i as u64*0x2c000+0xc000));
        for (j,(offset,size,flags)) in [(0,PAGE,0x00c0000000000443),
            (0x4000,PAGE,0x00c000000000044b),(0x8000,PAGE,0x00c0000000000443),
            (0xc000,0x20000,0x00c000000000044b)].into_iter().enumerate() {
            let start=lane.queue+offset;
            for at in (start..start+size).step_by(PAGE as usize) {
                let pte=vm.pte(&m,2,at)?;assert_eq!(pte&!MASK,flags);assert_ne!(pte&MASK,0);
                if offset==0xc000 {
                    let low=lane.scratch_low+at-start;
                    assert_eq!(vm.pte(&m,0,low)?,(pte&MASK)|0x0080000000000c8b);
                    assert_eq!(vm.pte(&m,1,low)?,0);
                }
            }
            std::fs::write(directory.join(format!("{case}-{}.bin",i*4+j)),bytes(&m,&vm,2,start,size)?).unwrap();
        }
        aliases.extend(bytes(&m,&vm,0,lane.scratch_low,0x20000)?);
    }
    std::fs::write(directory.join(format!("{case}-aliases.bin")),aliases).unwrap();
    let calls=m.calls;let before=bytes(&m,&vm,2,q.base,SIZE)?;
    // Firmware mutation is discarded only through the caller's retired-slot
    // reset. Exact original whole bytes/aliases and allocation count return.
    vm.write(&mut m,2,q.base,&vec![0xa5;SIZE as usize])?;
    for lane in &mut q.lanes {lane.head=233;lane.first=false;lane.priority=3;}
    q.reset(&mut m,&vm,case as u8)?;
    assert_eq!(m.calls,calls);assert_eq!(bytes(&m,&vm,2,q.base,SIZE)?,before);
    for lane in &q.lanes {
        assert_eq!((lane.head,lane.first,lane.priority),(0,true,case as u8));
        assert_eq!(bytes(&m,&vm,0,lane.scratch_low,0x20000)?,vec![0;0x20000]);
    }
    // Rebinding a retained engine restarts transport DATA but preserves
    // every firmware-owned context/scratch byte and both exact aliases.
    vm.write(&mut m,2,q.base,&vec![0x6d;SIZE as usize])?;
    q.reset_retained(&mut m,&vm,case as u8)?;
    let mut retained=before.clone();
    for lane in 0..3usize {
        let offset=lane*0x2c000+0xc000;
        retained[offset..offset+0x20000].fill(0x6d);
        assert_eq!(bytes(&m,&vm,0,q.lanes[lane].scratch_low,0x20000)?,vec![0x6d;0x20000]);
    }
    assert_eq!(bytes(&m,&vm,2,q.base,SIZE)?,retained);assert_eq!(m.calls,calls);
    q.reset(&mut m,&vm,case as u8)?;assert_eq!(bytes(&m,&vm,2,q.base,SIZE)?,before);
    assert!(q.reset(&mut m,&vm,4).is_err());
    assert_eq!(bytes(&m,&vm,2,q.base,SIZE)?,before);
    assert!(QueueSetG16C::new(&mut m,&mut vm,q.base,0,0,None).is_err());
    assert_eq!(m.calls,calls);assert_eq!(bytes(&m,&vm,2,q.base,SIZE)?,before);
'''
    code += '''
    // Canonical engine arrays have an independent lifetime and exact ctx0
    // aliases. No queue allocation owns or may reinitialize this backing.
    let scratch=scheduled_queues::EngineScratchG16C::new(&mut m,&mut vm)?;
    for i in 0..4 {
        assert_eq!(bytes(&m,&vm,2,scratch.high[i],0x20000)?,vec![0;0x20000]);
        assert_eq!(bytes(&m,&vm,0,scratch.low[i],0x20000)?,vec![0;0x20000]);
        for offset in (0..0x20000).step_by(PAGE as usize) {
            let high=vm.pte(&m,2,scratch.high[i]+offset)?;
            assert_eq!(high&!MASK,0x00c000000000044b);
            assert_eq!(vm.pte(&m,0,scratch.low[i]+offset)?,(high&MASK)|0x0080000000000c8b);
            assert_eq!(vm.pte(&m,1,scratch.low[i]+offset)?,0);
        }
        vm.write(&mut m,2,scratch.high[i],&[0x91+i as u8;64])?;
    }
    let calls=m.calls;
    assert!(scheduled_queues::EngineScratchG16C::new(&mut m,&mut vm).is_err());
    assert_eq!(m.calls,calls);
    for i in 0..4 {assert_eq!(bytes(&m,&vm,0,scratch.low[i],PAGE)?[..64],[0x91+i as u8;64]);}
    for i in 0..3 {
        q.lanes[i].scratch_high=scratch.high[i];q.lanes[i].scratch_low=scratch.low[i];
    }
    q.reset(&mut m,&vm,3)?;
    for i in 0..4 {assert_eq!(bytes(&m,&vm,0,scratch.low[i],PAGE)?[..64],[0x91+i as u8;64]);}
    // Even a mismatch in the LAST alias must reject before the first zero.
    // The retained baseline still proves old whole queue bytes are untouched.
    let before=bytes(&m,&vm,2,q.base,SIZE)?;
    let alias=0x7500000000+3*0x100000+2*0x20000+0x1c000;
    let (pte_address,pte)=vm.leaf_record(&m,0,alias)?;
    m.write64(pte_address,pte^0x4000)?;
    assert!(q.reset(&mut m,&vm,0).is_err());
    assert_eq!(bytes(&m,&vm,2,q.base,SIZE)?,before);
    m.write64(pte_address,pte)?;

    // Every malformed geometry is rejected before allocating or writing.
    let free=START+0x500000;let calls=m.calls;
    for (base,priority,aliases) in [(free,4,None),(free+1,0,None),
        (START-PAGE,0,None),(scheduled_queues::END-SIZE+PAGE,0,None),
        (free,0,Some([0x7600000001,0x7600020000,0x7600040000])),
        (free,0,Some([0x7600000000,0x7600004000,0x7600040000])),
        (free,0,Some([(1<<42)-PAGE,0x7600020000,0x7600040000]))] {
        assert!(QueueSetG16C::new(&mut m,&mut vm,base,priority,0,aliases).is_err());assert_eq!(m.calls,calls);
    }
    // Alias collision, including a pre-existing source bootstrap alias.
    assert!(QueueSetG16C::new(&mut m,&mut vm,free,0,0,
        Some([0x7408000000,0x7600020000,0x7600040000])).is_err());assert_eq!(m.calls,calls);
    // Fail every detached queue-backing allocation boundary. Empty parent
    // tables may remain, but no queue/alias leaf or producer may be installed.
    // The exact same reservation must succeed after removing the shortage.
    let mut bad=boot(&r);let mut bv=Vm::build_g16c(&mut bad,&r,&image)?;
    for failed in 0..12 {
        g17p_memory::DETACHED_FAIL_AFTER.store(failed,std::sync::atomic::Ordering::Relaxed);
        assert!(QueueSetG16C::new(&mut bad,&mut bv,START,0,1,None).is_err());
        for at in (START..START+SIZE).step_by(PAGE as usize){assert_eq!(bv.pte(&bad,2,at)?,0);}
        for stage in 0..3 {for off in (0..0x20000).step_by(PAGE as usize){assert_eq!(bv.pte(&bad,0,0x7408000000+stage*0x2c000+off)?,0);}}
    }
    g17p_memory::DETACHED_FAIL_AFTER.store(usize::MAX,std::sync::atomic::Ordering::Relaxed);
    let retry=QueueSetG16C::new(&mut bad,&mut bv,START,0,1,None)?;
    assert_eq!(retry.base,START);
    for failed in 0..4 {
        g17p_memory::DETACHED_FAIL_AFTER.store(failed,std::sync::atomic::Ordering::Relaxed);
        assert!(scheduled_queues::EngineScratchG16C::new(&mut bad,&mut bv).is_err());
        for stage in 0..4 {for off in (0..0x20000).step_by(PAGE as usize){
            assert_eq!(bv.pte(&bad,2,0xfffffc2206000000+stage*0x20000+off)?,0);
            assert_eq!(bv.pte(&bad,0,0x7406000000+stage*0x20000+off)?,0);
        }}
    }
    g17p_memory::DETACHED_FAIL_AFTER.store(usize::MAX,std::sync::atomic::Ordering::Relaxed);
    scheduled_queues::EngineScratchG16C::new(&mut bad,&mut bv)?;
    // Both parent-table allocation failures are bounded and leave no leaf.
    // Retry preserves completed EMPTY parents and does not allocate again.
    let address=0x30000000000;bad.fail_after=Some(bad.calls);
    assert_eq!(bv.prepare_g16c_empty(&mut bad,0,address,PAGE as usize),Err(ENOMEM));
    assert_eq!(bv.pte(&bad,0,address)?,0);
    bad.fail_after=Some(bad.calls+1);
    assert_eq!(bv.prepare_g16c_empty(&mut bad,0,address,PAGE as usize),Err(ENOMEM));
    assert_eq!(bv.pte(&bad,0,address)?,0);
    bad.fail_after=None;bv.prepare_g16c_empty(&mut bad,0,address,PAGE as usize)?;
    let prepared_calls=bad.calls;bv.prepare_g16c_empty(&mut bad,0,address,PAGE as usize)?;
    assert_eq!(bad.calls,prepared_calls);assert_eq!(bv.pte(&bad,0,address)?,0);
    assert_eq!(m.read(r.regions[0].base,0x4000)?,ttbs);
    for root in context1 {assert!(m.read(root,0x4000)?.iter().all(|x|*x==0));}
    println!("PASS common Memory/UAT: 4 complete Max queue sets, 48 RAM owners, 96 alias pages, typed permissions, malformed/colliding reservations and 16 detached allocation failures and successful same-reservation retry; no producer/root publication");Ok(())
}
'''
    (tmp/'main.rs').write_text(code)
    subprocess.run(['rustc', '--edition=2021', '-Dwarnings', str(tmp/'main.rs'), *flags,
                    '-o', str(tmp/'gate')], check=True)
    subprocess.run([str(tmp/'gate'), str(tmp/'images')], check=True)
    for name, want in expected.items():
        got=(tmp/'images'/name).read_bytes()
        assert got==want,(name,[hex(i) for i,(a,b) in enumerate(zip(got,want)) if a!=b][:16])
    args.evidence.mkdir(parents=True,exist_ok=False)
    for name, want in expected.items(): (args.evidence/name).write_bytes(want)
    summary=dict(actual_source_QueueSet_create_cases=4,detached_transport_backing_cases=4,transport_preparation_uses_only_preallocated_backing=True, whole_queue_RAM_owners=48,
        RAM_bytes=4*0x84000, context0_alias_pages=96, context1_unchanged=True,
        TTBS_unchanged=True, no_producer_publication=True,
        preallocation_geometry_rejections=7, collision_rejections=5,
        detached_queue_allocation_failure_boundaries=12,
        empty_parent_allocation_failure_boundaries=2,
        prepared_parent_retry_allocations=0,
        detached_engine_allocation_failure_boundaries=4,
        failed_allocation_installs_no_leaves=True,
        same_reservation_retry_after_allocation_failure=True, source_private_rings=True,
        independent_canonical_scratch_pages=48,
        canonical_scratch_aliases_exact=True,
        canonical_scratch_reallocation_refuses_without_writes=True,
        complete_queue_reset_images=4,complete_retained_scratch_reset_images=4,retained_scratch_bytes_checked=4*3*0x20000,retained_scratch_all_bytes_and_aliases_preserved=True,
        queue_reset_allocations=0,
        reset_canonical_engine_arrays_unchanged=True,
        last_alias_mismatch_rejects_before_any_zero=True,
        privileged_UAT_or_GPU_execution_tested=False,
        images={name:dict(bytes=len(v),sha256=hashlib.sha256(v).hexdigest()) for name,v in expected.items()})
    (args.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print('PASS: 48 backing and 4 whole alias images equal Python QueueSet.create; allocation-free whole resets, independent engine arrays and late-alias rejection')
