# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Exercise the real m1n1/libfdt population code and expose typed OF fixtures."""
import ctypes as C
import json
from pathlib import Path
import struct
import subprocess
import sys


class PerfState(C.Structure):
    _fields_=[('freq',C.c_uint32),('volt',C.c_uint32)]


class Scalar(C.Structure):
    _fields_=[('name',C.c_char_p),('value',C.c_uint32)]


class Region(C.Structure):
    _fields_=[('base',C.c_uint64),('size',C.c_uint64)]


class Input(C.Structure):
    _fields_=[('core',C.POINTER(PerfState)),('core_size',C.c_size_t),
              ('sram',C.POINTER(PerfState)),('sram_size',C.c_size_t),
              ('cs',C.c_void_p),('cs_size',C.c_size_t),('revision',C.c_uint32),
              ('sample_clocks',C.c_uint32),('gpc',C.c_bool),
              ('power',C.POINTER(Scalar)),('power_count',C.c_size_t),
              ('software_interrupt',C.c_uint64),('active_mcc_mask',C.c_uint32)]


class Loader:
    def __init__(self,m1n1,directory,fixture,cc='clang'):
        self.m1n1=Path(m1n1);self.fixture=Path(fixture).read_bytes()
        src=self.m1n1/'src'
        library=Path(directory)/('libgpu_fdt.dylib' if sys.platform=='darwin' else 'libgpu_fdt.so')
        names=['fdt.c','fdt_ro.c','fdt_rw.c','fdt_wip.c','fdt_sw.c','fdt_empty_tree.c','fdt_strerror.c','fdt_addresses.c']
        subprocess.run([cc,'-shared','-fPIC','-O2','-std=c11','-I'+str(src),str(src/'gpu_fdt.c'),
                        *[str(src/'libfdt'/n) for n in names],'-o',str(library)],check=True)
        self.lib=C.CDLL(str(library))
        def fn(name,args,result=C.c_int):
            f=getattr(self.lib,name);f.argtypes=args;f.restype=result;return f
        ptr=C.c_void_p;i=C.c_int;s=C.c_char_p
        self.open=fn('fdt_open_into',[ptr,ptr,i]);self.path=fn('fdt_path_offset',[ptr,s])
        self.first=fn('fdt_first_subnode',[ptr,i]);self.next=fn('fdt_next_subnode',[ptr,i])
        self.firstprop=fn('fdt_first_property_offset',[ptr,i]);self.nextprop=fn('fdt_next_property_offset',[ptr,i])
        self.getprop=fn('fdt_getprop',[ptr,i,s,C.POINTER(i)],ptr)
        self.getbyoffset=fn('fdt_getprop_by_offset',[ptr,i,C.POINTER(s),C.POINTER(i)],ptr)
        self.phandle=fn('fdt_node_offset_by_phandle',[ptr,C.c_uint32])
        self.add=fn('fdt_add_subnode',[ptr,i,s]);self.delete=fn('fdt_del_node',[ptr,i])
        self.set=fn('fdt_setprop',[ptr,i,s,ptr,i]);self.populate=fn('gpu_fdt_populate_g16c',[ptr,i,C.POINTER(Input)])
        self.ispower=fn('gpu_fdt_power_property',[s],C.c_bool)
        self.populate_memory=fn('gpu_fdt_populate_g16c_memory',[ptr,i,C.POINTER(Region),C.c_uint64,C.c_uint64])
        self.delprop=fn('fdt_delprop',[ptr,i,s])

    def property(self,dt,node,name):
        length=C.c_int();p=self.getprop(dt,node,name.encode(),C.byref(length))
        return C.string_at(p,length.value) if p else None

    def children(self,dt,node):
        child=self.first(dt,node)
        while child>=0:
            yield child;child=self.next(dt,child)

    def snapshot(self,dt,node,links=False):
        props={};offset=self.firstprop(dt,node)
        while offset>=0:
            name=C.c_char_p();length=C.c_int()
            value=self.getbyoffset(dt,offset,C.byref(name),C.byref(length));assert value
            props[name.value.decode()]=C.string_at(value,length.value)
            offset=self.nextprop(dt,offset)
        children=[self.snapshot(dt,c) for c in self.children(dt,node)]
        targets={}
        if links:
            for key in ['operating-points-v2','apple,cs-opp']:
                target=self.phandle(dt,struct.unpack('>I',props[key])[0]);assert target>=0
                targets[key]=self.snapshot(dt,target)
            handles=props.get('memory-region',b'')
            for index in range(len(handles)//4):
                handle=struct.unpack_from('>I',handles,index*4)[0]
                target=self.phandle(dt,handle);assert target>=0
                targets[f'memory-region#{index}']=self.snapshot(dt,target)
        return dict(props=props,links=targets,children=children)

    def prepare(self,platform,cs_count=None):
        dt=C.create_string_buffer(0x20000)
        assert self.open(self.fixture,dt,len(dt))==0
        gpu=self.path(dt,b'/gpu');assert gpu>=0
        # Synthetic CS-count cases resize the template only in the host test.
        # The shipped J616c DTS contains its actual eight CS OPP nodes.
        if cs_count is not None:
            ph=self.property(dt,gpu,'apple,cs-opp');table=self.phandle(dt,struct.unpack('>I',ph)[0])
            for child in reversed(list(self.children(dt,table))):assert self.delete(dt,child)==0
            for index in reversed(range(cs_count)):
                child=self.add(dt,table,f'opp{index:02d}'.encode());assert child>=0
                for key,data in [('opp-hz',bytes(8)),('opp-microvolt',bytes(4))]:
                    assert self.set(dt,child,key.encode(),data,len(data))==0
        core_bytes=platform.properties['perf-states'];sram_bytes=platform.properties['perf-states-sram']
        core=(PerfState*64).from_buffer_copy(core_bytes);sram=(PerfState*64).from_buffer_copy(sram_bytes)
        cs=C.create_string_buffer(platform.properties['cs-perf-states'])
        values=[(k,struct.unpack('<I',v)[0]) for k,v in platform.properties.items() if self.ispower(k.encode()) and len(v)==4]
        scalars=(Scalar*len(values))(*[Scalar(k.encode(),v) for k,v in values])
        incoming=Input(core,len(core_bytes),sram,len(sram_bytes),C.cast(cs,C.c_void_p),len(cs)-1,
            platform.revision,platform.period_clocks,platform.gpc,scalars,len(values),
            struct.unpack_from('<Q',platform.properties['meta-sw-interrupt'],
                               len(platform.properties['meta-sw-interrupt'])-20)[0],
            platform.word('active-mcc-bit-vector',3))
        return dt,incoming,(core,sram,cs,scalars)

    def build(self,platform,cs_count=None):
        dt,incoming,owners=self.prepare(platform,cs_count)
        result=self.populate(dt,self.path(dt,b'/gpu'),C.byref(incoming))
        assert result==0,('m1n1 FDT populate',result)
        return dt,self.snapshot(dt,self.path(dt,b'/gpu'),True)


    def rejection_checks(self,platform):
        mutations=[
            lambda dt,x,own:setattr(x,'software_interrupt',0x3fff),
            lambda dt,x,own:setattr(x,'software_interrupt',1<<48),
            lambda dt,x,own:setattr(x,'active_mcc_mask',0),
            lambda dt,x,own:setattr(x,'core_size',511),
            lambda dt,x,own:setattr(x,'sram_size',0),
            lambda dt,x,own:setattr(x,'cs_size',23),
            lambda dt,x,own:setattr(x,'sample_clocks',23999),
            lambda dt,x,own:setattr(x,'core',C.POINTER(PerfState)()),
            lambda dt,x,own:setattr(own[0][18],'volt',0xffffffff),
            lambda dt,x,own:C.memmove(own[2],struct.pack('<Q',2),8),
            lambda dt,x,own:C.memmove(C.addressof(own[2])+8,struct.pack('<Q',0),8),
            lambda dt,x,own:C.memmove(C.addressof(own[2])+8,struct.pack('<Q',17),8),
            lambda dt,x,own:C.memmove(C.addressof(own[2])+len(own[2])-9,struct.pack('<Q',1<<40),8),
        ]
        for mutation in mutations:
            dt,incoming,owners=self.prepare(platform)
            mutation(dt,incoming,owners)
            before=bytes(dt)
            assert self.populate(dt,self.path(dt,b'/gpu'),C.byref(incoming))<0
            assert bytes(dt)==before
        dt,incoming,owners=self.prepare(platform)
        invalid=(Scalar*1)(Scalar(b'gpu-unknown-config',1))
        incoming.power=invalid;incoming.power_count=1
        before=bytes(dt)
        assert self.populate(dt,self.path(dt,b'/gpu'),C.byref(incoming))<0 and bytes(dt)==before
        dt,incoming,owners=self.prepare(platform)
        gpu=self.path(dt,b'/gpu')
        table=self.phandle(dt,struct.unpack('>I',self.property(dt,gpu,'operating-points-v2'))[0])
        assert self.delete(dt,self.first(dt,table))==0
        before=bytes(dt)
        assert self.populate(dt,self.path(dt,b'/gpu'),C.byref(incoming))<0 and bytes(dt)==before
        return len(mutations)+2


def rust_node(snapshot):
    props='vec!['+','.join('('+json.dumps(k)+',vec!'+str(list(v))+')' for k,v in snapshot['props'].items())+']'
    links='vec!['+','.join('('+json.dumps(k)+','+rust_node(v)+')' for k,v in snapshot['links'].items())+']'
    children='vec!['+','.join(rust_node(v) for v in snapshot['children'])+']'
    return f'of::Node::new({props},{links},{children})'


def rust_prelude():
    return '''#![allow(dead_code,unused_imports)]
extern crate self as kernel;
pub mod macros { pub use host_versions::versions; }
#[macro_export] macro_rules! c_str { ($value:expr) => {$value}; }
#[derive(Debug,Clone,Copy,PartialEq,Eq)] pub struct Error(pub i32);
impl From<std::num::TryFromIntError> for Error { fn from(_:std::num::TryFromIntError)->Self {Self(22)} }
pub unsafe trait Zeroable {
    fn zeroed()->Self where Self:Sized {unsafe {core::mem::zeroed()}}
    fn init_zeroed()->Self where Self:Sized {Self::zeroed()}
}
pub mod prelude {
    pub use crate::{Error,Zeroable};
    pub type Result<T=()> = core::result::Result<T,Error>;
    pub type KVec<T> = Vec<T>;
    pub const EINVAL:Error=Error(22);pub const ENODEV:Error=Error(19);
    pub const GFP_KERNEL:u32=0;
    pub struct KBox<T>(Box<T>);
    impl<T> KBox<T>{pub fn new(value:T,_:u32)->Result<Self>{Ok(Self(Box::new(value)))}pub fn init(value:T,flags:u32)->Result<Self>{Self::new(value,flags)}}
    impl<T> std::ops::Deref for KBox<T>{type Target=T;fn deref(&self)->&T{&self.0}}
    impl<T> std::ops::DerefMut for KBox<T>{fn deref_mut(&mut self)->&mut T{&mut self.0}}
}
pub mod of {
    use std::collections::BTreeMap;
    use crate::{Error,prelude::Result};
    #[derive(Clone)] pub struct Node {
        pub props:BTreeMap<String,Vec<u8>>,pub links:BTreeMap<String,Node>,pub nodes:Vec<Node>,
    }
    impl Node {
        pub fn new(props:Vec<(&str,Vec<u8>)>,links:Vec<(&str,Node)>,nodes:Vec<Node>)->Self {
            Self{props:props.into_iter().map(|(k,v)|(k.to_owned(),v)).collect(),
                links:links.into_iter().map(|(k,v)|(k.to_owned(),v)).collect(),nodes}
        }
        pub fn get_property<T:Value>(&self,name:&str)->Result<T> {T::decode(self.props.get(name).ok_or(Error(2))?)}
        pub fn get_opt_property<T:Value>(&self,name:&str)->Result<Option<T>> {self.props.get(name).map(|v|T::decode(v)).transpose()}
        pub fn parse_phandle(&self,name:&str,index:usize)->Option<Node> {
            self.links.get(&format!("{name}#{index}")).cloned().or_else(||if index==0{self.links.get(name).cloned()}else{None})
        }
        pub fn reserved_mem_region_to_resource_byname(&self,name:&str)->Result<crate::io::resource::Resource>{
            let names=self.props.get("memory-region-names").ok_or(Error(2))?;
            let index=names.split(|b|*b==0).position(|n|n==name.as_bytes()).ok_or(Error(2))?;
            let node=self.parse_phandle("memory-region",index).ok_or(Error(2))?;
            let values:Vec<u64>=node.get_property("reg")?;
            if values.len()!=2{return Err(Error(22));}
            Ok(crate::io::resource::Resource{base:values[0],size:values[1]})
        }
        pub fn children(&self)->impl Iterator<Item=Node>+'_ {self.nodes.iter().cloned()}
    }
    pub trait Value:Sized {fn decode(bytes:&[u8])->Result<Self>;}
    impl Value for u32 {fn decode(b:&[u8])->Result<Self>{Ok(u32::from_be_bytes(b.try_into().map_err(|_|Error(22))?))}}
    impl Value for u64 {fn decode(b:&[u8])->Result<Self>{Ok(u64::from_be_bytes(b.try_into().map_err(|_|Error(22))?))}}
    impl Value for Vec<u32> {fn decode(b:&[u8])->Result<Self>{if b.len()%4!=0{return Err(Error(22));}b.chunks_exact(4).map(u32::decode).collect()}}
    impl Value for Vec<u64> {fn decode(b:&[u8])->Result<Self>{if b.len()%8!=0{return Err(Error(22));}b.chunks_exact(8).map(u64::decode).collect()}}
    impl Value for Vec<u8> {fn decode(b:&[u8])->Result<Self>{Ok(b.to_vec())}}
    pub struct Property<'a>(pub &'a [u8]);
    impl TryFrom<Property<'_>> for u32 {type Error=Error;fn try_from(p:Property<'_>)->Result<Self>{Self::decode(p.0)}}
    pub trait PropertyUnit:Sized {const UNIT_SIZE:usize;fn from_bytes(bytes:&[u8])->Result<Self>;}
    impl PropertyUnit for u32 {const UNIT_SIZE:usize=4;fn from_bytes(b:&[u8])->Result<Self>{Self::decode(b)}}
}
pub mod io {pub mod resource {
    pub struct Resource {pub base:u64,pub size:u64}
    impl Resource {pub fn start(&self)->usize{self.base as usize}pub fn size(&self)->usize{self.size as usize}}
}}
'''
