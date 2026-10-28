#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
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
def normalize(s):
    # Only source API spelling changes from the Linux 7.3 port. Firmware
    # construction, scheduling decisions and lifetime transitions stay compared.
    s=re.sub(r'\*\s*((?:crate::)?module_parameters::[a-zA-Z0-9_]+\.value\(\))',r'\1',s)
    s=s.replace('dev.as_ref().as_ref()', 'dev.as_ref()')
    s=s.replace('KBox::pin_init(request.iomap_sized::<SGX_SIZE>(), GFP_KERNEL)?',
                'request.iomap_sized::<SGX_SIZE>()?.into_devres()?')
    s=re.sub(r'(g17p_drm::register\(\s*)pdev\.as_ref\(\)',r'\1pdev',s)
    return re.sub(r'\s+','',re.sub(r'//[^\n]*','',s))
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
new_schedule=(root/'g17p_schedule.rs').read_text().replace('runtime.image.g17p()?','&runtime.image').replace('dev.as_ref().as_ref()', 'dev.as_ref()')
new_schedule=new_schedule.replace('runtime.session.poll_scheduled_g16c_work(dev.as_ref())','runtime.session.poll_g16c_work()')
new_schedule=new_schedule.replace("""super::super::g17p_compute_queues::build_private(&mut memory,
                    slot.as_mut().ok_or(EIO)?, parameters,matches!(dev.abi,super::super::GpuAbi::G16C))?;""",
    """super::super::g17p_compute_runtime::build_independent_client(&mut memory,
                    slot.as_mut().ok_or(EIO)?, parameters)?;""")

# Project only the exact G16C private-backing arm and guard. The Neo render
# arm still returns private=false, so engine==1 is redundant for Neo here.
max_private_arm="""Command::Render(p) if matches!(dev.abi, super::super::GpuAbi::G16C) => {
                        let mut p=*p;p.queue_owner=Some(prepared.queue_key);
                        (runtime.session.g16c_client(prepared.queue_key),runtime.session.g16c_render_needs_private(snapshot.client(),&p))
                    },"""
assert new_schedule.count(max_private_arm)==1
new_schedule=new_schedule.replace(max_private_arm,'')
max_private_guard='if engine==0 && matches!(dev.abi,super::super::GpuAbi::G16C)'
assert new_schedule.count(max_private_guard)==1
start=new_schedule.index(max_private_guard);brace=new_schedule.index('{',start)
end=brace+len(body(new_schedule,max_private_guard))+2
new_schedule=new_schedule[:start]+new_schedule[end:]
assert 'Command::Render(_) => (runtime.session.render_client()?, false),' in new_schedule
assert new_schedule.count('if engine==1 && private && prepared.compute_storage.is_none() {')==1
new_schedule=new_schedule.replace('if engine==1 && private && prepared.compute_storage.is_none() {','if private && prepared.compute_storage.is_none() {')
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
# The common scratch phase now also lowers G16C geometry. For Neo the
# Session adapter delegates to the original method and all defaults keep
# g16c_compute=None. Keep its worker body identical after that typed call.
new_scratch_call=""".session.render_scratch_seed_for(
                    prepared.render_snapshot.as_ref().ok_or(EIO)?.client(),parameters,prepared.queue_key)?"""
assert new_schedule.count(new_scratch_call)==1
assert body(new_boot,'pub(crate) fn render_scratch_seed_for(').strip().endswith('self.render_scratch_seed(p)')
new_schedule=new_schedule.replace(new_scratch_call,'.session.render_scratch_seed(parameters)?')
old_scratch=body(old('g17p_render_runtime.rs'),'impl ScratchSeed {')
new_scratch=body((root/'g17p_render_runtime.rs').read_text(),'impl ScratchSeedG17P {')
new_scratch=new_scratch.replace('PreparedScratchG17P','PreparedScratch')
assert normalize(old_scratch)==normalize(new_scratch), 'Neo off-lock scratch encoder changed'
# Shared lease lookup now uses the producer receipt's physical pool rather
# than inferring it from an engine grid. Project only that common helper/call.
marker='    fn retain_dependencies('
start=new_schedule.index(marker);brace=new_schedule.index('{',start)
end=brace+len(body(new_schedule,marker))+2
new_schedule=new_schedule[:start]+new_schedule[end:]
call='Self::retain_dependencies(&mut runtime.session,node,previous,dependencies)?;'
assert new_schedule.count(call)==2
new_schedule=new_schedule.replace(call,"""for point in [previous, dependencies[0], dependencies[1]].into_iter().flatten() {
                            runtime.session.retain_dependency(point, &node.point.completion)?;
                        }""",1)
new_schedule=new_schedule.replace(call,'for point in &waits { runtime.session.retain_dependency(*point, &node.point.completion)?; }',1)
# A common preparation fix lets a cold pool carry its root-rebind plan
# before descriptor geometry exists. The actual-method cold/warm regression
# and hardware controls qualify this intended extension on both backends.
# Project ONLY its four exact syntax changes before comparing all other code.
root_only_changes = {
    '-> Option<(usize,Option<KBox<super::super::g17p_render_runtime::PreparedAppend>>, Option<super::super::g17p_boot::PreparedPoolRebind>)>':
        '-> Option<(usize,KBox<super::super::g17p_render_runtime::PreparedAppend>, Option<super::super::g17p_boot::PreparedPoolRebind>)>',
    '''                let seed = runtime.session.claim_render_preparation(&parameters,snapshot.client()).ok()?;
                // A cold retained pool can need a caller/root rebind before
                // its geometry exists. Preserve that independently prepared
                // root plan even while descriptor preparation must wait.
                if seed.is_none() && rebind.is_none() { continue; }
                (seed,rebind)''':
        '''                let Some(seed) = runtime.session.claim_render_preparation(&parameters,snapshot.client()).ok()? else { continue; };
                (seed,rebind)''',
    'match seed.map(super::super::g17p_render_runtime::PreparationSeed::prepare).transpose() {':
        'match super::super::g17p_render_runtime::PreparationSeed::prepare(seed) {',
    'Some((index,objects,rebind)) => (objects.map(|objects|(index,objects)),rebind.map(|plan|(index,plan))),':
        'Some((index,objects,rebind)) => (Some((index,objects)),rebind.map(|plan|(index,plan))),',
}
for current, previous in root_only_changes.items():
    assert new_schedule.count(current)==1, 'Unexpected shared root-plan preparation change'
    new_schedule=new_schedule.replace(current,previous)
assert normalize(old_schedule)==normalize(new_schedule), 'Neo CPU scheduling changed beyond the documented image, dependency and root-plan preparation adapters'

old_drm=old('g17p_drm.rs');new_drm=(root/'g17p_drm.rs').read_text()
before_params=body(old_drm,'pub(crate) fn register(').split('let events =')[0]
after_params=body(new_drm,"pub(crate) fn register<'bound>(").split('register_params(')[0]
# The downstream barrier allocation moved to bit 29. Keep the shipped Neo
# bit alongside it; all hardware parameters and the old capability stay intact.
assert normalize(after_params).count('features:(1<<29)|(1<<1),') == 1
after_params=normalize(after_params).replace('features:(1<<29)|(1<<1),','features:1<<1,')
assert normalize(before_params)==normalize(after_params), 'Neo identity checks or UAPI parameters changed'
before_submit=normalize(body(old_drm,'fn submit(dev:'))
after_submit=normalize(remove_Max_branches(body(new_drm,'fn submit(dev:')))
assert after_submit.count('render_storage:None,')==1
after_submit=after_submit.replace('render_storage:None,','')
for field in ['render_transport','render_work']:
    assert after_submit.count(field+':None,')==1
    after_submit=after_submit.replace(field+':None,','')
after_submit=after_submit.replace('letmutg16c_kernel_start=0;','').replace('g16c_kernel_start,','')
assert after_submit.count('g16c_kernel_end') == 2, 'Max private limit escaped its zero/default metadata slots'
assert 'g16c_kernel_end = vm.kernel_end;' in body(new_drm,'fn submit(dev:'), 'Max must use the actual caller bound'
after_submit=after_submit.replace('letmutg16c_kernel_end=0;','').replace('g16c_kernel_end,','')
after_submit=after_submit.replace('letmutparsed=None;','').replace(
    'let(mutparameters,barriers)=matchparsed{Some(parameters)=>parameters,None=>Self::parameters(vm,&state.objects,&bytes)?,};',
    'let(mutparameters,barriers)=Self::parameters(vm,&state.objects,&bytes)?;')
assert before_submit==after_submit, 'Neo ioctl admission changed beyond the immutable Max gate'
assert normalize(body(new_drm,'fn parameters('))=='Self::parameters_with_limits(vm,objects,bytes,1024,2|(1<<3)|(1<<18)|(1<<29))', 'Neo sampler/flag limits changed beyond the downstream barrier alias'
parser_body=normalize(body(new_drm,'fn parameters_with_limits('))
assert parser_body.count('kernel_end:vm.kernel_end,') == 1, 'Compute private limit must come directly from the immutable VM'
parser_body=parser_body.replace('kernel_end:vm.kernel_end,','')
parser_body=parser_body.replace('fragment_count,sampler_limit,render_flags','fragment_count').replace('sampler_limit','1024')
assert normalize(body(old_drm,'fn parameters('))==parser_body, 'Shared parameter parser changed beyond explicit hardware policy'
render_body=normalize(body(new_drm,'fn render_parameters(')).replace('u32::from(cmd.sampler_count)','cmd.sampler_count').replace('sampler_limit','1024').replace('!render_flags','!(2|(1<<3)|(1<<18))')
assert render_body.count('p.kernel_start=vm.kernel_start;')==1 and render_body.count('p.kernel_end=vm.kernel_end;')==1, 'Render VM reservation must come from the immutable VM'
render_body=render_body.replace('p.kernel_start=vm.kernel_start;','').replace('p.kernel_end=vm.kernel_end;','')
barrier_alias=normalize('p.vdm_barrier_fragment = cmd.flags & ((1 << 29) | (1 << 3)) != 0;')
assert render_body.count(barrier_alias) == 1
render_body=render_body.replace(barrier_alias,normalize('p.vdm_barrier_fragment = cmd.flags & (1 << 3) != 0;'))
# The measured Neo Vulkan fix intentionally removes one inherited heuristic:
# fragment attachments cannot select a few-primitives dispatch mode. The real
# parser regression also requires this change to fail on the old baseline.
fixed_dispatch=normalize('p.aux_fb_flags = 0xc000 | (cmd.flags as u64 & (1 << 18));')
legacy_dispatch=normalize('p.aux_fb_flags = (if fragment_count > 1 { 0xc000 } else { 0xc001 }) | (cmd.flags as u64 & (1 << 18));')
assert render_body.count(fixed_dispatch)==1
assert normalize(body(old_drm,'fn render_parameters(')).count(legacy_dispatch)==1
render_body=render_body.replace(fixed_dispatch,legacy_dispatch)
assert normalize(body(old_drm,'fn render_parameters('))==render_body, 'Neo render admission changed beyond explicit hardware policy and the measured dispatch fix'
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
# The common unplug fix changes only Rust field destruction order. Preserve
# every worker/admission instruction while requiring the device to outlive GEMs.
execution=normalize(body((root/'g17p_async.rs').read_text(),'struct Execution'))
assert execution.endswith('dev:ARef<Device>,'), 'DRM device must drop after every GEM owner'
after_async=after_async.replace('structExecution{file:','structExecution{dev:ARef<Device>,file:').replace('control_timer_handle:Option<ArcHrTimerHandle<Job>>,dev:ARef<Device>,}', 'control_timer_handle:Option<ArcHrTimerHandle<Job>>,}')
# Linux 7.3 replaces the deprecated unbound queue with system_dfl().
# Only that named API migration may differ from the pinned worker snapshot.
assert before_async.count('workqueue::system_unbound()')==6
assert after_async.count('workqueue::system_dfl()')==6
before_async=before_async.replace('workqueue::system_unbound()', 'workqueue::system_dfl()')
assert before_async==after_async,'Neo async worker changed beyond common device/GEM drop order and queue API migration'

summary={'baseline':baseline,'identical_Session_initializer_fields':len(before),
         'Neo_probe_body_preserved':True,'linux73_registration_mapping_and_parameter_API_projection':True,'GPU_execution_tested':False,
         'Neo_CPU_scheduler_preserved_except_documented_image_dependency_and_root_plan_preparation_adapters':True,
         'shared_root_only_preparation_extension':True,
         'Neo_GET_PARAMS_and_identity_checks_preserved':True,'Neo_attachment_dispatch_heuristic_removed':True,
         'Neo_shipped_fragment_barrier_bit_preserved_with_downstream_alias':True,
         'Neo_submit_admission_preserved_except_Max_gate':True,
         'Neo_parameter_parser_and_sampler_limit_preserved':True,
         'Neo_permanent_BO_retention_preserved':True,
         'Neo_queue_create_destroy_preserved_except_Max_activity_owner':True,
         'Neo_entire_async_worker_preserved_except_Max_only_hooks_and_common_GEM_drop_order':True,
         'source_sha256':{name:hashlib.sha256((root/name).read_bytes()).hexdigest() for name in ['asahi_neo.rs','g17p_boot.rs','g17p_drm.rs','g17p_schedule.rs','g17p_async.rs']}}
if args.evidence:
    args.evidence.mkdir(parents=True,exist_ok=False)
    (args.evidence/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(f'PASS: {len(before)} tagged Neo Session defaults, probe, CPU scheduler outside documented adapters, identity/GET_PARAMS and submit admission preserved')
