#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Check every typed render field and complete source/native descriptor DATA.

Production Rust lowers commands to programs and body fields. Qualified
Python supplies expected bytes; this gate does not execute a GPU draw.
"""
import argparse
from dataclasses import replace
import hashlib
import json
from pathlib import Path
import random
import re
import subprocess
import sys
import tempfile
import host_versions

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('m1n1',type=Path)
parser.add_argument('--evidence',type=Path,required=True)
a=parser.parse_args()
tests=Path(__file__).resolve().parent
sys.path.insert(0,str(a.m1n1/'proxyclient'))
sys.path.insert(0,str(a.m1n1/'proxyclient/experiments'))
from m1n1.g16c.render import (Parameters,tiling_registers,fragment_registers,
    partial_store_registers,partial_resume_registers,partial_load_registers)
from m1n1.g16c.descriptor import RenderState,encode_descriptor
from agx_g16c_native_tiling import build_tiling_source
from agx_g16c_native_fragment import build_fragment_source

common_fields=set(re.findall(r'pub\(crate\) (\w+):',(tests.parent/'g17p_render.rs').read_text().split('pub(crate) struct Parameters {',1)[1].split('\n}',1)[0]))
state_fields=set(re.findall(r'pub\(crate\) (\w+):',(tests.parent/'scheduled_render_program.rs').read_text().split('pub(crate) struct ProgramStateG16C {',1)[1].split('\n}',1)[0]))

cases=[];lines=[];native={}
rng=random.Random(0x60418140)
bool_value=lambda x:'true' if x else 'false'
option=lambda x:'None' if x is None else f'Some({x})'
registers=lambda x:'&['+','.join(f'({n},{v})' for n,v in x)+']'

def emit(name,p,s,fragment,item,grid,captured=None):
    expected=encode_descriptor(p,s,fragment=fragment,item=item,grid=grid)
    if captured is not None:
        assert expected==captured,(name,'source differs from independent native DATA')
    c={k:v for k,v in vars(p).items()if k in common_fields and k!='usc_exec_base'}
    c.update(usc_exec_base=1<<40,emit_uapi_fields=p.usc_exec_base is not None,
             reactive_tvb_growth=p.request_tvb_growth)
    fields=','.join(f'{k}:{option(bool_value(v)if isinstance(v,bool)else v)if k=="fragment_sync_grow"else bool_value(v)if isinstance(v,bool)else v}'for k,v in c.items())
    lines.append('let command=g17p_render::Parameters{'+fields+',..Default::default()};')
    optional={'tiling_record_index','partial_load_tile_config','partial_load_depth_flags','partial_resume_depth_flags'}
    fields=','.join(f'{k}:{option(getattr(p,k))if k in optional else getattr(p,k)}'for k in state_fields)
    lines.append('let program_state=program::ProgramStateG16C{'+fields+'};')
    lines.append(f'let mut p=program::descriptor_parameters(&command,&program_state,{p.tpc_size}).unwrap();')
    values={k:getattr(s,k) for k in ('context','pool','event_control','alias','shared_tail',
        'driver_stamp','firmware_stamp','support','counter','stamp','event','uuid','timestamp',
        'timestamp_end','user_timestamp_start','user_timestamp_end','fragment_event','fragment_stamp',
        'stage_flag','field_93c','field_2128','field_21d8','field_222c','secondary_stamp',
        'descriptor_ordinal','work_index','tail_flags','tiling_submission_tag','tiling_queue_ordinal')}
    values.update(manager=s.buffer_manager,scene=s.buffer_thing,empty_scene=s.empty_buffer)
    opts=('timestamp_end','secondary_stamp','descriptor_ordinal','work_index')
    fields=','.join(f'{k}:{option(v) if k in opts else v}' for k,v in values.items())
    lines.append(f'let mut s=render::State{{{fields}}};')
    if fragment:
        lines.append('let primary=program::FragmentProgramG16C::new(&command,&program_state).unwrap();'
            'let store=program::partial_store(&command,&program_state).unwrap();'
            'let resume=program::partial_resume(&command,&program_state).unwrap();'
            'let load=program::partial_load(&command,&program_state).unwrap();'
            'let mut programs=render::Programs{primary:primary.registers(),'
            'partial_store:&store,partial_resume:&resume,partial_load:&load};')
        kind='Fragment'
        call=f'render::{kind}DescriptorG16C::write(&mut out,&p,&s,&programs,{item},{grid})'
    else:
        lines.append('let primary=program::TilingProgramG16C::new(&command,&program_state).unwrap();let primary=primary.registers();')
        kind='Tiling'
        call=f'render::{kind}DescriptorG16C::write(&mut out,&p,&s,primary,{item},{grid})'
    lines.append(f'let mut out=vec![0xa5;render::{kind}DescriptorG16C::SIZE];{call}.unwrap();'
                 'std::io::stdout().write_all(&out).unwrap();')
    # Verify errors cannot partly overwrite existing caller staging, including
    # errors found after all other input fields have been checked.
    lines.append('out.fill(0xa5);let seeded=out.clone();')
    for field,bad in [('context','0'),('pool','15'),('event','128'),('alias','u64::MAX')]:
        lines.append(f'let saved=s.{field};s.{field}={bad};assert!({call}.is_err());'
                     f'assert_eq!(out,seeded);s.{field}=saved;')
    for field,bad in [('sampler_count','u32::MAX'),('sampler_array','3'),('width','0')]:
        lines.append(f'let saved=p.{field};p.{field}={bad};assert!({call}.is_err());'
                     f'assert_eq!(out,seeded);p.{field}=saved;')
    # Missing late required register and address/time arithmetic overflow.
    if fragment:
        lines.append(f'programs.primary=&[(0x160e0,0),(0x16429,0),(0x10019,0),(0x100b1,0)];'
                     f'assert!({call}.is_err());assert_eq!(out,seeded);')
    else:
        lines.append('let primary=&[(0x1ca10,0),(0x10111,0x100000000)];'
                     f'assert!({call}.is_err());assert_eq!(out,seeded);')
    cases.append((name,expected))

for i in range(128):
    base=0x1000000000+i*0x4000000
    p=Parameters(width=(1,2,32,33,1970,16384)[i%6],height=(1,32,33,1123,16384)[i%5],
        layers=(1,2,7,2048)[i%4],samples=(1,2,4)[i%3],utile_width=(32,32,16)[i%3],
        utile_height=(32,16,16)[i%3],tib_blocks=(0,1,4,32,63,64,128)[i%7],
        context_base=base,scene_slot=i%80,sampler_array=0 if i%2==0 else base+0x100000,
        sampler_count=0 if i%2==0 else (1,17,65537,0xfffffffe)[i%4],
        request_tvb_growth=i%3==0,fragment_sync_grow=i%3==1,process_empty_tiles=i%3==2,
        usc_exec_base=None if i%2==0 else 1<<40)
    values={}
    for k in ('tilemap','heapmeta','tpc','deflake_1','deflake_2','deflake_3','encoder'):
        values[k]=base+rng.randrange(0x100,0x100000)//4*4
    for k in ('depth_bias_array','scissor_array','occlusion_query_base','load_pipeline',
        'partial_load_pipeline','depth_buffer','depth_aux_buffer','stencil_buffer','stencil_aux_buffer',
        'store_pipeline','partial_store_pipeline'):
        values[k]=0x10000000000+rng.randrange(0x1000000)//8*8
    for k in ('load_pipeline_bind','partial_load_pipeline_bind','depth_flags','depth_stride',
        'depth_aux_stride','stencil_stride','stencil_aux_stride','aux_fb_flags','aux_fb_page_count',
        'tile_config','store_pipeline_bind','partial_store_pipeline_bind','depth_clear_value_bits',
        'stencil_clear_value','depth_dimensions','multisample_control','merge_upper_x_bits','merge_upper_y_bits'):
        values[k]=rng.randrange(1<<32)
    values.update(tiling_lifecycle=rng.randrange(1<<64),fragment_lifecycle=rng.randrange(1<<64))
    p=replace(p,**values)
    item=(0,1,255,256,65535,0xaaaaaaa9)[i%6]
    for fragment in (False,True):
        grid=(i*7+int(fragment))%128
        s=RenderState(context=i%63+1,pool=i%15,event_control=0xfffffc2200000200,
            buffer_manager=0xfffffc20c3100000,buffer_thing=0xfffffc20c3108080,
            empty_buffer=0xfffffc20c310a800,alias=0x7400000000+i*0x4000,
            shared_tail=0xfffffc2200100000,driver_stamp=0xfffffc2200200000,
            firmware_stamp=0xfffffc2200300000,support=0xfffffc20c3200000,
            counter=i*9,stamp=i*257,event=grid,uuid=i*301,timestamp=0xfffffc2200400000,
            timestamp_end=None if i%3==0 else 0xfffffc2200500000,
            user_timestamp_start=i*0x4000,user_timestamp_end=i*0x4000+8,
            fragment_event=grid+1,fragment_stamp=i*513,stage_flag=i%3,
            field_93c=i*0x100000001,field_2128=i*19,field_21d8=i*23,field_222c=i*29,
            secondary_stamp=None if i%3==0 else i*619,descriptor_ordinal=None if i%3==0 else i*601,
            work_index=None if i%3==0 else i*811,tail_flags=i%256,
            tiling_submission_tag=i*0x101,tiling_queue_ordinal=i*31)
        emit(f'varied-{i}-{fragment}',p,s,fragment,item,grid)

for directory in ('single-user-texture-pixel-20261010','single-user-render32-msaa2-20261010'):
    for stage,builder in ((0,build_tiling_source),(1,build_fragment_source)):
        folder=a.m1n1/'build/m4max-gpu'/directory
        path=folder/f'stage-{stage}-descriptor.bin'
        captured=path.read_bytes()
        built,inputs=builder(captured,(folder/f'stage-{stage}-wrapper.bin').read_bytes())
        assert built==captured,(str(path),inputs['differing_offsets'][:16])
        emit(str(path),Parameters(**inputs['fields']),RenderState(**inputs['state']),
             bool(stage),inputs['item'],inputs['grid'],captured)
        native[str(path)]=hashlib.sha256(captured).hexdigest()

with tempfile.TemporaryDirectory() as directory:
    tmp=Path(directory);flags=host_versions.build(tmp)
    code='#![allow(dead_code,unused_mut,unused_assignments)]\nuse std::io::Write;\n'
    for name in ('g17p_compute','g17p_render'):
        code+=f'#[path={json.dumps(str(tests.parent/(name+".rs")))}]mod {name};\n'
    code+='mod g17p_render_lifecycle{pub const POOL_SLOTS:u32=(u32::BITS-2)/2;}\n'
    code+='use g17p_render::program;\n'
    code+='use g17p_render::wire as render;\nfn main(){\n'+'\n'.join(lines)+'\n}\n'
    (tmp/'main.rs').write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'main.rs'),*flags,'-o',str(tmp/'gate')],check=True)
    actual=subprocess.check_output([str(tmp/'gate')]);cursor=0
    for name,expected in cases:
        got=actual[cursor:cursor+len(expected)]
        assert got==expected,(name,[hex(i)for i,(x,y)in enumerate(zip(got,expected))if x!=y][:24])
        cursor+=len(expected)
    assert cursor==len(actual)
    # Compile the known-good Neo serializer independently. Exhaust late-tail
    # enable combinations and adversarial override overlap/byte precedence.
    baseline='a6883c3c6e5aa42548543a5a7b82bb6e375d6922'
    linux=tests.parents[4]
    (tmp/'legacy.rs').write_bytes(subprocess.check_output(['git','show',
        baseline+':drivers/gpu/drm/asahi/g17p_render.rs'],cwd=linux))
    neo='#![allow(dead_code)]\nuse std::io::Write;\n'
    neo+=f'#[path={json.dumps(str(tests.parent/"g17p_compute.rs"))}]mod g17p_compute;\n'
    neo+=f'#[path={json.dumps(str(tests.parent/"g17p_render.rs"))}]mod current;\n'
    neo+='mod legacy;\nmod g17p_render_lifecycle {pub const POOL_SLOTS:u32=(u32::BITS-2)/2;}\n'
    neo+='macro_rules! images {($r:ident,$i:expr)=>{{let i=$i;let mut p=$r::Parameters{\n'
    neo+='width:[1,32,33,129,1970,16384][i as usize%6],height:1123,layers:1+i%4,\n'
    neo+='samples:[1,2,4][i as usize%3],context_base:0x1000000000,queue_pair:i%4,\n'
    neo+='queue_item_index:i*17,lifecycle_ordinal:i*31,\n'
    neo+='..$r::Parameters::default()};\n'
    for k in ('tilemap','heapmeta','tpc','deflake_1','deflake_2','deflake_3','encoder'):
        neo+=f'p.{k}=p.context_base+i*0x4000;\n'
    for j,k in enumerate(('depth_bias_array','scissor_array','occlusion_query_base',
        'load_pipeline_bind','load_pipeline','partial_load_pipeline_bind','partial_load_pipeline',
        'depth_flags','depth_buffer','depth_stride','depth_aux_stride','depth_aux_buffer',
        'stencil_buffer','stencil_stride','stencil_aux_stride','stencil_aux_buffer','aux_fb_flags',
        'aux_fb_page_count','tile_config','store_pipeline_bind','store_pipeline',
        'partial_store_pipeline_bind','partial_store_pipeline','depth_clear_value_bits',
        'stencil_clear_value','sample_size','depth_dimensions','timestamp_a','timestamp_b',
        'ta_timestamp_end','fragment_timestamp_start','fragment_timestamp_end',
        'ta_user_timestamp_start','ta_user_timestamp_end','fragment_user_timestamp_start',
        'fragment_user_timestamp_end')):
        neo+=f'p.{k}=i*{j+1}+{j*17};\n'
    neo+='p.sampler_count=if i%2==0{0}else{i*17+1};p.sampler_array=if i%2==0{0}else{0x1001000000};\n'
    neo+='p.process_empty_tiles=i%3==0;p.fragment_sync_grow=[None,Some(false),Some(true)][i as usize%3];\n'
    neo+='p.vertex_store_flag=i%2==0;p.fragment_store_flag=i%3==0;p.reactive_tvb_growth=i%3==1;\n'
    neo+='let mut images=Vec::new();for kind in [$r::Kind::Tiling,$r::Kind::Fragment]{\n'
    neo+='let mut program=match kind{$r::Kind::Tiling=>$r::tiling_registers(&p).unwrap().to_vec(),\n'
    neo+='$r::Kind::Fragment=>$r::fragment_registers(&p).unwrap().to_vec()};\n'
    neo+='program.push((if kind==$r::Kind::Tiling{0x1ca10}else{0x160e0},0x123456789abcdef0));\n'
    neo+='let overrides=if kind==$r::Kind::Tiling{[(0x876,0xdeadbeef),(0x93c,0xaabbccdd),\n'
    neo+='(0x93d,0xeef01122),(0x932,0xeeee0011)]}else{[(0x2110,0xdeadbeef),\n'
    neo+='(0x2128,0xaabbccdd),(0x222c,0xeef01122),(0x21cc,0xeeee0011)]};\n'
    neo+='let d=$r::Descriptor{kind,index:(i*19)as u32,sequence:i<<40|i,ordinal:(i*37)as u32,\n'
    neo+='context:(i%64)as u32,queue_pair:(i%4)as u32,pool_bases:[0xfffffc20c0834000,0xfffffc20c0858000],\n'
    neo+='record_indices:[(i*2)as u32,(i*3)as u32],shared:[0xfffffc20c0920000,0xfffffc20c0924000],\n'
    neo+='low_alias:Some(0x7002000000+i*0x4000),status_base:Some(0xfffffc2001770000+i*0x4000),\n'
    neo+='grid:Some((i%128)as u32),write_tail:i&1!=0,write_lifecycle:i&2!=0,write_item:i&4!=0,\n'
    neo+='write_structural:i&8!=0,pointer_overrides:&[],item_overrides:&overrides};\n'
    neo+='let mut out=vec![0xa5;kind.size()];d.build(&mut out,&program,if i&16!=0{None}else{Some(&p)}).unwrap();\n'
    neo+='images.extend(out);}images}};}\nfn main(){for i in 0..256u64{\n'
    neo+='let old=images!(legacy,i);let new=images!(current,i);assert_eq!(old,new,"Neo case {i}");\n'
    neo+='std::io::stdout().write_all(&new).unwrap();}}\n'
    (tmp/'neo.rs').write_text(neo)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'neo.rs'),*flags,'-o',str(tmp/'neo')],check=True)
    neo_images=subprocess.check_output([str(tmp/'neo')])
a.evidence.mkdir(parents=True,exist_ok=False)
(a.evidence/'summary.json').write_text(json.dumps(dict(whole_descriptors=len(cases),
    independent_native_DATA_objects=len(native),native_DATA_sha256=native,
    every_field_size_alignment_asserted=True,whole_bytes=len(actual),
    malformed_requests_with_unchanged_staging=len(cases)*8,
    Neo_baseline=baseline,Neo_whole_descriptors=512,Neo_identical_bytes=len(neo_images),
    Neo_sha256=hashlib.sha256(neo_images).hexdigest(),
    program_inputs='production Rust program lowering and command-to-body adaptation',
    native_GPU_render_execution=False,sha256=hashlib.sha256(actual).hexdigest(),
    source_sha256=hashlib.sha256((tests.parent/'scheduled_render.rs').read_bytes()).hexdigest(),
    program_source_sha256=hashlib.sha256((tests.parent/'scheduled_render_program.rs').read_bytes()).hexdigest(),
    common_render_source_sha256=hashlib.sha256((tests.parent/'g17p_render.rs').read_bytes()).hexdigest()),indent=2)+'\n')
print('PASS',len(cases),'whole render descriptors;',len(native),'independent native DATA images; every layout assertion')
