#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only OR MIT
"""Freeze Neo's constructor defaults/probe body while sharing Session ownership."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--evidence',type=Path)
args=parser.parse_args()
tests=Path(__file__).resolve().parent
root=tests.parent
linux=tests.parents[4]
baseline='a6883c3c6e5aa42548543a5a7b82bb6e375d6922'
def old(name):return subprocess.check_output(['git','show',baseline+':'+str((root/name).relative_to(linux))],cwd=linux,text=True)
def normalize(s):return re.sub(r'\s+','',re.sub(r'//[^\n]*','',s))
def body(text,marker):
    a=text.index('{',text.index(marker))+1;depth=1;b=a
    while depth:
        if text[b]=='{':depth+=1
        if text[b]=='}':depth-=1
        b+=1
    return text[a:b-1]
def fields(text,start):
    content=body(text[start:],'Self {');parts=[];part='';depth=0
    for ch in content:
        if ch in '([{':depth+=1
        elif ch in ')]}':depth-=1
        if ch==',' and depth==0:
            if part.strip():parts.append(part.strip())
            part=''
        else:part+=ch
    if part.strip():parts.append(part.strip())
    result={}
    for part in parts:
        m=re.match(r'(\w+)\s*:(?!:)(.*)',part,re.S)
        key,value=m.groups() if m else (part.strip(),part.strip())
        result[key]=normalize(value)
    return result
old_boot=old('g17p_boot.rs');new_boot=(root/'g17p_boot.rs').read_text()
before=fields(old_boot,old_boot.index('pub(crate) fn prepare('))
after=fields(new_boot,new_boot.index('fn unstarted('))
for name in ['g16c_transport','g16c_compute','g16c_sgx']:
    assert after.pop(name) == 'None', 'Neo must not construct Max owners: '+name
after.update(ttbs='platform.regions[0].base',memory='Some(Memory::new(dev,platform)?)',events='events.clone()')
assert before==after,(before.keys()^after.keys(),[(k,before[k],after.get(k)) for k in before if before[k]!=after.get(k)])
old_driver=old('asahi_neo.rs');new_driver=(root/'asahi_neo.rs').read_text()
old_probe=body(old_driver,'fn probe(')
new_probe=body(new_driver,'fn probe_g17p(')
anchor='let request = pdev.io_request_by_name(c_str!("sgx")).ok_or(EINVAL)?;'
old_probe=old_probe[old_probe.index(anchor):]
new_probe=new_probe[new_probe.index(anchor):].replace('Ok(Self::G17P {','Ok(Self {').replace('image: g17p_drm::RuntimeImage::G17P(image),', 'image,')
assert normalize(old_probe)==normalize(new_probe),'Neo probe path changed beyond enum ownership wrapper'
def remove_Max_branches(text):
    marker='if matches!(dev.abi, '
    while marker in text:
        start=text.index(marker);at=start;parens=0
        while True:
            ch=text[at]
            if ch=='(':parens+=1
            elif ch==')':parens-=1
            elif ch=='{' and parens==0:break
            at+=1
        assert "GpuAbi::G16C" in text[start:at] and "GpuAbi::G17P" not in text[start:at]
        depth=1;end=at+1
        while depth:
            if text[end]=='{':depth+=1
            elif text[end]=='}':depth-=1
            end+=1
        text=text[:start]+text[end:]
    return text
old_schedule=old('g17p_schedule.rs')
new_schedule=(root/'g17p_schedule.rs').read_text().replace('runtime.image.g17p()?','&runtime.image')
new_schedule=new_schedule.replace("""Command::Render(_) if matches!(dev.abi, super::super::GpuAbi::G16C) =>
                        (runtime.session.g16c_client(prepared.queue_key), false),""", '')
new_schedule=new_schedule.replace(
    'let selected = if matches!(dev.abi, super::super::GpuAbi::G16C) {old} else if let Command::Render(p) = command {',
    'let selected = if let Command::Render(p) = command {')
new_schedule=new_schedule.replace("""Command::Compute(_) if matches!(dev.abi, super::super::GpuAbi::G16C) =>
                        (runtime.session.g16c_client(prepared.queue_key), false),""",'')
new_schedule=new_schedule.replace("""let (poll_result, mut completions) = match &runtime.image {
            super::RuntimeImage::G16C(_) => runtime.session.poll_g16c_work(),
            super::RuntimeImage::G17P(image) => runtime.session.poll_work_deferred(dev.as_ref(), image),
        };""", 'let (poll_result, mut completions) = runtime.session.poll_work_deferred(dev.as_ref(), &runtime.image);')
new_schedule=remove_Max_branches(new_schedule)
assert normalize(old_schedule)==normalize(new_schedule), 'Neo CPU scheduling changed beyond typed image access'
old_drm=old('g17p_drm.rs');new_drm=(root/'g17p_drm.rs').read_text()
before_params=body(old_drm,'pub(crate) fn register(').split('let events =')[0]
after_params=body(new_drm,'pub(crate) fn register(').split('register_params(')[0]
assert normalize(before_params)==normalize(after_params), 'Neo identity checks or UAPI parameters changed'
before_submit=normalize(body(old_drm,'fn submit(dev:'))
after_submit=normalize(remove_Max_branches(body(new_drm,'fn submit(dev:')))
after_submit=after_submit.replace('letmutg16c_kernel_start=0;','').replace('g16c_kernel_start,','')
assert after_submit.count('g16c_kernel_end') == 2, 'Max private limit escaped its zero/default metadata slots'
assert 'g16c_kernel_end = vm.kernel_end;' in body(new_drm,'fn submit(dev:'), 'Max must use the actual caller bound'
after_submit=after_submit.replace('letmutg16c_kernel_end=0;','').replace('g16c_kernel_end,','')
after_submit=after_submit.replace('letmutparsed=None;','').replace(
    'let(mutparameters,barriers)=matchparsed{Some(parameters)=>parameters,None=>Self::parameters(vm,&state.objects,&bytes)?,};',
    'let(mutparameters,barriers)=Self::parameters(vm,&state.objects,&bytes)?;')
assert before_submit==after_submit, 'Neo ioctl admission changed beyond the immutable Max gate'
assert normalize(body(new_drm,'fn parameters('))=='Self::parameters_with_limits(vm,objects,bytes,1024,2|(1<<3)|(1<<18))', 'Neo sampler/flag limits changed'
parser_body=normalize(body(new_drm,'fn parameters_with_limits('))
assert parser_body.count('kernel_end:vm.kernel_end,') == 1, 'Compute private limit must come directly from the immutable VM'
parser_body=parser_body.replace('kernel_end:vm.kernel_end,','')
parser_body=parser_body.replace('fragment_count,sampler_limit,render_flags','fragment_count').replace('sampler_limit','1024')
assert normalize(body(old_drm,'fn parameters('))==parser_body, 'Shared parameter parser changed beyond explicit hardware policy'
render_body=normalize(body(new_drm,'fn render_parameters(')).replace('u32::from(cmd.sampler_count)','cmd.sampler_count').replace('sampler_limit','1024').replace('!render_flags','!(2|(1<<3)|(1<<18))')
assert normalize(body(old_drm,'fn render_parameters('))==render_body, 'Neo render admission changed beyond explicit hardware policy'
before_retention=normalize(body(old_boot,'pub(crate) fn retain_source_backing('))
after_retention=normalize(body(new_boot,'pub(crate) fn retain_source_backing(')).replace('ifself.g16c_transport.is_some(){returnOk(());}','')
assert before_retention==after_retention, 'Neo permanent BO ownership changed'
before_create=normalize(body(old_drm,'fn queue_create('))
after_create=normalize(remove_Max_branches(body(new_drm,'fn queue_create(')))
after_create=after_create.replace('letmutactive_jobs=None;','').replace('QueueHistory::new(active_jobs)?','QueueHistory::new()?')
assert before_create==after_create,'Neo queue admission changed beyond Max-only activity owner'
assert normalize(body(old_drm,'fn queue_destroy('))==normalize(remove_Max_branches(body(new_drm,'fn queue_destroy('))), 'Neo queue destruction changed'
before_async=normalize(old('g17p_async.rs'))
after_async=(root/'g17p_async.rs').read_text().replace('if matches!(execution.dev.abi,','if matches!(dev.abi,')
after_async=normalize(remove_Max_branches(after_async)).replace('letmuterror=matchresult{','leterror=matchresult{')
assert before_async==after_async,'Neo async acceptance/retirement changed beyond Max-only activity hooks'

summary={'baseline':baseline,'identical_Session_initializer_fields':len(before),
         'Neo_probe_body_preserved':True,'GPU_execution_tested':False,
         'Neo_CPU_scheduler_preserved_excluding_G16C_only_branches':True,
         'Neo_GET_PARAMS_and_identity_checks_preserved':True,
         'Neo_submit_admission_preserved_except_Max_gate':True,
         'Neo_parameter_parser_and_sampler_limit_preserved':True,
         'Neo_permanent_BO_retention_preserved':True,
         'Neo_queue_create_destroy_preserved_except_Max_activity_owner':True,
         'Neo_entire_async_worker_preserved_except_Max_only_hooks':True,
         'source_sha256':{name:hashlib.sha256((root/name).read_bytes()).hexdigest() for name in ['asahi_neo.rs','g17p_boot.rs','g17p_drm.rs','g17p_schedule.rs','g17p_async.rs']}}
if args.evidence:
    args.evidence.mkdir(parents=True,exist_ok=False)
    (args.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(f'PASS: {len(before)} tagged Neo Session defaults, probe, CPU scheduler, identity/GET_PARAMS and submit admission preserved')
