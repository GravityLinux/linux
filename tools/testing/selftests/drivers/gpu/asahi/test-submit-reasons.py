#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Host source checks: every SUBMIT rejection names the failed check.

A bare EINVAL from DRM_IOCTL_ASAHI_SUBMIT reached userspace as device loss
with nothing in the kernel log (replaydump-kit2-20261024). In submit(), its
timestamp resolver, render_parameters(), compute_parameters() and the mapping
check, every EINVAL/ENOTSUPP goes through reject()/unsupported(), which log
the reason (the first 32 per boot, then one "not logged" line).
"""
from pathlib import Path
import re

here = Path(__file__).resolve()
root = next(p for p in here.parents if (p / 'drivers/gpu/drm/asahi/g16_drm.rs').exists())
drm = ''.join(p.read_text() for p in sorted((root / 'drivers/gpu/drm/asahi').glob('g16_drm*.rs')))


def function(source, name):
    m = re.search(r'\n(\s*)(?:pub\((?:crate|super)\) )?fn ' + name + r'\b', source)
    assert m, name
    indent = m.group(1).lstrip('\n')
    end = re.search(r'\n' + indent + r'}\n', source[m.end():])
    return source[m.start():m.end() + end.end()]


checks = 0
bodies = {n: function(drm, n) for n in ('submit', 'render_parameters', 'compute_parameters')}
m = re.search(r'impl Mapping \{.*?\n\}', drm, re.S)
bodies['Mapping::cover'] = m.group()
for name, body in bodies.items():
    # Sync-entry slicing (try_into of fixed 16-byte chunks) can't fail and keeps its plain mapping.
    stripped = re.sub(r'\.try_into\(\)\.map_err\(\|_\| EINVAL\)', '', body)
    for bare in ('Err(EINVAL)', 'ok_or(EINVAL)', 'Err(ENOTSUPP)', 'ok_or(ENOTSUPP)'):
        assert bare not in stripped, (name, bare)
checks += 1
assert sum(b.count('reject(format_args!(') for b in bodies.values()) >= 25
assert sum(b.count('unsupported(format_args!(') for b in bodies.values()) == 3
checks += 1
assert 'const REJECT_LOG: u64 = 32;' in drm
# Every rejection is counted in dep_stats (review R2), appended last.
timeline = (root / 'drivers/gpu/drm/asahi/g16_timeline.rs').read_text()
assert re.search(r'tvb_refused_memory \{\}\\nsubmit_rejected \{\}(?:"|\\n)', timeline)
assert re.search(r'TVB_REFUSED_MEMORY\.load\(Relaxed\),\s*SUBMIT_REJECTED\.load\(Relaxed\)\s*[,)]', timeline)
assert 'let n = crate::g16_timeline::SUBMIT_REJECTED.fetch_add(1, Ordering::Relaxed);' in drm
# Docs stay with their items (review R1).
assert re.search(r"/// The calling process's name, for hang reports\.\nfn current_comm\(\)", drm)
log = function(drm, 'log_rejection')
assert re.search(r'if n < REJECT_LOG \{.*?pr_warn!.*?\} else if n == REJECT_LOG \{\s*pr_warn!\("G16: further SUBMIT rejections are not logged', log, re.S)
checks += 1
# The operator's log filter (v2) treats G16 lines with these words as faults; a rejection is the
# client's error, not a GPU fault, so no reason may contain them ("hang" matches inside words too).
reasons = re.findall(r'(?:reject|unsupported)\(format_args!\(\s*"([^"]*)"', drm)
reasons.append('G16: SUBMIT rejected with {}: {} ({} pid {})')
for text in reasons:
    assert not re.search(r'fault|timeout|timed out|hang|\bEIO\b|halt|reset', text, re.I), text
checks += 1
print(f'test-submit-reasons: {checks} checks passed ({len(reasons) - 1} reasons)')
