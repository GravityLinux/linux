#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Check the downstream (G16) values in include/uapi/drm/asahi_drm.h.

Downstream feature bits and render flags are single bits allocated from bit 30
downwards, after the "Downstream (G16) extensions" comment in each enum.
Upstream's values stay below bit 24. The G16 render path accepts exactly the
expected render flags; the old downstream bits (feature 1, render flag 3) are
not aliases. With MESA_UAPI_HEADER set, Mesa's copy must be byte-identical.
"""
from pathlib import Path
import os
import re

root = next(p for p in Path(__file__).resolve().parents
            if (p / 'include/uapi/drm/asahi_drm.h').exists())
header = (root / 'include/uapi/drm/asahi_drm.h').read_text()


def values(enum):
    body = re.search(r'^enum ' + enum + r' \{\n(.*?)^\};', header, re.M | re.S).group(1)
    marker = body.index('Downstream (G16) extensions')
    found = {}
    for m in re.finditer(r'(DRM_ASAHI_\w+) = \(1UL?\)? ?<< (\d+)\)?,', body):
        found[m.group(1)] = (int(m.group(2)), m.start() > marker)
    return found


features = values('drm_asahi_feature')
flags = values('drm_asahi_render_flags')
for name, table in (('feature', features), ('render flag', flags)):
    bits = [bit for bit, _ in table.values()]
    assert len(bits) == len(set(bits)), f'duplicate {name} bit: {table}'
    for value, (bit, downstream) in table.items():
        if downstream:
            assert 24 <= bit <= 30, f'{value} = bit {bit} is outside the downstream range'
        else:
            assert bit < 24, f'upstream {value} = bit {bit} reaches the downstream range'

assert {k: v[0] for k, v in features.items() if v[1]} == {
    'DRM_ASAHI_FEATURE_FEW_PRIMITIVES': 30, 'DRM_ASAHI_FEATURE_FRAGMENT_BARRIER': 29}
assert {k: v[0] for k, v in flags.items() if v[1]} == {
    'DRM_ASAHI_RENDER_FEW_PRIMITIVES': 30, 'DRM_ASAHI_RENDER_VDM_BARRIER_FRAGMENT': 29}

# The render path's accepted-flag mask, by name, resolved through the header.
driver = ''.join(p.read_text() for p in sorted((root / 'drivers/gpu/drm/asahi').glob('g16_drm*.rs')))
aliases = dict(re.findall(r'drm_asahi_render_flags_(DRM_ASAHI_RENDER_\w+) as (\w+)', driver))
mask = re.search(r'c\.flags & !\(([^)]*)\)', driver).group(1)
accepted = {flags[{v: k for k, v in aliases.items()}[n.strip()]][0] for n in mask.split('|')}
assert accepted == {1, 18, 29, 30}, accepted
assert 3 not in accepted

mesa = os.environ.get('MESA_UAPI_HEADER')
if mesa:
    assert Path(mesa).read_bytes() == (root / 'include/uapi/drm/asahi_drm.h').read_bytes(), \
        f'{mesa} differs from the kernel header'
print('PASS: downstream asahi UAPI bits' + (' (Mesa header identical)' if mesa else ''))
