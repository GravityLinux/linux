#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Compare Rust opening objects with the actual Python partial-opening builders.

Compile only selected AST definitions/constants: no experiment module setup,
firmware import, hardware access, captures, shaders or workload fixtures.
"""
import argparse
import ast
import contextlib
import importlib.util
import io
from pathlib import Path
import struct
import subprocess
import tempfile
from types import SimpleNamespace as NS

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1', type=Path)
parser.add_argument('--rustc', default='rustc')
args = parser.parse_args()
root = args.m1n1 / 'proxyclient'
source = root / 'experiments/agx_g17p_boot.py'
parsed = ast.parse(source.read_text())
consts = set('INITIAL_QUEUE_CONTEXT_PAGES CONTEXT NATIVE_FIRMWARE_SLOT NATIVE_FIRMWARE_CONTEXT NATIVE_RENDER_SLOT NATIVE_RENDER_CONTEXT CONTEXT_QUEUE_WORDS CONTEXT_QUEUE_ADDRESSES PARTIAL_OPENING_SHARED_CONTROL_ADDRESS PARTIAL_OPENING_SHARED_CONTROL_INNER_ADDRESS CHANNEL_CONTROL_ADDRESS CHANNEL_CONTROL_STRIDE CHANNEL_CONTROL_RECORDS CHANNEL_CONTROL_ITEM_RECORD CHANNEL_CONTROL_WORDS CONTROL_OPERAND_TABLE_VA COMPUTE_BINDING_OPERAND_TABLE_VA COMPUTE_CLASS2_SUPPORT_TABLE_VA CONTROL_OPERAND_ENTRIES CONTROL_OPERAND_ENTRIES_RUNTIME PARTIAL_CONTROL_OPERAND_ENTRIES CONTROL_OPERAND_BUFFER_BASE CONTROL_OPERAND_BUFFER_STRIDE CONTROL_OPERAND_BUFFER_SIZE PARTIAL_SHARED_CONTROL_COUNT_AFTER PARTIAL_SHARED_CONTROL_COUNT_BEFORE'.split())
functions = set('stage_device_control prepare_final_26_6_opening_control build_context_queue_state bind_contexts'.split())
env = dict(PARTIAL_OPENING_GRAPH=True, PAGE=0x4000, struct=struct,
    os=NS(getenv=lambda key: '1' if key in ('G17P_PARTIAL_OPENING_GRAPH', 'G17P_FINAL_26_6_CONTROL_LIFECYCLE', 'G17P_SOURCE_PRESENT_PRIMARY_CONTROL_DONE') else None),
    LOW_ALIAS_FLAGS={}, NORMAL_OBJECT_FLAGS={}, RENDER_SNAPSHOT_ROOT=2,
    MemoryAttr=NS(Shared=2))
selected = []
for n in parsed.body:
    if isinstance(n, ast.Assign) and any(isinstance(t, ast.Name) and t.id in consts for t in n.targets):
        selected.append(n)
    elif isinstance(n, ast.If) and isinstance(n.test, ast.Name) and n.test.id == 'PARTIAL_OPENING_GRAPH':
        selected.extend(x for x in n.body if isinstance(x, ast.Assign) and any(isinstance(t, ast.Name) and t.id == 'CONTEXT_QUEUE_WORDS' for t in x.targets))
    elif isinstance(n, ast.FunctionDef) and n.name in functions:
        selected.append(n)
exec(compile(ast.Module(body=selected, type_ignores=[]), str(source), 'exec'), env)

def load(name):
    path = root / 'm1n1/agx' / (name + '.py')
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

env['g17p'] = g17p = load('g17p')
compute = ast.parse((root / 'm1n1/agx/g17p_compute.py').read_text())
ns = dict(struct=struct, COMPUTE_SHARED_SUPPORT_SIZE=0x4000)
exec(compile(ast.Module(body=[n for n in compute.body if isinstance(n, ast.FunctionDef) and n.name == 'build_compute_compact_control_support'], type_ignores=[]), 'g17p_compute.py', 'exec'), ns)
env['g17p_compute'] = NS(**ns)

class Ram:
    def __init__(self):
        self.pages, self.mappings, self.entries = {}, {}, []
        self.next = 0x10080000000
    def memalign(self, align, size):
        assert align == 0x4000 and size % align == 0
        pa = self.next
        self.next += size
        for at in range(pa, pa + size, align): self.pages[at] = bytearray(align)
        return pa
    def writemem(self, address, data):
        for offset, byte in enumerate(data):
            at = address + offset
            self.pages[at & -0x4000][at & 0x3fff] = byte
    def write32(self, at, value): self.writemem(at, struct.pack('<I', value))
    def write64(self, at, value): self.writemem(at, struct.pack('<Q', value))
    def memset32(self, at, value, size):
        assert value == 0
        self.writemem(at, bytes(size))
    def alloc_at(self, va, size, name, data=None, flags=None):
        pa = self.memalign(0x4000, size)
        self.iomap_at(1, va, pa, size)
        if data is not None: self.writemem(pa, data)
        return va, pa
    def iomap_at(self, ctx, va, pa, size, **flags):
        for offset in range(0, size, 0x4000): self.mappings[va + offset] = pa + offset
    def physical(self, va): return self.mappings.get(va)
    def dc_civac(self, *args): pass
    def inst(self, *args): pass
    def flush_dirty(self): pass
    def body(self, va): return bytes(self.pages[self.mappings[va]])

ram = Ram()
env.update(p=ram, u=ram, iface=ram)
capture = NS(flags=lambda va, default: default, flags_for_root=lambda root, va, default: default)
expected = {}
with contextlib.redirect_stdout(io.StringIO()):
    env['build_context_queue_state'](ram, ram, capture)
    for kind, name in enumerate(('tiling', 'fragment')):
        expected['context' + str(kind)] = ram.body(env['CONTEXT_QUEUE_ADDRESSES'][name]['high'])
    expected['channel'] = ram.body(env['CHANNEL_CONTROL_ADDRESS'])
    instances = []
    for slot in range(2):
        ring = ram.memalign(0x4000, 0x4000)
        counters = [ram.memalign(0x4000, 0x4000) for _ in range(3)]
        instances.append(dict(name=str(slot), control_ring_pa=ring, channel_state_pas=[None]*12+[counters]))
    env['stage_device_control'](ram, capture, instances, opening='done')
    for slot, entry in enumerate(instances):
        expected['message' + str(slot)] = bytes(ram.pages[entry['control_ring_pa']][:0x40])
        assert [struct.unpack_from('<I', ram.pages[at])[0] for at in entry['channel_state_pas'][12]] == [1,1,1]
    for va in (0xfffffc20c0828000, env['PARTIAL_OPENING_SHARED_CONTROL_INNER_ADDRESS']):
        ram.alloc_at(va, 0x4000, 'support')
    env['prepare_final_26_6_opening_control'](ram)
    expected['support'] = ram.body(0xfffffc20c0828000)[:0x70]
    assert ram.body(env['PARTIAL_OPENING_SHARED_CONTROL_INNER_ADDRESS'])[:4] == struct.pack('<I', 2)

# Run the current render entrypoint's post-ACK status writes too.
status_ast = ast.parse((root / 'm1n1/agx/g17p_render_startup.py').read_text())
status_ns = dict(struct=struct)
exec(compile(ast.Module(body=[n for n in status_ast.body if isinstance(n, ast.FunctionDef) and n.name == 'prepare_render_status_config'], type_ignores=[]), 'g17p_render_startup.py', 'exec'), status_ns)
status_pa = ram.memalign(0x4000, 0x10000)
base = 0xfffffc2000000000
status_ns['prepare_render_status_config'](NS(g17p=g17p, iface=ram, p=ram, u=ram), [dict(state_va=base+g17p.NATIVE_PRIMARY_WORK_STATE_OFFSET, status_b_pa=status_pa)])
expected['status'] = b''.join(bytes(ram.pages[status_pa+offset][:0x4000]) for offset in range(0,0x10000,0x4000))

# Actual bind_contexts control flow with preexisting nonzero slots. Context-0
# gets its independent low root after this step, exactly as the Rust VM does.
class Uat:
    NUM_CONTEXTS=64
    ttbr0_base=0x10057ba0000
    ttbr1_base=0x10021598000
    gpu_region=ram.memalign(0x4000, 0x4000)
    def bind_context(self, *args): pass
    def set_l0(self, slot, which, root, tag): ram.write64(self.gpu_region+slot*16+which*8, root | (tag << 48) | 1)
    def flush_dirty(self): pass
    def invalidate_cache(self): pass
uat=Uat()
ram.writemem(uat.gpu_region, bytes([255])*1024)
with contextlib.redirect_stdout(io.StringIO()): env['bind_contexts'](uat, macos_table=True)
uat.set_l0(0,0,0x10034bcc000,0)
for slot in range(64):
    words=struct.unpack_from('<QQ',ram.pages[uat.gpu_region],slot*16)
    assert words == ((0x10034bcc000|1,uat.ttbr1_base|1) if slot==0 else ((1<<48)|uat.ttbr0_base|1,(1<<48)|uat.ttbr1_base|1) if slot==1 else (0,0))

# Execute the kernel's exact core serializer module on the host.
module = Path(__file__).resolve().parent.parent / 'g17p_opening.rs'
harness = '''#![allow(dead_code)]
#[path = "MODULE"] mod opening;
fn emit(name: &str, bytes: &[u8]) { print!("{name} "); for b in bytes { print!("{b:02x}"); } println!(); }
fn main() {
 for i in 0..2 { let mut page=vec![0; 0x4000]; page[..0x380].copy_from_slice(&opening::context(i).unwrap()); emit(&format!("context{i}"), &page); emit(&format!("message{i}"), &opening::message(i==1)); }
 let mut page=vec![0;0x4000]; page[..0x40].copy_from_slice(&opening::channel_control()); emit("channel", &page);
 emit("support", &opening::support());
 let mut status=vec![0;0x10000]; for (offset,value) in opening::status_config(0xfffffc20001c0000) { status[offset..offset+8].copy_from_slice(&value.to_le_bytes()); } emit("status", &status);
}
'''.replace('MODULE',str(module))
with tempfile.TemporaryDirectory() as temp:
    src=Path(temp)/'opening.rs'; src.write_text(harness)
    binary=Path(temp)/'opening'
    subprocess.run([args.rustc,'--edition=2021',str(src),'-o',str(binary)],check=True)
    actual={name:bytes.fromhex(data) for name,data in (line.split() for line in subprocess.check_output([str(binary)],text=True).splitlines())}
assert actual.keys()==expected.keys()
for name in expected: assert actual[name]==expected[name], name
print(f'PASS: {len(expected)} complete source-built opening objects, presented counters and exact Python context-table control flow')
