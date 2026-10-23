#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Whole ordered Max render programs vs source and independent native DATA."""
import argparse
from dataclasses import asdict,replace
import hashlib,json,random,re,struct,subprocess,sys,tempfile
from pathlib import Path
import host_versions
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1',type=Path);p.add_argument('--evidence',type=Path,required=True)
a=p.parse_args();tests=Path(__file__).resolve().parent
sys.path.insert(0,str(a.m1n1/'proxyclient'));sys.path.insert(0,str(a.m1n1/'proxyclient/experiments'))
from m1n1.g16c.render import Parameters,tiling_registers,fragment_registers,partial_store_registers,partial_resume_registers,partial_load_registers
from agx_g16c_native_tiling import build_tiling_source
from agx_g16c_native_fragment import build_fragment_source
common_source=(tests.parent/'g17p_render.rs').read_text().split('pub(crate) struct Parameters {',1)[1].split('\n}',1)[0]
state_source=(tests.parent/'scheduled_render_program.rs').read_text().split('pub(crate) struct ProgramStateG16C {',1)[1].split('\n}',1)[0]
common=set(re.findall(r'pub\(crate\) (\w+):',common_source));state=set(re.findall(r'pub\(crate\) (\w+):',state_source))
cases=[];lines=[];native={};boolval=lambda v:'true' if v else 'false';option=lambda v:'None'if v is None else f'Some({boolval(v)if isinstance(v,bool)else v})'
def emit(name,p):
    values=asdict(p);c={k:v for k,v in values.items()if k in common and k!='usc_exec_base'}
    c['emit_uapi_fields']=p.usc_exec_base is not None
    c['usc_exec_base']=1<<40
    c['reactive_tvb_growth']=p.request_tvb_growth
    values=lambda v:boolval(v)if isinstance(v,bool)else str(v)
    fields=','.join(f'{k}:{option(v)if k=="fragment_sync_grow"else values(v)}'for k,v in c.items())
    lines.append('let mut p=g17p_render::Parameters{'+fields+',..Default::default()};')
    s={k:getattr(p,k)for k in state}
    optional={'tiling_record_index','partial_load_tile_config','partial_load_depth_flags','partial_resume_depth_flags'}
    fields=','.join(f'{k}:{option(v)if k in optional else v}'for k,v in s.items())
    lines.append('let mut s=program::ProgramStateG16C{'+fields+'};')
    lines.append('for r in [program::TilingProgramG16C::new(&p,&s).unwrap().registers(),'
                 'program::FragmentProgramG16C::new(&p,&s).unwrap().registers(),'
                 '&program::partial_store(&p,&s).unwrap(),&program::partial_resume(&p,&s).unwrap(),'
                 '&program::partial_load(&p,&s).unwrap()]{emit(r);}'
                 'std::io::stdout().write_all(&program::tib_encoding(&p).to_le_bytes()).unwrap();')
    for n,fn in [('TA',tiling_registers),('FR',fragment_registers),('store',partial_store_registers),('resume',partial_resume_registers),('load',partial_load_registers)]:
        cases.append((name+'-'+n,b''.join(struct.pack('<IQ',*r)for r in fn(p))))
    cases.append((name+'-tib',struct.pack('<Q',p.tib_encoding())))
    # Both primary constructors must reject invalid geometry/compact addressing
    # and the fixed USC exception before any work object can be constructed.
    for expr,undo in [('p.width=0','p.width=old.width'),('p.layers=0','p.layers=old.layers'),
        ('p.tilemap=p.context_base-1','p.tilemap=old.tilemap'),
        ('p.tilemap=p.context_base+0x100000000','p.tilemap=old.tilemap'),
        ('p.sampler_count=0xffffffff','p.sampler_count=old.sampler_count'),
        ('p.sampler_array=3','p.sampler_array=old.sampler_array'),
        ('p.usc_exec_base=0','p.usc_exec_base=old.usc_exec_base'),
        ('p.tib_blocks=129','p.tib_blocks=old.tib_blocks'),
        ('p.samples=8','p.samples=old.samples')]:
        lines.append('let old=p;'+expr+';assert!(program::TilingProgramG16C::new(&p,&s).is_err());'
                     'assert!(program::FragmentProgramG16C::new(&p,&s).is_err());'+undo+';')
    lines.append('let old=p;p.samples=4;p.utile_width=32;p.utile_height=32;'
                 'assert!(program::TilingProgramG16C::new(&p,&s).is_err());'
                 'assert!(program::FragmentProgramG16C::new(&p,&s).is_err());p=old;')
    lines.append('let old_p=p;let old_s=s;p.emit_uapi_fields=false;s.vertex_helper_binary=1;'
                 'assert!(program::TilingProgramG16C::new(&p,&s).is_err());'
                 'assert!(program::FragmentProgramG16C::new(&p,&s).is_err());p=old_p;s=old_s;')
    lines.append('s.scene_slot=80;assert!(program::TilingProgramG16C::new(&p,&s).is_err());'
                 'assert!(program::FragmentProgramG16C::new(&p,&s).is_err());')

rng=random.Random(0x604116c)
for i in range(128):
    base=0x1000000000+i*0x4000000
    p=Parameters(width=(1,32,33,1970,16384)[i%5],height=(1,1123,16384)[i%3],
        layers=(1,2,65,2048)[i%4],utile_width=(32,32,16)[i%3],utile_height=(32,16,16)[i%3],
        samples=(1,2,4)[i%3],tib_blocks=(0,1,4,31,32,63,64,128)[i%8],context_base=base,
        scene_slot=i%80,usc_exec_base=1<<40 if i%2 else None,
        sampler_array=0 if i%2==0 else base+0x100000,sampler_count=0 if i%2==0 else 65537+i)
    v={}
    for k in ('tilemap','heapmeta','tpc','deflake_1','deflake_2','deflake_3','encoder','layermeta'):
        v[k]=base+rng.randrange(0x1000000)//8*8
    for k in ('aux_fb','depth_buffer','stencil_buffer','depth_aux_buffer','stencil_aux_buffer',
        'depth_bias_array','scissor_array','load_pipeline','store_pipeline',
        'partial_load_pipeline','partial_store_pipeline','fragment_status','ta_status',
        'cycle','tiling_lifecycle','fragment_lifecycle','tiling_work_stamp','fragment_work_stamp',
        'tiling_gate','fragment_gate','tiling_admission','fragment_admission','tiling_control',
        'multisample_control','depth_flags','depth_stride','stencil_stride','depth_aux_stride',
        'stencil_aux_stride','occlusion_query_base','aux_fb_flags','aux_fb_page_count'):
        v[k]=rng.randrange(1<<64)
    for k in ('utile_config','ppp_control','tile_config','depth_clear_value_bits','depth_dimensions',
        'stencil_clear_value','stencil_clear_flags','partial_stencil_clear_flags',
        'merge_upper_x_bits','merge_upper_y_bits','load_pipeline_bind','store_pipeline_bind',
        'partial_load_pipeline_bind','partial_store_pipeline_bind'):
        v[k]=rng.randrange(1<<32)
    for k in ('partial_load_tile_config','partial_load_depth_flags','partial_resume_depth_flags','tiling_record_index'):
        v[k]=None if i%3==0 else rng.randrange(1<<64)
    for k in ('vertex_helper_binary','vertex_helper_data','vertex_helper_config',
        'fragment_helper_binary','fragment_helper_data','fragment_helper_config'):
        v[k]=0 if i%2==0 else rng.randrange(1<<64)
    emit(str(i),replace(p,**v))

for directory in ('single-user-texture-pixel-20261010','single-user-render32-msaa2-20261010'):
    for stage,builder in ((0,build_tiling_source),(1,build_fragment_source)):
        folder=a.m1n1/'build/m4max-gpu'/directory;path=folder/f'stage-{stage}-descriptor.bin'
        body=path.read_bytes();built,inputs=builder(body,(folder/f'stage-{stage}-wrapper.bin').read_bytes())
        assert body==built
        params=Parameters(**inputs['fields']);at=0xa0 if stage else 0x60
        ref=fragment_registers(params)if stage else tiling_registers(params)
        assert body[at:at+len(ref)*12]==b''.join(struct.pack('<IQ',*r)for r in ref)
        emit(str(path),params);native[str(path)]=hashlib.sha256(body).hexdigest()
with tempfile.TemporaryDirectory()as directory:
    tmp=Path(directory);flags=host_versions.build(tmp)
    code='#![allow(dead_code,unused_mut,unused_assignments)]\nuse std::io::Write;\n'
    for name in ('g17p_compute','g17p_render'):
        code+=f'#[path={json.dumps(str(tests.parent/(name+".rs")))}]mod {name};\n'
    code+='mod g17p_render_lifecycle{pub const POOL_SLOTS:u32=(u32::BITS-2)/2;}\n'
    code+='use g17p_render::program;fn emit(r:&[(u32,u64)]){for(n,v)in r{std::io::stdout().write_all(&n.to_le_bytes()).unwrap();std::io::stdout().write_all(&v.to_le_bytes()).unwrap();}}\n'
    code+='fn main(){\n'+'\n'.join(lines)+'''
    for samples in [1,2,4] {
        for (utile_width,utile_height) in [(32,32),(32,16),(16,16)] {
            if samples==4 && (utile_width,utile_height)==(32,32) {continue;}
            for tib_blocks in 0..=128 {
                let p=g17p_render::Parameters{samples,utile_width,utile_height,
                    tib_blocks,..Default::default()};
                std::io::stdout().write_all(&program::tib_encoding(&p).to_le_bytes()).unwrap();
            }
        }
    }
    }
    '''
    (tmp/'main.rs').write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'main.rs'),*flags,'-o',str(tmp/'gate')],check=True)
    actual=subprocess.check_output([str(tmp/'gate')]);cursor=0
    for name,want in cases:
        got=actual[cursor:cursor+len(want)];assert got==want,(name,[hex(i)for i,(x,y)in enumerate(zip(got,want))if x!=y][:16]);cursor+=len(want)
    tib_cross_product=0
    for samples in (1,2,4):
        for uw,uh in ((32,32),(32,16),(16,16)):
            if samples==4 and (uw,uh)==(32,32):continue
            for blocks in range(129):
                want=Parameters(samples=samples,utile_width=uw,utile_height=uh,tib_blocks=blocks).tib_encoding()
                assert actual[cursor:cursor+8]==struct.pack('<Q',want),(samples,uw,uh,blocks)
                cursor+=8;tib_cross_product+=1
    assert cursor==len(actual)
a.evidence.mkdir(parents=True,exist_ok=False)
(a.evidence/'summary.json').write_text(json.dumps(dict(whole_ordered_programs=len(cases)//6*5,
    tib_encodings=len(cases)//6,tib_cross_product=tib_cross_product,
    native_DATA=native,negative_cases=len(cases)//6*24,
    whole_bytes=len(actual),sha256=hashlib.sha256(actual).hexdigest(),
    source_sha256=hashlib.sha256((tests.parent/'scheduled_render_program.rs').read_bytes()).hexdigest(),
    native_GPU_render_execution=False),indent=2)+'\n')
print('PASS',len(cases)//6*5,'whole ordered programs and',len(cases)//6,'TIB encodings')
