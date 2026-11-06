#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute production snapshot identity and CPU-map pruning with fallible host maps."""
import argparse, hashlib, json, subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
s=(Path(__file__).resolve().parents[1]/'g17p_compute_runtime.rs').read_text()
def item(marker):
 start=s.index(marker);end=s.index('{',start)+1;depth=1
 while depth:depth+=(s[end]=='{')-(s[end]=='}');end+=1
 return s[start:end]
code=r'''
#![allow(dead_code,non_upper_case_globals)]
use std::{cell::{Cell,RefCell},collections::BTreeMap};
type Result<T=()> = std::result::Result<T,i32>;type ARef<T>=std::sync::Arc<T>;
const GFP_KERNEL:u32=0;const EINVAL:i32=22;const EIO:i32=5;
thread_local!{static MAPS:Cell<usize>=const{Cell::new(0)};static DROPS:Cell<usize>=const{Cell::new(0)};static SYNC:Cell<usize>=const{Cell::new(0)};static FAIL:Cell<bool>=const{Cell::new(false)};}
struct Arc<T>(std::sync::Arc<T>);
impl<T> Clone for Arc<T>{fn clone(&self)->Self{Self(self.0.clone())}}
impl<T>Arc<T>{fn new(x:T,_:u32)->Result<Self>{Ok(Self(std::sync::Arc::new(x)))}fn ptr_eq(a:&Self,b:&Self)->bool{std::sync::Arc::ptr_eq(&a.0,&b.0)}}
impl<T>std::ops::Deref for Arc<T>{type Target=T;fn deref(&self)->&T{&self.0}}
struct Mutex<T>(RefCell<T>);impl<T>Mutex<T>{fn lock(&self)->std::cell::RefMut<'_,T>{self.0.borrow_mut()}}
struct RBTree<K,V>(BTreeMap<K,V>);impl<K:Ord,V>RBTree<K,V>{
 fn new()->Self{Self(BTreeMap::new())}fn get(&self,k:&K)->Option<&V>{self.0.get(k)}
 fn try_create_and_insert(&mut self,k:K,v:V,_:u32)->Result{self.0.insert(k,v);Ok(())}
 fn remove_node(&mut self,k:&K)->Option<(K,V)>{self.0.remove_entry(k)}
 fn insert(&mut self,n:(K,V)){self.0.insert(n.0,n.1);}
}
struct Object{weak:std::sync::Weak<Object>,size:usize}
impl Object{
 fn new(size:usize)->ARef<Self>{std::sync::Arc::new_cyclic(|weak|Self{weak:weak.clone(),size})}
 fn size(&self)->usize{self.size}
 fn owned_vmap<T>(&self)->Result<shmem::VMap<(),T>>{
  if FAIL.with(|v|v.replace(false)){return Err(EIO)}
  MAPS.with(|v|v.set(v.get()+1));Ok(shmem::VMap{owner:self.weak.upgrade().unwrap(),phantom:std::marker::PhantomData})
 }
}
mod shmem{use super::*;pub struct VMap<T,U>{pub owner:ARef<Object>,pub phantom:std::marker::PhantomData<(T,U)>}
 impl<T,U>VMap<T,U>{pub fn owner(&self)->&Object{&self.owner}pub fn get(&self)->&Self{self}pub fn is_iomem(&self)->bool{false}pub fn ptr_from_index(&self,i:usize)->Result{if i<self.owner.size{Ok(())}else{Err(EIO)}}}
 impl<T,U>Drop for VMap<T,U>{fn drop(&mut self){DROPS.with(|v|v.set(v.get()+1));}}
}
mod g17p_drm{pub type Bo=();}
mod g17p_memory{pub fn sync(){super::SYNC.with(|v|v.set(v.get()+1));}}
mod actual{use super::*;
'''
code+='#[derive(Clone)]\n'+s[s.index('pub(crate) struct MappingIdentity('):s.index('impl MappingIdentity {')]+'\n'+item('impl MappingIdentity {')+'\n'
code+=item('struct CpuMapState {')+'\n#[derive(Clone)]\nstruct CpuMaps(Arc<Mutex<CpuMapState>>);\n'
code+='''struct Client { identity:Option<MappingIdentity>, owner:(u64,u32),bindings:Vec<(u64,u64,u64,u32)>,buffers:Vec<ARef<Object>> }\nimpl Client {\n'''+item('pub(crate) fn same_mapping(')+'\n}\n'+item('fn cache_buffers(')
code+=r'''
fn cache()->CpuMaps{CpuMaps(Arc::new(Mutex(RefCell::new(CpuMapState{maps:RBTree::new(),identity:None})),0).unwrap())}
fn client(identity:Option<MappingIdentity>,bo:&ARef<Object>)->Client{Client{identity,owner:(1,2),bindings:vec![(0,0x4000,0,1)],buffers:vec![bo.clone()]}}
pub fn run()->Result {
 let a=Object::new(0x4000);let b=Object::new(0x4000);
 let token=Some(MappingIdentity::new()?);let other=Some(MappingIdentity::new()?);
 assert!(MappingIdentity::same(&token,&token.clone()));assert!(!MappingIdentity::same(&token,&other));assert!(!MappingIdentity::same(&None,&None));
 let mut x=client(token.clone(),&a);let y=client(token.clone(),&a);
 assert!(x.same_mapping(&y));assert!(!x.same_mapping(&client(other.clone(),&a)));
 x.identity=None;assert!(x.same_mapping(&y));x.bindings[0].3=2;assert!(!x.same_mapping(&y));
 let mut x=client(token.clone(),&a);x.owner=(1,3);assert!(!x.same_mapping(&y));
 let baseline_a=std::sync::Arc::strong_count(&a);let held=cache();cache_buffers(&[a.clone(),a.clone()],&token,false,&held)?;
 assert_eq!(MAPS.with(Cell::get),1);assert_eq!(held.0.lock().maps.0.len(),1);
 for _ in 0..32{cache_buffers(&[a.clone(),a.clone()],&token,true,&held)?;}
 assert_eq!(MAPS.with(Cell::get),1);assert_eq!(DROPS.with(Cell::get),0);assert_eq!(SYNC.with(Cell::get),33);
 // Retained owner switches; old BO/vmap must be pruned and released.
 cache_buffers(&[b.clone()],&other,false,&held)?;assert_eq!(DROPS.with(Cell::get),1);
 assert_eq!(std::sync::Arc::strong_count(&a),baseline_a);
 assert_eq!(held.0.lock().maps.0.len(),1);
 // Failed switch invalidates the previous identity, leaving valid old maps
 // reusable. Retrying the old client must validate/prune again.
 FAIL.with(|v|v.set(true));assert_eq!(cache_buffers(&[a.clone()],&token,false,&held),Err(EIO));
 assert!(held.0.lock().identity.is_none());cache_buffers(&[b.clone()],&other,false,&held)?;
 assert!(MappingIdentity::same(&held.0.lock().identity,&other));
 // Unqualified primer/mutated clients never certify an identity cache hit.
 cache_buffers(&[a.clone()],&None,false,&held)?;assert!(held.0.lock().identity.is_none());
 cache_buffers(&[],&None,false,&held)?;assert!(held.0.lock().maps.0.is_empty());
 assert_eq!(std::sync::Arc::strong_count(&b),1);
 drop(held);println!("PASS production identity equality, engine/generation distinction, owner checks, unchanged cache hit/order, duplicate BOs, pruning, failures, unqualified mutations and empty-set release");Ok(())
}}
fn main(){actual::run().unwrap();}
'''
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code)
subprocess.run(['rustc','--edition=2021',str(a.evidence/'test.rs'),'-o',str(a.evidence/'gate')],check=True)
out=subprocess.check_output([str(a.evidence/'gate')],text=True);(a.evidence/'output.txt').write_text(out)
(a.evidence/'source.json').write_text(json.dumps({'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'GPU_execution':False},indent=2)+'\n');print(out,end='')
