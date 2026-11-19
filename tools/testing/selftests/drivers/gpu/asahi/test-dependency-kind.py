#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Host checks of asahi_m4.dependency_kind: kind 0 by default, range-checked at
probe, and applied only to render-to-render items (a render's slot 0)."""
from pathlib import Path
import re

root = next(p for p in Path(__file__).resolve().parents
            if (p / 'drivers/gpu/drm/asahi/g16_runtime.rs').exists())
driver = root / 'drivers/gpu/drm/asahi'
runtime = ''.join(p.read_text() for p in sorted(driver.glob('g16_runtime*.rs')))
m4 = (driver / 'asahi_m4.rs').read_text()

# Render-to-render items default to kind 0, as production boots pass.
assert re.search(r'\n        dependency_kind: u32 \{\s*default: 0,', m4)
assert 'module_parameters::dependency_kind.value() > 1' in m4
# Renders: slot 0 (render producer) takes dependency_kind; slot 1 (compute) stays kind 1.
assert "dep.map(|d| (if slot == 0 { render_kind } else { 1 }, d))" in runtime
print('test-dependency-kind: ok')
