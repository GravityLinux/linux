#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Compare complete Rust render objects with the current source-only Python builders.

No hardware imports, captures, or firmware binaries. A bounded in-memory allocator
executes the actual G17PWorkBuilder.item method, including its structural tails.
"""
import argparse
from dataclasses import MISSING, fields
import importlib
from pathlib import Path
import random
import struct
import subprocess
import sys
import tempfile
import types

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1', type=Path)
parser.add_argument('--rustc', default='rustc')
args = parser.parse_args()
package = types.ModuleType('render_reference')
package.__path__ = [str(args.m1n1 / 'proxyclient/m1n1/agx')]
sys.modules[package.__name__] = package
ref = importlib.import_module('render_reference.g17p_render')
backend = importlib.import_module('render_reference.g17p_backend')
submission = importlib.import_module('render_reference.g17p_submission')
expected = []
inputs = bytearray(struct.pack('<Q', 192))
rng = random.Random(0x814017)

def emit(name, body):
    expected.append((name, bytes(body)))

def registers(values):
    return b''.join(struct.pack('<IQ', n, v) for n, v in values)

for case in range(192):
    required = {f.name: 0 for f in fields(ref.G17PRenderParameters)
                if f.default is MISSING}
    p = ref.G17PRenderParameters(**required)
    values = {f.name: getattr(p, f.name) for f in fields(p)}
    # Vary every address and scalar, not only the compact first-work profile.
    values.update(width=(1, 32, 128, 16384, 129, 37)[case % 6],
        height=(37, 128, 16384, 1, 33, 129)[case % 6],
        context_base=0x1000000000 + case*0x4000000,
        layers=(1, 2, 7, 2048)[case % 4],
        utile_width=(32,32,16)[case % 3], utile_height=(32,16,16)[case % 3],
        samples=(1,2,4)[case % 3], sample_size=case%17,
        queue_pair=case%4, queue_item_index=case%37,
        lifecycle_ordinal=case*3, native_context_slot=(None,1,2)[case%3],
        status_queue_pair=None if case%2==0 else (case+1)%4,
        status_item_index=None if case%2==0 else case%19,
        native_item_fields=case%13==0, native_pair_registers=case%7==0,
        native_cycle_registers=case%5==0, native_record_index_register=case%6==0,
        native_status_registers=case%3==0, local_item_registers=case%2==0,
        pair_resource_stride=0x5e0000+case*0x4000,
        sampler_array=0 if case%2==0 else 0x10000080000+case*8,
        sampler_count=0 if case%2==0 else case%1024+1,
        process_empty_tiles=case%2==0, fragment_sync_grow=(None,False,True)[case%3],
        reactive_tvb_growth=case%2!=0, tvb_pool_id=(None,0,1)[case%3],
        emit_uapi_fields=case%2==0, vertex_store_flag=case%2==0,
        fragment_store_flag=case%3==0)
    for name in ('tilemap','heapmeta','tpc','deflake_1','deflake_2','deflake_3','encoder'):
        values[name]=values['context_base']+rng.randrange(0x100,0x10000000)//4*4
    for name in ('ta_status','fragment_status','scissor_array','depth_bias_array','aux_fb',
                 'occlusion_query_base','depth_buffer','stencil_buffer','depth_aux_buffer','stencil_aux_buffer'):
        values[name]=0x10000000000+rng.randrange(0x10000000)//8*8
    for name in ('store_pipeline','load_pipeline','partial_store_pipeline','partial_load_pipeline',
                 'depth_stride','stencil_stride','depth_aux_stride','stencil_aux_stride','aux_fb_page_count',
                 'multisample_control','utile_config','ppp_control','tile_config','aux_fb_flags',
                 'store_pipeline_bind','load_pipeline_bind','partial_store_pipeline_bind','partial_load_pipeline_bind',
                 'merge_upper_x_bits','merge_upper_y_bits','depth_clear_value_bits','stencil_clear_value','depth_flags','depth_dimensions'):
        values[name]=rng.randrange(1<<32)
    values['tib_blocks']=(1,4,8,16,64)[case%5]
    for name in ('timestamp_a','timestamp_b','ta_timestamp_end','fragment_timestamp_start',
                 'fragment_timestamp_end','ta_user_timestamp_start','ta_user_timestamp_end',
                 'fragment_user_timestamp_start','fragment_user_timestamp_end'):
        values[name]=0 if case%7==0 else 0xfffffc2100000000+rng.randrange(0x1000000)//8*8
    p=ref.G17PRenderParameters(**values)
    for f in fields(p):
        value=getattr(p,f.name)
        inputs.extend(struct.pack('<Q', (1<<64)-1 if value is None else int(value)))
    ta, frag = ref.build_tiling_registers(p), ref.build_fragment_registers(p)
    for name, r in [('ta',ta),('fragment',frag),('partial-store',ref.build_fragment_partial_store_registers(p)),
                   ('partial-resume',ref.build_fragment_partial_resume_registers(p)),
                   ('partial-load',ref.build_fragment_partial_load_registers(p))]:
        emit(f'{case}:{name}',registers(r))
    emit(f'{case}:class4-program',ref.build_render_class4_register_program(frag))
    operand=0x7000048000+case*0x4000
    firmware=0xfffffc20c0998000+case*0x4000
    emit(f'{case}:class2',ref.build_render_class2_prestate(operand,firmware,0x7000058080+case*16,case%4))
    emit(f'{case}:class4-before',ref.build_render_class4_prestate(operand,firmware,case%3))
    emit(f'{case}:class4-after',ref.build_render_class4_observed_state(operand,firmware,case%3))
    for kind, r in [('tiling',ta),('fragment',frag)]:
        r=list(r)
        if case%3==1: r.append((0x1ca10 if kind=='tiling' else 0x160e0,0x123456789abcdef0))
        if case%7==2: r.append((0x10111 if kind=='tiling' else 0x15131,0xfeed00881234))
        bodies=[]
        def alloc(size,name):
            assert size==backend.G17PWorkBuilder.BODY_STRIDE[kind]
            return 0xfffffc20c0500000
        def write(at,body):
            assert at==0xfffffc20c0500000
            bodies.append(bytes(body))
        b=backend.G17PWorkBuilder(alloc,write,kind,p.queue_pair)
        b.use_pools(0xfffffc20c0834000,0xfffffc20c0858000)
        b.write_tail=case%11!=0
        b.write_lifecycle_fields=case%4!=1
        b.write_item_fields=case%4!=2
        b.write_structural_tail=case%4!=3
        if case%3: b.low_alias={kind:0x7002000000+case*0x4000}
        if case%2:
            b.status_base=0xfffffc2001770000+case*0x4000
            b.tail_pointer_overrides={0x934 if kind=='tiling' else 0x21ce:firmware,
                0x8a6 if kind=='tiling' else 0x2140:0xfffffc2000120000+case*8}
            b.item_field_overrides={0x8c4 if kind=='tiling' else 0x2160:case*513+1}
        b.item(case%37,(0xfffffc20c0920000,0xfffffc20c0924000),r,0,0,
            context_id=1 if p.native_context_slot==1 else 2 if p.native_context_slot==2 else case%4,
            record_indices=(case*2,case*3),submission_ordinal=case*3,queue_pair=p.queue_pair,
            parameters=None if case%5==0 else p,submit_sequence=case<<40|case*2+int(kind=='tiling'),
            queue_grid_index=None if case%3==0 else case%12)
        assert len(bodies)==1
        emit(f'{case}:{kind}-descriptor',bodies[0])
for dimension in range(1,16385):
    emit(f'merge:{dimension}',struct.pack('<ff',ref.MERGE_SCALE/dimension,ref.MERGE_SCALE/(16385-dimension)))
emit('aux-fb',ref.build_aux_fb())
with tempfile.TemporaryDirectory() as tmp:
    binary=Path(tmp)/'render'
    subprocess.run([args.rustc,'--edition=2021','-Dwarnings',str(Path(__file__).with_name('g17p_render.rs')),'-o',str(binary)],check=True)
    actual=subprocess.check_output([str(binary)],input=inputs)
offset=0
for name, body in expected:
    chunk=actual[offset:offset+len(body)]
    if chunk!=body:
        differences=[(hex(i),hex(a),hex(b)) for i,(a,b) in enumerate(zip(chunk,body)) if a!=b][:16]
        raise AssertionError((name,differences,len(chunk),len(body)))
    offset+=len(body)
assert offset==len(actual),(offset,len(actual))
print(f'PASS: {len(expected)} complete render objects/scalars ({offset} bytes), 192 varied TA/fragment pairs, ordered partial programs, source class-2/4 states, all 16384 merge dimensions, bounds/USC rejection and unchanged output on serialization errors')
