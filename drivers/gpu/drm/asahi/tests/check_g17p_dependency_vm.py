#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Execute the shim's live dependency-root join against owned synthetic leaves.

Compare complete resulting PTEs with the same Rust join used by the kernel.
No hardware, captured pages, shaders or firmware binary are involved.
"""
import argparse
import ast
from copy import deepcopy
from contextlib import redirect_stdout
import io
from pathlib import Path
import struct
import subprocess
import tempfile
from types import SimpleNamespace as NS

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('m1n1',type=Path)
p.add_argument('--rustc',default='rustc')
args=p.parse_args()
PAGE=0x4000
ADDRESS=0x3ffffffc000
FLAGS=0xc0000000000c8b
tree=ast.parse((args.m1n1/'proxyclient/m1n1/agx/shim.py').read_text())
method=next(n for n in ast.walk(tree) if isinstance(n,ast.FunctionDef) and n.name=='_ensure_dependency_compute_state')
method.body=[n for n in method.body if not isinstance(n,(ast.Import,ast.ImportFrom))]
namespace={'MemoryAttr':NS(Shared=2)}
exec(compile(ast.fix_missing_locations(ast.Module(body=[method],type_ignores=[])),str(args.m1n1/'proxyclient/m1n1/agx/shim.py'),'exec'),namespace)
join=namespace[method.name]

class Pte:
    def __init__(self,word): self.word=word
    def valid(self): return bool(self.word&1)
    def offset(self): return self.word&ADDRESS
    def __getattr__(self,name):
        shift,size={'OS':(55,1),'UXN':(54,1),'PXN':(53,1),'AF':(10,1),'nG':(11,1),
                    'AttrIndex':(2,3),'AP':(6,2),'SH':(8,2)}[name]
        return self.word>>shift&((1<<size)-1)

class Uat:
    PAGE_SIZE=PAGE
    LEVELS=(None,(36,64,None),(25,2048,None),(14,2048,None))
    def __init__(self,leaves,root):
        self.leaves=leaves
        self.ttbr0_base=root
        self.paths={root:()}
        self.ids={():root}
        self.next=root+PAGE
    def fetch_pte(self,table,index,size,cls):
        prefix=self.paths[table]+(index,)
        def indices(va):return tuple((va>>shift)&mask for shift,mask in ((36,63),(25,2047),(14,2047)))
        if len(prefix)==3:
            word=next((pte for va,pte in self.leaves.items() if indices(va)==prefix),0)
            return Pte(word)
        if not any(indices(va)[:len(prefix)]==prefix for va in self.leaves):return Pte(0)
        if prefix not in self.ids:
            self.ids[prefix]=self.next
            self.paths[self.next]=prefix
            self.next+=PAGE
        return Pte(self.ids[prefix]|3)
    def iotranslate_root(self,root,va,size):
        assert root==self.ttbr0_base
        assert size==PAGE and va%PAGE==0
        return [(self.leaves.get(va,0)&ADDRESS or None,PAGE)]
    def iounmap(self,ctx,va,size):
        assert ctx==2 and size==PAGE
        del self.leaves[va]
    def iomap_at(self,ctx,va,pa,size,**kw):
        assert ctx==2 and size==PAGE
        word=pa|3
        for name,shift,default in [('OS',55,0),('UXN',54,0),('PXN',53,0),('AF',10,1),('nG',11,0),
                                   ('AttrIndex',2,0),('AP',6,1),('SH',8,0)]:
            word|=int(kw.get(name,default))<<shift
        self.leaves[va]=word
    def flush_dirty(self):pass
    def invalidate_cache(self):pass

ACTIVE=[0x7000000000,0x7000098000]+[base+off for base in (0x7000460000,0x7000488000)
    for off in range(0,8*PAGE,PAGE)]+[0x70017e0000,0x70019e8000]
CDMS=[0x10000400000,0x10000500000]
def fixture(case):
    render={va:(0x10020000000+i*PAGE)|FLAGS for i,va in enumerate(sorted(set(ACTIVE+[0x1000080000,0x1000198000,0x10001a8000,0x7000208000,0x7000238000,0x7000a60000])))}
    compute={}
    for i,(va,pte) in enumerate(sorted(render.items())):
        if (i+case)%3==0: compute[va]=pte^(1<<54) # same PA, differing attributes
        elif (i+case)%3==1:compute[va]=(0x10040000000+i*PAGE)|FLAGS
        # Third case is absent. Import reads live render leaves in all cases.
    for base in (0x70017e0000,0x7000220000):
        for off in range(0,0x14000,PAGE): compute.setdefault(base+off,(0x10050000000+base-0x7000000000+off)|FLAGS)
    for i,cdm in enumerate(CDMS):
        compute[cdm]=(0x10060000000+i*0x100000)|FLAGS
        compute[cdm+0x30000]=(0x10060030000+i*0x100000)|FLAGS
    # Stale inventory PA values must never be imported after a live VM_BIND.
    inventory={va:0x10070000000+i*PAGE for i,va in enumerate(sorted(render)) if (i+case)%2==0}
    return compute,render,inventory,[(CDMS[0],0x40000),(CDMS[1],0x40000)]

def python(fixture):
    compute,render,inventory,bindings=deepcopy(fixture)
    zeroed=[]
    def allocate(alignment,size):
        assert alignment==size==PAGE
        pa=0x10080000000+len(zeroed)*PAGE
        zeroed.append(pa)
        return pa
    def zero(pa,value,size):assert pa in zeroed and value==0 and size==PAGE
    backend=NS(space=NS(uat=Uat(render,0x10001000000)),retained_extent=inventory,u=NS(inst=lambda *a:None))
    source=NS(uat=Uat(compute,0x10002000000),u=NS(memalign=allocate),
              p=NS(memset32=zero,dc_civac=lambda *a:None),flush=lambda:None)
    runtime={'client':{'space':source},'native':NS(CONTEXT=2,STATE_BASE=0x7000220000,OPERAND_STATE_WORKLOAD_STRIDE=0x15c0000)}
    owner=NS(front=NS(g17p=backend))
    vm=NS(bindings=[NS(addr=va,end=va+size,size=size) for va,size in bindings])
    commands=[NS(header=NS(cmd_type=1),hardware_state=NS(cdm_base=va)) for va in CDMS]
    try:
        with redirect_stdout(io.StringIO()):
            join(owner,vm,runtime,commands)
    except RuntimeError as exc:
        message=str(exc)
        if 'overlap' in message:return 1,None
        if 'mapped' in message or 'owned page' in message:return 2,None
        raise
    words=[0,len(compute)]
    for va,pte in sorted(compute.items()):words.extend((va,pte))
    words.extend(runtime['dependency_robustness_pages'][word]['pa'] for word in (0x100,0x102))
    words.extend([len(zeroed),*zeroed])
    return 0,struct.pack('<%dQ'%len(words),*words)

def encoded(fixture):
    compute,render,inventory,bindings=fixture
    words=list(CDMS)
    for root in (compute,render):
        words.append(len(root))
        for va,pte in sorted(root.items()):words.extend((va,pte))
    words.extend([len(inventory),*sorted(inventory),len(bindings)])
    for va,size in bindings:words.extend((va,size))
    return struct.pack('<%dQ'%len(words),*words)

with tempfile.TemporaryDirectory() as tmp:
    binary=Path(tmp)/'dependency-vm'
    subprocess.run([args.rustc,'--edition=2021','-Dwarnings',str(Path(__file__).with_name('g17p_dependency_vm.rs')),'-o',str(binary)],check=True)
    successful=negative=0
    for case in range(16):
        f=fixture(case)
        status,expected=python(f)
        assert status==0
        actual=subprocess.check_output([str(binary)],input=encoded(f))
        assert actual==expected,('PTE mismatch',case,[(hex(i),hex(a),hex(b)) for i,(a,b) in enumerate(zip(actual,expected)) if a!=b][:16])
        successful+=1
        for kind in range(6):
            bad=deepcopy(f)
            if kind==0:bad[0].pop(CDMS[0])
            elif kind==1:bad[0].pop(CDMS[1]+0x30000)
            elif kind==2:
                va=next(va for va in ACTIVE if (bad[0].get(va,0)&ADDRESS)!=(bad[1][va]&ADDRESS) and va in bad[0])
                bad[3].append((va,PAGE))
            elif kind==3:bad[3].append((0x7000220000,PAGE))
            elif kind==4:
                bad[0][0x1000078000]=0x10090000000|FLAGS
                bad[3].append((0x1000078000,PAGE))
            else:bad[0][0x100000b0000]=0x10090000000|FLAGS
            status,_=python(bad)
            actual=subprocess.check_output([str(binary)],input=encoded(bad))
            assert status!=0 and struct.unpack_from('<Q',actual)[0]==status,(case,kind,status,actual)
            negative+=1
print(f'PASS: {successful} complete live-root joins and {negative} source admission failures; attributes, caller collisions, sequential state aliases and distinct fresh robustness owners')
