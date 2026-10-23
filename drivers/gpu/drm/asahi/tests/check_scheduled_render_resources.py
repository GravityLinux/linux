#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Whole Max parameter/FList DATA against qualified source constructors."""
import argparse,hashlib,json,struct,subprocess,sys,tempfile
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('m1n1',type=Path);p.add_argument('--evidence',type=Path,required=True)
a=p.parse_args();tests=Path(__file__).resolve().parent;sys.path.insert(0,str(a.m1n1/'proxyclient'))
from m1n1.g16c.pools import ParameterBuffer,OperandPool,BUFFER_STRIDE
from m1n1.g16c.firmware import register_flist,parameter_buffer_descriptor
from m1n1.g16c.events import FListReceipt
cases=[];lines=[]
for i in range(32):
    base=0xfffffc20c3000000+i*0x200000
    blocks=(1,23,64,19737)[i%4]
    param=ParameterBuffer(i%15,base+0x150000,base+0x120000,base+0x110000,base+0x114000,base+0x118000,initial_blocks=blocks)
    low_slots=0x1000080000+i*0x4000
    fields=f'slot:{param.slot},blocks:{blocks},page_list:{param.page_list},block_list:{param.block_list},block_control:{param.block_control},counter:{param.counter},scene_list:{param.scene_list}'
    lines.append('let mut p=resources::ParameterBufferG16C{'+fields+'};')
    expected=[param.encode(),param.scenes(low_slots),param.block_ids(),param.pages(),param.control_bytes(),param.counter_bytes()]
    for size,call in [('resources::PAGE','resources::ParameterManagerG16C::write(&mut out,&p)'),
        ('resources::PAGE',f'p.scenes(&mut out,{low_slots})'),('resources::BLOCK_LIST_SIZE','p.block_ids(&mut out)'),
        ('resources::PAGE_LIST_SIZE','p.pages(&mut out)'),('resources::PAGE','p.control(&mut out)'),('resources::PAGE','p.counter(&mut out)')]:
        lines.append(f'let mut out=vec![0xa5;{size}];{call}.unwrap();emit(&out);'
                     'out.fill(0xa5);let saved=out.clone();let original=p.slot;p.slot=15;'
                     f'assert!({call}.is_err());assert_eq!(out,saved);p.slot=original;')
    lines.append('for(a,n)in p.backing_ranges().unwrap(){emit(&a.to_le_bytes());emit(&n.to_le_bytes());}')
    expected.append(b''.join(struct.pack('<QQ',*r)for r in param.backing_ranges()))
    lines.append(f'emit(resources::ParameterDescriptorG16C::new(resources::GPU_PAGE_LIST,{blocks*4}).unwrap().bytes());')
    expected.append(parameter_buffer_descriptor(param.gpu_page_list,blocks*4))
    directory=0x7000000000+i*0x2000000;table=directory+0x1000000;aux=base+0x4000
    allocated=28;registered=(0,19,23,28)[i%4];entries=registered
    pool=OperandPool(directory,table,tuple(table+0x4000+j*BUFFER_STRIDE for j in range(allocated)))
    expected.append(pool.support(aux,registered=registered,entries=entries))
    lines.append(f'let record=operands::OperandSupportG16C::for_render({directory},{table},{aux},{allocated},{registered},{entries}).unwrap();'
                 'let mut out=vec![0;resources::PAGE];out[..record.as_bytes().len()].copy_from_slice(record.as_bytes());emit(&out);')
    for j,body in enumerate(expected):cases.append((f'parameter-{i}-{j}',body))

for i in range(64):
    context=i%63+1;support=0xfffffc20c3000000+i*0x200000;table=0x7001000000+i*0x2000000
    added=(1,5,6,23,255,1024,(1<<29)-1)[i%7];previous=(0,19,23,25,0xffffffff)[i%5]
    mode=i%3;tag=i*17;reply_value=(0,2,17,0xffffffff)[i%4]
    want=register_flist(context,support,table,added,previous=previous,mode=mode,tag=tag)
    fields=f'context:{context},support:{support},table:{table},added:{added},previous:{previous},mode:{mode},tag:{tag},reply_value:{reply_value}'
    lines.append('let r=admission::RegistrationG16C{'+fields+'};let(request,mut receipt)=r.prepare().unwrap();emit(request.bytes());')
    cases.append((f'flist-request-{i}',want))
    reply=struct.pack('<IIIQQQI',13,context,reply_value,support,table,table+previous*64,added*8)+bytes([0xa5])*32
    assert FListReceipt(context,support,table,added,previous,reply_value).matches(reply)
    lines.append('let reply=['+','.join(str(x)for x in reply)+'];')
    lines.append('for at in 0..40usize{let(_,mut fresh)=r.prepare().unwrap();let mut bad=reply;bad[at]^=1;assert!(!fresh.consume(&bad));assert!(!fresh.received);}'
                 'assert!(receipt.consume(&reply));assert!(receipt.received);assert!(!receipt.consume(&reply));')

with tempfile.TemporaryDirectory()as directory:
    tmp=Path(directory)
    code='#![allow(dead_code,unused_mut)]\nuse std::io::Write;#[derive(Debug)]enum Error{Invalid,Overflow}\n'
    for name,file in [('resources','scheduled_render_resources.rs'),('admission','scheduled_render_admission.rs'),('operands','scheduled_operands.rs')]:
        code+=f'#[path={json.dumps(str(tests.parent/file))}]mod {name};\n'
    code+='fn emit(bytes:&[u8]){std::io::stdout().write_all(bytes).unwrap();}\nfn main(){\n'+'\n'.join(lines)+'''
    let mut r=admission::RegistrationG16C{context:1,support:0xfffffc20c3000000,table:0x7001000000,added:5,previous:23,mode:0,tag:19,reply_value:0};
    r.context=0;assert!(r.prepare().is_err());r.context=64;assert!(r.prepare().is_err());r.context=1;
    r.added=0;assert!(r.prepare().is_err());r.added=1<<29;assert!(r.prepare().is_err());r.added=5;
    r.mode=3;assert!(r.prepare().is_err());r.mode=0;r.table=u64::MAX;assert!(r.prepare().is_err());
    for(list,pages)in[(0,1),(0x1000438001,1),(0x2000000000,1),(0x1000438000,0),(0x1000438000,1<<22)]{assert!(resources::ParameterDescriptorG16C::new(list,pages).is_err());}
    assert!(operands::OperandSupportG16C::for_render(0x7000000000,0x7001000000,0xfffffc20c3004000,28,29,29).is_none());
    assert!(operands::OperandSupportG16C::for_render(0x7000000000,0x7001000000,0xfffffc20c3004000,28,23,22).is_none());
    }
    '''
    (tmp/'gate.rs').write_text(code)
    subprocess.run(['rustc','--edition=2021','-Dwarnings',str(tmp/'gate.rs'),'-o',str(tmp/'gate')],check=True)
    actual=subprocess.check_output([str(tmp/'gate')]);cursor=0
    for name,want in cases:
        got=actual[cursor:cursor+len(want)];assert got==want,(name,[hex(i)for i,(a,b)in enumerate(zip(got,want))if a!=b][:16]);cursor+=len(want)
    assert len(actual)==cursor
summary=dict(whole_parameter_and_support_objects=32*9,whole_flist_requests=64,
    full_bytes=len(actual),sha256=hashlib.sha256(actual).hexdigest(),
    every_wire_field_offset_size_alignment_asserted=True,wrong_reply_byte_cases=64*40,
    duplicate_reply_rejects=64,unchanged_staging_on_error_cases=32*6,
    other_negative_cases=13,native_GPU_execution=False,
    source_sha256={name:hashlib.sha256((tests.parent/name).read_bytes()).hexdigest()for name in
        ('scheduled_render_resources.rs','scheduled_render_admission.rs','scheduled_operands.rs')})
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print('PASS',summary['whole_parameter_and_support_objects'],'whole parameter/support objects;',64,'FList requests; all fields and reply matching')
