#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Host tests of the G16 deferred-unmap bookkeeping (g16_deferred.rs).

Compiles the production module with deferred-unmap.rs. rustc must be on PATH;
no device or kernel build is needed.
"""
from pathlib import Path
import os
import subprocess
import tempfile

here = Path(__file__).resolve()
root = next(p for p in here.parents if (p / 'drivers/gpu/drm/asahi/g16_deferred.rs').exists())
source = f'''#![allow(dead_code)]
#[path = "{root / 'drivers/gpu/drm/asahi/g16_deferred.rs'}"] mod g16_deferred;
''' + here.with_name('deferred-unmap.rs').read_text()

with tempfile.TemporaryDirectory(prefix='asahi-deferred-') as temp:
    temp = Path(temp)
    (temp / 'test.rs').write_text(source)
    subprocess.run([os.environ.get('RUSTC', 'rustc'), '--edition=2021', '--test', '-Dwarnings',
                    str(temp / 'test.rs'), '-o', str(temp / 'test')], check=True)
    subprocess.run([str(temp / 'test')], check=True)
