#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Check real generated Pro layouts, reported constants and ADT -> FDT -> boot graph.

No Pro hardware qualification is implied. The report says Pro and Max wire
shapes are identical; compare every generated field boundary, and compile the
existing RE offset assertions, rather than defining a parallel test layout.
"""
import argparse
import ctypes as C
import json
from pathlib import Path
import re
from types import SimpleNamespace
import shutil
import struct
import subprocess
import tempfile

import host_fdt
import host_versions

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1', type=Path, help='bootloader with two-cluster FDT support')
parser.add_argument('--fdt-template', type=Path, required=True)
parser.add_argument('--evidence', type=Path, required=True)
args = parser.parse_args()
tests = Path(__file__).resolve().parent
source = tests.parent
inputs = json.loads((tests / 'g16s-j773s-inputs.json').read_text())
props = {k: bytes.fromhex(v) for k, v in inputs['properties'].items()}
p = SimpleNamespace(properties=props, revision=inputs['revision'],
                    period_clocks=200000, gpc=True)
p.word = lambda key, default: struct.unpack('<I', props[key])[0] if key in props else default
args.evidence.mkdir(parents=True, exist_ok=True)

with tempfile.TemporaryDirectory(prefix='g16s-profile-') as directory:
    tmp = Path(directory)
    flags = host_versions.build(tmp)
    # Keep all production relative modules; append assertions inside each module
    # so private staging fields are covered as well as public wire fields.
    for path in source.glob('*.rs'):
        shutil.copy(path, tmp / path.name)
    modules = ['scheduled_initdata', 'scheduled_boot_records', 'scheduled_hwdata',
               'scheduled_power', 'scheduled_wire', 'scheduled_dispatch',
               'scheduled_compute', 'scheduled_work', 'scheduled_render']
    shapes, fields = [], 0
    for module in modules:
        path = tmp / (module + '.rs')
        body = path.read_text()
        checks = []
        pattern = r'#\[versions\(AGX_SCHEDULED\)\]((?:\s*#\[[^\n]*\]|\s*//[^\n]*)*)\s*(?:pub(?:\(crate\))?\s+)?struct\s+(\w+)\s*\{'
        for match in re.finditer(pattern, body, re.S):
            if '#[ver(G == G17P)]' in match[1]:
                continue
            name = match[2]
            start = match.end()
            depth, end = 1, start
            while depth:
                depth += (body[end] == '{') - (body[end] == '}')
                end += 1
            checks += [f'assert!(core::mem::size_of::<{name}G16S>() == core::mem::size_of::<{name}G16C>());',
                       f'assert!(core::mem::align_of::<{name}G16S>() == core::mem::align_of::<{name}G16C>());']
            skip = False
            names = []
            for line in body[start:end-1].splitlines():
                if '#[ver(' in line:
                    skip = 'G == G17P' in line and 'G16' not in line
                    continue
                field = re.match(r'\s*(?:pub(?:\(crate\))?\s+)?(\w+)\s*:', line)
                if not field:
                    continue
                if not skip:
                    field = field[1]
                    assert field not in names, (module, name, field)
                    names.append(field)
                    checks.append(f'assert!(core::mem::offset_of!({name}G16S, {field}) == core::mem::offset_of!({name}G16C, {field}));')
                    fields += 1
                skip = False
            shapes.append(dict(module=module, name=name, fields=names))
        path.write_text(body + '\nconst _: () = {\n' + '\n'.join(checks) + '\n};\n')
    assert len(shapes) >= 15 and fields >= 400, (len(shapes), fields)

    loader = host_fdt.Loader(args.m1n1, tmp, args.fdt_template)
    dt, incoming, owners = loader.prepare(p)
    gpu = loader.path(dt, b'/gpu')
    compatible = b'apple,agx-t6040\0apple,agx-g16s\0'
    assert loader.set(dt, gpu, b'compatible', compatible, len(compatible)) == 0
    gpu = loader.path(dt, b'/gpu')
    ph = loader.property(dt, gpu, 'operating-points-v2')
    table = loader.phandle(dt, struct.unpack('>I', ph)[0])
    # The shipped Pro fragment has two voltage columns. Resize only the host
    # Max fixture here before invoking the actual inplace population routine.
    for index in range(11):
        child = list(loader.children(dt, table))[index]
        for key in ['opp-microvolt', 'apple,sram-microvolt']:
            assert loader.set(dt, child, key.encode(), bytes(8), 8) == 0
    gpu = loader.path(dt, b'/gpu')
    assert loader.populate(dt, gpu, C.byref(incoming)) == 0
    node = loader.snapshot(dt, loader.path(dt, b'/gpu'), True)
    assert struct.unpack('>I', node['props']['apple,max-power-milliwatt'])[0] == 68444
    opps = node['links']['operating-points-v2']['children']
    assert len(opps) == 11
    assert all(len(o['props']['opp-microvolt']) == 8 for o in opps)
    assert max(struct.unpack('>Q', o['props']['opp-hz'])[0] for o in opps) == 1578000000
    assert len(node['links']['apple,cs-opp']['children']) == 8
    # Invalid table dimensions must reject before changing any FDT bytes.
    valid_size = incoming.core_size
    for size in [0, 127, 128, 255, 384, 512]:
        incoming.core_size = size
        before = bytes(dt)
        assert loader.populate(dt, loader.path(dt,b'/gpu'),C.byref(incoming)) < 0
        assert bytes(dt) == before
    incoming.core_size = valid_size

    code = host_fdt.rust_prelude() + 'use std::{fs,path::Path};\n'
    for module in ['scheduled_profile', 'float', 'scheduled_initdata', 'scheduled_boot_records',
                   'scheduled_hwdata', 'scheduled_power', 'scheduled_board', 'scheduled_mmio',
                   'scheduled_initgraph', 'scheduled_wire', 'scheduled_dispatch',
                   'g17p_compute', 'g17p_render']:
        code += f'#[path={json.dumps(str(tmp/(module+".rs")))}] mod {module};\n'
    code += '''
mod g17p_render_lifecycle {pub const POOL_SLOTS:u32=(u32::BITS-2)/2;}
use scheduled_profile::Profile;
use scheduled_initgraph as graph;
struct Image(Vec<Vec<u8>>);
impl graph::Storage for Image {
    fn object(&mut self,i:usize)->Result<&mut [u8],graph::InvalidGraph>{
        self.0.get_mut(i).map(|v|v.as_mut_slice()).ok_or(graph::InvalidGraph)
    }
}
fn w(b:&[u8],at:usize)->u32 {u32::from_le_bytes(b[at..at+4].try_into().unwrap())}
fn q(b:&[u8],at:usize)->u64 {u64::from_le_bytes(b[at..at+8].try_into().unwrap())}
fn main(){let root=std::env::args().nth(1).unwrap();let root=Path::new(&root);
'''
    code += 'let node=' + host_fdt.rust_node(node) + ';\n'
    code += '''
let board=scheduled_board::BoardG16C::from_fdt(&node).unwrap();
assert_eq!(board.profile,Profile::G16S);assert_eq!(board.maximum_power,68444);
assert_eq!(board.period_ms,8);assert_eq!(board.cs_count,8);
let settings=scheduled_power::SettingsG16C::from_fdt(&node).unwrap();
let mmio=scheduled_mmio::MmioG16C::from_fdt(&node).unwrap();let regs=mmio.registers();
let mut storage=Image(graph::OBJECT_LAYOUT.iter().map(|o|vec![0xa5;o.size]).collect());
let graph=graph::build(&mut storage,&board,&regs,&settings,0x6040).unwrap();
assert!(graph.root(scheduled_initdata::FirmwareRole::Primary).is_some());
assert_eq!(graph.root(scheduled_initdata::FirmwareRole::Secondary),None);
let hw=&storage.0[25];
for (at,value) in [(0xe90,0x6040),(0xf88,20),(0xf8c,15),(0xfb8,0x1c),(0xfbc,4),(0xfc0,20),(0x2570,20)] {assert_eq!(w(hw,at),value,"hw {at:x}");}
let power=&hw[graph::POWER_OFFSET..];
// Field names are absolute bundle offsets minus the 0x140 capture header.
assert_eq!(w(power,0x7504-0x5240),10100);
assert_eq!(w(power,0x74b0-0x5240),9600);
assert_eq!(w(power,0xb5a0-0x5240),3);
for at in [0xb568,0xb56c]{assert_eq!(w(power,at-0x5240),0);}
for at in [0x74a8,0x8148,0x8d98]{assert_eq!(q(power,at-0x5240),0x2a222aa2);}
fs::write(root.join("pro-bundle.bin"),hw).unwrap();
assert_eq!(Profile::G16S.operand_bytes(),0x44000);assert_eq!(Profile::G16S.deflake(),[0xa0,0x820]);
assert_eq!(Profile::G16C.operand_bytes(),0x7c000);assert_eq!(Profile::G16C.deflake(),[0x120,0xda0]);
let p=g17p_compute::wire::ProgramParameters{save_area:0x1000000,cdm:0x2000000,identity:7,context:1,ordinal:0,robustness:0x3000000,operand_state:0x4000000,usc_exec_base:Some(1<<40),helper_binary:0,helper_data:0,helper_config:0,execution_gate:0,register_a599:0x13200400020};
let s=g17p_compute::wire::ComputeProgramG16S::new(&p).unwrap();
let c=g17p_compute::wire::ComputeProgramG16C::new(&p).unwrap();
let n=g17p_compute::wire::ComputeProgramG17P::new(&p).unwrap();
for (regs,save,operand) in [(s.registers(),0x1480,[0x3c800,0x3d800,0x1e405,0x3c400,5,0x1e000]),(c.registers(),0x2980,[0x78800,0x7a800,0x3c405,0x78400,5,0x3c000]),(n.registers(),0x1480,[0x12800,0x13000,0x9405,0x12400,5,0x9000])]{
for i in 0..4{assert_eq!(regs[2+i].1,p.save_area+save+i as u64*8);}
for i in 0..6{assert_eq!(regs[32+i].1,p.operand_state+operand[i]);}}
let selected=g17p_compute::wire::ComputeProgramG16C::for_profile(&p,Profile::G16S).unwrap();
assert_eq!(selected.registers(),s.registers());
println!("PASS: Pro ADT -> actual C FDT -> source boot graph; Pro/Max/Neo compute constants");
}
'''
    harness = tmp/'main.rs'
    harness.write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(harness),*flags,'-o',str(tmp/'gate')],check=True)
    subprocess.run([str(tmp/'gate'), str(args.evidence)], check=True)
    result = dict(pass_host=True, hardware_qualified=False, generated_shapes=len(shapes),
                  generated_fields=fields, layouts=shapes, adt_sha256=inputs['sha256'])
    (args.evidence/'summary.json').write_text(json.dumps(result,indent=2)+'\n')
    print(f'PASS: {len(shapes)} actual macro-generated G16S/G16C shapes, {fields} field offsets')
