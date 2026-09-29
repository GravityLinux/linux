#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Compare release with the executed shim tail and source-authored host ASM.

No target, firmware binary or captured RAM. The bounded interpreter executes
only the host's two publication primitives over explicit synthetic owned RAM.
"""
import argparse
import ast
from contextlib import redirect_stdout
import io
import importlib
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
from types import ModuleType, SimpleNamespace as NS

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1',type=Path)
p.add_argument('--rustc',default='rustc')
args=p.parse_args()
CONTROL=0xfffffc20c0780020
OUTER=(0xfffffc20c07800a0,0xfffffc20c07800b0,0xfffffc20c07800c0)
PRIMARY=0xfffffc20c0860060
SECONDARY=0xfffffc20c0850030
INNER=0xfffffc2001620000
CLOSING_CTX=0xfffffc2000250200
CLOSING_INNER=0xfffffc2001658040
ASC=0x100000

shim_tree=ast.parse((args.m1n1/'proxyclient/m1n1/agx/shim.py').read_text())
wave=next(n for n in ast.walk(shim_tree) if isinstance(n,ast.FunctionDef) and n.name=='_submit_dependency_wave')
start=next(i for i,n in enumerate(wave.body) if isinstance(n,ast.If) and ast.unparse(n.test)=='opening_compute_doorbell is None')
end=next(i for i,n in enumerate(wave.body) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='fences' for t in n.targets))
selected=ast.FunctionDef(name='source_release',args=ast.arguments(posonlyargs=[],args=[ast.arg(arg='self')],kwonlyargs=[],kw_defaults=[],defaults=[]),
    body=[next(n for n in wave.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='control_tick' for t in n.targets)),
          next(n for n in wave.body if isinstance(n,ast.FunctionDef) and n.name=='publish_tick')]+wave.body[start:end],decorator_list=[])
boot_tree=ast.parse((args.m1n1/'proxyclient/experiments/agx_g17p_boot.py').read_text())
assembly={name:next(n.value for n in ast.walk(next(m for m in ast.walk(boot_tree) if isinstance(m,ast.FunctionDef) and m.name==name))
    if isinstance(n,ast.Constant) and isinstance(n.value,str) and 'str ' in n.value and '#0x8800' in n.value)
    for name in ('release_dependency_window','publish_outer_with_work')}
package=ModuleType('dependency_release_reference')
package.__path__=[str(args.m1n1/'proxyclient/m1n1/agx')]
sys.modules[package.__name__]=package
reports=importlib.import_module(package.__name__+'.g17p_reports')
registration_node=next(n for n in boot_tree.body if isinstance(n,ast.FunctionDef) and n.name=='build_control_20_entry_object')
registration_ns=dict(struct=struct,g17p=NS(CONTROL_MESSAGE_SIZE=0x40))
exec(compile(ast.Module(body=[registration_node],type_ignores=[]),'source-registration','exec'),registration_ns)

def oracle(case,bad_state=False):
    ram={}
    trace=[]
    def store(address,value,size=4):
        for i,b in enumerate(int(value).to_bytes(size,'little')):ram[address+i]=b
    def read(address,size):return bytes(ram.get(address+i,0) for i in range(size))
    def write(address,value):store(address,value);trace.append(f'W {address:x} {value:x}')
    for i in range(8):store(PRIMARY+i*4,0xffffffff if case==15 else case*0x100+i*4)
    store(SECONDARY,0xffffffff if case==15 else case+10)
    store(SECONDARY+8,case+20)
    cursor=1
    def stage(body,deferred=False):
        nonlocal cursor
        trace.append(f'S {int(deferred)} {bytes(body).hex()}')
        cursor+=1
        if not deferred:write(CONTROL,cursor)
        return {'target':cursor,'deferred_producer':(CONTROL,struct.pack('<I',cursor))}
    def tick(counter,label,context_word=0,update_sequence=True):
        body=bytearray(0x40)
        struct.pack_into('<II',body,0,0x2e,counter)
        struct.pack_into('<I',body,0xc,context_word)
        return stage(body)
    def registration(kind,obj,operand,slot_offset,context_word,count,**kw):
        assert kind==1 and kw==dict(require_consumed=False,defer_tick=True,announce=False,defer_producer=True)
        # Use the actual pure registration serializer already checked by the
        # object oracle; avoid reconstructing its packed secondary opcode.
        body=registration_ns['build_control_20_entry_object'](kind,obj,operand,slot_offset,1,context_word=context_word,count=count)
        record=stage(body,True)
        return {'sequence':1,'0x20':record}
    # Keep imports in the executed source tail pointed at its real pure receipt
    # builder; its call also records the source's receipt-binding boundary.
    original_receipt=reports.build_class1_registration_receipt
    def bind_receipt(*a,**kw):
        trace.append('R 1')
        return original_receipt(*a,**kw)
    reports.build_class1_registration_receipt=bind_receipt

    def execute(name,arguments):
        lines=[]
        for line in assembly[name].splitlines():
            line=line.split('//',1)[0].strip()
            if line:lines.append(line)
        labels={line[:-1]:i for i,line in enumerate(lines) if line.endswith(':')}
        regs={0:ASC}
        regs.update(arguments)
        compare=False
        def value(text):return int(text[1:],0) if text.startswith('#') else regs.get(int(text[1:]),0)&(0xffffffff if text.startswith('w') else 0xffffffffffffffff)
        def reg(text,word):regs[int(text[1:])]=word&((1<<32)-1 if text.startswith('w') else (1<<64)-1)
        def address(text):
            parts=[s.strip() for s in text.strip('[]').split(',')]
            return value(parts[0])+(value(parts[1]) if len(parts)>1 else 0)
        pc=0;steps=0
        while pc<len(lines):
            line=lines[pc];pc+=1;steps+=1;assert steps<2048
            if line.endswith(':'):continue
            ins,tail=line.split(None,1)
            parts=[s.strip() for s in re.split(r',\s*(?![^\[]*\])',tail)]
            if ins in ('dc','dsb','dmb'):continue
            if ins=='mov':reg(parts[0],value(parts[1]))
            elif ins=='movk':
                shift=int(parts[2].split('#')[1]);reg(parts[0],(value(parts[0])&~(0xffff<<shift))|(value(parts[1])<<shift))
            elif ins=='lsl':reg(parts[0],value(parts[1])<<value(parts[2]))
            elif ins=='orr':reg(parts[0],value(parts[1])|value(parts[2]))
            elif ins=='add':reg(parts[0],value(parts[1])+value(parts[2]))
            elif ins=='cmp':compare=value(parts[0])<value(parts[1])
            elif ins in ('tbz','tbnz','cbz','b.lo'):
                jump=compare if ins=='b.lo' else value(parts[0])==0 if ins=='cbz' else bool(value(parts[0])&(1<<value(parts[1])))==(ins=='tbnz')
                if jump:pc=labels[parts[-1][:-1]]
            elif ins=='ldp':
                at=address(parts[2]);reg(parts[0],int.from_bytes(read(at,8),'little'));reg(parts[1],int.from_bytes(read(at+8,8),'little'))
            elif ins=='ldr':
                at=address(parts[1]);reg(parts[0],0 if at==ASC+0x8110 else int.from_bytes(read(at,4 if parts[0].startswith('w') else 8),'little'))
            elif ins=='str':
                at=address(parts[1]);word=value(parts[0]);size=4 if parts[0].startswith('w') else 8
                if at==ASC+0x8800:store(at,word,size)
                elif at==ASC+0x8808:
                    assert word==0x21
                    trace.append(f'N {int.from_bytes(read(ASC+0x8800,8),"little"):x}')
                else:
                    assert size==4
                    write(at,word)
            else:raise AssertionError(('unexpected source host instruction',line))
    def window(opening,class_producer,render,opening_channel,render_channel,notify_render):
        assert notify_render and opening[1]==render[0][1]==render[1][1]==struct.pack('<I',1)
        block=0x100000000
        words=[opening[0],render[0][0],render[1][0],class_producer[0],INNER,0xfffffc20c082010c,
            opening_channel,render_channel,struct.unpack('<I',class_producer[1])[0],0xfffffc20c0840048,PRIMARY,SECONDARY,int(notify_render),7]
        for i,word in enumerate(words):store(block+i*8,word,8)
        execute('release_dependency_window',{1:block})
    def closing(producer,channel):execute('publish_outer_with_work',{1:producer[0],2:struct.unpack('<I',producer[1])[0],3:channel})
    def await_control(record,label):
        trace.append(f'A {record["target"]}')
        if bad_state:store(INNER,0)
    def backend_write(address,body):
        if len(body)==4:write(address,struct.unpack('<I',body)[0])
        else:
            assert len(body)==0x200
            trace.append(f'C {address:x} {bytes(body).hex()}')
            for i,b in enumerate(body):ram[address+i]=b
    backend=NS(release_dependency_window=window,publish_outer_with_work=closing,_read_dva=read,_write_dva=backend_write,
        _clean_dva_range=lambda *a:None,space=NS(flush=lambda:None),u=NS(inst=lambda *a:None),
        control_done=lambda:trace.append('N 84000000000011'),channels=NS(entries=[None]*12+[None],counters=lambda *a:(cursor,cursor,cursor)))
    staged=[('compute',{'deferred_producer':(OUTER[0],struct.pack('<I',1))}),
        ('render',{'submission':{'deferred_outer_producers':[(OUTER[1],struct.pack('<I',1)),(OUTER[2],struct.pack('<I',1))]}}),
        ('compute',{'deferred_queue_producer':(CLOSING_INNER,struct.pack('<I',3)),'deferred_producer':(OUTER[0],struct.pack('<I',2)),
                    'deferred_queue_context':(CLOSING_CTX,bytes([case])*0x200)})]
    namespace=dict(struct=struct,os=NS(getenv=lambda *a:None),backend=backend,staged=staged,opening_compute_doorbell=10,render_doorbell=8,
        registration_staged=True,stage_tick=tick,control_tick=0,register_control=registration,dependency_snapshot=None,await_control=await_control,
        stage_control=lambda body,label:stage(body),announce_control=lambda record,label:backend.control_done(),g17p=NS(CHANNEL_TABLE_WORK_COUNT=12))
    namespace['__package__']=package.__name__
    exec(compile(ast.fix_missing_locations(ast.Module(body=[selected],type_ignores=[])),str(args.m1n1/'proxyclient/m1n1/agx/shim.py'),'exec'),namespace)
    owner=NS(compute_runtime={})
    try:
        with redirect_stdout(io.StringIO()):namespace['source_release'](owner)
        status='OK'
    except RuntimeError as exc:
        assert bad_state and 'did not advance state' in str(exc)
        status='STATE'
    finally:reports.build_class1_registration_receipt=original_receipt
    return trace,status

with tempfile.TemporaryDirectory() as tmp:
    binary=Path(tmp)/'release'
    subprocess.run([args.rustc,'--edition=2021','-Dwarnings',str(Path(__file__).with_name('g17p_dependency_release.rs')),'-o',str(binary)],check=True)
    def rust(case,fail=999999,bad=0):
        lines=subprocess.check_output([str(binary)],input=f'{case} {fail} {bad}\n'.encode()).decode().splitlines()
        result=lines.pop().split()
        return lines,result[1],int(result[2])
    for case in range(16):
        expected,status=oracle(case)
        actual,result,calls=rust(case)
        assert (actual,result)==(expected,status),(case,next(((i,a,b) for i,(a,b) in enumerate(zip(actual,expected)) if a!=b),None),len(actual),len(expected))
        expected,status=oracle(case,True)
        actual,result,_=rust(case,bad=1)
        assert (actual,result)==(expected,status),(case,'class state mismatch')
    for bad in range(2,11):
        trace,result,_=rust(0,bad=bad)
        assert result=='BOUNDARY' and not trace,(bad,trace,result)
    full,_,calls=rust(0)
    for failure in range(calls):
        prefix,result,_=rust(0,fail=failure)
        assert result=='ACCESS' and full[:len(prefix)]==prefix,(failure,result)
print(f'PASS: 16 full release traces against executed shim/host ASM, 16 class-state failures, 9 boundary rejections and {calls} access-failure prefixes; stores/control bodies/mailboxes match')
