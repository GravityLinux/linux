#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Host source checks: G16 flight and job bookkeeping is preallocated to fixed
bounds, so run_job, the job worker and retirement never allocate.

The bounds: MAX_FLIGHTS Works in flight (admission returns EBUSY at the cap)
and JOB_CREDITS scheduler credits (one per job, so at most that many jobs are
between run_job and their fence). No device or kernel build is needed.
"""
from pathlib import Path
import re

here = Path(__file__).resolve()
root = next(p for p in here.parents if (p / 'drivers/gpu/drm/asahi/g16_runtime.rs').exists())
driver = root / 'drivers/gpu/drm/asahi'
runtime = (driver / 'g16_runtime.rs').read_text()
drm = (driver / 'g16_drm.rs').read_text()
g16 = (driver / 'g16.rs').read_text()


def function(source, signature, nth=0):
    starts = [m for m in re.finditer(r'\n(\s*)(?:pub\(crate\) )?fn ' + re.escape(signature), source)]
    assert len(starts) > nth, signature
    m = starts[nth]
    indent = m.group(1).lstrip('\n')
    end = re.search(r'\n' + indent + r'}\n', source[m.end():])
    return source[m.start():m.end() + end.end()]


def allocates(body):
    return 'GFP_KERNEL' in body or re.search(r'\bKVec::with_capacity\b|\.reserve\(', body)


checks = 0
# The paths in the fence-signalling section that this series bounds.
worker = next(function(drm, 'run(this: Arc<Self>)', n) for n in range(2)
              if 'engine.take_finished()' in function(drm, 'run(this: Arc<Self>)', n))
for name, body in [('run_job', function(drm, 'run(job: &mut sched::Job<Self>)')),
                   ('job worker', worker),
                   ('reserve_flight', function(runtime, 'reserve_flight(')),
                   ('take_finished', function(drm, 'take_finished(')),
                   ('admit', function(drm, 'admit(')),
                   ('Ledger::add', function(runtime, 'add(&mut self, f: &Flight)')),
                   ('stage_queue', function(runtime, 'stage_queue('))]:
    assert not allocates(body), f'{name} allocates'
    checks += 1

# Retirement: no reserve, and every push goes into reserved capacity.
retire = function(runtime, 'retire(&mut self)')
scan = function(runtime, 'service_flights(') if 'fn service_flights(' in runtime else retire
for body in (retire, scan):
    assert '.reserve(' not in body and not re.search(r'\.push\([^)]*GFP_KERNEL', body)
checks += 1

# Preallocated at init to the bounds.
for field, size in [('flights', 'render_runtime::MAX_FLIGHTS'), ('retired', 'render_runtime::MAX_FLIGHTS'),
                    ('publications', '2 * render_runtime::MAX_FLIGHTS')]:
    assert f'{field}: KVec::with_capacity({size}, GFP_KERNEL)?' in g16, field
assert 'Ledger::with_capacity(render_runtime::MAX_FLIGHTS)?' in g16
assert re.search(r'lanes: KVec::with_capacity\(2 \* flights', runtime)
for field in ('jobs', 'finished'):
    assert f'{field}: KVec::with_capacity(JOB_CREDITS as usize, GFP_KERNEL)?' in drm, field
assert 'issued: KVec::with_capacity(crate::g16::MAX_FLIGHTS, GFP_KERNEL)?' in drm
checks += 1

# The scheduler's credit limit is the same constant, and every job takes one.
assert re.search(r'sched::Scheduler::new\(pdev\.as_ref\(\), \d+, JOB_CREDITS,', drm)
assert re.findall(r'\.new_job\((\d+),', drm) == ['1']
checks += 1

# At the cap, admission waits (EBUSY), it does not grow anything.
reserve = function(runtime, 'reserve_flight(')
assert re.search(r'self\.flights\.len\(\) >= MAX_FLIGHTS[\s\S]*?return Err\(EBUSY\)', reserve)
checks += 1

# A job's history holds one entry per command plus the previous stamp, and
# SUBMIT caps a submission at 64 commands.
submit = function(drm, 'submit(')
assert 'events.reserve(65, GFP_KERNEL)?' in submit and 'renders + computes == 64' in submit
assert 'job.history.take()' in function(drm, 'run(job: &mut sched::Job<Self>)')
checks += 1

print(f'test-bounded-flights: {checks} checks passed')
