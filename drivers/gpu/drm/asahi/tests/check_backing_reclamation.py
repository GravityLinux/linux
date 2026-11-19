#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Exercise the actual Session reap method with counted owners and busy gates.

This checks host ownership decisions, not firmware quiescence or UAT hardware.
The latter still require the on-device churn/copy/presentation qualification.
"""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
source = (root/'g17p_boot.rs').read_text()
start = source.index('    pub(crate) fn reap_source_backing(')
brace = source.index('{', start)
depth = 1
end = brace + 1
while depth:
    depth += (source[end] == '{') - (source[end] == '}')
    end += 1
method = source[start:end]
queues = (root/'g17p_compute_queues.rs').read_text()
qstart = queues.index('    pub(crate) fn reclaim_idle(')
qend = queues.index('\n    }', qstart)+6
queue_idle = queues[qstart:qend]
transport = (root/'scheduled_transport.rs').read_text()
tstart = transport.index('    pub(crate) fn idle(')
tend = transport.index('\n    }', tstart)+6
transport_idle = transport[tstart:tend]
harness = r'''
extern crate self as kernel;
use std::sync::{Arc, atomic::{AtomicBool,AtomicU32,AtomicUsize,Ordering}};
static INVALIDATIONS:AtomicUsize=AtomicUsize::new(0);
static LAST_DROP_INVALIDATION:AtomicUsize=AtomicUsize::new(0);
mod module_parameters {
    use super::*;
    pub static RELEASE:AtomicU32=AtomicU32::new(1);
    pub struct Param;
    impl Param {pub fn value(&self)->u32 {RELEASE.load(Ordering::Relaxed)}}
    #[allow(non_upper_case_globals)] pub static release_backing:Param=Param;
}
pub mod bindings { pub unsafe fn dma_fence_get_status(status:i32)->i32 {status} }
mod g17p_drm {
    use super::*;
    pub struct Object;
    impl Drop for Object {fn drop(&mut self) {
        LAST_DROP_INVALIDATION.store(INVALIDATIONS.load(Ordering::Relaxed),Ordering::Relaxed);
    }}
    pub fn sole_backing_reference(bo:&Arc<Object>)->bool {Arc::strong_count(bo)==1}
}
mod boot {
    use super::*;
    use std::collections::BTreeMap;
    #[derive(PartialEq)] pub enum Phase {Running,Failed}
    pub struct Vm;
    impl Vm {fn invalidate_gpu(){INVALIDATIONS.fetch_add(1,Ordering::Relaxed);}}
    pub struct Queue {pub busy:bool,pub maintenance:bool}
    impl Queue {fn reclaim_idle(&self)->bool {!self.busy && !self.maintenance}}
    pub struct Cleanup {pub idle:bool}
    impl Cleanup {fn require_idle(&self)->Result<(),()> {if self.idle {Ok(())}else{Err(())}}}
    pub struct Transport {pub idle:bool}
    impl Transport {fn idle(&self)->bool {self.idle}}
    pub struct Fence(pub i32);
    impl Fence {fn raw(&self)->i32 {self.0}}
    pub struct Session {
        pub phase:Phase,pub native:Option<()>,pub pending:Option<()>,
        pub compute_pending:Option<()>,pub render_control:Option<()>,
        pub independent_pending:Vec<()>,pub independent_compute:Queue,
        pub completions:Vec<()>,pub cleanup:Cleanup,
        pub render_prepare_leases:Vec<(u32,Arc<AtomicBool>)>,
        pub render_cpu_leases:Vec<(u32,Fence)>,pub g16c_transport:Option<Transport>,
        pub retained_buffers:Vec<Arc<g17p_drm::Object>>,
        pub retained_buffer_ids:BTreeMap<usize,()>,pub retained_since_reap:usize,
    }
    impl Session {
        pub fn new()->Self {Self {phase:Phase::Running,native:None,pending:None,
            compute_pending:None,render_control:None,independent_pending:vec![],
            independent_compute:Queue {busy:false,maintenance:false},completions:vec![],cleanup:Cleanup {idle:true},
            render_prepare_leases:vec![],render_cpu_leases:vec![],g16c_transport:None,
            retained_buffers:vec![],retained_buffer_ids:BTreeMap::new(),retained_since_reap:64}}
        pub fn add(&mut self,bo:Arc<g17p_drm::Object>) {
            self.retained_buffer_ids.insert(Arc::as_ptr(&bo) as usize,());
            self.retained_buffers.push(bo);
        }
        METHOD
    }
}
use boot::*;
mod actual_idle {
    pub struct Owner {pub ready:bool,pub reconfiguration:Option<()>}
    impl Owner {fn idle(&self)->bool {self.ready}}
    pub struct Queues {pub owners:Vec<Owner>}
    impl Queues { QUEUE_IDLE }
    pub struct Flight {pub quiescent:bool}
    pub struct StateG16C {pub poisoned:bool,pub flights:Vec<Flight>}
    impl StateG16C { TRANSPORT_IDLE }
    pub fn test() {
        let mut q=Queues {owners:vec![]};assert!(q.reclaim_idle());
        q.owners.push(Owner {ready:false,reconfiguration:None});assert!(!q.reclaim_idle());
        q.owners[0].ready=true;assert!(q.reclaim_idle());
        q.owners[0].reconfiguration=Some(());assert!(!q.reclaim_idle());
        let mut t=StateG16C {poisoned:false,flights:vec![]};assert!(t.idle());
        t.flights.push(Flight {quiescent:false});assert!(!t.idle());
        t.flights[0].quiescent=true;assert!(t.idle());
        t.poisoned=true;assert!(!t.idle());
    }
}
fn dead(s:&mut Session)->std::sync::Weak<g17p_drm::Object> {
    let bo=Arc::new(g17p_drm::Object);let weak=Arc::downgrade(&bo);s.add(bo);weak
}
fn main() {
    actual_idle::test();
    let gates:Vec<fn(&mut Session)>=vec![
        |s|s.phase=Phase::Failed,|s|s.native=Some(()),|s|s.pending=Some(()),
        |s|s.compute_pending=Some(()),|s|s.render_control=Some(()),
        |s|s.independent_pending.push(()),|s|s.independent_compute.busy=true,
        |s|s.independent_compute.maintenance=true,
        |s|s.completions.push(()),|s|s.cleanup.idle=false,
        |s|s.render_prepare_leases.push((0,Arc::new(AtomicBool::new(true)))),
        |s|s.render_cpu_leases.push((0,Fence(0))),
        |s|s.g16c_transport=Some(Transport {idle:false}),|s|s.retained_since_reap=63,
    ];
    for gate in &gates {
        let mut s=Session::new();let weak=dead(&mut s);gate(&mut s);
        let epoch=INVALIDATIONS.load(Ordering::Relaxed);s.reap_source_backing();
        assert!(weak.upgrade().is_some());assert_eq!(s.retained_buffers.len(),1);
        assert_eq!(s.retained_buffer_ids.len(),1);
        assert_eq!(INVALIDATIONS.load(Ordering::Relaxed),epoch);
    }
    let mut s=Session::new();let weak=dead(&mut s);
    module_parameters::RELEASE.store(0,Ordering::Relaxed);s.reap_source_backing();
    assert!(weak.upgrade().is_some());assert_eq!(s.retained_since_reap,64);
    module_parameters::RELEASE.store(1,Ordering::Relaxed);
    let live=Arc::new(g17p_drm::Object);s.add(live.clone());
    s.render_prepare_leases.push((0,Arc::new(AtomicBool::new(false))));
    s.render_cpu_leases.push((0,Fence(1)));s.render_cpu_leases.push((1,Fence(-5)));
    s.g16c_transport=Some(Transport {idle:true});
    let epoch=INVALIDATIONS.load(Ordering::Relaxed);s.reap_source_backing();
    assert!(weak.upgrade().is_none());assert_eq!(Arc::strong_count(&live),2);
    assert_eq!(LAST_DROP_INVALIDATION.load(Ordering::Relaxed),epoch+1);
    assert_eq!(s.retained_buffers.len(),1);assert_eq!(s.retained_buffer_ids.len(),1);
    assert!(s.retained_buffer_ids.contains_key(&(Arc::as_ptr(&live) as usize)));
    assert_eq!(INVALIDATIONS.load(Ordering::Relaxed),epoch+1);assert_eq!(s.retained_since_reap,0);
    // No invalidation for a scan that has only reachable backing.
    s.retained_since_reap=64;s.reap_source_backing();
    assert_eq!(INVALIDATIONS.load(Ordering::Relaxed),epoch+1);
    // Dropping a live root/handle makes its last retained reference eligible.
    let weak_live=Arc::downgrade(&live);drop(live);s.retained_since_reap=64;
    s.reap_source_backing();assert!(weak_live.upgrade().is_none());
    assert!(s.retained_buffers.is_empty());assert!(s.retained_buffer_ids.is_empty());
    // A blocked pass must stay armed until the receipt actually retires.
    let weak=dead(&mut s);s.retained_since_reap=64;s.pending=Some(());
    s.reap_source_backing();assert!(weak.upgrade().is_some());assert_eq!(s.retained_since_reap,64);
    s.pending=None;s.reap_source_backing();assert!(weak.upgrade().is_none());
    println!("PASS actual reap method: {} busy/batch gates, disabled mode, live owners, index removal, TLB batching, deferred retry",gates.len());
}
'''.replace('METHOD',method).replace('QUEUE_IDLE',queue_idle).replace('TRANSPORT_IDLE',transport_idle)
with tempfile.TemporaryDirectory() as directory:
    path=Path(directory)
    (path/'reap.rs').write_text(harness)
    subprocess.run(['rustc','--edition=2021','-Awarnings',str(path/'reap.rs'),'-o',str(path/'reap')],check=True)
    subprocess.run([str(path/'reap')],check=True)
