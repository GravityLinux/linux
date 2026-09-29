#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Export only the authored eight-target graphics caller for the native DRM test.

Execute selected pure constructors, never the experiment's proxy setup. The
explicit bundle contains our own Metal compiler/resource output; its loader
verifies every page hash and excludes the source-built driver auxiliary page.
"""
import argparse
import ast
import hashlib
import importlib
import json
from pathlib import Path
import struct
import sys
import types

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1',type=Path)
parser.add_argument('output',type=Path)
args=parser.parse_args()
root=args.m1n1.resolve()
package=types.ModuleType('render_caller')
package.__path__=[str(root/'proxyclient/m1n1/agx')]
sys.modules[package.__name__]=package
render=importlib.import_module('render_caller.g17p_render')
encoder=importlib.import_module('render_caller.g17p_encoder')
uapi=importlib.import_module('render_caller.g17p_uapi')
source=root/'proxyclient/experiments/agx_g17p_modern_partial.py'
constants=set('CONTEXT_BASE ENCODER DRAW_STATE BIND_GROUP VIEWPORT USC_BASE RESOURCE DIMENSIONS SCISSOR DBIAS OUTPUT_SIZE OUTPUTS PIXELS TRIANGLES MAX_TRIANGLES MAX_ARITHMETIC_TRIANGLES PAYLOAD CONSTANT_PAYLOAD OBSERVATION FRAGMENT_ARGUMENTS'.split())
functions=set('output_bases caller_profile build_encoder build_caller_pages partial_load_clear render_command'.split())
selected=[]
for node in ast.parse(source.read_text()).body:
    if isinstance(node,ast.Assign) and any(isinstance(t,ast.Name) and t.id in constants for t in node.targets): selected.append(node)
    elif isinstance(node,ast.FunctionDef) and node.name in functions: selected.append(node)
scope=dict(__file__=str(source),PAGE=0x4000,Path=Path,json=json,struct=struct,hashlib=hashlib,g17p_render=render,g17p_encoder=encoder,uapi=uapi)
exec(compile(ast.Module(body=selected,type_ignores=[]),str(source),'exec'),scope)
pages=scope['build_caller_pages'](scope['CONSTANT_PAYLOAD'],constant=True,triangles=1,viewport_x_shift=2)
outputs=scope['OUTPUTS']
stream=scope['render_command'](None)
parsed,=uapi.parse_command_buffer(stream)
assert len(parsed.payload.to_bytes())==240
text=['/* Own-source compiler/resources and pure generated caller data only. */',
      'struct workload { uint64_t address; size_t size; const unsigned char *data; int writable; };',
      '#define RENDER_WIDTH 128', '#define RENDER_HEIGHT 128', '#define RENDER_OUTPUT_SIZE 0x10000',
      '#define RENDER_PIXEL_0 0x7f04', '#define RENDER_PIXEL_1 0x7f08']
for index,(address,body) in enumerate(sorted(pages.items())):
    text.append(f'static const unsigned char render_data_{index}[{len(body)}] = {{')
    values=[f'[{i}]=0x{value:02x}' for i,value in enumerate(body) if value]
    text.extend('  '+','.join(values[i:i+12])+',' for i in range(0,len(values),12))
    text.append('};')
text.append('static const struct workload workloads[] = {')
for index,(address,body) in enumerate(sorted(pages.items())):
    text.append(f'  {{0x{address:x}ULL,{len(body)},render_data_{index},{int(address in outputs)}}},')
text.append('};')
text.append('static const uint64_t render_outputs[] = {'+','.join(f'0x{a:x}ULL' for a in outputs)+'};')
text.append('static const unsigned char render_command[] = {'+','.join(f'0x{v:02x}' for v in stream)+'};')
args.output.write_text('\n'.join(text)+'\n')
print(f'Generated {args.output}: {len(pages)} caller buffers, {sum(map(len,pages.values()))} bytes, 8 independent 128x128 R32F targets, exact two-pixel oracle; USC {scope["USC_BASE"]:#x}')
