#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Host checks: dep_stats dependency counters count published commands only,
and every deferral (EBUSY retry) is counted, by backpressure reason."""
from pathlib import Path
import re

root = next(p for p in Path(__file__).resolve().parents
            if (p / 'drivers/gpu/drm/asahi/g16_runtime.rs').exists())
driver = root / 'drivers/gpu/drm/asahi'
runtime = ''.join(p.read_text() for p in sorted(driver.glob('g16_runtime*.rs')))
drm = ''.join(p.read_text() for p in sorted(driver.glob('g16_drm*.rs')))
timeline = (driver / 'g16_timeline.rs').read_text()

# No DEP_* counter outside DepTally::publish: resolution only tallies.
publish = runtime[runtime.index('    fn publish(&self) {'):]
publish = publish[:publish.index('\n    }\n')]
for name in ('DEP_WORKS', 'DEP_ITEMS', 'DEP_SAME_LANE', 'DEP_REPEAT', 'DEP_REACHED', 'DEP_DROPPED'):
    assert runtime.count(f't::{name}.fetch_add') == 1 and f't::{name}.fetch_add' in publish, name
    assert f'crate::g16_timeline::{name}.fetch_add' not in runtime, name
# Published only: both submit paths commit the tally on an Ok result.
assert runtime.count('if result.is_ok() {\n            tally.publish();\n        }') == 2
# Deferrals: the total where the pump retries, and the reason per pressure bit.
assert re.search(r'Err\(EBUSY\) if !runtime\.render_failed\(\) => \{\s*crate::g16_timeline::DEFERRED\.fetch_add\(1, Ordering::Relaxed\);', drm)
assert 'crate::g16_timeline::DEFERRED_BY.get(bit.trailing_zeros() as usize)' in runtime
assert re.search(r'deferred \{\}\\ndeferred_asids \{\}', timeline)
# CPU holds are counted where they return EBUSY, not inferred.
assert runtime.count('crate::g16_timeline::DEFERRED_CPU_HOLD.fetch_add(1, Ordering::Relaxed);\n') == 2
assert 'deferred_cpu_hold {}' in timeline
print('dep_stats published: ok')
