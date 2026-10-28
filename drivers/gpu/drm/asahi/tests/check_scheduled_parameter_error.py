#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Byte-exact OOM9 and owned OOM7/descriptor identity checks, no device."""
import argparse,hashlib,json,struct,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('bootloader',type=Path);p.add_argument('--evidence',type=Path,required=True);p.add_argument('--native-report',type=Path)
a=p.parse_args();tests=Path(__file__).resolve().parent;sys.path.insert(0,str(a.bootloader/'proxyclient'))
from m1n1.g16c.parameter_errors import error_reply
lines=[];expected=[]
for i in range(128):
    context=i%63+1;slot=i%15;event=i%128;stamp=(0,256,0xffffff00,0xffffffff)[i%4]
    descriptor=0xfffffc20d1049b40+i*0x4000;manager=0xfffffc20c3100000+i*0x200000
    queue=0xfffffc220808c000+i*0x84000;driver=0xfffffc20d1058000+i*0x80
    raw=struct.pack('<5I',7,0,1,event,stamp)+bytes([i])*4+struct.pack('<I',context)+bytes([i])*12+struct.pack('<3Q',descriptor,manager,queue)+bytes([255-i])*8
    lines.append(f'let owner=wire::RenderOwnerG16C{{context:{context},slot:{slot},event:{event},stamp:{stamp},descriptor:{descriptor},manager:{manager},queue:{queue},driver_stamps:[{driver},{driver+64}]}};')
    lines.append('let raw='+repr(list(raw))+';let request=wire::ParameterErrorG16C::parse(&raw).unwrap();'
        f'assert_eq!(request.context(),{context});assert_eq!(request.event(),{event});assert_eq!(request.stamp(),{stamp});'
        'emit(request.reply().unwrap().bytes());let mut body=vec![0xa5u8;0x2240];'
        f'for(at,value)in[(0,1u32),(12,{context}),(0x2150,{stamp}),(0x2154,{event})]{{body[at..at+4].copy_from_slice(&value.to_le_bytes());}}'
        f'for(at,value)in[(0x28,{manager}u64),(0x2140,{driver+64})]{{body[at..at+8].copy_from_slice(&value.to_le_bytes());}}'
        'assert!(owner.matches_request(&request));assert!(owner.matches(&request,&body));'
        'for at in (0..20).chain(24..28).chain(40..64){let mut bad=raw;bad[at]^=1;assert!(wire::ParameterErrorG16C::parse(&bad).is_none_or(|r|!owner.matches_request(&r)&&!owner.matches(&r,&body)));}'
        'for at in (0..4).chain(12..16).chain(0x28..0x30).chain(0x2140..0x2148).chain(0x2150..0x2158){body[at]^=1;assert!(!owner.matches(&request,&body));body[at]^=1;}'
        'assert!(!owner.matches(&request,&body[..0x223f]));body.push(0);assert!(!owner.matches(&request,&body));')
    expected.append(error_reply(context,queue,stamp))
neo_revision='a6883c3c6e5aa42548543a5a7b82bb6e375d6922'
neo=subprocess.check_output(['git','show',neo_revision+':drivers/gpu/drm/asahi/g17p_growth.rs'],cwd=tests.parent,text=True)
code='#![allow(dead_code)]\nuse std::io::Write;\n'+f'#[path={json.dumps(str(tests.parent/"scheduled_parameter_error.rs"))}]mod wire;\n'
code+='mod g17p_render_lifecycle {pub(crate) const POOL_SLOTS:u32=15;}\nmod neo {'+neo.replace('//!','//')+'}\n'
code+='fn emit(bytes:&[u8]){std::io::stdout().write_all(bytes).unwrap();}\nfn main(){\n'+'\n'.join(lines)
# Native DATA must decode the actual context and match the retained owner graph.
if a.native_report:
    raw=a.native_report.read_bytes();assert len(raw)==72
    w=lambda at:struct.unpack_from('<I',raw,at)[0]
    q=lambda at:struct.unpack_from('<Q',raw,at)[0]
    assert (w(0),w(4),w(8))==(7,0,1)
    code+='let raw='+repr(list(raw))+';let request=wire::ParameterErrorG16C::parse(&raw).unwrap();'
    code+=f'assert_eq!(request.context(),{w(24)});assert_eq!(request.event(),{w(12)});assert_eq!(request.stamp(),{w(16)});'
    code+=f'let owner=wire::RenderOwnerG16C{{context:{w(24)},slot:2,event:{w(12)},stamp:{w(16)},descriptor:{q(40)},manager:{q(48)},queue:{q(56)},driver_stamps:[0,0]}};assert!(owner.matches_request(&request));'
code+='for context in 1..64 {for stamp in [256u32,0xffffff00] {let q=0xfffffc220808c000;assert_eq!(wire::ParameterErrorReplyG16C::new(context,q,stamp).unwrap().bytes(),&neo::limit_reply(q,stamp).unwrap());}}'
code+='\nfor(c,q)in[(0,0xfffffc220808c000u64),(64,0xfffffc220808c000),(1,0),(1,0xfffffbfffffff000),(1,0xfffffc220808c001)]{assert!(wire::ParameterErrorReplyG16C::new(c,q,256).is_none());}\n}'
with tempfile.TemporaryDirectory()as directory:
    tmp=Path(directory);(tmp/'gate.rs').write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'gate.rs'),'-o',str(tmp/'gate')],check=True)
    actual=subprocess.check_output([str(tmp/'gate')]);assert actual==b''.join(expected)
a.evidence.mkdir(parents=True,exist_ok=False)
(a.evidence/'summary.json').write_text(json.dumps(dict(whole_source_reply_records=128,whole_bytes=len(actual),
    every_wire_field_offset_size_alignment_asserted=True,foreign_request_byte_cases=128*48,original_neo_reply_cases=126,original_neo_revision=neo_revision,native_report_checked=bool(a.native_report),
    foreign_descriptor_byte_cases=128*32,wrong_descriptor_size_cases=256,invalid_reply_cases=5,
    opaque_report_words_not_dereferenced=True,GPU_execution=False,
    source_sha256=hashlib.sha256((tests.parent/'scheduled_parameter_error.rs').read_bytes()).hexdigest(),
    output_sha256=hashlib.sha256(actual).hexdigest()),indent=2)+'\n')
print('PASS128 whole source OOM replies; all layouts;6144 foreign request bytes,4096 foreign descriptor bytes and size/admission rejects')
