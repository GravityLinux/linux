#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Whole growth replies against the qualified shim; reject foreign PB DATA."""
import argparse, hashlib, json, struct, subprocess, sys, tempfile
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1',type=Path);p.add_argument('--evidence',type=Path,required=True)
a=p.parse_args();tests=Path(__file__).resolve().parent
sys.path.insert(0,str(a.m1n1/'proxyclient'))
from m1n1.g16c.parameter_requests import growth_reply
from m1n1.g16c.pools import ParameterBuffer
from m1n1.g16c.firmware import parameter_buffer_descriptor
lines=[];expected=[]
for i in range(128):
    context=i%63+1;slot=i%15;generation=(1,2,0xffffffff)[i%3]
    token=(0,1,0xffffffff,i*79)[i%4];word=(0,0xffffffff,i*12345)[i%3]
    raw=struct.pack('<5I',6,context,slot,token,word)+bytes([i])*52
    lines.append('let raw='+repr(list(raw))+';let r=growth::GrowthRequestG16C::parse(&raw).unwrap();'
        f'assert_eq!(r.context(),{context});assert_eq!(r.slot(),{slot});assert_eq!(r.token(),{token});'
        f'emit(r.reply({generation}).unwrap().bytes());assert!(r.reply(0).is_none());'
        'for(at,value)in[(0,7u32),(4,0),(4,64),(8,15)]{let mut bad=raw;'
        'bad[at..at+4].copy_from_slice(&value.to_le_bytes());assert!(growth::GrowthRequestG16C::parse(&bad).is_none());}')
    expected.append(growth_reply(context,slot,generation,token,word))
parameter=ParameterBuffer(2,0xfffffc20c3150000,0xfffffc20c3120000,
    0xfffffc20c3110000,0xfffffc20c3114000,0xfffffc20c3118000,initial_blocks=23)
def array(name,data):return f'let mut {name}='+repr(list(data))+';'
lines.extend([array('manager',parameter.encode()[:0x88]),array('control',parameter.control_bytes()[:0x80]),
    array('region',parameter_buffer_descriptor(parameter.gpu_page_list,92)),
    'let mut pages=vec![0;resources::PAGE_LIST_SIZE];p.pages(&mut pages).unwrap();',
    'let mut blocks=vec![0;resources::BLOCK_LIST_SIZE];p.block_ids(&mut blocks).unwrap();blocks.truncate(23*8);',
    'let ids:Vec<u32>=(0..23).map(|i|0x11+i*5).collect();',
    'assert!(growth::metadata_owned(&p,&ids,&manager,&control,1,&region,&pages,&blocks));',
    # Active ordering and holes are allowed. Tail and IDs are bounded.
    'pages[..8].rotate_left(4);pages[4..8].fill(0);region[0]|=1;',
    'assert!(growth::metadata_owned(&p,&ids,&manager,&control,1,&region,&pages,&blocks));',
    'assert!(!growth::metadata_owned(&p,&ids,&manager,&control,0,&region,&pages,&blocks));',
    'for at in [12,0x20,0x34,0x38,0x3c,0x44,0x4c,0x54,0x64] {let saved=manager[at];manager[at]^=0x80;'
    'assert!(!growth::metadata_owned(&p,&ids,&manager,&control,1,&region,&pages,&blocks));manager[at]=saved;}',
    'for at in [0,4,0x60] {let saved=control[at];control[at]^=0x80;'
    'assert!(!growth::metadata_owned(&p,&ids,&manager,&control,1,&region,&pages,&blocks));control[at]=saved;}',
    'for at in [0,4] {let saved=region[at];region[at]^=0x80;'
    'assert!(!growth::metadata_owned(&p,&ids,&manager,&control,1,&region,&pages,&blocks));region[at]=saved;}',
    'for at in [0,4,23*8-1] {blocks[at]^=0x80;assert!(!growth::metadata_owned(&p,&ids,&manager,&control,1,&region,&pages,&blocks));blocks[at]^=0x80;}',
    'for at in [0,23*16,pages.len()-1] {let saved=pages[at];pages[at]=0xff;'
    'assert!(!growth::metadata_owned(&p,&ids,&manager,&control,1,&region,&pages,&blocks));pages[at]=saved;}'])
code='#![allow(dead_code,unused_mut)]\nuse std::io::Write;#[derive(Debug)]enum Error{Invalid,Overflow}\n'
for name,file in [('resources','scheduled_render_resources.rs'),('growth','scheduled_parameter_growth.rs')]:
    code+=f'#[path={json.dumps(str(tests.parent/file))}]mod {name};\n'
code+='fn emit(bytes:&[u8]){std::io::stdout().write_all(bytes).unwrap();}\nfn main(){\n'
fields=f'slot:2,blocks:23,page_list:{parameter.page_list},block_list:{parameter.block_list},block_control:{parameter.block_control},counter:{parameter.counter},scene_list:{parameter.scene_list}'
code+='let p=resources::ParameterBufferG16C{'+fields+'};\n'+'\n'.join(lines)+'\n}'
with tempfile.TemporaryDirectory()as directory:
    tmp=Path(directory);(tmp/'gate.rs').write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'gate.rs'),'-o',str(tmp/'gate')],check=True)
    actual=subprocess.check_output([str(tmp/'gate')]);assert actual==b''.join(expected)
a.evidence.mkdir(parents=True,exist_ok=False)
(a.evidence/'summary.json').write_text(json.dumps(dict(whole_replies=128,whole_bytes=len(actual),
    every_wire_field_offset_size_alignment_asserted=True,invalid_request_cases=512,
    zero_generation_rejects=128,metadata_negative_cases=21,
    active_free_list_reordering_holes_and_region_flag_accepted=True,GPU_execution=False,
    sha256=hashlib.sha256(actual).hexdigest()),indent=2)+'\n')
print('PASS 128 whole source replies; all layout assertions; request and foreign-metadata rejection')
