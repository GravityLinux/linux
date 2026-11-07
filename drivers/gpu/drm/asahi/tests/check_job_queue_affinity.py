#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Execute production queue affinity setup across CPU/hotplug/cpumask configs."""
import argparse,hashlib,json,subprocess
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',required=True,type=Path);a=p.parse_args();src=Path(__file__).resolve().parents[1]/'g17p_async.rs';s=src.read_text()
def item(marker):
 i=s.index(marker);j=s.index('{',i)+1;depth=1
 while depth:depth+=(s[j]=='{')-(s[j]=='}');j+=1
 return s[i:j]
code=r'''
#![allow(dead_code,non_camel_case_types,non_upper_case_globals,unused_variables)]
use std::sync::{Arc,Mutex,MutexGuard,atomic::{AtomicBool,AtomicI32,AtomicUsize,Ordering}};
use std::pin::Pin;type KBox<T>=Box<T>;
#[derive(Debug,PartialEq)]struct Error(i32);impl Error{fn from_errno(e:i32)->Self{Self(e)}}
type Result<T=()>=std::result::Result<T,Error>;const ENOMEM:Error=Error(-12);
mod kernel{pub mod ffi{pub type c_ulong=u64;}}
static FAIL:AtomicI32=AtomicI32::new(0);static APPLIES:AtomicUsize=AtomicUsize::new(0);static ALLOCS:AtomicUsize=AtomicUsize::new(0);static FREES:AtomicUsize=AtomicUsize::new(0);static CPU_LOCK:AtomicUsize=AtomicUsize::new(0);static MASK:Mutex<[u64;4]>=Mutex::new([0;4]);
mod bindings{use super::*;
 pub struct cpumask{pub bits:[u64;4]}
 pub static mut __cpu_online_mask:cpumask=cpumask{bits:[0;4]};pub static mut nr_cpu_ids:u32=256;pub static mut FREQ:[u32;256]=[0;256];pub static mut DOMAIN:[u32;256]=[0;256];
 pub struct cpuinfo{pub max_freq:u32}pub struct cpufreq_policy{pub cpu:u32,pub cpuinfo:cpuinfo}
 #[cfg(CONFIG_CPU_FREQ)]pub unsafe fn cpufreq_cpu_get(cpu:u32)->*mut cpufreq_policy{unsafe{if FREQ[cpu as usize]==0{return std::ptr::null_mut();}Box::into_raw(Box::new(cpufreq_policy{cpu:DOMAIN[cpu as usize],cpuinfo:cpuinfo{max_freq:FREQ[cpu as usize]}}))}}
 #[cfg(CONFIG_CPU_FREQ)]pub unsafe fn cpufreq_cpu_put(p:*mut cpufreq_policy){unsafe{drop(Box::from_raw(p));}}
 #[cfg(CONFIG_HOTPLUG_CPU)]pub unsafe fn cpus_read_lock(){assert_eq!(CPU_LOCK.fetch_add(1,Ordering::SeqCst),0);}
 #[cfg(CONFIG_HOTPLUG_CPU)]pub unsafe fn cpus_read_unlock(){assert_eq!(CPU_LOCK.fetch_sub(1,Ordering::SeqCst),1);}
 pub struct workqueue_struct;
 #[cfg(CONFIG_CPUMASK_OFFSTACK)]pub struct workqueue_attrs{pub cpumask:*mut cpumask}
 #[cfg(not(CONFIG_CPUMASK_OFFSTACK))]pub struct workqueue_attrs{pub cpumask:[cpumask;1]}
 pub unsafe fn alloc_workqueue_attrs_noprof()->*mut workqueue_attrs{
  ALLOCS.fetch_add(1,Ordering::Relaxed);if FAIL.load(Ordering::Relaxed)==1{return std::ptr::null_mut()}
  #[cfg(CONFIG_CPUMASK_OFFSTACK)]let value=workqueue_attrs{cpumask:Box::into_raw(Box::new(cpumask{bits:[u64::MAX;4]}))};
  #[cfg(not(CONFIG_CPUMASK_OFFSTACK))]let value=workqueue_attrs{cpumask:[cpumask{bits:[u64::MAX;4]}]};
  Box::into_raw(Box::new(value))
 }
 pub unsafe fn free_workqueue_attrs(a:*mut workqueue_attrs){FREES.fetch_add(1,Ordering::Relaxed);let value=unsafe{Box::from_raw(a)};#[cfg(CONFIG_CPUMASK_OFFSTACK)]drop(unsafe{Box::from_raw(value.cpumask)});drop(value);}
 pub unsafe fn apply_workqueue_attrs(q:*mut workqueue_struct,a:*const workqueue_attrs)->i32{
  assert!(!q.is_null());assert_eq!(CPU_LOCK.load(Ordering::Relaxed),0,"do not nest CPU hotplug exclusion around workqueue configuration");
  if FAIL.load(Ordering::Relaxed)==2{return -12}
  #[cfg(CONFIG_CPUMASK_OFFSTACK)]let bits=unsafe{(*(*a).cpumask).bits};
  #[cfg(not(CONFIG_CPUMASK_OFFSTACK))]let bits=unsafe{(*a).cpumask[0].bits};
  *MASK.lock().unwrap()=bits;APPLIES.fetch_add(1,Ordering::Relaxed);0
 }
}
SELECT
struct Lock<T>(Mutex<T>);impl<T>Lock<T>{fn lock(&self)->MutexGuard<'_,T>{self.0.lock().unwrap()}}
struct JobQueue{queue:*mut bindings::workqueue_struct}unsafe impl Send for JobQueue{}unsafe impl Sync for JobQueue{}
struct Events{queue_owner:Option<Pin<KBox<JobQueue>>>,queue_configured:AtomicBool,queue_configuration:Lock<()>}
impl Events{CONFIGURE}
fn events()->Events{Events{queue_owner:Some(Box::pin(JobQueue{queue:std::ptr::NonNull::dangling().as_ptr()})),queue_configured:AtomicBool::new(false),queue_configuration:Lock(Mutex::new(()))}}
fn setup(rows:&[(u32,u32)],offline:&[(u32,u32)]){unsafe{bindings::__cpu_online_mask.bits=[0;4];bindings::FREQ=[0;256];bindings::DOMAIN=[0;256];for &(cpu,freq) in rows{bindings::__cpu_online_mask.bits[cpu as usize/64]|=1<<(cpu%64);bindings::FREQ[cpu as usize]=freq;}for &(cpu,freq)in offline{bindings::FREQ[cpu as usize]=freq;}}FAIL.store(0,Ordering::Relaxed);APPLIES.store(0,Ordering::Relaxed);ALLOCS.store(0,Ordering::Relaxed);FREES.store(0,Ordering::Relaxed);*MASK.lock().unwrap()=[0;4];}
fn mask(cpu:u32)->[u64;4]{let mut a=[0;4];a[cpu as usize/64]=1<<(cpu%64);a}
fn main(){
 assert_eq!(local_job_cpu([(0,4104,0),(15,4104,0),(4,2532,4)].into_iter()),Some(15));assert_eq!(local_job_cpu([(0,4104,0),(15,2532,15)].into_iter()),Some(0));assert_eq!(local_job_cpu([(3,0,3),(200,0,200)].into_iter()),None);assert_eq!(local_job_cpu(std::iter::empty()),None);
 assert_eq!(local_job_cpu([(0,4104,0),(5,4104,0),(9,4104,0),(10,4104,10),(15,4104,10),(1,2532,1)].into_iter()),Some(9));
 #[cfg(CONFIG_CPU_FREQ)]{setup(&[(0,4104),(5,4104),(9,4104),(10,4104),(15,4104)],&[]);unsafe{bindings::DOMAIN[10]=10;bindings::DOMAIN[15]=10;}let e=events();e.configure_queue().unwrap();assert_eq!(*MASK.lock().unwrap(),mask(9));}
 let mut cases=0;
 for (rows,offline,want) in [(vec![(0,4104),(1,2532),(10,4104),(15,4104)],vec![],15),(vec![(0,3900),(1,3900),(2,2400),(5,2400)],vec![],1),(vec![(7,3300),(199,4200)],vec![(250,5000)],199)]{
  setup(&rows,&offline);let e=Arc::new(events());let mut threads=Vec::new();for _ in 0..16{let e=e.clone();threads.push(std::thread::spawn(move||e.configure_queue().unwrap()));}for t in threads{t.join().unwrap();}
  #[cfg(CONFIG_CPU_FREQ)]{assert_eq!(*MASK.lock().unwrap(),mask(want));assert_eq!(APPLIES.load(Ordering::Relaxed),1);assert_eq!(ALLOCS.load(Ordering::Relaxed),1);assert_eq!(FREES.load(Ordering::Relaxed),1);}
  #[cfg(not(CONFIG_CPU_FREQ))]assert_eq!(APPLIES.load(Ordering::Relaxed),0);
  // The once-only setup must not overwrite later operator affinity tuning.
  *MASK.lock().unwrap()=mask(3);e.configure_queue().unwrap();assert_eq!(*MASK.lock().unwrap(),mask(3));assert_eq!(CPU_LOCK.load(Ordering::Relaxed),0);cases+=1;
 }
 setup(&[(0,0),(15,0)],&[]);let e=events();e.configure_queue().unwrap();assert_eq!(APPLIES.load(Ordering::Relaxed),0);assert!(e.queue_configured.load(Ordering::Acquire));
 #[cfg(CONFIG_CPU_FREQ)]for fail in [1,2]{setup(&[(0,4104),(15,4104)],&[]);let e=events();FAIL.store(fail,Ordering::Relaxed);assert_eq!(e.configure_queue(),Err(ENOMEM));assert!(!e.queue_configured.load(Ordering::Acquire));assert_eq!(CPU_LOCK.load(Ordering::Relaxed),0);assert_eq!(FREES.load(Ordering::Relaxed),usize::from(fail==2));FAIL.store(0,Ordering::Relaxed);e.configure_queue().unwrap();assert_eq!(*MASK.lock().unwrap(),mask(15));assert_eq!(APPLIES.load(Ordering::Relaxed),1);}
 println!("PASS actual selection/configuration: {} CPU topologies, offline exclusion, sparse multiword masks, 16 concurrent first submissions, operator override, missing-frequency fallback and allocation/apply retries",cases);
}
'''.replace('SELECT',item('fn local_job_cpu(')).replace('CONFIGURE',item('    fn configure_queue('))
a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'test.rs').write_text(code)
for name,cfg in [('inline-hotplug',['CONFIG_CPU_FREQ','CONFIG_HOTPLUG_CPU']),('offstack-hotplug',['CONFIG_CPU_FREQ','CONFIG_HOTPLUG_CPU','CONFIG_CPUMASK_OFFSTACK']),('inline-no-hotplug',['CONFIG_CPU_FREQ']),('no-cpufreq',[])]:
 cmd=['rustc','--edition=2021','-O'];
 for flag in cfg:cmd+=['--cfg',flag]
 subprocess.run(cmd+[str(a.evidence/'test.rs'),'-o',str(a.evidence/name)],check=True)
 out=subprocess.check_output([str(a.evidence/name)],text=True);(a.evidence/(name+'.txt')).write_text(out);print(name,out,end='')
(a.evidence/'source.json').write_text(json.dumps({'source_sha256':hashlib.sha256(s.encode()).hexdigest(),'GPU_execution':False},indent=2)+'\n')
