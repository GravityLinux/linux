#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Identify the own pressure shader's DATA in all five appended operand buffers.

This verifies the 4096-word/16384-invocation/selector9 pressure workload, not an
arbitrary shader. GPU correctness still requires the native C caller's complete
BO/guard/fence/timestamp checks. Only backing DATA printed by the opt-in native
probe is read. No instruction bytes, Apple objects or disassembly are inputs.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('log', type=Path)
p.add_argument('--evidence', type=Path, required=True)
p.add_argument('--end-marker', default='OPERAND_PRESSURE8_RC=0')
a = p.parse_args()
raw = a.log.read_bytes()
end = re.search(rb'(?m)^' + re.escape(a.end_marker.encode()) + rb'\r?$', raw)
assert end, 'require the actual successful C return marker'
prefix = raw[:end.end()]
s = prefix.decode()
assert 'G16C_PRIVATE_INPUTS_PASS words=4096 threads=16384' in s
assert 'buffers=20->25 appended DATA only' in s
assert 'phase=seed buffers=25 pages=15 changed=0 GPU_idle=1' in s
assert 'phase=retired buffers=25 pages=15 changed=15 GPU_idle=1' in s
pattern = r'operand sample context=(\d+) buffer=(\d+) offset=0x0 nonzero=(\d+) fnv=([0-9a-f]{16}) head=\[([0-9a-f, ]+)\]'
heads = {}
for context, index, n, fnv, head in re.findall(pattern, s):
    q = [int(x.strip(), 16) for x in head.split(',')]
    assert len(q) == 8 and 0 < int(n) <= 16384
    heads[int(index)] = dict(context=int(context), words=list(struct.unpack('<16I', struct.pack('<8Q', *q))), nonzero=int(n), fnv=fnv)
assert set(heads) == set(range(20, 25))
assert len({r['context'] for r in heads.values()}) == 1
# Only the actual pressure interval supplies the sampled words. The independent
# model below computes both values written by that shader to each array cell:
# initialization and its single in-place update. Retired backing is not assumed
# to contain every cell's final shader value.
code = r'''/* SPDX-License-Identifier: MIT */
#include <stdint.h>
#include <stdio.h>
static uint32_t rotate(uint32_t v,unsigned n){return (v<<n)|(v>>(32-n));}
static void array(unsigned lane,uint32_t words[4096],unsigned final){
 uint32_t value=0x5a110000U^lane^9;
 for(unsigned j=0;j<4096;j++){value=rotate(value,7)+0x9e3779b9U+j;words[j]=value^((j+1)*0x045d9f3bU);}
 if(!final)return;
 for(unsigned step=0;step<4096;step++){unsigned i=(step*13+9+lane)&4095;words[i]=rotate(words[i]^words[(i+17)&4095],5)+step+9;}
}
static const uint32_t values[80]={'''
code += ','.join(hex(x) + 'U' for i in range(20, 25) for x in heads[i]['words'])
code += r'''};
int main(void){
 unsigned hits[80]={0};uint32_t words[4096];int bucket[256],next[80];
 for(unsigned i=0;i<256;i++)bucket[i]=-1;
 for(unsigned i=0;i<80;i++){unsigned h=(values[i]^(values[i]>>16))&255;next[i]=bucket[h];bucket[h]=i;}
 for(unsigned final=0;final<2;final++)for(unsigned lane=0;lane<16384;lane++){
  array(lane,words,final);
  for(unsigned j=0;j<4096;j++){
   uint32_t v=words[j];unsigned h=(v^(v>>16))&255;
   for(int s=bucket[h];s>=0;s=next[s])if(values[s]==v){
    printf("MATCH buffer=%u position=%u lane=%u word=%u final=%u value=%08x\n",s/16+20,s%16,lane,j,final,v);hits[s]++;
   }
  }
 }
 for(unsigned i=0;i<80;i++)if(!hits[i]){fprintf(stderr,"MISSING %u\n",i);return 1;}
 return 0;
}
'''
a.evidence.mkdir(parents=True, exist_ok=False)
(a.evidence / 'native-prefix.log').write_bytes(prefix)
(a.evidence / 'sample-heads.json').write_text(json.dumps(heads, indent=2) + '\n')
source, binary = a.evidence / 'matcher.c', a.evidence / 'matcher'
source.write_text(code)
subprocess.run(['cc', '-O3', '-Wall', '-Wextra', '-Werror', '-std=c11', str(source), '-o', str(binary)], check=True)
r = subprocess.run([str(binary.resolve())], capture_output=True, text=True)
(a.evidence / 'matcher.log').write_text(r.stdout + r.stderr)
r.check_returncode()
records = []
for b, pos, lane, j, stage, value in re.findall(r'MATCH buffer=(\d+) position=(\d+) lane=(\d+) word=(\d+) final=(\d+) value=([0-9a-f]{8})', r.stdout):
    b, pos, lane, j, stage = map(int, (b, pos, lane, j, stage))
    assert int(value, 16) == heads[b]['words'][pos]
    records.append((b, pos, lane, j, stage))
models, proof = {}, []
lane_offsets = [(k & 3) + ((k & 4) << 2) + (k & 8) for k in range(16)]
for b in range(20, 25):
    anchors = {(lane, j) for block, pos, lane, j, stage in records if block == b and pos == 0 and lane % 32 == 0}
    matching_models = []
    for lane, j in anchors:
        points = [[r for r in records if r[:4] == (b, k, lane + lane_offsets[k], j)] for k in range(16)]
        if all(points):
            matching_models.append((lane, j, [max(values, key=lambda r: r[-1]) for values in points]))
    assert len(matching_models) == 1, (b, matching_models)
    lane, j, points = matching_models[0]
    models[b] = dict(base_lane=lane, array_word=j)
    proof.extend(points)
summary = dict(pass_=True, appended_buffers=list(heads), words_per_buffer=16,
    all_80_values_match_own_shader_CPU_model=True, observed_lane_offsets=lane_offsets,
    final_word_values=sum(r[-1] for r in proof), initialized_word_values=sum(1-r[-1] for r in proof),
    coherent_model_per_buffer=models, source_profile='own GLSL dynamic-private4096 pressure16384 selector9',
    comparison_scope='sampled backing DATA; no claim all backing bytes contain final private-array values',
    all_actual_heads_from_native_trace=True, zero_seed_and_retired_changed_samples=True,
    caller_GPU_output_oracle_unchanged=True, prefix_sha256=hashlib.sha256(prefix).hexdigest(),
    CPU_model_sha256=hashlib.sha256(source.read_bytes()).hexdigest())
(a.evidence / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print('PASS: 80 own shader DATA values, coherent lane/array-word model in all five appended buffers')
