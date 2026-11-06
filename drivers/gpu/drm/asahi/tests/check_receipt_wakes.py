#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exercise the actual stable DMA-fence callback allocation/binding/drop code."""
import argparse, hashlib, json, subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--evidence',type=Path,required=True)
a=p.parse_args()
s=(Path(__file__).resolve().parents[1]/'g17p_async.rs').read_text()
def item(marker):
    start=s.index(marker);end=s.index('{',start)+1;depth=1
    while depth:
        depth+=(s[end]=='{')-(s[end]=='}');end+=1
    return s[start:end]
code=r'''
#![allow(dead_code,non_camel_case_types,unused_imports)]
use std::{cell::{Cell,RefCell,UnsafeCell},ops::{Deref,DerefMut},sync::Arc,rc::Rc};
#[derive(Debug,PartialEq)]struct Error(i32);impl Error{fn from_errno(e:i32)->Self{Self(e)}}
type Result<T=()> = std::result::Result<T,Error>;const GFP_KERNEL:u32=0;
struct KBox<T>(Box<T>);impl<T>KBox<T>{fn new(t:T,_:u32)->Result<Self>{Ok(Self(Box::new(t)))}}
impl<T>Deref for KBox<T>{type Target=T;fn deref(&self)->&T{&self.0}}
impl<T>DerefMut for KBox<T>{fn deref_mut(&mut self)->&mut T{&mut self.0}}
struct Events{wakes:RefCell<Vec<usize>>}impl Events{fn wake_job(&self,id:usize){self.wakes.borrow_mut().push(id)}}
mod bindings{
 use super::*;pub const ENOENT:u32=2;
 #[repr(C)]pub struct dma_fence_cb{_list:[usize;2]}
 pub type Callback=unsafe extern "C" fn(*mut dma_fence,*mut dma_fence_cb);
 pub struct dma_fence{pub cb:Cell<*mut dma_fence_cb>,pub callback:Cell<Option<Callback>>,pub result:Cell<i32>,pub removals:Cell<usize>}
 pub unsafe fn dma_fence_add_callback(f:*mut dma_fence,c:*mut dma_fence_cb,w:Option<Callback>)->i32{
  let f=unsafe{&*f};if f.result.get()!=0{return f.result.get()};assert!(f.cb.get().is_null());f.cb.set(c);f.callback.set(w);0
 }
 pub unsafe fn dma_fence_remove_callback(f:*mut dma_fence,c:*mut dma_fence_cb){
  let f=unsafe{&*f};assert_eq!(f.cb.get(),c);f.cb.set(std::ptr::null_mut());f.callback.set(None);f.removals.set(f.removals.get()+1);
 }
}
mod dma_fence{
 use super::*;#[derive(Clone)]pub struct Fence(pub Rc<bindings::dma_fence>);
 impl Fence{pub fn raw(&self)->*mut bindings::dma_fence{Rc::as_ptr(&self.0).cast_mut()}}
}
'''
code+='#[repr(C)]\n'+item('struct FenceWake {')+'\n'+item('impl FenceWake {')+'\n'+item('impl Drop for FenceWake {')
code+=r'''
fn fence(result:i32)->dma_fence::Fence{dma_fence::Fence(Rc::new(bindings::dma_fence{cb:Cell::new(std::ptr::null_mut()),callback:Cell::new(None),result:Cell::new(result),removals:Cell::new(0)}))}
fn main(){
 let events=Arc::new(Events{wakes:RefCell::new(vec![])});
 let f=fence(0);let callback_ptr;
 {let mut slot=FenceWake::unbound(events.clone(),17).unwrap();assert!(slot.fence.is_none());
 callback_ptr=slot.callback.get();slot.bind(f.clone()).unwrap();assert_eq!(callback_ptr,slot.callback.get());
 assert!(events.wakes.borrow().is_empty());
 unsafe{f.0.callback.get().unwrap()(f.raw(),f.0.cb.get())};assert_eq!(*events.wakes.borrow(),vec![17]);}
 assert_eq!(f.0.removals.get(),1);assert!(f.0.cb.get().is_null());assert_eq!(Rc::strong_count(&f.0),1);
 // Signaling immediately before registration must schedule an explicit
 // recheck, since the DMA-fence core will never invoke this callback.
 let retired=fence(-2);{let _slot=FenceWake::new(retired.clone(),events.clone(),23).unwrap();}
 assert_eq!(*events.wakes.borrow(),vec![17,23]);assert_eq!(retired.0.removals.get(),0);assert_eq!(Rc::strong_count(&retired.0),1);
 let rejected=fence(-5);assert!(matches!(FenceWake::new(rejected.clone(),events.clone(),29),Err(Error(-5))));
 assert_eq!(rejected.0.removals.get(),0);assert_eq!(Rc::strong_count(&rejected.0),1);
 {let _unused=FenceWake::unbound(events.clone(),31).unwrap();}
 assert_eq!(Arc::strong_count(&events),1);
 println!("PASS actual receipt callback: stable preallocation, exact owner wake, retirement-before-registration wake, synchronized removal, allocation/error/unused teardown");
}
'''
a.evidence.mkdir(parents=True,exist_ok=False)
(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021','-O',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True)
(a.evidence/'output.txt').write_text(out)
(a.evidence/'source.json').write_text(json.dumps({'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'GPU_execution':False},indent=2)+'\n')
print(out,end='')
