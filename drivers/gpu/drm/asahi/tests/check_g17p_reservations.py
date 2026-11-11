#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute the actual kernel reservation parser against compiled Neo FDT data.

The old multi-reg node must lose 23 ranges with Linux 7.3's real parser;
individual no-map children must reserve every source-owned range. No GPU work.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import tempfile

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--dtb',type=Path,required=True)
p.add_argument('--legacy-dtb',type=Path,required=True)
p.add_argument('--evidence',type=Path,required=True)
a=p.parse_args();linux=Path(__file__).resolve().parents[5]

def nodes(path):
    data=path.read_bytes();magic,total,off,strings,_,_,_,_,string_size,size=struct.unpack_from('>10I',data)
    assert magic==0xd00dfeed and total==len(data)
    strings=data[strings:strings+string_size];end=off+size;stack=[];result={}
    while off<end:
        token,=struct.unpack_from('>I',data,off);off+=4
        if token==1:
            z=data.index(b'\0',off);stack.append(data[off:z].decode());off=(z+4)&~3
            result['/'.join(stack) or '/']={}
        elif token==2:stack.pop()
        elif token==3:
            length,name=struct.unpack_from('>II',data,off);off+=8
            key=strings[name:strings.index(b'\0',name)].decode()
            result['/'.join(stack) or '/'][key]=data[off:off+length];off=(off+length+3)&~3
        elif token==4:pass
        elif token==9:break
        else:raise AssertionError(token)
    return result

parser_source=linux/'drivers/of/of_reserved_mem.c'
source=parser_source.read_text()if parser_source.exists()else subprocess.check_output(['git','show','HEAD:drivers/of/of_reserved_mem.c'],cwd=linux,text=True)
start=source.index('static int __init __reserved_mem_reserve_reg(');brace=source.index('{',start);end=brace+1;depth=1
while depth:
    depth+=(source[end]=='{')-(source[end]=='}');end+=1
function=source[start:end]
topology=(linux/'drivers/gpu/drm/asahi/g17p_topology.rs').read_text()
start=topology.index('pub(crate) const RESERVATIONS:');end=topology.index('];',start)
expected=[tuple(int(v,16)for v in row)for row in re.findall(r'\((0x[0-9a-f]+), (0x[0-9a-f]+)\)',topology[start:end])]
assert len(expected)==24
report={'kernel_parser_sha256':hashlib.sha256(function.encode()).hexdigest(),'expected_ranges':expected,'cases':{}}
for label,path in [('legacy',a.legacy_dtb),('split',a.dtb)]:
    tree=nodes(path)
    children=[(name,props)for name,props in tree.items()if name.startswith('/reserved-memory/') and props.get('compatible')==b'apple,neo-gpu-source-memory\0']
    declarations=[];calls=[]
    for i,(name,props)in enumerate(children):
        reg=props['reg'];assert reg and len(reg)%16==0 and props['no-map']==b'' and props['status']==b'okay\0'
        if label=='split':assert len(reg)==16,(name,len(reg))
        cells=struct.unpack('>'+str(len(reg)//4)+'I',reg)
        declarations.append('static const uint32_t row'+str(i)+'[]={'+','.join(hex(c)for c in cells)+'};')
        calls.append('{current=row'+str(i)+';count='+str(len(reg)//16)+';if(__reserved_mem_reserve_reg('+str(i)+',"source"))return 1;}')
    code=r'''
#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <errno.h>
#include <string.h>
#define __init
#define __be32 uint32_t
typedef uint64_t phys_addr_t;
typedef uint64_t u64;
static const uint32_t *current;static int count;
static const uint32_t *of_flat_dt_get_addr_size_prop(unsigned long n,const char *p,int *len){(void)n;(void)p;*len=count;return current;}
static void *of_get_flat_dt_prop(unsigned long n,const char *p,void *len){(void)n;(void)p;(void)len;return &count;}
static int fdt_validate_reserved_mem_node(unsigned long n,void *p){(void)n;(void)p;return 0;}
static void of_flat_dt_read_addr_size(const uint32_t *p,int i,uint64_t *base,uint64_t *size){p+=i*4;*base=((uint64_t)p[0]<<32)|p[1];*size=((uint64_t)p[2]<<32)|p[3];}
static int early_init_dt_reserve_memory(uint64_t b,uint64_t s,bool nomap){printf("%llx %llx %d\n",(unsigned long long)b,(unsigned long long)s,nomap);return 0;}
#define fdt_fixup_reserved_mem_node(n,b,s) ((void)0)
#define pr_warn(...) ((void)0)
#define pr_debug(...) ((void)0)
#define pr_err(...) ((void)0)
'''+ '\n'.join(declarations)+'\n'+function+'\nint main(void){'+''.join(calls)+'return 0;}\n'
    with tempfile.TemporaryDirectory(prefix='neo-reservations-')as tmp:
        tmp=Path(tmp);(tmp/'parser.c').write_text(code)
        subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror','-Wno-unused-parameter',str(tmp/'parser.c'),'-o',str(tmp/'parser')],check=True)
        rows=[tuple(int(v,16)for v in row.split())for row in subprocess.check_output([str(tmp/'parser')],text=True).splitlines()]
    if label=='legacy':assert [(b,s)for b,s,_ in rows]==expected[:1],rows
    else:
        assert len(children)==24 and [(b,s)for b,s,_ in rows]==expected,rows
        by_handle={struct.unpack('>I',p['phandle'])[0]:p for p in tree.values()if 'phandle'in p}
        gpu=next(p for p in tree.values()if p.get('compatible',b'').split(b'\0')[0]==b'apple,agx-t8140')
        handles=struct.unpack('>'+str(len(gpu['memory-region'])//4)+'I',gpu['memory-region']);assert len(handles)==30
        for h,pair in zip(handles[6:],expected):assert struct.unpack('>QQ',by_handle[h]['reg'])==pair
        assert gpu['memory-region-names'].split(b'\0')[6:-1]==[f'source-graph-{i}'.encode()for i in range(24)]
    assert all(nomap==1 for _,_,nomap in rows)
    report['cases'][label]={'dtb_sha256':hashlib.sha256(path.read_bytes()).hexdigest(),'reserved_ranges':len(rows),'reserved_bytes':sum(s for _,s,_ in rows)}
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'summary.json').write_text(json.dumps(report,indent=2)+'\n')
print('PASS actual Linux parser: old node reserves 1/24 ranges; split no-map nodes reserve all 24, exact GPU phandle/name mapping')
