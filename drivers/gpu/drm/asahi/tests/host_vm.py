# SPDX-License-Identifier: GPL-2.0-only
"""Bounded RAM and allocation ownership for the actual Rust UAT walker."""
import host_fdt


def prelude():
    source=host_fdt.rust_prelude().replace('pub type KVec<T> = Vec<T>;', '''
    pub const EEXIST:Error=Error(17);pub const EBUSY:Error=Error(16);
    pub const EIO:Error=Error(5);pub const ENOMEM:Error=Error(12);
    pub const ETIMEDOUT:Error=Error(110);
    #[derive(PartialEq,Eq)]
    pub struct KVec<T>(Vec<T>);
    pub type KVVec<T> = KVec<T>;
    impl<T> KVec<T>{
        pub fn new()->Self{Self(Vec::new())}
        pub fn with_capacity(n:usize,_:u32)->Result<Self>{Ok(Self(Vec::with_capacity(n)))}
        pub fn push(&mut self,v:T,_:u32)->Result{self.0.push(v);Ok(())}
        pub fn reserve(&mut self,n:usize,_:u32)->Result{self.0.reserve(n);Ok(())}
        pub fn remove(&mut self,i:usize)->Result<T>{if i>=self.0.len(){return Err(EINVAL);}Ok(self.0.remove(i))}
        pub fn as_slice(&self)->&[T]{&self.0}
        pub fn as_mut_slice(&mut self)->&mut [T]{&mut self.0}
        pub fn extend_with(&mut self,n:usize,v:T,_:u32)->Result where T:Clone{self.0.extend(std::iter::repeat_n(v,n));Ok(())}
    }
    impl<T> std::ops::Deref for KVec<T>{type Target=[T];fn deref(&self)->&[T]{&self.0}}
    impl<T> std::ops::DerefMut for KVec<T>{fn deref_mut(&mut self)->&mut [T]{&mut self.0}}
    impl<'a,T> IntoIterator for &'a KVec<T>{type Item=&'a T;type IntoIter=std::slice::Iter<'a,T>;fn into_iter(self)->Self::IntoIter{self.0.iter()}}
    impl<'a,T> IntoIterator for &'a mut KVec<T>{type Item=&'a mut T;type IntoIter=std::slice::IterMut<'a,T>;fn into_iter(self)->Self::IntoIter{self.0.iter_mut()}}
    impl<T> IntoIterator for KVec<T>{type Item=T;type IntoIter=std::vec::IntoIter<T>;fn into_iter(self)->Self::IntoIter{self.0.into_iter()}}
    ''')
    # The OF adapter continues to use normal vectors, while the production
    # allocation API gets the flags-bearing wrapper above.
    source=source.replace('pub trait Value:Sized', '''
    impl Value for crate::prelude::KVec<u8>{fn decode(b:&[u8])->Result<Self>{let mut v=Self::new();for x in b{v.push(*x,0)?;}Ok(v)}}
    impl Value for crate::prelude::KVec<u32>{fn decode(b:&[u8])->Result<Self>{let mut v=Self::new();for x in Vec::<u32>::decode(b)?{v.push(x,0)?;}Ok(v)}}
    impl Value for crate::prelude::KVec<u64>{fn decode(b:&[u8])->Result<Self>{let mut v=Self::new();for x in Vec::<u64>::decode(b)?{v.push(x,0)?;}Ok(v)}}
    pub trait Value:Sized''')
    return source+'''
#[macro_export] macro_rules! pr_info {($($arg:tt)*)=>{{eprintln!($($arg)*);}};}
#[macro_export] macro_rules! pr_err {($($arg:tt)*)=>{{eprintln!($($arg)*);}};}
#[macro_export] macro_rules! dev_info {($dev:expr,$($arg:tt)*)=>{{let _=$dev;let _=format_args!($($arg)*);}};}
pub mod device {pub struct Device;}
mod g17p_platform {
    pub struct Region {pub base:u64}
    pub struct Platform {pub regions:[Region;2]}
}
mod g17p_image {
    pub struct Image {pub graph:super::g17p_initgraph::Graph,pub buffers:Vec<Vec<u8>>}
    impl Image {pub fn object(&self,i:usize)->super::prelude::Result<&[u8]>{self.buffers.get(i).map(Vec::as_slice).ok_or(super::prelude::EINVAL)}}
}
mod g17p_memory {
    use std::{cell::RefCell,collections::BTreeMap};
    use super::prelude::*;
    const PAGE:u64=0x4000;
    pub fn sync(){}
    static DETACHED_NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0x20000000000);
    pub static DETACHED_FAIL_AFTER:std::sync::atomic::AtomicUsize=std::sync::atomic::AtomicUsize::new(usize::MAX);
    pub struct Memory {
        pages:RefCell<BTreeMap<u64,Vec<u8>>>,
        pub next:u64,pub calls:usize,pub fail_after:Option<usize>,
    }
    pub struct Word64<'a>{owner:&'a Memory,address:u64}
    impl Word64<'_>{pub fn load(&self)->u64{self.owner.read64(self.address).unwrap()}pub fn store(&self,v:u64){self.owner.write_inner(self.address,&v.to_le_bytes()).unwrap();}}
    impl Memory {
        pub fn new()->Self{Self{pages:RefCell::new(BTreeMap::new()),next:0x10080000000,calls:0,fail_after:None}}
        pub fn detached()->Self{
            let mut m=Self::new();m.next=DETACHED_NEXT.fetch_add(0x40000000,std::sync::atomic::Ordering::Relaxed);
            let fail=DETACHED_FAIL_AFTER.load(std::sync::atomic::Ordering::Relaxed);m.fail_after=(fail!=usize::MAX).then_some(fail);m
        }
        pub fn absorb(&mut self,mut other:Self)->Result{
            self.next=self.next.max(other.next);self.calls+=other.calls;
            for(p,b)in std::mem::take(other.pages.get_mut()){assert!(self.pages.get_mut().insert(p,b).is_none());}Ok(())
        }
        pub fn reserve(&mut self,base:u64,size:u64){for p in (base..base+size).step_by(PAGE as usize){assert!(self.pages.get_mut().insert(p,vec![0;PAGE as usize]).is_none());}}
        pub fn allocate(&mut self,size:usize)->Result<u64>{
            if self.fail_after==Some(self.calls){return Err(ENOMEM);}
            self.calls+=1;
            let size=size.max(PAGE as usize).next_power_of_two() as u64;
            let base=(self.next+size-1)&!(size-1);self.reserve(base,size);self.next=base+size+PAGE;Ok(base)
        }
        pub fn read(&self,base:u64,size:usize)->Result<Vec<u8>>{
            let pages=self.pages.borrow();let mut out=Vec::with_capacity(size);let mut offset=0;
            while offset<size{let at=base+offset as u64;let n=(size-offset).min((PAGE-at%PAGE) as usize);let p=pages.get(&(at&!(PAGE-1))).ok_or(EINVAL)?;let start=(at%PAGE) as usize;out.extend_from_slice(&p[start..start+n]);offset+=n;}Ok(out)
        }
        fn write_inner(&self,base:u64,bytes:&[u8])->Result{
            let mut pages=self.pages.borrow_mut();let mut offset=0;
            while offset<bytes.len(){let at=base+offset as u64;let n=(bytes.len()-offset).min((PAGE-at%PAGE) as usize);let p=pages.get_mut(&(at&!(PAGE-1))).ok_or(EINVAL)?;let start=(at%PAGE) as usize;p[start..start+n].copy_from_slice(&bytes[offset..offset+n]);offset+=n;}Ok(())
        }
        pub fn write(&mut self,base:u64,b:&[u8])->Result{self.write_inner(base,b)}
        pub fn zero(&mut self,base:u64,size:usize)->Result{self.write(base,&vec![0;size])}
        pub fn write64(&mut self,base:u64,v:u64)->Result{self.write(base,&v.to_le_bytes())}
        pub fn read64(&self,base:u64)->Result<u64>{Ok(u64::from_le_bytes(self.read(base,8)?.try_into().unwrap()))}
        pub fn read_firmware32(&self,base:u64)->Result<u32>{if base%4!=0{return Err(EINVAL);}Ok(u32::from_le_bytes(self.read(base,4)?.try_into().unwrap()))}
        pub fn read_firmware_words(&self,base:u64,out:&mut[u8])->Result{if base%8!=0||out.is_empty()||out.len()%8!=0{return Err(EINVAL);}out.copy_from_slice(&self.read(base,out.len())?);Ok(())}
        pub fn write32(&mut self,base:u64,value:u32)->Result{self.write(base,&value.to_le_bytes())}
        pub fn clean(&self,base:u64,size:usize)->Result{self.read(base,size).map(|_|())}
        pub fn invalidate(&self,base:u64,size:usize)->Result{self.read(base,size).map(|_|())}
        pub fn word64(&self,base:u64)->Result<Word64<'_>>{if base%8!=0{return Err(EINVAL);}self.read(base,8)?;Ok(Word64{owner:self,address:base})}
        pub fn snapshot(&self,ranges:&[(u64,u64)])->Vec<Vec<u8>>{ranges.iter().map(|&(base,size)|self.read(base,size as usize).unwrap()).collect()}
        pub fn dump(&self,path:&std::path::Path){use std::io::Write;let mut f=std::io::BufWriter::new(std::fs::File::create(path).unwrap());for (pa,b) in self.pages.borrow().iter(){f.write_all(&pa.to_le_bytes()).unwrap();f.write_all(b).unwrap();}}
    }
}
'''
