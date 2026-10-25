#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Host source checks: TVB growth refusals are visible.

At a refusal the firmware only re-requests (about every 355 ms) until pages
free up, so renders crawl and a client's exit drains slowly (2026-10-24).
Every refusal records its reason (cap, render-arena VA, allocation); dep_stats
counts them by reason (summing to tvb_refused); each pool reports once, as a
warning for the first refusal of the boot and as info after that. No
growth, cap or VA behaviour changes.
"""
from pathlib import Path
import re

here = Path(__file__).resolve()
root = next(p for p in here.parents if (p / 'drivers/gpu/drm/asahi/g16_tvb.rs').exists())
driver = root / 'drivers/gpu/drm/asahi'
tvb = (driver / 'g16_tvb.rs').read_text()
runtime = ''.join(p.read_text() for p in sorted(driver.glob('g16_runtime*.rs')))
timeline = (driver / 'g16_timeline.rs').read_text()


def function(source, name):
    m = re.search(r'\n(\s*)(?:pub\((?:crate|super)\) )?fn ' + name + r'\b', source)
    assert m, name
    indent = m.group(1).lstrip('\n')
    end = re.search(r'\n' + indent + r'}\n', source[m.end():])
    return source[m.start():m.end() + end.end()]


checks = 0
grow = function(tvb, 'grow')
# Each Ok(false) is preceded by a recorded reason.
assert grow.count('Ok(false)') == 2
assert re.search(r'if new == old \{\s*self\.refusal = Some\(Refusal::Cap\);\s*return Ok\(false\);', grow)
assert re.search(r'if add == 1 \{\s*self\.refusal = Some\(if client\.tvb_va_blocks\(\) == 0 \{\s*Refusal::Va\s*\} else \{\s*'
                 r'Refusal::Memory\s*\}\);\s*return Ok\(false\);', grow)
# No behaviour change: the cap clamp and growth policy are p2's.
assert 'let limit = (crate::module_parameters::tvb_max_blocks.value()).min(MAX_BLOCKS);' in grow
assert 'let mut new = old.saturating_mul(3).min(limit).max(old);' in grow and 'tvb_va_blocks' not in grow.split('if add == 1')[0]
checks += 1

# dep_stats: three new fields appended after the existing ones.
assert re.search(r'partial_renders \{\}\\n\\\s*tvb_refused_cap \{\}\\ntvb_refused_va \{\}\\ntvb_refused_memory \{\}', timeline)
assert re.search(r'PARTIAL_RENDERS\.load\(Relaxed\),\s*TVB_REFUSED_CAP\.load\(Relaxed\),\s*TVB_REFUSED_VA\.load\(Relaxed\),\s*'
                 r'TVB_REFUSED_MEMORY\.load\(Relaxed\)', timeline)
checks += 1

service = function(runtime, 'service_render_events')
m = re.search(r'if grown \{ &tl::TVB_GROWN \} else \{ &tl::TVB_REFUSED \}\.fetch_add\(1, Relaxed\);\s*if !grown \{(.*?)\n                \}',
              service, re.S)
assert m, 'refusal accounting next to tvb_refused'
body = m.group(1)
for reason, counter in (('Cap', 'TVB_REFUSED_CAP'), ('Va', 'TVB_REFUSED_VA'), ('Memory', 'TVB_REFUSED_MEMORY')):
    assert f'crate::g16_tvb::Refusal::{reason} => &tl::{counter},' in body
# One warning per boot (the first refusal), info for later pools (review V1).
assert re.search(r'if !pool\.warned \{\s*pool\.warned = true;.*?'
                 r'if !crate::g16_tvb::REFUSAL_WARNED\.swap\(true, Relaxed\) \{\s*dev_warn!.*?\} else \{\s*dev_info!', body, re.S)
assert 'pub(crate) static REFUSAL_WARNED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);' in tvb
assert 'return Err' not in body
checks += 1
print(f'test-tvb-refusal-visibility: {checks} checks passed')
