#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Compare actual shared render planning to the original Neo instruction body.

Check complete Max records against Neo's ordered dependency bytes with only
Max's captured timeout/control prefix retained. This is a host layout gate.
"""
import argparse,hashlib,json,subprocess,tempfile
from pathlib import Path
import host_versions
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--evidence',type=Path,required=True);a=p.parse_args()
tests=Path(__file__).resolve().parent;root=tests.parents[4];baseline='a6883c3c6e5aa42548543a5a7b82bb6e375d6922'
old=subprocess.check_output(['git','show',baseline+':drivers/gpu/drm/asahi/g17p_render_lifecycle.rs'],cwd=root,text=True)
start=old.index('        let (internal, count) = graph::paired_grid_dependencies(',old.index('    pub(crate) fn context(self'))
end=old.index('        graph::Context {',start);instructions=old[start:end].replace('self.layout.grids','grids').replace('self.index','item')
code='#![allow(dead_code)]\n'
for name in ['g17p_compute','g17p_compute_memory','g17p_render','g17p_render_graph']:
 code+=f'#[path={json.dumps(str(tests.parent/(name+".rs")))}]mod {name};\n'
life=(tests.parent/'g17p_render_lifecycle.rs').read_text();start=life.index('pub(crate) fn pool_grids(');end=life.index('\n}',start)+2
constant=next(line for line in life.splitlines()if line.startswith('pub(crate) const POOL_SLOTS:'))
code+='mod g17p_render_lifecycle{use crate::g17p_compute::{Error,Result};'+constant+life[start:end]+'}\n'
code+='use g17p_compute as c;use g17p_render_graph as graph;use g17p_render::{Kind,Parameters};\n'
code+='fn original(kind:Kind,grids:[u32;2],item:u32,p:&Parameters)->c::Result<([(u8,u32);4],usize)>{\n'+instructions+'Ok((points,count))}\n'
code+=r'''
fn main(){let mut cases=0;
 for grids in [[0,1],[2,3],[6,7],[30,31]]{for item in [0,1,255,256,65535]{for fragment in [false,true]{for late in [false,true]{
 let kind=if fragment{Kind::Fragment}else{Kind::Tiling};
 for (prior,compute,vertex)in [
  (None,None,None),
  (Some((grids[0]as u8,item)),None,None),
  (None,Some((32,1)),None),
  (Some((8,7)),Some((32,9)),Some((16,11))),
  (Some((32,1)),Some((32,7)),Some((32,3))),
  (None,Some((127,(1<<30)-1)),Some((grids[1]as u8,item+2))),
 ]{
 let p=Parameters{prior_queue_ta:prior,cdm_dependency:compute,vdm_dependency:vertex,vdm_barrier_fragment:late,..Default::default()};
 let external=[if !fragment{prior}else{None},if !fragment{compute}else{None},if fragment==late{vertex}else{None}];
 let expected=original(kind,grids,item,&p).unwrap();
 let actual=graph::render_dependencies(kind,grids,item,external).unwrap();assert_eq!(actual,expected);
 let arrays=[(0xa0,73),(0x7c0,16),(0xee0,23),(0x1600,10)];
 let mut record=c::work::WorkRecordG16C::for_render(&c::work::RenderRecordParametersG16C{
  descriptor:0xfffffc20d1049b40,queue:0xfffffc2208000000,grid:grids[usize::from(fragment)]as u8,item,fragment,
  timeout:4000,ready:true,arrays:if fragment{&arrays}else{&arrays[..1]},
  control:Some(if fragment{0x1800}else{0xc00}),control_low:None,field_30:None,field_38:0}).unwrap();
 let before=record.as_bytes().to_vec();let mut expected_record=before.clone();
 let mut points=[0u8;0x110];c::points(&mut points,if fragment{24}else{12},0,&expected.0[..expected.1]).unwrap();
 let prefix=u64::from_le_bytes(before[0x20..0x28].try_into().unwrap())&!0xffffffff;
 let mask=u64::from_le_bytes(points[..8].try_into().unwrap())&0xffffffff;
 points[..8].copy_from_slice(&(prefix|mask).to_le_bytes());expected_record[0x20..0x130].copy_from_slice(&points);
 record.set_dependencies(&actual.0[..actual.1]).unwrap();assert_eq!(record.as_bytes(),expected_record);
 assert_eq!(&record.as_bytes()[..0x20],&before[..0x20]);assert_eq!(&record.as_bytes()[0x130..],&before[0x130..]);cases+=1;
 }
 }}}}
 for fragment in [false,true]{let kind=if fragment{Kind::Fragment}else{Kind::Tiling};
  for bad in [(128,1),(32,1<<30)]{assert!(graph::render_dependencies(kind,[0,1],0,[Some(bad),None,None]).is_err());}
 }
 assert!(graph::render_dependencies(Kind::Fragment,[0,1],0,[Some((2,1)),Some((3,1)),Some((4,1))]).is_err());
 let arrays=[(0x60,73)];let mut r=c::work::WorkRecordG16C::for_render(&c::work::RenderRecordParametersG16C{
  descriptor:0xfffffc20d1042940,queue:0xfffffc2208000000,grid:0,item:0,fragment:false,timeout:4000,ready:true,arrays:&arrays,
  control:Some(0xc00),control_low:None,field_30:None,field_38:0}).unwrap();
 let before=r.as_bytes().to_vec();
 for points in [vec![],vec![(128,1)],vec![(32,1<<30)],vec![(32,1);5]]{assert!(r.set_dependencies(&points).is_err());assert_eq!(r.as_bytes(),before);}
 println!("PASS {cases} original Neo plans and complete Max dependency records; invalid/oversized vectors preserve staging");
}
'''
with tempfile.TemporaryDirectory(prefix='render-dependencies-')as d:
 tmp=Path(d);flags=host_versions.build(tmp);(tmp/'main.rs').write_text(code)
 subprocess.run(['rustc','--edition=2021',*flags,str(tmp/'main.rs'),'-o',str(tmp/'gate')],check=True)
 result=subprocess.check_output([str(tmp/'gate')],text=True);assert result.startswith('PASS 480 '),result
 a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'harness.rs').write_text(code)
 (a.evidence/'summary.json').write_text(json.dumps(dict(cases=480,whole_Max_record_bytes=512,original_Neo_instruction_projection=True,original_Neo_revision=baseline,non_dependency_bytes_preserved=True,invalid_vectors_leave_record_unchanged=True,GPU_execution=False,result=result.strip(),source_sha256={n:hashlib.sha256((tests.parent/n).read_bytes()).hexdigest()for n in ['g17p_render_graph.rs','g17p_render_lifecycle.rs','g17p_compute.rs','scheduled_work.rs']}),indent=2)+'\n')
 print(result,end='')
