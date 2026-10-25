#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Host source checks: G16 TVB growth, which answers a firmware request from
the fence-signalling path, never enters reclaim and never takes the atomic
reserves. Its data allocations use GFP_NOWAIT | __GFP_NOWARN, and a failure
halves the request down to one block and
then refuses, so the render finishes under its memory limit. Page tables
for the new range are still allocated by map_pages (phase 2).
"""
from pathlib import Path
import re

here = Path(__file__).resolve()
root = next(p for p in here.parents if (p / 'drivers/gpu/drm/asahi/g16_tvb.rs').exists())
driver = root / 'drivers/gpu/drm/asahi'
tvb = (driver / 'g16_tvb.rs').read_text()
vm = (driver / 'g16_vm.rs').read_text()


def function(source, name):
    m = re.search(r'\n(\s*)(?:pub\((?:crate|super)\) )?fn ' + name + r'\b', source)
    assert m, name
    indent = m.group(1).lstrip('\n')
    end = re.search(r'\n' + indent + r'}\n', source[m.end():])
    return source[m.start():m.end() + end.end()]


grow = function(tvb, 'grow')
blocks = function(vm, 'alloc_tvb_blocks')
checks = 0
for name, body in (('grow', grow), ('alloc_tvb_blocks', blocks)):
    assert 'GFP_KERNEL' not in body, f'{name} may reclaim'
    checks += 1
assert 'let gfp = GFP_NOWAIT | __GFP_NOWARN;' in grow and 'GFP_ATOMIC' not in grow
assert 'client.alloc_tvb_blocks((new - old) as usize, gfp)?' in grow
assert re.search(r'extend_with\(\(new - old\) as usize \* 8, 0u8, gfp\)', grow)
checks += 1
# The fallback: halve, then refuse at one block.
assert re.search(r'if add == 1 \{\s*self\.refusal = Some\(.*?\);\s*return Ok\(false\);\s*\}\s*new = old \+ add / 2;', grow, re.S)
checks += 1
# Every allocation in alloc_tvb_blocks takes the caller's flags; none fails the context.
assert 'Page::alloc_page(gfp | __GFP_ZERO)' in blocks and 'RBTreeNode::new(va, page, gfp)' in blocks
assert 'pages.reserve(count * 8, gfp)' in blocks and 'push_within_capacity(node)' in blocks
for failure in re.findall(r'Err\(_\) => return ([^,]+),', blocks):
    assert failure == 'Ok(None)', failure
checks += 1
print(f'test-tvb-growth-gfp: {checks} checks passed')
