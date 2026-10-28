#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare all owned Max boot RAM and aliases against actual source startup.

Executes qualified Bootstrap.build_graph in bounded host RAM, actual m1n1 C FDT
population and production Rust encoders. No target or Apple executable access.
"""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

import g16c_graph_reference as reference
import host_fdt
import host_versions

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1', type=Path)
parser.add_argument('--inputs', type=Path, required=True)
parser.add_argument('--fdt-template', type=Path, required=True)
parser.add_argument('--evidence', type=Path)
parser.add_argument('--rustc', default='rustc')
args = parser.parse_args()
tests = Path(__file__).resolve().parent
sys.path.insert(0, str(args.m1n1.resolve() / 'proxyclient'))
from m1n1.g16c.platform import Platform

inputs = json.loads(args.inputs.read_text())
properties = {k: bytes.fromhex(v) for k, v in inputs['properties'].items()}
baseline = Platform(inputs['sgx_base'], inputs['sgx_span'], inputs['revision'],
                    properties, inputs['core'], inputs['sram'])
expected = {}
manifest = []
with tempfile.TemporaryDirectory(prefix='scheduled-graph-') as temporary:
    tmp = Path(temporary)
    flags = host_versions.build(tmp, args.rustc)
    loader = host_fdt.Loader(args.m1n1, tmp, args.fdt_template)
    code = host_fdt.rust_prelude() + 'use std::{fs,path::Path};\n'
    for name in ['float', 'scheduled_initdata', 'scheduled_boot_records',
                 'scheduled_hwdata', 'scheduled_power', 'scheduled_board',
                 'scheduled_initgraph', 'scheduled_mmio']:
        code += f'#[path={json.dumps(str(tests.parent / (name + ".rs")))}] mod {name};\n'
    code += '''
use scheduled_initgraph as graph;
use scheduled_initdata as wire;
struct Image(Vec<Vec<u8>>);
impl graph::Storage for Image {
    fn object(&mut self,i:usize)->Result<&mut [u8],graph::InvalidGraph> {
        let b=self.0.get_mut(i).ok_or(graph::InvalidGraph)?;
        let end=b.len()-16;Ok(&mut b[16..end])
    }
}
fn image()->Image {Image(graph::OBJECT_LAYOUT.iter().map(|o|vec![0xa5;o.size+32]).collect())}
fn unchanged(i:&Image){assert!(i.0.iter().flatten().all(|v|*v==0xa5));}
fn guards(i:&Image){for b in &i.0{assert_eq!(&b[..16],&[0xa5;16]);assert_eq!(&b[b.len()-16..],&[0xa5;16]);}}
fn main(){let directory=std::env::args().nth(1).unwrap();let directory=Path::new(&directory);fs::create_dir_all(directory).unwrap();
'''
    for case, entropy in enumerate([0, 0xffffffff, 0x60412662, 0x12345678]):
        p = copy.copy(baseline)
        p.properties = dict(properties)
        if case == 1:
            p.properties['active-mcc-bit-vector'] = struct.pack('<I', 0x80000025)
            irq = bytearray(p.properties['meta-sw-interrupt'])
            struct.pack_into('<Q', irq, len(irq)-20, 0x502414084 + 0x10000)
            p.properties['meta-sw-interrupt'] = bytes(irq)
            p._registers()
        if case == 2:
            p.properties['gpu-avg-power-ki-only'] = struct.pack('<I', 0x41000000)
            p.properties['gpu-turbo-controller-input'] = struct.pack('<I', 1)
        if case == 3:
            p.revision += 1
            p.period_clocks = 392000
            p.period_ms = p.period_clocks // 24000
            p.gpc = not baseline.gpc
        b, ram = reference.build(args.m1n1, p, entropy)
        spans = reference.spans(b, ram)
        aliases = reference.aliases(b, ram)
        assert len(spans) == 36 and len(aliases) == 8
        # Entire RAM extent, including explicit zeros, not selected fields.
        for i, (address, size, attributes) in enumerate(spans):
            expected[f'{case}-{i}.bin'] = b.firmware.read(address, size)
        expected[f'{case}-objects.bin'] = b''.join(struct.pack('<QQQ', *s) for s in spans)
        expected[f'{case}-aliases.bin'] = b''.join(struct.pack('<QQQQ', *a) for a in aliases)
        expected[f'{case}-channels.bin'] = b''.join(struct.pack('<QQQQ', c.read, c.write, c.completion, c.ring)
                                                   for c in b.main.channels)
        expected[f'{case}-root.bin'] = struct.pack('<Q', b.root.main - 0x1e100 - 0xc04d8000 + 0x100000)
        maps = list(p.register_mappings()) + [(0xfffffc218100c000, p.sgx_base+0xd60000,
                                              0x20000, 0x00c000000000044b)]
        expected[f'{case}-mmio.bin'] = b''.join(struct.pack('<QQQQ', *m) for m in maps)
        _, node = loader.build(p)
        code += '{\nlet node=' + host_fdt.rust_node(node) + ';\n'
        code += 'let board=scheduled_board::BoardG16C::from_fdt(&node).unwrap();let settings=scheduled_power::SettingsG16C::from_fdt(&node).unwrap();\n'
        code += 'let mmio=scheduled_mmio::MmioG16C::from_fdt(&node).unwrap();let registers=mmio.registers();\n'
        code += f'''let mappings=mmio.mappings().iter().flat_map(|m|[m.address,m.physical,m.size as u64,m.pte_flags]).flat_map(u64::to_le_bytes).collect::<Vec<_>>();
fs::write(directory.join("{case}-mmio.bin"),mappings).unwrap();
'''
        code += f'let mut storage=image();let g=graph::build(&mut storage,&board,&registers,&settings,{entropy}).unwrap();guards(&storage);\n'
        code += f'''for (i,b) in storage.0.iter().enumerate() {{fs::write(directory.join(format!("{case}-{{i}}.bin")),&b[16..b.len()-16]).unwrap();}}
let objects=graph::OBJECT_LAYOUT.iter().flat_map(|o|[graph::BASE+o.offset as u64,o.size as u64,o.pte_flags]).flat_map(u64::to_le_bytes).collect::<Vec<_>>();
fs::write(directory.join("{case}-objects.bin"),objects).unwrap();
let aliases=graph::ALIASES.iter().flat_map(|a|[a.low,a.high,a.size as u64,a.pte_flags]).flat_map(u64::to_le_bytes).collect::<Vec<_>>();
fs::write(directory.join("{case}-aliases.bin"),aliases).unwrap();
let channels=g.channels.iter().flat_map(|c|c.states.iter().copied().chain(core::iter::once(c.ring))).flatten().collect::<Vec<_>>();
fs::write(directory.join("{case}-channels.bin"),channels).unwrap();
fs::write(directory.join("{case}-root.bin"),g.root(wire::FirmwareRole::Primary).unwrap().to_le_bytes()).unwrap();
assert_eq!(g.root(wire::FirmwareRole::Secondary),None);
'''
        if case == 0:
            code += '''
// Every truncated/oversized/missing owner must fail before any RAM changes.
for i in 0..graph::OBJECTS {
    for delta in [-1isize,1] {
        let mut s=image();let n=(s.0[i].len() as isize+delta) as usize;s.0[i].resize(n,0xa5);
        assert!(graph::build(&mut s,&board,&registers,&settings,0).is_err());unchanged(&s);
    }
}
let mut s=image();s.0.pop();assert!(graph::build(&mut s,&board,&registers,&settings,0).is_err());unchanged(&s);
for count in [0,17,usize::MAX]{let mut bad=scheduled_board::BoardG16C::from_fdt(&node).unwrap();bad.cs_count=count;let mut s=image();assert!(graph::build(&mut s,&bad,&registers,&settings,0).is_err());unchanged(&s);}
let mut bad=scheduled_board::BoardG16C::from_fdt(&node).unwrap();bad.period_ms=0;let mut s=image();assert!(graph::build(&mut s,&bad,&registers,&settings,0).is_err());unchanged(&s);
let bad=scheduled_power::SettingsG16C{pwr_min_duty_cycle:Some(1001),..Default::default()};
let mut s=image();assert!(graph::build(&mut s,&board,&registers,&bad,0).is_err());unchanged(&s);
let mut bad=registers.to_vec();bad.push(registers[0]);let mut s=image();assert!(graph::build(&mut s,&board,&bad,&settings,0).is_err());unchanged(&s);
let mut bad=registers.to_vec();bad[0].0=53;let mut s=image();assert!(graph::build(&mut s,&board,&bad,&settings,0).is_err());unchanged(&s);
for key in ["apple,firmware-sw-interrupt","apple,active-mcc-mask"] {
    let mut bad=node.clone();bad.props.remove(key);assert!(scheduled_mmio::MmioG16C::from_fdt(&bad).is_err());
    let mut bad=node.clone();bad.props.insert(key.to_owned(),vec![0;3]);assert!(scheduled_mmio::MmioG16C::from_fdt(&bad).is_err());
}
for irq in [0,0x3fff,1<<48,u64::MAX] {
    let mut bad=node.clone();bad.props.insert("apple,firmware-sw-interrupt".to_owned(),irq.to_be_bytes().to_vec());
    assert!(scheduled_mmio::MmioG16C::from_fdt(&bad).is_err());
}
let mut bad=node.clone();bad.props.insert("apple,active-mcc-mask".to_owned(),0u32.to_be_bytes().to_vec());assert!(scheduled_mmio::MmioG16C::from_fdt(&bad).is_err());
// Every MCC bit and the maximum window count fit, with page-aligned maps.
for mask in (0..32).map(|i|1u32<<i).chain([u32::MAX]) {
    let mut n=node.clone();n.props.insert("apple,active-mcc-mask".to_owned(),mask.to_be_bytes().to_vec());
    let m=scheduled_mmio::MmioG16C::from_fdt(&n).unwrap();assert_eq!(m.mappings().len(),28+mask.count_ones() as usize);
    for p in m.mappings(){assert_eq!(p.address&0x3fff,0);assert_eq!(p.physical&0x3fff,0);assert_eq!(p.size&0x3fff,0);}
    let bytes=m.mappings().iter().flat_map(|p|[p.address,p.physical,p.size as u64,p.pte_flags]).flat_map(u64::to_le_bytes).collect::<Vec<_>>();
    fs::write(directory.join(format!("mask-{mask:x}-maps.bin")),bytes).unwrap();
    let bytes=m.registers().iter().flat_map(|(slot,r)|(*slot as u64).to_le_bytes().into_iter().chain(r.as_bytes().iter().copied())).collect::<Vec<_>>();
    fs::write(directory.join(format!("mask-{mask:x}-registers.bin")),bytes).unwrap();
}
// The whole opening slot lies inside MainConfig's own bundle allocation.
let ring=u64::from_le_bytes(g.channels[12].ring);let offset=(ring-graph::BASE-graph::BUNDLE_OFFSET as u64) as usize;
let owner=graph::OBJECT_LAYOUT.iter().position(|o|o.offset==graph::BUNDLE_OFFSET).unwrap();
let opening=&storage.0[owner][16+offset..16+offset+0x40];assert_eq!(&opening[..4],&0x16u32.to_le_bytes());assert!(opening[4..].iter().all(|v|*v==0));
'''
        code += '}\n'
        manifest.append({'case': case, 'entropy': entropy, 'RAM_spans': len(spans),
                         'RAM_bytes': sum(s[1] for s in spans), 'alias_spans': len(aliases),
                         'alias_pages': sum(a[2] // 0x4000 for a in aliases)})
    code += '}\n'
    for mask in [1 << i for i in range(32)] + [0xffffffff]:
        p = copy.copy(baseline)
        p.properties = dict(properties, **{'active-mcc-bit-vector': struct.pack('<I', mask)})
        p._registers()
        maps = list(p.register_mappings()) + [(0xfffffc218100c000, p.sgx_base+0xd60000,
                                              0x20000, 0x00c000000000044b)]
        expected[f'mask-{mask:x}-maps.bin'] = b''.join(struct.pack('<QQQQ', *m) for m in maps)
        expected[f'mask-{mask:x}-registers.bin'] = b''.join(
            struct.pack('<QQQIIQII', r.slot, r.physical, r.address, r.size, r.stride, r.relative, r.flags, 0)
            for r in p.registers)
    (tmp / 'main.rs').write_text(code)
    subprocess.run([args.rustc, '--edition=2021', '-Dwarnings', str(tmp / 'main.rs'),
                    *flags, '-o', str(tmp / 'gate')], check=True)
    subprocess.run([str(tmp / 'gate'), str(tmp / 'images')], check=True)
    for name, want in expected.items():
        actual = (tmp / 'images' / name).read_bytes()
        assert actual == want, (name, len(actual), len(want), [
            (hex(i), a, b) for i, (a, b) in enumerate(zip(actual, want)) if a != b][:16])
    summary = {'cases': manifest, 'whole_images_and_manifests': len(expected),
               'invalid_geometry_rejections_before_mutation': 80,
               'invalid_typed_mmio_properties': 9, 'additional_mcc_mask_cases': 33,
               'control_ring_aliases_main_config': True, 'native_execution_tested': False,
               'input_sha256': hashlib.sha256(args.inputs.read_bytes()).hexdigest(),
               'fdt_template_sha256': hashlib.sha256(args.fdt_template.read_bytes()).hexdigest(),
               'images': {name: {'bytes': len(b), 'sha256': hashlib.sha256(b).hexdigest()}
                          for name, b in expected.items()}}
    if args.evidence:
        args.evidence.mkdir(parents=True, exist_ok=False)
        for name, data in expected.items():
            (args.evidence / name).write_bytes(data)
        (args.evidence / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print('PASS: 4 full source-built Max boot graphs; 144 complete RAM owners; '
      '36 RAM spans / 8 aliases / 283 shared pages per graph; '
      '80 rejection-before-mutation checks; 33 extra complete MCC tables/maps; '
      '9 invalid typed MMIO properties; opening slot shares main storage')
